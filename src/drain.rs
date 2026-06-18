//! consign drain — push eligible repos safely.
//!
//! Runs survey, classifies each repo, and for each auto-ok ahead/no-upstream
//! repo either performs (or plans) the appropriate git push. Never force-pushes.

use std::path::{Path, PathBuf};
use std::process::Command;

use serde::{Deserialize, Serialize};

use crate::{DebtClass, RepoDebt};

// ---------------------------------------------------------------------------
// Inline policy — minimal classification sufficient for drain.
// When consign-policy lands, this can delegate to policy::classify().
// ---------------------------------------------------------------------------

/// Policy decision for a repo.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PolicyDecision {
    /// Drain may push automatically.
    AutoOk,
    /// Diverged — leave for human.
    ManualOnly,
    /// No remote — drain doesn't mint remotes; use consign-publish.
    NoRemote,
    /// Already clean.
    Clean,
}

/// Classify a repo's push-debt into a policy decision.
pub fn classify(debt: &RepoDebt) -> PolicyDecision {
    match &debt.class {
        DebtClass::Ahead { .. } => PolicyDecision::AutoOk,
        DebtClass::NoUpstream { .. } => PolicyDecision::AutoOk,
        DebtClass::Diverged { .. } => PolicyDecision::ManualOnly,
        DebtClass::NoRemote => PolicyDecision::NoRemote,
        DebtClass::Clean => PolicyDecision::Clean,
    }
}

// ---------------------------------------------------------------------------
// Drain action types
// ---------------------------------------------------------------------------

/// The push action that drain will take for a repo.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum DrainAction {
    /// `git push` (upstream set, n commits ahead)
    Push,
    /// `git push --set-upstream origin <branch>`
    SetUpstream,
    /// Skip — diverged or manual-only; human must resolve
    Skip,
    /// Skip — no remote; consign-publish handles this
    NoRemote,
    /// Skip — already clean
    Clean,
}

/// Result of a single drain attempt.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ActionResult {
    Ok,
    DryRun,
    Error,
}

/// Per-repo receipt emitted by drain.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DrainReceipt {
    /// Repo name (last path component).
    pub name: String,
    /// Absolute path to the repo root.
    pub path: PathBuf,
    /// Current branch.
    pub branch: String,
    /// The action taken (or planned in dry-run).
    pub action: DrainAction,
    /// Result status.
    pub result: ActionResult,
    /// Number of commits pushed (0 for dry-run or skip).
    pub commits_pushed: u32,
    /// Error detail if result is Error.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_detail: Option<String>,
}

// ---------------------------------------------------------------------------
// Push runner (injectable for testing)
// ---------------------------------------------------------------------------

/// The result of running a git push command.
#[derive(Debug)]
pub struct PushOutcome {
    pub success: bool,
    pub stderr: String,
}

/// Trait for running git push — injectable in tests.
pub trait PushRunner: Send + Sync {
    /// Run a git push. The args are the arguments passed to `git push`.
    /// INVARIANT: args must never contain "--force" or "--force-with-lease".
    fn run_push(&self, repo: &Path, args: &[&str]) -> PushOutcome;
}

/// Production runner — actually invokes git.
pub struct RealPushRunner;

impl PushRunner for RealPushRunner {
    fn run_push(&self, repo: &Path, args: &[&str]) -> PushOutcome {
        // Safety: verify no force flags are in args (belt-and-suspenders)
        for arg in args {
            assert!(
                *arg != "--force" && *arg != "--force-with-lease",
                "drain: INVARIANT VIOLATED — force flag in push args: {:?}",
                args
            );
        }
        let out = Command::new("git")
            .args(["-C", &repo.to_string_lossy()])
            .arg("push")
            .args(args)
            .output();
        match out {
            Ok(o) => PushOutcome {
                success: o.status.success(),
                stderr: String::from_utf8_lossy(&o.stderr).trim().to_string(),
            },
            Err(e) => PushOutcome {
                success: false,
                stderr: format!("failed to spawn git: {}", e),
            },
        }
    }
}

// ---------------------------------------------------------------------------
// Drain configuration
// ---------------------------------------------------------------------------

/// Configuration for a drain run.
pub struct DrainConfig<'a> {
    /// If true, plan but do not push.
    pub dry_run: bool,
    /// If Some, only drain repos whose name matches one of the given names.
    pub only: &'a [String],
    /// Push runner (real or injected test double).
    pub runner: &'a dyn PushRunner,
}

// ---------------------------------------------------------------------------
// Core drain logic
// ---------------------------------------------------------------------------

/// Drain a single repo according to its classification.
/// Returns a receipt regardless of outcome.
pub fn drain_one(debt: &RepoDebt, config: &DrainConfig<'_>) -> DrainReceipt {
    let name = debt
        .path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| debt.path.display().to_string());

    // --only filter
    if !config.only.is_empty() && !config.only.iter().any(|o| *o == name) {
        return DrainReceipt {
            name,
            path: debt.path.clone(),
            branch: debt.branch.clone(),
            action: DrainAction::Skip,
            result: ActionResult::Ok,
            commits_pushed: 0,
            error_detail: Some("excluded by --only filter".into()),
        };
    }

    let policy = classify(debt);

    match policy {
        PolicyDecision::Clean => DrainReceipt {
            name,
            path: debt.path.clone(),
            branch: debt.branch.clone(),
            action: DrainAction::Clean,
            result: ActionResult::Ok,
            commits_pushed: 0,
            error_detail: None,
        },
        PolicyDecision::NoRemote => DrainReceipt {
            name,
            path: debt.path.clone(),
            branch: debt.branch.clone(),
            action: DrainAction::NoRemote,
            result: ActionResult::Ok,
            commits_pushed: 0,
            error_detail: Some(
                "no remote configured; use consign-publish to create remote".into(),
            ),
        },
        PolicyDecision::ManualOnly => DrainReceipt {
            name,
            path: debt.path.clone(),
            branch: debt.branch.clone(),
            action: DrainAction::Skip,
            result: ActionResult::Ok,
            commits_pushed: 0,
            error_detail: Some(format!(
                "diverged (ahead {} behind {}) — needs human review",
                match &debt.class {
                    DebtClass::Diverged { a, .. } => *a,
                    _ => 0,
                },
                match &debt.class {
                    DebtClass::Diverged { b, .. } => *b,
                    _ => 0,
                },
            )),
        },
        PolicyDecision::AutoOk => {
            let commits_ahead = match &debt.class {
                DebtClass::Ahead { n } => *n,
                DebtClass::NoUpstream { n } => *n,
                _ => 0,
            };

            let action = match &debt.class {
                DebtClass::NoUpstream { .. } => DrainAction::SetUpstream,
                _ => DrainAction::Push,
            };

            if config.dry_run {
                return DrainReceipt {
                    name,
                    path: debt.path.clone(),
                    branch: debt.branch.clone(),
                    action,
                    result: ActionResult::DryRun,
                    commits_pushed: 0,
                    error_detail: None,
                };
            }

            // Perform the actual push
            let outcome = match &action {
                DrainAction::SetUpstream => config.runner.run_push(
                    &debt.path,
                    &["--set-upstream", "origin", &debt.branch],
                ),
                DrainAction::Push => config.runner.run_push(&debt.path, &[]),
                _ => unreachable!(),
            };

            if outcome.success {
                DrainReceipt {
                    name,
                    path: debt.path.clone(),
                    branch: debt.branch.clone(),
                    action,
                    result: ActionResult::Ok,
                    commits_pushed: commits_ahead,
                    error_detail: None,
                }
            } else {
                DrainReceipt {
                    name,
                    path: debt.path.clone(),
                    branch: debt.branch.clone(),
                    action,
                    result: ActionResult::Error,
                    commits_pushed: 0,
                    error_detail: Some(outcome.stderr),
                }
            }
        }
    }
}

/// Run drain over a set of pre-surveyed repo debts.
/// Returns (receipts, needs_human) where needs_human lists skipped diverged repos.
pub fn drain_all(
    debts: &[RepoDebt],
    config: &DrainConfig<'_>,
) -> (Vec<DrainReceipt>, Vec<DrainReceipt>) {
    let mut receipts = Vec::new();
    let mut needs_human = Vec::new();

    for debt in debts {
        let receipt = drain_one(debt, config);
        match receipt.action {
            DrainAction::Skip if receipt.error_detail.as_deref().map_or(false, |d| d.contains("diverged")) => {
                needs_human.push(receipt);
            }
            _ => receipts.push(receipt),
        }
    }

    (receipts, needs_human)
}

// ---------------------------------------------------------------------------
// Text rendering
// ---------------------------------------------------------------------------

/// Render drain receipts as human-readable table output.
pub fn render_drain_table(receipts: &[DrainReceipt], needs_human: &[DrainReceipt]) -> String {
    use std::fmt::Write;
    let mut out = String::new();

    let actionable: Vec<&DrainReceipt> = receipts
        .iter()
        .filter(|r| {
            matches!(
                r.action,
                DrainAction::Push | DrainAction::SetUpstream
            )
        })
        .collect();

    if actionable.is_empty() && needs_human.is_empty() {
        let _ = writeln!(out, "nothing to do");
        return out;
    }

    if !actionable.is_empty() {
        let _ = writeln!(out, "Push plan:");
        for r in &actionable {
            let status = match r.result {
                ActionResult::DryRun => "[dry-run]".to_string(),
                ActionResult::Ok => format!("ok (+{})", r.commits_pushed),
                ActionResult::Error => format!(
                    "error: {}",
                    r.error_detail.as_deref().unwrap_or("unknown")
                ),
            };
            let action_str = match r.action {
                DrainAction::Push => "push",
                DrainAction::SetUpstream => "push --set-upstream",
                _ => "skip",
            };
            let _ = writeln!(
                out,
                "  {} {} ({}) — {}",
                action_str, r.name, r.branch, status
            );
        }
    }

    if !needs_human.is_empty() {
        let _ = writeln!(out, "\nNeeds human:");
        for r in needs_human {
            let detail = r.error_detail.as_deref().unwrap_or("manual review required");
            let _ = writeln!(out, "  {} ({}) — {}", r.name, r.branch, detail);
        }
    }

    out
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    use tempfile::TempDir;

    // -----------------------------------------------------------------------
    // Test helpers
    // -----------------------------------------------------------------------

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
        std::fs::write(dir.join("README.md"), "hi").unwrap();
        git(&dir, &["add", "."]);
        git(&dir, &["commit", "-m", "init"]);
        dir
    }

    fn make_bare(parent: &Path, name: &str) -> PathBuf {
        let dir = parent.join(name);
        std::fs::create_dir(&dir).unwrap();
        git(&dir, &["init", "--bare", "-b", "main"]);
        dir
    }

    fn add_commit(dir: &Path, file: &str) {
        std::fs::write(dir.join(file), file).unwrap();
        git(dir, &["add", "."]);
        git(dir, &["commit", "-m", &format!("add {}", file)]);
    }

    fn make_debt(path: PathBuf, branch: &str, class: DebtClass) -> RepoDebt {
        RepoDebt {
            path,
            branch: branch.to_string(),
            default_branch: Some("main".to_string()),
            remote_url: None,
            class,
            branch_is_default: true,
        }
    }

    // -----------------------------------------------------------------------
    // Spy runner — records push invocations without actually running git
    // -----------------------------------------------------------------------

    struct SpyRunner {
        calls: Mutex<Vec<(PathBuf, Vec<String>)>>,
        succeed: bool,
    }

    impl SpyRunner {
        fn new(succeed: bool) -> Self {
            SpyRunner {
                calls: Mutex::new(Vec::new()),
                succeed,
            }
        }

        fn calls(&self) -> Vec<(PathBuf, Vec<String>)> {
            self.calls.lock().unwrap().clone()
        }
    }

    impl PushRunner for SpyRunner {
        fn run_push(&self, repo: &Path, args: &[&str]) -> PushOutcome {
            // AC3: verify no force flags ever
            for arg in args {
                assert!(
                    *arg != "--force" && *arg != "--force-with-lease",
                    "force flag detected in push args: {:?}",
                    args
                );
            }
            self.calls
                .lock()
                .unwrap()
                .push((repo.to_path_buf(), args.iter().map(|s| s.to_string()).collect()));
            PushOutcome {
                success: self.succeed,
                stderr: if self.succeed {
                    String::new()
                } else {
                    "simulated push error".into()
                },
            }
        }
    }

    // -----------------------------------------------------------------------
    // AC1: dry-run does not push
    // -----------------------------------------------------------------------

    #[test]
    fn test_dry_run_does_not_push() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("myrepo");
        std::fs::create_dir(&path).unwrap();

        let debt = make_debt(path, "main", DebtClass::Ahead { n: 3 });
        let runner = SpyRunner::new(true);
        let config = DrainConfig {
            dry_run: true,
            only: &[],
            runner: &runner,
        };

        let receipt = drain_one(&debt, &config);

        // No calls should have been made
        assert!(
            runner.calls().is_empty(),
            "dry-run must not invoke the push runner"
        );
        assert_eq!(receipt.result, ActionResult::DryRun);
        assert_eq!(receipt.action, DrainAction::Push);
    }

    // -----------------------------------------------------------------------
    // AC2: --no-dry-run pushes ahead with plain push
    // -----------------------------------------------------------------------

    #[test]
    fn test_nodryrun_pushes_ahead() {
        let tmp = TempDir::new().unwrap();
        // Create actual repos so classification data is meaningful
        let bare = make_bare(tmp.path(), "origin.git");
        let local = make_repo(tmp.path(), "local");
        git(&local, &["remote", "add", "origin", &bare.to_string_lossy()]);
        git(&local, &["push", "-u", "origin", "main"]);
        add_commit(&local, "extra.txt");

        // Classify the actual repo to get real data
        let debt = crate::classify_repo(&local).unwrap();
        assert_eq!(debt.class, DebtClass::Ahead { n: 1 });

        let runner = SpyRunner::new(true);
        let config = DrainConfig {
            dry_run: false,
            only: &[],
            runner: &runner,
        };

        let receipt = drain_one(&debt, &config);

        assert_eq!(receipt.result, ActionResult::Ok);
        assert_eq!(receipt.action, DrainAction::Push);
        assert_eq!(receipt.commits_pushed, 1);

        // Should have called runner with no extra args (plain push)
        let calls = runner.calls();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].1, Vec::<String>::new(), "ahead push args should be empty");
    }

    // -----------------------------------------------------------------------
    // AC2: --no-dry-run sets upstream for no-upstream repos
    // -----------------------------------------------------------------------

    #[test]
    fn test_nodryrun_sets_upstream() {
        let tmp = TempDir::new().unwrap();
        let bare = make_bare(tmp.path(), "origin.git");
        let local = make_repo(tmp.path(), "local");
        git(&local, &["remote", "add", "origin", &bare.to_string_lossy()]);
        // No push — no upstream tracking branch
        add_commit(&local, "extra.txt");

        let debt = crate::classify_repo(&local).unwrap();
        assert!(
            matches!(debt.class, DebtClass::NoUpstream { .. }),
            "expected NoUpstream, got {:?}",
            debt.class
        );

        let runner = SpyRunner::new(true);
        let config = DrainConfig {
            dry_run: false,
            only: &[],
            runner: &runner,
        };

        let receipt = drain_one(&debt, &config);

        assert_eq!(receipt.result, ActionResult::Ok);
        assert_eq!(receipt.action, DrainAction::SetUpstream);

        let calls = runner.calls();
        assert_eq!(calls.len(), 1);
        let args = &calls[0].1;
        assert!(
            args.contains(&"--set-upstream".to_string()),
            "no-upstream push must use --set-upstream; got {:?}",
            args
        );
        assert!(
            args.contains(&"origin".to_string()),
            "no-upstream push must specify 'origin'; got {:?}",
            args
        );
        // AC3: verify no force
        assert!(!args.contains(&"--force".to_string()));
        assert!(!args.contains(&"--force-with-lease".to_string()));
    }

    // -----------------------------------------------------------------------
    // AC3: no force flags ever emitted
    // -----------------------------------------------------------------------

    #[test]
    fn test_no_force_flags() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("myrepo");
        std::fs::create_dir(&path).unwrap();

        // Test ahead
        let debt_ahead = make_debt(path.clone(), "main", DebtClass::Ahead { n: 1 });
        let runner = SpyRunner::new(true);
        let config = DrainConfig {
            dry_run: false,
            only: &[],
            runner: &runner,
        };
        drain_one(&debt_ahead, &config);

        // Test no-upstream
        let debt_nu = make_debt(path.clone(), "main", DebtClass::NoUpstream { n: 1 });
        drain_one(&debt_nu, &config);

        for (_, args) in runner.calls() {
            assert!(
                !args.contains(&"--force".to_string()),
                "--force must never appear in push args"
            );
            assert!(
                !args.contains(&"--force-with-lease".to_string()),
                "--force-with-lease must never appear in push args"
            );
        }
    }

    // -----------------------------------------------------------------------
    // AC4: diverged repos are skipped and appear in needs_human
    // -----------------------------------------------------------------------

    #[test]
    fn test_diverged_skipped() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("myrepo");
        std::fs::create_dir(&path).unwrap();

        let debt = make_debt(path, "main", DebtClass::Diverged { a: 2, b: 2 });
        let runner = SpyRunner::new(true);
        let config = DrainConfig {
            dry_run: false,
            only: &[],
            runner: &runner,
        };

        let (receipts, needs_human) = drain_all(&[debt], &config);

        // No push calls
        assert!(runner.calls().is_empty(), "diverged must not be pushed");
        // Appears in needs_human
        assert_eq!(needs_human.len(), 1, "diverged must appear in needs_human");
        // Not in regular receipts
        assert!(
            !receipts.iter().any(|r| r.action == DrainAction::Push),
            "diverged must not be in push receipts"
        );
    }

    // -----------------------------------------------------------------------
    // AC5: push error records structured error, batch continues
    // -----------------------------------------------------------------------

    #[test]
    fn test_push_error_continues() {
        let tmp = TempDir::new().unwrap();
        let path1 = tmp.path().join("r1");
        let path2 = tmp.path().join("r2");
        std::fs::create_dir(&path1).unwrap();
        std::fs::create_dir(&path2).unwrap();

        let debts = vec![
            make_debt(path1, "main", DebtClass::Ahead { n: 1 }),
            make_debt(path2, "main", DebtClass::Ahead { n: 2 }),
        ];

        // Runner that always fails
        let runner = SpyRunner::new(false);
        let config = DrainConfig {
            dry_run: false,
            only: &[],
            runner: &runner,
        };

        let (receipts, _) = drain_all(&debts, &config);

        // Both repos should have receipts
        let push_receipts: Vec<_> = receipts
            .iter()
            .filter(|r| r.action == DrainAction::Push)
            .collect();
        assert_eq!(push_receipts.len(), 2, "both repos should have receipts");

        // Both should record error
        for r in &push_receipts {
            assert_eq!(r.result, ActionResult::Error, "failed push should be Error");
            assert!(
                r.error_detail.is_some(),
                "error receipt must have error_detail"
            );
        }
    }

    // -----------------------------------------------------------------------
    // AC6: clean fleet yields "nothing to do"
    // -----------------------------------------------------------------------

    #[test]
    fn test_nothing_to_do() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("myrepo");
        std::fs::create_dir(&path).unwrap();

        let debts = vec![make_debt(path, "main", DebtClass::Clean)];
        let runner = SpyRunner::new(true);
        let config = DrainConfig {
            dry_run: false,
            only: &[],
            runner: &runner,
        };

        let (receipts, needs_human) = drain_all(&debts, &config);
        let table = render_drain_table(&receipts, &needs_human);

        assert!(runner.calls().is_empty());
        assert!(
            table.contains("nothing to do"),
            "clean fleet must emit 'nothing to do': {}",
            table
        );
    }
}
