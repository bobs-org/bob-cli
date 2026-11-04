//! Transfer shell guard: `>` eaten by the shell must refuse before mutation.
//!
//! Phase transfer-lexer wires only the stdout regular-file guard; full
//! transfer execution lands in later phases. These CLI tests prove the
//! guard refuses when stdout is a regular `@` file and stays quiet for
//! normal files and pipes.

use crate::support::*;
use std::fs;
use std::process::Stdio;

#[test]
fn shell_guard_refuses_at_redirect_before_mutation() {
    let temp = TempDir::new("bob-cli-transfer-shell-guard");
    let vault = temp.path().join("vault");
    let day_file = vault.join("day.md");
    write_file(&day_file, "## Pomodoros\n\n- [ ] Current (0900-0930)\n");
    let before = fs::read_to_string(&day_file).expect("read day");
    let at_file = temp.path().join("@c");
    let out_file = fs::File::create(&at_file).expect("create @c");
    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("jot")
        .env("BOB_DAY_FILE", &day_file)
        .env("BOB_NOW", "2026-07-10 13:40:00")
        .stdout(Stdio::from(out_file))
        .output()
        .expect("run capture with @ stdout");
    assert!(
        !output.status.success(),
        "guard must refuse: {}",
        format_output(&output)
    );
    let err = stderr(&output);
    assert!(
        err.contains("your shell ate"),
        "canonical guard copy on stderr: {err}"
    );
    assert!(
        err.contains("quote the draft"),
        "guard teaches quoting: {err}"
    );
    assert_eq!(
        fs::read_to_string(&day_file).expect("read day after"),
        before,
        "vault unchanged when the shell ate `>`"
    );
}

#[test]
fn shell_guard_lets_normal_files_and_pipes_through() {
    let temp = TempDir::new("bob-cli-transfer-shell-guard-ok");
    let vault = temp.path().join("vault");
    let day_file = vault.join("day.md");
    write_file(&day_file, "## Pomodoros\n\n- [ ] Current (0900-0930)\n");
    let out_path = temp.path().join("out.txt");
    let out_file = fs::File::create(&out_path).expect("create out");
    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("jot")
        .env("BOB_DAY_FILE", &day_file)
        .env("BOB_NOW", "2026-07-10 13:40:00")
        .stdout(Stdio::from(out_file))
        .output()
        .expect("run capture with normal file stdout");
    assert_success(&output);
    let piped = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("-f")
        .arg("json")
        .arg("pipe")
        .arg("ok")
        .env("BOB_DAY_FILE", &day_file)
        .env("BOB_NOW", "2026-07-10 13:40:00")
        .output()
        .expect("run capture with piped stdout");
    assert_success(&piped);
}
