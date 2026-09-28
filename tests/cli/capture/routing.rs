//! Remaining capture inbox, schedule, dry-run, JSON routing.

use crate::support::*;
use std::fs;
use std::io::Write;
use std::process::Stdio;

#[test]
fn capture_unrouted_appends_to_mac_inbox() {
    let temp = TempDir::new("bob-cli-capture-inbox");
    let vault = temp.path().join("vault");
    fs::create_dir_all(&vault).expect("create vault");

    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("buy")
        .arg("milk")
        .env("BOB_NOW", "2026-06-15 10:11:12")
        .output()
        .expect("run bob capture inbox");

    assert_success(&output);
    assert!(
        stderr(&output).is_empty(),
        "unexpected capture stderr:\n{}",
        format_output(&output)
    );
    let out = stdout(&output);
    assert!(
        out.contains("captured  mac_inbox.md")
            && out.contains("- [ ] #task buy milk [created::2026-06-15]"),
        "unexpected capture output:\n{out}"
    );
    assert_stdout_has_no_ansi(&output);
    assert_eq!(
        fs::read_to_string(vault.join("mac_inbox.md")).expect("read inbox"),
        "- [ ] #task buy milk [created::2026-06-15]\n"
    );
}

#[test]
fn capture_unrouted_scheduled_offset_appends_property() {
    let temp = TempDir::new("bob-cli-capture-scheduled-inbox");
    let vault = temp.path().join("vault");
    fs::create_dir_all(&vault).expect("create vault");

    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("buy")
        .arg("milk")
        .arg("s:1")
        .env("BOB_NOW", "2026-06-15 10:11:12")
        .output()
        .expect("run scheduled inbox capture");

    assert_success(&output);
    let expected =
        "- [?] #task buy milk [created::2026-06-15] [scheduled::2026-06-16]";
    assert!(
        stdout(&output).contains(expected),
        "unexpected capture output:\n{}",
        format_output(&output)
    );
    assert_eq!(
        fs::read_to_string(vault.join("mac_inbox.md")).expect("read inbox"),
        format!("{expected}\n")
    );
}

#[test]
fn capture_scheduled_zero_uses_created_date() {
    let temp = TempDir::new("bob-cli-capture-scheduled-zero");
    let vault = temp.path().join("vault");
    fs::create_dir_all(&vault).expect("create vault");

    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("buy")
        .arg("milk")
        .arg("s:0")
        .env("BOB_NOW", "2026-06-15 10:11:12")
        .output()
        .expect("run zero scheduled capture");

    assert_success(&output);
    assert_eq!(
        fs::read_to_string(vault.join("mac_inbox.md")).expect("read inbox"),
        "- [?] #task buy milk [created::2026-06-15] [scheduled::2026-06-15]\n"
    );
}

#[test]
fn capture_unrouted_prefers_tasks_section_in_existing_inbox() {
    let temp = TempDir::new("bob-cli-capture-inbox-tasks-section");
    let vault = temp.path().join("vault");
    write_file(
        &vault.join("mac_inbox.md"),
        "# Inbox\n- [ ] #task root\n## Tasks\n- [ ] #task existing\nTail\n",
    );

    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("buy")
        .arg("milk")
        .env("BOB_NOW", "2026-06-15 10:11:12")
        .output()
        .expect("run bob capture existing inbox");

    assert_success(&output);
    assert!(
        stdout(&output).contains("captured  mac_inbox.md"),
        "unexpected capture output:\n{}",
        format_output(&output)
    );
    assert_eq!(
        fs::read_to_string(vault.join("mac_inbox.md")).expect("read inbox"),
        "# Inbox\n- [ ] #task root\n## Tasks\n- [ ] #task existing\n- [ ] #task buy milk [created::2026-06-15]\nTail\n"
    );
}

#[test]
fn capture_routed_prefix_inserts_and_suffix_creates_file() {
    let temp = TempDir::new("bob-cli-capture-routed");
    let vault = temp.path().join("vault");
    write_file(
        &vault.join("groceries.md"),
        "# Groceries\n- [ ] #task existing\n  detail\n\nNext\n",
    );

    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("@Groceries")
        .arg("pick")
        .arg("apples")
        .env("BOB_NOW", "2026-06-15")
        .output()
        .expect("run prefix routed capture");

    assert_success(&output);
    assert!(
        stdout(&output).contains("captured  groceries.md"),
        "unexpected prefix capture output:\n{}",
        format_output(&output)
    );
    assert_eq!(
        fs::read_to_string(vault.join("groceries.md")).expect("read groceries"),
        "# Groceries\n- [ ] #task existing\n  detail\n- [ ] #task pick apples [created::2026-06-15]\n\nNext\n"
    );

    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("call")
        .arg("vet")
        .arg("@Errands")
        .env("BOB_NOW", "2026-06-15")
        .output()
        .expect("run suffix routed capture");

    assert_success(&output);
    assert!(
        stdout(&output).contains("captured  errands.md"),
        "unexpected suffix capture output:\n{}",
        format_output(&output)
    );
    assert_eq!(
        fs::read_to_string(vault.join("errands.md")).expect("read errands"),
        "- [ ] #task call vet [created::2026-06-15]\n"
    );
}

#[test]
fn capture_scheduled_offset_routes_in_either_order() {
    let render = |args: &[&str]| -> String {
        let temp = TempDir::new("bob-cli-capture-scheduled-route-order");
        let vault = temp.path().join("vault");
        fs::create_dir_all(&vault).expect("create vault");

        let mut command = bob_command();
        command.arg("capture").arg("-b").arg(&vault);
        for arg in args {
            command.arg(arg);
        }
        let output = command
            .env("BOB_NOW", "2026-06-15 10:11:12")
            .output()
            .expect("run scheduled routed capture");

        assert_success(&output);
        fs::read_to_string(vault.join("groceries.md")).expect("read groceries")
    };

    let schedule_then_route = render(&["buy", "milk", "s:2", "@groceries"]);
    let route_then_schedule = render(&["buy", "milk", "@groceries", "s:2"]);
    assert_eq!(schedule_then_route, route_then_schedule);
    assert_eq!(
        schedule_then_route,
        "- [?] #task buy milk [created::2026-06-15] [scheduled::2026-06-17]\n"
    );

    let leading_route = render(&["@groceries", "buy", "milk", "s:3"]);
    assert_eq!(
        leading_route,
        "- [?] #task buy milk [created::2026-06-15] [scheduled::2026-06-18]\n"
    );
}

#[test]
fn capture_routed_prefers_tasks_section_over_root_task() {
    let temp = TempDir::new("bob-cli-capture-routed-tasks-section");
    let vault = temp.path().join("vault");
    write_file(
        &vault.join("groceries.md"),
        "# Groceries\n- [ ] #task root\n## Tasks\nNotes\n",
    );

    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("@Groceries")
        .arg("pick")
        .arg("apples")
        .env("BOB_NOW", "2026-06-15")
        .output()
        .expect("run routed capture into tasks section");

    assert_success(&output);
    assert!(
        stdout(&output).contains("captured  groceries.md"),
        "unexpected capture output:\n{}",
        format_output(&output)
    );
    assert_eq!(
        fs::read_to_string(vault.join("groceries.md")).expect("read groceries"),
        "# Groceries\n- [ ] #task root\n## Tasks\n\n- [ ] #task pick apples [created::2026-06-15]\nNotes\n"
    );
}

#[test]
fn capture_nonterminal_schedule_token_stays_literal() {
    let temp = TempDir::new("bob-cli-capture-scheduled-literal-middle");
    let vault = temp.path().join("vault");
    fs::create_dir_all(&vault).expect("create vault");

    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("take")
        .arg("s:1")
        .arg("pill")
        .env("BOB_NOW", "2026-06-15")
        .output()
        .expect("run literal middle schedule capture");

    assert_success(&output);
    assert_eq!(
        fs::read_to_string(vault.join("mac_inbox.md")).expect("read inbox"),
        "- [ ] #task take s:1 pill [created::2026-06-15]\n"
    );
}

#[test]
fn capture_route_override_keeps_at_tokens_literal() {
    let temp = TempDir::new("bob-cli-capture-route-override");
    let vault = temp.path().join("vault");
    fs::create_dir_all(&vault).expect("create vault");

    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("-r")
        .arg("Work")
        .arg("buy")
        .arg("milk")
        .arg("@groceries")
        .env("BOB_NOW", "2026-06-15")
        .output()
        .expect("run forced-route capture");

    assert_success(&output);
    assert_eq!(
        fs::read_to_string(vault.join("work.md")).expect("read work route"),
        "- [ ] #task buy milk @groceries [created::2026-06-15]\n"
    );
    assert!(
        !vault.join("groceries.md").exists(),
        "--route should bypass auto @route parsing"
    );
}

#[test]
fn capture_dry_run_reports_without_writing() {
    let temp = TempDir::new("bob-cli-capture-dry-run");
    let vault = temp.path().join("vault");
    fs::create_dir_all(&vault).expect("create vault");

    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("-d")
        .arg("buy")
        .arg("milk")
        .arg("@groceries")
        .env("BOB_NOW", "2026-06-15")
        .output()
        .expect("run dry-run capture");

    assert_success(&output);
    let out = stdout(&output);
    assert!(
        out.contains("[dry-run] ok would capture  groceries.md")
            && out.contains("- [ ] #task buy milk [created::2026-06-15]"),
        "unexpected dry-run output:\n{out}"
    );
    assert!(
        !vault.join("groceries.md").exists(),
        "dry-run must not create routed target"
    );
    assert_stdout_has_no_ansi(&output);
}

#[test]
fn capture_scheduled_dry_run_reports_without_writing() {
    let temp = TempDir::new("bob-cli-capture-scheduled-dry-run");
    let vault = temp.path().join("vault");
    fs::create_dir_all(&vault).expect("create vault");

    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("-d")
        .arg("buy")
        .arg("milk")
        .arg("s:1")
        .arg("@groceries")
        .env("BOB_NOW", "2026-06-15")
        .output()
        .expect("run scheduled dry-run capture");

    assert_success(&output);
    let out = stdout(&output);
    assert!(
        out.contains("[dry-run] ok would capture  groceries.md")
            && out.contains("- [?] #task buy milk [created::2026-06-15] [scheduled::2026-06-16]"),
        "unexpected dry-run output:\n{out}"
    );
    assert!(
        !vault.join("groceries.md").exists(),
        "dry-run must not create routed target"
    );
    assert_stdout_has_no_ansi(&output);
}

#[test]
fn capture_json_output_is_machine_readable() {
    let temp = TempDir::new("bob-cli-capture-json");
    let vault = temp.path().join("vault");
    fs::create_dir_all(&vault).expect("create vault");

    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("-f")
        .arg("json")
        .arg("buy")
        .arg("milk")
        .arg("@Groceries")
        .env("BOB_NOW", "2026-06-15")
        .output()
        .expect("run json capture");

    assert_success(&output);
    assert!(
        stderr(&output).is_empty(),
        "json capture should keep stderr clean:\n{}",
        format_output(&output)
    );
    let json: serde_json::Value = serde_json::from_str(stdout(&output).trim())
        .unwrap_or_else(|error| {
            panic!("stdout should be JSON: {error}\n{}", format_output(&output))
        });
    assert_eq!(json["ok"], true);
    assert_eq!(json["dry_run"], false);
    assert_eq!(json["routed"], true);
    assert_eq!(json["route"], "groceries");
    assert_eq!(json["route_label"], "groceries.md");
    assert_eq!(json["relative_target"], "groceries.md");
    assert_eq!(
        json["target"],
        vault.join("groceries.md").display().to_string()
    );
    assert_eq!(json["text"], "buy milk");
    assert_eq!(
        json["task_line"],
        "- [ ] #task buy milk [created::2026-06-15]"
    );
    assert_eq!(json["created"], "2026-06-15");
    assert!(json["scheduled"].is_null(), "unexpected json: {json}");
    assert_eq!(json["placement"], "created");
    assert!(json.get("captures").is_none(), "{json}");
}

#[test]
fn capture_json_output_includes_scheduled_date() {
    let temp = TempDir::new("bob-cli-capture-scheduled-json");
    let vault = temp.path().join("vault");
    fs::create_dir_all(&vault).expect("create vault");

    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("-f")
        .arg("json")
        .arg("buy")
        .arg("milk")
        .arg("s:1")
        .env("BOB_NOW", "2026-06-15")
        .output()
        .expect("run scheduled json capture");

    assert_success(&output);
    let json: serde_json::Value = serde_json::from_str(stdout(&output).trim())
        .unwrap_or_else(|error| {
            panic!("stdout should be JSON: {error}\n{}", format_output(&output))
        });
    assert_eq!(json["text"], "buy milk");
    assert_eq!(json["created"], "2026-06-15");
    assert_eq!(json["scheduled"], "2026-06-16");
    assert_eq!(
        json["task_line"],
        "- [?] #task buy milk [created::2026-06-15] [scheduled::2026-06-16]"
    );
}

#[test]
fn capture_reads_the_complete_piped_stdin_stream_when_text_is_absent() {
    let temp = TempDir::new("bob-cli-capture-stdin");
    let vault = temp.path().join("vault");
    fs::create_dir_all(&vault).expect("create vault");

    let mut child = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .env("BOB_NOW", "2026-06-15")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn stdin capture");
    child
        .stdin
        .as_mut()
        .expect("stdin pipe")
        .write_all(b"ping team @work\n- follow up tomorrow\n")
        .expect("write stdin");
    drop(child.stdin.take());

    let output = child.wait_with_output().expect("wait stdin capture");
    assert_success(&output);
    assert_eq!(
        fs::read_to_string(vault.join("work.md")).expect("read work route"),
        "- [ ] #task ping team [created::2026-06-15]\n\t- follow up tomorrow\n"
    );
}

#[test]
fn capture_rejects_stdin_continuation_text_that_is_not_a_bullet() {
    let temp = TempDir::new("bob-cli-capture-stdin-invalid");
    let vault = temp.path().join("vault");
    fs::create_dir_all(&vault).expect("create vault");

    let mut child = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .env("BOB_NOW", "2026-06-15")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn stdin capture");
    child
        .stdin
        .as_mut()
        .expect("stdin pipe")
        .write_all(b"ping team @work\nignored input\n")
        .expect("write stdin");
    drop(child.stdin.take());

    let output = child.wait_with_output().expect("wait stdin capture");
    assert_eq!(output.status.code(), Some(2));
    assert_eq!(
        String::from_utf8_lossy(&output.stderr).trim(),
        "bob capture: capture item 1 starting on line 1: capture line 2 must be a column-zero bullet or a two-space nested bullet using \"-\", \"*\", or \"+\" followed by a space or tab, or be left blank"
    );
    assert!(!vault.join("work.md").exists());
}

#[test]
fn capture_json_omits_sub_bullets_for_an_ordinary_single_line_capture() {
    let temp = TempDir::new("bob-cli-capture-authored-omit");
    let vault = temp.path().join("vault");
    fs::create_dir_all(&vault).expect("create vault");

    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("-f")
        .arg("json")
        .arg("buy milk @groceries")
        .env("BOB_NOW", "2026-06-15")
        .output()
        .expect("run ordinary single-line capture");

    assert_success(&output);
    let json: serde_json::Value =
        serde_json::from_str(stdout(&output).trim()).expect("capture JSON");
    assert!(json.get("sub_bullets").is_none(), "{json}");
}

#[test]
fn capture_empty_input_is_usage_error() {
    let temp = TempDir::new("bob-cli-capture-empty");
    let vault = temp.path().join("vault");
    fs::create_dir_all(&vault).expect("create vault");

    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .output()
        .expect("run empty capture");

    assert_eq!(
        output.status.code(),
        Some(2),
        "empty capture should be a usage error:\n{}",
        format_output(&output)
    );
    assert!(
        stdout(&output).is_empty()
            && stderr(&output).contains("task text is required"),
        "expected empty-input error:\n{}",
        format_output(&output)
    );
}

#[test]
fn capture_schedule_only_is_usage_error() {
    let temp = TempDir::new("bob-cli-capture-schedule-only");
    let vault = temp.path().join("vault");
    fs::create_dir_all(&vault).expect("create vault");

    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("s:1")
        .env("BOB_NOW", "2026-06-15")
        .output()
        .expect("run schedule-only capture");

    assert_eq!(
        output.status.code(),
        Some(2),
        "schedule-only capture should be a usage error:\n{}",
        format_output(&output)
    );
    assert!(
        stderr(&output).contains("task text is required"),
        "expected schedule-only error:\n{}",
        format_output(&output)
    );
    assert!(
        !vault.join("mac_inbox.md").exists(),
        "usage error must not write inbox"
    );
}

#[test]
fn capture_scheduled_offset_out_of_range_is_usage_error() {
    let temp = TempDir::new("bob-cli-capture-schedule-overflow");
    let vault = temp.path().join("vault");
    fs::create_dir_all(&vault).expect("create vault");

    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("buy")
        .arg("milk")
        .arg("s:9999999999")
        .env("BOB_NOW", "2026-06-15")
        .output()
        .expect("run schedule overflow capture");

    assert_eq!(
        output.status.code(),
        Some(2),
        "overflow capture should be a usage error:\n{}",
        format_output(&output)
    );
    assert!(
        stderr(&output).contains("scheduled offset is out of range"),
        "expected schedule overflow error:\n{}",
        format_output(&output)
    );
    assert!(
        !vault.join("mac_inbox.md").exists(),
        "usage error must not write inbox"
    );
}

#[test]
fn capture_json_failure_prints_error_object() {
    let temp = TempDir::new("bob-cli-capture-json-failure");
    let missing_vault = temp.path().join("missing-vault");

    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&missing_vault)
        .arg("-f")
        .arg("json")
        .arg("buy")
        .arg("milk")
        .env("BOB_NOW", "2026-06-15")
        .output()
        .expect("run json capture failure");

    assert_eq!(
        output.status.code(),
        Some(1),
        "missing vault should be an IO failure:\n{}",
        format_output(&output)
    );
    assert!(
        stderr(&output).is_empty(),
        "json failure should keep stderr clean:\n{}",
        format_output(&output)
    );
    let json: serde_json::Value = serde_json::from_str(stdout(&output).trim())
        .unwrap_or_else(|error| {
            panic!("stdout should be JSON: {error}\n{}", format_output(&output))
        });
    assert_eq!(json["ok"], false);
    assert!(
        json["error"]
            .as_str()
            .is_some_and(|error| error.contains("create target")),
        "unexpected json failure object: {json}"
    );
}

#[test]
fn capture_routed_bullet_inserts_into_section_by_prefix() {
    let temp = TempDir::new("bob-cli-capture-bullet-routed");
    let vault = temp.path().join("vault");
    write_file(
        &vault.join("foo.md"),
        "# Foo\n## Ideas\n- existing idea\nTail\n",
    );

    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("Some")
        .arg("bullet")
        .arg("@foo#Ideas")
        .env("BOB_NOW", "2026-06-15")
        .output()
        .expect("run routed bullet capture");

    assert_success(&output);
    assert!(
        stdout(&output).contains("captured  foo.md")
            && stdout(&output).contains("- Some bullet [created::2026-06-15]"),
        "unexpected bullet capture output:\n{}",
        format_output(&output)
    );
    assert_eq!(
        fs::read_to_string(vault.join("foo.md")).expect("read foo"),
        "# Foo\n## Ideas\n- existing idea\n- Some bullet [created::2026-06-15]\nTail\n"
    );
}

#[test]
fn capture_leading_route_bullet_inserts_into_section_by_prefix() {
    let temp = TempDir::new("bob-cli-capture-bullet-leading");
    let vault = temp.path().join("vault");
    write_file(
        &vault.join("foo.md"),
        "# Foo\n## Ideas\n- existing idea\nTail\n",
    );

    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("@foo#Ideas")
        .arg("Some")
        .arg("bullet")
        .env("BOB_NOW", "2026-06-15")
        .output()
        .expect("run leading-route bullet capture");

    assert_success(&output);
    assert!(
        stdout(&output).contains("captured  foo.md")
            && stdout(&output).contains("- Some bullet [created::2026-06-15]"),
        "unexpected bullet capture output:\n{}",
        format_output(&output)
    );
    assert_eq!(
        fs::read_to_string(vault.join("foo.md")).expect("read foo"),
        "# Foo\n## Ideas\n- existing idea\n- Some bullet [created::2026-06-15]\nTail\n"
    );
}

#[test]
fn capture_bullet_marker_order_routes_equivalently() {
    let render = |args: &[&str]| -> String {
        let temp = TempDir::new("bob-cli-capture-bullet-order");
        let vault = temp.path().join("vault");
        write_file(&vault.join("foo.md"), "## Ideas\n- existing\n");

        let mut command = bob_command();
        command.arg("capture").arg("-b").arg(&vault);
        for arg in args {
            command.arg(arg);
        }
        let output = command
            .env("BOB_NOW", "2026-06-15")
            .output()
            .expect("run bullet capture");

        assert_success(&output);
        fs::read_to_string(vault.join("foo.md")).expect("read foo")
    };

    let trailing = render(&["Some", "bullet", "@foo#"]);
    let leading = render(&["@foo#", "Some", "bullet"]);
    assert_eq!(trailing, leading);
    assert_eq!(
        trailing,
        "## Ideas\n- existing\n- Some bullet [created::2026-06-15]\n"
    );
}

#[test]
fn capture_legacy_standalone_marker_form_is_usage_error() {
    let temp = TempDir::new("bob-cli-capture-bullet-legacy");
    let vault = temp.path().join("vault");
    write_file(&vault.join("foo.md"), "## Ideas\n- existing\n");

    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("Some")
        .arg("bullet")
        .arg("#Ideas")
        .arg("@foo")
        .env("BOB_NOW", "2026-06-15")
        .output()
        .expect("run legacy standalone marker capture");

    assert_eq!(
        output.status.code(),
        Some(2),
        "legacy standalone marker should be a usage error:\n{}",
        format_output(&output)
    );
    assert!(
        stderr(&output).contains("@foo#bar instead of #bar @foo"),
        "expected legacy marker usage error:\n{}",
        format_output(&output)
    );
    assert_eq!(
        fs::read_to_string(vault.join("foo.md")).expect("read foo"),
        "## Ideas\n- existing\n",
        "legacy marker must not modify the target"
    );
}

#[test]
fn capture_bullet_json_reports_rendered_line() {
    let temp = TempDir::new("bob-cli-capture-bullet-json");
    let vault = temp.path().join("vault");
    write_file(&vault.join("foo.md"), "## Ideas\n- existing\n");

    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("-f")
        .arg("json")
        .arg("Some")
        .arg("bullet")
        .arg("@foo#Ideas")
        .env("BOB_NOW", "2026-06-15")
        .output()
        .expect("run json bullet capture");

    assert_success(&output);
    assert!(
        stderr(&output).is_empty(),
        "json bullet capture should keep stderr clean:\n{}",
        format_output(&output)
    );
    let json: serde_json::Value = serde_json::from_str(stdout(&output).trim())
        .unwrap_or_else(|error| {
            panic!("stdout should be JSON: {error}\n{}", format_output(&output))
        });
    assert_eq!(json["ok"], true);
    assert_eq!(json["routed"], true);
    assert_eq!(json["route"], "foo");
    assert_eq!(json["kind"], "bullet");
    assert_eq!(json["text"], "Some bullet");
    assert_eq!(json["task_line"], "- Some bullet [created::2026-06-15]");
    assert_eq!(json["placement"], "inserted");
}

#[test]
fn capture_bullet_prefix_prefers_non_h1_and_ignores_prefix_case() {
    let render = |route: &str| -> String {
        let temp = TempDir::new("bob-cli-capture-bullet-section");
        let vault = temp.path().join("vault");
        write_file(
            &vault.join("foo.md"),
            "# Roadmap\nintro\n\n## Research\nnotes\n",
        );

        let output = bob_command()
            .arg("capture")
            .arg("-b")
            .arg(&vault)
            .arg("Some")
            .arg("note")
            .arg(route)
            .env("BOB_NOW", "2026-06-15")
            .output()
            .expect("run routed bullet capture");

        assert_success(&output);
        fs::read_to_string(vault.join("foo.md")).expect("read foo")
    };

    let lowercase = render("@foo#r");
    assert_eq!(
        lowercase,
        "# Roadmap\nintro\n\n## Research\n\n- Some note [created::2026-06-15]\nnotes\n"
    );

    // A `#R` prefix selects the same section and produces identical contents.
    let uppercase = render("@foo#R");
    assert_eq!(lowercase, uppercase);
}
