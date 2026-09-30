//! Capture pomodoro start tests.

use crate::support::*;
use std::fs;

#[test]
fn capture_pomodoro_start_default_and_explicit_durations() {
    let cases = [
        ("default-empty", "=", "0905", "0930", 25, 0),
        ("explicit-3", "=3", "0905", "0920", 15, 0),
        ("offset-only", "=-2", "0855", "0920", 25, 2),
        ("bare-dash", "=-", "0900", "0925", 25, 1),
        ("dur-offset", "=3-", "0900", "0915", 15, 1),
        ("dur-offset-nums", "=2-1", "0900", "0910", 10, 1),
    ];
    for (name, suffix, expect_start, expect_end, expect_dur, expect_off) in
        cases
    {
        let temp = TempDir::new(&format!("bob-cli-capture-start-{name}"));
        let vault = temp.path().join("vault");
        let target = vault.join("sase.md");
        let day_file = vault.join("day.md");
        write_file(&target, "# S\n## Tasks\n");
        write_file(&day_file, "## Pomodoros\n- [ ] ()\n");
        let marker = format!("@sase:task-{name}{suffix}");
        let output = bob_command()
            .arg("capture")
            .arg("-b")
            .arg(&vault)
            .arg("-f")
            .arg("json")
            .arg("Work")
            .arg(&marker)
            .env("BOB_DAY_FILE", &day_file)
            .env("BOB_NOW", "2026-07-10 09:02:00")
            .output()
            .expect("run start capture");
        assert_success(&output);
        let json: serde_json::Value =
            serde_json::from_str(stdout(&output).trim()).expect("start JSON");
        assert_eq!(json["kind"], "pomodoro_task", "{name}");
        let start = &json["pomodoro_start"];
        assert_eq!(start["start"], expect_start, "{name}: {json}");
        assert_eq!(start["end"], expect_end, "{name}: {json}");
        assert_eq!(start["duration_minutes"], expect_dur, "{name}");
        assert_eq!(start["offset_units"], expect_off, "{name}");
        assert_eq!(start["created_pomodoro"], false, "{name}");
        let day_after = fs::read_to_string(&day_file).expect("read day");
        assert!(
            day_after.contains(&format!(
                "(**{expect_start}-{expect_end}** [t:: {expect_dur}m])"
            )),
            "{name}: {day_after}"
        );
        assert!(
            day_after.contains(&format!("[[sase#^task-{name}]]")),
            "{name}: {day_after}"
        );
    }
}

#[test]
fn capture_pomodoro_start_named_existing_and_new() {
    let temp = TempDir::new("bob-cli-capture-start-named");
    let vault = temp.path().join("vault");
    let target = vault.join("sase.md");
    let day_file = vault.join("day.md");
    write_file(&target, "# S\n## Tasks\n");
    write_file(
        &day_file,
        "## Pomodoros\n- [ ] () — BUGS\n- [ ] () — FOCUS\n",
    );
    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("-f")
        .arg("json")
        .arg("Work")
        .arg("@sase:named1#bugs=")
        .env("BOB_DAY_FILE", &day_file)
        .env("BOB_NOW", "2026-07-10 09:02:00")
        .output()
        .expect("run named existing start");
    assert_success(&output);
    let json: serde_json::Value =
        serde_json::from_str(stdout(&output).trim()).expect("named JSON");
    assert_eq!(json["pomodoro_start"]["pomodoro_name"], "BUGS");
    assert_eq!(json["pomodoro_start"]["created_pomodoro"], false);
    let day_after = fs::read_to_string(&day_file).expect("read day");
    assert!(
        day_after.contains("(**0905-0930** [t:: 25m]) — BUGS"),
        "{day_after}"
    );

    let temp2 = TempDir::new("bob-cli-capture-start-named-new");
    let vault2 = temp2.path().join("vault");
    let target2 = vault2.join("sase.md");
    let day2 = vault2.join("day.md");
    write_file(&target2, "# S\n## Tasks\n");
    write_file(
        &day2,
        "## Pomodoros\n- [x] (0900-0925) Done\n- [ ] () — BUGS\n",
    );
    let output2 = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault2)
        .arg("-f")
        .arg("json")
        .arg("Work")
        .arg("@sase:named2#deep-work=")
        .env("BOB_DAY_FILE", &day2)
        .env("BOB_NOW", "2026-07-10 09:02:00")
        .output()
        .expect("run named new start");
    assert_success(&output2);
    let json2: serde_json::Value =
        serde_json::from_str(stdout(&output2).trim()).expect("named new JSON");
    assert_eq!(json2["pomodoro_start"]["pomodoro_name"], "DEEP-WORK");
    assert_eq!(json2["pomodoro_start"]["created_pomodoro"], true);
    let day2_after = fs::read_to_string(&day2).expect("read day2");
    assert!(
        day2_after.contains("(**0905-0930** [t:: 25m]) — DEEP-WORK"),
        "{day2_after}"
    );
}

#[test]
fn capture_pomodoro_start_creates_unnamed_when_no_placeholder() {
    let temp = TempDir::new("bob-cli-capture-start-no-placeholder");
    let vault = temp.path().join("vault");
    let target = vault.join("sase.md");
    let day_file = vault.join("day.md");
    write_file(&target, "# S\n## Tasks\n");
    write_file(&day_file, "## Pomodoros\n- [x] (0900-0925) Done\n");
    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("-f")
        .arg("json")
        .arg("Work")
        .arg("@sase:created=")
        .env("BOB_DAY_FILE", &day_file)
        .env("BOB_NOW", "2026-07-10 09:02:00")
        .output()
        .expect("run unnamed creation");
    assert_success(&output);
    let json: serde_json::Value =
        serde_json::from_str(stdout(&output).trim()).expect("creation JSON");
    assert_eq!(json["pomodoro_start"]["created_pomodoro"], true);
    assert!(
        json["pomodoro_start"].get("pomodoro_name").is_none(),
        "{json}"
    );
    let day_after = fs::read_to_string(&day_file).expect("read day");
    assert_eq!(
        day_after,
        "## Pomodoros\n- [x] (0900-0925) Done\n- [ ] (**0905-0930** [t:: 25m])\n  - [[sase#^created]]\n",
        "{day_after}"
    );

    let temp2 = TempDir::new("bob-cli-capture-start-empty-section");
    let vault2 = temp2.path().join("vault");
    let day2 = vault2.join("day.md");
    write_file(&vault2.join("sase.md"), "# S\n## Tasks\n");
    write_file(&day2, "## Pomodoros\n");
    let output2 = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault2)
        .arg("-f")
        .arg("json")
        .arg("Work")
        .arg("@sase:empty=")
        .env("BOB_DAY_FILE", &day2)
        .env("BOB_NOW", "2026-07-10 09:02:00")
        .output()
        .expect("run empty section creation");
    assert_success(&output2);
    let day2_after = fs::read_to_string(&day2).expect("read day2");
    assert!(
        day2_after.contains("(**0905-0930** [t:: 25m])"),
        "{day2_after}"
    );
}

#[test]
fn capture_pomodoro_start_new_entry_uses_first_open_placement() {
    let day_before =
        "# Day\n\n## Pomodoros\n\n- [ ] () — GTD\n\t- [[#^gtd]]\n\n## Tasks\n";
    let temp = TempDir::new("bob-cli-capture-start-named-new-placement");
    let vault = temp.path().join("vault");
    let target = vault.join("sase.md");
    let day_file = vault.join("day.md");
    write_file(&target, "# S\n## Tasks\n");
    write_file(&day_file, day_before);
    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("-f")
        .arg("json")
        .arg("Relaunch")
        .arg("@sase:relaunch#relaunch=10")
        .env("BOB_DAY_FILE", &day_file)
        .env("BOB_NOW", "2026-09-27 07:12:00")
        .output()
        .expect("run named-new start");
    assert_success(&output);
    let json: serde_json::Value =
        serde_json::from_str(stdout(&output).trim()).expect("named-new JSON");
    assert_eq!(json["pomodoro_start"]["created_pomodoro"], true);
    assert_eq!(json["pomodoro_start"]["pomodoro_line"], 5);
    assert_eq!(
        fs::read_to_string(&day_file).expect("read day"),
        "# Day\n\n## Pomodoros\n\n- [ ] (**0715-0805** [t:: 50m]) — RELAUNCH\n\t- [[sase#^relaunch]]\n- [ ] () — GTD\n\t- [[#^gtd]]\n\n## Tasks\n",
    );

    let unnamed_before =
        "# Day\n\n## Pomodoros\n\n- [ ] Review inbox\n\t- [[#^gtd]]\n\n## Tasks\n";
    let temp2 = TempDir::new("bob-cli-capture-start-unnamed-placement");
    let vault2 = temp2.path().join("vault");
    write_file(&vault2.join("sase.md"), "# S\n## Tasks\n");
    let day2 = vault2.join("day.md");
    write_file(&day2, unnamed_before);
    let output2 = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault2)
        .arg("-f")
        .arg("json")
        .arg("Work")
        .arg("@sase:u2=")
        .env("BOB_DAY_FILE", &day2)
        .env("BOB_NOW", "2026-09-27 07:12:00")
        .output()
        .expect("run unnamed start");
    assert_success(&output2);
    let json2: serde_json::Value =
        serde_json::from_str(stdout(&output2).trim()).expect("unnamed JSON");
    assert_eq!(json2["pomodoro_start"]["created_pomodoro"], true);
    assert_eq!(json2["pomodoro_start"]["pomodoro_line"], 5);
    assert_eq!(
        fs::read_to_string(&day2).expect("read day2"),
        "# Day\n\n## Pomodoros\n\n- [ ] (**0715-0740** [t:: 25m])\n\t- [[sase#^u2]]\n- [ ] Review inbox\n\t- [[#^gtd]]\n\n## Tasks\n",
    );

    let crlf_before =
        "# Day\r\n\r\n## Pomodoros\r\n\r\n- [ ] () — GTD\r\n\t- [[#^gtd]]\r\n\r\n## Tasks\r\n";
    let temp3 = TempDir::new("bob-cli-capture-start-crlf-placement");
    let vault3 = temp3.path().join("vault");
    write_file(&vault3.join("sase.md"), "# S\n## Tasks\n");
    let day3 = vault3.join("day.md");
    write_file(&day3, crlf_before);
    let output3 = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault3)
        .arg("-f")
        .arg("json")
        .arg("Relaunch")
        .arg("@sase:relaunch#relaunch=10")
        .env("BOB_DAY_FILE", &day3)
        .env("BOB_NOW", "2026-09-27 07:12:00")
        .output()
        .expect("run CRLF start");
    assert_success(&output3);
    assert_eq!(
        fs::read_to_string(&day3).expect("read day3"),
        "# Day\r\n\r\n## Pomodoros\r\n\r\n- [ ] (**0715-0805** [t:: 50m]) — RELAUNCH\r\n\t- [[sase#^relaunch]]\r\n- [ ] () — GTD\r\n\t- [[#^gtd]]\r\n\r\n## Tasks\r\n",
    );

    let temp4 = TempDir::new("bob-cli-capture-named-create-placement");
    let vault4 = temp4.path().join("vault");
    write_file(&vault4.join("sase.md"), "# S\n## Tasks\n");
    let day4 = vault4.join("day.md");
    write_file(&day4, day_before);
    let output4 = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault4)
        .arg("-f")
        .arg("json")
        .arg("Work")
        .arg("@sase:n1#relaunch")
        .env("BOB_DAY_FILE", &day4)
        .env("BOB_NOW", "2026-09-27 07:12:00")
        .output()
        .expect("run non-start named creation");
    assert_success(&output4);
    assert_eq!(
        fs::read_to_string(&day4).expect("read day4"),
        "# Day\n\n## Pomodoros\n\n- [ ] () — RELAUNCH\n\t- [[sase#^n1]]\n- [ ] () — GTD\n\t- [[#^gtd]]\n\n## Tasks\n",
    );
}

#[test]
fn capture_pomodoro_start_active_and_ambiguous_fail_atomically() {
    for (name, day_before) in [
        (
            "active-single",
            "## Pomodoros\n- [ ] (**1330-1400** [t:: 30m]) Current\n- [ ] () — BUGS\n",
        ),
        (
            "ambiguous",
            "## Pomodoros\n- [ ] (**0900-0925** [t:: 25m]) One\n- [ ] (**0930-0955** [t:: 25m]) Two\n",
        ),
    ] {
        let temp = TempDir::new(&format!("bob-cli-capture-start-{name}"));
        let vault = temp.path().join("vault");
        let target = vault.join("sase.md");
        let day_file = vault.join("day.md");
        let target_before = "# S\n## Tasks\n";
        write_file(&target, target_before);
        write_file(&day_file, day_before);
        let output = bob_command()
            .arg("capture")
            .arg("-b")
            .arg(&vault)
            .arg("-f")
            .arg("json")
            .arg("Work")
            .arg("@sase:new-id=")
            .env("BOB_DAY_FILE", &day_file)
            .env("BOB_NOW", "2026-07-10 13:40:00")
            .output()
            .expect("run failing start");
        assert_eq!(
            output.status.code(),
            Some(1),
            "{name}: {}",
            format_output(&output)
        );
        let json: serde_json::Value =
            serde_json::from_str(stdout(&output).trim()).expect("failure JSON");
        assert!(
            json["error"]
                .as_str()
                .is_some_and(|e| e.contains("finish the current Pomodoro first")),
            "{name}: {json}"
        );
        assert_eq!(
            fs::read_to_string(&target).expect("untouched target"),
            target_before,
            "{name}"
        );
        assert_eq!(
            fs::read_to_string(&day_file).expect("untouched day"),
            day_before,
            "{name}"
        );
    }
}

#[test]
fn capture_pomodoro_start_rejects_invalid_and_conflicting_syntax() {
    let temp = TempDir::new("bob-cli-capture-start-invalid");
    let vault = temp.path().join("vault");
    fs::create_dir_all(&vault).expect("create vault");
    for marker in ["@sase:b1==3", "@sase:b1=a", "@sase:b1=3--", "@sase:b1=3-2-"]
    {
        let output = bob_command()
            .arg("capture")
            .arg("-b")
            .arg(&vault)
            .arg("Work")
            .arg(marker)
            .output()
            .expect("run invalid start");
        assert_eq!(
            output.status.code(),
            Some(2),
            "{marker}: {}",
            format_output(&output)
        );
        assert_eq!(
            fs::read_dir(&vault).expect("read vault").count(),
            0,
            "{marker}"
        );
    }
    let day_file = vault.join("day.md");
    write_file(&day_file, "## Pomodoros\n- [ ] ()\n");
    write_file(&vault.join("sase.md"), "# S\n## Tasks\n");
    for (marker, extra, expected) in [
        ("@sase:b1=3", "s:1", "cannot be combined with `s:<N>`"),
        ("@sase:b1=3", "p:2", "cannot be combined with `p:<N>`"),
    ] {
        let output = bob_command()
            .arg("capture")
            .arg("-b")
            .arg(&vault)
            .arg("Work")
            .arg(marker)
            .arg(extra)
            .env("BOB_DAY_FILE", &day_file)
            .output()
            .expect("run conflicting start");
        assert_eq!(
            output.status.code(),
            Some(2),
            "{marker} {extra}: {}",
            format_output(&output)
        );
        assert!(
            stderr(&output).contains(expected),
            "{}",
            format_output(&output)
        );
    }
    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("Work")
        .arg("@sase:b1+=3")
        .env("BOB_DAY_FILE", &day_file)
        .output()
        .expect("run project-note start");
    assert_eq!(output.status.code(), Some(2), "{}", format_output(&output));
}

#[test]
fn capture_pomodoro_start_midnight_wrap_and_bob_now() {
    let temp = TempDir::new("bob-cli-capture-start-midnight");
    let vault = temp.path().join("vault");
    let day_file = vault.join("day.md");
    write_file(&vault.join("sase.md"), "# S\n## Tasks\n");
    write_file(&day_file, "## Pomodoros\n- [ ] ()\n");
    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("-f")
        .arg("json")
        .arg("Late")
        .arg("@sase:late=")
        .env("BOB_DAY_FILE", &day_file)
        .env("BOB_NOW", "2026-07-10 23:58:00")
        .output()
        .expect("run midnight start");
    assert_success(&output);
    let json: serde_json::Value =
        serde_json::from_str(stdout(&output).trim()).expect("midnight JSON");
    assert_eq!(json["pomodoro_start"]["start"], "0000");
    assert_eq!(json["pomodoro_start"]["end"], "0025");
    let day_after = fs::read_to_string(&day_file).expect("read day");
    assert!(
        day_after.contains("(**0000-0025** [t:: 25m])"),
        "{day_after}"
    );

    let temp2 = TempDir::new("bob-cli-capture-start-seconds-ignored");
    let vault2 = temp2.path().join("vault");
    let day2 = vault2.join("day.md");
    write_file(&vault2.join("sase.md"), "# S\n## Tasks\n");
    write_file(&day2, "## Pomodoros\n- [ ] ()\n");
    let output2 = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault2)
        .arg("-f")
        .arg("json")
        .arg("Work")
        .arg("@sase:sec=3")
        .env("BOB_DAY_FILE", &day2)
        .env("BOB_NOW", "2026-07-10 09:02:45")
        .output()
        .expect("run seconds-ignored start");
    assert_success(&output2);
    let json2: serde_json::Value =
        serde_json::from_str(stdout(&output2).trim()).expect("seconds JSON");
    assert_eq!(json2["pomodoro_start"]["start"], "0905");
    assert_eq!(json2["pomodoro_start"]["end"], "0920");
}

#[test]
fn capture_pomodoro_start_dry_run_and_batch_rollback() {
    let temp = TempDir::new("bob-cli-capture-start-dry-run");
    let vault = temp.path().join("vault");
    let target = vault.join("sase.md");
    let day_file = vault.join("day.md");
    let target_before = "# S\n## Tasks\n";
    let day_before = "## Pomodoros\n- [ ] ()\n";
    write_file(&target, target_before);
    write_file(&day_file, day_before);
    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("-d")
        .arg("-f")
        .arg("json")
        .arg("Work")
        .arg("@sase:dry1=")
        .env("BOB_DAY_FILE", &day_file)
        .env("BOB_NOW", "2026-07-10 09:02:00")
        .output()
        .expect("run dry-run start");
    assert_success(&output);
    let json: serde_json::Value =
        serde_json::from_str(stdout(&output).trim()).expect("dry-run JSON");
    assert_eq!(json["dry_run"], true);
    assert_eq!(json["pomodoro_start"]["start"], "0905");
    assert_eq!(
        fs::read_to_string(&target).expect("untouched target"),
        target_before
    );
    assert_eq!(
        fs::read_to_string(&day_file).expect("untouched day"),
        day_before
    );

    let temp2 = TempDir::new("bob-cli-capture-start-batch-order");
    let vault2 = temp2.path().join("vault");
    let target2 = vault2.join("sase.md");
    let day2 = vault2.join("day.md");
    write_file(&target2, target_before);
    write_file(&day2, day_before);
    let output2 = run_with_stdin(
        bob_command()
            .arg("capture")
            .arg("-b")
            .arg(&vault2)
            .arg("-f")
            .arg("json")
            .env("BOB_DAY_FILE", &day2)
            .env("BOB_NOW", "2026-07-10 09:02:00"),
        "First @sase:a1=\n\nSecond @sase:a2=\n",
    );
    assert_eq!(
        output2.status.code(),
        Some(1),
        "{}",
        format_output(&output2)
    );
    assert_eq!(
        fs::read_to_string(&target2).expect("rolled back target"),
        target_before
    );
    assert_eq!(
        fs::read_to_string(&day2).expect("rolled back day"),
        day_before
    );

    let temp3 = TempDir::new("bob-cli-capture-start-mixed-rollback");
    let vault3 = temp3.path().join("vault");
    let target3 = vault3.join("sase.md");
    let day3 = vault3.join("day.md");
    write_file(&target3, "- [ ] #task Existing ^dup\n");
    write_file(&day3, day_before);
    let output3 = run_with_stdin(
        bob_command()
            .arg("capture")
            .arg("-b")
            .arg(&vault3)
            .arg("-f")
            .arg("json")
            .env("BOB_DAY_FILE", &day3)
            .env("BOB_NOW", "2026-07-10 09:02:00"),
        "Good @sase:good1=\n\nBad @sase:dup\n",
    );
    assert_eq!(
        output3.status.code(),
        Some(1),
        "{}",
        format_output(&output3)
    );
    assert_eq!(
        fs::read_to_string(&day3).expect("rolled back day3"),
        day_before
    );
}

#[test]
fn capture_pomodoro_start_moves_to_current_slot_screenshot_repro() {
    fn repro_vault(
        name: &str,
    ) -> (TempDir, std::path::PathBuf, std::path::PathBuf) {
        let temp = TempDir::new(name);
        let vault = temp.path().join("vault");
        let day_file = vault.join("day.md");
        write_toggle_task_settings(&vault);
        write_file(
            &vault.join("sase_goals.md"),
            "- [*] #task Research goals ^research\n",
        );
        write_file(
            &day_file,
            concat!(
                "## Pomodoros\n",
                "- [x] (**0715-0805** [t:: 50m]) — RELAUNCH\n",
                "\t- [[sase#^relaunch-260927]]\n",
                "- [x] (**1010-1030** [t:: 20m]) — SUBTABS\n",
                "\t- \u{1F345} [[sase#^agents-sub-tabs]]\n",
                "\t\t- Started 0t4!\n",
                "- [ ] () — SUBTABS\n",
                "\t- [[sase#^agents-sub-tabs]]\n",
                "- [ ] () — GTD\n",
                "\t- [[#^gtd]]\n",
                "- [ ] () — GOALS\n",
                "\t- [[sase_goals#^research]]\n",
                "- [ ] () — SASE\n",
                "\t- [[sase#^recovery-panel]]\n",
            ),
        );
        (temp, vault, day_file)
    }
    fn expected_after() -> String {
        concat!(
            "## Pomodoros\n",
            "- [x] (**0715-0805** [t:: 50m]) — RELAUNCH\n",
            "\t- [[sase#^relaunch-260927]]\n",
            "- [x] (**1010-1030** [t:: 20m]) — SUBTABS\n",
            "\t- \u{1F345} [[sase#^agents-sub-tabs]]\n",
            "\t\t- Started 0t4!\n",
            "- [ ] (**1315-1340** [t:: 25m]) — GOALS\n",
            "\t- [[sase_goals#^research]]\n",
            "- [ ] () — SUBTABS\n",
            "\t- [[sase#^agents-sub-tabs]]\n",
            "- [ ] () — GTD\n",
            "\t- [[#^gtd]]\n",
            "- [ ] () — SASE\n",
            "\t- [[sase#^recovery-panel]]\n",
        )
        .to_string()
    }
    fn run_item(
        vault: &std::path::Path,
        day_file: &std::path::Path,
        item: &str,
    ) -> serde_json::Value {
        let output = bob_command()
            .arg("capture")
            .arg("-b")
            .arg(vault)
            .arg("-f")
            .arg("json")
            .arg("--")
            .arg(item)
            .env("BOB_DAY_FILE", day_file)
            .env("BOB_NOW", "2026-09-27 13:12:00")
            .output()
            .expect("run repro start");
        assert_success(&output);
        serde_json::from_str(stdout(&output).trim()).expect("repro JSON")
    }

    let (_t, vault, day_file) = repro_vault("bob-cli-start-slot-repro");
    let day_before = fs::read_to_string(&day_file).expect("read before");
    let target_before =
        fs::read_to_string(vault.join("sase_goals.md")).expect("read target");
    let json = run_item(&vault, &day_file, "^sase_goals:research=5");
    assert_eq!(json["pomodoro_link_action"], "already_current");
    assert_eq!(json["pomodoro_link_source"]["line"], 11);
    assert_eq!(json["pomodoro_link_destination"]["line"], 7);
    assert_eq!(json["pomodoro_start"]["pomodoro_line"], 7);
    assert_eq!(
        fs::read_to_string(&day_file).expect("read day"),
        expected_after()
    );

    let (_t, vault, day_file) = repro_vault("bob-cli-start-slot-at");
    let json = run_item(&vault, &day_file, "@sase_goals:research=5");
    assert_eq!(json["pomodoro_link_action"], "already_current");
    assert_eq!(json["pomodoro_link_destination"]["line"], 7);
    assert_eq!(
        fs::read_to_string(&day_file).expect("read day"),
        expected_after()
    );

    let (_t, vault, day_file) = repro_vault("bob-cli-start-slot-named");
    let json = run_item(&vault, &day_file, "^sase_goals:research#goals=5");
    assert_eq!(json["pomodoro_link_action"], "already_current");
    assert_eq!(json["pomodoro_link_destination"]["line"], 7);
    assert_eq!(
        fs::read_to_string(&day_file).expect("read day"),
        expected_after()
    );

    let (_t, vault, day_file) = repro_vault("bob-cli-start-slot-dry");
    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("-d")
        .arg("-f")
        .arg("json")
        .arg("--")
        .arg("^sase_goals:research=5")
        .env("BOB_DAY_FILE", &day_file)
        .env("BOB_NOW", "2026-09-27 13:12:00")
        .output()
        .expect("dry run");
    assert_success(&output);
    let json: serde_json::Value =
        serde_json::from_str(stdout(&output).trim()).expect("dry JSON");
    assert_eq!(json["dry_run"], true);
    assert_eq!(json["pomodoro_link_destination"]["line"], 7);
    assert_eq!(json["pomodoro_start"]["pomodoro_line"], 7);
    assert_eq!(
        fs::read_to_string(&day_file).expect("untouched day"),
        day_before
    );
    assert_eq!(
        fs::read_to_string(vault.join("sase_goals.md")).expect("untouched"),
        target_before
    );
}

#[test]
fn capture_pomodoro_start_moves_named_destination_before_link_move() {
    let temp = TempDir::new("bob-cli-start-slot-q-ne-dest");
    let vault = temp.path().join("vault");
    let day_file = vault.join("day.md");
    write_toggle_task_settings(&vault);
    write_file(
        &vault.join("sase_goals.md"),
        "- [*] #task Research goals ^research\n",
    );
    write_file(
        &day_file,
        concat!(
            "## Pomodoros\n",
            "- [x] (**0800-0825** [t:: 25m]) — DONE\n",
            "- [ ] () — GTD\n",
            "\t- [[sase_goals#^research]]\n",
            "\t\t- notes\n",
            "- [ ] () — GOALS\n",
            "- [ ] () — AFTER\n",
        ),
    );
    let day_before = fs::read_to_string(&day_file).expect("read day before");
    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("-f")
        .arg("json")
        .arg("--")
        .arg("^sase_goals:research#goals=")
        .env("BOB_DAY_FILE", &day_file)
        .env("BOB_NOW", "2026-07-10 09:02:00")
        .output()
        .expect("run named move");
    assert_success(&output);
    let json: serde_json::Value =
        serde_json::from_str(stdout(&output).trim()).expect("move JSON");
    assert_eq!(json["pomodoro_link_action"], "moved");
    assert_eq!(json["pomodoro_link_source"]["line"], 3);
    assert_eq!(json["pomodoro_link_source"]["name"], "GTD");
    assert_eq!(json["pomodoro_link_destination"]["name"], "GOALS");
    assert_eq!(
        json["pomodoro_link_destination"]["line"],
        json["pomodoro_start"]["pomodoro_line"]
    );
    let day_after = fs::read_to_string(&day_file).expect("read day");
    assert_eq!(
        day_after,
        concat!(
            "## Pomodoros\n",
            "- [x] (**0800-0825** [t:: 25m]) — DONE\n",
            "- [ ] (**0905-0930** [t:: 25m]) — GOALS\n",
            "\t- [[sase_goals#^research]]\n",
            "\t\t- notes\n",
            "- [ ] () — GTD\n",
            "- [ ] () — AFTER\n",
        )
    );
    assert_pomodoro_blocks_cover_changes(&day_before, &day_after, &json);
}

#[test]
fn capture_pomodoro_start_named_existing_moves_first() {
    let temp = TempDir::new("bob-cli-start-slot-named-first");
    let vault = temp.path().join("vault");
    let day_file = vault.join("day.md");
    write_file(&vault.join("sase.md"), "# S\n## Tasks\n");
    write_file(
        &day_file,
        "## Pomodoros\n- [ ] () — BUGS\n- [ ] () — FOCUS\n",
    );
    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("-f")
        .arg("json")
        .arg("Work")
        .arg("@sase:w1#focus=")
        .env("BOB_DAY_FILE", &day_file)
        .env("BOB_NOW", "2026-07-10 09:02:00")
        .output()
        .expect("run named first");
    assert_success(&output);
    let json: serde_json::Value =
        serde_json::from_str(stdout(&output).trim()).expect("named JSON");
    assert_eq!(json["pomodoro_start"]["pomodoro_line"], 2);
    assert_eq!(json["pomodoro_start"]["pomodoro_name"], "FOCUS");
    assert_eq!(
        fs::read_to_string(&day_file).expect("read day"),
        concat!(
            "## Pomodoros\n",
            "- [ ] (**0905-0930** [t:: 25m]) — FOCUS\n",
            "  - [[sase#^w1]]\n",
            "- [ ] () — BUGS\n",
        )
    );
}

#[test]
fn capture_pomodoro_start_unnamed_moves_between_done_and_review() {
    let temp = TempDir::new("bob-cli-start-slot-unnamed-between");
    let vault = temp.path().join("vault");
    let day_file = vault.join("day.md");
    write_file(&vault.join("sase.md"), "# S\n## Tasks\n");
    write_file(
        &day_file,
        "## Pomodoros\n- [x] Done\n- [ ] Review inbox\n- [ ] () — GTD\n",
    );
    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("-f")
        .arg("json")
        .arg("Work")
        .arg("@sase:w2=")
        .env("BOB_DAY_FILE", &day_file)
        .env("BOB_NOW", "2026-07-10 09:02:00")
        .output()
        .expect("run unnamed between");
    assert_success(&output);
    let json: serde_json::Value =
        serde_json::from_str(stdout(&output).trim()).expect("unnamed JSON");
    assert_eq!(json["pomodoro_start"]["pomodoro_line"], 3);
    assert_eq!(
        fs::read_to_string(&day_file).expect("read day"),
        concat!(
            "## Pomodoros\n",
            "- [x] Done\n",
            "- [ ] (**0905-0930** [t:: 25m]) — GTD\n",
            "  - [[sase#^w2]]\n",
            "- [ ] Review inbox\n",
        )
    );

    let temp2 = TempDir::new("bob-cli-start-slot-unnamed-no-done");
    let vault2 = temp2.path().join("vault");
    let day2 = vault2.join("day.md");
    write_file(&vault2.join("sase.md"), "# S\n## Tasks\n");
    write_file(&day2, "## Pomodoros\n- [ ] Review inbox\n- [ ] () — GTD\n");
    let output2 = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault2)
        .arg("-f")
        .arg("json")
        .arg("Work")
        .arg("@sase:w2=")
        .env("BOB_DAY_FILE", &day2)
        .env("BOB_NOW", "2026-07-10 09:02:00")
        .output()
        .expect("run unnamed no done");
    assert_success(&output2);
    let json2: serde_json::Value =
        serde_json::from_str(stdout(&output2).trim()).expect("unnamed2 JSON");
    assert_eq!(json2["pomodoro_start"]["pomodoro_line"], 2);
    assert_eq!(
        fs::read_to_string(&day2).expect("read day2"),
        concat!(
            "## Pomodoros\n",
            "- [ ] (**0905-0930** [t:: 25m]) — GTD\n",
            "  - [[sase#^w2]]\n",
            "- [ ] Review inbox\n",
        )
    );
}

#[test]
fn capture_pomodoro_start_already_in_slot_keeps_blank_line() {
    let temp = TempDir::new("bob-cli-start-slot-already");
    let vault = temp.path().join("vault");
    let day_file = vault.join("day.md");
    write_file(&vault.join("sase.md"), "# S\n## Tasks\n");
    write_file(
        &day_file,
        "## Pomodoros\n- [x] Done\n  - child\n\n- [ ] () — GOALS\n",
    );
    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("-f")
        .arg("json")
        .arg("Work")
        .arg("@sase:w1#goals=")
        .env("BOB_DAY_FILE", &day_file)
        .env("BOB_NOW", "2026-07-10 09:02:00")
        .output()
        .expect("run already");
    assert_success(&output);
    let json: serde_json::Value =
        serde_json::from_str(stdout(&output).trim()).expect("already JSON");
    assert_eq!(json["pomodoro_start"]["pomodoro_line"], 5);
    assert_eq!(
        fs::read_to_string(&day_file).expect("read day"),
        concat!(
            "## Pomodoros\n",
            "- [x] Done\n",
            "  - child\n",
            "\n",
            "- [ ] (**0905-0930** [t:: 25m]) — GOALS\n",
            "  - [[sase#^w1]]\n",
        )
    );
}

#[test]
fn capture_pomodoro_start_crlf_no_final_newline_moves_up() {
    let temp = TempDir::new("bob-cli-start-slot-crlf-eof");
    let vault = temp.path().join("vault");
    let day_file = vault.join("day.md");
    write_file(&vault.join("sase.md"), "# S\n## Tasks\n");
    write_file(
        &day_file,
        "## Pomodoros\r\n- [x] Done\r\n- [ ] () — PLANNED\r\n- [ ] () — GOALS",
    );
    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("-f")
        .arg("json")
        .arg("Work")
        .arg("@sase:w1#goals=")
        .env("BOB_DAY_FILE", &day_file)
        .env("BOB_NOW", "2026-07-10 09:02:00")
        .output()
        .expect("run crlf");
    assert_success(&output);
    let json: serde_json::Value =
        serde_json::from_str(stdout(&output).trim()).expect("crlf JSON");
    assert_eq!(json["pomodoro_start"]["pomodoro_line"], 3);
    let day_after = fs::read(&day_file).expect("read day bytes");
    assert_eq!(
        day_after,
        concat!(
            "## Pomodoros\r\n",
            "- [x] Done\r\n",
            "- [ ] (**0905-0930** [t:: 25m]) — GOALS\r\n",
            "  - [[sase#^w1]]\r\n",
            "- [ ] () — PLANNED",
        )
        .as_bytes()
    );
    assert!(!day_after.ends_with(b"\n"));
}

#[test]
fn capture_pomodoro_start_batch_adjusts_moved_entry() {
    let temp = TempDir::new("bob-cli-start-slot-batch");
    let vault = temp.path().join("vault");
    let day_file = vault.join("day.md");
    write_toggle_task_settings(&vault);
    write_file(
        &vault.join("sase_goals.md"),
        "- [*] #task Research goals ^research\n",
    );
    write_file(
        &day_file,
        concat!(
            "## Pomodoros\n",
            "- [x] (**0715-0805** [t:: 50m]) — RELAUNCH\n",
            "\t- [[sase#^relaunch-260927]]\n",
            "- [x] (**1010-1030** [t:: 20m]) — SUBTABS\n",
            "\t- \u{1F345} [[sase#^agents-sub-tabs]]\n",
            "\t\t- Started 0t4!\n",
            "- [ ] () — SUBTABS\n",
            "\t- [[sase#^agents-sub-tabs]]\n",
            "- [ ] () — GTD\n",
            "\t- [[#^gtd]]\n",
            "- [ ] () — GOALS\n",
            "\t- [[sase_goals#^research]]\n",
            "- [ ] () — SASE\n",
            "\t- [[sase#^recovery-panel]]\n",
        ),
    );
    let output = run_with_stdin(
        bob_command()
            .arg("capture")
            .arg("-b")
            .arg(&vault)
            .arg("-f")
            .arg("json")
            .env("BOB_DAY_FILE", &day_file)
            .env("BOB_NOW", "2026-09-27 13:12:00"),
        "^sase_goals:research=5\n\n+2\n",
    );
    assert_success(&output);
    let day_after = fs::read_to_string(&day_file).expect("read day");
    assert_eq!(
        day_after,
        concat!(
            "## Pomodoros\n",
            "- [x] (**0715-0805** [t:: 50m]) — RELAUNCH\n",
            "\t- [[sase#^relaunch-260927]]\n",
            "- [x] (**1010-1030** [t:: 20m]) — SUBTABS\n",
            "\t- \u{1F345} [[sase#^agents-sub-tabs]]\n",
            "\t\t- Started 0t4!\n",
            "- [ ] (**1315-1350** [t:: 35m]) — GOALS\n",
            "\t- [[sase_goals#^research]]\n",
            "- [ ] () — SUBTABS\n",
            "\t- [[sase#^agents-sub-tabs]]\n",
            "- [ ] () — GTD\n",
            "\t- [[#^gtd]]\n",
            "- [ ] () — SASE\n",
            "\t- [[sase#^recovery-panel]]\n",
        )
    );
}
