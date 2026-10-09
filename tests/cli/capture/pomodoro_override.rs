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

fn run_json_err(
    vault: &std::path::Path,
    day_file: &std::path::Path,
    args: &[&str],
) -> std::process::Output {
    bob_command()
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
        .expect("run capture")
}

#[test]
fn override_swap_kept_ledger_worked_example() {
    let (_temp, vault, day_file) = worked_vault("bob-cli-override-swap-kept");
    let json = run_json(&vault, &day_file, &["==#bugs"]);
    assert_eq!(json["kind"], "pomodoro_start", "{json}");
    assert_eq!(json["text"], "==#bugs", "{json}");
    assert_eq!(json["placement"], "started", "{json}");
    let start = &json["pomodoro_start"];
    assert_eq!(start["pomodoro_name"], "BUGS", "{json}");
    assert_eq!(start["pomodoro_line"], 5, "{json}");
    // Kept ledger: the transferred range describes the session.
    assert_eq!(start["start"], "0920", "{json}");
    assert_eq!(start["end"], "0945", "{json}");
    assert_eq!(start["duration_minutes"], 25, "{json}");
    assert_eq!(start["offset_units"], 0, "{json}");
    assert_eq!(start["time_range"], "(**0920-0945** [t:: 25m])", "{json}");
    assert_eq!(start["created_pomodoro"], false, "{json}");
    assert_eq!(
        json["task_line"], "- [ ] (**0920-0945** [t:: 25m]) — BUGS",
        "{json}"
    );
    let override_json = &start["override"];
    assert_eq!(override_json["action"], "swap", "{json}");
    assert_eq!(override_json["ledger"], "kept", "{json}");
    assert_eq!(
        override_json["previous"]["pomodoro_name"], "CAPTURE",
        "{json}"
    );
    assert_eq!(override_json["previous"]["pomodoro_line"], 5, "{json}");
    assert_eq!(
        override_json["previous"]["time_range"], "0920-0945",
        "{json}"
    );
    assert_eq!(override_json["previous"]["start"], "0920", "{json}");
    assert_eq!(override_json["previous"]["end"], "0945", "{json}");
    assert_eq!(override_json["previous"]["duration_minutes"], 25, "{json}");
    let demoted = &override_json["demoted"];
    assert_eq!(demoted["pomodoro_name"], "CAPTURE", "{json}");
    assert_eq!(demoted["pomodoro_line"], 7, "{json}");
    assert_eq!(demoted["entry_line"], "- [ ] () — CAPTURE", "{json}");
    assert_eq!(demoted["task_links"], 2, "{json}");
    assert_eq!(demoted["has_notes"], false, "{json}");
    // The running session reports `started`; R reports `reset`.
    let blocks = json["pomodoro_blocks"].as_array().expect("blocks");
    let roles: Vec<Vec<String>> = blocks
        .iter()
        .map(|block| {
            block["roles"]
                .as_array()
                .expect("roles")
                .iter()
                .map(|role| role.as_str().unwrap_or("").to_string())
                .collect()
        })
        .collect();
    assert!(roles.iter().any(|entry| entry == &["started"]), "{json}");
    assert!(roles.iter().any(|entry| entry == &["reset"]), "{json}");
    // The day file matches the plan's worked example byte for byte.
    assert_eq!(
        fs::read_to_string(&day_file).expect("read day"),
        concat!(
            "## Pomodoros\n",
            "\n",
            "- [x] (**0830-0855** [t:: 25m]) — PLAN\n",
            "\t- \u{1F345} [[bob#^capture-stop]]\n",
            "- [ ] (**0920-0945** [t:: 25m]) — BUGS\n",
            "\t- [[sase#^fix-flaky]]\n",
            "- [ ] () — CAPTURE\n",
            "\t- [[bob#^capture-stop]]\n",
            "\t- [[bob#^web-capture]]\n",
            "- [ ] () — SASE\n",
        )
    );
}

#[test]
fn override_swap_fresh_timing_worked_example() {
    let (_temp, vault, day_file) = worked_vault("bob-cli-override-swap-fresh");
    let json = run_json(&vault, &day_file, &["==3#bugs"]);
    let start = &json["pomodoro_start"];
    assert_eq!(start["pomodoro_name"], "BUGS", "{json}");
    assert_eq!(start["start"], "0935", "{json}");
    assert_eq!(start["end"], "0950", "{json}");
    assert_eq!(start["duration_minutes"], 15, "{json}");
    assert_eq!(start["time_range"], "(**0935-0950** [t:: 15m])", "{json}");
    assert_eq!(start["override"]["action"], "swap", "{json}");
    assert_eq!(start["override"]["ledger"], "fresh", "{json}");
    assert_eq!(start["override"]["demoted"]["pomodoro_line"], 7, "{json}");
    let day_after = fs::read_to_string(&day_file).expect("read day");
    assert!(
        day_after.contains("- [ ] (**0935-0950** [t:: 15m]) — BUGS"),
        "{day_after}"
    );
    assert!(day_after.contains("- [ ] () — CAPTURE"), "{day_after}");
    // BUGS moved under PLAN; CAPTURE is first future; SASE follows.
    let bugs = day_after.find("— BUGS").expect("bugs line");
    let capture = day_after.find("— CAPTURE").expect("capture line");
    let sase = day_after.find("— SASE").expect("sase line");
    assert!(bugs < capture && capture < sase, "{day_after}");
}

#[test]
fn override_swap_again_and_created_targets() {
    // `==#plan` reopens completed PLAN, taking over the running ledger.
    let (_temp, vault, day_file) = worked_vault("bob-cli-override-swap-again");
    let json = run_json(&vault, &day_file, &["==#plan"]);
    let start = &json["pomodoro_start"];
    assert_eq!(start["pomodoro_name"], "PLAN", "{json}");
    assert_eq!(start["created_pomodoro"], true, "{json}");
    assert_eq!(start["override"]["ledger"], "kept", "{json}");
    assert_eq!(start["time_range"], "(**0920-0945** [t:: 25m])", "{json}");
    let day_after = fs::read_to_string(&day_file).expect("read day");
    assert!(
        day_after.contains("- [ ] (**0920-0945** [t:: 25m]) — PLAN\n"),
        "{day_after}"
    );
    assert!(day_after.contains("- [ ] () — CAPTURE"), "{day_after}");

    // `==#fresh` creates a brand-new session with the kept ledger.
    let (_temp, vault, day_file) =
        worked_vault("bob-cli-override-swap-created");
    let json = run_json(&vault, &day_file, &["==#fresh"]);
    let start = &json["pomodoro_start"];
    assert_eq!(start["pomodoro_name"], "FRESH", "{json}");
    assert_eq!(start["created_pomodoro"], true, "{json}");
    assert_eq!(start["override"]["ledger"], "kept", "{json}");
    let day_after = fs::read_to_string(&day_file).expect("read day");
    assert!(
        day_after.contains("- [ ] (**0920-0945** [t:: 25m]) — FRESH"),
        "{day_after}"
    );
}

#[test]
fn override_swap_naming_running_refuses() {
    let (_temp, vault, day_file) = worked_vault("bob-cli-override-swap-self");
    let before = fs::read_to_string(&day_file).expect("read before");
    let output = run_json_err(&vault, &day_file, &["==#capture"]);
    assert!(!output.status.success(), "{}", format_output(&output));
    assert!(
        format_output(&output).contains(
            "`==#capture` names CAPTURE, which is already running (0920-0945, line 5); restart it with `==` or `==<X>`, or name another Pomodoro to swap in"
        ),
        "{}",
        format_output(&output)
    );
    assert_eq!(fs::read_to_string(&day_file).expect("read day"), before);
}

#[test]
fn override_swap_named_restart_matches_plain_restart() {
    // `==3#capture` resolves to the running session with fresh timing:
    // a restart, byte-identical to `==3` on a note-free ledger.
    let (_temp, vault, day_file) =
        worked_vault("bob-cli-override-swap-restart-a");
    let json = run_json(&vault, &day_file, &["==3#capture"]);
    assert_eq!(
        json["pomodoro_start"]["override"]["action"], "restart",
        "{json}"
    );
    let swap_day = fs::read_to_string(&day_file).expect("read day");

    let (_temp, vault, day_file) =
        worked_vault("bob-cli-override-swap-restart-b");
    run_json(&vault, &day_file, &["==3"]);
    let restart_day = fs::read_to_string(&day_file).expect("read day");
    assert_eq!(swap_day, restart_day);
}

#[test]
fn override_swap_matches_reset_then_start() {
    // Fresh-timing swaps equal `=x0` then `=` on a note-free ledger.
    let (_temp, vault, day_file) = worked_vault("bob-cli-override-swap-eq-a");
    run_json(&vault, &day_file, &["==3#bugs"]);
    let swap_day = fs::read_to_string(&day_file).expect("read day");

    let (_temp, vault, day_file) = worked_vault("bob-cli-override-swap-eq-b");
    run_json(&vault, &day_file, &["=x0"]);
    run_json(&vault, &day_file, &["=3#bugs"]);
    let reset_start_day = fs::read_to_string(&day_file).expect("read day");
    assert_eq!(swap_day, reset_start_day);

    // A kept ledger differs from `=x0 =#bugs` only in the target ledger:
    // the running clock carries over instead of the 25-minute default.
    let (_temp, vault, day_file) = worked_vault("bob-cli-override-swap-eq-c");
    run_json(&vault, &day_file, &["==#bugs"]);
    let kept_day = fs::read_to_string(&day_file).expect("read day");

    let (_temp, vault, day_file) = worked_vault("bob-cli-override-swap-eq-d");
    run_json(&vault, &day_file, &["=x0"]);
    run_json(&vault, &day_file, &["=#bugs"]);
    let default_day = fs::read_to_string(&day_file).expect("read day");
    assert_eq!(
        kept_day.replace(
            "- [ ] (**0920-0945** [t:: 25m]) — BUGS",
            "- [ ] (**0935-1000** [t:: 25m]) — BUGS"
        ),
        default_day
    );
}

#[test]
fn override_swap_drops_target_lineup() {
    // Drops number the target's lineup exactly like `=` starts (BUGS
    // queues the worked example's one Task Link; SASE queues none).
    let (_temp, vault, day_file) = worked_vault("bob-cli-override-swap-drop");
    let json = run_json(&vault, &day_file, &["==#bugs~1"]);
    let start = &json["pomodoro_start"];
    assert_eq!(start["override"]["action"], "swap", "{json}");
    assert_eq!(start["override"]["ledger"], "kept", "{json}");
    assert_eq!(start["tasks"].as_array().map(Vec::len), Some(0), "{json}");
    assert_eq!(start["dropped"].as_array().map(Vec::len), Some(1), "{json}");
    let day_after = fs::read_to_string(&day_file).expect("read day");
    assert!(
        day_after.contains("- [ ] (**0920-0945** [t:: 25m]) — BUGS"),
        "{day_after}"
    );
    assert!(!day_after.contains("[[sase#^fix-flaky]]"), "{day_after}");
    // The demoted session keeps its own lineup: CAPTURE still queues 2.
    assert_eq!(start["override"]["demoted"]["task_links"], 2, "{json}");

    // A created target starts empty, so drops fail before any write.
    let (_temp, vault, day_file) =
        worked_vault("bob-cli-override-swap-drop-created");
    let before = fs::read_to_string(&day_file).expect("read before");
    let output = run_json_err(&vault, &day_file, &["==#fresh~1"]);
    assert!(!output.status.success(), "{}", format_output(&output));
    assert!(
        format_output(&output).contains("no queued Task Links"),
        "{}",
        format_output(&output)
    );
    assert_eq!(fs::read_to_string(&day_file).expect("read day"), before);
}

#[test]
fn override_swap_placeholder_above_stays_first_future() {
    let temp = TempDir::new("bob-cli-override-swap-above");
    let vault = temp.path().join("vault");
    let day_file = vault.join("day.md");
    write_toggle_task_settings(&vault);
    write_file(
        &day_file,
        concat!(
            "## Pomodoros\n",
            "\n",
            "- [x] (**0830-0855** [t:: 25m]) — PLAN\n",
            "- [ ] () — SASE\n",
            "- [ ] (**0920-0945** [t:: 25m]) — CAPTURE\n",
            "\t- [[bob#^capture-stop]]\n",
            "- [ ] () — BUGS\n",
        ),
    );
    write_file(
        &vault.join("bob.md"),
        "## Tasks\n\n- [*] #task Stop [created::2026-10-01] ^capture-stop\n",
    );
    let json = run_json(&vault, &day_file, &["==#bugs"]);
    assert_eq!(
        json["pomodoro_start"]["override"]["action"], "swap",
        "{json}"
    );
    let day_after = fs::read_to_string(&day_file).expect("read day");
    // Demoted CAPTURE still lands first future even though SASE sat above R.
    assert_eq!(
        day_after,
        concat!(
            "## Pomodoros\n",
            "\n",
            "- [x] (**0830-0855** [t:: 25m]) — PLAN\n",
            "- [ ] (**0920-0945** [t:: 25m]) — BUGS\n",
            "- [ ] () — CAPTURE\n",
            "\t- [[bob#^capture-stop]]\n",
            "- [ ] () — SASE\n",
        )
    );
}

#[test]
fn override_swap_unnamed_running() {
    // An unnamed R demotes next to other unnamed `()` placeholders with
    // identical headline text; index-tracked refs stay exact (debug builds
    // panic on wrong refs).
    let temp = TempDir::new("bob-cli-override-swap-unnamed");
    let vault = temp.path().join("vault");
    let day_file = vault.join("day.md");
    write_toggle_task_settings(&vault);
    write_file(
        &day_file,
        concat!(
            "## Pomodoros\n",
            "- [ ] ()\n",
            "- [ ] (**0920-0945** [t:: 25m])\n",
            "\t- [[bob#^capture-stop]]\n",
            "- [ ] () — BUGS\n",
        ),
    );
    write_file(
        &vault.join("bob.md"),
        "## Tasks\n\n- [*] #task Stop [created::2026-10-01] ^capture-stop\n",
    );
    let json = run_json(&vault, &day_file, &["==#bugs"]);
    let start = &json["pomodoro_start"];
    assert_eq!(start["override"]["action"], "swap", "{json}");
    assert_eq!(start["override"]["ledger"], "kept", "{json}");
    assert!(
        start["override"]["previous"].get("pomodoro_name").is_none(),
        "{json}"
    );
    assert!(
        start["override"]["demoted"].get("pomodoro_name").is_none(),
        "{json}"
    );
    assert_eq!(
        start["override"]["demoted"]["entry_line"], "- [ ] ()",
        "{json}"
    );
    let day_after = fs::read_to_string(&day_file).expect("read day");
    assert_eq!(
        day_after,
        concat!(
            "## Pomodoros\n",
            "- [ ] (**0920-0945** [t:: 25m]) — BUGS\n",
            "- [ ] ()\n",
            "\t- [[bob#^capture-stop]]\n",
            "- [ ] ()\n",
        )
    );
}

#[test]
fn override_swap_keeps_annotated_ledger_verbatim() {
    let temp = TempDir::new("bob-cli-override-swap-annotated");
    let vault = temp.path().join("vault");
    let day_file = vault.join("day.md");
    write_toggle_task_settings(&vault);
    write_file(
        &day_file,
        concat!(
            "## Pomodoros\n",
            "- [ ] (**0920-0945** [t:: 25m] [k:: v]) — CAPTURE\n",
            "- [ ] () — BUGS\n",
        ),
    );
    let json = run_json(&vault, &day_file, &["==#bugs"]);
    assert_eq!(
        json["pomodoro_start"]["time_range"],
        "(**0920-0945** [t:: 25m] [k:: v])",
        "{json}"
    );
    let day_after = fs::read_to_string(&day_file).expect("read day");
    assert!(
        day_after.contains("- [ ] (**0920-0945** [t:: 25m] [k:: v]) — BUGS"),
        "{day_after}"
    );
}

#[test]
fn override_swap_chains() {
    // Same-line chains report a batch envelope: one entry per operator.
    let swap_entry = |json: &serde_json::Value| {
        json["captures"]
            .as_array()
            .expect("chain captures")
            .iter()
            .find(|entry| entry["text"] == "==#bugs")
            .cloned()
            .expect("swap entry")
    };
    // `==#bugs +2`: swap, then BUGS extends to 0920-0955.
    let (_temp, vault, day_file) =
        worked_vault("bob-cli-override-swap-chain-after");
    let json = run_json(&vault, &day_file, &["==#bugs +2"]);
    let swap = swap_entry(&json);
    assert_eq!(
        swap["pomodoro_start"]["time_range"], "(**0920-0945** [t:: 25m])",
        "{json}"
    );
    let day_after = fs::read_to_string(&day_file).expect("read day");
    assert!(
        day_after.contains("- [ ] (**0920-0955** [t:: 35m]) — BUGS"),
        "{day_after}"
    );

    // `+2 ==#bugs`: the swap inherits the extended ledger.
    let (_temp, vault, day_file) =
        worked_vault("bob-cli-override-swap-chain-before");
    let json = run_json(&vault, &day_file, &["+2 ==#bugs"]);
    let swap = swap_entry(&json);
    assert_eq!(
        swap["pomodoro_start"]["override"]["ledger"], "kept",
        "{json}"
    );
    assert_eq!(
        swap["pomodoro_start"]["time_range"], "(**0920-0955** [t:: 35m])",
        "{json}"
    );
    let day_after = fs::read_to_string(&day_file).expect("read day");
    assert!(
        day_after.contains("- [ ] (**0920-0955** [t:: 35m]) — BUGS"),
        "{day_after}"
    );
}

#[test]
fn override_swap_human_output() {
    let (_temp, vault, day_file) = worked_vault("bob-cli-override-swap-human");
    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("--dry-run")
        .arg("--")
        .arg("==#bugs")
        .env("BOB_DAY_FILE", &day_file)
        .env("BOB_NOW", NOW)
        .output()
        .expect("run swap human");
    assert_success(&output);
    let out = stdout(&output);
    assert!(out.contains("would swap"), "{out}");
    assert!(
        out.contains("BUGS takes over 0920-0945 (25m) at line 5"),
        "{out}"
    );
    assert!(
        out.contains("CAPTURE → first future at line 7 · keeps 2 Task Links"),
        "{out}"
    );
    assert_stdout_has_no_ansi(&output);

    // Fresh swaps name the new range and what R left behind.
    let (_temp, vault, day_file) =
        worked_vault("bob-cli-override-swap-human-fresh");
    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("--dry-run")
        .arg("--")
        .arg("==3#bugs")
        .env("BOB_DAY_FILE", &day_file)
        .env("BOB_NOW", NOW)
        .output()
        .expect("run fresh swap human");
    assert_success(&output);
    let out = stdout(&output);
    assert!(out.contains("would swap"), "{out}");
    assert!(out.contains("BUGS 0935-0950 (15m) at line 5"), "{out}");
    assert!(
        out.contains(
            "CAPTURE 0920-0945 → first future at line 7 · keeps 2 Task Links"
        ),
        "{out}"
    );
}

#[test]
fn override_swap_reports_notes() {
    let temp = TempDir::new("bob-cli-override-swap-notes");
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
            "- [ ] () — BUGS\n",
        ),
    );
    write_file(
        &vault.join("bob.md"),
        "## Tasks\n\n- [*] #task Stop [created::2026-10-01] ^capture-stop\n",
    );
    // No note gate: the swap demotes a note-bearing session intact.
    let json = run_json(&vault, &day_file, &["==#bugs"]);
    assert_eq!(
        json["pomodoro_start"]["override"]["demoted"]["has_notes"], true,
        "{json}"
    );
    let day_after = fs::read_to_string(&day_file).expect("read day");
    assert!(
        day_after.contains("- [ ] () — CAPTURE")
            && day_after.contains("session note"),
        "{day_after}"
    );
    // A fresh vault for the dry run: the real swap above already runs BUGS.
    let temp = TempDir::new("bob-cli-override-swap-notes-human");
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
            "- [ ] () — BUGS\n",
        ),
    );
    write_file(
        &vault.join("bob.md"),
        "## Tasks\n\n- [*] #task Stop [created::2026-10-01] ^capture-stop\n",
    );
    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("--dry-run")
        .arg("--")
        .arg("==#bugs")
        .env("BOB_DAY_FILE", &day_file)
        .env("BOB_NOW", NOW)
        .output()
        .expect("run swap human");
    assert_success(&output);
    assert!(
        stdout(&output).contains("and its notes"),
        "{}",
        stdout(&output)
    );
}

#[test]
fn override_swap_teaching_errors() {
    let (_temp, vault, day_file) = worked_vault("bob-cli-override-swap-teach");
    let before = fs::read_to_string(&day_file).expect("read before");
    // Unnamed `=<X>` while a session runs teaches the concrete restart
    // spelling of the typed suffix.
    for (item, hint) in [
        ("=", "or capture `==` to restart CAPTURE now"),
        ("=3", "or capture `==3` to restart CAPTURE now"),
    ] {
        let output = run_json_err(&vault, &day_file, &[item]);
        assert!(!output.status.success(), "{}", format_output(&output));
        assert!(
            format_output(&output).contains(hint),
            "{item}: {}",
            format_output(&output)
        );
    }
    // Named `=` while another session runs teaches the swap spelling.
    let output = run_json_err(&vault, &day_file, &["=#bugs"]);
    assert!(!output.status.success(), "{}", format_output(&output));
    assert!(
        format_output(&output).contains(
            "or `==#bugs` to swap it in and return CAPTURE to first future"
        ),
        "{}",
        format_output(&output)
    );
    // Naming the running session teaches the restart spelling.
    let output = run_json_err(&vault, &day_file, &["=#capture"]);
    assert!(!output.status.success(), "{}", format_output(&output));
    assert!(
        format_output(&output).contains("capture `==` to restart it now"),
        "{}",
        format_output(&output)
    );
    // `=x#bugs` teaches the swap without closing.
    let bad = run_with_stdin(
        bob_command()
            .arg("capture")
            .arg("-b")
            .arg(&vault)
            .arg("-f")
            .arg("json")
            .env("BOB_DAY_FILE", &day_file)
            .env("BOB_NOW", NOW),
        "=x#bugs\n",
    );
    assert!(!bad.status.success(), "{}", format_output(&bad));
    assert!(
        format_output(&bad).contains(
            "`==#bugs` swaps it in without closing the running session"
        ),
        "{}",
        format_output(&bad)
    );
    assert_eq!(fs::read_to_string(&day_file).expect("read day"), before);
}

#[test]
fn override_swap_did_you_mean_spells_override() {
    let (_temp, vault, day_file) =
        worked_vault("bob-cli-override-swap-suggest");
    let json = run_json(&vault, &day_file, &["==#bugz"]);
    let start = &json["pomodoro_start"];
    assert_eq!(start["created_pomodoro"], true, "{json}");
    assert_eq!(start["override"]["action"], "swap", "{json}");
    let warnings = json["warnings"].as_array().expect("warnings");
    assert!(
        warnings.iter().any(|warning| {
            warning
                .as_str()
                .is_some_and(|text| text.contains("use `==#bugs`"))
        }),
        "{json}"
    );
}

#[test]
fn override_swap_strict_never_refuses_created_target() {
    let temp = TempDir::new("bob-cli-override-swap-strict");
    let vault = temp.path().join("vault");
    let day_file = vault.join("day.md");
    let config = temp.path().join("config.yml");
    write_toggle_task_settings(&vault);
    write_file(
        &day_file,
        concat!(
            "## Pomodoros\n",
            "- [ ] (**0920-0945** [t:: 25m]) — GOALS\n",
            "  - [[dev#^aaa]]\n",
            "- [ ] () — DECKS\n",
            "  - [[dev#^bbb]]\n",
            "- [ ] () — BOB\n",
            "  - [[dev#^ccc]]\n",
        ),
    );
    write_file(
        &vault.join("dev.md"),
        "## Tasks\n\n- [ ] #task Aaa [created::2026-10-01] ^aaa\n- [ ] #task Bbb [created::2026-10-01] ^bbb\n- [ ] #task Ccc [created::2026-10-01] ^ccc\n",
    );
    write_file(&config, "plan:\n  strict: true\n");
    // A swap that creates a fourth theme past the cap still succeeds.
    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("-f")
        .arg("json")
        .arg("--")
        .arg("==#fresh")
        .env("BOB_DAY_FILE", &day_file)
        .env("BOB_NOW", NOW)
        .env("BOB_CONFIG_FILE", &config)
        .output()
        .expect("run swap strict");
    assert_success(&output);
    let json: serde_json::Value =
        serde_json::from_str(stdout(&output).trim()).expect("capture JSON");
    assert_eq!(json["ok"], true, "{json}");
    assert_eq!(json["pomodoro_start"]["created_pomodoro"], true, "{json}");
    assert_eq!(
        json["pomodoro_start"]["override"]["action"], "swap",
        "{json}"
    );
    let warnings = json["plan_budget"]["warnings"]
        .as_array()
        .expect("budget warnings");
    assert!(
        warnings
            .iter()
            .any(|warning| warning["code"] == "plan_theme_cap_exceeded"),
        "{json}"
    );
}

#[test]
fn override_swap_dry_run_matches_real() {
    let (_temp, vault, day_file) = worked_vault("bob-cli-override-swap-dry");
    capture_json_dry_run_matches_real(&vault, &day_file, NOW, &["==#bugs"]);
    let (_temp, vault, day_file) =
        worked_vault("bob-cli-override-swap-dry-fresh");
    capture_json_dry_run_matches_real(&vault, &day_file, NOW, &["==3#bugs"]);
}

#[test]
fn override_swap_batch_rollback() {
    let (_temp, vault, day_file) =
        worked_vault("bob-cli-override-swap-rollback");
    let before = fs::read_to_string(&day_file).expect("read before");
    let bad = run_with_stdin(
        bob_command()
            .arg("capture")
            .arg("-b")
            .arg(&vault)
            .arg("-f")
            .arg("json")
            .env("BOB_DAY_FILE", &day_file)
            .env("BOB_NOW", NOW),
        "==#bugs\n\n=3 more\n",
    );
    assert_eq!(bad.status.code(), Some(2), "{}", format_output(&bad));
    assert_eq!(
        fs::read_to_string(&day_file).expect("rolled back day"),
        before
    );
}

#[test]
fn override_swap_preserves_crlf_and_missing_newline() {
    let temp = TempDir::new("bob-cli-override-swap-crlf");
    let vault = temp.path().join("vault");
    let day_file = vault.join("day.md");
    write_toggle_task_settings(&vault);
    let crlf = "## Pomodoros\r\n- [ ] (**0920-0945** [t:: 25m]) — CAPTURE\r\n\t- [[bob#^capture-stop]]\r\n- [ ] () — BUGS\r\n";
    write_file(&day_file, crlf);
    write_file(
        &vault.join("bob.md"),
        "## Tasks\n\n- [*] #task Stop [created::2026-10-01] ^capture-stop\n",
    );
    run_json(&vault, &day_file, &["==#bugs"]);
    let after = fs::read_to_string(&day_file).expect("read day");
    assert!(after.contains("\r\n"), "{after:?}");
    assert!(after.contains("- [ ] (**0920-0945** [t:: 25m]) — BUGS"));

    let temp2 = TempDir::new("bob-cli-override-swap-noeol");
    let vault2 = temp2.path().join("vault");
    let day2 = vault2.join("day.md");
    write_toggle_task_settings(&vault2);
    let noeol = "## Pomodoros\n- [ ] (**0920-0945** [t:: 25m]) — CAPTURE\n- [ ] () — BUGS";
    write_file(&day2, noeol);
    write_file(
        &vault2.join("bob.md"),
        "## Tasks\n\n- [*] #task Stop [created::2026-10-01] ^capture-stop\n",
    );
    run_json(&vault2, &day2, &["==#bugs"]);
    let after2 = fs::read_to_string(&day2).expect("read day");
    assert!(!after2.ends_with('\n'), "{after2:?}");
    assert!(after2.contains("- [ ] (**0920-0945** [t:: 25m]) — BUGS"));
}
