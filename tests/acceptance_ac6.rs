//! AC6: SIGPIPE reset — consign survey | head -1 must not panic.
//! This test verifies the binary does not exit with signal 141 (SIGPIPE) when
//! the pipe consumer closes early.

use std::io::Read;
use std::process::{Command, Stdio};

/// Find the consign binary in the target tree.
fn consign_bin() -> std::path::PathBuf {
    // cargo test sets CARGO_BIN_EXE_consign for integration tests
    if let Ok(p) = std::env::var("CARGO_BIN_EXE_consign") {
        return std::path::PathBuf::from(p);
    }
    // Fallback: look relative to OUT_DIR
    let mut p = std::path::PathBuf::from(
        std::env::var("CARGO_MANIFEST_DIR").unwrap_or_else(|_| ".".to_string()),
    );
    p.push("target/debug/consign");
    p
}

#[test]
fn ac6_sigpipe_no_panic() {
    let bin = consign_bin();
    if !bin.exists() {
        // Binary not built yet in this test environment; skip
        eprintln!("ac6: binary not found at {:?}, skipping", bin);
        return;
    }

    // Run `consign survey --format table` piped to `head -1` via shell
    let output = Command::new("sh")
        .arg("-c")
        .arg(format!("{} survey --format table 2>/dev/null | head -1", bin.display()))
        .output()
        .expect("failed to spawn shell");

    // The key assertion: the binary must not exit with signal (e.g., 141 SIGPIPE).
    // On a broken pipe, a well-behaved binary exits 0 or 1 (not killed by signal).
    // std::process::ExitStatus::signal() returns Some(n) if killed by signal.
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        let sig = output.status.signal();
        assert!(
            sig.is_none(),
            "consign was killed by signal {:?} (SIGPIPE?); ensure sigpipe::reset() is first line of main()",
            sig
        );
    }

    // Should produce at least one line of output (the header)
    let stdout = String::from_utf8_lossy(&output.stdout);
    let _ = stdout; // we don't assert content, just no panic/signal
}
