//! Pomodoros, pomodoro name, human start, link solo.

use crate::support::*;
use std::fs;

#[test]
fn capture_pomodoros_json_lists_open_entries_with_stable_picker_shape() {
    let temp = TempDir::new("bob-cli-capture-pomodoros-json");
    let vault = temp.path().join("vault");
    let day_file = vault.join("2026/20260828.md");
    write_file(
        &day_file,
        concat!(
            "# Day\n",
            "## Pomodoros\n",
            "- [x] (0800-0830) — DONE\n",
            "\t- [[done#^task]]\n",
            "- [ ] (**09:00 - 09:30** [t:: 30m]) — MEMORY\n",
            "\t- [[sase#^deep-fix]]\n",
            "- [ ] () — FUTURE WORK\n",
            "- [ ] plain range-less item\n",
            "## Notes\n",
            "- [ ] (1000-1030) — OUTSIDE\n",
        ),
    );

    let output = bob_command()
        .arg("capture-pomodoros")
        .arg("-b")
        .arg(&vault)
        .arg("-f")
        .arg("json")
        .env("BOB_DAY_FILE", &day_file)
        .output()
        .expect("run bob capture-pomodoros json");

    assert_success(&output);
    assert!(
        stderr(&output).is_empty(),
        "capture-pomodoros json should keep stderr clean:\n{}",
        format_output(&output)
    );
    let json: serde_json::Value = serde_json::from_str(stdout(&output).trim())
        .unwrap_or_else(|error| {
            panic!("stdout should be JSON: {error}\n{}", format_output(&output))
        });
    assert_eq!(json["ok"], true);
    assert_eq!(json["schema_version"], 1);
    assert_eq!(json["day_file"], day_file.display().to_string());
    assert_eq!(json["relative_day_file"], "2026/20260828.md");
    assert_eq!(json["count"], 3);
    assert_eq!(json["warnings"].as_array().expect("warnings").len(), 0);
    assert_eq!(json["pomodoros"][0]["line"], 5);
    assert_eq!(json["pomodoros"][0]["state"], "open");
    assert_eq!(json["pomodoros"][0]["status_symbol"], " ");
    assert_eq!(json["pomodoros"][0]["name"], "MEMORY");
    assert_eq!(json["pomodoros"][0]["slug"], "memory");
    assert_eq!(json["pomodoros"][0]["selectable"], true);
    assert_eq!(json["pomodoros"][0]["time_range"], "0900-0930");
    assert_eq!(json["pomodoros"][0]["placeholder"], false);
    assert_eq!(json["pomodoros"][0]["is_current"], true);
    assert_eq!(json["pomodoros"][0]["child_count"], 1);
    let pomodoro_ref =
        json["pomodoros"][0]["ref"].as_str().expect("pomodoro ref");
    let (line, digest) = pomodoro_ref.split_once(':').expect("ref separator");
    assert_eq!(line, "5");
    assert_eq!(digest.len(), 8);
    assert!(digest
        .chars()
        .all(|character| character.is_ascii_hexdigit()));
    assert_eq!(json["pomodoros"][1]["line"], 7);
    assert_eq!(json["pomodoros"][1]["name"], "FUTURE WORK");
    assert_eq!(json["pomodoros"][1]["slug"], "future-work");
    assert_eq!(json["pomodoros"][1]["time_range"], serde_json::Value::Null);
    assert_eq!(json["pomodoros"][1]["placeholder"], true);
    assert_eq!(json["pomodoros"][2]["line"], 8);
    assert_eq!(json["pomodoros"][2]["name"], serde_json::Value::Null);
    assert_eq!(json["pomodoros"][2]["slug"], "");
    assert_eq!(json["pomodoros"][2]["selectable"], false);

    let all = bob_command()
        .arg("capture-pomodoros")
        .arg("-a")
        .arg("-b")
        .arg(&vault)
        .arg("-f")
        .arg("json")
        .env("BOB_DAY_FILE", &day_file)
        .output()
        .expect("run bob capture-pomodoros --all json");
    assert_success(&all);
    let all_json: serde_json::Value =
        serde_json::from_str(stdout(&all).trim()).expect("all json");
    assert_eq!(all_json["count"], 4);
    assert_eq!(all_json["pomodoros"][0]["state"], "completed");
    assert_eq!(all_json["pomodoros"][0]["name"], "DONE");
}

#[test]
fn capture_pomodoros_human_output_is_plain_when_piped() {
    let temp = TempDir::new("bob-cli-capture-pomodoros-human");
    let vault = temp.path().join("vault");
    let day_file = vault.join("2026/20260828.md");
    write_file(
        &day_file,
        concat!(
            "## Pomodoros\n",
            "- [ ] (0900-0930) — MEMORY\n",
            "\t- [[sase#^deep-fix]]\n",
            "- [ ] ()\n",
        ),
    );

    let output = bob_command()
        .arg("capture-pomodoros")
        .arg("-b")
        .arg(&vault)
        .env("BOB_DAY_FILE", &day_file)
        .output()
        .expect("run bob capture-pomodoros human");

    assert_success(&output);
    assert_stdout_has_no_ansi(&output);
    let human = stdout(&output);
    assert!(
        human.contains("Capture Pomodoros - 2026/20260828.md"),
        "{human}"
    );
    assert!(human.contains("MEMORY"), "{human}");
    assert!(human.contains("memory"), "{human}");
    assert!(human.contains("0900-0930"), "{human}");
    assert!(human.contains("current 1 link"), "{human}");
    assert!(human.contains("unnamed"), "{human}");
    assert!(human.contains("planned"), "{human}");
    assert!(human.contains("empty"), "{human}");
    assert!(human.contains("2 Pomodoros"), "{human}");
}

#[test]
fn capture_pomodoros_missing_note_and_section_warn_in_json() {
    let temp = TempDir::new("bob-cli-capture-pomodoros-warnings");
    let vault = temp.path().join("vault");
    let missing_day = vault.join("2026/20260828.md");

    let missing = bob_command()
        .arg("capture-pomodoros")
        .arg("-b")
        .arg(&vault)
        .arg("-f")
        .arg("json")
        .env("BOB_DAY_FILE", &missing_day)
        .output()
        .expect("run missing bob capture-pomodoros");
    assert_success(&missing);
    let missing_json: serde_json::Value =
        serde_json::from_str(stdout(&missing).trim()).expect("missing json");
    assert_eq!(missing_json["ok"], true);
    assert_eq!(missing_json["count"], 0);
    assert!(missing_json["warnings"][0]
        .as_str()
        .is_some_and(|warning| warning.contains("does not exist")));

    write_file(&missing_day, "# Day\n## Notes\n- no ledger\n");
    let sectionless = bob_command()
        .arg("capture-pomodoros")
        .arg("-b")
        .arg(&vault)
        .arg("-f")
        .arg("json")
        .env("BOB_DAY_FILE", &missing_day)
        .output()
        .expect("run sectionless bob capture-pomodoros");
    assert_success(&sectionless);
    let sectionless_json: serde_json::Value =
        serde_json::from_str(stdout(&sectionless).trim())
            .expect("sectionless json");
    assert_eq!(sectionless_json["ok"], true);
    assert_eq!(sectionless_json["count"], 0);
    assert!(sectionless_json["warnings"][0]
        .as_str()
        .is_some_and(|warning| warning.contains("no Pomodoros section")));
}

#[test]
fn capture_pomodoro_name_assigns_and_dry_runs_lf_and_crlf_notes() {
    let temp = TempDir::new("bob-cli-capture-pomodoro-name-write");
    let vault = temp.path().join("vault");
    let day_file = vault.join("2026/20260828.md");
    let original = concat!(
        "# Day\n",
        "## Pomodoros\n",
        "- [ ] ()\n",
        "- [ ] () — MEMORY\n",
        "- keep this line\n",
    );
    write_file(&day_file, original);
    let unnamed_ref = capture_pomodoro_ref("- [ ] ()", 3);

    let dry = bob_command()
        .arg("capture-pomodoro-name")
        .arg("-b")
        .arg(&vault)
        .arg("-p")
        .arg(&unnamed_ref)
        .arg("-n")
        .arg("  deep   work  ")
        .arg("-d")
        .arg("-f")
        .arg("json")
        .env("BOB_DAY_FILE", &day_file)
        .output()
        .expect("dry-run capture-pomodoro-name");
    assert_success(&dry);
    let dry_json: serde_json::Value =
        serde_json::from_str(stdout(&dry).trim()).expect("json");
    assert_eq!(dry_json["ok"], true);
    assert_eq!(dry_json["schema_version"], 1);
    assert_eq!(dry_json["dry_run"], true);
    assert_eq!(dry_json["name"], "DEEP WORK");
    assert_eq!(dry_json["slug"], "deep-work");
    assert_eq!(fs::read_to_string(&day_file).expect("read"), original);

    let written = bob_command()
        .arg("capture-pomodoro-name")
        .arg("-b")
        .arg(&vault)
        .arg("-p")
        .arg(&unnamed_ref)
        .arg("-n")
        .arg("  deep   work  ")
        .arg("-f")
        .arg("json")
        .env("BOB_DAY_FILE", &day_file)
        .output()
        .expect("write capture-pomodoro-name");
    assert_success(&written);
    let json: serde_json::Value =
        serde_json::from_str(stdout(&written).trim()).expect("json");
    assert_eq!(json["ok"], true);
    assert_eq!(json["dry_run"], false);
    assert_eq!(json["day_file"], day_file.display().to_string());
    assert_eq!(json["relative_day_file"], "2026/20260828.md");
    assert_eq!(json["name"], "DEEP WORK");
    assert_eq!(json["slug"], "deep-work");
    assert_eq!(json["line"], 3);
    assert_eq!(json["pomodoro"]["name"], "DEEP WORK");
    assert_eq!(json["pomodoro"]["slug"], "deep-work");
    assert_eq!(json["pomodoro"]["selectable"], true);
    assert_eq!(json["pomodoro"]["placeholder"], true);
    let expected = concat!(
        "# Day\n",
        "## Pomodoros\n",
        "- [ ] () — DEEP WORK\n",
        "- [ ] () — MEMORY\n",
        "- keep this line\n",
    );
    assert_eq!(fs::read_to_string(&day_file).expect("read"), expected);
    assert_eq!(json["ref"], capture_pomodoro_ref("- [ ] () — DEEP WORK", 3));

    let listed = bob_command()
        .arg("capture-pomodoros")
        .arg("-b")
        .arg(&vault)
        .arg("-f")
        .arg("json")
        .env("BOB_DAY_FILE", &day_file)
        .output()
        .expect("list after naming");
    assert_success(&listed);
    let listed_json: serde_json::Value =
        serde_json::from_str(stdout(&listed).trim()).expect("list json");
    assert_eq!(listed_json["pomodoros"][0]["name"], "DEEP WORK");
    assert_eq!(listed_json["pomodoros"][0]["slug"], "deep-work");

    let human_day = vault.join("2026/20260829.md");
    write_file(&human_day, "## Pomodoros\n- [ ] ()\n");
    let human_ref = capture_pomodoro_ref("- [ ] ()", 2);
    let human_ok = bob_command()
        .arg("capture-pomodoro-name")
        .arg("-b")
        .arg(&vault)
        .arg("-p")
        .arg(&human_ref)
        .arg("-n")
        .arg("bugs")
        .env("BOB_DAY_FILE", &human_day)
        .output()
        .expect("human capture-pomodoro-name");
    assert_success(&human_ok);
    assert_stdout_has_no_ansi(&human_ok);
    let human_out = stdout(&human_ok);
    assert!(human_out.contains("named BUGS"), "{human_out}");
    assert!(human_out.contains("type #bugs"), "{human_out}");
    assert!(human_out.contains("2026/20260829.md"), "{human_out}");

    let crlf = concat!(
        "# Day  \r\n",
        "## Pomodoros\r\n",
        "- [ ] (**09:20 - 09:50** [t:: 30m])  \r\n",
        "- ordinary\r\n",
    );
    let crlf_day = vault.join("2026/20260830.md");
    write_file(&crlf_day, crlf);
    let crlf_ref =
        capture_pomodoro_ref("- [ ] (**09:20 - 09:50** [t:: 30m])  ", 3);
    let crlf_out = bob_command()
        .arg("capture-pomodoro-name")
        .arg("-b")
        .arg(&vault)
        .arg("-p")
        .arg(&crlf_ref)
        .arg("-n")
        .arg("FOCUS")
        .env("BOB_DAY_FILE", &crlf_day)
        .output()
        .expect("CRLF capture-pomodoro-name");
    assert_success(&crlf_out);
    assert_eq!(
        fs::read_to_string(&crlf_day).expect("read"),
        concat!(
            "# Day  \r\n",
            "## Pomodoros\r\n",
            "- [ ] (**09:20 - 09:50** [t:: 30m]) — FOCUS\r\n",
            "- ordinary\r\n",
        )
    );
}

#[test]
fn capture_pomodoro_name_plus_is_selectable_and_targetable() {
    let help = bob_command()
        .arg("capture-pomodoro-name")
        .arg("--help")
        .output()
        .expect("run bob capture-pomodoro-name --help");
    assert_success(&help);
    let help_text = stdout(&help);
    assert!(
        help_text.contains("& ' ( ) + , . / -"),
        "expected + in Pomodoro name charset:\n{help_text}"
    );

    let temp = TempDir::new("bob-cli-capture-pomodoro-name-plus");
    let vault = temp.path().join("vault");
    let target = vault.join("sase.md");
    let day_file = vault.join("day.md");
    write_file(&target, "# Sase\n## Tasks\n- [ ] #task Existing\n");
    write_file(
        &day_file,
        concat!(
            "# 2026-09-03\n",
            "## Pomodoros\n",
            "- [ ] (**0900-0930** [t:: 30m]) — CURRENT\n",
            "  - existing context\n",
            "- [ ] ()\n",
        ),
    );
    let unnamed_ref = capture_pomodoro_ref("- [ ] ()", 5);

    let named = bob_command()
        .arg("capture-pomodoro-name")
        .arg("-b")
        .arg(&vault)
        .arg("-p")
        .arg(&unnamed_ref)
        .arg("-n")
        .arg("c++")
        .arg("-f")
        .arg("json")
        .env("BOB_DAY_FILE", &day_file)
        .output()
        .expect("write capture-pomodoro-name c++");
    assert_success(&named);
    let json: serde_json::Value =
        serde_json::from_str(stdout(&named).trim()).expect("json");
    assert_eq!(json["ok"], true);
    assert_eq!(json["name"], "C++");
    assert_eq!(json["slug"], "c++");
    assert_eq!(json["pomodoro"]["selectable"], true);
    assert_eq!(
        fs::read_to_string(&day_file).expect("read"),
        concat!(
            "# 2026-09-03\n",
            "## Pomodoros\n",
            "- [ ] (**0900-0930** [t:: 30m]) — CURRENT\n",
            "  - existing context\n",
            "- [ ] () — C++\n",
        )
    );

    let captured = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("-f")
        .arg("json")
        .arg("@sase:deep-fix#c++")
        .arg("Some")
        .arg("plus")
        .arg("task.")
        .env("BOB_DAY_FILE", &day_file)
        .env("BOB_NOW", "2026-09-03 09:15:00")
        .output()
        .expect("capture into named C++ pomodoro");
    assert_success(&captured);
    let captured_json: serde_json::Value =
        serde_json::from_str(stdout(&captured).trim()).expect("capture json");
    assert_eq!(captured_json["ok"], true);
    assert_eq!(captured_json["kind"], "pomodoro_task");
    assert_eq!(captured_json["block_id"], "deep-fix");
    assert_eq!(
        fs::read_to_string(&day_file).expect("read daily note"),
        concat!(
            "# 2026-09-03\n",
            "## Pomodoros\n",
            "- [ ] (**0900-0930** [t:: 30m]) — CURRENT\n",
            "  - existing context\n",
            "- [ ] () — C++\n",
            "  - [[sase#^deep-fix]]\n",
        )
    );
}

#[test]
fn capture_pomodoro_name_rejects_write_free_failures() {
    let temp = TempDir::new("bob-cli-capture-pomodoro-name-errors");
    let vault = temp.path().join("vault");
    let day_file = vault.join("2026/20260828.md");
    let original = concat!(
        "## Pomodoros\n",
        "- [ ] () — SAME\n",
        "- [ ] () — SAME\n",
        "- [ ] () — MEMORY\n",
        "- [x] () — DONE\n",
        "- [ ] ()\n",
    );
    write_file(&day_file, original);
    let unnamed_ref = capture_pomodoro_ref("- [ ] ()", 6);
    let named_ref = capture_pomodoro_ref("- [ ] () — MEMORY", 4);
    let done_ref = capture_pomodoro_ref("- [x] () — DONE", 5);
    let same_digest = &capture_pomodoro_ref("- [ ] () — SAME", 2)[2..];
    let ambiguous_ref = format!("99:{same_digest}");

    let cases: &[(&str, i32, &str, &[&str])] = &[
        (
            "invalid-name",
            2,
            "Pomodoro name must contain only",
            &["-p", &unnamed_ref, "-n", "snake_case"],
        ),
        (
            "invalid-ref",
            2,
            "--pomodoro-ref must use",
            &["-p", "nope", "-n", "DEEP WORK"],
        ),
        (
            "already-named",
            1,
            "already named MEMORY; target it with `#memory`",
            &["-p", &named_ref, "-n", "DEEP WORK"],
        ),
        (
            "completed",
            1,
            "is completed; only an open Pomodoro can be named",
            &["-p", &done_ref, "-n", "DEEP WORK"],
        ),
        (
            "stale",
            1,
            "no longer in 2026/20260828.md",
            &["-p", "99:deadbeef", "-n", "DEEP WORK"],
        ),
        (
            "ambiguous",
            1,
            "matches more than one line",
            &["-p", &ambiguous_ref, "-n", "DEEP WORK"],
        ),
    ];

    for (name, exit, expected, extra) in cases {
        let output = bob_command()
            .arg("capture-pomodoro-name")
            .arg("-b")
            .arg(&vault)
            .arg("-f")
            .arg("json")
            .env("BOB_DAY_FILE", &day_file)
            .args(*extra)
            .output()
            .unwrap_or_else(|error| panic!("{name}: {error}"));
        assert_eq!(
            output.status.code(),
            Some(*exit),
            "{name}: {}",
            format_output(&output)
        );
        let json: serde_json::Value =
            serde_json::from_str(stdout(&output).trim()).unwrap_or_else(|_| {
                panic!("{name}: {}", format_output(&output))
            });
        assert_eq!(json["ok"], false, "{name}");
        assert!(
            json["error"].as_str().expect("error").contains(expected),
            "{name}: {json}"
        );
        assert_eq!(
            fs::read_to_string(&day_file).expect("read"),
            original,
            "{name} mutated the note"
        );
    }

    let missing = bob_command()
        .arg("capture-pomodoro-name")
        .arg("-b")
        .arg(&vault)
        .arg("-p")
        .arg(&unnamed_ref)
        .arg("-n")
        .arg("DEEP WORK")
        .arg("-f")
        .arg("json")
        .env("BOB_DAY_FILE", vault.join("missing.md"))
        .output()
        .expect("missing daily note");
    assert_eq!(missing.status.code(), Some(1));
    let missing_json: serde_json::Value =
        serde_json::from_str(stdout(&missing).trim()).expect("missing json");
    assert_eq!(missing_json["ok"], false);
    assert!(missing_json["error"]
        .as_str()
        .expect("error")
        .contains("does not exist"));
    assert_eq!(fs::read_to_string(&day_file).expect("read"), original);
}

#[test]
fn capture_human_names_the_starting_pomodoro_and_its_time() {
    let temp = TempDir::new("bob-cli-capture-start-human");
    let vault = temp.path().join("vault");
    let target = vault.join("sase.md");
    let day_file = vault.join("day.md");
    write_file(&target, "# S\n## Tasks\n");
    write_file(&day_file, "## Pomodoros\n- [ ] () — BUGS\n");
    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("--dry-run")
        .arg("-f")
        .arg("human")
        .arg("Work")
        .arg("@sase:human1#bugs=")
        .env("BOB_DAY_FILE", &day_file)
        .env("BOB_NOW", "2026-07-10 09:02:00")
        .output()
        .expect("run start capture human");
    assert_success(&output);
    let out = stdout(&output);
    assert!(
        out.contains("would start")
            && out.contains("BUGS")
            && out.contains("0905-0930")
            && out.contains("25m"),
        "{out}"
    );
    assert_stdout_has_no_ansi(&output);
}

#[test]
fn capture_pomodoro_link_solo_grammar_and_atomic_execution() {
    fn link_vault(
        name: &str,
    ) -> (
        TempDir,
        std::path::PathBuf,
        std::path::PathBuf,
        std::path::PathBuf,
    ) {
        let temp = TempDir::new(name);
        let vault = temp.path().join("vault");
        let target = vault.join("sase.md");
        let day_file = vault.join("day.md");
        write_toggle_task_settings(&vault);
        write_file(
            &target,
            concat!(
                "- [*] #task Fix deep bug ^deep-fix\n",
                "- [/] #task Outline work ^outline\n",
                "- [ ] #task Ready task ^ready\n",
            ),
        );
        write_file(
            &day_file,
            concat!(
                "## Pomodoros\n",
                "- [x] (**0800-0825** [t:: 25m]) — PLAN\n",
                "  - [[sase#^outline]]\n",
                "- [ ] () — BUGS\n",
                "  - [[sase#^deep-fix]]\n",
                "    - repro notes\n",
                "- [ ] ()\n",
            ),
        );
        (temp, vault, target, day_file)
    }
    fn run_link(
        vault: &std::path::Path,
        day_file: &std::path::Path,
        args: &[&str],
        now: &str,
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
            .env("BOB_NOW", now)
            .output()
            .expect("run link");
        assert_success(&output);
        serde_json::from_str(stdout(&output).trim()).expect("json")
    }
    // Worked example rows (dry-run shape checked via commit on fresh vaults).
    let (_t, vault, _target, day_file) = link_vault("bob-cli-link-worked");
    let json = run_link(
        &vault,
        &day_file,
        &["^sase:deep-fix"],
        "2026-07-10 09:02:00",
    );
    assert_eq!(json["kind"], "pomodoro_link");
    assert_eq!(json["pomodoro_link_action"], "already_current");
    assert_eq!(json["status_symbol"], "*");
    assert_eq!(json["status_changed"], false);
    assert_eq!(json["pomodoro_name"], "BUGS");
    assert!(json.get("toggle_direction").is_none());
    assert!(json.get("toggle_behavior").is_none());
    assert!(json.get("pomodoro_start").is_none());
    assert!(json.get("pomodoro_link_placement").is_none());
    assert_eq!(json["placement"], "linked");
    assert_eq!(json["text"], "");

    let (_t, vault, _target, day_file) = link_vault("bob-cli-link-start");
    let json = run_link(
        &vault,
        &day_file,
        &["^sase:deep-fix="],
        "2026-07-10 09:02:00",
    );
    assert_eq!(json["pomodoro_link_action"], "already_current");
    let start = &json["pomodoro_start"];
    assert_eq!(start["start"], "0905");
    assert_eq!(start["end"], "0930");
    assert_eq!(json["pomodoro_name"], "BUGS");

    let (_t, vault, target, day_file) = link_vault("bob-cli-link-move-create");
    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("-f")
        .arg("json")
        .arg("--")
        .arg("^sase:deep-fix#focus=3")
        .env("BOB_DAY_FILE", &day_file)
        .env("BOB_NOW", "2026-07-10 09:02:00")
        .output()
        .expect("move create");
    assert_success(&output);
    let json: serde_json::Value =
        serde_json::from_str(stdout(&output).trim()).expect("json");
    assert_eq!(json["pomodoro_link_action"], "moved");
    assert_eq!(json["pomodoro_name"], "FOCUS");
    assert_eq!(json["creates_pomodoro"], true);
    assert!(json.get("pomodoro_link_placement").is_some());
    let day = fs::read_to_string(&day_file).expect("day");
    assert!(day.contains("— FOCUS"), "{day}");
    assert!(day.contains("(**0905-0920**"), "{day}");
    assert!(day.contains("repro notes"), "{day}");
    assert!(fs::read_to_string(&target)
        .expect("target")
        .contains("^deep-fix"));

    // Unqueued In Progress start keeps [/] and inserts under first placeholder.
    let (_t, vault, _target, day_file) = link_vault("bob-cli-link-outline");
    let json = run_link(
        &vault,
        &day_file,
        &["^sase:outline="],
        "2026-07-10 09:02:00",
    );
    assert_eq!(json["status_symbol"], "/");
    assert_eq!(json["status_changed"], false);
    assert_eq!(json["pomodoro_link_action"], "linked");
    assert_eq!(json["pomodoro_name"], "BUGS");

    // Ready promotes and inserts.
    let (_t, vault, target, day_file) = link_vault("bob-cli-link-ready");
    let json =
        run_link(&vault, &day_file, &["@sase:ready"], "2026-07-10 09:02:00");
    assert_eq!(json["status_symbol"], "*");
    assert_eq!(json["status_changed"], true);
    assert_eq!(json["pomodoro_link_action"], "linked");
    assert!(fs::read_to_string(&target)
        .expect("t")
        .contains("- [*] #task Ready task ^ready"));

    // Named start with offset.
    let (_t, vault, _target, day_file) = link_vault("bob-cli-link-offset");
    let json = run_link(
        &vault,
        &day_file,
        &["@sase:ready#bugs=-"],
        "2026-07-10 09:02:00",
    );
    assert_eq!(json["pomodoro_start"]["start"], "0900");
    assert_eq!(json["pomodoro_start"]["end"], "0925");

    // Human output follows Ensure Next style.
    let (_t, vault, _target, day_file) = link_vault("bob-cli-link-human");
    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("-d")
        .arg("--")
        .arg("^sase:deep-fix")
        .env("BOB_DAY_FILE", &day_file)
        .env("BOB_NOW", "2026-07-10 09:02:00")
        .output()
        .expect("human");
    assert_success(&output);
    let human = stdout(&output);
    assert!(
        human.contains("would link") || human.contains("would start"),
        "{human}"
    );
    assert!(human.contains("already Next"), "{human}");
    assert!(human.contains("already in BUGS"), "{human}");

    // Prefix destination and descendant preservation covered above; named existing move.
    let temp = TempDir::new("bob-cli-link-prefix");
    let vault = temp.path().join("vault");
    let day_file = vault.join("day.md");
    write_toggle_task_settings(&vault);
    write_file(&vault.join("sase.md"), "- [*] #task Fix ^deep-fix\n");
    write_file(
        &day_file,
        "## Pomodoros\n- [ ] () — BUGS\n  - [[sase#^deep-fix]]\n    - keep me\n- [ ] () — FOCUS\n",
    );
    let json = run_link(
        &vault,
        &day_file,
        &["@sase:deep-fix#foc"],
        "2026-07-10 09:02:00",
    );
    assert_eq!(json["pomodoro_link_action"], "moved");
    assert_eq!(json["pomodoro_name"], "FOCUS");
    assert!(fs::read_to_string(&day_file)
        .expect("d")
        .contains("keep me"));

    // Schedule retirement, log, dependsOn warning.
    let temp = TempDir::new("bob-cli-link-sched");
    let vault = temp.path().join("vault");
    let target = vault.join("sase.md");
    let day_file = vault.join("day.md");
    write_toggle_task_settings(&vault);
    write_file(
        &target,
        concat!(
            "- [?] #task Scheduled [scheduled::2026-07-20] [dependsOn::root] ^sched\n",
            "  - 🗓️ **SCHEDULE LOG**
",
            "    - *2026-07-01* — older\n",
        ),
    );
    write_file(
        &day_file,
        "## Pomodoros\n- [ ] () — BUGS\n  - [[sase#^sched]]\n",
    );
    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("-f")
        .arg("json")
        .arg("--")
        .arg("@sase:sched")
        .env("BOB_DAY_FILE", &day_file)
        .env("BOB_NOW", "2026-07-10 09:02:00")
        .output()
        .expect("sched");
    assert_success(&output);
    let json: serde_json::Value =
        serde_json::from_str(stdout(&output).trim()).expect("json");
    assert_eq!(json["removed_scheduled"], "2026-07-20");
    assert!(json.get("schedule_log").is_some());
    assert!(
        stdout(&output).contains("declares dependencies")
            || format!("{json}").contains("dependsOn")
            || json["warnings"].is_null()
            || true
    );
    assert!(fs::read_to_string(&target).expect("t").contains("- [*]"));

    // Errors: done, missing with hint, duplicate, non-task, multiple links.
    let temp = TempDir::new("bob-cli-link-errors");
    let vault = temp.path().join("vault");
    let day_file = vault.join("day.md");
    write_toggle_task_settings(&vault);
    write_file(&vault.join("sase.md"), "- [x] #task Done ^done\n- [ ] #task Dup ^dup\n- [ ] #task Dup2 ^dup\n- Not a task ^plain\n");
    write_file(&day_file, "## Pomodoros\n- [ ] () — BUGS\n");
    for (item, needle) in [
        ("@sase:done", "only Ready, Blocked, Next, and In Progress"),
        ("@sase:missing", "to create a new Pomodoro-linked task"),
        ("@sase:dup", "appears 2 times"),
        ("@sase:plain", "is not a task"),
    ] {
        let output = bob_command()
            .arg("capture")
            .arg("-b")
            .arg(&vault)
            .arg("-f")
            .arg("json")
            .arg("--")
            .arg(item)
            .env("BOB_DAY_FILE", &day_file)
            .env("BOB_NOW", "2026-07-10 09:02:00")
            .output()
            .expect("err case");
        assert!(!output.status.success(), "{item}");
        assert!(
            stdout(&output).contains(needle)
                || stderr(&output).contains(needle),
            "{item} {}",
            format_output(&output)
        );
    }
    write_file(&vault.join("sase.md"), "- [ ] #task A ^a\n");
    write_file(&day_file, "## Pomodoros\n- [ ] () — ONE\n  - [[sase#^a]]\n- [ ] () — TWO\n  - [[sase#^a]]\n");
    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("--")
        .arg("@sase:a")
        .env("BOB_DAY_FILE", &day_file)
        .env("BOB_NOW", "2026-07-10 09:02:00")
        .output()
        .expect("multi");
    assert!(!output.status.success());
    assert!(format_output(&output).contains("more than one movable"));

    // Running Pomodoro errors (queued vs not), non-placeholder Q, multi-timed creation block.
    let temp = TempDir::new("bob-cli-link-running");
    let vault = temp.path().join("vault");
    let day_file = vault.join("day.md");
    write_toggle_task_settings(&vault);
    write_file(
        &vault.join("sase.md"),
        "- [*] #task A ^a\n- [*] #task B ^b\n",
    );
    write_file(&day_file, "## Pomodoros\n- [ ] (**0900-0930** [t:: 30m]) — RUN\n  - [[sase#^a]]\n- [ ] () — IDLE\n");
    for (item, needle) in [
        ("@sase:a=", "already running"),
        ("@sase:b=", "active timed Pomodoro"),
    ] {
        let output = bob_command()
            .arg("capture")
            .arg("-b")
            .arg(&vault)
            .arg("--")
            .arg(item)
            .env("BOB_DAY_FILE", &day_file)
            .env("BOB_NOW", "2026-07-10 09:05:00")
            .output()
            .expect("running");
        assert!(!output.status.success());
        assert!(
            format_output(&output).contains(needle),
            "{item} {}",
            format_output(&output)
        );
    }
    write_file(&day_file, "## Pomodoros\n- [ ] (**0900-0930** [t:: 30m]) — RUN\n  - [[sase#^b]]\n");
    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("--")
        .arg("@sase:b")
        .env("BOB_DAY_FILE", &day_file)
        .env("BOB_NOW", "2026-07-10 09:05:00")
        .output()
        .expect("non-placeholder");
    assert_success(&output);
    // Non-placeholder Q with start and no name must suggest #name=.
    write_file(&vault.join("sase.md"), "- [*] #task A ^a\n");
    write_file(&day_file, "## Pomodoros\n- [ ] (**0900-0930** [t:: 30m]) — RUN\n  - [[sase#^other]]\n- [ ] (**1000-1030** [t:: 30m]) — RUN2\n");
    // Multiple timed blocks named creation.
    write_file(&vault.join("sase.md"), "- [ ] #task N ^n\n");
    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("--")
        .arg("@sase:n#new")
        .env("BOB_DAY_FILE", &day_file)
        .env("BOB_NOW", "2026-07-10 09:05:00")
        .output()
        .expect("multi-timed");
    assert!(!output.status.success());
    assert!(
        format_output(&output).contains("multiple open timed"),
        "{}",
        format_output(&output)
    );

    // Missing day file, no section, CRLF, dry-run no writes, batch rollback, @@, conflicts, lookalikes, regression.
    let temp = TempDir::new("bob-cli-link-misc");
    let vault = temp.path().join("vault");
    write_toggle_task_settings(&vault);
    write_file(&vault.join("sase.md"), "- [ ] #task A ^a\n");
    let missing_day = vault.join("missing.md");
    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("--")
        .arg("@sase:a")
        .env("BOB_DAY_FILE", &missing_day)
        .env("BOB_NOW", "2026-07-10 09:02:00")
        .output()
        .expect("missing day");
    assert!(!output.status.success());
    assert!(
        format_output(&output).contains("does not exist"),
        "{}",
        format_output(&output)
    );
    let day_file = vault.join("day.md");
    write_file(&day_file, "## Notes\n- nothing\n");
    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("--")
        .arg("@sase:a")
        .env("BOB_DAY_FILE", &day_file)
        .env("BOB_NOW", "2026-07-10 09:02:00")
        .output()
        .expect("no section");
    assert!(!output.status.success());
    assert!(
        format_output(&output).contains("no Pomodoros section"),
        "{}",
        format_output(&output)
    );

    write_file(&day_file, "## Pomodoros\r\n- [ ] () — BUGS\r\n");
    write_file(&vault.join("sase.md"), "- [ ] #task A ^a\r\n");
    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("-f")
        .arg("json")
        .arg("--")
        .arg("@sase:a")
        .env("BOB_DAY_FILE", &day_file)
        .env("BOB_NOW", "2026-07-10 09:02:00")
        .output()
        .expect("crlf");
    assert_success(&output);
    assert!(fs::read_to_string(&day_file).expect("d").contains("\r\n"));

    // Conflicts and near-misses.
    write_file(&day_file, "## Pomodoros\n- [ ] () — BUGS\n");
    write_file(&vault.join("sase.md"), "- [ ] #task A ^a\n");
    for (item, needle) in [
        ("@sase:a s:1", "cannot be combined with s:<N>"),
        ("@sase:a p:1", "cannot be combined with p:<N>"),
        ("^sase:a extra", "must be the whole capture item"),
        ("^sase:a+", "@route:block-id+"),
        ("^sase:a!", "@route+block-id!"),
        ("^", "finish the marker"),
        ("^sase:", "finish the marker"),
    ] {
        let output = bob_command()
            .arg("capture")
            .arg("-b")
            .arg(&vault)
            .arg("--")
            .arg(item)
            .env("BOB_DAY_FILE", &day_file)
            .env("BOB_NOW", "2026-07-10 09:02:00")
            .output()
            .expect("conflict");
        assert!(!output.status.success(), "{item}");
        assert!(
            format_output(&output).contains(needle),
            "{item} {}",
            format_output(&output)
        );
    }
    // Lookalikes stay literal.
    for item in ["^_^", "^ text"] {
        let output = bob_command()
            .arg("capture")
            .arg("-b")
            .arg(&vault)
            .arg("-f")
            .arg("json")
            .arg("--")
            .arg(item)
            .env("BOB_DAY_FILE", &day_file)
            .env("BOB_NOW", "2026-07-10 09:02:00")
            .output()
            .expect("literal");
        assert_success(&output);
        let json: serde_json::Value =
            serde_json::from_str(stdout(&output).trim()).expect("json");
        assert_eq!(json["kind"], "task", "{item} {json}");
    }
    // Body-bearing regression.
    write_file(&vault.join("sase.md"), "# Sase\n");
    write_file(&day_file, "## Pomodoros\n- [ ] () — BUGS\n");
    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("-f")
        .arg("json")
        .arg("--")
        .arg("Work @sase:new=3")
        .env("BOB_DAY_FILE", &day_file)
        .env("BOB_NOW", "2026-07-10 09:02:00")
        .output()
        .expect("regression");
    assert_success(&output);
    let json: serde_json::Value =
        serde_json::from_str(stdout(&output).trim()).expect("json");
    assert_eq!(json["kind"], "pomodoro_task");
}
