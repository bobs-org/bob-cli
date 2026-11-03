//! Clipboard notes under an existing task: `@route+id %c`.
use crate::support::*;
use std::fs;

fn write_clipboard(path: &std::path::Path, script: &str) {
    write_executable(path, script);
}

#[test]
fn clip_notes_are_direct_children_in_order_before_managed_logs() {
    let temp = TempDir::new("bob-cli-clip-notes-basic");
    let vault = temp.path().join("vault");
    let clipboard = temp.path().join("clipboard");
    write_clipboard(&clipboard, "#!/bin/sh\nprintf 'hello\\n'\n");
    write_file(
        &vault.join("cash.md"),
        "- [ ] #task Parent ^goog-exit\n\t- existing note\n",
    );

    for raw in ["@cash+goog-exit %c", "%c @cash+goog-exit"] {
        let output = bob_command()
            .arg("capture")
            .arg("-b")
            .arg(&vault)
            .arg(raw)
            .env("BOB_CLIPBOARD_CMD", &clipboard)
            .env("BOB_NOW", "2026-07-31")
            .output()
            .expect("clipboard notes capture");
        assert_success(&output);
    }
    // Two runs appended two direct children in order.
    assert_eq!(
        fs::read_to_string(vault.join("cash.md")).expect("read note"),
        concat!(
            "- [ ] #task Parent ^goog-exit\n",
            "\t- existing note\n",
            "\t- hello\n",
            "\t- hello\n",
        )
    );

    // Two-line clip lands as two direct children before a managed log.
    let vault2 = temp.path().join("vault2");
    write_clipboard(&clipboard, "#!/bin/sh\nprintf 'first\\nsecond\\n'\n");
    write_file(
        &vault2.join("cash.md"),
        concat!(
            "- [ ] #task Parent ^goog-exit\n",
            "\t- keep me\n",
            "\t- \u{1F5D3}\u{FE0F} **SCHEDULE LOG**\n",
            "\t\t- *2026-08-01* — keep\n",
        ),
    );
    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault2)
        .arg("@cash+goog-exit %c")
        .env("BOB_CLIPBOARD_CMD", &clipboard)
        .env("BOB_NOW", "2026-07-31")
        .output()
        .expect("two-line clipboard notes");
    assert_success(&output);
    assert_eq!(
        fs::read_to_string(vault2.join("cash.md")).expect("read note2"),
        concat!(
            "- [ ] #task Parent ^goog-exit\n",
            "\t- keep me\n",
            "\t- first\n",
            "\t- second\n",
            "\t- \u{1F5D3}\u{FE0F} **SCHEDULE LOG**\n",
            "\t\t- *2026-08-01* — keep\n",
        )
    );

    // Two-space indented notes use two spaces.
    let vault3 = temp.path().join("vault3");
    write_clipboard(&clipboard, "#!/bin/sh\nprintf 'spaced\\n'\n");
    write_file(
        &vault3.join("cash.md"),
        "- [ ] #task Parent ^goog-exit\n  - existing\n",
    );
    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault3)
        .arg("@cash+goog-exit %c")
        .env("BOB_CLIPBOARD_CMD", &clipboard)
        .env("BOB_NOW", "2026-07-31")
        .output()
        .expect("spaced clipboard notes");
    assert_success(&output);
    assert_eq!(
        fs::read_to_string(vault3.join("cash.md")).expect("read spaced"),
        concat!(
            "- [ ] #task Parent ^goog-exit\n",
            "  - existing\n",
            "  - spaced\n",
        )
    );
}

#[test]
fn clip_notes_flatten_history_in_order() {
    let temp = TempDir::new("bob-cli-clip-notes-history");
    let vault = temp.path().join("vault");
    let clipboard = temp.path().join("clipboard");
    let history = temp.path().join("history");
    write_clipboard(&clipboard, "#!/bin/sh\nprintf 'live\\n'\n");
    write_executable(
        &history,
        concat!(
            "#!/bin/sh\n",
            "[ \"$1\" = 2 ] || { echo wrong-count >&2; exit 42; }\n",
            "printf '%s\\n' '[\"live\",\"older\"]'\n",
        ),
    );
    write_file(&vault.join("cash.md"), "- [ ] #task Parent ^id\n");

    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("-f")
        .arg("json")
        .arg("@cash+id %c2")
        .env("BOB_CLIPBOARD_CMD", &clipboard)
        .env("BOB_CLIPBOARD_HISTORY_CMD", &history)
        .env("BOB_NOW", "2026-07-31")
        .output()
        .expect("history clipboard notes");
    assert_success(&output);
    let json: serde_json::Value =
        serde_json::from_str(stdout(&output).trim()).expect("history JSON");
    assert_eq!(json["kind"], "sub_bullet");
    assert_eq!(json["clip"]["mode"], "history");
    assert_eq!(
        json["clip"]["lines"],
        serde_json::json!(["- live", "- older"])
    );
    assert_eq!(
        fs::read_to_string(vault.join("cash.md")).expect("read note"),
        concat!("- [ ] #task Parent ^id\n", "\t- live\n", "\t- older\n",)
    );
}

#[test]
fn clip_notes_attachments_and_snippets_are_direct_children() {
    let temp = TempDir::new("bob-cli-clip-notes-files");
    let vault = temp.path().join("vault");
    let clipboard = temp.path().join("clipboard");
    let source = temp.path().join("doc.txt");
    fs::write(&source, b"attachment bytes").expect("write source");
    write_executable(
        &clipboard,
        &format!(
            "#!/bin/sh\nprintf '%s\\n' {}\n",
            shell_single_quote(source.to_str().expect("utf8 source"))
        ),
    );
    write_file(&vault.join("cash.md"), "- [ ] #task Parent ^id\n");

    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("-f")
        .arg("json")
        .arg("@cash+id %c")
        .env("BOB_CLIPBOARD_CMD", &clipboard)
        .env("BOB_NOW", "2026-07-31 13:14:15")
        .output()
        .expect("attachment clipboard notes");
    assert_success(&output);
    let json: serde_json::Value =
        serde_json::from_str(stdout(&output).trim()).expect("attach JSON");
    assert_eq!(json["clip"]["mode"], "attachments");
    assert_eq!(
        fs::read_to_string(vault.join("cash.md")).expect("read attach"),
        concat!("- [ ] #task Parent ^id\n", "\t- [[file/doc.txt]]\n",)
    );

    // Snippet fallback creates file/clip-*.md and links it directly.
    write_clipboard(
        &clipboard,
        "#!/bin/sh\nprintf '# Heading\\n\\n- preserved list\\n'\n",
    );
    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("-f")
        .arg("json")
        .arg("@cash+id %c")
        .env("BOB_CLIPBOARD_CMD", &clipboard)
        .env("BOB_NOW", "2026-07-31 13:14:15")
        .output()
        .expect("snippet clipboard notes");
    assert_success(&output);
    let json: serde_json::Value =
        serde_json::from_str(stdout(&output).trim()).expect("snippet JSON");
    assert_eq!(json["clip"]["mode"], "snippet");
    let snippet = json["clip"]["snippet"].as_str().expect("snippet path");
    assert!(snippet.starts_with("file/clip-"), "{json}");
    let reference = snippet.strip_suffix(".md").unwrap_or(snippet);
    assert!(fs::read_to_string(vault.join("cash.md"))
        .expect("read snippet note")
        .contains(&format!("\t- [[{reference}]]")));
    assert!(vault.join(snippet).exists());
}

#[test]
fn clip_notes_drawers_and_prj_sections() {
    let temp = TempDir::new("bob-cli-clip-notes-drawers");
    let vault = temp.path().join("vault");
    let clipboard = temp.path().join("clipboard");
    write_clipboard(&clipboard, "#!/bin/sh\nprintf 'note one\\nnote two\\n'\n");
    write_file(
        &vault.join("cash.md"),
        concat!(
            "- [ ] #task Parent ^id\n",
            "\t- REQUIREMENTS\n",
            "\t\t- existing\n",
        ),
    );

    // Into an existing drawer.
    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("@cash+id#requirements %c")
        .env("BOB_CLIPBOARD_CMD", &clipboard)
        .env("BOB_NOW", "2026-07-31")
        .output()
        .expect("existing drawer clipboard notes");
    assert_success(&output);
    assert_eq!(
        fs::read_to_string(vault.join("cash.md")).expect("read drawer"),
        concat!(
            "- [ ] #task Parent ^id\n",
            "\t- REQUIREMENTS\n",
            "\t\t- existing\n",
            "\t\t- note one\n",
            "\t\t- note two\n",
        )
    );

    // Into a missing drawer (created with the lines).
    let vault2 = temp.path().join("vault2");
    write_file(&vault2.join("cash.md"), "- [ ] #task Parent ^id\n");
    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault2)
        .arg("-f")
        .arg("json")
        .arg("@cash+id#requirements %c")
        .env("BOB_CLIPBOARD_CMD", &clipboard)
        .env("BOB_NOW", "2026-07-31")
        .output()
        .expect("missing drawer clipboard notes");
    assert_success(&output);
    let json: serde_json::Value =
        serde_json::from_str(stdout(&output).trim()).expect("drawer JSON");
    assert_eq!(json["drawer"]["title"], "REQUIREMENTS");
    assert_eq!(json["drawer"]["created"], true);
    assert_eq!(
        fs::read_to_string(vault2.join("cash.md")).expect("read created"),
        concat!(
            "- [ ] #task Parent ^id\n",
            "\t- REQUIREMENTS\n",
            "\t\t- note one\n",
            "\t\t- note two\n",
        )
    );

    // On a ^prj target the selector names a ## section.
    let vault3 = temp.path().join("vault3");
    write_file(
        &vault3.join("proj.md"),
        concat!(
            "---\n",
            "type: \"[[project]]\"\n",
            "---\n",
            "\n",
            "# Project\n",
            "\n",
            "- [ ] #task #prj Ship it ^prj\n",
            "\n",
            "## Requirements\n",
            "\n",
            "notes\n",
        ),
    );
    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault3)
        .arg("@proj+prj#requirements %c")
        .env("BOB_CLIPBOARD_CMD", &clipboard)
        .env("BOB_NOW", "2026-07-31")
        .output()
        .expect("prj clipboard notes");
    assert_success(&output);
    let after = fs::read_to_string(vault3.join("proj.md")).expect("read prj");
    assert!(
        after.contains("- note one") && after.contains("- note two"),
        "{after}"
    );
    assert!(after.contains("## Requirements"), "{after}");
}

#[test]
fn clip_notes_retype_forced_clip_json_and_human() {
    let temp = TempDir::new("bob-cli-clip-notes-retype");
    let vault = temp.path().join("vault");
    let clipboard = temp.path().join("clipboard");
    let config = temp.path().join("config.yml");
    write_clipboard(&clipboard, "#!/bin/sh\nprintf 'clip note\\n'\n");
    write_toggle_task_settings(&vault);
    write_file(&config, "plan:\nthemes:\n  FIX: blue\n");
    write_file(&vault.join("cash.md"), "- [ ] #task Parent ^id\n");
    let day = vault.join("2026/20260731.md");
    write_file(&day, "# Day\n");

    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("-f")
        .arg("json")
        .arg("@cash+id #fix %c")
        .env("BOB_CLIPBOARD_CMD", &clipboard)
        .env("BOB_CONFIG_FILE", &config)
        .env("BOB_NOW", "2026-07-31")
        .output()
        .expect("retype clipboard notes");
    assert_success(&output);
    let json: serde_json::Value =
        serde_json::from_str(stdout(&output).trim()).expect("retype JSON");
    assert_eq!(json["kind"], "sub_bullet");
    assert_eq!(json["text"], "");
    assert_eq!(json["task_line"], "");
    assert_eq!(json["clip"]["lines"], serde_json::json!(["- clip note"]));
    assert!(fs::read_to_string(vault.join("cash.md"))
        .expect("read retyped")
        .contains("[type:: FIX]"));
    // Status checkbox unchanged and daily note untouched.
    assert!(fs::read_to_string(vault.join("cash.md"))
        .expect("read status")
        .starts_with("- [ ] #task"));
    assert_eq!(fs::read_to_string(&day).expect("read day"), "# Day\n");

    // --clip matches %c byte for byte.
    let vault_a = temp.path().join("vault-a");
    let vault_b = temp.path().join("vault-b");
    for dir in [&vault_a, &vault_b] {
        write_file(&dir.join("cash.md"), "- [ ] #task Parent ^id\n");
    }
    let out_a = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault_a)
        .arg("@cash+id %c")
        .env("BOB_CLIPBOARD_CMD", &clipboard)
        .env("BOB_NOW", "2026-07-31")
        .output()
        .expect("percent clip");
    assert_success(&out_a);
    let out_b = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault_b)
        .arg("--clip")
        .arg("@cash+id")
        .env("BOB_CLIPBOARD_CMD", &clipboard)
        .env("BOB_NOW", "2026-07-31")
        .output()
        .expect("forced clip");
    assert_success(&out_b);
    assert_eq!(
        fs::read_to_string(vault_a.join("cash.md")).expect("read a"),
        fs::read_to_string(vault_b.join("cash.md")).expect("read b")
    );

    // Explicit toggle still refuses both spellings.
    for args in [vec!["@cash+id! %c"], vec!["--clip", "@cash+id!"]] {
        let output = bob_command()
            .arg("capture")
            .arg("-b")
            .arg(&vault)
            .args(&args)
            .env("BOB_CLIPBOARD_CMD", &clipboard)
            .env("BOB_NOW", "2026-07-31")
            .output()
            .expect("explicit toggle refusal");
        assert!(!output.status.success(), "{:?}", args);
    }

    // Human output has no blank task_line row.
    let vault_h = temp.path().join("vault-h");
    write_file(&vault_h.join("cash.md"), "- [ ] #task Parent ^id\n");
    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault_h)
        .arg("@cash+id %c")
        .env("BOB_CLIPBOARD_CMD", &clipboard)
        .env("BOB_NOW", "2026-07-31")
        .output()
        .expect("human clipboard notes");
    assert_success(&output);
    let human = String::from_utf8_lossy(&output.stdout).to_string();
    assert!(human.contains("under [ ] Parent"), "{human}");
    assert!(human.contains("- clip note"), "{human}");
}

#[test]
fn clip_notes_dry_run_no_clip_atomicity() {
    let temp = TempDir::new("bob-cli-clip-notes-dryrun");
    let vault = temp.path().join("vault");
    let clipboard = temp.path().join("clipboard");
    write_clipboard(&clipboard, "#!/bin/sh\nprintf 'hello\\n'\n");
    write_file(&vault.join("cash.md"), "- [ ] #task Parent ^id\n");
    let before =
        fs::read_to_string(vault.join("cash.md")).expect("read before");

    // Dry run with --no-clip validates but stages nothing.
    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("--dry-run")
        .arg("--no-clip")
        .arg("-f")
        .arg("json")
        .arg("@cash+id %c")
        .env("BOB_CLIPBOARD_CMD", &clipboard)
        .env("BOB_NOW", "2026-07-31")
        .output()
        .expect("dry-run no-clip");
    assert_success(&output);
    let json: serde_json::Value =
        serde_json::from_str(stdout(&output).trim()).expect("dry JSON");
    assert_eq!(json["clip"]["status"], "not_read");
    assert_eq!(
        fs::read_to_string(vault.join("cash.md")).expect("read after dry"),
        before
    );

    // Missing block ID reports the same error as body-bearing capture.
    let missing_clip = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("--dry-run")
        .arg("--no-clip")
        .arg("-f")
        .arg("json")
        .arg("@cash+missing %c")
        .env("BOB_CLIPBOARD_CMD", &clipboard)
        .env("BOB_NOW", "2026-07-31")
        .output()
        .expect("missing dry");
    let missing_body = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("--dry-run")
        .arg("--no-clip")
        .arg("-f")
        .arg("json")
        .arg("@cash+missing note")
        .env("BOB_CLIPBOARD_CMD", &clipboard)
        .env("BOB_NOW", "2026-07-31")
        .output()
        .expect("missing body dry");
    assert!(!missing_clip.status.success());
    assert!(!missing_body.status.success());
    assert_eq!(
        String::from_utf8_lossy(&missing_clip.stdout),
        String::from_utf8_lossy(&missing_body.stdout)
    );

    // A real capture with --no-clip refuses.
    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("--no-clip")
        .arg("-f")
        .arg("json")
        .arg("@cash+id %c")
        .env("BOB_CLIPBOARD_CMD", &clipboard)
        .env("BOB_NOW", "2026-07-31")
        .output()
        .expect("real no-clip");
    assert!(!output.status.success());
    let json: serde_json::Value =
        serde_json::from_str(stdout(&output).trim()).expect("refusal JSON");
    assert_eq!(json["code"], "clip_not_read");

    // Batch atomicity: a snippet-sized clip plus a failing item rolls back.
    let vault2 = temp.path().join("vault2");
    write_file(&vault2.join("cash.md"), "- [ ] #task Parent ^id\n");
    let big = "line\n".repeat(12);
    let escaped = big.replace('\n', "\\n");
    let clip_script = format!("#!/bin/sh\nprintf '{escaped}'\n");
    write_clipboard(&clipboard, &clip_script);
    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault2)
        .arg("@cash+id %c\n\n@cash+missing note")
        .env("BOB_CLIPBOARD_CMD", &clipboard)
        .env("BOB_NOW", "2026-07-31 13:14:15")
        .output()
        .expect("atomic batch");
    assert!(!output.status.success(), "{}", format_output(&output));
    assert_eq!(
        fs::read_to_string(vault2.join("cash.md")).expect("read atomic"),
        "- [ ] #task Parent ^id\n"
    );
    assert!(
        fs::read_dir(vault2.join("file"))
            .map(|mut d| d.next().is_none())
            .unwrap_or(true),
        "no snippet file should remain"
    );
}

#[test]
fn clip_notes_keep_relationship_refusal() {
    let temp = TempDir::new("bob-cli-clip-notes-relationship");
    let vault = temp.path().join("vault");
    let clipboard = temp.path().join("clipboard");
    write_clipboard(&clipboard, "#!/bin/sh\nprintf 'hello\\n'\n");
    write_file(
        &vault.join("cash.md"),
        concat!("- [ ] #task Parent ^id\n", "- [ ] #task Other ^other\n",),
    );
    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("-f")
        .arg("json")
        .arg("@cash+id %c &cash:other")
        .env("BOB_CLIPBOARD_CMD", &clipboard)
        .env("BOB_NOW", "2026-07-31")
        .output()
        .expect("relationship refusal");
    assert!(!output.status.success());
    let json: serde_json::Value =
        serde_json::from_str(stdout(&output).trim()).expect("refusal JSON");
    let message = json["error"].as_str().unwrap_or_default();
    assert!(
        message.contains("needs task text"),
        "expected relationship_needs_text refusal, got: {json}"
    );
}
