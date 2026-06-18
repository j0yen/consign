//! AC4: table format prints aligned table and totals footer; totals sum to repo count.

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
fn ac4_table_totals_sum_to_repo_count() {
    let tmp = tempfile::TempDir::new().unwrap();

    // Create 3 repos of different classes
    for name in &["r1", "r2", "r3"] {
        let repo = tmp.path().join(name);
        std::fs::create_dir(&repo).unwrap();
        git(&repo, &["init", "-b", "main"]);
        git(&repo, &["config", "user.email", "test@example.com"]);
        git(&repo, &["config", "user.name", "Test"]);
        std::fs::write(repo.join("f.txt"), "x").unwrap();
        git(&repo, &["add", "."]);
        git(&repo, &["commit", "-m", "init"]);
    }

    let debts = consign::survey(&[tmp.path().to_path_buf()]).unwrap();
    assert_eq!(debts.len(), 3);

    let table = consign::render_table(&debts);
    // Table should contain "3 repos"
    assert!(
        table.contains("3 repos"),
        "footer should say '3 repos'; got:\n{}",
        table
    );
    // The totals in the footer should parse and sum to 3
    // Footer format: "N repos: X clean, Y ahead, Z no-upstream, W no-remote, V diverged"
    let footer_line = table
        .lines()
        .last()
        .unwrap_or("");
    // Extract all numbers from the footer
    let nums: Vec<u32> = footer_line
        .split_whitespace()
        .filter_map(|tok| tok.trim_end_matches(',').parse::<u32>().ok())
        .collect();
    // nums[0] = total, rest = per-class counts
    assert!(!nums.is_empty(), "no numbers in footer: {}", footer_line);
    let total = nums[0];
    let sum: u32 = nums[1..].iter().sum();
    assert_eq!(total, sum, "totals must sum to repo count");
    assert_eq!(total, 3);
}
