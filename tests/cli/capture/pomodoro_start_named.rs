//! Named Pomodoro starts (`=<X>#name`) in `bob capture`.

use crate::support::*;
use std::fs;

fn named_vault(
    name: &str,
    day_contents: &str,
) -> (TempDir, std::path::PathBuf, std::path::PathBuf) {
    let temp = TempDir::new(name);
    let vault = temp.path().join("vault");
    let day_file = vault.join("day.md");
    write_file(&day_file, day_contents);
    (temp, vault, day_file)
}

fn run_capture(
    vault: &std::path::Path,
    day_file: &std::path::Path,
    args: &[&str],
    now: &str,
) -> std::process::Output {
    let mut command = bob_command();
    command
        .arg("capture")
        .arg("-b")
        .arg(vault)
        .arg("-f")
        .arg("json");
    for arg in args {
        command.arg("--");
        command.arg(arg);
    }
    command
        .env("BOB_DAY_FILE", day_file)
        .env("BOB_NOW", now)
        .output()
        .expect("run named start capture")
}

const WORKED: &str = concat!(
    "# 2026-07-10\n\n",
    "## Pomodoros\n\n",
    "- [x] (**0830-0855** [t:: 25m]) — PLAN\n",
    "  - [[sase#^plan-day]]\n",
    "- [ ] () — BUGS\n",
    "  - [[sase#^deep-fix]]\n",
    "- [ ] () — DEEP WORK\n",
    "  - [[bob#^outline]]\n",
    "  - [[bob#^draft]]\n",
    "- [ ] ()\n",
    "  - [[bob#^inbox-zero]]\n",
);

#[test]
fn named_start_existing_reports_json_and_moves_to_current_slot() {
    let (_temp, vault, day_file) =
        named_vault("bob-cli-named-existing", WORKED);
    let output =
        run_capture(&vault, &day_file, &["=#deep-work"], "2026-07-10 09:02:00");
    assert_success(&output);
    let json: serde_json::Value =
        serde_json::from_str(stdout(&output).trim()).expect("named JSON");
    assert_eq!(json["kind"], "pomodoro_start");
    assert_eq!(json["text"], "=#deep-work");
    assert_eq!(json["placement"], "started");
    assert_eq!(json["pomodoro_name"], "DEEP WORK");
    assert_eq!(json["pomodoro_start"]["pomodoro_name"], "DEEP WORK");
    assert_eq!(json["pomodoro_start"]["created_pomodoro"], false);
    assert_eq!(json["pomodoro_start"]["start"], "0905");
    assert_eq!(json["pomodoro_start"]["end"], "0930");
    let after = fs::read_to_string(&day_file).expect("read day");
    assert!(
        after.contains("- [ ] (**0905-0930** [t:: 25m]) — DEEP WORK"),
        "{after}"
    );
    // Moved before BUGS (current slot after the completed PLAN block).
    let deep = after.find("DEEP WORK").expect("deep");
    let bugs = after.find("BUGS").expect("bugs");
    assert!(deep < bugs, "{after}");
}

#[test]
fn named_start_prefix_and_duration_forms() {
    let (_temp, vault, day_file) = named_vault("bob-cli-named-prefix", WORKED);
    let output =
        run_capture(&vault, &day_file, &["=#deep"], "2026-07-10 09:02:00");
    assert_success(&output);
    let json: serde_json::Value =
        serde_json::from_str(stdout(&output).trim()).expect("prefix JSON");
    assert_eq!(json["pomodoro_start"]["pomodoro_name"], "DEEP WORK");

    let (_temp, vault, day_file) = named_vault("bob-cli-named-dur", WORKED);
    let output =
        run_capture(&vault, &day_file, &["=3#bugs"], "2026-07-10 09:02:00");
    assert_success(&output);
    let json: serde_json::Value =
        serde_json::from_str(stdout(&output).trim()).expect("dur JSON");
    assert_eq!(json["pomodoro_start"]["pomodoro_name"], "BUGS");
    assert_eq!(json["pomodoro_start"]["start"], "0905");
    assert_eq!(json["pomodoro_start"]["end"], "0920");
    assert_eq!(json["pomodoro_start"]["duration_minutes"], 15);
}

#[test]
fn named_start_again_creates_from_completed_name() {
    let (_temp, vault, day_file) = named_vault("bob-cli-named-again", WORKED);
    let output =
        run_capture(&vault, &day_file, &["=#plan"], "2026-07-10 09:02:00");
    assert_success(&output);
    let json: serde_json::Value =
        serde_json::from_str(stdout(&output).trim()).expect("again JSON");
    assert_eq!(json["pomodoro_start"]["pomodoro_name"], "PLAN");
    assert_eq!(json["pomodoro_start"]["created_pomodoro"], true);
    let after = fs::read_to_string(&day_file).expect("read day");
    // Completed PLAN untouched; a new PLAN started.
    assert!(
        after.contains("- [x] (**0830-0855** [t:: 25m]) — PLAN"),
        "{after}"
    );
    assert!(
        after.contains("- [ ] (**0905-0930** [t:: 25m]) — PLAN"),
        "{after}"
    );
}

#[test]
fn named_start_missing_creates_and_warns_on_near_miss() {
    let (_temp, vault, day_file) = named_vault("bob-cli-named-new", WORKED);
    let output =
        run_capture(&vault, &day_file, &["=#review"], "2026-07-10 09:02:00");
    assert_success(&output);
    let json: serde_json::Value =
        serde_json::from_str(stdout(&output).trim()).expect("new JSON");
    assert_eq!(json["pomodoro_start"]["pomodoro_name"], "REVIEW");
    assert_eq!(json["pomodoro_start"]["created_pomodoro"], true);

    let (_temp, vault, day_file) = named_vault("bob-cli-named-suggest", WORKED);
    let output =
        run_capture(&vault, &day_file, &["=#bgus"], "2026-07-10 09:02:00");
    assert_success(&output);
    let json: serde_json::Value =
        serde_json::from_str(stdout(&output).trim()).expect("suggest JSON");
    assert_eq!(json["pomodoro_start"]["pomodoro_name"], "BGUS");
    let warnings = json["warnings"].as_array().cloned().unwrap_or_default();
    assert!(
        warnings.iter().any(|warning| warning
            .as_str()
            .unwrap_or_default()
            .contains("did you mean BUGS")),
        "{json}"
    );
}

#[test]
fn named_start_grammar_errors_are_precise() {
    let (_temp, vault, day_file) = named_vault("bob-cli-named-errors", WORKED);
    for (token, needle) in [
        ("=#", "is incomplete: type a Pomodoro name after `#`"),
        ("=#bugs=3", "write the duration before the name: `=3#bugs`"),
        ("=x#bugs", "`=x` always closes the running Pomodoro"),
        ("=#deep work", "must be the whole capture item"),
    ] {
        let output =
            run_capture(&vault, &day_file, &[token], "2026-07-10 09:02:00");
        assert!(!output.status.success(), "{token}");
        let error = stdout(&output);
        assert!(error.contains(needle), "{token}: {error}");
    }
    // Untouched on failure.
    let before = fs::read_to_string(&day_file).expect("read day");
    assert!(before.contains("- [ ] () — BUGS"), "{before}");
}

#[test]
fn named_start_running_guards_and_switch_chain() {
    let running = concat!(
        "## Pomodoros\n",
        "- [ ] (**0840-0905** [t:: 25m]) — BUGS\n",
        "- [ ] () — DEEP WORK\n",
    );
    let (_temp, vault, day_file) =
        named_vault("bob-cli-named-running", running);
    let output =
        run_capture(&vault, &day_file, &["=#deep-work"], "2026-07-10 09:02:00");
    assert!(!output.status.success());
    assert!(
        stdout(&output).contains("is still running")
            && stdout(&output).contains("=x =#deep-work"),
        "{}",
        stdout(&output)
    );

    let (_temp, vault, day_file) =
        named_vault("bob-cli-named-already", running);
    let output =
        run_capture(&vault, &day_file, &["=#bugs"], "2026-07-10 09:02:00");
    assert!(!output.status.success());
    assert!(
        stdout(&output).contains("is already running"),
        "{}",
        stdout(&output)
    );

    // Switch chain closes then starts atomically.
    let (_temp, vault, day_file) = named_vault("bob-cli-named-switch", running);
    let mut command = bob_command();
    let output = command
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("-f")
        .arg("json")
        .arg("--")
        .arg("=x =#deep-work")
        .env("BOB_DAY_FILE", &day_file)
        .env("BOB_NOW", "2026-07-10 09:02:00")
        .output()
        .expect("run switch chain");
    assert_success(&output);
    let after = fs::read_to_string(&day_file).expect("read day");
    assert!(after.contains("DEEP WORK"), "{after}");
}

#[test]
fn named_start_human_created_dry_run_and_forced_flags() {
    let (_temp, vault, day_file) = named_vault("bob-cli-named-human", WORKED);
    let human = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("--dry-run")
        .arg("=#plan")
        .env("BOB_DAY_FILE", &day_file)
        .env("BOB_NOW", "2026-07-10 09:02:00")
        .output()
        .expect("run human dry-run");
    assert_success(&human);
    let out = stdout(&human);
    assert!(
        out.contains("would start")
            && out.contains("(created)")
            && out.contains("PLAN 0905-0930 (25m)"),
        "{out}"
    );

    // Forced destination rejected.
    let forced = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("--route")
        .arg("foo")
        .arg("--")
        .arg("=#bugs")
        .env("BOB_DAY_FILE", &day_file)
        .env("BOB_NOW", "2026-07-10 09:02:00")
        .output()
        .expect("run forced");
    assert!(!forced.status.success());
}

#[test]
fn named_start_created_reports_added_block() {
    let (_temp, vault, day_file) = named_vault(
        "bob-cli-named-created-blocks",
        concat!(
            "## Pomodoros\n",
            "- [x] (**0620-0710** [t:: 50m]) — CLEANUP\n",
            "- [ ] () — GTD\n",
            "\t- [[#^gtd]]\n",
        ),
    );
    let day_before = fs::read_to_string(&day_file).expect("read day before");

    let json = capture_json_dry_run_matches_real(
        &vault,
        &day_file,
        "2026-09-30 07:20:00",
        &["=#focus"],
    );
    let day_after = fs::read_to_string(&day_file).expect("read day after");
    assert_pomodoro_blocks_cover_changes(&day_before, &day_after, &json);
    assert_eq!(
        json["pomodoro_blocks"],
        serde_json::json!([
            {
                "relative_target": "day.md",
                "line": 3,
                "name": "FOCUS",
                "time_range": "0720-0745",
                "status": "running",
                "created": true,
                "roles": ["started"],
                "lines": [
                    {
                        "text": "- [ ] (**0720-0745** [t:: 25m]) — FOCUS",
                        "depth": 0,
                        "change": "added",
                    },
                ],
            },
        ])
    );
}
