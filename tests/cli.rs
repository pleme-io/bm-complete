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

#[test]
fn complete_with_explicit_position() {
    cmd()
        .args(["complete", "--buffer", "git commit", "--position", "3"])
        .assert()
        .success();
}

#[test]
fn complete_flag_prefix_buffer() {
    let dir = tempfile::tempdir().unwrap();
    let fish_file = dir.path().join("git.fish");
    std::fs::write(
        &fish_file,
        "complete -c git -l verbose -d 'Be verbose'\n",
    )
    .unwrap();

    cmd()
        .args(["index", "--fish-dir", dir.path().to_str().unwrap()])
        .assert()
        .success();

    cmd()
        .args(["complete", "--buffer", "git --ver"])
        .assert()
        .success();
}

#[test]
fn index_multiple_fish_files() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("git.fish"),
        "complete -c git -l commit\ncomplete -c git -l push\n",
    )
    .unwrap();
    std::fs::write(
        dir.path().join("cargo.fish"),
        "complete -c cargo -l build\ncomplete -c cargo -l test\n",
    )
    .unwrap();

    cmd()
        .args(["index", "--fish-dir", dir.path().to_str().unwrap()])
        .assert()
        .success()
        .stdout(predicate::str::contains("indexed"))
        .stdout(predicate::str::contains("indexing complete"));
}

#[test]
fn complete_unknown_command() {
    cmd()
        .args(["complete", "--buffer", "nonexistent_cmd_xyz "])
        .assert()
        .success();
}

#[test]
fn status_with_default_socket() {
    cmd()
        .args(["status", "--socket", "/tmp/bm-complete-test-no-socket"])
        .assert()
        .success()
        .stdout(predicate::str::contains("daemon not running"));
}

#[test]
fn help_flag() {
    cmd()
        .args(["--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Shell completion daemon"));
}

#[test]
fn version_flag() {
    cmd()
        .args(["--version"])
        .assert()
        .success()
        .stdout(predicate::str::contains("bm-complete"));
}

#[test]
fn complete_single_word() {
    cmd()
        .args(["complete", "--buffer", "git"])
        .assert()
        .success();
}

#[test]
fn index_empty_dir() {
    let dir = tempfile::tempdir().unwrap();
    cmd()
        .args(["index", "--fish-dir", dir.path().to_str().unwrap()])
        .assert()
        .success()
        .stdout(predicate::str::contains("indexing complete"));
}
