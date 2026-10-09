//! Note-free `=x0` reset coverage: reset versus note-preserving close.

use super::pomodoro_close::{
    close_worked_vault, run_close_expect_error, run_close_json,
};
use crate::support::*;
use std::fs;

fn reset_vault(
    name: &str,
    pomodoros: &str,
) -> (TempDir, std::path::PathBuf, std::path::PathBuf) {
    let temp = TempDir::new(name);
    let vault = temp.path().join("vault");
    let day_file = vault.join("2026").join("20261009.md");
    write_toggle_task_settings(&vault);
    write_file(&day_file, pomodoros);
    write_file(
        &vault.join("bob.md"),
        concat!(
            "## Tasks\n",
            "\n",
            "- [ ] #task Capture stop [created::2026-10-01] ^capture-stop\n",
            "- [ ] #task Web capture [created::2026-10-01] ^web-capture\n",
            "- [ ] #task Ready task [created::2026-10-01] ^ready\n",
        ),
    );
    (temp, vault, day_file)
}

fn base_capture() -> String {
    concat!(
        "## Pomodoros\n",
        "\n",
        "- [x] (**0850-0915** [t:: 25m]) — EARLIER\n",
        "- [ ] (**0920-0950** [t:: 30m]) — CAPTURE\n",
        "  - [[bob#^capture-stop]]\n",
        "    - Design details to retain\n",
        "  - [[bob#^web-capture]]#\n",
        "- [ ] () — SASE\n",
    )
    .to_string()
}

#[test]
fn pomodoro_reset_clears_note_free_session() {
    let (_temp, vault, day_file) =
        reset_vault("bob-cli-reset-basic", &base_capture());
    let json =
        run_close_json(&vault, &day_file, "2026-10-09 09:37:00", &["=x0"]);
    assert_eq!(json["ok"], true);
    assert_eq!(json["kind"], "pomodoro_reset");
    assert_eq!(json["placement"], "updated");
    assert!(json.get("pomodoro_close").is_none());
    let reset = &json["pomodoro_reset"];
    assert_eq!(reset["raw"], "=x0");
    assert_eq!(reset["pomodoro_name"], "CAPTURE");
    assert_eq!(reset["previous_pomodoro_line"], 4);
    assert_eq!(reset["pomodoro_line"], 4);
    assert_eq!(reset["time_range"], serde_json::Value::Null);
    assert_eq!(reset["created_pomodoro"], false);
    assert_eq!(reset["moved"], false);
    let after = fs::read_to_string(&day_file).expect("read day");
    assert!(after.contains("- [ ] () — CAPTURE"), "{after}");
    assert!(
        after.contains("  - [[bob#^capture-stop]]"),
        "task links kept: {after}"
    );
    assert!(
        after.contains("    - Design details to retain"),
        "nested details kept byte-for-byte: {after}"
    );
    assert!(!after.contains("0920-0950"), "timing cleared: {after}");
    // No completed history row and no extra placeholder.
    assert_eq!(after.matches("- [x]").count(), 1, "{after}");
    assert_eq!(after.matches("- [ ] ()").count(), 2, "{after}");
}

#[test]
fn pomodoro_reset_note_fallback_stays_close() {
    // The shared worked vault owns a direct `quick note`: `=x0` closes.
    let (_temp, vault, day_file) = close_worked_vault("bob-cli-reset-fallback");
    let json =
        run_close_json(&vault, &day_file, "2026-09-28 09:37:00", &["=x0"]);
    assert_eq!(json["kind"], "pomodoro_close");
    assert!(json.get("pomodoro_reset").is_none());
}

#[test]
fn pomodoro_reset_moves_before_earlier_future() {
    let (_temp, vault, day_file) = reset_vault(
        "bob-cli-reset-move",
        concat!(
            "## Pomodoros\n",
            "\n",
            "- [ ] () — EARLY\n",
            "- [ ] (**0920-0950** [t:: 30m]) — CAPTURE\n",
            "  - [[bob#^capture-stop]]\n",
            "- [ ] () — SASE\n",
        ),
    );
    let json =
        run_close_json(&vault, &day_file, "2026-10-09 09:37:00", &["=x0"]);
    assert_eq!(json["kind"], "pomodoro_reset");
    assert_eq!(json["pomodoro_reset"]["moved"], true);
    assert_eq!(json["pomodoro_reset"]["pomodoro_line"], 3);
    let after = fs::read_to_string(&day_file).expect("read day");
    let capture = after.find("— CAPTURE").expect("capture");
    let early = after.find("— EARLY").expect("early");
    let sase = after.find("— SASE").expect("sase");
    assert!(capture < early && early < sase, "{after}");
}

#[test]
fn pomodoro_reset_uppercase_and_modifiers() {
    let (_temp, vault, day_file) =
        reset_vault("bob-cli-reset-upper", &base_capture());
    let upper =
        run_close_json(&vault, &day_file, "2026-10-09 09:37:00", &["=X0"]);
    assert_eq!(upper["kind"], "pomodoro_reset");

    // Explicit modifiers keep close semantics: `=x0*2` parks task 2
    // and closes instead of resetting.
    let (_temp, vault, day_file) =
        reset_vault("bob-cli-reset-mod", &base_capture());
    let modified =
        run_close_json(&vault, &day_file, "2026-10-09 09:37:00", &["=x0*2"]);
    assert_eq!(modified["kind"], "pomodoro_close");
    assert!(modified.get("pomodoro_reset").is_none());
    assert_eq!(modified["pomodoro_close"]["park"], serde_json::json!([2]));

    // Out-of-range modifier errors atomically without writes.
    let (_temp, vault, day_file) =
        reset_vault("bob-cli-reset-bad", &base_capture());
    let before = fs::read_to_string(&day_file).expect("read day");
    let error = run_close_expect_error(
        &vault,
        &day_file,
        "2026-10-09 09:37:00",
        &["=x0*9"],
    );
    assert!(error.contains("names task 9"), "{error}");
    let after = fs::read_to_string(&day_file).expect("read day");
    assert_eq!(before, after, "failed modifier writes nothing");
}
