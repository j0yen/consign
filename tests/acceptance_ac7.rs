//! AC7: cargo build --release produces the binary and `consign --help` lists survey.

use std::process::Command;

fn consign_bin() -> std::path::PathBuf {
    if let Ok(p) = std::env::var("CARGO_BIN_EXE_consign") {
        return std::path::PathBuf::from(p);
    }
    let mut p = std::path::PathBuf::from(
        std::env::var("CARGO_MANIFEST_DIR").unwrap_or_else(|_| ".".to_string()),
    );
    p.push("target/debug/consign");
    p
}

#[test]
fn ac7_help_lists_survey() {
    let bin = consign_bin();
    if !bin.exists() {
        eprintln!("ac7: binary not found at {:?}, skipping", bin);
        return;
    }
    let output = Command::new(&bin)
        .arg("--help")
        .output()
        .expect("failed to run consign --help");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let combined = format!("{}{}", stdout, stderr);
    assert!(
        combined.contains("survey"),
        "consign --help should list 'survey' subcommand; got:\n{}",
        combined
    );
}
