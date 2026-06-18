//! consign — accurate fleet push-debt enumerator
//!
//! Walks a directory of git repos and classifies their push-debt
//! into named buckets: clean, ahead, no-upstream, no-remote, diverged.

#![deny(unsafe_code)]

pub mod drain;
pub mod verify;

pub use verify::{OverallVerdict, VerifyReceipt};

use std::path::{Path, PathBuf};
use std::process::Command;

use serde::{Deserialize, Serialize};

/// Classification of a repo's push-debt.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "class", rename_all = "kebab-case")]
pub enum DebtClass {
    /// HEAD is on a remote, nothing ahead.
    Clean,
    /// Upstream set, n commits ahead.
    Ahead { n: u32 },
    /// Has a remote but no upstream branch; n commits on no remote.
    NoUpstream { n: u32 },
    /// No remote configured.
    NoRemote,
    /// Upstream set, ahead a and behind b > 0.
    Diverged { a: u32, b: u32 },
}

impl DebtClass {
    pub fn name(&self) -> &'static str {
        match self {
            DebtClass::Clean => "clean",
            DebtClass::Ahead { .. } => "ahead",
            DebtClass::NoUpstream { .. } => "no-upstream",
            DebtClass::NoRemote => "no-remote",
            DebtClass::Diverged { .. } => "diverged",
        }
    }
}

/// Push-debt information for a single repository.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RepoDebt {
    /// Absolute path to the repo root.
    pub path: PathBuf,
    /// Current branch name (or detached HEAD description).
    pub branch: String,
    /// Best-guess default branch (main/master/trunk, or first branch found).
    pub default_branch: Option<String>,
    /// Remote URL (first remote), if any.
    pub remote_url: Option<String>,
    /// Push-debt classification.
    #[serde(flatten)]
    pub class: DebtClass,
    /// True if `branch` matches the guessed `default_branch`.
    pub branch_is_default: bool,
}

/// Error type for survey operations.
#[derive(Debug)]
pub enum SurveyError {
    UnreadableRoot(PathBuf, std::io::Error),
    Git(String),
}

impl std::fmt::Display for SurveyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SurveyError::UnreadableRoot(p, e) => {
                write!(f, "cannot read root directory {}: {}", p.display(), e)
            }
            SurveyError::Git(msg) => write!(f, "git error: {}", msg),
        }
    }
}

impl std::error::Error for SurveyError {}

/// Run `git` inside `repo` and return stdout as a String (trimmed).
/// Returns Err if git exits non-zero.
fn git_output(repo: &Path, args: &[&str]) -> Result<String, String> {
    let out = Command::new("git")
        .args(["-C", &repo.to_string_lossy()])
        .args(args)
        .output()
        .map_err(|e| format!("failed to run git: {}", e))?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
    }
}

/// Like git_output but also succeeds on non-zero (returns empty string).
fn git_output_tolerant(repo: &Path, args: &[&str]) -> String {
    Command::new("git")
        .args(["-C", &repo.to_string_lossy()])
        .args(args)
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default()
}

/// True if `dir` is the root of a git repo (has a `.git` entry).
pub fn is_git_repo(dir: &Path) -> bool {
    dir.join(".git").exists()
}

/// Classify a single git repo at `repo`.
pub fn classify_repo(repo: &Path) -> Result<RepoDebt, SurveyError> {
    // Current branch
    let branch = git_output(repo, &["rev-parse", "--abbrev-ref", "HEAD"])
        .unwrap_or_else(|_| "HEAD".to_string());

    // Remotes
    let remotes_raw = git_output_tolerant(repo, &["remote"]);
    let remotes: Vec<&str> = remotes_raw
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect();

    let remote_url = if let Some(r) = remotes.first() {
        git_output(repo, &["remote", "get-url", r]).ok()
    } else {
        None
    };

    // Default branch guess
    let default_branch = guess_default_branch(repo);

    let branch_is_default = default_branch
        .as_deref()
        .map(|d| d == branch)
        .unwrap_or(false);

    // No remote at all
    if remotes.is_empty() {
        return Ok(RepoDebt {
            path: repo.to_path_buf(),
            branch,
            default_branch,
            remote_url,
            class: DebtClass::NoRemote,
            branch_is_default,
        });
    }

    // Try to resolve upstream
    let upstream = git_output(repo, &["rev-parse", "--abbrev-ref", "@{u}"]);

    let class = match upstream {
        Ok(_upstream_ref) => {
            // Upstream exists — check ahead/behind
            let ahead_str = git_output_tolerant(repo, &["rev-list", "--count", "@{u}..HEAD"]);
            let behind_str = git_output_tolerant(repo, &["rev-list", "--count", "HEAD..@{u}"]);
            let ahead: u32 = ahead_str.parse().unwrap_or(0);
            let behind: u32 = behind_str.parse().unwrap_or(0);
            match (ahead, behind) {
                (0, 0) => DebtClass::Clean,
                (a, 0) => DebtClass::Ahead { n: a },
                (0, _) => DebtClass::Clean, // behind but not ahead → clean push-debt
                (a, b) => DebtClass::Diverged { a, b },
            }
        }
        Err(_) => {
            // Has a remote but no upstream tracking branch set
            // Count commits not on any remote
            let count_str = git_output_tolerant(
                repo,
                &[
                    "log",
                    "--branches",
                    "--not",
                    "--remotes",
                    "--oneline",
                    "--",
                ],
            );
            let n = count_str.lines().filter(|l| !l.trim().is_empty()).count() as u32;
            DebtClass::NoUpstream { n }
        }
    };

    Ok(RepoDebt {
        path: repo.to_path_buf(),
        branch,
        default_branch,
        remote_url,
        class,
        branch_is_default,
    })
}

/// Guess the default branch for a repo (main > master > trunk > first branch).
fn guess_default_branch(repo: &Path) -> Option<String> {
    // Try HEAD on origin
    if let Ok(out) = git_output(repo, &["symbolic-ref", "refs/remotes/origin/HEAD"]) {
        // refs/remotes/origin/main -> main
        if let Some(branch) = out.strip_prefix("refs/remotes/origin/") {
            return Some(branch.to_string());
        }
    }
    // Fallback: known names
    for candidate in &["main", "master", "trunk"] {
        let check = git_output(repo, &["rev-parse", "--verify", candidate]);
        if check.is_ok() {
            return Some(candidate.to_string());
        }
    }
    // Last resort: first branch
    git_output(
        repo,
        &["branch", "--format=%(refname:short)", "--sort=-committerdate"],
    )
    .ok()
    .and_then(|s| s.lines().next().map(|l| l.trim().to_string()))
    .filter(|s| !s.is_empty())
}

/// Survey all immediate child directories of each `root` that are git repos.
/// Non-git dirs are skipped silently. An unreadable root returns a structured error.
pub fn survey(roots: &[PathBuf]) -> Result<Vec<RepoDebt>, SurveyError> {
    let mut results = Vec::new();
    for root in roots {
        let entries = std::fs::read_dir(root)
            .map_err(|e| SurveyError::UnreadableRoot(root.clone(), e))?;
        let mut dirs: Vec<PathBuf> = entries
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| p.is_dir() && is_git_repo(p))
            .collect();
        dirs.sort();
        for dir in dirs {
            match classify_repo(&dir) {
                Ok(debt) => results.push(debt),
                Err(e) => {
                    // Log to stderr but continue
                    eprintln!("consign: skipping {}: {}", dir.display(), e);
                }
            }
        }
    }
    Ok(results)
}

/// Render a `&[RepoDebt]` as an aligned table with a totals footer.
pub fn render_table(debts: &[RepoDebt]) -> String {
    use std::fmt::Write;

    let mut out = String::new();
    // Column widths
    let path_w = debts
        .iter()
        .map(|d| d.path.display().to_string().len())
        .max()
        .unwrap_or(4)
        .max(4);
    let branch_w = debts
        .iter()
        .map(|d| d.branch.len())
        .max()
        .unwrap_or(6)
        .max(6);
    let class_w = debts
        .iter()
        .map(|d| d.class.name().len())
        .max()
        .unwrap_or(5)
        .max(5);

    // Header
    let _ = writeln!(
        out,
        "{:<path_w$}  {:<branch_w$}  {:<class_w$}  detail",
        "PATH",
        "BRANCH",
        "CLASS",
        path_w = path_w,
        branch_w = branch_w,
        class_w = class_w,
    );
    let sep = format!(
        "{}  {}  {}  {}",
        "-".repeat(path_w),
        "-".repeat(branch_w),
        "-".repeat(class_w),
        "------"
    );
    let _ = writeln!(out, "{}", sep);

    // Rows
    for d in debts {
        let detail = match &d.class {
            DebtClass::Clean => String::new(),
            DebtClass::Ahead { n } => format!("+{}", n),
            DebtClass::NoUpstream { n } => format!("~{} no-remote-commits", n),
            DebtClass::NoRemote => String::new(),
            DebtClass::Diverged { a, b } => format!("+{} -{}",  a, b),
        };
        let _ = writeln!(
            out,
            "{:<path_w$}  {:<branch_w$}  {:<class_w$}  {}",
            d.path.display(),
            d.branch,
            d.class.name(),
            detail,
            path_w = path_w,
            branch_w = branch_w,
            class_w = class_w,
        );
    }

    // Totals
    let _ = writeln!(out, "{}", sep);
    let total = debts.len();
    let n_clean = debts.iter().filter(|d| d.class == DebtClass::Clean).count();
    let n_ahead = debts
        .iter()
        .filter(|d| matches!(d.class, DebtClass::Ahead { .. }))
        .count();
    let n_no_upstream = debts
        .iter()
        .filter(|d| matches!(d.class, DebtClass::NoUpstream { .. }))
        .count();
    let n_no_remote = debts
        .iter()
        .filter(|d| d.class == DebtClass::NoRemote)
        .count();
    let n_diverged = debts
        .iter()
        .filter(|d| matches!(d.class, DebtClass::Diverged { .. }))
        .count();
    let _ = writeln!(
        out,
        "{} repos: {} clean, {} ahead, {} no-upstream, {} no-remote, {} diverged",
        total, n_clean, n_ahead, n_no_upstream, n_no_remote, n_diverged
    );
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;
    use tempfile::TempDir;

    fn git(dir: &Path, args: &[&str]) {
        let status = Command::new("git")
            .args(["-C", &dir.to_string_lossy()])
            .args(args)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_AUTHOR_NAME", "Test")
            .env("GIT_AUTHOR_EMAIL", "test@example.com")
            .env("GIT_COMMITTER_NAME", "Test")
            .env("GIT_COMMITTER_EMAIL", "test@example.com")
            .status()
            .expect("git failed");
        assert!(status.success(), "git {:?} failed", args);
    }

    fn make_repo(parent: &Path, name: &str) -> PathBuf {
        let dir = parent.join(name);
        std::fs::create_dir(&dir).unwrap();
        git(&dir, &["init", "-b", "main"]);
        git(&dir, &["config", "user.email", "test@example.com"]);
        git(&dir, &["config", "user.name", "Test"]);
        // Initial commit
        std::fs::write(dir.join("README.md"), "hi").unwrap();
        git(&dir, &["add", "."]);
        git(&dir, &["commit", "-m", "init"]);
        dir
    }

    fn add_commit(dir: &Path, file: &str) {
        std::fs::write(dir.join(file), file).unwrap();
        git(dir, &["add", "."]);
        git(dir, &["commit", "-m", &format!("add {}", file)]);
    }

    /// AC3 partial: no-remote classification
    #[test]
    fn test_no_remote() {
        let tmp = TempDir::new().unwrap();
        let repo = make_repo(tmp.path(), "repo");
        let debt = classify_repo(&repo).unwrap();
        assert_eq!(debt.class, DebtClass::NoRemote, "no remote => NoRemote");
    }

    /// AC3: ahead classification
    #[test]
    fn test_ahead() {
        let tmp = TempDir::new().unwrap();
        // "origin" bare repo
        let bare = tmp.path().join("origin.git");
        std::fs::create_dir(&bare).unwrap();
        git(&bare, &["init", "--bare", "-b", "main"]);

        let local = make_repo(tmp.path(), "local");
        git(&local, &["remote", "add", "origin", &bare.to_string_lossy()]);
        git(&local, &["push", "-u", "origin", "main"]);

        // Now add 2 commits ahead
        add_commit(&local, "a.txt");
        add_commit(&local, "b.txt");

        let debt = classify_repo(&local).unwrap();
        assert_eq!(
            debt.class,
            DebtClass::Ahead { n: 2 },
            "2 commits ahead => Ahead(2)"
        );
    }

    /// AC3: diverged classification
    #[test]
    fn test_diverged() {
        let tmp = TempDir::new().unwrap();
        let bare = tmp.path().join("origin.git");
        std::fs::create_dir(&bare).unwrap();
        git(&bare, &["init", "--bare", "-b", "main"]);

        let local = make_repo(tmp.path(), "local");
        git(&local, &["remote", "add", "origin", &bare.to_string_lossy()]);
        git(&local, &["push", "-u", "origin", "main"]);

        // Clone a second local to add a remote commit
        let local2 = tmp.path().join("local2");
        git(tmp.path(), &["clone", &bare.to_string_lossy(), "local2"]);
        git(&local2, &["config", "user.email", "test@example.com"]);
        git(&local2, &["config", "user.name", "Test"]);
        add_commit(&local2, "remote_file.txt");
        git(&local2, &["push", "origin", "main"]);

        // Now add a local commit on local (diverged)
        add_commit(&local, "local_file.txt");
        // Fetch to see the remote commit
        git(&local, &["fetch", "origin"]);

        let debt = classify_repo(&local).unwrap();
        assert!(
            matches!(debt.class, DebtClass::Diverged { a: 1, b: 1 }),
            "diverged: got {:?}",
            debt.class
        );
    }

    /// AC2: no-upstream classification (has remote, no tracking branch)
    #[test]
    fn test_no_upstream() {
        let tmp = TempDir::new().unwrap();
        let bare = tmp.path().join("origin.git");
        std::fs::create_dir(&bare).unwrap();
        git(&bare, &["init", "--bare", "-b", "main"]);

        let local = make_repo(tmp.path(), "local");
        git(&local, &["remote", "add", "origin", &bare.to_string_lossy()]);
        // Add a commit but DO NOT push (no tracking branch set)
        add_commit(&local, "unpushed.txt");

        let debt = classify_repo(&local).unwrap();
        assert!(
            matches!(debt.class, DebtClass::NoUpstream { .. }),
            "no upstream => NoUpstream; got {:?}",
            debt.class
        );
        if let DebtClass::NoUpstream { n } = debt.class {
            assert!(n > 0, "should have unpushed commits, got n=0");
        }
    }

    /// AC4: table totals sum to repo count
    #[test]
    fn test_table_totals() {
        let tmp = TempDir::new().unwrap();
        let r1 = make_repo(tmp.path(), "r1");
        let r2 = make_repo(tmp.path(), "r2");
        let debts: Vec<RepoDebt> = vec![
            classify_repo(&r1).unwrap(),
            classify_repo(&r2).unwrap(),
        ];
        let table = render_table(&debts);
        // Footer should contain "2 repos"
        assert!(table.contains("2 repos"), "table footer: {}", table);
    }

    /// AC5: non-git dirs skipped, survey returns only git repos
    #[test]
    fn test_survey_skips_non_git() {
        let tmp = TempDir::new().unwrap();
        make_repo(tmp.path(), "git_repo");
        std::fs::create_dir(tmp.path().join("not_a_repo")).unwrap();
        let results = survey(&[tmp.path().to_path_buf()]).unwrap();
        assert_eq!(results.len(), 1);
        assert!(results[0].path.ends_with("git_repo"));
    }

    /// AC5: unreadable root returns structured error
    #[test]
    fn test_survey_unreadable_root() {
        let bad = PathBuf::from("/nonexistent/path/that/does/not/exist");
        let result = survey(&[bad]);
        assert!(result.is_err(), "unreadable root should return Err");
        match result {
            Err(SurveyError::UnreadableRoot(_, _)) => {}
            other => panic!("expected UnreadableRoot, got {:?}", other),
        }
    }
}
