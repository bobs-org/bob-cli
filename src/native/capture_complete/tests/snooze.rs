use super::{
    super::model::{Candidates, SnoozeCandidate},
    result, write_file, TempDir,
};
use crate::native::{capture_language::CompletionContext, env::TestEnvGuard};
use std::ffi::OsStr;

fn snooze_rows(
    value: &super::super::model::CaptureCompleteResult,
) -> &[SnoozeCandidate] {
    let Candidates::Snooze(rows) = &value.candidates else {
        panic!("expected snooze candidates");
    };
    rows
}

fn with_clock<T>(
    day_file: &std::path::Path,
    now: &str,
    f: impl FnOnce() -> T,
) -> T {
    let _guard = TestEnvGuard::set(&[
        ("BOB_NOW", Some(OsStr::new(now))),
        ("BOB_DAY_FILE", Some(day_file.as_os_str())),
        ("TZ", Some(OsStr::new("UTC"))),
        (
            "BOB_CONFIG_FILE",
            Some(OsStr::new("/nonexistent/bob-cli-test-config.yml")),
        ),
    ]);
    f()
}

fn completion(
    root: &std::path::Path,
    raw: &str,
) -> super::super::model::CaptureCompleteResult {
    result(root, raw, raw.len())
}

#[test]
fn event_ends_then_relative_then_used_times_are_deduplicated_in_order() {
    let temp = TempDir::new("bob-cli-capture-complete-snooze-order");
    let day = temp.path().join("2026/20261020.md");
    write_file(
        &day,
        concat!(
            "## Events\n",
            "- [ ] #task **1215-1245** Running interview ^running\n",
            "- [ ] #task **1300-1320** Yogi interview ^yogi\n",
            "- [x] #task **1320-1340** Closed event ^closed\n",
            "- [-] #task **1330-1350** Cancelled event ^cancelled\n",
            "- **1345-1400** Client call\n",
            "- **all day** Holiday\n",
            "\n## Snoozed\n",
            "- **1500** [[jobs#^later]]\n",
            "- **1230** [[jobs#^duplicate]]\n",
        ),
    );
    let raw = "Remember z:";
    let value = with_clock(&day, "2026-10-20 12:00:00", || {
        completion(temp.path(), raw)
    });

    assert_eq!(value.context, Some(CompletionContext::Snooze));
    assert_eq!(value.query.as_deref(), Some(""));
    assert_eq!(value.initial_selection, Some("none"));
    let rows = snooze_rows(&value);
    assert_eq!(
        rows.iter()
            .map(|row| row.replacement.as_str())
            .collect::<Vec<_>>(),
        ["1245", "1320", "1400", "1230", "1300", "1500"]
    );
    assert_eq!(rows[0].label, "12:45 · after Running interview");
    assert_eq!(rows[1].label, "13:20 · after Yogi interview");
    assert_eq!(rows[2].label, "14:00 · after Client call");
    assert_eq!(rows[3].label, "+30m · 12:30");
    assert_eq!(rows[4].label, "+1h · 13:00");
    assert_eq!(rows[5].label, "15:00 · used today");

    let json = serde_json::to_value(&value).expect("completion JSON");
    assert_eq!(json["candidates"][0]["replacement"], "1245");
    assert_eq!(
        json["candidates"][0]["label"],
        "12:45 · after Running interview"
    );
    assert!(json["candidates"][0].get("filter_aliases").is_none());
}

#[test]
fn typed_prefix_filters_candidates_without_changing_their_group_order() {
    let temp = TempDir::new("bob-cli-capture-complete-snooze-prefix");
    let day = temp.path().join("2026/20261020.md");
    write_file(
        &day,
        concat!(
            "## Events\n",
            "- **1240-1320** Yogi interview\n",
            "\n## Snoozed\n",
            "- **1500** [[jobs#^later]]\n",
        ),
    );
    let relative = "Remember z:+1";
    let relative_result = with_clock(&day, "2026-10-20 12:00:00", || {
        completion(temp.path(), relative)
    });
    let relative_rows = snooze_rows(&relative_result);
    assert_eq!(relative_result.query.as_deref(), Some("+1"));
    assert_eq!(relative_rows.len(), 1);
    assert_eq!(relative_rows[0].replacement, "1300");
    assert_eq!(relative_rows[0].label, "+1h · 13:00");

    let numeric = "Remember z:13";
    let numeric_result = with_clock(&day, "2026-10-20 12:00:00", || {
        completion(temp.path(), numeric)
    });
    let numeric_rows = snooze_rows(&numeric_result);
    assert_eq!(
        numeric_rows
            .iter()
            .map(|row| row.replacement.as_str())
            .collect::<Vec<_>>(),
        ["1320", "1300"]
    );
    assert_eq!(numeric_rows[0].label, "13:20 · after Yogi interview");
    assert_eq!(numeric_result.initial_selection, None);
}

#[test]
fn relative_suggestions_near_midnight_drop_overflow_and_nonfuture_rows() {
    let temp = TempDir::new("bob-cli-capture-complete-snooze-midnight");
    let day = temp.path().join("2026/20261020.md");
    write_file(
        &day,
        concat!(
            "## Events\n",
            "- **2300-2345** Late call\n",
            "- **2350-0010** Overnight event\n",
            "- **all day** Holiday\n",
            "\n## Snoozed\n",
            "- **2355** [[jobs#^later]]\n",
            "- **2315** [[jobs#^past]]\n",
        ),
    );
    let value = with_clock(&day, "2026-10-20 23:20:00", || {
        completion(temp.path(), "Remember z:")
    });
    let rows = snooze_rows(&value);
    assert_eq!(
        rows.iter()
            .map(|row| row.replacement.as_str())
            .collect::<Vec<_>>(),
        ["2345", "2350", "2355"]
    );
    assert_eq!(rows[1].label, "+30m · 23:50");
}

#[test]
fn deduplicated_event_row_keeps_relative_aliases() {
    // An event ending at 13:00 collides with `+1h` at a fixed 12:00 clock.
    // One candidate survives with the event label, yet `+1`/`+1h` still
    // match it through merged private aliases. A duplicate used-today 1300
    // row adds no output; a distinct 1245 event proves ordering.
    let temp = TempDir::new("bob-cli-capture-complete-snooze-dedup-alias");
    let day = temp.path().join("2026/20261020.md");
    write_file(
        &day,
        concat!(
            "## Events\n",
            "- **1215-1245** Standup\n",
            "- **1200-1300** Lunch\n",
            "\n## Snoozed\n",
            "- **1300** [[jobs#^later]]\n",
        ),
    );
    let value = with_clock(&day, "2026-10-20 12:00:00", || {
        completion(temp.path(), "Remember z:")
    });
    let rows = snooze_rows(&value);
    assert_eq!(
        rows.iter()
            .map(|row| row.replacement.as_str())
            .collect::<Vec<_>>(),
        ["1245", "1300", "1230", "1400"]
    );
    assert_eq!(rows[1].label, "13:00 · after Lunch");
    assert_eq!(
        rows.iter().filter(|row| row.replacement == "1300").count(),
        1,
        "duplicate minutes collapse to one row"
    );

    for query in ["", "13", "+1", "+1h"] {
        let raw = format!("Remember z:{query}");
        let filtered = with_clock(&day, "2026-10-20 12:00:00", || {
            completion(temp.path(), &raw)
        });
        let filtered_rows = snooze_rows(&filtered);
        let thirteens: Vec<_> = filtered_rows
            .iter()
            .filter(|row| row.replacement == "1300")
            .collect();
        assert_eq!(
            thirteens.len(),
            1,
            "{query:?} keeps exactly one 1300 candidate"
        );
        assert_eq!(
            thirteens[0].label, "13:00 · after Lunch",
            "{query:?} keeps the earliest event label"
        );
    }

    let plus_one = with_clock(&day, "2026-10-20 12:00:00", || {
        completion(temp.path(), "Remember z:+1")
    });
    let plus_rows = snooze_rows(&plus_one);
    assert_eq!(plus_rows.len(), 1);
    assert_eq!(plus_rows[0].replacement, "1300");
    assert_eq!(plus_rows[0].label, "13:00 · after Lunch");

    let plus_hour = with_clock(&day, "2026-10-20 12:00:00", || {
        completion(temp.path(), "Remember z:+1h")
    });
    let hour_rows = snooze_rows(&plus_hour);
    assert_eq!(hour_rows.len(), 1);
    assert_eq!(hour_rows[0].replacement, "1300");

    let numeric = with_clock(&day, "2026-10-20 12:00:00", || {
        completion(temp.path(), "Remember z:13")
    });
    let numeric_rows = snooze_rows(&numeric);
    assert_eq!(numeric_rows.len(), 1);
    assert_eq!(numeric_rows[0].replacement, "1300");
    assert_eq!(numeric_rows[0].label, "13:00 · after Lunch");

    let json = serde_json::to_value(&value).expect("completion JSON");
    for candidate in json["candidates"].as_array().expect("candidates") {
        assert!(
            candidate.get("filter_aliases").is_none(),
            "aliases stay private: {candidate}"
        );
        assert!(
            candidate["replacement"]
                .as_str()
                .is_some_and(|replacement| replacement.len() == 4),
            "replacement stays a literal HHMM: {candidate}"
        );
    }
}

#[test]
fn missing_daily_note_still_offers_valid_relative_times() {
    let temp = TempDir::new("bob-cli-capture-complete-snooze-missing-day");
    let missing = temp.path().join("missing.md");
    let value = with_clock(&missing, "2026-10-20 12:00:00", || {
        completion(temp.path(), "Remember z:")
    });
    assert_eq!(
        snooze_rows(&value)
            .iter()
            .map(|row| row.replacement.as_str())
            .collect::<Vec<_>>(),
        ["1230", "1300", "1400"]
    );
}
