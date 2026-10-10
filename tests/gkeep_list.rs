//! `bob gkeep list` reconciliation view: sections, states, filters,
//! footers, Keep failure, and JSON.

mod gkeep_support;

use std::fs;

use gkeep_support::{
    error_response, note, snapshot_ok, stderr, stdout, FakeAdapter, GkeepEnv,
};

/// The canonical content string Rust hashes: struct field order.
fn canonical(title: &str, text: &str) -> String {
    format!(
        "{{\"title\":{},\"text\":{},\"items\":[]}}",
        serde_json::to_string(title).expect("escape title"),
        serde_json::to_string(text).expect("escape text"),
    )
}

/// The 12-hex content fingerprint for a plain title/text note.
fn fingerprint(title: &str, text: &str) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(canonical(title, text).as_bytes());
    hex::encode(hasher.finalize())[..12].to_string()
}

/// Install a stub `token_command` printing a master token, so Keep reads
/// work without a real `pass` store.
fn install_token_stub(env: &GkeepEnv) {
    let path = env.vault().join("token.sh");
    fs::write(&path, "#!/bin/sh\nprintf 'aas_et/test-master-token\\n'\n")
        .expect("write token stub");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut permissions =
            fs::metadata(&path).expect("stat token stub").permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&path, permissions).expect("chmod token stub");
    }
    let quoted = path.to_string_lossy().replace('\'', "'\\''");
    env.write_config(&format!(
        "gkeep:\n  email: bryanbugyi34@gmail.com\n  token_command: \"sh '{quoted}'\"\n",
    ));
}

fn write_target(env: &GkeepEnv, contents: &str) {
    fs::write(env.vault().join("gkeep_inbox.md"), contents)
        .expect("write target note");
}

fn target_with_note(marker: &str) -> String {
    format!(
        "---\ntitle: gkeep inbox\n---\n\n- Seed bullet one.\n- The tasks below are pulled in by the `bob gkeep` command.\n\n## Tasks\n\n- [ ] #task Pending task [created::2026-09-27]\n\t- Source: Google Keep · 2026-09-27 21:14 {marker}\n"
    )
}

/// A `bob gkeep list` command with deterministic time and width.
fn list_command(
    env: &GkeepEnv,
    fake: &FakeAdapter,
    extra: &[&str],
) -> std::process::Command {
    let mut command = env.command();
    fake.install(&mut command);
    command
        .arg("gkeep")
        .arg("list")
        .args(extra)
        .env("TZ", "UTC")
        .env("BOB_NOW", "2026-09-28 12:00:00")
        .env("COLUMNS", "200");
    command
}

#[test]
fn default_subcommand_is_list() {
    let env = GkeepEnv::new("bob-cli-gkeep-list-default");
    install_token_stub(&env);
    let fake = FakeAdapter::new(&env, "adapter");
    fake.respond(
        "snapshot",
        &snapshot_ok(
            "bryanbugyi34@gmail.com",
            vec![note("Buy milk").id("keep-new-1").build()],
        ),
    );

    let mut bare = env.command();
    fake.install(&mut bare);
    bare.arg("gkeep")
        .env("TZ", "UTC")
        .env("BOB_NOW", "2026-09-28 12:00:00")
        .env("COLUMNS", "200");
    let bare = bare.output().expect("run bob gkeep");
    let listed = list_command(&env, &fake, &[]).output().expect("run list");
    assert_eq!(bare.status.code(), Some(0));
    assert_eq!(stdout(&bare), stdout(&listed));
    assert!(stdout(&listed).contains("Google Keep"));
}

#[test]
fn each_state_is_rendered() {
    let env = GkeepEnv::new("bob-cli-gkeep-list-states");
    install_token_stub(&env);
    let pending_fp = fingerprint("Pending task", "");
    write_target(
        &env,
        &target_with_note(&format!("%%gkeep:v1:keep-pending-1:{pending_fp}%%")),
    );
    let fake = FakeAdapter::new(&env, "adapter");
    fake.respond(
        "snapshot",
        &snapshot_ok(
            "bryanbugyi34@gmail.com",
            vec![
                note("Fresh idea")
                    .id("keep-new-1")
                    .created("2026-09-27T21:14:03Z")
                    .build(),
                note("Pending task")
                    .id("keep-pending-1")
                    .created("2026-09-26T08:00:00Z")
                    .build(),
                note("Changed mind")
                    .id("keep-revised-1")
                    .created("2026-09-25T08:00:00Z")
                    .text("v2 body")
                    .build(),
                note("Wi-Fi password").id("keep-pinned-1").pinned().build(),
                note("Shared list").id("keep-shared-1").shared().build(),
                note("").id("keep-empty-1").build(),
                note("Old note").id("keep-archived-1").archived().build(),
            ],
        ),
    );
    // A stale marker makes keep-revised-1 revised instead of new.
    fs::write(
        env.vault().join("stale.md"),
        "%%gkeep:v1:keep-revised-1:000000000000%%\n",
    )
    .expect("write stale marker");

    let output = list_command(&env, &fake, &[]).output().expect("run list");
    assert_eq!(output.status.code(), Some(0));
    let text = stdout(&output);
    for state in ["new", "pending", "revised", "pinned", "shared", "empty"] {
        assert!(text.contains(state), "expected state `{state}` in:\n{text}");
    }
    assert!(
        !text.contains("archived"),
        "archived notes stay hidden without --all:\n{text}"
    );
    assert!(
        text.contains("↺ still in Keep"),
        "the pending vault task links back to Keep:\n{text}"
    );
    assert!(
        text.contains("→  bob gkeep pull"),
        "actionable notes point at pull:\n{text}"
    );

    let output = list_command(&env, &fake, &["--all"])
        .output()
        .expect("run list");
    assert_eq!(output.status.code(), Some(0));
    assert!(
        stdout(&output).contains("archived"),
        "--all shows archived notes:\n{}",
        stdout(&output)
    );
}

#[test]
fn vault_source_needs_no_config_or_adapter() {
    let env = GkeepEnv::new("bob-cli-gkeep-list-vault-only");
    // Even a broken config file must not matter for `-s vault`.
    env.write_config("gkeep: [unclosed\n");
    write_target(
        &env,
        "## Tasks\n\n- [ ] #task Solo vault task [created::2026-09-27]\n",
    );
    let fake = FakeAdapter::new(&env, "adapter");

    let mut command = env.command();
    command
        .arg("gkeep")
        .arg("list")
        .arg("-s")
        .arg("vault")
        .env("TZ", "UTC")
        .env("BOB_NOW", "2026-09-28 12:00:00")
        .env("COLUMNS", "200")
        .env("BOB_GKEEP_ADAPTER", fake.path());
    let output = command.output().expect("run list -s vault");
    assert_eq!(output.status.code(), Some(0));
    assert_eq!(fake.call_count(), 0, "vault-only makes no adapter call");
    let text = stdout(&output);
    assert!(text.contains("gkeep_inbox.md"));
    assert!(text.contains("Solo vault task"));
    assert!(
        !text.contains("Google Keep"),
        "no Keep section for -s vault:\n{text}"
    );
}

#[test]
fn marked_keep_source_link_is_compact_in_table_and_preserved_in_json() {
    let env = GkeepEnv::new("bob-cli-gkeep-list-source-icon");
    write_target(
        &env,
        "## Tasks\n\n- [ ] #task Call dentist [💡](https://keep.google.com/u/0/#NOTE/id \"Open in Google Keep\") [created::2026-09-27]\n  %%gkeep:v1:note-1:0123456789ab%%\n",
    );
    let fake = FakeAdapter::new(&env, "adapter");
    let table = list_command(&env, &fake, &["-s", "vault"])
        .output()
        .expect("run table");
    assert_eq!(table.status.code(), Some(0));
    let text = stdout(&table);
    assert!(text.contains("Call dentist 💡"), "{text}");
    assert!(!text.contains("keep.google.com"), "{text}");
    assert!(!text.contains("Open in Google Keep"), "{text}");

    let json = list_command(&env, &fake, &["-s", "vault", "-f", "json"])
        .output()
        .expect("run JSON");
    let document: serde_json::Value =
        serde_json::from_slice(&json.stdout).expect("parse JSON");
    let description = document["vault"]["tasks"][0]["description"]
        .as_str()
        .expect("description");
    assert!(description.contains("[💡](https://keep.google.com"));
    assert!(description.contains("Open in Google Keep"));
}

#[test]
fn keep_source_omits_the_vault_section() {
    let env = GkeepEnv::new("bob-cli-gkeep-list-keep-only");
    install_token_stub(&env);
    let fake = FakeAdapter::new(&env, "adapter");
    fake.respond(
        "snapshot",
        &snapshot_ok(
            "bryanbugyi34@gmail.com",
            vec![note("Keep only").id("keep-new-1").build()],
        ),
    );

    let output = list_command(&env, &fake, &["-s", "keep"])
        .output()
        .expect("run");
    assert_eq!(output.status.code(), Some(0));
    let text = stdout(&output);
    assert!(text.contains("Google Keep"));
    assert!(
        !text.contains("gkeep_inbox.md"),
        "no vault section for -s keep:\n{text}"
    );

    let output = list_command(&env, &fake, &["-s", "keep", "-f", "json"])
        .output()
        .expect("run json");
    let document: serde_json::Value =
        serde_json::from_str(&stdout(&output)).expect("parse json");
    assert!(document["vault"].is_null());
    assert!(document["keep"]["notes"].is_array());
}

#[test]
fn all_shows_archived_notes_and_done_tasks() {
    let env = GkeepEnv::new("bob-cli-gkeep-list-all");
    install_token_stub(&env);
    write_target(
        &env,
        "## Tasks\n\n- [ ] #task Open thing [created::2026-09-27]\n- [x] #task Done thing [created::2026-09-20]\n",
    );
    let fake = FakeAdapter::new(&env, "adapter");
    fake.respond(
        "snapshot",
        &snapshot_ok(
            "bryanbugyi34@gmail.com",
            vec![
                note("Fresh").id("keep-new-1").build(),
                note("Old").id("keep-archived-1").archived().build(),
            ],
        ),
    );

    let plain = list_command(&env, &fake, &[]).output().expect("run");
    assert!(!stdout(&plain).contains("Done thing"));
    assert!(!stdout(&plain).contains("archived"));

    let output = list_command(&env, &fake, &["--all"])
        .output()
        .expect("run --all");
    assert_eq!(output.status.code(), Some(0));
    let text = stdout(&output);
    assert!(
        text.contains("Done thing"),
        "--all shows done tasks:\n{text}"
    );
    assert!(
        text.contains("archived"),
        "--all shows archived notes:\n{text}"
    );
    assert!(text.contains("tasks ·"), "all-mode vault title:\n{text}");
}

#[test]
fn footer_covers_the_clear_and_actionable_cases() {
    let env = GkeepEnv::new("bob-cli-gkeep-list-footer");
    install_token_stub(&env);
    let fake = FakeAdapter::new(&env, "adapter");

    fake.respond("snapshot", &snapshot_ok("bryanbugyi34@gmail.com", vec![]));
    let output = list_command(&env, &fake, &[]).output().expect("run");
    let text = stdout(&output);
    assert!(
        text.contains("Keep inbox is empty ✓"),
        "empty state:\n{text}"
    );
    assert!(
        text.contains("✓ Keep inbox is clear"),
        "clear footer:\n{text}"
    );
    assert!(!text.contains("bob gkeep pull"), "no next step:\n{text}");

    fake.respond(
        "snapshot",
        &snapshot_ok(
            "bryanbugyi34@gmail.com",
            vec![note("Pinned").id("keep-pinned-1").pinned().build()],
        ),
    );
    let output = list_command(&env, &fake, &[]).output().expect("run");
    let text = stdout(&output);
    assert!(
        text.contains("✓ Keep inbox is clear · 1 pinned stays in Keep"),
        "clear footer names pinned notes:\n{text}"
    );

    fake.respond(
        "snapshot",
        &snapshot_ok(
            "bryanbugyi34@gmail.com",
            vec![
                note("One").id("keep-new-1").build(),
                note("Two").id("keep-new-2").build(),
            ],
        ),
    );
    let output = list_command(&env, &fake, &[]).output().expect("run");
    assert!(
        stdout(&output).contains("2 new  →  bob gkeep pull"),
        "actionable footer:\n{}",
        stdout(&output)
    );
}

#[test]
fn duplicate_markers_warn_on_stderr() {
    let env = GkeepEnv::new("bob-cli-gkeep-list-duplicates");
    install_token_stub(&env);
    let fp = fingerprint("Pending task", "");
    write_target(
        &env,
        &target_with_note(&format!("%%gkeep:v1:keep-pending-1:{fp}%%")),
    );
    fs::write(
        env.vault().join("copy.md"),
        format!("- [ ] #task Copy [created::2026-09-27]\n\t- Source: x %%gkeep:v1:keep-pending-1:{fp}%%\n"),
    )
    .expect("write duplicate marker");
    let fake = FakeAdapter::new(&env, "adapter");
    fake.respond(
        "snapshot",
        &snapshot_ok(
            "bryanbugyi34@gmail.com",
            vec![note("Pending task")
                .id("keep-pending-1")
                .created("2026-09-26T08:00:00Z")
                .build()],
        ),
    );

    let output = list_command(&env, &fake, &[]).output().expect("run");
    assert_eq!(output.status.code(), Some(0));
    let errors = stderr(&output);
    assert!(
        errors.contains("warning") && errors.contains("gkeep_inbox.md:"),
        "duplicate warning names the first location:\n{errors}"
    );
    assert!(
        errors.contains("copy.md:"),
        "duplicate warning names every location:\n{errors}"
    );

    let output = list_command(&env, &fake, &["-f", "json"])
        .output()
        .expect("run");
    let document: serde_json::Value =
        serde_json::from_str(&stdout(&output)).expect("parse json");
    assert_eq!(document["summary"]["duplicates"], 1);
}

#[test]
fn keep_auth_error_still_shows_the_vault() {
    let env = GkeepEnv::new("bob-cli-gkeep-list-keep-failure");
    install_token_stub(&env);
    write_target(
        &env,
        "## Tasks\n\n- [ ] #task Vault survivor [created::2026-09-27]\n",
    );
    let fake = FakeAdapter::new(&env, "adapter");
    fake.respond("snapshot", &error_response("auth", "login expired"));

    let output = list_command(&env, &fake, &[]).output().expect("run");
    assert_eq!(output.status.code(), Some(1));
    let text = stdout(&output);
    assert!(text.contains("login expired"), "error shown:\n{text}");
    assert!(
        text.contains("bob gkeep doctor"),
        "auth hint shown:\n{text}"
    );
    assert!(
        text.contains("Vault survivor"),
        "vault section still printed:\n{text}"
    );

    let output = list_command(&env, &fake, &["-f", "json"])
        .output()
        .expect("run");
    assert_eq!(output.status.code(), Some(1));
    let document: serde_json::Value =
        serde_json::from_str(&stdout(&output)).expect("parse json");
    assert_eq!(document["ok"], false);
    assert_eq!(document["keep"]["error"]["kind"], "auth");
    assert!(document["vault"]["tasks"].is_array());
}

#[test]
fn json_output_has_schema_version_and_sections() {
    let env = GkeepEnv::new("bob-cli-gkeep-list-json");
    install_token_stub(&env);
    let pending_fp = fingerprint("Pending task", "");
    write_target(
        &env,
        &target_with_note(&format!("%%gkeep:v1:keep-pending-1:{pending_fp}%%")),
    );
    let fake = FakeAdapter::new(&env, "adapter");
    fake.respond(
        "snapshot",
        &snapshot_ok(
            "bryanbugyi34@gmail.com",
            vec![
                note("Fresh idea")
                    .id("keep-new-1")
                    .created("2026-09-27T21:14:03Z")
                    .label("errands")
                    .list(vec![("screws", false, false), ("sand", true, false)])
                    .build(),
                note("Pending task")
                    .id("keep-pending-1")
                    .created("2026-09-26T08:00:00Z")
                    .build(),
            ],
        ),
    );

    let output = list_command(&env, &fake, &["-f", "json"])
        .output()
        .expect("run");
    assert_eq!(output.status.code(), Some(0));
    let document: serde_json::Value =
        serde_json::from_str(&stdout(&output)).expect("parse json");
    assert_eq!(document["schema_version"], 1);
    assert_eq!(document["ok"], true);
    assert_eq!(document["keep"]["account"], "bryanbugyi34@gmail.com");
    assert!(document["keep"]["fetched_at"].is_string());
    let notes = document["keep"]["notes"].as_array().expect("notes array");
    assert_eq!(notes.len(), 2);
    // Oldest first: the pending note was created first.
    assert_eq!(notes[0]["id"], "keep-pending-1");
    assert_eq!(notes[0]["state"], "pending");
    for key in [
        "id",
        "ref",
        "kind",
        "title",
        "state",
        "pinned",
        "shared",
        "archived",
        "labels",
        "created",
        "edited",
        "url",
        "fingerprint",
        "lines",
        "items_open",
        "items_checked",
        "attachments",
    ] {
        assert!(
            notes[1].get(key).is_some(),
            "keep note carries `{key}`:\n{document}"
        );
    }
    assert_eq!(notes[1]["kind"], "list");
    assert_eq!(notes[1]["state"], "new");
    assert_eq!(notes[1]["items_open"], 1);
    assert_eq!(notes[1]["items_checked"], 1);
    assert_eq!(notes[1]["fingerprint"].as_str().expect("fp").len(), 12);
    assert_eq!(notes[1]["ref"].as_str().expect("ref").len(), 7);

    let tasks = document["vault"]["tasks"].as_array().expect("tasks array");
    assert_eq!(tasks.len(), 1);
    for key in [
        "line",
        "status",
        "description",
        "created",
        "keep_id",
        "keep_state",
    ] {
        assert!(
            tasks[0].get(key).is_some(),
            "vault task carries `{key}`:\n{document}"
        );
    }
    assert_eq!(tasks[0]["keep_id"], "keep-pending-1");
    assert_eq!(tasks[0]["keep_state"], "pending");
    assert_eq!(document["vault"]["path"], "gkeep_inbox.md");

    for key in ["new", "pending", "revised", "skipped", "duplicates"] {
        assert!(
            document["summary"].get(key).is_some(),
            "summary carries `{key}`:\n{document}"
        );
    }
    assert_eq!(document["summary"]["new"], 1);
    assert_eq!(document["summary"]["pending"], 1);
}

#[test]
fn output_is_plain_when_piped() {
    let env = GkeepEnv::new("bob-cli-gkeep-list-plain");
    install_token_stub(&env);
    let pending_fp = fingerprint("Pending task", "");
    write_target(
        &env,
        &target_with_note(&format!("%%gkeep:v1:keep-pending-1:{pending_fp}%%")),
    );
    let fake = FakeAdapter::new(&env, "adapter");
    fake.respond(
        "snapshot",
        &snapshot_ok(
            "bryanbugyi34@gmail.com",
            vec![
                note("Fresh idea").id("keep-new-1").build(),
                note("Pending task").id("keep-pending-1").build(),
            ],
        ),
    );

    let output = list_command(&env, &fake, &[]).output().expect("run");
    assert_eq!(output.status.code(), Some(0));
    assert!(
        !output.stdout.contains(&0x1bu8),
        "table output stays plain when piped"
    );
}

#[test]
fn ages_follow_bob_now() {
    let env = GkeepEnv::new("bob-cli-gkeep-list-ages");
    install_token_stub(&env);
    write_target(
        &env,
        "## Tasks\n\n- [ ] #task Aged vault task [created::2026-09-26]\n",
    );
    let fake = FakeAdapter::new(&env, "adapter");
    fake.respond(
        "snapshot",
        &snapshot_ok(
            "bryanbugyi34@gmail.com",
            vec![note("Age probe note")
                .id("keep-new-1")
                .created("2026-09-23T12:00:00Z")
                .build()],
        ),
    );

    let output = list_command(&env, &fake, &[]).output().expect("run");
    assert_eq!(output.status.code(), Some(0));
    let text = stdout(&output);
    let keep_row = text
        .lines()
        .find(|line| line.contains("Age probe note"))
        .expect("keep row");
    assert!(
        keep_row.contains(" 5d "),
        "Keep age honors BOB_NOW:\n{keep_row}"
    );
    let vault_row = text
        .lines()
        .find(|line| line.contains("Aged vault task"))
        .expect("vault row");
    assert!(
        vault_row.contains(" 2d "),
        "vault age honors BOB_NOW:\n{vault_row}"
    );
}

#[test]
fn keep_age_is_correct_outside_utc() {
    let env = GkeepEnv::new("bob-cli-gkeep-list-tz");
    install_token_stub(&env);
    write_target(&env, "## Tasks\n");
    let fake = FakeAdapter::new(&env, "adapter");
    fake.respond(
        "snapshot",
        &snapshot_ok(
            "bryanbugyi34@gmail.com",
            vec![note("TZ probe note")
                .id("keep-tz-1")
                .created("2026-09-28T15:30:00Z")
                .build()],
        ),
    );

    // Local noon in New York (EDT, UTC-4) is 16:00 UTC; the note from
    // 15:30 UTC is 30 minutes old. The old `.and_utc()` code treated
    // local noon as 12:00 UTC and reported "now".
    let mut command = env.command();
    fake.install(&mut command);
    command
        .arg("gkeep")
        .arg("list")
        .env("TZ", "America/New_York")
        .env("BOB_NOW", "2026-09-28 12:00:00")
        .env("COLUMNS", "200");
    let output = command.output().expect("run");
    assert_eq!(output.status.code(), Some(0));
    let text = stdout(&output);
    let keep_row = text
        .lines()
        .find(|line| line.contains("TZ probe note"))
        .expect("keep row");
    assert!(
        keep_row.contains(" 30m "),
        "Keep age converts local time to UTC:\n{keep_row}"
    );
}

#[test]
fn dst_gap_reports_45m_not_now() {
    let env = GkeepEnv::new("bob-cli-gkeep-list-dst");
    install_token_stub(&env);
    write_target(&env, "## Tasks\n");
    let fake = FakeAdapter::new(&env, "adapter");
    fake.respond(
        "snapshot",
        &snapshot_ok(
            "bryanbugyi34@gmail.com",
            vec![note("DST probe")
                .id("keep-dst-1")
                .created("2026-03-08T06:45:00Z")
                .build()],
        ),
    );
    let mut command = env.command();
    fake.install(&mut command);
    command
        .arg("gkeep")
        .arg("list")
        .env("TZ", "America/New_York")
        .env("BOB_NOW", "2026-03-08 02:30")
        .env("COLUMNS", "200");
    let output = command.output().expect("run");
    assert_eq!(output.status.code(), Some(0));
    let text = stdout(&output);
    let row = text
        .lines()
        .find(|line| line.contains("DST probe"))
        .expect("keep row");
    assert!(
        row.contains(" 45m "),
        "DST gap retries +1h (old fallback shows now):\n{row}"
    );
}

#[test]
fn still_in_keep_covers_pinned_shared_and_empty() {
    let env = GkeepEnv::new("bob-cli-gkeep-list-still");
    install_token_stub(&env);
    // Vault tasks whose markers match pinned, shared, and empty Keep notes.
    let pinned_fp = fingerprint("Pinned note", "");
    let shared_fp = fingerprint("Shared note", "");
    // Empty note has no title/text/items; fingerprint of empty content.
    let empty_fp = fingerprint("", "");
    let target = format!(
        "## Tasks\n\n- [ ] #task Pinned note [created::2026-09-27]\n\t- Source: Google Keep · 2026-09-27 21:14 %%gkeep:v1:keep-pinned-1:{pinned_fp}%%\n- [ ] #task Shared note [created::2026-09-27]\n\t- Source: Google Keep · 2026-09-27 21:14 %%gkeep:v1:keep-shared-1:{shared_fp}%%\n- [ ] #task Empty [created::2026-09-27]\n\t- Source: Google Keep · 2026-09-27 21:14 %%gkeep:v1:keep-empty-1:{empty_fp}%%\n"
    );
    write_target(&env, &target);
    let fake = FakeAdapter::new(&env, "adapter");
    fake.respond(
        "snapshot",
        &snapshot_ok(
            "bryanbugyi34@gmail.com",
            vec![
                note("Pinned note").id("keep-pinned-1").pinned().build(),
                note("Shared note").id("keep-shared-1").shared().build(),
                note("").id("keep-empty-1").build(),
            ],
        ),
    );
    let output = list_command(&env, &fake, &[]).output().expect("run");
    assert_eq!(output.status.code(), Some(0));
    let text = stdout(&output);
    // All three vault rows show the footer hint.
    assert_eq!(
        text.matches("↺ still in Keep").count(),
        3,
        "pinned, shared, empty show still in Keep:\n{text}"
    );
    // With -s vault the hint never appears.
    let output = list_command(&env, &fake, &["-s", "vault"])
        .output()
        .expect("run vault");
    assert!(
        !stdout(&output).contains("↺ still in Keep"),
        "no still-in-Keep with -s vault:\n{}",
        stdout(&output)
    );
}

#[test]
fn vault_rows_sort_oldest_first_with_missing_last() {
    let env = GkeepEnv::new("bob-cli-gkeep-list-sort");
    install_token_stub(&env);
    write_target(
        &env,
        "## Tasks\n\n- [ ] #task Newest [created::2026-09-27]\n- [ ] #task No date\n- [ ] #task Oldest [created::2026-09-20]\n- [ ] #task Middle [created::2026-09-25]\n",
    );
    let fake = FakeAdapter::new(&env, "adapter");
    fake.respond("snapshot", &snapshot_ok("bryanbugyi34@gmail.com", vec![]));
    let output = list_command(&env, &fake, &["-s", "vault"])
        .output()
        .expect("run");
    assert_eq!(output.status.code(), Some(0));
    let text = stdout(&output);
    let pos_old = text.find("Oldest").expect("oldest");
    let pos_mid = text.find("Middle").expect("middle");
    let pos_new = text.find("Newest").expect("newest");
    let pos_nodate = text.find("No date").expect("nodate");
    assert!(
        pos_old < pos_mid && pos_mid < pos_new && pos_new < pos_nodate,
        "oldest first, nodate last:\n{text}"
    );
}

#[test]
fn missing_target_prints_exact_line() {
    let env = GkeepEnv::new("bob-cli-gkeep-list-missing");
    install_token_stub(&env);
    // No target file.
    let fake = FakeAdapter::new(&env, "adapter");
    fake.respond("snapshot", &snapshot_ok("bryanbugyi34@gmail.com", vec![]));
    let output = list_command(&env, &fake, &["-s", "vault"])
        .output()
        .expect("run");
    assert_eq!(output.status.code(), Some(0));
    let text = stdout(&output);
    assert!(
        text.contains(
            "  gkeep_inbox.md not found · create it or set gkeep.target"
        ),
        "exact missing line:\n{text}"
    );
}

#[test]
fn all_with_no_tasks_prints_no_tasks() {
    let env = GkeepEnv::new("bob-cli-gkeep-list-empty-all");
    install_token_stub(&env);
    write_target(&env, "## Tasks\n");
    let fake = FakeAdapter::new(&env, "adapter");
    fake.respond("snapshot", &snapshot_ok("bryanbugyi34@gmail.com", vec![]));
    let output = list_command(&env, &fake, &["-s", "vault", "--all"])
        .output()
        .expect("run");
    assert_eq!(output.status.code(), Some(0));
    assert!(
        stdout(&output).contains("No tasks"),
        "expected No tasks:\n{}",
        stdout(&output)
    );
}

#[test]
fn url_only_note_shows_ref_hint_and_json_verdict() {
    let env = GkeepEnv::new("bob-cli-gkeep-list-ref");
    install_token_stub(&env);
    write_target(&env, "## Tasks\n");
    let fake = FakeAdapter::new(&env, "adapter");
    let url_note = note("")
        .id("note-1")
        .text("https://example.com/post")
        .build();
    let task_note = note("Call dentist")
        .id("note-2")
        .text("They close at 5")
        .build();
    fake.respond(
        "snapshot",
        &snapshot_ok("bryanbugyi34@gmail.com", vec![url_note, task_note]),
    );
    let output = list_command(&env, &fake, &[]).output().expect("run");
    assert_eq!(output.status.code(), Some(0));
    let text = stdout(&output);
    assert!(text.contains("🔗 ref"), "ref hint:\n{text}");
    let output = list_command(&env, &fake, &["-f", "json"])
        .output()
        .expect("run");
    assert_eq!(output.status.code(), Some(0));
    let document: serde_json::Value =
        serde_json::from_str(&stdout(&output)).expect("json parses");
    let notes = document["keep"]["notes"].as_array().expect("notes array");
    assert_eq!(notes.len(), 2);
    assert_eq!(notes[0]["id"], "note-1");
    assert_eq!(notes[0]["state"], "new");
    assert_eq!(notes[0]["clip"]["url"], "https://example.com/post");
    assert_eq!(notes[0]["clip"]["display"], "example.com/post");
    assert_eq!(notes[0]["clip"]["verdict"], "not_found");
    assert!(
        notes[1].get("clip").is_none(),
        "ordinary notes carry no clip:\n{document}"
    );
}

#[test]
fn url_only_note_shows_parent_hint_for_resolved_and_asks_routes() {
    let env = GkeepEnv::new("bob-cli-gkeep-list-parent");
    install_token_stub(&env);
    write_target(&env, "## Tasks\n");
    fs::write(env.vault().join("sase.md"), "---\ntype: [[area]]\n---\n")
        .expect("sase parent");
    let fake = FakeAdapter::new(&env, "adapter");
    let routed = note("")
        .id("note-1")
        .text("https://example.com/a @sase")
        .build();
    let asks = note("").id("note-2").text("https://example.com/b").build();
    fake.respond(
        "snapshot",
        &snapshot_ok("bryanbugyi34@gmail.com", vec![routed, asks]),
    );
    let output = list_command(&env, &fake, &[]).output().expect("run");
    assert_eq!(output.status.code(), Some(0));
    let text = stdout(&output);
    assert!(text.contains("🔗 ref → sase"), "resolved hint:\n{text}");
    assert!(
        text.contains("🔗 ref → asks · gkeep_inbox"),
        "asks hint:\n{text}"
    );
    let output = list_command(&env, &fake, &["-f", "json"])
        .output()
        .expect("run");
    let document: serde_json::Value =
        serde_json::from_str(&stdout(&output)).expect("json parses");
    let notes = document["keep"]["notes"].as_array().expect("notes array");
    assert_eq!(notes[0]["clip"]["parent"], "sase");
    assert_eq!(notes[1]["clip"]["parent"], "gkeep_inbox");
}
