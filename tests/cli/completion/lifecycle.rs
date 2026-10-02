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
# colon-separated entry lists.
for last in "$@"; do :; done
script="$last"
split_print() {
    old_ifs="$IFS"
    IFS=':'
    # shellcheck disable=SC2086
    for entry in $1; do
        [ -n "$entry" ] && printf '%s\n' "$entry"
    done
    IFS="$old_ifs"
}
case "$script" in
    *bob-completion-fpath-probe*)
        printf 'bob-fpath-start\n'
        split_print "$FAKE_FPATH"
        printf 'bob-fpath-end\n'
        ;;
    *bob-completion-verify-probe*)
        case "$FAKE_VERIFY" in
            sleep) sleep 5 ;;
            broken) printf 'rc noise without markers\n' ;;
            *)
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
                    bound)
                        printf 'bob-comp=_other\nbob-source=\n' ;;
                    *) printf 'bob-comp=\nbob-source=\n' ;;
                esac
                printf 'bob-verify-end\n'
                ;;
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
    // the fpath probe is zsh-only and never reaches bash.
    let script = r#"#!/bin/sh
# Fake bash for bob completion lifecycle tests.
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
        second.contains("Completion is live"),
        "live closer:\n{second}"
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
