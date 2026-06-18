//! consign policy — default-deny push-eligibility gate.
//!
//! Classifies each `RepoDebt` into one of three verdicts:
//!   - `auto-ok`      — safe to push automatically
//!   - `private-hold` — contains sensitive material; surface only, never push
//!   - `manual-only`  — diverged, non-default branch, or unknown state; needs human
//!
//! ## Configuration override
//!
//! An optional `~/.config/consign/policy.toml` (or the path in
//! `CONSIGN_POLICY_CONFIG`) can supply extra hold-name globs and deny-path globs.
//! Absent or unreadable config is silently ignored (built-in defaults apply).
//!
//! ## Design rule
//!
//! **Ambiguous ⇒ `manual-only`, never `auto-ok`.** The classifier only returns
//! `auto-ok` for states it explicitly affirms; everything else falls through to
//! `manual-only`.

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::{DebtClass, RepoDebt};

// ---------------------------------------------------------------------------
// PolicyClass — the three verdicts
// ---------------------------------------------------------------------------

/// The push-eligibility verdict for a single repo.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PolicyClass {
    /// Drain / publish may act automatically.
    AutoOk,
    /// Sensitive content detected; surface only, never auto-push.
    PrivateHold,
    /// Needs human review (diverged, wrong branch, unknown state).
    ManualOnly,
}

impl PolicyClass {
    /// Kebab-case name for display / JSON.
    pub fn as_str(&self) -> &'static str {
        match self {
            PolicyClass::AutoOk => "auto-ok",
            PolicyClass::PrivateHold => "private-hold",
            PolicyClass::ManualOnly => "manual-only",
        }
    }
}

impl std::fmt::Display for PolicyClass {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

// ---------------------------------------------------------------------------
// PolicyCfg — configurable policy parameters
// ---------------------------------------------------------------------------

/// Policy configuration.  Built-in defaults apply when absent.
///
/// Loaded from `~/.config/consign/policy.toml` or `CONSIGN_POLICY_CONFIG`.
/// Missing / unreadable config is NOT an error.
#[derive(Debug, Clone, Default)]
pub struct PolicyCfg {
    /// Additional name globs (fnmatch-style `*`) that force `private-hold`.
    /// Built-in: `*-private`, `autobuilder*`.
    pub extra_hold_globs: Vec<String>,
    /// Additional repo-root path substrings that force `private-hold`.
    pub extra_hold_paths: Vec<String>,
}

impl PolicyCfg {
    /// Load from `~/.config/consign/policy.toml` (or `$CONSIGN_POLICY_CONFIG`).
    /// Returns the built-in defaults on any error (silent).
    pub fn load() -> Self {
        let path = std::env::var_os("CONSIGN_POLICY_CONFIG")
            .map(std::path::PathBuf::from)
            .or_else(|| {
                std::env::var_os("HOME").map(|h| {
                    std::path::PathBuf::from(h)
                        .join(".config")
                        .join("consign")
                        .join("policy.toml")
                })
            });

        if let Some(p) = path {
            if let Ok(s) = std::fs::read_to_string(&p) {
                // Minimal TOML parse: look for `extra_hold_globs = [...]` lines.
                // We avoid a full TOML dep and just do best-effort parsing.
                return Self::parse_simple_toml(&s);
            }
        }
        Self::default()
    }

    /// Best-effort parse of a simple TOML array for our two fields.
    fn parse_simple_toml(s: &str) -> Self {
        let mut cfg = Self::default();
        for line in s.lines() {
            let line = line.trim();
            if line.starts_with("extra_hold_globs") {
                cfg.extra_hold_globs = Self::extract_string_array(line);
            } else if line.starts_with("extra_hold_paths") {
                cfg.extra_hold_paths = Self::extract_string_array(line);
            }
        }
        cfg
    }

    fn extract_string_array(line: &str) -> Vec<String> {
        // e.g. extra_hold_globs = ["foo*", "bar"]
        let after_eq = line.find('=').map(|i| &line[i + 1..]).unwrap_or("").trim();
        if !after_eq.starts_with('[') {
            return Vec::new();
        }
        let inner = after_eq.trim_start_matches('[').trim_end_matches(']');
        inner
            .split(',')
            .map(|s| s.trim().trim_matches('"').trim_matches('\'').to_string())
            .filter(|s| !s.is_empty())
            .collect()
    }
}

// ---------------------------------------------------------------------------
// Built-in hold patterns
// ---------------------------------------------------------------------------

/// Built-in name patterns that force `private-hold`.
const BUILTIN_HOLD_GLOBS: &[&str] = &["*-private", "autobuilder*"];

/// Secret-file patterns: if a tracked file in the repo matches one of these
/// globs, the repo is classified `private-hold`.
const SECRET_PATTERNS: &[&str] = &[
    ".env",
    "*.pem",
    "id_rsa",
    "id_ed25519",
    "id_ecdsa",
    "id_dsa",
    "*credential*",
    "*credentials*",
    "*secret*",
];

// ---------------------------------------------------------------------------
// Core classify function
// ---------------------------------------------------------------------------

/// Classify a single repo's push-eligibility.
///
/// Call signature is stable and library-exported so drain/publish can call it
/// directly without re-parsing CLI output.
pub fn classify(repo: &RepoDebt, cfg: &PolicyCfg) -> PolicyClass {
    // ── Step 1: private-hold — name / path checks ──────────────────────────

    let repo_name = repo
        .path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();

    // .consign-hold sentinel file
    if repo.path.join(".consign-hold").exists() {
        return PolicyClass::PrivateHold;
    }

    // Name matches built-in hold globs
    for glob in BUILTIN_HOLD_GLOBS {
        if glob_match(glob, &repo_name) {
            return PolicyClass::PrivateHold;
        }
    }

    // Name matches user-supplied hold globs
    for glob in &cfg.extra_hold_globs {
        if glob_match(glob, &repo_name) {
            return PolicyClass::PrivateHold;
        }
    }

    // Remote URL contains hold path substring
    if let Some(url) = &repo.remote_url {
        for pat in &cfg.extra_hold_paths {
            if url.contains(pat.as_str()) {
                return PolicyClass::PrivateHold;
            }
        }
    }

    // ── Step 2: private-hold — secret file detection ───────────────────────

    if has_tracked_secret(&repo.path) {
        return PolicyClass::PrivateHold;
    }

    // ── Step 3: manual-only — branch / state checks ────────────────────────

    // Non-default branch (worktree / feature branch)
    if !repo.branch_is_default {
        return PolicyClass::ManualOnly;
    }

    // Detached HEAD
    if repo.branch == "HEAD" {
        return PolicyClass::ManualOnly;
    }

    // Diverged
    if matches!(repo.class, DebtClass::Diverged { .. }) {
        return PolicyClass::ManualOnly;
    }

    // ── Step 4: auto-ok — only explicitly affirmed states ──────────────────

    match &repo.class {
        DebtClass::Ahead { .. } | DebtClass::NoUpstream { .. } | DebtClass::NoRemote => {
            PolicyClass::AutoOk
        }
        // Clean = nothing to do; treat as auto-ok (drain will no-op it)
        DebtClass::Clean => PolicyClass::AutoOk,
        // Diverged already caught above; catch-all for any future variants.
        _ => PolicyClass::ManualOnly,
    }
}

// ---------------------------------------------------------------------------
// Secret detection
// ---------------------------------------------------------------------------

/// Returns true if `git ls-files` in `repo` lists any file matching a secret
/// pattern. Failures (not a repo, git not found) return false conservatively.
fn has_tracked_secret(repo: &Path) -> bool {
    use std::process::Command;

    let out = Command::new("git")
        .args(["-C", &repo.to_string_lossy()])
        .args(["ls-files", "--full-name"])
        .output();

    let stdout = match out {
        Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout).into_owned(),
        _ => return false,
    };

    for line in stdout.lines() {
        let filename = line.trim();
        // Only check the basename for pattern matching
        let basename = std::path::Path::new(filename)
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| filename.to_string());

        for pat in SECRET_PATTERNS {
            if glob_match(pat, &basename) || glob_match(pat, filename) {
                return true;
            }
        }
    }
    false
}

// ---------------------------------------------------------------------------
// Minimal glob matching  (* matches any sequence, no ? or [] needed)
// ---------------------------------------------------------------------------

/// Match `pattern` against `name`. Supports `*` wildcard only.
/// Case-sensitive.
pub fn glob_match(pattern: &str, name: &str) -> bool {
    // Split on `*` and match greedily left-to-right.
    let parts: Vec<&str> = pattern.split('*').collect();
    if parts.len() == 1 {
        return pattern == name;
    }

    let mut remaining = name;

    for (i, part) in parts.iter().enumerate() {
        if part.is_empty() {
            // A leading/trailing/consecutive * — skip
            continue;
        }
        if i == 0 {
            // First part must be a prefix
            if !remaining.starts_with(part) {
                return false;
            }
            remaining = &remaining[part.len()..];
        } else if i == parts.len() - 1 {
            // Last part must be a suffix
            if !remaining.ends_with(part) {
                return false;
            }
        } else {
            // Middle part: find first occurrence
            match remaining.find(part) {
                Some(pos) => {
                    remaining = &remaining[pos + part.len()..];
                }
                None => return false,
            }
        }
    }
    true
}

// ---------------------------------------------------------------------------
// SurveyWithPolicy — augmented output type
// ---------------------------------------------------------------------------

/// A `RepoDebt` augmented with a `PolicyClass`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PolicyEntry {
    #[serde(flatten)]
    pub debt: RepoDebt,
    /// The policy verdict for this repo.
    pub policy: PolicyClass,
}

/// Run survey over `roots` and attach a policy verdict to each repo.
pub fn survey_with_policy(
    roots: &[std::path::PathBuf],
    cfg: &PolicyCfg,
) -> Result<Vec<PolicyEntry>, crate::SurveyError> {
    let debts = crate::survey(roots)?;
    let entries = debts
        .into_iter()
        .map(|debt| {
            let policy = classify(&debt, cfg);
            PolicyEntry { debt, policy }
        })
        .collect();
    Ok(entries)
}

/// Render a `&[PolicyEntry]` as a human-readable table.
pub fn render_policy_table(entries: &[PolicyEntry]) -> String {
    use std::fmt::Write;
    let mut out = String::new();

    let path_w = entries
        .iter()
        .map(|e| e.debt.path.display().to_string().len())
        .max()
        .unwrap_or(4)
        .max(4);
    let branch_w = entries
        .iter()
        .map(|e| e.debt.branch.len())
        .max()
        .unwrap_or(6)
        .max(6);
    let class_w = entries
        .iter()
        .map(|e| e.debt.class.name().len())
        .max()
        .unwrap_or(5)
        .max(5);
    let policy_w = "manual-only".len(); // widest verdict

    let _ = writeln!(
        out,
        "{:<pw$}  {:<bw$}  {:<cw$}  {:<vw$}",
        "PATH",
        "BRANCH",
        "CLASS",
        "POLICY",
        pw = path_w,
        bw = branch_w,
        cw = class_w,
        vw = policy_w,
    );
    let sep = format!(
        "{}  {}  {}  {}",
        "-".repeat(path_w),
        "-".repeat(branch_w),
        "-".repeat(class_w),
        "-".repeat(policy_w),
    );
    let _ = writeln!(out, "{}", sep);

    for e in entries {
        let _ = writeln!(
            out,
            "{:<pw$}  {:<bw$}  {:<cw$}  {:<vw$}",
            e.debt.path.display(),
            e.debt.branch,
            e.debt.class.name(),
            e.policy.as_str(),
            pw = path_w,
            bw = branch_w,
            cw = class_w,
            vw = policy_w,
        );
    }

    let _ = writeln!(out, "{}", sep);
    let n_ok = entries
        .iter()
        .filter(|e| e.policy == PolicyClass::AutoOk)
        .count();
    let n_hold = entries
        .iter()
        .filter(|e| e.policy == PolicyClass::PrivateHold)
        .count();
    let n_manual = entries
        .iter()
        .filter(|e| e.policy == PolicyClass::ManualOnly)
        .count();
    let _ = writeln!(
        out,
        "{} repos: {} auto-ok, {} private-hold, {} manual-only",
        entries.len(),
        n_ok,
        n_hold,
        n_manual,
    );
    out
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{DebtClass, RepoDebt};
    use std::path::PathBuf;
    use std::process::Command;
    use tempfile::TempDir;

    fn cfg() -> PolicyCfg {
        PolicyCfg::default()
    }

    fn make_debt(
        path: PathBuf,
        branch: &str,
        class: DebtClass,
        branch_is_default: bool,
        remote_url: Option<String>,
    ) -> RepoDebt {
        RepoDebt {
            path,
            branch: branch.to_string(),
            default_branch: Some("main".to_string()),
            remote_url,
            class,
            branch_is_default,
        }
    }

    fn git(dir: &std::path::Path, args: &[&str]) {
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

    fn make_repo(parent: &std::path::Path, name: &str) -> PathBuf {
        let dir = parent.join(name);
        std::fs::create_dir(&dir).unwrap();
        git(&dir, &["init", "-b", "main"]);
        git(&dir, &["config", "user.email", "test@example.com"]);
        git(&dir, &["config", "user.name", "Test"]);
        std::fs::write(dir.join("README.md"), "hi").unwrap();
        git(&dir, &["add", "."]);
        git(&dir, &["commit", "-m", "init"]);
        dir
    }

    // -----------------------------------------------------------------------
    // glob_match unit tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_glob_exact() {
        assert!(glob_match("foo", "foo"));
        assert!(!glob_match("foo", "bar"));
    }

    #[test]
    fn test_glob_suffix_star() {
        assert!(glob_match("foo*", "foobar"));
        assert!(glob_match("foo*", "foo"));
        assert!(!glob_match("foo*", "barfoo"));
    }

    #[test]
    fn test_glob_prefix_star() {
        assert!(glob_match("*-private", "autobuilder-private"));
        assert!(glob_match("*-private", "my-repo-private"));
        assert!(!glob_match("*-private", "autobuilder"));
    }

    #[test]
    fn test_glob_autobuilder() {
        assert!(glob_match("autobuilder*", "autobuilder"));
        assert!(glob_match("autobuilder*", "autobuilder-private"));
        assert!(!glob_match("autobuilder*", "not-autobuilder"));
    }

    // -----------------------------------------------------------------------
    // AC1: consign policy JSON output has a "policy" field
    // (tested via PolicyEntry serialisation)
    // -----------------------------------------------------------------------

    #[test]
    fn test_policy_entry_serialises_policy_field() {
        let tmp = TempDir::new().unwrap();
        let debt = make_debt(
            tmp.path().join("myrepo"),
            "main",
            DebtClass::Ahead { n: 1 },
            true,
            None,
        );
        let entry = PolicyEntry {
            debt,
            policy: PolicyClass::AutoOk,
        };
        let json = serde_json::to_string(&entry).unwrap();
        assert!(json.contains("\"policy\""), "JSON must have a 'policy' field");
        assert!(json.contains("\"auto-ok\""), "policy must be 'auto-ok'");
    }

    // -----------------------------------------------------------------------
    // AC2: *-private and autobuilder* names → private-hold even if ahead
    // -----------------------------------------------------------------------

    #[test]
    fn test_private_name_suffix_hold() {
        let tmp = TempDir::new().unwrap();
        let debt = make_debt(
            tmp.path().join("autobuilder-private"),
            "main",
            DebtClass::Ahead { n: 2 },
            true,
            None,
        );
        assert_eq!(classify(&debt, &cfg()), PolicyClass::PrivateHold);
    }

    #[test]
    fn test_autobuilder_prefix_hold() {
        let tmp = TempDir::new().unwrap();
        let debt = make_debt(
            tmp.path().join("autobuilder"),
            "main",
            DebtClass::Ahead { n: 1 },
            true,
            None,
        );
        assert_eq!(classify(&debt, &cfg()), PolicyClass::PrivateHold);
    }

    #[test]
    fn test_consign_hold_file() {
        let tmp = TempDir::new().unwrap();
        let repo_dir = tmp.path().join("myrepo");
        std::fs::create_dir(&repo_dir).unwrap();
        // Create .consign-hold sentinel
        std::fs::write(repo_dir.join(".consign-hold"), "hold").unwrap();
        let debt = make_debt(repo_dir, "main", DebtClass::Ahead { n: 1 }, true, None);
        assert_eq!(classify(&debt, &cfg()), PolicyClass::PrivateHold);
    }

    // -----------------------------------------------------------------------
    // AC3: diverged → manual-only; non-default branch → manual-only
    // -----------------------------------------------------------------------

    #[test]
    fn test_diverged_is_manual_only() {
        let tmp = TempDir::new().unwrap();
        let debt = make_debt(
            tmp.path().join("mqo-narrative-compose"),
            "main",
            DebtClass::Diverged { a: 2, b: 2 },
            true,
            None,
        );
        assert_eq!(classify(&debt, &cfg()), PolicyClass::ManualOnly);
    }

    #[test]
    fn test_non_default_branch_is_manual_only() {
        let tmp = TempDir::new().unwrap();
        // Models homeward's autobuilder/homeward-found-geocode branch
        let debt = make_debt(
            tmp.path().join("homeward"),
            "autobuilder/homeward-found-geocode",
            DebtClass::Ahead { n: 2 },
            false, // branch_is_default = false
            None,
        );
        assert_eq!(classify(&debt, &cfg()), PolicyClass::ManualOnly);
    }

    // -----------------------------------------------------------------------
    // AC4: plain ahead/no-upstream/no-remote on default branch → auto-ok
    // -----------------------------------------------------------------------

    #[test]
    fn test_ahead_on_default_is_auto_ok() {
        let tmp = TempDir::new().unwrap();
        let debt = make_debt(
            tmp.path().join("myrepo"),
            "main",
            DebtClass::Ahead { n: 3 },
            true,
            None,
        );
        assert_eq!(classify(&debt, &cfg()), PolicyClass::AutoOk);
    }

    #[test]
    fn test_no_upstream_on_default_is_auto_ok() {
        let tmp = TempDir::new().unwrap();
        let debt = make_debt(
            tmp.path().join("myrepo"),
            "main",
            DebtClass::NoUpstream { n: 1 },
            true,
            None,
        );
        assert_eq!(classify(&debt, &cfg()), PolicyClass::AutoOk);
    }

    #[test]
    fn test_no_remote_on_default_is_auto_ok() {
        let tmp = TempDir::new().unwrap();
        let debt = make_debt(
            tmp.path().join("myrepo"),
            "main",
            DebtClass::NoRemote,
            true,
            None,
        );
        assert_eq!(classify(&debt, &cfg()), PolicyClass::AutoOk);
    }

    // -----------------------------------------------------------------------
    // AC5: tracked secret file → private-hold
    // -----------------------------------------------------------------------

    #[test]
    fn test_tracked_env_file_is_private_hold() {
        let tmp = TempDir::new().unwrap();
        let repo_dir = make_repo(tmp.path(), "myrepo");

        // Create and track a .env file
        std::fs::write(repo_dir.join(".env"), "SECRET=abc123").unwrap();
        git(&repo_dir, &["add", ".env"]);
        git(&repo_dir, &["commit", "-m", "add .env"]);

        let debt = crate::classify_repo(&repo_dir).unwrap();
        assert_eq!(
            classify(&debt, &cfg()),
            PolicyClass::PrivateHold,
            "repo with tracked .env must be private-hold"
        );
    }

    #[test]
    fn test_tracked_pem_file_is_private_hold() {
        let tmp = TempDir::new().unwrap();
        let repo_dir = make_repo(tmp.path(), "myrepo");

        std::fs::write(repo_dir.join("server.pem"), "FAKE CERT").unwrap();
        git(&repo_dir, &["add", "server.pem"]);
        git(&repo_dir, &["commit", "-m", "add pem"]);

        let debt = crate::classify_repo(&repo_dir).unwrap();
        assert_eq!(classify(&debt, &cfg()), PolicyClass::PrivateHold);
    }

    #[test]
    fn test_tracked_id_rsa_is_private_hold() {
        let tmp = TempDir::new().unwrap();
        let repo_dir = make_repo(tmp.path(), "myrepo");

        std::fs::write(repo_dir.join("id_rsa"), "FAKE KEY").unwrap();
        git(&repo_dir, &["add", "id_rsa"]);
        git(&repo_dir, &["commit", "-m", "add key"]);

        let debt = crate::classify_repo(&repo_dir).unwrap();
        assert_eq!(classify(&debt, &cfg()), PolicyClass::PrivateHold);
    }

    #[test]
    fn test_tracked_credential_file_is_private_hold() {
        let tmp = TempDir::new().unwrap();
        let repo_dir = make_repo(tmp.path(), "myrepo");

        std::fs::write(repo_dir.join("aws_credentials"), "KEY=secret").unwrap();
        git(&repo_dir, &["add", "aws_credentials"]);
        git(&repo_dir, &["commit", "-m", "add creds"]);

        let debt = crate::classify_repo(&repo_dir).unwrap();
        assert_eq!(classify(&debt, &cfg()), PolicyClass::PrivateHold);
    }

    // -----------------------------------------------------------------------
    // AC6: default-deny — clean repo not in affirmed states → manual-only
    // (The "clean" class is actually treated as auto-ok, but diverged and
    // non-default-branch always fall through to manual-only.)
    // -----------------------------------------------------------------------

    #[test]
    fn test_detached_head_is_manual_only() {
        let tmp = TempDir::new().unwrap();
        // branch = "HEAD" signals detached
        let debt = make_debt(
            tmp.path().join("myrepo"),
            "HEAD",
            DebtClass::Ahead { n: 1 },
            false, // detached HEAD: branch_is_default is false too
            None,
        );
        assert_eq!(classify(&debt, &cfg()), PolicyClass::ManualOnly);
    }

    // -----------------------------------------------------------------------
    // AC2 extra: user-supplied hold glob
    // -----------------------------------------------------------------------

    #[test]
    fn test_user_supplied_hold_glob() {
        let tmp = TempDir::new().unwrap();
        let debt = make_debt(
            tmp.path().join("secret-corp"),
            "main",
            DebtClass::Ahead { n: 1 },
            true,
            None,
        );
        let cfg = PolicyCfg {
            extra_hold_globs: vec!["secret-*".to_string()],
            extra_hold_paths: vec![],
        };
        assert_eq!(classify(&debt, &cfg), PolicyClass::PrivateHold);
    }

    // -----------------------------------------------------------------------
    // PolicyCfg::load — no error when config absent
    // -----------------------------------------------------------------------

    #[test]
    fn test_policy_cfg_load_no_config() {
        // Override env to point to a nonexistent file
        std::env::set_var("CONSIGN_POLICY_CONFIG", "/nonexistent/policy.toml");
        let cfg = PolicyCfg::load();
        assert!(cfg.extra_hold_globs.is_empty());
        assert!(cfg.extra_hold_paths.is_empty());
    }

    // -----------------------------------------------------------------------
    // render_policy_table smoke test
    // -----------------------------------------------------------------------

    #[test]
    fn test_render_policy_table_smoke() {
        let tmp = TempDir::new().unwrap();
        let entries = vec![PolicyEntry {
            debt: make_debt(
                tmp.path().join("myrepo"),
                "main",
                DebtClass::Ahead { n: 1 },
                true,
                None,
            ),
            policy: PolicyClass::AutoOk,
        }];
        let table = render_policy_table(&entries);
        assert!(table.contains("auto-ok"), "table must show 'auto-ok': {}", table);
        assert!(table.contains("1 repos"), "table must show totals: {}", table);
    }
}
