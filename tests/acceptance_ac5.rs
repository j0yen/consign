//! AC5: --root overrides default; non-git dirs skipped; unreadable root => structured error.

use std::path::PathBuf;
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
fn ac5_non_git_dirs_skipped() {
    let tmp = tempfile::TempDir::new().unwrap();
    // One git repo
    let repo = tmp.path().join("my_repo");
    std::fs::create_dir(&repo).unwrap();
    git(&repo, &["init", "-b", "main"]);
    git(&repo, &["config", "user.email", "test@example.com"]);
    git(&repo, &["config", "user.name", "Test"]);
    std::fs::write(repo.join("f.txt"), "x").unwrap();
    git(&repo, &["add", "."]);
    git(&repo, &["commit", "-m", "init"]);
    // One non-git dir
    std::fs::create_dir(tmp.path().join("not_a_repo")).unwrap();

    let results = consign::survey(&[tmp.path().to_path_buf()]).unwrap();
    assert_eq!(results.len(), 1, "non-git dirs must be skipped");
    assert!(results[0].path.ends_with("my_repo"));
}

#[test]
fn ac5_unreadable_root_is_structured_error() {
    let bad = PathBuf::from("/this/path/definitely/does/not/exist/here");
    let result = consign::survey(&[bad]);
    assert!(result.is_err(), "expected Err for unreadable root");
    match result {
        Err(consign::SurveyError::UnreadableRoot(_, _)) => {} // correct
        other => panic!("expected SurveyError::UnreadableRoot, got {:?}", other),
    }
}

#[test]
fn ac5_multiple_roots() {
    let tmp1 = tempfile::TempDir::new().unwrap();
    let tmp2 = tempfile::TempDir::new().unwrap();
    for (tmp, name) in [(&tmp1, "r1"), (&tmp2, "r2")] {
        let repo = tmp.path().join(name);
        std::fs::create_dir(&repo).unwrap();
        git(&repo, &["init", "-b", "main"]);
        git(&repo, &["config", "user.email", "test@example.com"]);
        git(&repo, &["config", "user.name", "Test"]);
        std::fs::write(repo.join("f.txt"), "x").unwrap();
        git(&repo, &["add", "."]);
        git(&repo, &["commit", "-m", "init"]);
    }
    let results =
        consign::survey(&[tmp1.path().to_path_buf(), tmp2.path().to_path_buf()]).unwrap();
    assert_eq!(results.len(), 2, "multiple roots merged");
}
