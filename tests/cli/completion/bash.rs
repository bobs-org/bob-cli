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
    let line = "bob capture fix @dev:";
    // Bash splits `@dev:` at `:`; the adapter rebuilds from COMP_LINE.
    let reply = run_bash_case(
        &vault,
        line,
        line.chars().count(),
        &["bob", "capture", "fix", "@dev", ":"],
        4,
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
    // One quoted shell word carrying its own `fix it ` prefix (!prefix 7).
    let line = "bob capture 'fix it @dev:";
    let reply = run_bash_case(
        &vault,
        line,
        line.chars().count(),
        &["bob", "capture", "fix it @dev:"],
        2,
    );
    assert!(
        reply.contains(&"ship-it".to_string()),
        "quoted word offers ship-it: {reply:?}"
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
    let line = "bob capture café @dev:";
    let reply = run_bash_case(
        &vault,
        line,
        line.chars().count(),
        &["bob", "capture", "café", "@dev", ":"],
        4,
    );
    assert!(
        reply.contains(&"ship-it".to_string()),
        "unicode prefix offers ship-it: {reply:?}"
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
