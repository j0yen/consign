//! AC1: consign survey --format json returns a JSON array with one object per
//! immediate-child git repo; each object has path, branch, class, and class-appropriate counts.

use std::path::Path;
use std::process::Command;

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

#[test]
fn ac1_json_array_has_required_fields() {
    let tmp = tempfile::TempDir::new().unwrap();
    let repo1 = tmp.path().join("repo_a");
    let repo2 = tmp.path().join("repo_b");
    std::fs::create_dir(&repo1).unwrap();
    std::fs::create_dir(&repo2).unwrap();
    for repo in [&repo1, &repo2] {
        git(repo, &["init", "-b", "main"]);
        git(repo, &["config", "user.email", "test@example.com"]);
        git(repo, &["config", "user.name", "Test"]);
        std::fs::write(repo.join("f.txt"), "x").unwrap();
        git(repo, &["add", "."]);
        git(repo, &["commit", "-m", "init"]);
    }
    // Also create a non-git dir (should be skipped)
    std::fs::create_dir(tmp.path().join("not_git")).unwrap();

    let debts = consign::survey(&[tmp.path().to_path_buf()]).unwrap();
    assert_eq!(debts.len(), 2, "only git repos returned");

    // Serialize to JSON and parse
    let json_str = serde_json::to_string_pretty(&debts).unwrap();
    let arr: serde_json::Value = serde_json::from_str(&json_str).unwrap();
    assert!(arr.is_array());
    for obj in arr.as_array().unwrap() {
        assert!(obj.get("path").is_some(), "has path");
        assert!(obj.get("branch").is_some(), "has branch");
        assert!(obj.get("class").is_some(), "has class");
    }
}
