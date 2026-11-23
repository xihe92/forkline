//! The `forkline-plan check` binary.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, missing_docs)]

use std::path::PathBuf;
use std::process::Command;

const BIN: &str = env!("CARGO_BIN_EXE_forkline-plan");

fn write(name: &str, text: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("forkline-plan-cli-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(name);
    std::fs::write(&path, text).unwrap();
    path
}

const GOOD: &str = "[plans.p]\nroot = \"a\"\nterminal = \"e\"\nlinks = [\"a -> b\"]\n\
    [plans.p.nodes]\na = { module = \"A\" }\nb = { module = \"B\" }\ne = { module = \"E\" }\n";

#[test]
fn check_reports_each_file_and_fails_if_any_is_bad() {
    let good = write("good.toml", GOOD);
    let out = Command::new(BIN).arg("check").arg(&good).output().unwrap();
    assert!(out.status.success());
    assert_eq!(String::from_utf8_lossy(&out.stdout), format!("ok: {} (1 plans)\n", good.display()));

    let bad = write("bad.toml", &GOOD.replace("\"a -> b\"", "\"a -> b\", \"b -> a\""));
    let missing = PathBuf::from("/nonexistent/plans.toml");
    let out = Command::new(BIN).arg("check").arg(&good).arg(&bad).arg(&missing).output().unwrap();
    assert_eq!(out.status.code(), Some(1));
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stdout.contains("ok: "), "the good file is still reported: {stdout}");
    assert!(stderr.contains(&format!("error: {}: plan `p`: the links form a cycle", bad.display())), "{stderr}");
    assert!(stderr.contains("/nonexistent/plans.toml"), "{stderr}");

    let out = Command::new(BIN).arg("check").output().unwrap();
    assert_eq!(out.status.code(), Some(2), "at least one file is required");
}
