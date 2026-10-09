//! Capture `==` override restart tests (phase override-restart).

use crate::support::*;
use std::fs;

const NOW: &str = "2026-10-09 09:32:00";

fn worked_day() -> String {
    concat!(
        "## Pomodoros\n",
        "\n",
        "- [x] (**0830-0855** [t:: 25m]) — PLAN\n",
        "\t- \u{1F345} [[bob#^capture-stop]]\n",
        "- [ ] (**0920-0945** [t:: 25m]) — CAPTURE\n",
        "\t- [[bob#^capture-stop]]\n",
        "\t- [[bob#^web-capture]]\n",
        "- [ ] () — SASE\n",
        "- [ ] () — BUGS\n",
        "\t- [[sase#^fix-flaky]]\n",
    )
    .to_string()
}

fn worked_vault(
    name: &str,
) -> (TempDir, std::path::PathBuf, std::path::PathBuf) {
    let temp = TempDir::new(name);
    let vault = temp.path().join("vault");
    let day_file = vault.join("day.md");
    write_toggle_task_settings(&vault);
    write_file(&day_file, &worked_day());
    write_file(
        &vault.join("bob.md"),
        concat!(
            "## Tasks\n",
            "\n",
            "- [*] #task Add support for `=x` syntax! [created::2026-10-01] ^capture-stop\n",
            "- [*] #task Add capture support for web URLs! [created::2026-10-01] ^web-capture\n",
        ),
    );
    write_file(
        &vault.join("sase.md"),
        concat!(
            "## Tasks\n",
            "\n",
            "- [*] #task Fix flaky gkeep test [created::2026-10-01] ^fix-flaky\n",
        ),
    );
    (temp, vault, day_file)
}

fn run_json(
    vault: &std::path::Path,
    day_file: &std::path::Path,
    args: &[&str],
) -> serde_json::Value {
    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(vault)
        .arg("-f")
        .arg("json")
        .arg("--")
        .args(args)
        .env("BOB_DAY_FILE", day_file)
        .env("BOB_NOW", NOW)
        .output()
        .expect("run capture");
    assert_success(&output);
    serde_json::from_str(stdout(&output).trim()).expect("capture JSON")
}

#[test]
fn override_restart_worked_example_rows() {
    let cases = [
        (
            "==",
            "0935",
            "1000",
            25,
            "(**0935-1000** [t:: 25m]) — CAPTURE",
        ),
        (
            "==3",
            "0935",
            "0950",
            15,
            "(**0935-0950** [t:: 15m]) — CAPTURE",
        ),
        (
            "==-2",
            "0925",
            "0950",
            25,
            "(**0925-0950** [t:: 25m]) — CAPTURE",
        ),
    ];
    for (token, expect_start, expect_end, expect_dur, expect_fragment) in cases
    {
        let (_temp, vault, day_file) =
            worked_vault("bob-cli-override-restart-row");
        let json = run_json(&vault, &day_file, &[token]);
        assert_eq!(json["kind"], "pomodoro_start", "{token}");
        assert_eq!(json["text"], token, "{token}");
        assert_eq!(json["placement"], "started", "{token}");
        let start = &json["pomodoro_start"];
        assert_eq!(start["start"], expect_start, "{token}");
        assert_eq!(start["end"], expect_end, "{token}");
        assert_eq!(start["duration_minutes"], expect_dur, "{token}");
        assert_eq!(start["pomodoro_name"], "CAPTURE", "{token}");
        assert_eq!(start["created_pomodoro"], false, "{token}");
        let override_json = &start["override"];
        assert_eq!(override_json["action"], "restart", "{token}: {json}");
        assert_eq!(override_json["ledger"], "fresh", "{token}: {json}");
        assert_eq!(
            override_json["previous"]["pomodoro_name"], "CAPTURE",
            "{token}: {json}"
        );
        assert_eq!(
            override_json["previous"]["time_range"], "0920-0945",
            "{token}: {json}"
        );
        assert_eq!(
            override_json["previous"]["start"], "0920",
            "{token}: {json}"
        );
        assert!(override_json.get("demoted").is_none(), "{token}: {json}");
        let day_after = fs::read_to_string(&day_file).expect("read day");
        assert!(day_after.contains(expect_fragment), "{token}: {day_after}");
        assert!(
            day_after.contains("[[bob#^capture-stop]]")
                && day_after.contains("[[bob#^web-capture]]"),
            "{token}: lineup kept: {day_after}"
        );
    }

    // `==~2` drops the second queued link and restarts at the default time.
    let (_temp, vault, day_file) =
        worked_vault("bob-cli-override-restart-drop");
    let json = run_json(&vault, &day_file, &["==~2"]);
    let start = &json["pomodoro_start"];
    assert_eq!(start["override"]["action"], "restart", "{json}");
    assert_eq!(start["time_range"], "(**0935-1000** [t:: 25m])", "{json}");
    assert_eq!(start["tasks"].as_array().map(Vec::len), Some(1), "{json}");
    let day_after = fs::read_to_string(&day_file).expect("read day");
    assert!(
        day_after.contains("(**0935-1000** [t:: 25m]) — CAPTURE"),
        "{day_after}"
    );
    assert!(
        day_after.contains("[[bob#^capture-stop]]")
            && !day_after.contains("[[bob#^web-capture]]"),
        "{day_after}"
    );
}

#[test]
fn override_restart_human_output() {
    let (_temp, vault, day_file) =
        worked_vault("bob-cli-override-restart-human");
    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("--dry-run")
        .arg("--")
        .arg("==3")
        .env("BOB_DAY_FILE", &day_file)
        .env("BOB_NOW", NOW)
        .output()
        .expect("run restart human");
    assert_success(&output);
    let out = stdout(&output);
    assert!(
        out.contains("would restart")
            && out.contains(
                "CAPTURE 0920-0945 \u{2192} 0935-0950 (15m) at line 5"
            ),
        "{out}"
    );
    assert_stdout_has_no_ansi(&output);
}

#[test]
fn override_idle_fallback_matches_plain_start() {
    for token in ["==", "==3"] {
        let plain = token.strip_prefix("==").expect("strip == sigil");
        let plain_token = format!("={plain}");
        let (_temp_eq, vault_eq, day_eq) =
            worked_vault("bob-cli-override-idle-plain");
        // Remove the running ledger so nothing runs: CAPTURE back to ().
        let idle_day = worked_day().replace(
            "- [ ] (**0920-0945** [t:: 25m]) — CAPTURE",
            "- [ ] () — CAPTURE",
        );
        write_file(&day_eq, &idle_day);
        let plain_json = run_json(&vault_eq, &day_eq, &[&plain_token]);
        let plain_day = fs::read_to_string(&day_eq).expect("read plain day");

        let (_temp_ov, vault_ov, day_ov) =
            worked_vault("bob-cli-override-idle-override");
        write_file(&day_ov, &idle_day);
        let override_json = run_json(&vault_ov, &day_ov, &[token]);
        let override_day = fs::read_to_string(&day_ov).expect("read day");
        assert_eq!(override_day, plain_day, "{token}");
        assert_eq!(
            override_json["pomodoro_start"]["start"],
            plain_json["pomodoro_start"]["start"],
            "{token}"
        );
        assert!(
            override_json["pomodoro_start"].get("override").is_some(),
            "{token}: {override_json}"
        );
        assert_eq!(
            override_json["pomodoro_start"]["override"]["action"], "start",
            "{token}: {override_json}"
        );
        assert!(
            override_json["pomodoro_start"]["override"]
                .get("previous")
                .is_none(),
            "{token}: {override_json}"
        );
        assert!(
            override_json["pomodoro_start"]["override"]
                .get("demoted")
                .is_none(),
            "{token}: {override_json}"
        );
        assert!(
            plain_json["pomodoro_start"].get("override").is_none(),
            "{token}: {plain_json}"
        );

        // Human output adds the idle note (fresh vault: the real run
        // above already started the session there).
        let (_temp_h, vault_h, day_h) =
            worked_vault("bob-cli-override-idle-human");
        write_file(&day_h, &idle_day);
        let human = bob_command()
            .arg("capture")
            .arg("-b")
            .arg(&vault_h)
            .arg("--dry-run")
            .arg("--")
            .arg(token)
            .env("BOB_DAY_FILE", &day_h)
            .env("BOB_NOW", NOW)
            .output()
            .expect("run idle human");
        assert_success(&human);
        let out = stdout(&human);
        assert!(
            out.contains("nothing was running, so == started it like ="),
            "{token}: {out}"
        );
    }
}

#[test]
fn override_restart_matches_reset_then_start_on_note_free() {
    let (_temp_a, vault_a, day_a) =
        worked_vault("bob-cli-override-restart-eq-a");
    run_json(&vault_a, &day_a, &["==3"]);
    let restart_day = fs::read_to_string(&day_a).expect("read restart day");

    let (_temp_b, vault_b, day_b) =
        worked_vault("bob-cli-override-restart-eq-b");
    run_json(&vault_b, &day_b, &["=x0"]);
    run_json(&vault_b, &day_b, &["=3"]);
    let reset_start_day = fs::read_to_string(&day_b).expect("read day");
    assert_eq!(restart_day, reset_start_day, "{restart_day}");
}

#[test]
fn override_restart_keeps_note_bearing_session() {
    let temp = TempDir::new("bob-cli-override-restart-notes");
    let vault = temp.path().join("vault");
    let day_file = vault.join("day.md");
    write_toggle_task_settings(&vault);
    write_file(
        &day_file,
        concat!(
            "## Pomodoros\n",
            "- [ ] (**0920-0945** [t:: 25m]) — CAPTURE\n",
            "\t- [[bob#^capture-stop]]\n",
            "\t- session note\n",
            "- [ ] () — SASE\n",
        ),
    );
    write_file(
        &vault.join("bob.md"),
        "## Tasks\n\n- [*] #task Stop [created::2026-10-01] ^capture-stop\n",
    );
    let json = run_json(&vault, &day_file, &["=="]);
    assert_eq!(json["kind"], "pomodoro_start");
    assert_eq!(json["pomodoro_start"]["override"]["action"], "restart");
    let day_after = fs::read_to_string(&day_file).expect("read day");
    assert!(
        day_after.contains("(**0935-1000** [t:: 25m]) — CAPTURE")
            && day_after.contains("session note"),
        "{day_after}"
    );
}

#[test]
fn override_restart_preserves_crlf_and_missing_newline() {
    // CRLF survives a restart splice.
    let temp = TempDir::new("bob-cli-override-restart-crlf");
    let vault = temp.path().join("vault");
    let day_file = vault.join("day.md");
    write_toggle_task_settings(&vault);
    let crlf = "## Pomodoros\r\n- [ ] (**0920-0945** [t:: 25m]) — CAPTURE\r\n\t- [[bob#^capture-stop]]\r\n- [ ] () — SASE\r\n";
    write_file(&day_file, crlf);
    write_file(
        &vault.join("bob.md"),
        "## Tasks\n\n- [*] #task Stop [created::2026-10-01] ^capture-stop\n",
    );
    run_json(&vault, &day_file, &["=="]);
    let after = fs::read_to_string(&day_file).expect("read day");
    assert!(after.contains("\r\n"), "{after:?}");
    assert!(after.contains("(**0935-1000** [t:: 25m]) — CAPTURE"));

    // A missing final newline stays missing.
    let temp2 = TempDir::new("bob-cli-override-restart-noeol");
    let vault2 = temp2.path().join("vault");
    let day2 = vault2.join("day.md");
    write_toggle_task_settings(&vault2);
    let noeol = "## Pomodoros\n- [ ] (**0920-0945** [t:: 25m]) — CAPTURE\n- [ ] () — SASE";
    write_file(&day2, noeol);
    write_file(
        &vault2.join("bob.md"),
        "## Tasks\n\n- [*] #task Stop [created::2026-10-01] ^capture-stop\n",
    );
    run_json(&vault2, &day2, &["=="]);
    let after2 = fs::read_to_string(&day2).expect("read day");
    assert!(!after2.ends_with('\n'), "{after2:?}");
}

#[test]
fn override_restart_refuses_multiple_running() {
    let temp = TempDir::new("bob-cli-override-restart-multi");
    let vault = temp.path().join("vault");
    let day_file = vault.join("day.md");
    write_toggle_task_settings(&vault);
    let before = "## Pomodoros\n- [ ] (**0920-0945** [t:: 25m]) — A\n- [ ] (**0920-0945** [t:: 25m]) — B\n";
    write_file(&day_file, before);
    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("-f")
        .arg("json")
        .arg("--")
        .arg("==")
        .env("BOB_DAY_FILE", &day_file)
        .env("BOB_NOW", NOW)
        .output()
        .expect("run restart");
    assert!(!output.status.success(), "{}", format_output(&output));
    assert_eq!(fs::read_to_string(&day_file).expect("read day"), before);
}

#[test]
fn override_restart_dry_run_matches_real() {
    let (_temp, vault, day_file) = worked_vault("bob-cli-override-restart-dry");
    capture_json_dry_run_matches_real(&vault, &day_file, NOW, &["==3"]);
}

#[test]
fn override_restart_batch_rollback() {
    let (_temp, vault, day_file) =
        worked_vault("bob-cli-override-restart-rollback");
    let before = fs::read_to_string(&day_file).expect("read before");
    // A later invalid item rolls back the earlier staged restart.
    let bad = run_with_stdin(
        bob_command()
            .arg("capture")
            .arg("-b")
            .arg(&vault)
            .arg("-f")
            .arg("json")
            .env("BOB_DAY_FILE", &day_file)
            .env("BOB_NOW", NOW),
        "==3\n\n=3 more\n",
    );
    assert_eq!(bad.status.code(), Some(2), "{}", format_output(&bad));
    assert_eq!(
        fs::read_to_string(&day_file).expect("rolled back day"),
        before
    );
}
