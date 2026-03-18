use assert_cmd::Command;
use predicates::prelude::*;

fn cmd() -> Command {
    Command::cargo_bin("bm-complete").expect("binary should be built")
}

#[test]
fn index_then_complete() {
    let dir = tempfile::tempdir().unwrap();
    let fish_file = dir.path().join("git.fish");
    std::fs::write(
        &fish_file,
        "complete -c git -l commit -d 'Record changes'\ncomplete -c git -l push -d 'Update remote'\n",
    )
    .unwrap();

    // Index from the temp fish dir
    cmd()
        .args(["index", "--fish-dir", dir.path().to_str().unwrap()])
        .assert()
        .success()
        .stdout(predicate::str::contains("indexing complete"));

    // Complete should return something (may include path completions too)
    cmd()
        .args(["complete", "--buffer", "git co"])
        .assert()
        .success();
}

#[test]
fn complete_empty_buffer() {
    cmd()
        .args(["complete", "--buffer", ""])
        .assert()
        .success()
        .stdout(predicate::str::is_empty());
}

#[test]
fn status_no_daemon() {
    let dir = tempfile::tempdir().unwrap();
    let socket = dir.path().join("nonexistent.socket");

    cmd()
        .args(["status", "--socket", socket.to_str().unwrap()])
        .assert()
        .success()
        .stdout(predicate::str::contains("daemon not running"));
}

#[test]
fn index_nonexistent_dir() {
    cmd()
        .args([
            "index",
            "--fish-dir",
            "/tmp/bm-complete-test-nonexistent-dir",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("indexing complete"));
}
