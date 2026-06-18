//! Integration tests for consign verify subcommand.
//!
//! Covers AC1–AC6 from PRD-consign-verify:
//!   AC1: converged — all repos clean, exit 0
//!   AC2: contradicted — drain-claimed repo still ahead, exit 1
//!   AC3: residual — auto-ok debt with no receipt, exit 0 + warning
//!   AC4: manual-only/diverged/no-remote never trigger contradicted
//!   AC5: survey always runs (survey_ran == true)
//!   AC6: SIGPIPE safe; JSON output is parseable

use std::path::{Path, PathBuf};
use std::process::Command;

use consign::drain::{ActionResult, DrainAction, DrainReceipt};
use consign::verify::{OverallVerdict, VerifyConfig};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

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

fn make_drain_receipt(name: &str, path: &Path, action: DrainAction, result: ActionResult) -> DrainReceipt {
    DrainReceipt {
        name: name.to_string(),
        path: path.to_path_buf(),
        branch: "main".to_string(),
        action,
        result,
        commits_pushed: 1,
        error_detail: None,
    }
}

// ---------------------------------------------------------------------------
// AC1: converged — all repos clean
// ---------------------------------------------------------------------------

#[test]
fn test_converged_all_clean() {
    let tmp = tempfile::TempDir::new().unwrap();

    // Create a fleet dir with one repo that is clean (pushed to bare remote)
    let fleet = tmp.path().join("fleet");
    std::fs::create_dir(&fleet).unwrap();

    let bare = make_bare(tmp.path(), "origin.git");
    let repo = make_repo(&fleet, "myrepo");
    git(&repo, &["remote", "add", "origin", &bare.to_string_lossy()]);
    git(&repo, &["push", "-u", "origin", "main"]);

    let config = VerifyConfig {
        roots: vec![fleet],
        drain_receipt: None,
    };

    let receipt = consign::verify::verify(&config).unwrap();

    assert_eq!(receipt.overall, OverallVerdict::Converged, "all-clean fleet must converge");
    assert!(receipt.contradictions.is_empty());
    assert!(receipt.residuals.is_empty());
    assert!(receipt.survey_ran, "survey_ran must be true (AC5)");
    assert_eq!(receipt.overall.exit_code(), 0);
}

// ---------------------------------------------------------------------------
// AC2: contradicted — drain claimed pushed but repo still ahead
// ---------------------------------------------------------------------------

#[test]
fn test_contradicted_still_ahead() {
    let tmp = tempfile::TempDir::new().unwrap();

    let fleet = tmp.path().join("fleet");
    std::fs::create_dir(&fleet).unwrap();

    let bare = make_bare(tmp.path(), "origin.git");
    let repo = make_repo(&fleet, "myrepo");
    git(&repo, &["remote", "add", "origin", &bare.to_string_lossy()]);
    git(&repo, &["push", "-u", "origin", "main"]);

    // Add a commit AFTER "drain" — so drain receipt says pushed but it's still ahead
    add_commit(&repo, "extra.txt");

    // Fake drain receipt claiming myrepo was pushed successfully
    let receipt_entry = make_drain_receipt("myrepo", &repo, DrainAction::Push, ActionResult::Ok);

    let config = VerifyConfig {
        roots: vec![fleet],
        drain_receipt: Some(vec![receipt_entry]),
    };

    let receipt = consign::verify::verify(&config).unwrap();

    assert_eq!(receipt.overall, OverallVerdict::Contradicted, "still-ahead repo after drain must be contradicted");
    assert!(receipt.contradictions.contains(&"myrepo".to_string()), "myrepo must be in contradictions");
    assert!(receipt.survey_ran, "survey_ran must be true (AC5)");
    assert_eq!(receipt.overall.exit_code(), 1);
}

// ---------------------------------------------------------------------------
// AC3: residual — auto-ok debt, no receipt
// ---------------------------------------------------------------------------

#[test]
fn test_residual_no_receipt() {
    let tmp = tempfile::TempDir::new().unwrap();

    let fleet = tmp.path().join("fleet");
    std::fs::create_dir(&fleet).unwrap();

    let bare = make_bare(tmp.path(), "origin.git");
    let repo = make_repo(&fleet, "myrepo");
    git(&repo, &["remote", "add", "origin", &bare.to_string_lossy()]);
    git(&repo, &["push", "-u", "origin", "main"]);

    // Repo is ahead — has debt
    add_commit(&repo, "unpushed.txt");

    // No drain receipt provided
    let config = VerifyConfig {
        roots: vec![fleet],
        drain_receipt: None,
    };

    let receipt = consign::verify::verify(&config).unwrap();

    assert_eq!(receipt.overall, OverallVerdict::Residual, "auto-ok debt with no receipt must be residual");
    assert!(receipt.residuals.contains(&"myrepo".to_string()), "myrepo must be in residuals");
    assert!(receipt.contradictions.is_empty(), "no receipt => no contradictions");
    assert!(receipt.survey_ran, "survey_ran must be true (AC5)");
    assert_eq!(receipt.overall.exit_code(), 0, "residual is exit 0");
}

// ---------------------------------------------------------------------------
// AC4: manual-only/diverged debt never triggers contradicted
// ---------------------------------------------------------------------------

#[test]
fn test_diverged_out_of_scope() {
    let tmp = tempfile::TempDir::new().unwrap();

    let fleet = tmp.path().join("fleet");
    std::fs::create_dir(&fleet).unwrap();

    let bare = make_bare(tmp.path(), "origin.git");
    let local = make_repo(&fleet, "diverged_repo");
    git(&local, &["remote", "add", "origin", &bare.to_string_lossy()]);
    git(&local, &["push", "-u", "origin", "main"]);

    // Create a diverged state: another clone adds a remote commit, then local also commits
    let local2 = tmp.path().join("local2");
    git(tmp.path(), &["clone", &bare.to_string_lossy(), "local2"]);
    git(&local2, &["config", "user.email", "test@example.com"]);
    git(&local2, &["config", "user.name", "Test"]);
    add_commit(&local2, "remote_file.txt");
    git(&local2, &["push", "origin", "main"]);
    // Local also has a commit
    add_commit(&local, "local_file.txt");
    git(&local, &["fetch", "origin"]);

    // Fake drain receipt claiming diverged_repo was pushed (shouldn't happen but let's test)
    let receipt_entry = make_drain_receipt("diverged_repo", &local, DrainAction::Push, ActionResult::Ok);

    let config = VerifyConfig {
        roots: vec![fleet],
        drain_receipt: Some(vec![receipt_entry]),
    };

    let receipt = consign::verify::verify(&config).unwrap();

    // Diverged repos are out of scope — must NOT trigger contradicted
    assert_ne!(
        receipt.overall,
        OverallVerdict::Contradicted,
        "diverged repo must not trigger contradicted verdict"
    );
    assert!(
        receipt.contradictions.is_empty(),
        "diverged repo must never be in contradictions"
    );
}

// ---------------------------------------------------------------------------
// AC4: no-remote debt never triggers contradicted
// ---------------------------------------------------------------------------

#[test]
fn test_no_remote_out_of_scope() {
    let tmp = tempfile::TempDir::new().unwrap();

    let fleet = tmp.path().join("fleet");
    std::fs::create_dir(&fleet).unwrap();

    // Repo with no remote at all
    let repo = make_repo(&fleet, "no_remote_repo");
    // No remote added

    // Fake drain receipt claiming it was pushed (absurd, but tests the guard)
    let receipt_entry = make_drain_receipt("no_remote_repo", &repo, DrainAction::Push, ActionResult::Ok);

    let config = VerifyConfig {
        roots: vec![fleet],
        drain_receipt: Some(vec![receipt_entry]),
    };

    let receipt = consign::verify::verify(&config).unwrap();

    assert_ne!(
        receipt.overall,
        OverallVerdict::Contradicted,
        "no-remote repo must not trigger contradicted verdict"
    );
    assert!(
        receipt.contradictions.is_empty(),
        "no-remote repo must never be in contradictions"
    );
}

// ---------------------------------------------------------------------------
// AC5: survey always runs, even with --against supplied
// ---------------------------------------------------------------------------

#[test]
fn test_survey_runs_with_receipt() {
    let tmp = tempfile::TempDir::new().unwrap();

    let fleet = tmp.path().join("fleet");
    std::fs::create_dir(&fleet).unwrap();

    let bare = make_bare(tmp.path(), "origin.git");
    let repo = make_repo(&fleet, "repo1");
    git(&repo, &["remote", "add", "origin", &bare.to_string_lossy()]);
    git(&repo, &["push", "-u", "origin", "main"]);

    let receipt_entry = make_drain_receipt("repo1", &repo, DrainAction::Push, ActionResult::Ok);

    let config = VerifyConfig {
        roots: vec![fleet],
        drain_receipt: Some(vec![receipt_entry]),
    };

    let receipt = consign::verify::verify(&config).unwrap();

    // survey_ran must always be true
    assert!(receipt.survey_ran, "survey must run even when drain_receipt is supplied");
    // repo1 is clean so should converge
    assert_eq!(receipt.overall, OverallVerdict::Converged);
}

// ---------------------------------------------------------------------------
// AC6: JSON output is stable and parseable
// ---------------------------------------------------------------------------

#[test]
fn test_json_output_parseable() {
    let tmp = tempfile::TempDir::new().unwrap();

    let fleet = tmp.path().join("fleet");
    std::fs::create_dir(&fleet).unwrap();

    let bare = make_bare(tmp.path(), "origin.git");
    let repo = make_repo(&fleet, "clean_repo");
    git(&repo, &["remote", "add", "origin", &bare.to_string_lossy()]);
    git(&repo, &["push", "-u", "origin", "main"]);

    let config = VerifyConfig {
        roots: vec![fleet],
        drain_receipt: None,
    };

    let receipt = consign::verify::verify(&config).unwrap();
    let json = serde_json::to_string_pretty(&receipt).unwrap();

    // Must be parseable back
    let _reparsed: consign::verify::VerifyReceipt = serde_json::from_str(&json)
        .expect("VerifyReceipt JSON must round-trip cleanly");

    // Must contain known keys
    assert!(json.contains("\"overall\""), "JSON must contain 'overall' key");
    assert!(json.contains("\"repos\""), "JSON must contain 'repos' key");
    assert!(json.contains("\"survey_ran\""), "JSON must contain 'survey_ran' key");
}
