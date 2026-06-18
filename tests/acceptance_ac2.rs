//! AC2: repos with no upstream tracking branch are classified `no-upstream`
//! with a non-zero unpushed count, NOT `clean`.

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
fn ac2_no_upstream_not_clean() {
    // Simulate five repos like the known fleet repos (constellation-burst-builder,
    // doxa, homeward, mqo-chart-caption, wm-skills): each has a remote but no
    // upstream tracking branch set and ≥1 unpushed commit.
    let names = [
        "constellation-burst-builder",
        "doxa",
        "homeward",
        "mqo-chart-caption",
        "wm-skills",
    ];
    let tmp = tempfile::TempDir::new().unwrap();

    for name in &names {
        let bare = tmp.path().join(format!("{}.git", name));
        std::fs::create_dir(&bare).unwrap();
        git(&bare, &["init", "--bare", "-b", "main"]);

        let local = tmp.path().join(name);
        std::fs::create_dir(&local).unwrap();
        git(&local, &["init", "-b", "main"]);
        git(&local, &["config", "user.email", "test@example.com"]);
        git(&local, &["config", "user.name", "Test"]);
        std::fs::write(local.join("README.md"), "hi").unwrap();
        git(&local, &["add", "."]);
        git(&local, &["commit", "-m", "init"]);
        git(
            &local,
            &["remote", "add", "origin", &bare.to_string_lossy()],
        );
        // Crucially: do NOT push and do NOT set upstream — no tracking branch

        let debt = consign::classify_repo(&local).unwrap();
        assert!(
            matches!(debt.class, consign::DebtClass::NoUpstream { .. }),
            "{}: expected NoUpstream, got {:?}",
            name,
            debt.class
        );
        if let consign::DebtClass::NoUpstream { n } = debt.class {
            assert!(
                n > 0,
                "{}: unpushed count should be >0, got {}",
                name,
                n
            );
        }
    }
}
