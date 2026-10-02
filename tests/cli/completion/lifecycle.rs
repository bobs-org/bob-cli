//! Lifecycle coverage for `bob completion`.
//!
//! Every test runs with a temporary `HOME`, `XDG_STATE_HOME`,
//! `XDG_DATA_HOME`, `ZDOTDIR`, and `SHELL`, so no test touches Bryan's
//! real dotfiles. Shell probes stay deterministic through fake `zsh` and
//! `bash` binaries on `PATH` that branch on the probe scripts' marker
//! comments.

use crate::support::*;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;

const ADAPTER_HEAD: &str = "#compdef bob";

fn fakebin(temp: &TempDir) -> PathBuf {
    let dir = temp.path().join("fakebin");
    fs::create_dir_all(&dir).expect("create fakebin");
    write_fake_zsh(&dir);
    write_fake_bash(&dir);
    dir
}

fn write_fake_zsh(dir: &Path) {
    let script = r#"#!/bin/sh
# Fake zsh for bob completion lifecycle tests. Branches on the probe
# script's marker comments; $FAKE_FPATH and $FAKE_VERIFY_FPATH are
# colon-separated entry lists. When $FAKE_LOG is set, every probe branch
# appends one line naming the probe kind, so tests can prove a command
# never spawned a shell.
for last in "$@"; do :; done
script="$last"
log_probe() {
    [ -n "$FAKE_LOG" ] && printf '%s\n' "$1" >> "$FAKE_LOG"
}
split_print() {
    old_ifs="$IFS"
    IFS=':'
    # shellcheck disable=SC2086
    for entry in $1; do
        [ -n "$entry" ] && printf '%s\n' "$entry"
    done
    IFS="$old_ifs"
}
print_verify_block() {
    printf 'bob-verify-start\n'
    old_ifs="$IFS"
    IFS=':'
    # shellcheck disable=SC2086
    for entry in $FAKE_VERIFY_FPATH; do
        [ -n "$entry" ] && printf 'bob-fpath-entry=%s\n' "$entry"
    done
    IFS="$old_ifs"
    case "$FAKE_VERIFY" in
        registered)
            printf 'bob-comp=_bob\nbob-source=%s\n' "$FAKE_SOURCE" ;;
        shadowed)
            printf 'bob-comp=_bob\nbob-source=%s\n' "$FAKE_SOURCE" ;;
        tty-read)
            printf 'bob-comp=_bob\nbob-source=%s\n' "$FAKE_SOURCE" ;;
        bound)
            printf 'bob-comp=_other\nbob-source=\n' ;;
        *) printf 'bob-comp=\nbob-source=\n' ;;
    esac
    printf 'bob-verify-end\n'
}
case "$script" in
    *bob-completion-fpath-probe*)
        log_probe "fpath-probe"
        printf 'bob-fpath-start\n'
        split_print "$FAKE_FPATH"
        printf 'bob-fpath-end\n'
        ;;
    *bob-completion-verify-probe*)
        log_probe "verify-probe"
        case "$FAKE_VERIFY" in
            sleep) sleep 5 ;;
            broken) printf 'rc noise without markers\n' ;;
            tty-read)
                # Read from the controlling terminal first: with no
                # controlling terminal (a setsid probe) this fails
                # immediately, but from a background process group with a
                # terminal it stops on SIGTTIN and the probe times out.
                read -r _ < /dev/tty 2>/dev/null
                print_verify_block
                ;;
            *) print_verify_block ;;
        esac
        ;;
    *) printf 'fake zsh: unknown script\n' ;;
esac
"#;
    let path = dir.join("zsh");
    fs::write(&path, script).expect("write fake zsh");
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755))
        .expect("chmod fake zsh");
}

fn write_fake_bash(dir: &Path) {
    // Fake bash for lifecycle tests. Branches on the verify probe marker;
    // the fpath probe is zsh-only and never reaches bash. A `registered`
    // answer requires the adapter file at $FAKE_BASH_ADAPTER to exist, so
    // a not-installed bash is never reported registered. When $FAKE_LOG
    // is set, the verify branch logs one line.
    let script = r#"#!/bin/sh
# Fake bash for bob completion lifecycle tests.
for last in "$@"; do :; done
script="$last"
case "$script" in
    *bob-completion-verify-probe-bash*)
        [ -n "$FAKE_LOG" ] && printf 'verify-probe\n' >> "$FAKE_LOG"
        case "$FAKE_VERIFY" in
            sleep) sleep 5 ;;
            broken) printf 'rc noise without markers\n' ;;
            *)
                printf 'bob-verify-start\n'
                case "$FAKE_VERIFY" in
                    registered)
                        if [ -z "$FAKE_BASH_ADAPTER" ] || [ -f "$FAKE_BASH_ADAPTER" ]; then
                            printf 'complete -F _bob bob\n'
                        else
                            printf '\n'
                        fi
                        ;;
                    shadowed) printf 'complete -F _bob bob\n' ;;
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
}

fn completion_command(temp: &TempDir, fake: &Path) -> Command {
    let mut command = Command::new(BOB_BIN);
    command
        .env("HOME", temp.path())
        .env("XDG_STATE_HOME", temp.path().join("state"))
        .env("XDG_DATA_HOME", temp.path().join("data"))
        .env("ZDOTDIR", temp.path().join("zdot"))
        .env("SHELL", "/bin/zsh")
        .env_remove("ZSH")
        .env_remove("ZSH_CUSTOM")
        .env_remove("BOB_COMPLETION_PROBE_TIMEOUT_MS")
        .env_remove("FAKE_VERIFY")
        .env_remove("FAKE_SOURCE")
        .env_remove("FAKE_FPATH")
        .env_remove("FAKE_VERIFY_FPATH")
        .env_remove("FAKE_LOG")
        .env_remove("FAKE_BASH_ADAPTER")
        .env_remove("BASH_COMPLETION_USER_DIR");
    let path = format!(
        "{}:{}",
        fake.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    command.env("PATH", path);
    command.arg("completion");
    command
}

fn manifest_bytes(temp: &TempDir) -> Option<Vec<u8>> {
    fs::read(
        temp.path()
            .join("state")
            .join("bob-cli")
            .join("completion")
            .join("manifest.json"),
    )
    .ok()
}

fn manifest_json(temp: &TempDir) -> serde_json::Value {
    let bytes = manifest_bytes(temp).expect("manifest exists");
    let value: serde_json::Value =
        serde_json::from_slice(&bytes).expect("manifest parses");
    assert_eq!(value["schema_version"], 1, "manifest schema");
    value
}

fn snapshot_tree(root: &Path) -> Vec<String> {
    let mut entries = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(read) = fs::read_dir(&dir) else {
            continue;
        };
        for entry in read.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path.clone());
            }
            entries.push(
                path.strip_prefix(root)
                    .expect("prefix")
                    .display()
                    .to_string(),
            );
        }
    }
    entries.sort();
    entries
}

fn file_digest(bytes: &[u8]) -> String {
    use sha2::Digest;
    hex::encode(sha2::Sha256::digest(bytes))
}

#[test]
fn first_install_then_idempotent_unchanged() {
    let temp = TempDir::new("bob-cli-completion-install");
    let fake = fakebin(&temp);
    let target = temp.path().join("zfunc");

    let install = completion_command(&temp, &fake)
        .args(["install", "zsh", "-n", "-t"])
        .arg(&target)
        .output()
        .expect("run install");
    assert_success(&install);
    let first = stdout(&install);
    assert!(
        first.contains("installed · protocol 1"),
        "first install:\n{first}"
    );
    assert!(first.contains("~"), "home renders as ~:\n{first}");
    assert_stdout_has_no_ansi(&install);

    let adapter = target.join("_bob");
    let bytes = fs::read(&adapter).expect("adapter written");
    let text = String::from_utf8_lossy(&bytes);
    assert!(text.starts_with(ADAPTER_HEAD), "stamp head");
    assert!(text.contains("protocol 1"), "stamp protocol");

    let again = completion_command(&temp, &fake)
        .args(["install", "zsh", "-n", "-t"])
        .arg(&target)
        .output()
        .expect("run install again");
    assert_success(&again);
    let second = stdout(&again);
    assert!(
        second.contains("unchanged · protocol 1"),
        "idempotent:\n{second}"
    );
    assert!(
        second.contains("registration not checked → bob completion status -v"),
        "no-verify pointer:\n{second}"
    );
    // Never probed, so never registered: no live closer.
    assert!(
        !second.contains("Completion is live"),
        "no live closer without registration:\n{second}"
    );
}

#[test]
fn recorded_verification_survives_into_status() {
    let temp = TempDir::new("bob-cli-completion-recorded");
    let fake = fakebin(&temp);
    let target = temp.path().join("zfunc");
    let adapter = target.join("_bob");

    let install = completion_command(&temp, &fake)
        .args(["install", "zsh", "-t"])
        .arg(&target)
        .env("FAKE_VERIFY", "registered")
        .env("FAKE_SOURCE", &adapter)
        .output()
        .expect("run install with probe");
    assert_success(&install);
    assert!(
        stdout(&install).contains("registered as _bob"),
        "live registration:\n{}",
        format_output(&install)
    );

    let status = completion_command(&temp, &fake)
        .arg("status")
        .output()
        .expect("run status");
    assert_success(&status);
    assert!(
        stdout(&status).contains("registered as _bob"),
        "recorded registration:\n{}",
        format_output(&status)
    );
}

#[test]
fn outdated_adapter_updates_without_force() {
    let temp = TempDir::new("bob-cli-completion-outdated");
    let fake = fakebin(&temp);
    let target = temp.path().join("zfunc");

    let install = completion_command(&temp, &fake)
        .args(["install", "zsh", "-n", "-t"])
        .arg(&target)
        .output()
        .expect("run install");
    assert_success(&install);

    // Simulate an older bob-owned install: stamped bytes that differ from
    // this binary's adapter, with the manifest digest matching the file.
    let adapter = target.join("_bob");
    let mut aged = fs::read(&adapter).expect("read adapter");
    aged.extend_from_slice(b"\n# aged\n");
    fs::write(&adapter, &aged).expect("age adapter");
    let manifest_path = temp
        .path()
        .join("state")
        .join("bob-cli")
        .join("completion")
        .join("manifest.json");
    let mut manifest = manifest_json(&temp);
    manifest["shells"]["zsh"]["sha256"] =
        serde_json::Value::String(file_digest(&aged));
    fs::write(
        &manifest_path,
        serde_json::to_vec_pretty(&manifest).expect("encode"),
    )
    .expect("rewrite manifest");

    let stale = completion_command(&temp, &fake)
        .arg("status")
        .output()
        .expect("run status");
    assert_success(&stale);
    assert!(
        stdout(&stale).contains("outdated"),
        "stale state:\n{}",
        format_output(&stale)
    );

    let update = completion_command(&temp, &fake)
        .args(["install", "zsh", "-n", "-t"])
        .arg(&target)
        .output()
        .expect("run update");
    assert_success(&update);
    assert!(
        stdout(&update).contains("updated · protocol 1"),
        "update:\n{}",
        format_output(&update)
    );
}

#[test]
fn foreign_edited_and_symlink_refuse_without_force() {
    for case in ["foreign", "edited", "symlink"] {
        let temp = TempDir::new("bob-cli-completion-refuse");
        let fake = fakebin(&temp);
        let target = temp.path().join("zfunc");
        let adapter = target.join("_bob");

        let install = completion_command(&temp, &fake)
            .args(["install", "zsh", "-n", "-t"])
            .arg(&target)
            .output()
            .expect("run install");
        assert_success(&install);
        let pristine = fs::read(&adapter).expect("read adapter");

        match case {
            "foreign" => {
                fs::write(&adapter, "# my own _bob\n").expect("write foreign");
            }
            "edited" => {
                let mut bytes = pristine.clone();
                bytes.extend_from_slice(b"\n# local tweak\n");
                fs::write(&adapter, &bytes).expect("edit adapter");
            }
            "symlink" => {
                let other = target.join("other");
                fs::write(&other, pristine).expect("write other");
                fs::remove_file(&adapter).expect("remove adapter");
                std::os::unix::fs::symlink(&other, &adapter).expect("link");
            }
            _ => unreachable!(),
        }

        let refused = completion_command(&temp, &fake)
            .args(["install", "zsh", "-n", "-t"])
            .arg(&target)
            .output()
            .expect("run refused install");
        assert_eq!(
            refused.status.code(),
            Some(1),
            "refusal exits 1 for {case}:\n{}",
            format_output(&refused)
        );
        assert!(
            stdout(&refused).contains("refused without --force"),
            "refusal for {case}:\n{}",
            format_output(&refused)
        );

        let forced = completion_command(&temp, &fake)
            .args(["install", "zsh", "-n", "-f", "-t"])
            .arg(&target)
            .output()
            .expect("run forced install");
        assert_success(&forced);
        assert!(
            !fs::symlink_metadata(&adapter)
                .expect("stat adapter")
                .file_type()
                .is_symlink(),
            "--force leaves a regular file for {case}"
        );
        assert!(
            stdout(&forced).contains("updated · protocol 1"),
            "forced update for {case}:\n{}",
            format_output(&forced)
        );
    }
}

#[test]
fn externally_managed_adapter_is_reported_never_adopted() {
    let temp = TempDir::new("bob-cli-completion-external");
    let fake = fakebin(&temp);
    let target = temp.path().join("zfunc");
    fs::create_dir_all(&target).expect("create target");

    // The user wrote the adapter themselves: no manifest entry exists.
    let printed = completion_command(&temp, &fake)
        .args(["zsh", "-o"])
        .arg(target.join("_bob"))
        .output()
        .expect("run zsh -o");
    assert_success(&printed);

    // Status resolves the manifest-less default under $HOME, so repeat
    // the setup in a tree where the default lands on the hand-written file.
    let temp2 = TempDir::new("bob-cli-completion-external2");
    let fake2 = fakebin(&temp2);
    let home_target = temp2.path().join(".zfunc");
    fs::create_dir_all(&home_target).expect("create home target");
    let printed2 = completion_command(&temp2, &fake2)
        .args(["zsh", "-o"])
        .arg(home_target.join("_bob"))
        .output()
        .expect("run zsh -o");
    assert_success(&printed2);

    let status2 = completion_command(&temp2, &fake2)
        .arg("status")
        .output()
        .expect("run status");
    assert_success(&status2);
    assert!(
        stdout(&status2).contains("current (externally managed)"),
        "external state:\n{}",
        format_output(&status2)
    );
    assert!(
        manifest_bytes(&temp2).is_none(),
        "status never adopts the file"
    );

    let install = completion_command(&temp2, &fake2)
        .args(["install", "-n"])
        .output()
        .expect("run install");
    assert_success(&install);
    assert!(
        stdout(&install).contains("unchanged"),
        "already-current bytes:\n{}",
        format_output(&install)
    );
    assert!(
        manifest_bytes(&temp2).is_none(),
        "install never adopts the file"
    );
}

#[test]
fn dry_run_writes_nothing() {
    let temp = TempDir::new("bob-cli-completion-dry");
    let fake = fakebin(&temp);
    let before = snapshot_tree(temp.path());

    let dry = completion_command(&temp, &fake)
        .args(["install", "zsh", "-d", "-t"])
        .arg(temp.path().join("zfunc"))
        .output()
        .expect("run dry install");
    assert_success(&dry);
    let out = stdout(&dry);
    assert!(out.contains("dry run"), "dry header:\n{out}");
    assert!(out.contains("would install"), "dry plan:\n{out}");
    assert_eq!(
        snapshot_tree(temp.path()),
        before,
        "dry run creates nothing"
    );
    assert!(
        manifest_bytes(&temp).is_none(),
        "dry run skips the manifest"
    );
}

#[test]
fn target_rules_pick_in_order() {
    // Explicit --target wins and is recorded.
    let temp = TempDir::new("bob-cli-completion-target");
    let fake = fakebin(&temp);
    let explicit = temp.path().join("explicit");
    let out = completion_command(&temp, &fake)
        .args(["install", "zsh", "-n", "-d", "-t"])
        .arg(&explicit)
        .output()
        .expect("run dry install");
    assert_success(&out);
    assert!(
        stdout(&out).contains("explicit/_bob")
            && stdout(&out).contains("(--target)"),
        "explicit target:\n{}",
        format_output(&out)
    );

    // First writable fpath entry under $HOME wins over the default.
    let fpath_dir = temp.path().join("myzfunc");
    fs::create_dir_all(&fpath_dir).expect("create fpath dir");
    let out = completion_command(&temp, &fake)
        .args(["install", "zsh", "-n", "-d"])
        .env("FAKE_FPATH", fpath_dir.display().to_string())
        .output()
        .expect("run fpath install");
    assert_success(&out);
    let text = stdout(&out);
    assert!(text.contains("myzfunc/_bob"), "fpath target:\n{text}");
    assert!(
        text.contains("first writable fpath entry"),
        "reason:\n{text}"
    );

    // A `plugins` fpath entry is skipped.
    let plugin_dir = temp.path().join("plugins").join("foo");
    fs::create_dir_all(&plugin_dir).expect("create plugin dir");
    let fpath_value =
        format!("{}:{}", plugin_dir.display(), fpath_dir.display());
    let out = completion_command(&temp, &fake)
        .args(["install", "zsh", "-n", "-d"])
        .env("FAKE_FPATH", &fpath_value)
        .output()
        .expect("run plugin-skipping install");
    assert!(
        stdout(&out).contains("myzfunc/_bob"),
        "plugins entry skipped:\n{}",
        format_output(&out)
    );

    // oh-my-zsh completions win when no fpath candidate exists.
    let omz = temp.path().join(".oh-my-zsh");
    fs::create_dir_all(&omz).expect("create omz");
    let out = completion_command(&temp, &fake)
        .args(["install", "zsh", "-n", "-d"])
        .env("ZSH", &omz)
        .output()
        .expect("run omz install");
    assert_success(&out);
    let text = stdout(&out);
    assert!(
        text.contains("custom/completions/_bob"),
        "omz target:\n{text}"
    );
    assert!(text.contains("oh-my-zsh completions"), "reason:\n{text}");

    // Otherwise ~/.zfunc is created.
    let temp2 = TempDir::new("bob-cli-completion-ztarget");
    let fake2 = fakebin(&temp2);
    let out = completion_command(&temp2, &fake2)
        .args(["install", "zsh", "-n", "-d"])
        .output()
        .expect("run default install");
    assert_success(&out);
    let text = stdout(&out);
    assert!(text.contains("~/.zfunc/_bob"), "default target:\n{text}");
}

#[test]
fn verify_probe_classifications() {
    let temp = TempDir::new("bob-cli-completion-verify");
    let fake = fakebin(&temp);
    let target = temp.path().join("zfunc");
    let adapter = target.join("_bob");

    let install = completion_command(&temp, &fake)
        .args(["install", "zsh", "-n", "-t"])
        .arg(&target)
        .output()
        .expect("run install");
    assert_success(&install);

    // Registered.
    let status = completion_command(&temp, &fake)
        .args(["status", "-v"])
        .env("FAKE_VERIFY", "registered")
        .env("FAKE_SOURCE", &adapter)
        .output()
        .expect("run status -v");
    assert_success(&status);
    assert!(
        stdout(&status).contains("registered as _bob"),
        "registered:\n{}",
        format_output(&status)
    );

    // Shadowed by another file.
    let status = completion_command(&temp, &fake)
        .args(["status", "-v"])
        .env("FAKE_VERIFY", "shadowed")
        .env("FAKE_SOURCE", "/other/_bob")
        .output()
        .expect("run shadowed status -v");
    assert_eq!(status.status.code(), Some(1));
    assert!(
        stdout(&status).contains("shadowed by /other/_bob"),
        "shadowed:\n{}",
        format_output(&status)
    );

    // Not registered with the directory off fpath: fpath remedy.
    let status = completion_command(&temp, &fake)
        .args(["status", "-v"])
        .env("FAKE_VERIFY", "off")
        .output()
        .expect("run off-fpath status -v");
    assert_eq!(status.status.code(), Some(1));
    let text = stdout(&status);
    assert!(text.contains("not registered"), "state:\n{text}");
    assert!(text.contains("fpath=("), "fpath remedy:\n{text}");

    // Not registered with the directory on fpath: compdump remedy.
    let status = completion_command(&temp, &fake)
        .args(["status", "-v"])
        .env("FAKE_VERIFY", "off")
        .env("FAKE_VERIFY_FPATH", target.display().to_string())
        .output()
        .expect("run on-fpath status -v");
    assert_eq!(status.status.code(), Some(1));
    assert!(
        stdout(&status).contains(".zcompdump"),
        "compdump remedy:\n{}",
        format_output(&status)
    );

    // A stuck probe is a warning, never success, and never hangs the suite.
    let slow = completion_command(&temp, &fake)
        .args(["install", "zsh", "-t"])
        .arg(&target)
        .env("FAKE_VERIFY", "sleep")
        .env("BOB_COMPLETION_PROBE_TIMEOUT_MS", "200")
        .output()
        .expect("run slow install");
    assert!(
        stdout(&slow).contains("unverified (timed out)"),
        "timeout:\n{}",
        format_output(&slow)
    );
}

#[test]
fn path_shadow_warns() {
    let temp = TempDir::new("bob-cli-completion-shadow");
    let fake = fakebin(&temp);
    fs::write(fake.join("bob"), "#!/bin/sh\nexit 0\n").expect("write fake bob");
    fs::set_permissions(fake.join("bob"), fs::Permissions::from_mode(0o755))
        .expect("chmod fake bob");

    let status = completion_command(&temp, &fake)
        .arg("status")
        .output()
        .expect("run status");
    assert_success(&status);
    assert!(
        stdout(&status).contains("on PATH is"),
        "shadow warning:\n{}",
        format_output(&status)
    );
}

#[test]
fn status_json_shape_and_bare_forms() {
    let temp = TempDir::new("bob-cli-completion-json");
    let fake = fakebin(&temp);
    let target = temp.path().join("zfunc");

    let install = completion_command(&temp, &fake)
        .args(["install", "zsh", "-n", "-t"])
        .arg(&target)
        .output()
        .expect("run install");
    assert_success(&install);

    let status = completion_command(&temp, &fake)
        .args(["status", "-j"])
        .output()
        .expect("run status -j");
    assert_success(&status);
    let value: serde_json::Value =
        serde_json::from_str(stdout(&status).trim()).expect("status JSON");
    assert_eq!(value["schema_version"], 1);
    assert_eq!(value["protocol"], 1);
    assert_eq!(value["bob"]["version"], env!("CARGO_PKG_VERSION"));
    let shells = value["shells"].as_array().expect("shells array");
    assert_eq!(shells.len(), 2);
    let shell = shells
        .iter()
        .find(|entry| entry["shell"] == "zsh")
        .expect("zsh entry present");
    for key in [
        "shell",
        "state",
        "path",
        "protocol",
        "owned",
        "registration",
        "registration_checked",
        "target_reason",
        "remedy",
    ] {
        assert!(shell.get(key).is_some(), "shell entry has {key}:\n{shell}");
    }
    assert_eq!(shell["shell"], "zsh");
    assert_eq!(shell["state"], "current");
    assert_eq!(shell["owned"], true);

    let bare = completion_command(&temp, &fake)
        .output()
        .expect("run bare completion");
    let explicit = completion_command(&temp, &fake)
        .arg("status")
        .output()
        .expect("run status");
    assert_success(&bare);
    assert_success(&explicit);
    assert_eq!(
        stdout(&bare),
        stdout(&explicit),
        "bare completion equals status"
    );

    let list = completion_command(&temp, &fake)
        .arg("list")
        .output()
        .expect("run list alias");
    assert_success(&list);
    assert!(
        stdout(&list).contains("zsh"),
        "list alias:\n{}",
        format_output(&list)
    );
}

#[test]
fn uninstall_removes_only_bob_owned_files() {
    let temp = TempDir::new("bob-cli-completion-uninstall");
    let fake = fakebin(&temp);
    let target = temp.path().join("zfunc");
    let adapter = target.join("_bob");

    let install = completion_command(&temp, &fake)
        .args(["install", "zsh", "-n", "-t"])
        .arg(&target)
        .output()
        .expect("run install");
    assert_success(&install);
    fs::write(target.join("_bob.zwc"), "stale dump").expect("write zwc");

    let dry = completion_command(&temp, &fake)
        .args(["uninstall", "zsh", "-d"])
        .output()
        .expect("run dry uninstall");
    assert_success(&dry);
    assert!(stdout(&dry).contains("would remove"), "dry plan");
    assert!(adapter.is_file(), "dry run removes nothing");

    let remove = completion_command(&temp, &fake)
        .args(["uninstall", "zsh"])
        .output()
        .expect("run uninstall");
    assert_success(&remove);
    assert!(stdout(&remove).contains("removed"), "removed");
    assert!(!adapter.exists(), "adapter removed");
    assert!(!target.join("_bob.zwc").exists(), "stale zwc removed");
    assert!(
        stdout(&remove).contains("drop the loaded completion"),
        "closer:\n{}",
        format_output(&remove)
    );

    // An edited file is refused with the exact rm command.
    let install = completion_command(&temp, &fake)
        .args(["install", "zsh", "-n", "-t"])
        .arg(&target)
        .output()
        .expect("run reinstall");
    assert_success(&install);
    // Rewrite the adapter through `zsh -o` (stamped, current bytes),
    // then tweak a byte: the stamp survives but the manifest digest no
    // longer matches, which is the edited state.
    let printed = completion_command(&temp, &fake)
        .args(["zsh", "-o"])
        .arg(&adapter)
        .output()
        .expect("rewrite adapter");
    assert_success(&printed);
    let mut bytes = fs::read(&adapter).expect("read adapter");
    bytes.extend_from_slice(b"\n# local tweak\n");
    fs::write(&adapter, &bytes).expect("tweak adapter");

    let refused = completion_command(&temp, &fake)
        .args(["uninstall", "zsh"])
        .output()
        .expect("run refused uninstall");
    assert_eq!(
        refused.status.code(),
        Some(1),
        "edited refusal:\n{}",
        format_output(&refused)
    );
    assert!(
        stdout(&refused).contains("not removed"),
        "refusal:\n{}",
        format_output(&refused)
    );
    assert!(
        stdout(&refused).contains("rm "),
        "exact rm command:\n{}",
        format_output(&refused)
    );
    assert!(adapter.is_file(), "edited file kept");
}

#[test]
fn usage_errors_exit_2_and_shell_selection() {
    let temp = TempDir::new("bob-cli-completion-usage");
    let fake = fakebin(&temp);

    let bad_shell = completion_command(&temp, &fake)
        .args(["install", "fish"])
        .output()
        .expect("run install fish");
    assert_eq!(
        bad_shell.status.code(),
        Some(2),
        "unknown shell is usage:\n{}",
        format_output(&bad_shell)
    );

    // Both supported shells install explicitly.
    let bash_target = temp.path().join("bcomp");
    let bash_install = completion_command(&temp, &fake)
        .args(["install", "bash", "-n", "-t"])
        .arg(&bash_target)
        .output()
        .expect("run install bash");
    assert_success(&bash_install);
    assert!(
        stdout(&bash_install).contains("bash"),
        "bash installs:\n{}",
        format_output(&bash_install)
    );

    // Repeating the one supported shell still installs once.
    let target = temp.path().join("zfunc");
    let repeat = completion_command(&temp, &fake)
        .args(["install", "zsh", "zsh", "-n", "-t"])
        .arg(&target)
        .output()
        .expect("run repeated install");
    assert_success(&repeat);

    // An unsupported $SHELL with no owned adapters names a shell.
    let temp2 = TempDir::new("bob-cli-completion-noshell");
    let fake2 = fakebin(&temp2);
    let mut command = completion_command(&temp2, &fake2);
    command.args(["install", "-n"]).env("SHELL", "/bin/fish");
    let no_shell = command.output().expect("run shell-less install");
    assert_eq!(
        no_shell.status.code(),
        Some(1),
        "unsupported shell:\n{}",
        format_output(&no_shell)
    );
    assert!(
        stderr(&no_shell).contains("pass a shell"),
        "names a shell:\n{}",
        format_output(&no_shell)
    );

    // An unknown subcommand is a usage error.
    let unknown = completion_command(&temp, &fake)
        .args(["bogus"])
        .output()
        .expect("run unknown subcommand");
    assert_eq!(
        unknown.status.code(),
        Some(2),
        "unknown subcommand is usage:\n{}",
        format_output(&unknown)
    );
}

#[test]
fn missing_manifest_entry_reports_missing() {
    let temp = TempDir::new("bob-cli-completion-missing");
    let fake = fakebin(&temp);
    let target = temp.path().join("zfunc");
    let adapter = target.join("_bob");

    let install = completion_command(&temp, &fake)
        .args(["install", "zsh", "-n", "-t"])
        .arg(&target)
        .output()
        .expect("run install");
    assert_success(&install);
    fs::remove_file(&adapter).expect("remove adapter");

    let status = completion_command(&temp, &fake)
        .args(["status", "-j"])
        .output()
        .expect("run status");
    assert_success(&status);
    let value: serde_json::Value =
        serde_json::from_str(stdout(&status).trim()).expect("status JSON");
    let zsh = value["shells"]
        .as_array()
        .expect("shells array")
        .iter()
        .find(|entry| entry["shell"] == "zsh")
        .expect("zsh entry present");
    assert_eq!(zsh["state"], "missing");

    let live = completion_command(&temp, &fake)
        .args(["status", "-v"])
        .env("FAKE_VERIFY", "off")
        .output()
        .expect("run status -v");
    assert_eq!(
        live.status.code(),
        Some(1),
        "missing install fails -v:\n{}",
        format_output(&live)
    );
}

#[test]
fn zsh_print_matches_installed_bytes() {
    let temp = TempDir::new("bob-cli-completion-print");
    let fake = fakebin(&temp);

    let printed = completion_command(&temp, &fake)
        .arg("zsh")
        .output()
        .expect("run zsh print");
    assert_success(&printed);
    assert!(stdout(&printed).starts_with(ADAPTER_HEAD));
    assert_stdout_has_no_ansi(&printed);
}

/// True when `zsh` runs; prints the skip note otherwise.
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

fn probe_log(temp: &TempDir) -> PathBuf {
    temp.path().join("probes.log")
}

fn logged_probes(temp: &TempDir) -> String {
    fs::read_to_string(probe_log(temp)).unwrap_or_default()
}

#[test]
fn not_installed_shell_never_probes_and_never_fails() {
    let temp = TempDir::new("bob-cli-completion-noprobe");
    let fake = fakebin(&temp);
    let log = probe_log(&temp);

    for args in [
        Vec::<String>::new(),
        vec!["status".to_string()],
        vec!["status".to_string(), "-j".to_string()],
        vec!["status".to_string(), "-v".to_string()],
    ] {
        let mut command = completion_command(&temp, &fake);
        command.args(&args).env("FAKE_LOG", &log);
        let out = command.output().expect("run completion");
        assert_success(&out);
        let text = stdout(&out);
        assert!(
            text.contains("not installed"),
            "not-installed form for {args:?}:\n{}",
            format_output(&out)
        );
        if args.iter().any(|arg| arg == "-j") {
            assert!(
                text.contains("\"remedy\":\"bob completion install"),
                "remedy for {args:?}:\n{}",
                format_output(&out)
            );
        } else {
            assert!(
                text.contains("→ bob completion install"),
                "remedy for {args:?}:\n{}",
                format_output(&out)
            );
        }
    }
    assert!(
        logged_probes(&temp).is_empty(),
        "no shell was spawned:\n{}",
        logged_probes(&temp)
    );
}

#[test]
fn install_without_shell_args_honors_shell_plus_owned() {
    let temp = TempDir::new("bob-cli-completion-shellsel");
    let fake = fakebin(&temp);
    let bash_target = temp.path().join("bcomp");

    let bash = completion_command(&temp, &fake)
        .args(["install", "bash", "-n", "-t"])
        .arg(&bash_target)
        .output()
        .expect("own bash");
    assert_success(&bash);

    // $SHELL is zsh and bash is owned: both install, no usage error.
    let both = completion_command(&temp, &fake)
        .args(["install", "-n", "-d"])
        .env("SHELL", "/bin/zsh")
        .output()
        .expect("install both");
    assert_success(&both);
    let text = stdout(&both);
    assert!(text.contains("zsh"), "zsh selected:\n{text}");
    assert!(text.contains("bash"), "owned bash kept:\n{text}");

    // With -t and no SHELL arguments only $SHELL installs, so the second
    // owned adapter never triggers the two-shell usage error.
    let single = completion_command(&temp, &fake)
        .args(["install", "-n", "-d", "-t"])
        .arg(temp.path().join("zonly"))
        .env("SHELL", "/bin/zsh")
        .output()
        .expect("single-target install");
    assert_success(&single);
    let text = stdout(&single);
    assert!(text.contains("zsh"), "only zsh:\n{text}");
    assert!(!text.contains("bash"), "bash untouched:\n{text}");

    // -t with two explicit shells is still a usage error.
    let two = completion_command(&temp, &fake)
        .args(["install", "zsh", "bash", "-n", "-d", "-t"])
        .arg(temp.path().join("both"))
        .output()
        .expect("two-shell target");
    assert_eq!(
        two.status.code(),
        Some(2),
        "two shells with -t:\n{}",
        format_output(&two)
    );
}

#[test]
fn install_glyphs_follow_registration() {
    // Not registered, shadowed, and bound fail with ✗.
    for (verify, source, needle) in [
        ("off", "", "not registered"),
        ("shadowed", "/other/_bob", "shadowed by /other/_bob"),
        ("bound", "", "bob is bound to _other"),
    ] {
        let temp = TempDir::new("bob-cli-completion-glyph");
        let fake = fakebin(&temp);
        let target = temp.path().join("zfunc");
        let mut command = completion_command(&temp, &fake);
        command
            .args(["install", "zsh", "-t"])
            .arg(&target)
            .env("FAKE_VERIFY", verify);
        if !source.is_empty() {
            command.env("FAKE_SOURCE", source);
        }
        let out = command.output().expect("run install");
        assert_eq!(
            out.status.code(),
            Some(1),
            "{verify} exits 1:\n{}",
            format_output(&out)
        );
        let text = stdout(&out);
        assert!(text.contains("✗"), "fail glyph for {verify}:\n{text}");
        assert!(text.contains(needle), "registration for {verify}:\n{text}");
    }

    // A stuck probe warns with ⚠ and stays 0.
    let temp = TempDir::new("bob-cli-completion-unverified");
    let fake = fakebin(&temp);
    let slow = completion_command(&temp, &fake)
        .args(["install", "zsh", "-t"])
        .arg(temp.path().join("zfunc"))
        .env("FAKE_VERIFY", "sleep")
        .env("BOB_COMPLETION_PROBE_TIMEOUT_MS", "200")
        .output()
        .expect("run slow install");
    assert_success(&slow);
    let text = stdout(&slow);
    assert!(
        text.contains("⚠") && text.contains("unverified (timed out)"),
        "warn glyph:\n{text}"
    );

    // -n never probes: · plus the status -v pointer.
    let temp = TempDir::new("bob-cli-completion-noverify");
    let fake = fakebin(&temp);
    let skipped = completion_command(&temp, &fake)
        .args(["install", "zsh", "-n", "-t"])
        .arg(temp.path().join("zfunc"))
        .output()
        .expect("run -n install");
    assert_success(&skipped);
    let text = stdout(&skipped);
    assert!(
        text.contains("registration not checked → bob completion status -v"),
        "-n pointer:\n{text}"
    );
    assert!(
        !text.contains("unverified (skipped"),
        "no stale skipped text:\n{text}"
    );
}

#[test]
fn status_without_verify_warns_on_recorded_unhealthy() {
    let temp = TempDir::new("bob-cli-completion-recorded-bad");
    let fake = fakebin(&temp);
    let target = temp.path().join("zfunc");

    // Record an unhealthy verification at install time.
    let install = completion_command(&temp, &fake)
        .args(["install", "zsh", "-t"])
        .arg(&target)
        .env("FAKE_VERIFY", "off")
        .output()
        .expect("run install");
    assert_eq!(install.status.code(), Some(1));

    // Without -v the recorded failure renders ⚠, never ✓, and exits 0.
    let status = completion_command(&temp, &fake)
        .arg("status")
        .output()
        .expect("run status");
    assert_success(&status);
    let text = stdout(&status);
    assert!(text.contains("⚠"), "warn glyph:\n{text}");
    assert!(text.contains("not registered"), "recorded text:\n{text}");
}

#[test]
fn closers_tell_the_truth() {
    // A dry run never prints a closer.
    let temp = TempDir::new("bob-cli-completion-closer-dry");
    let fake = fakebin(&temp);
    let dry = completion_command(&temp, &fake)
        .args(["install", "zsh", "-d", "-t"])
        .arg(temp.path().join("zfunc"))
        .output()
        .expect("run dry install");
    assert_success(&dry);
    let text = stdout(&dry);
    assert!(
        !text.contains("Completion is live"),
        "dry run has no live closer:\n{text}"
    );
    assert!(
        !text.contains("Open a new shell"),
        "dry run has no reopen closer:\n{text}"
    );

    // Unchanged but never registered: no closer at all.
    let again = completion_command(&temp, &fake)
        .args(["install", "zsh", "-n", "-t"])
        .arg(temp.path().join("zfunc"))
        .output()
        .expect("run -n install");
    assert_success(&again);
    let install_text = stdout(&again);
    assert!(
        !install_text.contains("Completion is live"),
        "unregistered install has no closer"
    );
    let repeat = completion_command(&temp, &fake)
        .args(["install", "zsh", "-n", "-t"])
        .arg(temp.path().join("zfunc"))
        .output()
        .expect("run repeat install");
    assert_success(&repeat);
    let text = stdout(&repeat);
    assert!(
        !text.contains("Completion is live")
            && !text.contains("Open a new shell"),
        "unhealthy unchanged has no closer:\n{text}"
    );

    // Unchanged and registered: the live closer prints.
    let temp = TempDir::new("bob-cli-completion-closer-live");
    let fake = fakebin(&temp);
    let target = temp.path().join("zfunc");
    let adapter = target.join("_bob");
    let install = completion_command(&temp, &fake)
        .args(["install", "zsh", "-t"])
        .arg(&target)
        .env("FAKE_VERIFY", "registered")
        .env("FAKE_SOURCE", &adapter)
        .output()
        .expect("run live install");
    assert_success(&install);
    let repeat = completion_command(&temp, &fake)
        .args(["install", "zsh", "-n", "-t"])
        .arg(&target)
        .output()
        .expect("run repeat install");
    assert_success(&repeat);
    assert!(
        stdout(&repeat).contains("Completion is live"),
        "registered unchanged is live:\n{}",
        format_output(&repeat)
    );
}

#[test]
fn unrecorded_stamped_file_is_outdated_externally_managed() {
    let temp = TempDir::new("bob-cli-completion-unrecorded");
    let fake = fakebin(&temp);
    // The manifest-less default under $HOME, so status and install find
    // the hand-written file without any flags.
    let target = temp.path().join(".zfunc");
    fs::create_dir_all(&target).expect("create target");
    let adapter = target.join("_bob");

    // The user wrote the adapter themselves, then bob upgraded: stamped
    // bytes that differ, with no manifest record.
    let printed = completion_command(&temp, &fake)
        .args(["zsh", "-o"])
        .arg(&adapter)
        .output()
        .expect("run zsh -o");
    assert_success(&printed);
    let mut aged = fs::read(&adapter).expect("read adapter");
    aged.extend_from_slice(b"\n# aged\n");
    fs::write(&adapter, &aged).expect("age adapter");

    let status = completion_command(&temp, &fake)
        .arg("status")
        .output()
        .expect("run status");
    assert_success(&status);
    assert!(
        stdout(&status).contains("outdated (externally managed)"),
        "state:\n{}",
        format_output(&status)
    );

    let refused = completion_command(&temp, &fake)
        .args(["install", "zsh", "-n", "-t"])
        .arg(&target)
        .output()
        .expect("run refused install");
    assert_eq!(
        refused.status.code(),
        Some(1),
        "refused without --force:\n{}",
        format_output(&refused)
    );
    assert!(
        stdout(&refused).contains("refused without --force"),
        "refusal:\n{}",
        format_output(&refused)
    );
    assert!(
        manifest_bytes(&temp).is_none(),
        "never adopted into the manifest"
    );

    let forced = completion_command(&temp, &fake)
        .args(["install", "zsh", "-n", "-f", "-t"])
        .arg(&target)
        .output()
        .expect("run forced install");
    assert_success(&forced);
    assert!(
        stdout(&forced).contains("updated · protocol 1"),
        "forced update:\n{}",
        format_output(&forced)
    );
}

#[test]
fn target_move_removes_previous_adapter() {
    let temp = TempDir::new("bob-cli-completion-move");
    let fake = fakebin(&temp);
    let first = temp.path().join("first");
    let second = temp.path().join("second");

    let install = completion_command(&temp, &fake)
        .args(["install", "zsh", "-n", "-t"])
        .arg(&first)
        .output()
        .expect("run first install");
    assert_success(&install);
    fs::write(first.join("_bob.zwc"), "stale dump").expect("write zwc");

    let moved = completion_command(&temp, &fake)
        .args(["install", "zsh", "-n", "-t"])
        .arg(&second)
        .output()
        .expect("run moved install");
    assert_success(&moved);
    let text = stdout(&moved);
    assert!(
        text.contains("removed previous adapter"),
        "removal note:\n{text}"
    );
    assert!(!first.join("_bob").exists(), "old adapter removed");
    assert!(!first.join("_bob.zwc").exists(), "old zwc removed");
    assert!(second.join("_bob").is_file(), "new adapter written");

    // An edited previous file is left behind with the exact rm command.
    let temp = TempDir::new("bob-cli-completion-move-edited");
    let fake = fakebin(&temp);
    let first = temp.path().join("first");
    let second = temp.path().join("second");
    let install = completion_command(&temp, &fake)
        .args(["install", "zsh", "-n", "-t"])
        .arg(&first)
        .output()
        .expect("run first install");
    assert_success(&install);
    let mut bytes = fs::read(first.join("_bob")).expect("read adapter");
    bytes.extend_from_slice(b"\n# local tweak\n");
    fs::write(first.join("_bob"), &bytes).expect("tweak adapter");

    let moved = completion_command(&temp, &fake)
        .args(["install", "zsh", "-n", "-t"])
        .arg(&second)
        .output()
        .expect("run moved install");
    assert_success(&moved);
    let text = stdout(&moved);
    assert!(
        text.contains("previous adapter left") && text.contains("rm "),
        "exact rm command:\n{text}"
    );
    assert!(first.join("_bob").is_file(), "edited file kept");
}

#[test]
fn home_default_fpath_line_prints_without_probe() {
    let temp = TempDir::new("bob-cli-completion-fpathline");
    let fake = fakebin(&temp);

    // Dry run and -n installs on the ~/.zfunc home default print the
    // fpath line even though nothing probes.
    let dry = completion_command(&temp, &fake)
        .args(["install", "zsh", "-n", "-d"])
        .env("SHELL", "/bin/zsh")
        .output()
        .expect("run dry install");
    assert_success(&dry);
    assert!(
        stdout(&dry).contains("fpath=(~/.zfunc $fpath)"),
        "dry fpath line:\n{}",
        format_output(&dry)
    );

    let install = completion_command(&temp, &fake)
        .args(["install", "zsh", "-n"])
        .env("SHELL", "/bin/zsh")
        .output()
        .expect("run -n install");
    assert_success(&install);
    assert!(
        stdout(&install).contains("fpath=(~/.zfunc $fpath)"),
        "-n fpath line:\n{}",
        format_output(&install)
    );
}

#[test]
fn plain_output_has_no_ansi_when_piped() {
    let temp = TempDir::new("bob-cli-completion-plain");
    let fake = fakebin(&temp);

    let bare = completion_command(&temp, &fake)
        .output()
        .expect("run bare completion");
    assert_success(&bare);
    assert_stdout_has_no_ansi(&bare);

    let status = completion_command(&temp, &fake)
        .arg("status")
        .output()
        .expect("run status");
    assert_success(&status);
    assert_stdout_has_no_ansi(&status);
}

#[test]
fn zsh_bound_to_function_reports() {
    let temp = TempDir::new("bob-cli-completion-bound");
    let fake = fakebin(&temp);
    let target = temp.path().join("zfunc");

    let install = completion_command(&temp, &fake)
        .args(["install", "zsh", "-n", "-t"])
        .arg(&target)
        .output()
        .expect("run install");
    assert_success(&install);

    let status = completion_command(&temp, &fake)
        .args(["status", "-v"])
        .env("FAKE_VERIFY", "bound")
        .output()
        .expect("run status -v");
    assert_eq!(
        status.status.code(),
        Some(1),
        "bound fails -v:\n{}",
        format_output(&status)
    );
    assert!(
        stdout(&status).contains("bob is bound to _other"),
        "bound text:\n{}",
        format_output(&status)
    );
}

#[test]
fn previous_install_target_rule_and_reason() {
    let temp = TempDir::new("bob-cli-completion-prev");
    let fake = fakebin(&temp);
    let explicit = temp.path().join("explicit");

    let install = completion_command(&temp, &fake)
        .args(["install", "zsh", "-n", "-t"])
        .arg(&explicit)
        .output()
        .expect("run install");
    assert_success(&install);

    // The manifest location wins over fpath discovery, with its reason.
    let elsewhere = temp.path().join("fpathdir");
    fs::create_dir_all(&elsewhere).expect("create fpath dir");
    let dry = completion_command(&temp, &fake)
        .args(["install", "zsh", "-n", "-d"])
        .env("FAKE_FPATH", elsewhere.display().to_string())
        .output()
        .expect("run dry install");
    assert_success(&dry);
    let text = stdout(&dry);
    assert!(
        text.contains("explicit/_bob"),
        "previous install wins:\n{text}"
    );
    assert!(
        text.contains("previous install"),
        "recorded reason:\n{text}"
    );
}

#[test]
fn fake_bash_never_reports_registered_when_missing() {
    let temp = TempDir::new("bob-cli-completion-fakebash");
    let fake = fakebin(&temp);
    let target = temp.path().join("bcomp");
    let adapter = target.join("bob");

    let install = completion_command(&temp, &fake)
        .args(["install", "bash", "-n", "-t"])
        .arg(&target)
        .output()
        .expect("run install");
    assert_success(&install);

    // The adapter exists: the fake may report it registered.
    let status = completion_command(&temp, &fake)
        .args(["status", "-v"])
        .env("FAKE_VERIFY", "registered")
        .env("FAKE_BASH_ADAPTER", &adapter)
        .output()
        .expect("run status -v");
    assert_success(&status);
    assert!(
        stdout(&status).contains("registered as _bob"),
        "registered:\n{}",
        format_output(&status)
    );

    // Pointing at a missing file, the same fake never reports registered.
    let missing = completion_command(&temp, &fake)
        .args(["status", "-v"])
        .env("FAKE_VERIFY", "registered")
        .env("FAKE_BASH_ADAPTER", temp.path().join("nowhere").join("bob"))
        .output()
        .expect("run missing status -v");
    assert!(
        !stdout(&missing).contains("registered as _bob"),
        "missing file never registered:\n{}",
        format_output(&missing)
    );
}

#[test]
fn probe_without_controlling_terminal_via_zpty() {
    if !have_zsh() {
        return;
    }
    if !Command::new("zsh")
        .args(["-f", "-c", "zmodload zsh/zpty"])
        .output()
        .map(|output| output.status.success())
        .unwrap_or(false)
    {
        println!("skipped: zsh/zpty not available");
        return;
    }
    let temp = TempDir::new("bob-cli-completion-ctty");
    let dir = temp.path();
    let fake = fakebin(&temp);
    let target = dir.join("zfunc");
    let adapter = target.join("_bob");

    // Install without probing so the manifest records the target; the
    // live probe runs next under a controlling terminal.
    let install = completion_command(&temp, &fake)
        .args(["install", "zsh", "-n", "-t"])
        .arg(&target)
        .output()
        .expect("run install");
    assert_success(&install);

    // The pty child must be the real zsh (resolved before the fake
    // binaries go on PATH); only the probe inside bob may see the fake.
    let real_zsh = String::from_utf8_lossy(
        &Command::new("sh")
            .args(["-c", "command -v zsh"])
            .output()
            .expect("locate real zsh")
            .stdout,
    )
    .trim()
    .to_string();
    assert!(
        !real_zsh.is_empty() && Path::new(&real_zsh).is_absolute(),
        "real zsh found"
    );
    let out_file = dir.join("status.out");
    let q = |value: &str| format!("'{}'", value.replace('\'', "'\\''"));
    let mut driver = String::new();
    // The outer driver runs under the real zsh; only the pty child sees
    // the fake shell binaries first on PATH.
    driver.push_str(&format!(
        "export PATH={}:$PATH\n",
        q(&fake.display().to_string())
    ));
    driver
        .push_str("zmodload zsh/zpty || { print \"ZPTY-MISSING\"; exit 2 }\n");
    driver.push_str("zpty -d pb 2>/dev/null\n");
    driver.push_str(&format!("zpty pb {} -f\n", q(&real_zsh)));
    driver.push_str("sleep 1\n");
    driver.push_str(&format!(
        "zpty -w pb {}",
        q(&format!(
            "{} completion status -v > {} 2>&1; echo BOB-DONE-$?",
            env!("CARGO_BIN_EXE_bob"),
            out_file.display()
        ))
    ));
    driver.push('\n');
    driver.push_str(
        "if zpty -r pb done '*BOB-DONE-[0-9]*' 2>/dev/null; then print \"DRIVER-DONE\"; else print \"DRIVER-READ-FAILED\"; fi\n",
    );
    driver.push_str("zpty -d pb\n");
    let driver_path = dir.join("driver.zsh");
    fs::write(&driver_path, &driver).expect("write driver");

    let timeout = Command::new("timeout")
        .arg("--version")
        .output()
        .map(|output| output.status.success())
        .unwrap_or(false);
    let mut command = if timeout {
        let mut wrapped = Command::new("timeout");
        wrapped.arg("60").arg("zsh");
        wrapped
    } else {
        Command::new("zsh")
    };
    let output = command
        .arg("-f")
        .arg(&driver_path)
        .env("HOME", dir)
        .env("XDG_STATE_HOME", dir.join("state"))
        .env("XDG_DATA_HOME", dir.join("data"))
        .env("ZDOTDIR", dir.join("zdot"))
        .env("SHELL", "/bin/zsh")
        .env("FAKE_VERIFY", "tty-read")
        .env("FAKE_SOURCE", &adapter)
        .env("BOB_COMPLETION_PROBE_TIMEOUT_MS", "20000")
        .output()
        .expect("run zpty driver");
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    if stdout.contains("ZPTY-MISSING") {
        println!("skipped: zsh/zpty not available");
        return;
    }
    assert!(
        output.status.success() && stdout.contains("DRIVER-DONE"),
        "zpty driver failed, stdout:\n{stdout}\nstderr:\n{stderr}"
    );
    let report = fs::read_to_string(&out_file).unwrap_or_default();
    assert!(
        report.contains("registered as _bob"),
        "probe classifies under a terminal instead of timing out:\n{report}"
    );
    assert!(
        !report.contains("timed out"),
        "no stall under a terminal:\n{report}"
    );
}
