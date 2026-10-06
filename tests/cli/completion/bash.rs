//! Real-bash coverage for the values-only `bob` bash adapter.
//!
//! Each case runs `bash --norc --noprofile` with the test-built `bob` on
//! `PATH`, sets `COMP_LINE`/`COMP_POINT`/`COMP_WORDS`/`COMP_CWORD` the way
//! bash splits them (including `:` wordbreak splits and quoted words),
//! calls `_bob`, and asserts `COMPREPLY`. Lifecycle cases use a fake
//! `bash` probe so no test touches a real interactive shell.

use crate::support::*;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;

const BOB_NOW: &str = "2026-06-01 09:10:01";

fn adapter_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("src/native/completion/adapters/bob.bash")
}

fn have_bash() -> bool {
    let ok = Command::new("bash")
        .arg("--version")
        .output()
        .map(|output| output.status.success())
        .unwrap_or(false);
    if !ok {
        println!("skipped: bash not found on PATH");
    }
    ok
}

fn fixture_vault(temp: &TempDir) -> PathBuf {
    let vault = temp.path().join("vault");
    write_capture_task_settings(&vault);
    write_file(
        &vault.join("dev.md"),
        concat!(
            "---\n",
            "type: [[project]]\n",
            "status: active\n",
            "---\n",
            "# Dev\n",
            "\n",
            "- [ ] #task Ship the release ^ship-it\n",
            "- [ ] #task Remote power ^remote-power\n",
        ),
    );
    write_file(
        &vault.join("cash.md"),
        concat!(
            "---\n",
            "type: \"[[area]]\"\n",
            "---\n",
            "# Cash\n",
            "\n",
            "## Shopping\n",
            "\n",
            "- [ ] #task Buy oat milk ^buy-milk\n",
        ),
    );
    vault
}

/// Run one real-bash completion case.
///
/// `comp_line` is the full command line, `comp_point` the cursor offset in
/// characters, `comp_words` the bash-split words, and `comp_cword` the
/// cursor word index. Returns the `COMPREPLY` entries, one per line.
fn run_bash_case(
    vault: &Path,
    comp_line: &str,
    comp_point: usize,
    comp_words: &[&str],
    comp_cword: usize,
) -> Vec<String> {
    let temp = TempDir::new("bob-bash-case");
    let bin = temp.path().join("bin");
    fs::create_dir_all(&bin).expect("create bin");
    std::os::unix::fs::symlink(BOB_BIN, bin.join("bob")).expect("link bob");
    let q = |value: &str| shell_single_quote(value);
    let mut script = String::new();
    script.push_str("export LC_ALL=C.UTF-8 LANG=C.UTF-8\n");
    script.push_str(&format!(
        "export PATH={}:${{PATH}}\n",
        q(&bin.display().to_string())
    ));
    script.push_str(&format!(
        "export BOB_DIR={}\n",
        q(&vault.display().to_string())
    ));
    script.push_str(&format!("export BOB_NOW={}\n", q(BOB_NOW)));
    script.push_str(&format!(
        "export BOB_CONFIG_FILE={}\n",
        q(TEST_MISSING_CONFIG_FILE)
    ));
    script.push_str(&format!(
        "export BOB_WEB_CLIP_ADAPTER={}\n",
        q(TEST_MISSING_WEB_CLIP_ADAPTER)
    ));
    script.push_str(&format!(
        "source {}\n",
        q(&adapter_path().display().to_string())
    ));
    script.push_str(&format!("COMP_LINE={}\n", q(comp_line)));
    script.push_str(&format!("COMP_POINT={comp_point}\n"));
    script.push_str("COMP_WORDS=(");
    for word in comp_words {
        script.push_str(&q(word));
        script.push(' ');
    }
    script.push_str(")\n");
    script.push_str(&format!("COMP_CWORD={comp_cword}\n"));
    script.push_str("_bob\n");
    script.push_str("printf '%s\\n' \"${COMPREPLY[@]}\"\n");
    let output = Command::new("bash")
        .args(["--norc", "--noprofile", "-c", &script])
        .output()
        .expect("run bash case");
    assert_success(&output);
    stdout(&output)
        .lines()
        .map(str::to_string)
        .filter(|line| !line.is_empty())
        .collect()
}

#[test]
fn bash_completes_command_prefix() {
    if !have_bash() {
        return;
    }
    let temp = TempDir::new("bob-bash-cap");
    let vault = fixture_vault(&temp);
    let reply = run_bash_case(&vault, "bob cap", 7, &["bob", "cap"], 1);
    assert!(
        reply.contains(&"capture".to_string()),
        "bob cap offers capture: {reply:?}"
    );
    assert!(
        !reply.iter().any(|value| value.starts_with('-')),
        "command slot offers no options: {reply:?}"
    );
}

#[test]
fn bash_completes_attached_format_value() {
    if !have_bash() {
        return;
    }
    let temp = TempDir::new("bob-bash-format");
    let vault = fixture_vault(&temp);
    // Bash splits at `=`: the broken words are `(--format=, j)`.
    let reply = run_bash_case(
        &vault,
        "bob capture --format=j",
        22,
        &["bob", "capture", "--format=", "j"],
        3,
    );
    assert_eq!(reply, vec!["json".to_string()], "attached value: {reply:?}");
}

#[test]
fn bash_reassembles_colon_split_marker() {
    if !have_bash() {
        return;
    }
    let temp = TempDir::new("bob-bash-colon");
    let vault = fixture_vault(&temp);
    // A solo `@dev:` links existing tasks; bash splits it at `:` and
    // the adapter rebuilds from COMP_LINE.
    let line = "bob capture @dev:";
    let reply = run_bash_case(
        &vault,
        line,
        line.chars().count(),
        &["bob", "capture", "@dev", ":"],
        3,
    );
    assert!(
        reply.contains(&"ship-it".to_string()),
        "colon split offers ship-it: {reply:?}"
    );
    assert!(
        reply.contains(&"remote-power".to_string()),
        "colon split offers remote-power: {reply:?}"
    );
    assert!(
        !reply.iter().any(|value| value.contains('@')),
        "wordbreak-safe replies strip the already-typed prefix: {reply:?}"
    );
}

#[test]
fn bash_unquotes_single_quoted_word() {
    if !have_bash() {
        return;
    }
    let temp = TempDir::new("bob-bash-quoted");
    let vault = fixture_vault(&temp);
    // One open-quoted shell word carrying its own `fix it ` prefix
    // (!prefix 7); body-bearing text suggests a new ID. Readline
    // replaces the whole quoted text, so the reply keeps the prefix.
    let line = "bob capture 'fix it @dev:";
    let reply = run_bash_case(
        &vault,
        line,
        line.chars().count(),
        &["bob", "capture", "fix it @dev:"],
        2,
    );
    assert_eq!(
        reply,
        vec!["fix it @dev:fix".to_string()],
        "open quote keeps prefix plus new ID: {reply:?}"
    );
}

#[test]
fn bash_open_quote_completes_route() {
    if !have_bash() {
        return;
    }
    let temp = TempDir::new("bob-bash-open-route");
    let vault = fixture_vault(&temp);
    let line = "bob capture 'fix it @ca";
    let reply = run_bash_case(
        &vault,
        line,
        line.chars().count(),
        &["bob", "capture", "fix it @ca"],
        2,
    );
    assert!(
        reply.contains(&"fix it @cash".to_string()),
        "open quote keeps prefix plus route: {reply:?}"
    );
}

#[test]
fn bash_counts_unicode_prefix_in_chars() {
    if !have_bash() {
        return;
    }
    let temp = TempDir::new("bob-bash-unicode");
    let vault = fixture_vault(&temp);
    // `café ` is 5 characters (é is one scalar); bob answers !prefix 5.
    let line = "bob capture 'café @dev:";
    let reply = run_bash_case(
        &vault,
        line,
        line.chars().count(),
        &["bob", "capture", "café @dev:"],
        2,
    );
    assert!(
        reply.contains(&"café @dev:caf".to_string()),
        "unicode prefix keeps chars plus new ID: {reply:?}"
    );
}

#[test]
fn bash_strips_every_wordbreak_not_just_colon() {
    if !have_bash() {
        return;
    }
    let temp = TempDir::new("bob-bash-wordbreak");
    let vault = e2e_vault(&temp);
    // Bash splits `=#` at `=`; readline replaces only `#`, so the reply
    // strips the `=` instead of doubling it.
    let line = "bob capture =#";
    let reply = run_bash_case(
        &vault,
        line,
        line.chars().count(),
        &["bob", "capture", "=", "#"],
        3,
    );
    assert!(
        reply.iter().any(|value| value.ends_with("#deep-work")),
        "equals wordbreak strips like colon does: {reply:?}"
    );
    assert!(
        !reply.iter().any(|value| value.contains("=#")),
        "no doubled marker: {reply:?}"
    );
}

#[test]
fn bash_keeps_spaces_in_one_argument() {
    if !have_bash() {
        return;
    }
    let temp = TempDir::new("bob-bash-spaces");
    let vault = e2e_vault(&temp);
    let line = "bob capture --route cash --task fix-sink --task-section F";
    let reply = run_bash_case(
        &vault,
        line,
        line.chars().count(),
        &[
            "bob",
            "capture",
            "--route",
            "cash",
            "--task",
            "fix-sink",
            "--task-section",
            "F",
        ],
        7,
    );
    assert!(
        reply.iter().any(|value| value.contains("FIRST\\ STEPS")),
        "task section stays one escaped argument: {reply:?}"
    );
}

#[test]
fn bash_completes_attached_files_in_value() {
    if !have_bash() {
        return;
    }
    let temp = TempDir::new("bob-bash-files-in");
    let vault = e2e_vault(&temp);
    // Bash splits `--tasks-note=cash` at `=`; the note filter reads the
    // text after the kept prefix, not the whole prefix.
    let line = "bob query --tasks-note=cash";
    let reply = run_bash_case(
        &vault,
        line,
        line.chars().count(),
        &["bob", "query", "--tasks-note=", "cash"],
        3,
    );
    assert!(
        reply.contains(&"cash.md".to_string()),
        "attached files-in value completes: {reply:?}"
    );
}

#[test]
fn bash_offers_nothing_once_text_starts() {
    if !have_bash() {
        return;
    }
    let temp = TempDir::new("bob-bash-text");
    let vault = fixture_vault(&temp);
    let line = "bob capture fix --r";
    let reply = run_bash_case(
        &vault,
        line,
        line.chars().count(),
        &["bob", "capture", "fix", "--r"],
        3,
    );
    assert!(reply.is_empty(), "TEXT slot offers no options: {reply:?}");
}

#[test]
fn bash_maps_dirs_directive() {
    if !have_bash() {
        return;
    }
    let temp = TempDir::new("bob-bash-dirs");
    let vault = fixture_vault(&temp);
    let work = temp.path().join("work");
    fs::create_dir_all(work.join("alpha")).expect("alpha");
    fs::create_dir_all(work.join("beta")).expect("beta");
    let bin = temp.path().join("bin");
    fs::create_dir_all(&bin).expect("bin");
    std::os::unix::fs::symlink(BOB_BIN, bin.join("bob")).expect("link bob");
    let q = |value: &str| shell_single_quote(value);
    let mut script = String::new();
    script.push_str(&format!(
        "export PATH={}:$PATH\n",
        q(&bin.display().to_string())
    ));
    script.push_str(&format!(
        "export BOB_DIR={}\n",
        q(&vault.display().to_string())
    ));
    script.push_str(&format!(
        "source {}\n",
        q(&adapter_path().display().to_string())
    ));
    script.push_str(&format!("cd {}\n", q(&work.display().to_string())));
    script.push_str("COMP_LINE='bob capture --bob-dir '\n");
    script.push_str("COMP_POINT=22\n");
    script.push_str("COMP_WORDS=(bob capture --bob-dir '')\n");
    script.push_str("COMP_CWORD=3\n");
    script.push_str("_bob\n");
    script.push_str("printf '%s\\n' \"${COMPREPLY[@]}\"\n");
    let output = Command::new("bash")
        .args(["--norc", "--noprofile", "-c", &script])
        .output()
        .expect("run dirs case");
    assert_success(&output);
    let reply: Vec<String> = stdout(&output)
        .lines()
        .map(str::to_string)
        .filter(|line| !line.is_empty())
        .collect();
    assert!(
        reply.iter().any(|value| value.contains("alpha")),
        "dirs offer alpha: {reply:?}"
    );
    assert!(
        reply.iter().any(|value| value.contains("beta")),
        "dirs offer beta: {reply:?}"
    );
}

/// Vault for the wordbreak/space/files-in cases and the readline
/// end-to-end test: an area with one sectioned task, a project with one
/// task (so solo `@dev:` is unambiguous), and a daily note with open
/// Pomodoros.
fn e2e_vault(temp: &TempDir) -> PathBuf {
    let vault = temp.path().join("vault");
    write_capture_task_settings(&vault);
    write_file(
        &vault.join("cash.md"),
        concat!(
            "---\n",
            "type: \"[[area]]\"\n",
            "---\n",
            "# Cash\n",
            "\n",
            "## Shopping\n",
            "\n",
            "- [ ] #task Buy oat milk ^buy-milk\n",
            "- [ ] #task Fix the sink ^fix-sink\n",
            "  - FIRST STEPS\n",
            "    - gather parts\n",
        ),
    );
    write_file(
        &vault.join("dev.md"),
        concat!(
            "---\n",
            "type: [[project]]\n",
            "status: active\n",
            "---\n",
            "# Dev\n",
            "\n",
            "- [ ] #task Ship the release ^ship-it\n",
        ),
    );
    write_file(
        &vault.join("2026/20260601.md"),
        concat!(
            "# 2026-06-01\n",
            "\n",
            "## Pomodoros\n",
            "\n",
            "- [ ] (0900-0930) Morning pages\n",
            "- [ ] () — DEEP WORK\n",
        ),
    );
    vault
}

fn have_zsh() -> bool {
    let ok = Command::new("zsh")
        .arg("--version")
        .output()
        .map(|output| output.status.success())
        .unwrap_or(false);
    if !ok {
        println!("skipped: zsh not found on PATH");
    }
    ok
}

/// Real interactive bash through zsh's `zpty` module: TAB inserts exactly
/// what bob returned, and `printf '<%s>'` proves the resulting shell
/// words. Each row types the line, sends TAB, then moves to column zero
/// and prefixes `printf '<%s>\n'` before Enter.
#[test]
fn bash_readline_inserts_what_bob_returned() {
    if !have_zsh() || !have_bash() {
        return;
    }
    let temp = TempDir::new("bob-bash-readline");
    let vault = e2e_vault(&temp);
    let dir = temp.path();
    let bin = dir.join("bin");
    fs::create_dir_all(&bin).expect("create bin");
    std::os::unix::fs::symlink(BOB_BIN, bin.join("bob")).expect("link bob");
    write_file(&dir.join("driver.zsh"), READLINE_DRIVER);
    let timeout = Command::new("timeout")
        .arg("--version")
        .output()
        .map(|output| output.status.success())
        .unwrap_or(false);
    let mut command = if timeout {
        let mut with_timeout = Command::new("timeout");
        with_timeout.arg("180").arg("zsh");
        with_timeout
    } else {
        Command::new("zsh")
    };
    let path = format!(
        "{}:{}",
        bin.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let output = command
        .arg("-f")
        .arg(dir.join("driver.zsh"))
        .arg(dir)
        .arg(adapter_path())
        .env("PATH", &path)
        .env("BOB_DIR", &vault)
        .env("BOB_NOW", BOB_NOW)
        .env("BOB_CONFIG_FILE", TEST_MISSING_CONFIG_FILE)
        .env("BOB_WEB_CLIP_ADAPTER", TEST_MISSING_WEB_CLIP_ADAPTER)
        .env("INPUTRC", "/dev/null")
        .env("TERM", "dumb")
        .output()
        .expect("run readline driver");
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    assert!(
        output.status.success(),
        "readline driver failed, stdout:\n{stdout}\nstderr:\n{stderr}"
    );
    for marker in [
        "SETUP OK",
        "EQUALS OK",
        "QUOTE OK",
        "SPACES OK",
        "FILESIN OK",
        "FORMAT OK",
        "LINK OK",
        "ROUTE OK",
    ] {
        assert!(
            stdout.contains(marker),
            "missing {marker:?}, stdout:\n{stdout}\nstderr:\n{stderr}"
        );
    }
}

/// One pty session covering every readline insertion row. The driver
/// inherits the fixture env from the test process; `set +H` keeps `=`
/// words literal and `shopt -u interactive_comments` keeps `#` words
/// literal, so the assertions see completion, not shell parsing.
const READLINE_DRIVER: &str = r#"
zmodload zsh/zpty || { print "ZPTY-MISSING"; exit 2 }
TMP=$1
ADAPTER=$2
shift 2
zpty -d pb 2>/dev/null
zpty pb bash --norc --noprofile -i
sleep 1
zpty -w pb "PS1='PB> '"
sleep 0.3
zpty -w pb "set +H; shopt -u interactive_comments; source \"$ADAPTER\""
sleep 0.3
zpty -r pb setup '*PB>*' || { print -r -- "SETUP-READ-FAILED"; zpty -d pb; exit 1 }
print -r -- "SETUP OK"
# `bob capture =#` inserts the named start without doubling `=`.
zpty -w -n pb $'bob capture =#\t'
sleep 1
zpty -w -n pb $'\x01'
sleep 0.3
zpty -w -n pb "printf '<%s>\\n' "
sleep 0.3
zpty -w -n pb $'\r'
sleep 0.5
zpty -w -n pb $'\x03'
sleep 0.3
if zpty -r pb equals '*<=#deep-work>*' 2>/dev/null; then print -r -- "EQUALS OK"; else print -r -- "EQUALS MISS"; fi
zpty -w -n pb $'\x03'
sleep 0.5
# An open quote completes inside the quotes; readline closes them.
zpty -w -n pb $'bob capture \'fix it @ca\t'
sleep 1
zpty -w -n pb $'\x01'
sleep 0.3
zpty -w -n pb "printf '<%s>\\n' "
sleep 0.3
zpty -w -n pb $'\r'
sleep 0.5
zpty -w -n pb $'\x03'
sleep 0.3
if zpty -r pb quote '*<fix it @cash>*' 2>/dev/null; then print -r -- "QUOTE OK"; else print -r -- "QUOTE MISS"; fi
zpty -w -n pb $'\x03'
sleep 0.5
# A value with a space stays one argument.
zpty -w -n pb $'bob capture --route cash --task fix-sink --task-section F\t'
sleep 1
zpty -w -n pb $'\x01'
sleep 0.3
zpty -w -n pb "printf '<%s>\\n' "
sleep 0.3
zpty -w -n pb $'\r'
sleep 0.5
zpty -w -n pb $'\x03'
sleep 0.3
if zpty -r pb spaces '*<FIRST STEPS>*' 2>/dev/null; then print -r -- "SPACES OK"; else print -r -- "SPACES MISS"; fi
zpty -w -n pb $'\x03'
sleep 0.5
# An attached `!files-in` value completes the note name.
zpty -w -n pb $'bob query --tasks-note=c\t'
sleep 1
zpty -w -n pb $'\x01'
sleep 0.3
zpty -w -n pb "printf '<%s>\\n' "
sleep 0.3
zpty -w -n pb $'\r'
sleep 0.5
zpty -w -n pb $'\x03'
sleep 0.3
if zpty -r pb filesin '*<--tasks-note=cash.md>*' 2>/dev/null; then print -r -- "FILESIN OK"; else print -r -- "FILESIN MISS"; fi
zpty -w -n pb $'\x03'
sleep 0.5
# Attached option values keep working, as does the solo link form.
zpty -w -n pb $'bob capture --format=j\t'
sleep 1
zpty -w -n pb $'\x01'
sleep 0.3
zpty -w -n pb "printf '<%s>\\n' "
sleep 0.3
zpty -w -n pb $'\r'
sleep 0.5
zpty -w -n pb $'\x03'
sleep 0.3
if zpty -r pb format '*<--format=json>*' 2>/dev/null; then print -r -- "FORMAT OK"; else print -r -- "FORMAT MISS"; fi
zpty -w -n pb $'\x03'
sleep 0.5
zpty -w -n pb $'bob capture @dev:\t'
sleep 1
zpty -w -n pb $'\x01'
sleep 0.3
zpty -w -n pb "printf '<%s>\\n' "
sleep 0.3
zpty -w -n pb $'\r'
sleep 0.5
zpty -w -n pb $'\x03'
sleep 0.3
if zpty -r pb link '*<@dev:ship-it>*' 2>/dev/null; then print -r -- "LINK OK"; else print -r -- "LINK MISS"; fi
zpty -w -n pb $'\x03'
sleep 0.5
zpty -w -n pb $'bob capture --route ca\t'
sleep 1
zpty -w -n pb $'\x01'
sleep 0.3
zpty -w -n pb "printf '<%s>\\n' "
sleep 0.3
zpty -w -n pb $'\r'
sleep 0.5
zpty -w -n pb $'\x03'
sleep 0.3
if zpty -r pb route1 '*<--route>*' 2>/dev/null && zpty -r pb route2 '*<cash>*' 2>/dev/null; then print -r -- "ROUTE OK"; else print -r -- "ROUTE MISS"; fi
zpty -d pb
"#;

// --- lifecycle -----------------------------------------------------------

fn fakebin(temp: &TempDir) -> PathBuf {
    let dir = temp.path().join("fakebin");
    fs::create_dir_all(&dir).expect("create fakebin");
    let script = r#"#!/bin/sh
for last in "$@"; do :; done
script="$last"
case "$script" in
    *bob-completion-verify-probe-bash*)
        case "$FAKE_VERIFY" in
            sleep) sleep 5 ;;
            broken) printf 'rc noise without markers\n' ;;
            *)
                printf 'bob-verify-start\n'
                case "$FAKE_VERIFY" in
                    registered) printf 'complete -F _bob bob\n' ;;
                    bound) printf 'complete -F _other bob\n' ;;
                    *) printf '\n' ;;
                esac
                printf 'bob-verify-end\n'
                ;;
        esac
        ;;
    *) printf 'bob-verify-start\nbob-verify-end\n' ;;
esac
"#;
    let path = dir.join("bash");
    fs::write(&path, script).expect("write fake bash");
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755))
        .expect("chmod fake bash");
    dir
}

fn completion_command(temp: &TempDir, fake: &Path) -> Command {
    let mut command = Command::new(BOB_BIN);
    command
        .env("HOME", temp.path())
        .env("XDG_STATE_HOME", temp.path().join("state"))
        .env("XDG_DATA_HOME", temp.path().join("data"))
        .env("SHELL", "/bin/bash")
        .env_remove("BASH_COMPLETION_USER_DIR")
        .env_remove("BOB_COMPLETION_PROBE_TIMEOUT_MS")
        .env_remove("FAKE_VERIFY");
    let path = format!(
        "{}:{}",
        fake.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    command.env("PATH", path);
    command.arg("completion");
    command
}

#[test]
fn bash_install_status_uninstall_round_trip() {
    let temp = TempDir::new("bob-cli-bash-lifecycle");
    let fake = fakebin(&temp);
    let target = temp.path().join("bcomp");

    let install = completion_command(&temp, &fake)
        .args(["install", "bash", "-n", "-t"])
        .arg(&target)
        .output()
        .expect("run bash install");
    assert_success(&install);
    assert!(
        stdout(&install).contains("installed · protocol 1"),
        "bash install:\n{}",
        format_output(&install)
    );
    let adapter = target.join("bob");
    assert!(adapter.is_file(), "bash adapter written");

    let again = completion_command(&temp, &fake)
        .args(["install", "bash", "-n", "-t"])
        .arg(&target)
        .output()
        .expect("run bash install again");
    assert_success(&again);
    assert!(
        stdout(&again).contains("unchanged"),
        "bash idempotent:\n{}",
        format_output(&again)
    );

    let status = completion_command(&temp, &fake)
        .args(["status", "-j"])
        .output()
        .expect("run bash status");
    assert_success(&status);
    let value: serde_json::Value =
        serde_json::from_str(stdout(&status).trim()).expect("status JSON");
    let bash = value["shells"]
        .as_array()
        .expect("shells array")
        .iter()
        .find(|entry| entry["shell"] == "bash")
        .expect("bash entry present");
    assert_eq!(bash["state"], "current");

    let remove = completion_command(&temp, &fake)
        .args(["uninstall", "bash"])
        .output()
        .expect("run bash uninstall");
    assert_success(&remove);
    assert!(!adapter.exists(), "bash adapter removed");
}

#[test]
fn bash_default_target_honors_env() {
    let temp = TempDir::new("bob-cli-bash-target");
    let fake = fakebin(&temp);
    // BASH_COMPLETION_USER_DIR wins over XDG_DATA_HOME.
    let user_dir = temp.path().join("userdir");
    let out = completion_command(&temp, &fake)
        .args(["install", "bash", "-n", "-d"])
        .env("BASH_COMPLETION_USER_DIR", &user_dir)
        .output()
        .expect("run bash dry install");
    assert_success(&out);
    let text = stdout(&out);
    assert!(
        text.contains("userdir/completions/bob"),
        "user dir target:\n{text}"
    );
    assert!(text.contains("bash-completion user dir"), "reason:\n{text}");

    // Without the override the XDG data home is used.
    let out = completion_command(&temp, &fake)
        .args(["install", "bash", "-n", "-d"])
        .output()
        .expect("run bash default install");
    assert_success(&out);
    assert!(
        stdout(&out).contains("data/bash-completion/completions/bob"),
        "xdg target:\n{}",
        format_output(&out)
    );
}

#[test]
fn bash_verify_reports_source_remedy() {
    let temp = TempDir::new("bob-cli-bash-verify");
    let fake = fakebin(&temp);
    let target = temp.path().join("bcomp");
    let install = completion_command(&temp, &fake)
        .args(["install", "bash", "-n", "-t"])
        .arg(&target)
        .output()
        .expect("run bash install");
    assert_success(&install);

    let live = completion_command(&temp, &fake)
        .args(["status", "-v"])
        .env("FAKE_VERIFY", "off")
        .output()
        .expect("run bash status -v");
    assert_eq!(
        live.status.code(),
        Some(1),
        "unregistered bash fails -v:\n{}",
        format_output(&live)
    );
    assert!(
        stdout(&live).contains("source "),
        "source remedy:\n{}",
        format_output(&live)
    );
    assert!(
        stdout(&live).contains("~/.bashrc"),
        "bashrc remedy:\n{}",
        format_output(&live)
    );
}

#[test]
fn bash_shell_selection() {
    let temp = TempDir::new("bob-cli-bash-select");
    let fake = fakebin(&temp);
    let mut command = completion_command(&temp, &fake);
    command
        .args(["install", "-n", "-d"])
        .env("SHELL", "/bin/bash");
    let out = command.output().expect("run bash-selected install");
    assert_success(&out);
    assert!(
        stdout(&out).contains("bash"),
        "SHELL=/bin/bash selects bash:\n{}",
        format_output(&out)
    );
}

#[test]
fn bash_filters_create_target_by_extension() {
    if !have_bash() {
        return;
    }
    let temp = TempDir::new("bob-bash-target-filter");
    let vault = fixture_vault(&temp);
    let work = temp.path().join("work");
    fs::create_dir_all(&work).expect("work");
    write_file(&work.join("a.md"), "# A\n");
    write_file(&work.join("b.pdf"), "%PDF-1.4\n");
    write_file(&work.join("c.txt"), "plain\n");
    let bin = temp.path().join("bin");
    fs::create_dir_all(&bin).expect("bin");
    std::os::unix::fs::symlink(BOB_BIN, bin.join("bob")).expect("link bob");
    let q = |value: &str| shell_single_quote(value);
    let mut script = String::new();
    script.push_str("export LC_ALL=C.UTF-8 LANG=C.UTF-8\n");
    script.push_str(&format!(
        "export PATH={}:${{PATH}}\n",
        q(&bin.display().to_string())
    ));
    script.push_str(&format!(
        "export BOB_DIR={}\n",
        q(&vault.display().to_string())
    ));
    script.push_str(&format!("export BOB_NOW={}\n", q(BOB_NOW)));
    script.push_str(&format!(
        "export BOB_CONFIG_FILE={}\n",
        q(TEST_MISSING_CONFIG_FILE)
    ));
    script.push_str(&format!(
        "export BOB_WEB_CLIP_ADAPTER={}\n",
        q(TEST_MISSING_WEB_CLIP_ADAPTER)
    ));
    script.push_str(&format!(
        "source {}\n",
        q(&adapter_path().display().to_string())
    ));
    script.push_str(&format!("cd {}\n", q(&work.display().to_string())));
    script.push_str("COMP_LINE='bob highlights create '\n");
    script.push_str("COMP_POINT=22\n");
    script.push_str("COMP_WORDS=(bob highlights create '')\n");
    script.push_str("COMP_CWORD=3\n");
    script.push_str("_bob\n");
    script.push_str("printf '%s\\n' \"${COMPREPLY[@]}\"\n");
    let output = Command::new("bash")
        .args(["--norc", "--noprofile", "-c", &script])
        .output()
        .expect("run target filter case");
    assert_success(&output);
    let reply: Vec<String> = stdout(&output)
        .lines()
        .map(str::to_string)
        .filter(|line| !line.is_empty())
        .collect();
    assert!(
        reply.iter().any(|value| value.ends_with("a.md")),
        "create TARGET keeps a.md: {reply:?}"
    );
    assert!(
        reply.iter().any(|value| value.ends_with("b.pdf")),
        "create TARGET keeps b.pdf: {reply:?}"
    );
    assert!(
        !reply.iter().any(|value| value.ends_with("c.txt")),
        "create TARGET drops c.txt: {reply:?}"
    );
}

#[test]
fn bash_print_matches_installed_bytes() {
    let temp = TempDir::new("bob-cli-bash-print");
    let fake = fakebin(&temp);
    let printed = completion_command(&temp, &fake)
        .arg("bash")
        .output()
        .expect("run bash print");
    assert_success(&printed);
    let text = stdout(&printed);
    assert!(
        text.contains("Generated by bob completion (protocol 1)"),
        "bash stamp:\n{text}"
    );
    assert!(
        text.contains("complete -F _bob bob"),
        "bash registration:\n{text}"
    );
    assert_stdout_has_no_ansi(&printed);
}
