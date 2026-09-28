//! Clip, history, clipboard and percent-one.

use crate::support::*;
use sha2::Digest;
use sha2::Sha256;
use std::fs;

#[test]
fn capture_clip_marker_composes_with_schedule_routes_bullets_and_pomodoro() {
    let temp = TempDir::new("bob-cli-capture-clip-compose");
    let vault = temp.path().join("vault");
    let clipboard = temp.path().join("clipboard");
    fs::create_dir_all(&vault).expect("create vault");
    write_executable(
        &clipboard,
        "#!/bin/sh\nprintf 'hello from clipboard\n'\n",
    );
    write_file(
        &vault.join("work.md"),
        "# Work\n## Tasks\n- [ ] #task Existing\n",
    );

    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("-f")
        .arg("json")
        .args(["do thing", "s:1", "%build_log", "@work"])
        .env("BOB_CLIPBOARD_CMD", &clipboard)
        .env("BOB_NOW", "2026-07-15 13:14:15")
        .output()
        .expect("run task clipboard capture");
    assert_success(&output);
    let json: serde_json::Value = serde_json::from_str(stdout(&output).trim())
        .unwrap_or_else(|error| {
            panic!("clipboard JSON: {error}\n{}", format_output(&output))
        });
    assert_eq!(json["text"], "do thing");
    assert_eq!(json["scheduled"], "2026-07-16");
    assert_eq!(json["clip"]["header"], "BUILD LOG");
    assert_eq!(json["clip"]["mode"], "inline");
    assert_eq!(
        json["clip"]["lines"],
        serde_json::json!(["\t- **BUILD LOG:** hello from clipboard"])
    );
    assert_eq!(
        fs::read_to_string(vault.join("work.md")).expect("read work note"),
        concat!(
            "# Work\n",
            "## Tasks\n",
            "- [ ] #task Existing\n",
            "- [?] #task do thing [created::2026-07-15] [scheduled::2026-07-16]\n",
            "\t- **BUILD LOG:** hello from clipboard\n",
        )
    );

    write_file(&vault.join("notes.md"), "# Notes\n## Ideas\n");
    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .args(["jot idea", "%", "@notes#Ideas"])
        .env("BOB_CLIPBOARD_CMD", &clipboard)
        .env("BOB_NOW", "2026-07-15")
        .output()
        .expect("run bullet clipboard capture");
    assert_success(&output);
    assert_eq!(
        fs::read_to_string(vault.join("notes.md")).expect("read notes"),
        concat!(
            "# Notes\n",
            "## Ideas\n",
            "\n",
            "- jot idea [created::2026-07-15]\n",
            "\t- hello from clipboard\n",
        )
    );

    let day_file = vault.join("day.md");
    write_file(&vault.join("dev.md"), "# Dev\n## Tasks\n");
    write_file(&day_file, "## Pomodoros\n- [ ] Current\n");
    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .args(["ship it", "%log", "@dev:ship-id"])
        .env("BOB_CLIPBOARD_CMD", &clipboard)
        .env("BOB_DAY_FILE", &day_file)
        .env("BOB_NOW", "2026-07-15")
        .output()
        .expect("run Pomodoro clipboard capture");
    assert_success(&output);
    assert_eq!(
        fs::read_to_string(vault.join("dev.md")).expect("read dev"),
        concat!(
            "# Dev\n",
            "## Tasks\n",
            "\n",
            "- [*] #task ship it [created::2026-07-15] ^ship-id\n",
            "\t- **LOG:** hello from clipboard\n",
        )
    );
    assert_eq!(
        fs::read_to_string(day_file).expect("read day"),
        "## Pomodoros\n- [ ] Current\n  - [[dev#^ship-id]]\n"
    );
}

#[test]
fn capture_headerless_clip_marker_renders_under_tasks_and_pomodoros() {
    let temp = TempDir::new("bob-cli-capture-headerless-clip-placement");
    let vault = temp.path().join("vault");
    let clipboard = temp.path().join("clipboard");
    fs::create_dir_all(&vault).expect("create vault");
    write_executable(&clipboard, "#!/bin/sh\nprintf 'clipboard child\n'\n");

    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("-f")
        .arg("json")
        .args(["task parent", "%"])
        .env("BOB_CLIPBOARD_CMD", &clipboard)
        .env("BOB_NOW", "2026-07-15")
        .output()
        .expect("run headerless task clipboard capture");
    assert_success(&output);
    let json: serde_json::Value = serde_json::from_str(stdout(&output).trim())
        .unwrap_or_else(|error| {
            panic!("clipboard JSON: {error}\n{}", format_output(&output))
        });
    assert_eq!(json["clip"]["header"], serde_json::Value::Null);
    assert_eq!(
        json["clip"]["lines"],
        serde_json::json!(["\t- clipboard child"])
    );
    assert_eq!(
        fs::read_to_string(vault.join("mac_inbox.md")).expect("read inbox"),
        concat!(
            "- [ ] #task task parent [created::2026-07-15]\n",
            "\t- clipboard child\n",
        )
    );

    let day_file = vault.join("day.md");
    write_file(&vault.join("dev.md"), "# Dev\n## Tasks\n");
    write_file(&day_file, "## Pomodoros\n- [ ] Current\n");
    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .args(["Pomodoro parent", "%", "@dev:clip-id"])
        .env("BOB_CLIPBOARD_CMD", &clipboard)
        .env("BOB_DAY_FILE", &day_file)
        .env("BOB_NOW", "2026-07-15")
        .output()
        .expect("run headerless Pomodoro clipboard capture");
    assert_success(&output);
    assert_eq!(
        fs::read_to_string(vault.join("dev.md")).expect("read dev"),
        concat!(
            "# Dev\n",
            "## Tasks\n",
            "\n",
            "- [*] #task Pomodoro parent [created::2026-07-15] ^clip-id\n",
            "\t- clipboard child\n",
        )
    );
    assert_eq!(
        fs::read_to_string(day_file).expect("read day"),
        "## Pomodoros\n- [ ] Current\n  - [[dev#^clip-id]]\n"
    );
}

#[test]
fn capture_clip_uses_each_target_notes_indent_and_tabs_for_a_fresh_note() {
    let temp = TempDir::new("bob-cli-capture-clip-target-indent");
    let vault = temp.path().join("vault");
    let clipboard = temp.path().join("clipboard");
    write_file(
        &vault.join("tabbed.md"),
        "# Tabbed\n- [ ] #task Existing\n\t- existing child\n",
    );
    write_file(
        &vault.join("spaced.md"),
        "# Spaced\n- [ ] #task Existing\n  - existing child\n",
    );
    write_executable(&clipboard, "#!/bin/sh\nprintf 'shared clipboard\n'\n");

    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .args(["tab parent", "%", "@tabbed"])
        .env("BOB_CLIPBOARD_CMD", &clipboard)
        .env("BOB_NOW", "2026-07-15")
        .output()
        .expect("capture into tab-indented note");
    assert_success(&output);
    assert!(fs::read_to_string(vault.join("tabbed.md"))
        .expect("read tab-indented note")
        .contains(concat!(
            "- [ ] #task tab parent [created::2026-07-15]\n",
            "\t- shared clipboard\n",
        )));

    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .args(["space parent", "%", "@spaced"])
        .env("BOB_CLIPBOARD_CMD", &clipboard)
        .env("BOB_NOW", "2026-07-15")
        .output()
        .expect("capture into two-space-indented note");
    assert_success(&output);
    assert!(fs::read_to_string(vault.join("spaced.md"))
        .expect("read two-space-indented note")
        .contains(concat!(
            "- [ ] #task space parent [created::2026-07-15]\n",
            "  - shared clipboard\n",
        )));

    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .args(["fresh parent", "%"])
        .env("BOB_CLIPBOARD_CMD", &clipboard)
        .env("BOB_NOW", "2026-07-15")
        .output()
        .expect("capture into fresh inbox note");
    assert_success(&output);
    assert_eq!(
        fs::read_to_string(vault.join("mac_inbox.md"))
            .expect("read fresh inbox note"),
        concat!(
            "- [ ] #task fresh parent [created::2026-07-15]\n",
            "\t- shared clipboard\n",
        )
    );
}

#[test]
fn capture_flat_clipboard_list_routes_normalized_children() {
    let temp = TempDir::new("bob-cli-capture-flat-clipboard-list");
    let vault = temp.path().join("vault");
    let clipboard = temp.path().join("clipboard");
    fs::create_dir_all(&vault).expect("create vault");
    write_executable(
        &clipboard,
        concat!(
            "#!/bin/sh\n",
            "printf '%s\\n' '- Use `@` symbol instead of `#` for tribe prefix.'\n",
            "printf '%s\\n' '- Support expansion of families within clan.'\n",
            "printf '%s\\n' '- Family members must be launched sequentially.'\n",
        ),
    );

    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("-f")
        .arg("json")
        .arg("foo bar baz @foo %")
        .env("BOB_CLIPBOARD_CMD", &clipboard)
        .env("BOB_NOW", "2026-07-17")
        .output()
        .expect("capture routed clipboard list");
    assert_success(&output);
    let json: serde_json::Value = serde_json::from_str(stdout(&output).trim())
        .unwrap_or_else(|error| {
            panic!("clipboard JSON: {error}\n{}", format_output(&output))
        });
    assert_eq!(json["clip"]["mode"], "lines");
    assert_eq!(
        json["clip"]["lines"],
        serde_json::json!([
            "\t- Use `@` symbol instead of `#` for tribe prefix.",
            "\t- Support expansion of families within clan.",
            "\t- Family members must be launched sequentially.",
        ])
    );
    assert_eq!(
        json["task_line"],
        "- [ ] #task foo bar baz [created::2026-07-17]"
    );
    assert_eq!(
        fs::read_to_string(vault.join("foo.md")).expect("read routed note"),
        concat!(
            "- [ ] #task foo bar baz [created::2026-07-17]\n",
            "\t- Use `@` symbol instead of `#` for tribe prefix.\n",
            "\t- Support expansion of families within clan.\n",
            "\t- Family members must be launched sequentially.\n",
        )
    );
}

#[test]
fn capture_percent_one_is_an_exact_single_clip_alias() {
    let temp = TempDir::new("bob-cli-capture-percent-one");
    let vault = temp.path().join("vault");
    let clipboard = temp.path().join("clipboard");
    fs::create_dir_all(&vault).expect("create vault");
    write_executable(&clipboard, "#!/bin/sh\nprintf 'live value\n'\n");

    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("-f")
        .arg("json")
        .arg("single %1")
        .env("BOB_CLIPBOARD_CMD", &clipboard)
        .env_remove("BOB_CLIPBOARD_HISTORY_CMD")
        .env("BOB_NOW", "2026-07-15")
        .output()
        .expect("capture %1");
    assert_success(&output);
    let json: serde_json::Value =
        serde_json::from_str(stdout(&output).trim()).expect("single JSON");
    assert_eq!(json["clip"]["header"], serde_json::Value::Null);
    assert_eq!(json["clip"]["mode"], "inline");
    assert_eq!(json["clip"]["lines"], serde_json::json!(["\t- live value"]));
    assert_eq!(json["clip"]["entries"], serde_json::json!([]));
    assert_eq!(
        fs::read_to_string(vault.join("mac_inbox.md")).expect("read inbox"),
        concat!(
            "- [ ] #task single [created::2026-07-15]\n",
            "\t- live value\n",
        )
    );
}

#[test]
fn capture_history_is_headerless_structured_and_composes_with_routes() {
    let temp = TempDir::new("bob-cli-capture-history-compose");
    let vault = temp.path().join("vault");
    let clipboard = temp.path().join("clipboard");
    let history = temp.path().join("history");
    fs::create_dir_all(&vault).expect("create vault");
    write_executable(&clipboard, "#!/bin/sh\nprintf 'live value\n'\n");
    write_executable(
        &history,
        concat!(
            "#!/bin/sh\n",
            "[ \"$1\" = 3 ] || { echo wrong-count >&2; exit 42; }\n",
            "printf '%s\\n' '[\"live value\",\"older one\\nolder two\",\"oldest\"]'\n",
        ),
    );

    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("-f")
        .arg("json")
        .args(["research links", "s:1", "%3"])
        .env("BOB_CLIPBOARD_CMD", &clipboard)
        .env("BOB_CLIPBOARD_HISTORY_CMD", &history)
        .env("BOB_NOW", "2026-07-15")
        .output()
        .expect("capture task history");
    assert_success(&output);
    let json: serde_json::Value =
        serde_json::from_str(stdout(&output).trim()).expect("history JSON");
    assert_eq!(json["clip"]["header"], serde_json::Value::Null);
    assert_eq!(json["clip"]["mode"], "history");
    assert_eq!(
        json["clip"]["lines"],
        serde_json::json!([
            "\t- live value",
            "\t- older one",
            "\t- older two",
            "\t- oldest",
        ])
    );
    assert_eq!(json["clip"]["entries"].as_array().unwrap().len(), 3);
    assert_eq!(json["clip"]["entries"][0]["mode"], "inline");
    assert_eq!(json["clip"]["entries"][1]["mode"], "lines");
    assert_eq!(json["clip"]["entries"][2]["mode"], "inline");
    for entry in json["clip"]["entries"].as_array().unwrap() {
        assert_eq!(entry["entries"], serde_json::json!([]));
    }
    assert!(json["clip"].get("snippet").is_none(), "{json}");
    assert_eq!(
        fs::read_to_string(vault.join("mac_inbox.md")).expect("read inbox"),
        concat!(
            "- [?] #task research links [created::2026-07-15] [scheduled::2026-07-16]\n",
            "\t- live value\n",
            "\t- older one\n",
            "\t- older two\n",
            "\t- oldest\n",
        )
    );

    write_file(&vault.join("notes.md"), "# Notes\n## Ideas\n");
    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .args(["history bullet", "@notes#Ideas", "%3"])
        .env("BOB_CLIPBOARD_CMD", &clipboard)
        .env("BOB_CLIPBOARD_HISTORY_CMD", &history)
        .env("BOB_NOW", "2026-07-15")
        .output()
        .expect("capture bullet history");
    assert_success(&output);
    let human = stdout(&output);
    assert!(
        human.contains("\t- live value")
            && human.contains("\t- older one")
            && human.contains("\t- oldest")
            && !human.contains("entry 1")
            && !human.contains("**CLIP:**"),
        "{}",
        format_output(&output)
    );
    assert_eq!(
        fs::read_to_string(vault.join("notes.md")).expect("read notes"),
        concat!(
            "# Notes\n",
            "## Ideas\n",
            "\n",
            "- history bullet [created::2026-07-15]\n",
            "\t- live value\n",
            "\t- older one\n",
            "\t- older two\n",
            "\t- oldest\n",
        )
    );

    let day_file = vault.join("day.md");
    write_file(&vault.join("dev.md"), "# Dev\n## Tasks\n");
    write_file(&day_file, "## Pomodoros\n- [ ] Current\n");
    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .args(["history pomodoro", "%3", "@dev:history-id"])
        .env("BOB_CLIPBOARD_CMD", &clipboard)
        .env("BOB_CLIPBOARD_HISTORY_CMD", &history)
        .env("BOB_DAY_FILE", &day_file)
        .env("BOB_NOW", "2026-07-15")
        .output()
        .expect("capture Pomodoro history");
    assert_success(&output);
    assert_eq!(
        fs::read_to_string(vault.join("dev.md")).expect("read dev"),
        concat!(
            "# Dev\n",
            "## Tasks\n",
            "\n",
            "- [*] #task history pomodoro [created::2026-07-15] ^history-id\n",
            "\t- live value\n",
            "\t- older one\n",
            "\t- older two\n",
            "\t- oldest\n",
        )
    );
    assert_eq!(
        fs::read_to_string(day_file).expect("read day"),
        "## Pomodoros\n- [ ] Current\n  - [[dev#^history-id]]\n"
    );
}

#[test]
fn capture_clip_json_always_emits_collection_fields() {
    let temp = TempDir::new("bob-cli-capture-clip-json-collections");
    let vault = temp.path().join("vault");
    let clipboard = temp.path().join("clipboard");
    fs::create_dir_all(&vault).expect("create vault");
    write_executable(&clipboard, "#!/bin/sh\nprintf 'live value\n'\n");

    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("-f")
        .arg("json")
        .arg("single %")
        .env("BOB_CLIPBOARD_CMD", &clipboard)
        .env("BOB_NOW", "2026-07-15")
        .output()
        .expect("capture plain clip");
    assert_success(&output);
    let json: serde_json::Value =
        serde_json::from_str(stdout(&output).trim()).expect("clip JSON");
    let clip = json["clip"]
        .as_object()
        .expect("plain clip emits a JSON object");

    assert!(clip.contains_key("lines"), "{json}");
    assert!(clip.contains_key("attachments"), "{json}");
    assert!(clip.contains_key("entries"), "{json}");
    assert_eq!(clip["lines"], serde_json::json!(["\t- live value"]));
    assert_eq!(clip["attachments"], serde_json::json!([]));
    assert_eq!(clip["entries"], serde_json::json!([]));
}

#[test]
fn capture_history_provider_failures_leave_the_vault_untouched() {
    let cases = [
        (
            "command-failed",
            "#!/bin/sh\necho provider-failed >&2\nexit 7\n",
            "exited with 7: provider-failed",
        ),
        (
            "malformed",
            "#!/bin/sh\nprintf 'not json\n'\n",
            "JSON array of strings",
        ),
        (
            "insufficient",
            "#!/bin/sh\nprintf '%s\\n' '[\"live value\",\"older\"]'\n",
            "requested 3 entries but only 2",
        ),
        (
            "invalid-entry",
            "#!/bin/sh\nprintf '%s\\n' '[\"live value\",\"older\",\"\"]'\n",
            "history entry 3: clipboard is empty",
        ),
        (
            "later-plan-failure",
            "#!/bin/sh\nprintf '%s\\n' '[\"live value\",\"/definitely/missing/history-file\",\"older\"]'\n",
            "history entry 2: clipboard attachment does not exist",
        ),
    ];

    for (name, script, expected) in cases {
        let temp = TempDir::new(&format!("bob-cli-history-failure-{name}"));
        let vault = temp.path().join("vault");
        let inbox = vault.join("mac_inbox.md");
        let clipboard = temp.path().join("clipboard");
        let history = temp.path().join("history");
        write_file(&inbox, "sentinel\n");
        write_executable(&clipboard, "#!/bin/sh\nprintf 'live value\n'\n");
        write_executable(&history, script);

        let output = bob_command()
            .arg("capture")
            .arg("-b")
            .arg(&vault)
            .arg("task %3")
            .env("BOB_CLIPBOARD_CMD", &clipboard)
            .env("BOB_CLIPBOARD_HISTORY_CMD", &history)
            .env("BOB_NOW", "2026-07-15")
            .output()
            .expect("failing history capture");
        assert_eq!(output.status.code(), Some(1), "{}", format_output(&output));
        assert!(
            stderr(&output).contains(expected),
            "{}",
            format_output(&output)
        );
        assert_eq!(
            fs::read_to_string(&inbox).expect("read inbox"),
            "sentinel\n"
        );
        assert!(!vault.join("img").exists());
        assert!(!vault.join("file").exists());
    }
}

#[test]
fn capture_history_dry_run_plans_colliding_files_without_writes() {
    let temp = TempDir::new("bob-cli-capture-history-files-dry-run");
    let vault = temp.path().join("vault");
    let first_dir = temp.path().join("first");
    let second_dir = temp.path().join("second");
    let clipboard = temp.path().join("clipboard");
    let history = temp.path().join("history");
    fs::create_dir_all(&vault).expect("create vault");
    fs::create_dir_all(&first_dir).expect("first dir");
    fs::create_dir_all(&second_dir).expect("second dir");
    let first = first_dir.join("report.txt");
    let second = second_dir.join("report.txt");
    fs::write(&first, b"first").expect("first attachment");
    fs::write(&second, b"second").expect("second attachment");
    write_executable(
        &clipboard,
        &format!(
            "#!/bin/sh\nprintf '%s\\n' {}\n",
            shell_single_quote(first.to_str().expect("utf8 first"))
        ),
    );
    let history_json = serde_json::json!([
        first.display().to_string(),
        second.display().to_string(),
        "# Structured\n\nbody",
    ])
    .to_string();
    write_executable(
        &history,
        &format!(
            "#!/bin/sh\nprintf '%s\\n' {}\n",
            shell_single_quote(&history_json)
        ),
    );

    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("--dry-run")
        .arg("history files %3")
        .env("BOB_CLIPBOARD_CMD", &clipboard)
        .env("BOB_CLIPBOARD_HISTORY_CMD", &history)
        .env("BOB_NOW", "2026-07-15 13:14:15")
        .output()
        .expect("dry-run history files");
    assert_success(&output);
    let human = stdout(&output);
    let second_hash = hex::encode(sha2::Sha256::digest(b"second"));
    assert!(
        human.contains("would save")
            && human.contains("file/report.txt")
            && human
                .contains(&format!("file/report-{}.txt", &second_hash[..8]))
            && human.contains("file/clip-20260715-131415-structured.md"),
        "{}",
        format_output(&output)
    );
    assert!(!vault.join("mac_inbox.md").exists());
    assert!(!vault.join("img").exists());
    assert!(!vault.join("file").exists());
}

#[test]
fn capture_clip_options_force_or_disable_marker_parsing() {
    let temp = TempDir::new("bob-cli-capture-clip-options");
    let vault = temp.path().join("vault");
    let clipboard = temp.path().join("clipboard");
    fs::create_dir_all(&vault).expect("create vault");
    write_executable(&clipboard, "#!/bin/sh\nprintf 'forced text'\n");

    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("--clip=review_notes")
        .arg("--")
        .arg("keep %literal")
        .env("BOB_CLIPBOARD_CMD", &clipboard)
        .env("BOB_NOW", "2026-07-15")
        .output()
        .expect("run forced clipboard capture");
    assert_success(&output);
    assert_eq!(
        fs::read_to_string(vault.join("mac_inbox.md")).expect("read inbox"),
        concat!(
            "- [ ] #task keep %literal [created::2026-07-15]\n",
            "\t- **REVIEW NOTES:** forced text\n",
        )
    );

    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("-f")
        .arg("json")
        .arg("--clip")
        .arg("--")
        .arg("headerless %literal")
        .env("BOB_CLIPBOARD_CMD", &clipboard)
        .env("BOB_NOW", "2026-07-15")
        .output()
        .expect("run headerless forced clipboard capture");
    assert_success(&output);
    let json: serde_json::Value = serde_json::from_str(stdout(&output).trim())
        .unwrap_or_else(|error| {
            panic!("clipboard JSON: {error}\n{}", format_output(&output))
        });
    assert_eq!(json["text"], "headerless %literal");
    assert_eq!(json["clip"]["header"], serde_json::Value::Null);
    assert_eq!(
        json["clip"]["lines"],
        serde_json::json!(["\t- forced text"])
    );
    let inbox =
        fs::read_to_string(vault.join("mac_inbox.md")).expect("read inbox");
    assert!(inbox.contains(concat!(
        "- [ ] #task headerless %literal [created::2026-07-15]\n",
        "\t- forced text\n",
    )));
    assert!(!inbox.contains("**CLIP:**"));

    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("--clip=20")
        .arg("--")
        .arg("numeric header %3")
        .env("BOB_CLIPBOARD_CMD", &clipboard)
        .env("BOB_NOW", "2026-07-15")
        .output()
        .expect("run forced numeric header");
    assert_success(&output);
    assert!(
        fs::read_to_string(vault.join("mac_inbox.md"))
            .expect("read inbox")
            .contains(concat!(
                "#task numeric header %3 [created::2026-07-15]\n",
                "\t- **20:** forced text"
            )),
        "numeric forced header should remain available"
    );

    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("literal %0")
        .env_remove("BOB_CLIPBOARD_CMD")
        .env_remove("BOB_CLIPBOARD_HISTORY_CMD")
        .env("BOB_NOW", "2026-07-15")
        .output()
        .expect("run literal zero marker");
    assert_success(&output);
    assert!(
        fs::read_to_string(vault.join("mac_inbox.md"))
            .expect("read inbox")
            .contains("#task literal %0 [created::2026-07-15]"),
        "zero marker should stay literal"
    );

    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("--no-clip")
        .arg("--")
        .arg("literal %20")
        .env_remove("BOB_CLIPBOARD_CMD")
        .env_remove("DISPLAY")
        .env_remove("WAYLAND_DISPLAY")
        .env_remove("TMUX")
        .env("BOB_NOW", "2026-07-15")
        .output()
        .expect("run no-clip capture");
    assert_success(&output);
    assert!(
        fs::read_to_string(vault.join("mac_inbox.md"))
            .expect("read inbox")
            .contains("#task literal %20 [created::2026-07-15]"),
        "trailing marker should be literal"
    );

    let output = bob_command()
        .arg("capture")
        .arg("--clip")
        .arg("--no-clip")
        .arg("text")
        .output()
        .expect("run conflicting options");
    assert_eq!(output.status.code(), Some(2), "{}", format_output(&output));

    let output = bob_command()
        .arg("capture")
        .arg("--clip=bad!")
        .arg("text")
        .output()
        .expect("run invalid header");
    assert_eq!(output.status.code(), Some(2), "{}", format_output(&output));

    let output = bob_command()
        .arg("capture")
        .arg("--clip=")
        .arg("text")
        .output()
        .expect("run empty header");
    assert_eq!(output.status.code(), Some(2), "{}", format_output(&output));
}

#[cfg(not(target_os = "macos"))]
#[test]
fn capture_history_without_a_provider_has_actionable_guidance() {
    let temp = TempDir::new("bob-cli-capture-history-no-provider");
    let vault = temp.path().join("vault");
    let clipboard = temp.path().join("clipboard");
    fs::create_dir_all(&vault).expect("create vault");
    write_executable(&clipboard, "#!/bin/sh\nprintf 'live value\n'\n");

    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("task %2")
        .env("BOB_CLIPBOARD_CMD", &clipboard)
        .env_remove("BOB_CLIPBOARD_HISTORY_CMD")
        .env("BOB_NOW", "2026-07-15")
        .output()
        .expect("capture without history provider");
    assert_eq!(output.status.code(), Some(1), "{}", format_output(&output));
    assert!(
        stderr(&output).contains("BOB_CLIPBOARD_HISTORY_CMD")
            && stderr(&output).contains("JSON array"),
        "{}",
        format_output(&output)
    );
    assert!(!vault.join("mac_inbox.md").exists());
}

#[test]
fn capture_clip_saves_attachments_snippets_and_reports_dry_run() {
    let temp = TempDir::new("bob-cli-capture-clip-files");
    let vault = temp.path().join("vault");
    let source = temp.path().join("screen:shot.PNG");
    let clipboard = temp.path().join("clipboard");
    fs::create_dir_all(&vault).expect("create vault");
    fs::write(&source, b"fake image bytes").expect("write source");
    write_executable(
        &clipboard,
        &format!(
            "#!/bin/sh\nprintf '%s\\n' {}\n",
            shell_single_quote(source.to_str().expect("utf8 source"))
        ),
    );

    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("-f")
        .arg("json")
        .arg("attach image %")
        .env("BOB_CLIPBOARD_CMD", &clipboard)
        .env("BOB_NOW", "2026-07-15 13:14:15")
        .output()
        .expect("capture image attachment");
    assert_success(&output);
    let json: serde_json::Value =
        serde_json::from_str(stdout(&output).trim()).expect("attachment JSON");
    assert_eq!(json["clip"]["header"], serde_json::Value::Null);
    assert_eq!(json["clip"]["mode"], "attachments");
    assert_eq!(json["clip"]["attachments"][0]["kind"], "image");
    assert_eq!(
        json["clip"]["attachments"][0]["saved"],
        "img/screen-shot.PNG"
    );
    assert_eq!(json["clip"]["attachments"][0]["reused"], false);
    assert_eq!(
        fs::read(vault.join("img/screen-shot.PNG")).expect("saved image"),
        b"fake image bytes"
    );
    assert!(fs::read_to_string(vault.join("mac_inbox.md"))
        .expect("read inbox")
        .contains("\t- ![[img/screen-shot.PNG|400]]"));

    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("-f")
        .arg("json")
        .arg("reuse image %")
        .env("BOB_CLIPBOARD_CMD", &clipboard)
        .env("BOB_NOW", "2026-07-15 13:14:15")
        .output()
        .expect("reuse image attachment");
    assert_success(&output);
    let json: serde_json::Value =
        serde_json::from_str(stdout(&output).trim()).expect("reuse JSON");
    assert_eq!(json["clip"]["attachments"][0]["reused"], true);

    write_executable(
        &clipboard,
        "#!/bin/sh\nprintf '# Heading\\n\\n- preserved list\\n'\n",
    );
    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("-f")
        .arg("json")
        .arg("save snippet %")
        .env("BOB_CLIPBOARD_CMD", &clipboard)
        .env("BOB_NOW", "2026-07-15 13:14:15")
        .output()
        .expect("capture snippet");
    assert_success(&output);
    let json: serde_json::Value =
        serde_json::from_str(stdout(&output).trim()).expect("snippet JSON");
    assert_eq!(json["clip"]["header"], serde_json::Value::Null);
    assert_eq!(json["clip"]["mode"], "snippet");
    assert_eq!(
        json["clip"]["lines"],
        serde_json::json!(["\t- [[file/clip-20260715-131415-heading]]"])
    );
    assert_eq!(
        json["clip"]["snippet"],
        "file/clip-20260715-131415-heading.md"
    );
    assert_eq!(
        fs::read_to_string(vault.join("file/clip-20260715-131415-heading.md"))
            .expect("snippet file"),
        "# Heading\n\n- preserved list\n"
    );

    let dry_vault = temp.path().join("dry-vault");
    fs::create_dir_all(&dry_vault).expect("dry vault");
    write_executable(
        &clipboard,
        &format!(
            "#!/bin/sh\nprintf '%s\\n' {}\n",
            shell_single_quote(source.to_str().expect("utf8 source"))
        ),
    );
    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&dry_vault)
        .arg("--dry-run")
        .arg("dry attachment %")
        .env("BOB_CLIPBOARD_CMD", &clipboard)
        .env("BOB_NOW", "2026-07-15 13:14:15")
        .output()
        .expect("dry-run clipboard capture");
    assert_success(&output);
    assert!(
        stdout(&output).contains("would save"),
        "{}",
        format_output(&output)
    );
    assert!(!dry_vault.join("mac_inbox.md").exists());
    assert!(!dry_vault.join("img").exists());
    assert!(!dry_vault.join("file").exists());
}

#[test]
fn capture_clip_failures_leave_vault_untouched() {
    let cases = [
        ("empty", "#!/bin/sh\nexit 0\n", "clipboard is empty"),
        ("binary", "#!/bin/sh\nprintf 'a\\000b'\n", "binary data"),
        (
            "missing",
            "#!/bin/sh\nprintf '/definitely/missing/bob-clip-file'\n",
            "does not exist",
        ),
    ];

    for (name, script, expected) in cases {
        let temp = TempDir::new(&format!("bob-cli-capture-clip-fail-{name}"));
        let vault = temp.path().join("vault");
        let clipboard = temp.path().join("clipboard");
        let inbox = vault.join("mac_inbox.md");
        write_file(&inbox, "sentinel\n");
        write_executable(&clipboard, script);
        let output = bob_command()
            .arg("capture")
            .arg("-b")
            .arg(&vault)
            .arg("task %")
            .env("BOB_CLIPBOARD_CMD", &clipboard)
            .env("BOB_NOW", "2026-07-15")
            .output()
            .expect("run failing clipboard capture");
        assert_eq!(output.status.code(), Some(1), "{}", format_output(&output));
        assert!(
            stderr(&output).contains(expected),
            "{}",
            format_output(&output)
        );
        assert_eq!(
            fs::read_to_string(&inbox).expect("read inbox"),
            "sentinel\n"
        );
        assert!(!vault.join("img").exists());
        assert!(!vault.join("file").exists());
    }
}

#[test]
fn capture_bare_terminal_marker_clipboard_marker_writes_children_beneath_note()
{
    let temp = TempDir::new("bob-cli-capture-pomodoro-note-clip");
    let vault = temp.path().join("vault");
    let day_file = vault.join("day.md");
    let clipboard = temp.path().join("clipboard");
    write_executable(
        &clipboard,
        "#!/bin/sh\nprintf 'hello from clipboard\n'\n",
    );
    write_file(
        &day_file,
        "## Pomodoros\n\n- [ ] Current (0900-0930)\n  - existing child\n",
    );

    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .args(["remember this", "%", "#"])
        .env("BOB_CLIPBOARD_CMD", &clipboard)
        .env("BOB_DAY_FILE", &day_file)
        .env("BOB_NOW", "2026-07-10 13:40:00")
        .output()
        .expect("run Pomodoro-note capture with a clipboard marker");

    assert_success(&output);
    assert_eq!(
        fs::read_to_string(&day_file).expect("read day"),
        concat!(
            "## Pomodoros\n\n",
            "- [ ] Current (0900-0930)\n",
            "  - existing child\n",
            "  - remember this\n",
            "    - hello from clipboard\n",
        )
    );
}

#[test]
fn capture_sub_bullet_task_ref_recovers_shift_and_nests_clipboard() {
    let temp = TempDir::new("bob-cli-sub-bullet-ref-clip");
    let vault = temp.path().join("vault");
    let clipboard = temp.path().join("clipboard");
    let parent = "- [?] #task Parent without ID";
    let digest = hex::encode(Sha256::digest(parent.as_bytes()));
    write_file(
        &vault.join("cash.md"),
        &format!("shifted line\n## Tasks\n{parent}\nTail\n"),
    );
    write_executable(&clipboard, "#!/bin/sh\nprintf 'first\\nsecond\\n'\n");

    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("-f")
        .arg("json")
        .arg("--route")
        .arg("cash")
        .arg("--task-ref")
        .arg(format!("1:{}", &digest[..8]))
        .arg("--")
        .arg("new note %")
        .env("BOB_CLIPBOARD_CMD", &clipboard)
        .env("BOB_NOW", "2026-07-31")
        .output()
        .expect("task-ref clipboard sub-bullet");
    assert_success(&output);
    let json: serde_json::Value =
        serde_json::from_str(stdout(&output).trim()).expect("capture JSON");
    assert_eq!(json["parent_line"], 3);
    assert_eq!(json["parent_status_symbol"], "?");
    assert_eq!(json["parent_status_name"], "Unknown");
    assert!(json.get("block_id").is_none(), "{json}");
    assert_eq!(json["created"], "2026-07-31");
    assert_eq!(
        fs::read_to_string(vault.join("cash.md")).expect("read note"),
        concat!(
            "shifted line\n",
            "## Tasks\n",
            "- [?] #task Parent without ID\n",
            "\t- new note\n",
            "\t\t- first\n",
            "\t\t- second\n",
            "Tail\n",
        )
    );
}
