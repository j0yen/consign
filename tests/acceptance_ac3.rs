//! AC3: no-remote, ahead(n), and diverged(a/b) classifications against constructed fixture repos.

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

fn make_bare(parent: &Path, name: &str) -> std::path::PathBuf {
    let bare = parent.join(name);
    std::fs::create_dir(&bare).unwrap();
    git(&bare, &["init", "--bare", "-b", "main"]);
    bare
}

fn make_local(parent: &Path, name: &str) -> std::path::PathBuf {
    let local = parent.join(name);
    std::fs::create_dir(&local).unwrap();
    git(&local, &["init", "-b", "main"]);
    git(&local, &["config", "user.email", "test@example.com"]);
    git(&local, &["config", "user.name", "Test"]);
    std::fs::write(local.join("README.md"), "hi").unwrap();
    git(&local, &["add", "."]);
    git(&local, &["commit", "-m", "init"]);
    local
}

#[test]
fn ac3_no_remote() {
    let tmp = tempfile::TempDir::new().unwrap();
    let local = make_local(tmp.path(), "local");
    let debt = consign::classify_repo(&local).unwrap();
    assert_eq!(debt.class, consign::DebtClass::NoRemote);
}

#[test]
fn ac3_ahead_n() {
    let tmp = tempfile::TempDir::new().unwrap();
    let bare = make_bare(tmp.path(), "origin.git");
    let local = make_local(tmp.path(), "local");
    git(&local, &["remote", "add", "origin", &bare.to_string_lossy()]);
    git(&local, &["push", "-u", "origin", "main"]);

    // Add 3 commits
    for i in 0..3u8 {
        std::fs::write(local.join(format!("f{}.txt", i)), "x").unwrap();
        git(&local, &["add", "."]);
        git(&local, &["commit", "-m", &format!("add f{}", i)]);
    }

    let debt = consign::classify_repo(&local).unwrap();
    assert_eq!(debt.class, consign::DebtClass::Ahead { n: 3 });
}

#[test]
fn ac3_diverged() {
    let tmp = tempfile::TempDir::new().unwrap();
    let bare = make_bare(tmp.path(), "origin.git");
    let local = make_local(tmp.path(), "local");
    git(&local, &["remote", "add", "origin", &bare.to_string_lossy()]);
    git(&local, &["push", "-u", "origin", "main"]);

    // Remote gets a commit via a second clone
    let local2 = tmp.path().join("local2");
    git(tmp.path(), &["clone", &bare.to_string_lossy(), "local2"]);
    git(&local2, &["config", "user.email", "test@example.com"]);
    git(&local2, &["config", "user.name", "Test"]);
    std::fs::write(local2.join("remote.txt"), "r").unwrap();
    git(&local2, &["add", "."]);
    git(&local2, &["commit", "-m", "remote commit"]);
    git(&local2, &["push", "origin", "main"]);

    // Local gets a different commit (now diverged)
    std::fs::write(local.join("local.txt"), "l").unwrap();
    git(&local, &["add", "."]);
    git(&local, &["commit", "-m", "local commit"]);
    git(&local, &["fetch", "origin"]);

    let debt = consign::classify_repo(&local).unwrap();
    assert!(
        matches!(debt.class, consign::DebtClass::Diverged { a: 1, b: 1 }),
        "expected Diverged(1,1), got {:?}",
        debt.class
    );
}
