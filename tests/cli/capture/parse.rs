//! Capture parse except pomodoro protocol.

use crate::support::*;
use std::fs;

#[test]
fn capture_parse_human_output_is_plain_and_concise() {
    let output = bob_command()
        .arg("capture-parse")
        .arg("--")
        .arg("Call bank @Cash+")
        .output()
        .expect("run bob capture-parse human");

    assert_success(&output);
    assert!(
        stderr(&output).is_empty(),
        "unexpected capture-parse stderr:\n{}",
        format_output(&output)
    );
    let out = stdout(&output);
    assert_text_order(
        &out,
        &[
            "incomplete",
            "body",
            "Call bank",
            "route",
            "cash",
            "needs",
            "task",
            "sub_bullet_route",
            "interactive_placeholder",
        ],
    );
    assert_stdout_has_no_ansi(&output);
}

#[test]
fn capture_parse_json_output_is_stable_and_parseable() {
    let output = bob_command()
        .arg("capture-parse")
        .arg("-f")
        .arg("json")
        .arg("--")
        .arg("Call bank @Cash+")
        .output()
        .expect("run bob capture-parse json");

    assert_success(&output);
    assert!(
        stderr(&output).is_empty(),
        "unexpected capture-parse stderr:\n{}",
        format_output(&output)
    );
    let json: serde_json::Value = serde_json::from_str(stdout(&output).trim())
        .expect("capture-parse JSON");
    assert_eq!(json["ok"], true);
    assert_eq!(json["schema_version"], 1);
    assert_eq!(json["input"], "Call bank @Cash+");
    assert_eq!(json["body"], "Call bank");
    assert_eq!(json["mode"], "incomplete");
    assert_eq!(json["route"], "cash");
    assert!(json["section"].is_null());
    assert!(json["block_id"].is_null());
    assert_eq!(json["needs"], serde_json::json!(["task"]));
    assert_eq!(
        json["spans"],
        serde_json::json!([
            { "start": 10, "end": 15, "kind": "sub_bullet_route" },
            { "start": 15, "end": 16, "kind": "interactive_placeholder" },
        ])
    );
    assert_eq!(json["diagnostics"], serde_json::json!([]));
    assert!(json.get("targets").is_none(), "{json}");
    assert_stdout_has_no_ansi(&output);
}

#[test]
fn capture_parse_json_reports_public_target_runs_and_batch_offsets() {
    let parse = |raw: &str| {
        let output = bob_command()
            .arg("capture-parse")
            .arg("-f")
            .arg("json")
            .arg("--")
            .arg(raw)
            .output()
            .expect("run target-run capture-parse");
        assert_success(&output);
        serde_json::from_str::<serde_json::Value>(stdout(&output).trim())
            .expect("capture-parse JSON")
    };

    let leading_raw = "@work+a @dev+b note";
    let leading = parse(leading_raw);
    assert_eq!(leading["grammar"], 9);
    assert_eq!(leading["body"], "note");
    assert_eq!(leading["route"], "work");
    assert!(leading.get("items").is_none(), "{leading}");
    let targets = leading["targets"].as_array().expect("targets");
    assert_eq!(targets.len(), 2);
    assert_eq!(targets[0]["marker"], "@work+a");
    assert_eq!(targets[1]["marker"], "@dev+b");
    assert_eq!(targets[0]["mode"], "sub_bullet");
    assert_eq!(
        (targets[0]["start"].as_u64(), targets[0]["end"].as_u64()),
        (Some(0), Some(7))
    );
    assert_eq!(
        (targets[1]["start"].as_u64(), targets[1]["end"].as_u64()),
        (Some(8), Some(14))
    );

    let trailing = parse("note @work+a @dev+b");
    assert_eq!(trailing["body"], "note");
    assert_eq!(trailing["targets"].as_array().unwrap().len(), 2);

    let partial = parse("@work+a @dev+ note");
    assert_eq!(partial["body"], "note");
    assert_eq!(partial["needs"], serde_json::json!(["task"]));
    assert_eq!(partial["targets"][1]["mode"], "incomplete");
    assert_eq!(partial["targets"][1]["marker"], "@dev+");

    let batch = parse("first\n\n@work+a @dev+b note");
    let items = batch["items"].as_array().expect("batch items");
    assert_eq!(items.len(), 2);
    assert_eq!(items[1]["body"], "note");
    assert_eq!(items[1]["targets"].as_array().unwrap().len(), 2);
    assert_eq!(items[1]["targets"][0]["start"], 7);
}

#[test]
fn capture_parse_json_reports_duplicate_and_split_target_diagnostics() {
    for (raw, code, offending) in [
        (
            "@work+a @work+a note",
            "duplicate_capture_target",
            "@work+a",
        ),
        ("@a+x note @b+y", "split_capture_targets", "@b+y"),
    ] {
        let output = bob_command()
            .arg("capture-parse")
            .arg("-f")
            .arg("json")
            .arg("--")
            .arg(raw)
            .output()
            .expect("run target refusal parse");
        assert_success(&output);
        let json: serde_json::Value =
            serde_json::from_str(stdout(&output).trim())
                .expect("capture-parse JSON");
        let diagnostic = json["diagnostics"]
            .as_array()
            .unwrap()
            .iter()
            .find(|diagnostic| diagnostic["code"] == code)
            .unwrap_or_else(|| panic!("missing {code}: {json}"));
        assert_eq!(diagnostic["code"], code);
        let start = raw.rfind(offending).unwrap();
        assert_eq!(
            diagnostic["range"],
            serde_json::json!([start, start + offending.len()])
        );
    }
}

#[test]
fn capture_strict_json_preserves_target_refusal_codes() {
    let temp = TempDir::new("bob-cli-capture-target-refusal-codes");
    let vault = temp.path().join("vault");
    fs::create_dir_all(&vault).expect("create scratch vault");
    for (raw, code) in [
        ("@work+a @work+a note", "duplicate_capture_target"),
        ("@a+x note @b+y", "split_capture_targets"),
    ] {
        let output = bob_command()
            .arg("capture")
            .arg("-b")
            .arg(&vault)
            .arg("-f")
            .arg("json")
            .arg("--dry-run")
            .arg("--")
            .arg(raw)
            .env("BOB_NOW", "2026-11-03 09:30")
            .env("BOB_REF_JOBS_KICK", "off")
            .output()
            .expect("run strict target refusal");
        assert!(!output.status.success(), "{raw}: {}", stdout(&output));
        let json: serde_json::Value =
            serde_json::from_str(stdout(&output).trim()).expect("capture JSON");
        assert_eq!(json["code"], code, "{raw}: {json}");
    }
}

#[test]
fn capture_parse_and_dry_run_use_the_same_public_target_run_in_scratch_vault() {
    fn snapshot(root: &std::path::Path) -> Vec<(String, Vec<u8>)> {
        fn visit(
            root: &std::path::Path,
            current: &std::path::Path,
            files: &mut Vec<(String, Vec<u8>)>,
        ) {
            for entry in fs::read_dir(current).expect("read scratch tree") {
                let entry = entry.expect("scratch entry");
                let path = entry.path();
                if path.is_dir() {
                    visit(root, &path, files);
                } else {
                    files.push((
                        path.strip_prefix(root)
                            .unwrap()
                            .to_string_lossy()
                            .into_owned(),
                        fs::read(&path).expect("read scratch file"),
                    ));
                }
            }
        }
        let mut files = Vec::new();
        visit(root, root, &mut files);
        files.sort_by(|left, right| left.0.cmp(&right.0));
        files
    }

    let temp = TempDir::new("bob-cli-capture-target-run-scratch");
    let home = temp.path().join("home");
    let config = temp.path().join("xdg-config");
    let data = temp.path().join("xdg-data");
    let state = temp.path().join("xdg-state");
    let cache = temp.path().join("xdg-cache");
    let vault = temp.path().join("vault");
    for directory in [&home, &config, &data, &state, &cache, &vault] {
        fs::create_dir_all(directory)
            .expect("create isolated scratch directory");
    }
    write_file(
        &vault.join("work.md"),
        "# Work\n\n## Tasks\n- [ ] #task Existing work ^a\n",
    );
    write_file(
        &vault.join("dev.md"),
        "# Dev\n\n## Tasks\n- [ ] #task Existing dev ^b\n",
    );
    let before = snapshot(&vault);
    let draft = "@work+a @dev+b note";
    let scratch = |command: &str| {
        let mut process = bob_command();
        process
            .arg(command)
            .env("HOME", &home)
            .env("XDG_CONFIG_HOME", &config)
            .env("XDG_DATA_HOME", &data)
            .env("XDG_STATE_HOME", &state)
            .env("XDG_CACHE_HOME", &cache)
            .env("BOB_DIR", &vault)
            .env("BOB_NOW", "2026-11-03 09:30")
            .env("BOB_REF_JOBS_KICK", "off");
        process
    };

    let parsed = scratch("capture-parse")
        .arg("-f")
        .arg("json")
        .arg("--")
        .arg(draft)
        .output()
        .expect("run scratch capture-parse");
    assert_success(&parsed);
    let parsed_json: serde_json::Value =
        serde_json::from_str(stdout(&parsed).trim()).expect("parse JSON");
    assert_eq!(parsed_json["grammar"], 9);
    assert_eq!(parsed_json["body"], "note");
    assert_eq!(parsed_json["targets"].as_array().unwrap().len(), 2);

    let dry_run = scratch("capture")
        .arg("-b")
        .arg(&vault)
        .arg("-f")
        .arg("json")
        .arg("--dry-run")
        .arg("--")
        .arg(draft)
        .output()
        .expect("run scratch capture dry-run");
    assert_success(&dry_run);
    let capture_json: serde_json::Value =
        serde_json::from_str(stdout(&dry_run).trim()).expect("capture JSON");
    let captures = capture_json["captures"].as_array().expect("captures");
    assert_eq!(captures.len(), 2);
    assert_eq!(captures[0]["kind"], "sub_bullet");
    assert_eq!(captures[1]["kind"], "sub_bullet");
    assert_eq!(captures[0]["text"], "note");
    assert_eq!(captures[1]["text"], "note");
    assert_eq!(captures[0]["route"], "work");
    assert_eq!(captures[1]["route"], "dev");
    assert_eq!(
        snapshot(&vault),
        before,
        "dry-run must not change vault bytes"
    );
}

#[test]
fn capture_parse_json_reports_batch_items_with_global_ranges() {
    let draft = "First @cash\n- child\n\nSecond @notes#Ideas";
    let output = bob_command()
        .arg("capture-parse")
        .arg("-f")
        .arg("json")
        .arg(draft)
        .output()
        .expect("run batch capture-parse json");

    assert_success(&output);
    let json: serde_json::Value = serde_json::from_str(stdout(&output).trim())
        .expect("capture-parse JSON");
    let first_end = draft.find("\n\n").expect("separator");
    let second_start = first_end + 2;

    assert_eq!(json["schema_version"], 1);
    assert_eq!(json["body"], "First");
    assert_eq!(json["route"], "cash");
    assert_eq!(json["sub_bullets"], serde_json::json!(["child"]));
    assert_eq!(json["items"].as_array().expect("items").len(), 2);
    assert_eq!(json["items"][0]["index"], 1);
    assert_eq!(
        json["items"][0]["range"],
        serde_json::json!({ "start": 0, "end": first_end })
    );
    assert_eq!(json["items"][0]["line_start"], 1);
    assert_eq!(json["items"][0]["line_end"], 2);
    assert_eq!(json["items"][0]["body"], "First");
    assert_eq!(json["items"][0]["route"], "cash");
    assert_eq!(
        json["items"][0]["sub_bullets"],
        serde_json::json!(["child"])
    );
    assert_eq!(json["items"][1]["index"], 2);
    assert_eq!(
        json["items"][1]["range"],
        serde_json::json!({ "start": second_start, "end": draft.len() })
    );
    assert_eq!(json["items"][1]["line_start"], 4);
    assert_eq!(json["items"][1]["line_end"], 4);
    assert_eq!(json["items"][1]["body"], "Second");
    assert_eq!(json["items"][1]["mode"], "bullet");
    assert_eq!(json["items"][1]["route"], "notes");
    assert_eq!(json["items"][1]["section"], "Ideas");
    assert_eq!(
        json["spans"],
        serde_json::json!([
            { "start": 6, "end": 11, "kind": "route" },
            { "start": 28, "end": 34, "kind": "route" },
            { "start": 35, "end": 40, "kind": "section" },
        ])
    );
}

#[test]
fn capture_parse_json_reports_task_block_id_marker_spans_and_needs() {
    let output = bob_command()
        .arg("capture-parse")
        .arg("-f")
        .arg("json")
        .arg("--")
        .arg("Do work @Dev^")
        .output()
        .expect("run bob capture-parse task block ID marker");

    assert_success(&output);
    let json: serde_json::Value = serde_json::from_str(stdout(&output).trim())
        .expect("capture-parse JSON");
    assert_eq!(json["mode"], "incomplete");
    assert_eq!(json["route"], "dev");
    assert!(json["block_id"].is_null());
    assert_eq!(json["needs"], serde_json::json!(["block_id"]));
    assert_eq!(
        json["spans"],
        serde_json::json!([
            { "start": 8, "end": 12, "kind": "task_block_id_route" },
            { "start": 12, "end": 13, "kind": "interactive_placeholder" },
        ])
    );

    let invalid = bob_command()
        .arg("capture-parse")
        .arg("-f")
        .arg("json")
        .arg("--")
        .arg("Do work @Dev^bad.id")
        .output()
        .expect("run invalid task block ID parse");
    assert_success(&invalid);
    let json: serde_json::Value = serde_json::from_str(stdout(&invalid).trim())
        .expect("capture-parse JSON");
    assert_eq!(json["mode"], "task");
    assert_eq!(json["diagnostics"][0]["code"], "invalid_task_block_id");
}

#[test]
fn capture_parse_json_reports_project_note_markers() {
    let vault_temp = TempDir::new("bob-cli-capture-parse-themes");
    let vault = vault_temp.path().join("vault");
    std::fs::create_dir_all(&vault).expect("create scratch vault");
    let output = bob_command()
        .arg("capture-parse")
        .arg("-f")
        .arg("json")
        .arg("--")
        .arg("Do work @cash^goog-exit@")
        .env("BOB_DIR", &vault)
        .output()
        .expect("run bob capture-parse project-note caret marker");

    assert_success(&output);
    let json: serde_json::Value = serde_json::from_str(stdout(&output).trim())
        .expect("capture-parse JSON");
    assert_eq!(json["mode"], "project_note");
    assert_eq!(json["body"], "Do work");
    assert_eq!(json["route"], "cash");
    assert_eq!(json["block_id"], "goog-exit");
    assert!(json["section"].is_null());
    assert_eq!(json["needs"], serde_json::json!([]));
    assert_eq!(
        json["spans"],
        serde_json::json!([
            { "start": 8, "end": 13, "kind": "task_block_id_route", "project_note": "create" },
            { "start": 13, "end": 14, "kind": "project_note_separator", "project_note": "create" },
            { "start": 14, "end": 23, "kind": "task_block_id", "project_note": "create" },
            { "start": 23, "end": 24, "kind": "project_note_marker", "project_note": "create" },
        ])
    );
    assert!(json["diagnostics"].as_array().unwrap().is_empty());

    let output = bob_command()
        .arg("capture-parse")
        .arg("-f")
        .arg("json")
        .arg("--")
        .arg("Do work @cash^goog-exit@#bugs")
        .env("BOB_DIR", &vault)
        .output()
        .expect("run bob capture-parse project-note pomodoro marker");

    assert_success(&output);
    let json: serde_json::Value = serde_json::from_str(stdout(&output).trim())
        .expect("capture-parse JSON");
    assert_eq!(json["mode"], "project_note");
    assert_eq!(json["body"], "Do work");
    assert_eq!(json["route"], "cash");
    assert_eq!(json["block_id"], "goog-exit");
    assert_eq!(json["section"], "bugs");
    assert_eq!(json["needs"], serde_json::json!([]));
    assert_eq!(
        json["spans"],
        serde_json::json!([
            { "start": 8, "end": 13, "kind": "task_block_id_route", "project_note": "create" },
            { "start": 13, "end": 14, "kind": "project_note_separator", "project_note": "create" },
            { "start": 14, "end": 23, "kind": "task_block_id", "project_note": "create" },
            { "start": 23, "end": 24, "kind": "project_note_marker", "project_note": "create" },
            {
                "start": 25,
                "end": 29,
                "kind": "pomodoro_name",
                "themes": [
                    {
                        "key": "bugs",
                        "name": "bugs",
                        "hue": "aqua",
                        "hue_source": "hash",
                        "known": false,
                        "start": 25,
                        "end": 29,
                    },
                ],
            },
        ])
    );
    assert_eq!(
        json["diagnostics"],
        serde_json::json!([
            {
                "severity": "error",
                "code": "unused_project_note_pomodoro",
                "message": "`#bugs` picks the Pomodoro for ` :<id>` task links, but no task bullet ends with ` :<id>`; add one or remove `#bugs`",
                "range": [24, 29],
            }
        ])
    );

    let output = bob_command()
        .arg("capture-parse")
        .arg("-f")
        .arg("json")
        .arg("--")
        .arg("Do work @^focus-123@")
        .env("BOB_DIR", &vault)
        .output()
        .expect("run bob capture-parse routeless project-note marker");

    assert_success(&output);
    let json: serde_json::Value = serde_json::from_str(stdout(&output).trim())
        .expect("capture-parse JSON");
    assert_eq!(json["mode"], "project_note");
    assert!(json["route"].is_null());
    assert_eq!(json["block_id"], "focus-123");
    assert_eq!(json["needs"], serde_json::json!(["route"]));

    let invalid = bob_command()
        .arg("capture-parse")
        .arg("-f")
        .arg("json")
        .arg("--")
        .arg("Do work @cash:goog-exit+#bugs")
        .env("BOB_DIR", &vault)
        .output()
        .expect("run retired colon project-note marker");
    assert_success(&invalid);
    let json: serde_json::Value = serde_json::from_str(stdout(&invalid).trim())
        .expect("capture-parse JSON");
    assert_eq!(json["mode"], "task");
    assert_eq!(
        json["diagnostics"][0]["code"],
        "retired_project_note_marker"
    );
    assert!(
        json["diagnostics"][0]["message"]
            .as_str()
            .is_some_and(|message| {
                message.contains("is retired")
                    && message.contains("@cash^goog-exit@#bugs")
            }),
        "{json}"
    );

    for (input, expected_message) in [
        (
            "@@cash^goog-exit@",
            "invalid `@@` declaration: `@@` takes a route (`@@foo`)",
        ),
        (
            "@@cash+goog-exit\nTask",
            "invalid `@@` declaration: `@@` takes a route (`@@foo`)",
        ),
    ] {
        let global = bob_command()
            .arg("capture-parse")
            .arg("-f")
            .arg("json")
            .arg("--")
            .arg(input)
            .env("BOB_DIR", &vault)
            .output()
            .expect("run invalid global declaration parse");
        assert_success(&global);
        let json: serde_json::Value =
            serde_json::from_str(stdout(&global).trim())
                .expect("capture-parse JSON");
        assert_eq!(
            json["diagnostics"][0]["code"],
            "invalid_global_destination"
        );
        assert_eq!(json["diagnostics"][0]["message"], expected_message);
    }
}

#[test]
fn capture_parse_json_reports_project_note_offer_spans_and_mode() {
    // The offer seal reports `pomodoro_project_note` with the `offer`
    // modifier on every marker span and the `project_note_offer_marker`
    // seal kind; `#name` keeps no modifier.
    let vault_temp = TempDir::new("bob-cli-capture-parse-offer");
    let vault = vault_temp.path().join("vault");
    std::fs::create_dir_all(&vault).expect("create scratch vault");
    let output = bob_command()
        .arg("capture-parse")
        .arg("-f")
        .arg("json")
        .arg("--")
        .arg("Finish it @cash:goog-exit@#bugs")
        .env("BOB_DIR", &vault)
        .output()
        .expect("run bob capture-parse project-note offer marker");

    assert_success(&output);
    let json: serde_json::Value = serde_json::from_str(stdout(&output).trim())
        .expect("capture-parse JSON");
    assert_eq!(json["mode"], "pomodoro_project_note");
    assert_eq!(json["body"], "Finish it");
    assert_eq!(json["route"], "cash");
    assert_eq!(json["block_id"], "goog-exit");
    assert_eq!(json["section"], "bugs");
    assert_eq!(json["needs"], serde_json::json!([]));
    assert_eq!(
        json["spans"],
        serde_json::json!([
            { "start": 10, "end": 15, "kind": "pomodoro_route", "project_note": "offer" },
            { "start": 15, "end": 16, "kind": "project_note_separator", "project_note": "offer" },
            { "start": 16, "end": 25, "kind": "pomodoro_block_id", "project_note": "offer" },
            { "start": 25, "end": 26, "kind": "project_note_offer_marker", "project_note": "offer" },
            {
                "start": 27,
                "end": 31,
                "kind": "pomodoro_name",
                "themes": [
                    {
                        "key": "bugs",
                        "name": "bugs",
                        "hue": "aqua",
                        "hue_source": "hash",
                        "known": false,
                        "start": 27,
                        "end": 31,
                    },
                ],
            },
        ])
    );
    assert!(json["diagnostics"].as_array().unwrap().is_empty());

    // A completed seal offers no picker and carries no needs; a trailing
    // `#` opens Pomodoro-name completion.
    let output = bob_command()
        .arg("capture-parse")
        .arg("-f")
        .arg("json")
        .arg("--")
        .arg("Finish it @cash:goog-exit@")
        .env("BOB_DIR", &vault)
        .output()
        .expect("run bob capture-parse sealed offer marker");
    assert_success(&output);
    let json: serde_json::Value = serde_json::from_str(stdout(&output).trim())
        .expect("capture-parse JSON");
    assert_eq!(json["mode"], "pomodoro_project_note");
    assert_eq!(json["needs"], serde_json::json!([]));

    let output = bob_command()
        .arg("capture-parse")
        .arg("-f")
        .arg("json")
        .arg("--")
        .arg("Finish it @cash:goog-exit@#")
        .env("BOB_DIR", &vault)
        .output()
        .expect("run bob capture-parse offer name placeholder");
    assert_success(&output);
    let json: serde_json::Value = serde_json::from_str(stdout(&output).trim())
        .expect("capture-parse JSON");
    assert_eq!(json["needs"], serde_json::json!(["pomodoro_name"]));

    // Route-less seals report the route need in the offer mode.
    let output = bob_command()
        .arg("capture-parse")
        .arg("-f")
        .arg("json")
        .arg("--")
        .arg("Do work @:goog-exit@")
        .env("BOB_DIR", &vault)
        .output()
        .expect("run bob capture-parse routeless offer marker");
    assert_success(&output);
    let json: serde_json::Value = serde_json::from_str(stdout(&output).trim())
        .expect("capture-parse JSON");
    assert_eq!(json["mode"], "pomodoro_project_note");
    assert!(json["route"].is_null());
    assert_eq!(json["block_id"], "goog-exit");
    assert_eq!(json["needs"], serde_json::json!(["route"]));

    // Junk after the seal and misordered seals refuse with the offer
    // teaching.
    for (input, code, needle) in [
        (
            "Finish it @cash:goog-exit@x",
            "invalid_project_note_marker",
            "the project-note `@` ends the marker",
        ),
        (
            "Finish it @cash:goog-exit#bugs@",
            "invalid_project_note_marker",
            "right after the block ID",
        ),
        (
            "Finish it @cash:goog-exit@=3",
            "invalid_project_note_marker",
            "start suffix",
        ),
    ] {
        let output = bob_command()
            .arg("capture-parse")
            .arg("-f")
            .arg("json")
            .arg("--")
            .arg(input)
            .env("BOB_DIR", &vault)
            .output()
            .expect("run bob capture-parse invalid offer marker");
        assert_success(&output);
        let json: serde_json::Value =
            serde_json::from_str(stdout(&output).trim())
                .expect("capture-parse JSON");
        assert_eq!(json["diagnostics"][0]["code"], code, "{input}");
        assert!(
            json["diagnostics"][0]["message"]
                .as_str()
                .is_some_and(|message| message.contains(needle)),
            "{input}: {json}"
        );
    }
}

#[test]
fn capture_parse_json_keeps_offer_mode_while_a_sealed_offer_is_incomplete() {
    // Every recognized sealed offer keeps `pomodoro_project_note` and the
    // `offer` modifier on its marker, route/ID, and placeholder spans; the
    // needs name the missing route, ID, or name, and a `#name` keeps no
    // modifier. Spans are byte offsets, so the multibyte prefix shifts them.
    let vault_temp = TempDir::new("bob-cli-capture-parse-offer-incomplete");
    let vault = vault_temp.path().join("vault");
    std::fs::create_dir_all(&vault).expect("create scratch vault");

    // (input, needs, spans as (start, end, kind, project_note))
    let cases: &[(&str, &[&str], &[(u64, u64, &str, &str)])] = &[
        (
            "café @:pkt@",
            &["route"],
            &[
                (6, 8, "interactive_placeholder", "offer"),
                (8, 11, "pomodoro_block_id", "offer"),
                (11, 12, "project_note_offer_marker", "offer"),
            ],
        ),
        (
            "café @:pkt@#admin",
            &["route"],
            &[
                (6, 8, "interactive_placeholder", "offer"),
                (8, 11, "pomodoro_block_id", "offer"),
                (11, 12, "project_note_offer_marker", "offer"),
                (13, 18, "pomodoro_name", ""),
            ],
        ),
        (
            "café @:pkt@#\"Lit Name\"",
            &["route"],
            &[
                (6, 8, "interactive_placeholder", "offer"),
                (8, 11, "pomodoro_block_id", "offer"),
                (11, 12, "project_note_offer_marker", "offer"),
                (13, 23, "pomodoro_name", ""),
            ],
        ),
        (
            "@:pkt@#admin Finish café",
            &["route"],
            &[
                (0, 2, "interactive_placeholder", "offer"),
                (2, 5, "pomodoro_block_id", "offer"),
                (5, 6, "project_note_offer_marker", "offer"),
                (7, 12, "pomodoro_name", ""),
            ],
        ),
        (
            "café @:pkt@#",
            &["route", "pomodoro_name"],
            &[
                (6, 8, "interactive_placeholder", "offer"),
                (8, 11, "pomodoro_block_id", "offer"),
                (11, 12, "project_note_offer_marker", "offer"),
                (12, 13, "interactive_placeholder", ""),
            ],
        ),
        (
            "café @cash:@",
            &["pomodoro_id"],
            &[
                (6, 11, "pomodoro_route", "offer"),
                (11, 12, "interactive_placeholder", "offer"),
                (12, 13, "project_note_offer_marker", "offer"),
            ],
        ),
        (
            "café @cash:@#bugs",
            &["pomodoro_id"],
            &[
                (6, 11, "pomodoro_route", "offer"),
                (11, 12, "interactive_placeholder", "offer"),
                (12, 13, "project_note_offer_marker", "offer"),
                (14, 18, "pomodoro_name", ""),
            ],
        ),
        (
            "café @cash:pkt@#",
            &["pomodoro_name"],
            &[
                (6, 11, "pomodoro_route", "offer"),
                (11, 12, "project_note_separator", "offer"),
                (12, 15, "pomodoro_block_id", "offer"),
                (15, 16, "project_note_offer_marker", "offer"),
                (16, 17, "interactive_placeholder", ""),
            ],
        ),
        (
            "café @cash:pkt@#\"Lit Na",
            &["pomodoro_name"],
            &[
                (6, 11, "pomodoro_route", "offer"),
                (11, 12, "project_note_separator", "offer"),
                (12, 15, "pomodoro_block_id", "offer"),
                (15, 16, "project_note_offer_marker", "offer"),
                (17, 24, "pomodoro_name", ""),
            ],
        ),
        (
            "café @:@#",
            &["route", "pomodoro_id", "pomodoro_name"],
            &[
                (6, 8, "interactive_placeholder", "offer"),
                (8, 9, "project_note_offer_marker", "offer"),
                (9, 10, "interactive_placeholder", ""),
            ],
        ),
    ];

    for (input, needs, spans) in cases {
        let output = bob_command()
            .arg("capture-parse")
            .arg("-f")
            .arg("json")
            .arg("--")
            .arg(input)
            .env("BOB_DIR", &vault)
            .output()
            .expect("run bob capture-parse incomplete offer marker");
        assert_success(&output);
        let json: serde_json::Value =
            serde_json::from_str(stdout(&output).trim())
                .expect("capture-parse JSON");
        assert_eq!(json["mode"], "pomodoro_project_note", "{input}");
        assert_eq!(json["needs"], serde_json::json!(needs), "{input}");
        assert!(
            json["diagnostics"].as_array().unwrap().is_empty(),
            "{input}: {json}"
        );
        let observed: Vec<(u64, u64, &str, &str)> = json["spans"]
            .as_array()
            .expect("spans")
            .iter()
            .map(|span| {
                (
                    span["start"].as_u64().expect("start"),
                    span["end"].as_u64().expect("end"),
                    span["kind"].as_str().expect("kind"),
                    span["project_note"].as_str().unwrap_or(""),
                )
            })
            .collect();
        assert_eq!(&observed, spans, "{input}");
        for pair in observed.windows(2) {
            assert!(pair[0].1 <= pair[1].0, "{input}: overlapping spans");
        }
    }
}

#[test]
fn capture_parse_json_reports_project_task_ids_spans_and_mode() {
    let input = "Finish the Google exit packet! @cash^goog-exit@#admin\n- Draft the resignation memo :draft-memo\n  - keep it short\n- Collect the equity paperwork ^equity-docs";
    let output = bob_command()
        .arg("capture-parse")
        .arg("-f")
        .arg("json")
        .arg("--")
        .arg(input)
        .output()
        .expect("run bob capture-parse project task IDs");

    assert_success(&output);
    let json: serde_json::Value = serde_json::from_str(stdout(&output).trim())
        .expect("capture-parse JSON");
    assert_eq!(json["schema_version"], 1);
    assert_eq!(json["mode"], "pomodoro_project_note");
    assert_eq!(json["body"], "Finish the Google exit packet!");
    assert_eq!(json["route"], "cash");
    assert_eq!(json["section"], "admin");
    assert_eq!(
        json["sub_bullets"],
        serde_json::json!([
            "Draft the resignation memo",
            "keep it short",
            "Collect the equity paperwork",
        ])
    );
    assert_eq!(json["sub_bullet_depths"], serde_json::json!([1, 2, 1]));
    assert_eq!(
        json["sub_bullet_task_ids"],
        serde_json::json!([
            { "block_id": "draft-memo", "link": true },
            null,
            { "block_id": "equity-docs", "link": false },
        ])
    );
    assert!(json["diagnostics"].as_array().unwrap().is_empty(), "{json}");
    let kinds: Vec<&str> = json["spans"]
        .as_array()
        .unwrap()
        .iter()
        .map(|span| span["kind"].as_str().unwrap())
        .collect();
    assert!(kinds.contains(&"project_task_link_marker"), "{json}");
    assert!(kinds.contains(&"project_task_block_id"), "{json}");
    let memo = input.find(":draft-memo").unwrap();
    assert!(
        json["spans"].as_array().unwrap().iter().any(|span| {
            span["start"] == memo
                && span["end"] == memo + 1
                && span["kind"] == "project_task_link_marker"
        }),
        "{json}"
    );
    assert!(
        json["spans"].as_array().unwrap().iter().any(|span| {
            span["start"] == memo + 1
                && span["end"] == memo + ":draft-memo".len()
                && span["kind"] == "project_task_block_id"
        }),
        "{json}"
    );

    // Inputs without task IDs keep their exact older shape: no
    // `sub_bullet_task_ids` key at all.
    let plain = bob_command()
        .arg("capture-parse")
        .arg("-f")
        .arg("json")
        .arg("--")
        .arg("Do work @cash^goog-exit@")
        .output()
        .expect("run plain project-note parse");
    assert_success(&plain);
    let json: serde_json::Value = serde_json::from_str(stdout(&plain).trim())
        .expect("capture-parse JSON");
    assert_eq!(json["mode"], "project_note");
    assert!(json.get("sub_bullet_task_ids").is_none(), "{json}");

    // Human output repeats each ID after its sub-bullet.
    let human = bob_command()
        .arg("capture-parse")
        .arg("--")
        .arg(input)
        .output()
        .expect("run human project task-ID parse");
    assert_success(&human);
    let out = stdout(&human);
    assert!(
        out.contains("- Draft the resignation memo :draft-memo"),
        "{out}"
    );
    assert!(
        out.contains("- Collect the equity paperwork ^equity-docs"),
        "{out}"
    );
    assert!(out.contains("- keep it short"), "{out}");

    // A lone sigil is an unfinished ID: incomplete needing `block_id`.
    let unfinished = bob_command()
        .arg("capture-parse")
        .arg("-f")
        .arg("json")
        .arg("--")
        .arg("Finish it @cash^x@\n- Draft :")
        .output()
        .expect("run unfinished task-ID parse");
    assert_success(&unfinished);
    let json: serde_json::Value =
        serde_json::from_str(stdout(&unfinished).trim())
            .expect("capture-parse JSON");
    assert_eq!(json["mode"], "incomplete");
    assert_eq!(json["needs"], serde_json::json!(["block_id"]));
    assert!(json["diagnostics"].as_array().unwrap().is_empty(), "{json}");
}

#[test]
fn capture_parse_json_reports_retired_double_colon_as_migration_guidance() {
    let output = bob_command()
        .arg("capture-parse")
        .arg("-f")
        .arg("json")
        .arg("--")
        .arg("Follow up @file::new-id")
        .output()
        .expect("run bob capture-parse retired double colon");

    assert_success(&output);
    let json: serde_json::Value = serde_json::from_str(stdout(&output).trim())
        .expect("capture-parse JSON");
    assert_eq!(json["mode"], "task");
    assert_eq!(
        json["diagnostics"][0]["code"],
        "retired_task_block_id_marker"
    );
    assert!(
        json["diagnostics"][0]["message"]
            .as_str()
            .is_some_and(|message| {
                message.contains("'@<route>::<block-id>' is no longer accepted")
                    && message.contains("@<route>^<block-id>")
            }),
        "{json}"
    );
}

#[test]
fn capture_parse_json_reports_pomodoro_note_mode_and_span() {
    let output = bob_command()
        .arg("capture-parse")
        .arg("-f")
        .arg("json")
        .arg("--")
        .arg("note %n")
        .output()
        .expect("run bob capture-parse on a %n directive");

    assert_success(&output);
    let json: serde_json::Value = serde_json::from_str(stdout(&output).trim())
        .expect("capture-parse JSON");
    assert_eq!(json["mode"], "pomodoro_note");
    assert_eq!(json["body"], "note");
    assert!(json["route"].is_null());
    assert_eq!(json["needs"], serde_json::json!([]));
    assert_eq!(
        json["spans"],
        serde_json::json!([{ "start": 5, "end": 7, "kind": "directive", "directive": "note" }])
    );
    assert!(json["diagnostics"].as_array().unwrap().is_empty());

    let conflict = bob_command()
        .arg("capture-parse")
        .arg("-f")
        .arg("json")
        .arg("--")
        .arg("note %n @work")
        .output()
        .expect("run bob capture-parse on a conflicting %n and @route");

    assert_success(&conflict);
    let json: serde_json::Value =
        serde_json::from_str(stdout(&conflict).trim())
            .expect("capture-parse JSON");
    assert_eq!(json["diagnostics"][0]["code"], "pomodoro_note_conflict");
}

#[test]
fn capture_parse_json_reports_wikilink_semantic_spans() {
    let output = bob_command()
        .arg("capture-parse")
        .arg("-f")
        .arg("json")
        .arg("--")
        .arg("See [[sase#Design|Spec]]")
        .output()
        .expect("run bob capture-parse wikilink json");

    assert_success(&output);
    let json: serde_json::Value = serde_json::from_str(stdout(&output).trim())
        .expect("capture-parse JSON");
    assert_eq!(json["ok"], true);
    assert_eq!(json["body"], "See [[sase#Design|Spec]]");
    assert_eq!(
        json["spans"]
            .as_array()
            .expect("spans array")
            .iter()
            .map(|span| span["kind"].as_str().expect("kind"))
            .collect::<Vec<_>>(),
        vec![
            "wikilink_delimiter",
            "wikilink_target",
            "wikilink_delimiter",
            "wikilink_heading",
            "wikilink_delimiter",
            "wikilink_alias",
            "wikilink_delimiter",
        ]
    );
}

#[test]
fn capture_parse_json_reports_task_wikilink_spans_without_stealing_capture_spans(
) {
    let parse = |draft: &str| {
        let output = bob_command()
            .arg("capture-parse")
            .arg("-f")
            .arg("json")
            .arg("--")
            .arg(draft)
            .output()
            .expect("run bob capture-parse");
        assert_success(&output);
        serde_json::from_str::<serde_json::Value>(stdout(&output).trim())
            .expect("capture-parse JSON")
    };

    let empty_query = parse("foo bar baz ((");
    assert_eq!(
        empty_query["spans"],
        serde_json::json!([
            { "start": 12, "end": 14, "kind": "wikilink_task_delimiter" }
        ])
    );

    let closed = parse("Follow up ((foo))");
    assert_eq!(
        closed["spans"],
        serde_json::json!([
            { "start": 10, "end": 12, "kind": "wikilink_task_delimiter" },
            { "start": 12, "end": 15, "kind": "wikilink_task_query" },
            { "start": 15, "end": 17, "kind": "wikilink_task_delimiter" },
        ])
    );

    let route = parse("((foo @x");
    assert!(route["spans"].as_array().unwrap().iter().any(|span| {
        span["kind"] == "route" && span["start"] == 6 && span["end"] == 8
    }));
    assert!(route["spans"].as_array().unwrap().iter().any(|span| {
        span["kind"] == "wikilink_task_query"
            && span["start"] == 2
            && span["end"] == 6
    }));
    let spans = route["spans"].as_array().unwrap();
    assert!(spans.windows(2).all(|pair| pair[0]["end"].as_u64().unwrap()
        <= pair[1]["start"].as_u64().unwrap()));

    for draft in [
        "f((x",
        "(((x",
        "See ` ((x`",
        "See [[note ((x]]",
        "x ((foo",
        "=3 ((foo",
        "x\n- Follow up ((s",
        "Task {0} %t\n\n((x\n",
    ] {
        let parsed = parse(draft);
        assert!(
            parsed["spans"].as_array().unwrap().iter().all(|span| {
                !span["kind"]
                    .as_str()
                    .unwrap_or_default()
                    .starts_with("wikilink_task_")
            }),
            "unexpected task wikilink span in {draft:?}: {}",
            parsed["spans"]
        );
    }
}

#[test]
fn capture_parse_reports_diagnostics_without_failing() {
    let output = bob_command()
        .arg("capture-parse")
        .arg("-f")
        .arg("json")
        .arg("--")
        .arg("note @dev+bad.id")
        .output()
        .expect("run bob capture-parse diagnostic");

    assert_success(&output);
    let json: serde_json::Value = serde_json::from_str(stdout(&output).trim())
        .expect("capture-parse JSON");
    assert_eq!(json["ok"], true);
    assert_eq!(json["mode"], "task");
    assert_eq!(
        json["diagnostics"][0]["code"],
        "invalid_sub_bullet_block_id"
    );
    assert_eq!(json["diagnostics"][0]["severity"], "error");
    assert_eq!(json["diagnostics"][0]["range"], serde_json::json!([5, 16]));
}

#[test]
fn capture_parse_reads_one_stdin_line_when_text_is_omitted() {
    let output = run_with_stdin(
        bob_command().arg("capture-parse").arg("-f").arg("json"),
        "jot idea @notes#Ideas\n",
    );

    assert_success(&output);
    let json: serde_json::Value = serde_json::from_str(stdout(&output).trim())
        .expect("capture-parse JSON");
    assert_eq!(json["mode"], "bullet");
    assert_eq!(json["route"], "notes");
    assert_eq!(json["section"], "Ideas");
    assert_eq!(json["body"], "jot idea");
}

#[test]
fn capture_parse_missing_text_is_a_usage_error() {
    let human = run_with_stdin(bob_command().arg("capture-parse"), "   \n");
    assert_eq!(human.status.code(), Some(2));
    assert!(
        stdout(&human).is_empty()
            && stderr(&human).contains("task text is required"),
        "unexpected capture-parse missing-text output:\n{}",
        format_output(&human)
    );

    let json_output = run_with_stdin(
        bob_command().arg("capture-parse").arg("-f").arg("json"),
        "   \n",
    );
    assert_eq!(json_output.status.code(), Some(2));
    assert!(
        stderr(&json_output).is_empty(),
        "JSON failures keep stderr clean:\n{}",
        format_output(&json_output)
    );
    let json: serde_json::Value =
        serde_json::from_str(stdout(&json_output).trim())
            .expect("capture-parse failure JSON");
    assert_eq!(json["ok"], false);
    assert!(
        json["error"]
            .as_str()
            .expect("error string")
            .contains("task text is required"),
        "unexpected capture-parse failure JSON: {json}"
    );
}

#[test]
fn capture_parse_reports_utf8_byte_offsets() {
    let text = "caf\u{e9} run \u{1f680} @Cash+goog-exit";
    let output = bob_command()
        .arg("capture-parse")
        .arg("-f")
        .arg("json")
        .arg("--")
        .arg(text)
        .output()
        .expect("run bob capture-parse utf8");

    assert_success(&output);
    let json: serde_json::Value = serde_json::from_str(stdout(&output).trim())
        .expect("capture-parse JSON");
    assert_eq!(json["mode"], "sub_bullet");
    assert_eq!(json["body"], "caf\u{e9} run \u{1f680}");
    assert_eq!(
        json["spans"],
        serde_json::json!([
            { "start": 15, "end": 20, "kind": "sub_bullet_route" },
            { "start": 21, "end": 30, "kind": "sub_bullet_block_id" },
        ])
    );
    assert_eq!(&text[15..20], "@Cash");
    assert_eq!(&text[21..30], "goog-exit");
}

#[test]
fn capture_parse_json_reports_drawer_and_incomplete_needs() {
    let complete = bob_command()
        .arg("capture-parse")
        .arg("-f")
        .arg("json")
        .arg("--")
        .arg("Postgres 17 minimum @foo+bar#req")
        .output()
        .expect("run complete three-component parse");
    assert_success(&complete);
    let json: serde_json::Value =
        serde_json::from_str(stdout(&complete).trim()).expect("json");
    assert_eq!(json["mode"], "sub_bullet");
    assert_eq!(json["body"], "Postgres 17 minimum");
    assert_eq!(json["route"], "foo");
    assert_eq!(json["block_id"], "bar");
    assert_eq!(json["section"], "req");
    assert_eq!(json["needs"], serde_json::json!([]));
    assert_eq!(
        json["spans"],
        serde_json::json!([
            { "start": 20, "end": 24, "kind": "sub_bullet_route" },
            { "start": 25, "end": 28, "kind": "sub_bullet_block_id" },
            { "start": 29, "end": 32, "kind": "drawer", "title": "REQ", "key": "REQ" },
        ])
    );

    let incomplete = bob_command()
        .arg("capture-parse")
        .arg("-f")
        .arg("json")
        .arg("--")
        .arg("Postgres 17 minimum @foo+bar#")
        .output()
        .expect("run incomplete section parse");
    assert_success(&incomplete);
    let json: serde_json::Value =
        serde_json::from_str(stdout(&incomplete).trim()).expect("json");
    assert_eq!(json["mode"], "incomplete");
    assert_eq!(json["route"], "foo");
    assert_eq!(json["block_id"], "bar");
    assert!(json["section"].is_null());
    assert_eq!(json["needs"], serde_json::json!(["drawer"]));

    let needs_task = bob_command()
        .arg("capture-parse")
        .arg("-f")
        .arg("json")
        .arg("--")
        .arg("note @foo+#req")
        .output()
        .expect("run empty block-id parse");
    assert_success(&needs_task);
    let json: serde_json::Value =
        serde_json::from_str(stdout(&needs_task).trim()).expect("json");
    assert_eq!(json["mode"], "incomplete");
    assert_eq!(json["needs"], serde_json::json!(["task"]));
    assert_eq!(json["section"], "req");

    let both = bob_command()
        .arg("capture-parse")
        .arg("-f")
        .arg("json")
        .arg("--")
        .arg("note @foo+#")
        .output()
        .expect("run empty block-id and section parse");
    assert_success(&both);
    let json: serde_json::Value =
        serde_json::from_str(stdout(&both).trim()).expect("json");
    assert_eq!(json["mode"], "incomplete");
    assert_eq!(json["needs"], serde_json::json!(["task", "drawer"]));

    let invalid = bob_command()
        .arg("capture-parse")
        .arg("-f")
        .arg("json")
        .arg("--")
        .arg("note @foo+bar#bad_id")
        .output()
        .expect("run invalid drawer parse");
    assert_success(&invalid);
    let json: serde_json::Value =
        serde_json::from_str(stdout(&invalid).trim()).expect("json");
    assert_eq!(json["diagnostics"][0]["code"], "invalid_drawer");

    let note_bullet = bob_command()
        .arg("capture-parse")
        .arg("-f")
        .arg("json")
        .arg("--")
        .arg("Some note @foo#Ideas")
        .output()
        .expect("run note-bullet parse");
    assert_success(&note_bullet);
    let json: serde_json::Value =
        serde_json::from_str(stdout(&note_bullet).trim()).expect("json");
    assert_eq!(json["mode"], "bullet");
    assert_eq!(json["section"], "Ideas");
    assert!(json["block_id"].is_null());

    let pomodoro_note = bob_command()
        .arg("capture-parse")
        .arg("-f")
        .arg("json")
        .arg("--")
        .arg("remembered to bump the timeout %n")
        .output()
        .expect("run pomodoro-note parse");
    assert_success(&pomodoro_note);
    let json: serde_json::Value =
        serde_json::from_str(stdout(&pomodoro_note).trim()).expect("json");
    assert_eq!(json["mode"], "pomodoro_note");

    let incomplete = bob_command()
        .arg("capture-parse")
        .arg("-f")
        .arg("json")
        .arg("--")
        .arg("jot idea #")
        .output()
        .expect("run bare hash parse");
    assert_success(&incomplete);
    let json: serde_json::Value =
        serde_json::from_str(stdout(&incomplete).trim()).expect("json");
    assert_eq!(json["mode"], "incomplete");
    assert_eq!(json["needs"], serde_json::json!(["task_type"]));
    assert_eq!(json["body"], "jot idea");
    let span = json["spans"]
        .as_array()
        .unwrap()
        .iter()
        .find(|span| span["kind"] == "task_type")
        .expect("task_type span");
    assert_eq!(span["start"], "jot idea ".len());
    assert_eq!(span["end"], "jot idea #".len());
}

#[test]
fn capture_parse_never_touches_the_vault_or_clipboard() {
    let temp = TempDir::new("bob-cli-capture-parse-no-io");
    let missing = temp.path().join("missing-vault");
    let output = bob_command()
        .arg("capture-parse")
        .arg("-f")
        .arg("json")
        .arg("--")
        .arg("save this %c @dev:focus-1")
        .env("BOB_DIR", &missing)
        .env("BOB_CLIPBOARD_CMD", "/nonexistent/clipboard-command")
        .output()
        .expect("run bob capture-parse without a vault");

    assert_success(&output);
    assert!(
        stderr(&output).is_empty(),
        "unexpected capture-parse stderr:\n{}",
        format_output(&output)
    );
    let json: serde_json::Value = serde_json::from_str(stdout(&output).trim())
        .expect("capture-parse JSON");
    assert_eq!(json["ok"], true);
    assert_eq!(json["mode"], "pomodoro_task");
    assert_eq!(json["route"], "dev");
    assert_eq!(json["block_id"], "focus-1");
    assert_eq!(json["body"], "save this");
    assert!(
        !missing.exists(),
        "capture-parse must not create the vault directory"
    );

    // Contrast: the same input makes `bob capture` reach for the clipboard.
    let captured = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&missing)
        .arg("--")
        .arg("save this %c @dev:focus-1")
        .env("BOB_CLIPBOARD_CMD", "/nonexistent/clipboard-command")
        .output()
        .expect("run bob capture without a vault");
    assert!(
        !captured.status.success(),
        "expected bob capture to fail without clipboard access:\n{}",
        format_output(&captured)
    );
}

#[test]
fn capture_parse_json_reports_explicit_toggle_and_attached_type_spans() {
    let vault_temp = TempDir::new("bob-cli-capture-parse-themes");
    let vault = vault_temp.path().join("vault");
    std::fs::create_dir_all(&vault).expect("create scratch vault");
    let output = bob_command()
        .arg("capture-parse")
        .arg("-f")
        .arg("json")
        .arg("--")
        .arg("@cash+goog-exit!")
        .env("BOB_DIR", &vault)
        .output()
        .expect("run explicit-toggle parse");
    assert_success(&output);
    let json: serde_json::Value =
        serde_json::from_str(stdout(&output).trim()).expect("parse JSON");
    assert_eq!(json["ok"], true);
    assert_eq!(json["mode"], "task_toggle");
    assert_eq!(json["body"], "");
    assert_eq!(json["route"], "cash");
    assert_eq!(json["block_id"], "goog-exit");
    assert!(json["section"].is_null());
    assert_eq!(json["needs"], serde_json::json!([]));
    assert_eq!(
        json["spans"],
        serde_json::json!([
            { "start": 0, "end": 5, "kind": "task_toggle_route" },
            { "start": 6, "end": 15, "kind": "task_toggle_block_id" },
            { "start": 15, "end": 16, "kind": "task_toggle_explicit_toggle" },
        ])
    );
    assert!(json["diagnostics"].as_array().unwrap().is_empty());

    let output = bob_command()
        .arg("capture-parse")
        .arg("-f")
        .arg("json")
        .arg("--")
        .arg("@cash+goog-exit")
        .env("BOB_DIR", &vault)
        .output()
        .expect("run plain ensure-Next parse");
    assert_success(&output);
    let json: serde_json::Value =
        serde_json::from_str(stdout(&output).trim()).expect("parse JSON");
    assert_eq!(json["mode"], "task_toggle");
    assert_eq!(
        json["spans"],
        serde_json::json!([
            { "start": 0, "end": 5, "kind": "task_toggle_route" },
            { "start": 6, "end": 15, "kind": "task_toggle_block_id" },
        ])
    );

    let output = bob_command()
        .arg("capture-parse")
        .arg("-f")
        .arg("json")
        .arg("--")
        .arg("@cash+goog-exit#deep-work")
        .env("BOB_DIR", &vault)
        .output()
        .expect("run drawer parse");
    assert_success(&output);
    let json: serde_json::Value =
        serde_json::from_str(stdout(&output).trim()).expect("parse JSON");
    assert_eq!(json["mode"], "sub_bullet");
    assert_eq!(json["section"], "deep-work");
    assert_eq!(json["needs"], serde_json::json!(["drawer_text"]));
    assert_eq!(
        json["spans"],
        serde_json::json!([
            { "start": 0, "end": 5, "kind": "sub_bullet_route" },
            { "start": 6, "end": 15, "kind": "sub_bullet_block_id" },
            { "start": 16, "end": 25, "kind": "drawer", "title": "DEEP WORK", "key": "DEEP WORK" },
        ])
    );
}

#[test]
fn capture_parse_named_pomodoro_reports_incomplete_need() {
    let output = bob_command()
        .arg("capture-parse")
        .arg("-f")
        .arg("json")
        .arg("--")
        .arg("@dev:some-id#")
        .output()
        .expect("run capture-parse named pomodoro");
    assert_success(&output);
    let json: serde_json::Value =
        serde_json::from_str(stdout(&output).trim()).expect("parse JSON");
    assert_eq!(json["ok"], true);
    assert_eq!(json["mode"], "incomplete");
    assert_eq!(json["route"], "dev");
    assert_eq!(json["block_id"], "some-id");
    assert_eq!(json["needs"][0], "pomodoro_name");
}

#[test]
fn capture_parse_reports_global_destination_metadata() {
    let output = bob_command()
        .arg("capture-parse")
        .arg("-f")
        .arg("json")
        .arg("First task\n\nSecond @@foo\n\nThird @bar")
        .output()
        .expect("run global capture-parse");
    assert_success(&output);
    let json: serde_json::Value =
        serde_json::from_str(stdout(&output).trim()).expect("parse JSON");
    assert_eq!(json["schema_version"], 1);
    assert_eq!(json["route"], "foo");
    assert_eq!(json["global_destination"]["route"], "foo");
    assert!(json["global_destination"].get("mode").is_none());
    assert!(json["global_destination"].get("block_id").is_none());
    assert_eq!(json["global_destination"]["line"], 3);
    assert_eq!(json["items"][0]["route"], "foo");
    assert_eq!(json["items"][1]["route"], "foo");
    assert_eq!(json["items"][1]["body"], "Second");
    assert_eq!(json["items"][2]["route"], "bar");
    assert_eq!(json["spans"][0]["kind"], "global_route");
}

#[test]
fn capture_parse_reports_duplicate_global_destination_diagnostics() {
    let output = bob_command()
        .arg("capture-parse")
        .arg("-f")
        .arg("json")
        .arg("@@foo\nFirst @@bar\n\nSecond @@baz")
        .output()
        .expect("run duplicate global capture-parse");
    assert_success(&output);
    let json: serde_json::Value =
        serde_json::from_str(stdout(&output).trim()).expect("parse JSON");
    assert_eq!(json["global_destination"]["route"], "foo");
    let diagnostics = json["diagnostics"].as_array().expect("diagnostics");
    let duplicate_diagnostics: Vec<_> = diagnostics
        .iter()
        .filter(|diagnostic| {
            diagnostic["code"] == "duplicate_global_destination"
        })
        .collect();
    assert_eq!(duplicate_diagnostics.len(), 2, "{json}");
    assert_eq!(duplicate_diagnostics[0]["severity"], "error");
    assert!(duplicate_diagnostics[0]["message"]
        .as_str()
        .unwrap()
        .contains("line 2"));
    assert!(duplicate_diagnostics[1]["message"]
        .as_str()
        .unwrap()
        .contains("line 4"));
}

#[test]
fn capture_parse_and_capture_report_shadowed_global_destination_warnings() {
    let draft = "Buy milk @dev @@groceries\n\nOther";

    let parse_output = bob_command()
        .arg("capture-parse")
        .arg("-f")
        .arg("json")
        .arg(draft)
        .output()
        .expect("run shadowed global capture-parse");
    assert_success(&parse_output);
    let parse_json: serde_json::Value =
        serde_json::from_str(stdout(&parse_output).trim()).expect("parse JSON");
    assert_eq!(parse_json["items"][0]["route"], "dev");
    assert_eq!(parse_json["items"][1]["route"], "groceries");
    let diagnostic = parse_json["diagnostics"]
        .as_array()
        .expect("diagnostics")
        .iter()
        .find(|diagnostic| diagnostic["code"] == "global_destination_shadowed")
        .expect("shadow warning");
    assert_eq!(diagnostic["severity"], "warning");
    assert!(diagnostic["message"].as_str().unwrap().contains("@dev"));
    assert!(diagnostic["message"]
        .as_str()
        .unwrap()
        .contains("@@groceries"));

    let temp = TempDir::new("bob-cli-capture-global-shadowed");
    let vault = temp.path().join("vault");
    fs::create_dir_all(&vault).expect("create vault");
    let json_output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("-f")
        .arg("json")
        .arg(draft)
        .env("BOB_NOW", "2026-06-15")
        .output()
        .expect("run shadowed global capture json");
    assert_success(&json_output);
    assert!(
        stderr(&json_output).is_empty(),
        "json capture should keep warnings in JSON:\n{}",
        format_output(&json_output)
    );
    let capture_json: serde_json::Value =
        serde_json::from_str(stdout(&json_output).trim())
            .expect("capture JSON");
    assert_eq!(capture_json["warnings"][0], diagnostic["message"]);

    let human_output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("--dry-run")
        .arg(draft)
        .env("BOB_NOW", "2026-06-15")
        .output()
        .expect("run shadowed global capture human");
    assert_success(&human_output);
    assert!(stderr(&human_output).contains("warning"));
    assert!(stderr(&human_output).contains("@@groceries"));
}

#[test]
fn capture_parse_reports_sub_bullets_for_a_multiline_draft() {
    let output = bob_command()
        .arg("capture-parse")
        .arg("-f")
        .arg("json")
        .arg("@work parent line\n- first child\n- second child @work")
        .output()
        .expect("run multiline capture-parse");

    assert_success(&output);
    let json: serde_json::Value = serde_json::from_str(stdout(&output).trim())
        .expect("capture-parse JSON");
    assert_eq!(json["body"], "parent line");
    assert_eq!(json["route"], "work");
    assert_eq!(
        json["sub_bullets"],
        serde_json::json!(["first child", "second child"])
    );
    assert_eq!(json["sub_bullet_depths"], serde_json::json!([1, 1]));
}

#[test]
fn capture_parse_reports_nested_sub_bullets_and_depths() {
    let output = bob_command()
        .arg("capture-parse")
        .arg("-f")
        .arg("json")
        .arg("parent line\n- first child\n  - first detail @work\n- second child")
        .output()
        .expect("run nested multiline capture-parse");

    assert_success(&output);
    let json: serde_json::Value = serde_json::from_str(stdout(&output).trim())
        .expect("capture-parse JSON");
    assert_eq!(json["body"], "parent line");
    assert_eq!(json["route"], "work");
    assert_eq!(
        json["sub_bullets"],
        serde_json::json!(["first child", "first detail", "second child"])
    );
    assert_eq!(json["sub_bullet_depths"], serde_json::json!([1, 2, 1]));
}

#[test]
fn capture_parse_reports_orphaned_nested_bullet_diagnostic() {
    let output = bob_command()
        .arg("capture-parse")
        .arg("-f")
        .arg("json")
        .arg("parent line\n  - orphan @work")
        .output()
        .expect("run orphaned nested capture-parse");

    assert_success(&output);
    let json: serde_json::Value = serde_json::from_str(stdout(&output).trim())
        .expect("capture-parse JSON");
    assert!(json.get("sub_bullets").is_none(), "{json}");
    assert!(json.get("sub_bullet_depths").is_none(), "{json}");
    assert_eq!(json["route"], serde_json::Value::Null);
    assert_eq!(json["diagnostics"][0]["code"], "orphaned_nested_bullet");
}

#[test]
fn capture_parse_reports_start_suffix_spans_and_batch_offsets() {
    let vault_temp = TempDir::new("bob-cli-capture-parse-themes");
    let vault = vault_temp.path().join("vault");
    std::fs::create_dir_all(&vault).expect("create scratch vault");
    let parse = |text: &str| {
        let output = bob_command()
            .arg("capture-parse")
            .arg("-f")
            .arg("json")
            .arg("--")
            .arg(text)
            .env("BOB_DIR", &vault)
            .output()
            .expect("run capture-parse");
        assert_success(&output);
        serde_json::from_str::<serde_json::Value>(stdout(&output).trim())
            .expect("capture-parse JSON")
    };

    let named = parse("Do work @sase:outline#deep=-2");
    assert_eq!(named["schema_version"], 1);
    assert_eq!(named["mode"], "pomodoro_task");
    assert_eq!(named["section"], "deep");
    assert_eq!(
        named["pomodoro_start"],
        serde_json::json!({
            "raw": "-2",
            "duration_units": 5,
            "offset_units": 2,
        })
    );
    assert_eq!(
        named["spans"],
        serde_json::json!([
            { "start": 8, "end": 13, "kind": "pomodoro_route" },
            { "start": 14, "end": 21, "kind": "pomodoro_block_id" },
            {
                "start": 22,
                "end": 26,
                "kind": "pomodoro_name",
                "themes": [
                    {
                        "key": "deep",
                        "name": "deep",
                        "hue": "red",
                        "hue_source": "hash",
                        "known": false,
                        "start": 22,
                        "end": 26,
                    },
                ],
            },
            { "start": 26, "end": 29, "kind": "pomodoro_start" },
        ])
    );

    let block_only = parse("Do work @sase:outline=3");
    assert_eq!(
        block_only["pomodoro_start"],
        serde_json::json!({
            "raw": "3",
            "duration_units": 3,
            "offset_units": 0,
        })
    );

    // A partial marker still typing its block ID already reports the start.
    let partial = parse("Do work @sase:=3");
    assert_eq!(partial["mode"], "incomplete");
    assert_eq!(partial["needs"], serde_json::json!(["pomodoro_id"]));
    assert_eq!(
        partial["pomodoro_start"],
        serde_json::json!({
            "raw": "3",
            "duration_units": 3,
            "offset_units": 0,
        })
    );

    let invalid = parse("Do work @sase:outline=3abc");
    assert!(invalid.get("pomodoro_start").is_none(), "{invalid}");
    assert_eq!(invalid["diagnostics"][0]["code"], "invalid_pomodoro_start");

    // Older marker shapes keep the version-1 shape with no start field.
    let plain = parse("Do work @dev:id#bugs");
    assert!(plain.get("pomodoro_start").is_none(), "{plain}");

    let batch = parse("First @sase:one=3\n\nSecond @sase:two#deep=-");
    assert_eq!(batch["items"].as_array().expect("items").len(), 2);
    assert_eq!(
        batch["items"][0]["pomodoro_start"],
        serde_json::json!({
            "raw": "3",
            "duration_units": 3,
            "offset_units": 0,
        })
    );
    assert_eq!(
        batch["items"][1]["pomodoro_start"],
        serde_json::json!({
            "raw": "-",
            "duration_units": 5,
            "offset_units": 1,
        })
    );
}

#[test]
fn capture_parse_human_reports_start_suffix() {
    let output = bob_command()
        .arg("capture-parse")
        .arg("--")
        .arg("Do work @sase:outline=3")
        .output()
        .expect("run capture-parse human");
    assert_success(&output);
    let out = stdout(&output);
    assert!(out.contains("start"), "{out}");
    assert!(out.contains("=3 (15m, offset 0u)"), "{out}");
    assert!(out.contains("pomodoro_start"), "{out}");
    assert_stdout_has_no_ansi(&output);

    let plain = bob_command()
        .arg("capture-parse")
        .arg("--")
        .arg("Do work @dev:id#bugs")
        .output()
        .expect("run plain capture-parse human");
    assert_success(&plain);
    assert!(!stdout(&plain).contains("start"), "{}", stdout(&plain));
}

#[test]
fn capture_parse_json_reports_move_bullets() {
    // MC1 parse-side: a lone `- ^` in a sealed project-note item needs
    // `project_task_move` (not `block_id`), with marker and placeholder
    // spans and a null-ID move entry.
    let vault_temp = TempDir::new("bob-cli-capture-parse-moves");
    let vault = vault_temp.path().join("vault");
    std::fs::create_dir_all(&vault).expect("create scratch vault");
    let output = bob_command()
        .arg("capture-parse")
        .arg("-f")
        .arg("json")
        .arg("--")
        .arg("Finish @cash^x@\n- ^")
        .env("BOB_DIR", &vault)
        .output()
        .expect("run bob capture-parse unfinished move");
    assert_success(&output);
    let json: serde_json::Value = serde_json::from_str(stdout(&output).trim())
        .expect("capture-parse JSON");
    assert_eq!(json["grammar"], 9);
    assert_eq!(json["mode"], "incomplete");
    assert_eq!(json["needs"], serde_json::json!(["project_task_move"]));
    assert_eq!(
        json["spans"],
        serde_json::json!([
            { "start": 7, "end": 12, "kind": "task_block_id_route", "project_note": "create" },
            { "start": 12, "end": 13, "kind": "project_note_separator", "project_note": "create" },
            { "start": 13, "end": 14, "kind": "task_block_id", "project_note": "create" },
            { "start": 14, "end": 15, "kind": "project_note_marker", "project_note": "create" },
            { "start": 18, "end": 19, "kind": "project_task_move_marker" },
            { "start": 18, "end": 19, "kind": "interactive_placeholder" },
        ])
    );
    assert_eq!(
        json["sub_bullet_moves"],
        serde_json::json!([
            { "sigil": "^", "block_id": null, "range": [18, 19] },
        ])
    );
    assert!(json["diagnostics"].as_array().unwrap().is_empty());

    // MC2 spans: a partial `- ^dr` keeps the marker span over `^` and the
    // block span over the typed prefix.
    let output = bob_command()
        .arg("capture-parse")
        .arg("-f")
        .arg("json")
        .arg("--")
        .arg("Finish @cash^x@\n- ^dr")
        .env("BOB_DIR", &vault)
        .output()
        .expect("run bob capture-parse partial move");
    assert_success(&output);
    let json: serde_json::Value = serde_json::from_str(stdout(&output).trim())
        .expect("capture-parse JSON");
    assert_eq!(json["mode"], "project_note");
    assert_eq!(
        json["spans"],
        serde_json::json!([
            { "start": 7, "end": 12, "kind": "task_block_id_route", "project_note": "create" },
            { "start": 12, "end": 13, "kind": "project_note_separator", "project_note": "create" },
            { "start": 13, "end": 14, "kind": "task_block_id", "project_note": "create" },
            { "start": 14, "end": 15, "kind": "project_note_marker", "project_note": "create" },
            { "start": 18, "end": 19, "kind": "project_task_move_marker" },
            { "start": 19, "end": 21, "kind": "project_task_move_block_id" },
        ])
    );
    assert_eq!(
        json["sub_bullet_moves"],
        serde_json::json!([
            { "sigil": "^", "block_id": "dr", "range": [18, 21] },
        ])
    );
    assert_eq!(json["sub_bullets"], serde_json::json!([""]));

    // MV7 lexical: a repeated move reports `move_bullet_duplicate`.
    let output = bob_command()
        .arg("capture-parse")
        .arg("-f")
        .arg("json")
        .arg("--")
        .arg("Finish @cash^x@\n- ^a\n- ^a")
        .env("BOB_DIR", &vault)
        .output()
        .expect("run bob capture-parse duplicate move");
    assert_success(&output);
    let json: serde_json::Value = serde_json::from_str(stdout(&output).trim())
        .expect("capture-parse JSON");
    assert_eq!(json["diagnostics"][0]["code"], "move_bullet_duplicate");
    assert_eq!(
        json["diagnostics"][0]["message"],
        serde_json::json!("`^a` is already moved on line 2")
    );
}
