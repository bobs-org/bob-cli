use super::{
    super::{
        model::{
            Candidates, CaptureCompleteResult, Replacement, SCHEMA_VERSION,
        },
        pomodoros::{
            apply_bare_start_name_replacements, pomodoro_name_candidates_at,
            pomodoro_name_candidates_from_entries,
            pomodoro_name_candidates_from_scan,
            pomodoro_start_name_candidates_at,
            pomodoro_start_name_candidates_from_scan,
        },
        render::candidate_lines,
    },
    result, write_file, TempDir,
};
use crate::native::{
    capture_language::{self, CompletionContext},
    capture_pomodoros::{self, PomodoroState},
};
fn named_start_ledger() -> capture_pomodoros::PomodoroScan {
    capture_pomodoros::scan(concat!(
        "# 2026-07-10\n",
        "\n",
        "## Pomodoros\n",
        "\n",
        "- [x] (**0830-0855** [t:: 25m]) — PLAN\n",
        "  - 🍅 [[sase#^plan-day]]\n",
        "- [ ] () — BUGS\n",
        "  - [[sase#^deep-fix]]\n",
        "- [ ] () — DEEP WORK\n",
        "  - [[bob#^outline]]\n",
        "  - [[bob#^draft]]\n",
        "- [ ] ()\n",
        "  - [[bob#^inbox-zero]]\n",
    ))
}

#[test]
fn pomodoro_past_name_fields_preserve_suffixes_and_chain_siblings() {
    for (marked, query, replaced, bare) in [
        ("x=|", "", "", true),
        ("x=#|", "", "", false),
        ("x=3#|", "", "", false),
        ("x=fi|x!", "fi", "fix", true),
        ("x=#fi|x=*3", "fi", "fix", false),
        ("x=3#fi|x!=1!2", "fi", "fix", false),
        ("x=fi|x@2!=*3", "fi", "fix", true),
        ("x=fi|x~2", "fi", "fix", true),
        ("x=#\"DEEP |WORK\"@3!", "DEEP ", "\"DEEP WORK\"", false),
        ("x=#\"DEEP|", "DEEP", "\"DEEP", false),
        ("x=#\"FIX!=@|\"=1", "FIX!=@", "\"FIX!=@\"", false),
        ("x=#“CAFÉ”|@3", "CAFÉ", "“CAFÉ”", false),
        ("=3 x=fi|x@2 x=build", "fi", "fix", true),
        ("x=build\n\nx=3#fi|x!", "fi", "fix", false),
        ("  x=fi|x entry", "fi", "fix", true),
    ] {
        let cursor = marked.find('|').unwrap();
        let raw = marked.replace('|', "");
        let field = capture_language::completion_field_at(&raw, cursor)
            .unwrap_or_else(|| panic!("missing field: {marked}"));
        assert_eq!(
            field.context,
            CompletionContext::PomodoroPastName,
            "{marked}"
        );
        assert_eq!(field.query, query, "{marked}");
        assert_eq!(field.bare_name, bare, "{marked}");
        let start = if replaced.is_empty() {
            cursor
        } else {
            raw.find(replaced).unwrap()
        };
        assert_eq!(
            field.replacement,
            (start, start + replaced.len()),
            "{marked}"
        );
    }
    for marked in [
        "x|=fix",
        "x=|3#fix",
        "x=3|#fix",
        "x=3|",
        "x=fix!|",
        "x=fix@|3",
        "x=fix@3|",
        "x=fix=|",
        "x=fix=*3|",
        "x==~|",
        "x=3!=1!2|",
        "X=fi|x",
        "Plan x=fi|x",
        "x=fix log @|",
        "x=fix\n  - @|",
    ] {
        let cursor = marked.find('|').unwrap();
        let raw = marked.replace('|', "");
        assert!(
            capture_language::completion_field_at(&raw, cursor).is_none(),
            "unexpected field: {marked}"
        );
    }
}

#[test]
fn pomodoro_past_name_menu_excludes_running_targets_without_override() {
    let temp = TempDir::new("bob-cli-capture-complete-past-name");
    let day = temp.path().join("2026/20261104.md");
    write_file(
        &day,
        concat!(
            "## Pomodoros\n",
            "- [x] (**0800-0825** [t:: 25m]) — READ\n",
            "- [ ] (**0900-0925** [t:: 25m]) — BUILD\n",
            "- [ ] () — FIX\n",
            "- [ ] () — BUILD\n",
            "- [ ] () — DEEP WORK\n",
            "- [ ] () — 2FA\n",
        ),
    );
    crate::native::env::with_var("BOB_DAY_FILE", &day, || {
        let menu = result(temp.path(), "x=", 2);
        assert_eq!(menu.context, Some(CompletionContext::PomodoroPastName));
        assert!(menu.r#override.is_none());
        let json = serde_json::to_value(&menu).expect("completion JSON");
        assert_eq!(json["context"], "pomodoro_past_name");
        assert!(json.get("override").is_none());
        let Candidates::PomodoroName(rows) = &menu.candidates else {
            panic!("expected name candidates");
        };
        for name in ["FIX", "READ", "DEEP WORK", "2FA"] {
            assert!(
                rows.iter().any(|row| row.name.as_deref() == Some(name)),
                "{name}: {rows:?}"
            );
        }
        assert!(rows.iter().all(|row| row.name.as_deref() != Some("BUILD")));
        assert!(rows.iter().any(|row| row.replacement == "#2fa"));
        assert!(rows.iter().any(|row| row.replacement == "#deep-work"));
        assert!(rows
            .iter()
            .any(|row| row.name.as_deref() == Some("READ")
                && row.creates_pomodoro));
        for raw in ["x=#build", "x=#\"BUILD\""] {
            let menu = result(temp.path(), raw, raw.len());
            let Candidates::PomodoroName(rows) = menu.candidates else {
                panic!("expected name candidates");
            };
            assert!(
                rows.is_empty(),
                "running target offered for {raw}: {rows:?}"
            );
            assert!(menu.r#override.is_none());
        }
        // A typed prefix may create a distinct name when its accepted
        // replacement is quoted, so it cannot select the running BUILD.
        let menu = result(temp.path(), "x=bui", 5);
        let Candidates::PomodoroName(rows) = menu.candidates else {
            panic!("expected name candidates");
        };
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].replacement, "#\"BUI\"");
        assert!(rows[0].creates_pomodoro);
        let raw = "x=#\"DEEP W";
        let menu = result(temp.path(), raw, raw.len());
        let Candidates::PomodoroName(rows) = menu.candidates else {
            panic!("expected name candidates");
        };
        assert_eq!(rows[0].replacement, "\"DEEP WORK\"");
        let shell = super::super::shell::shell_completion(temp.path(), "x=", 2)
            .expect("shell completion")
            .expect("past name field");
        assert!(shell.rows.iter().any(|row| row.full == "x=fix"));
        assert!(shell.rows.iter().all(|row| row.full != "x=build"));
    });
}

#[test]
fn pomodoro_name_completion_lists_named_then_nameable_rows() {
    let scan = capture_pomodoros::scan(concat!(
        "## Pomodoros\n",
        "- [ ] (0900-0930) — MEMORY\n",
        "\t- [[dev#^focus]]\n",
        "- [ ] () — BUGS\n",
        "- [ ] () — MEMORY\n",
        "- [ ] ()\n",
        "- [ ] () — SNAKE_CASE\n",
        "- [x] () — DONE\n",
    ));

    let candidates = pomodoro_name_candidates_from_entries(&scan.entries, "");
    let rows = candidates
        .iter()
        .map(|candidate| {
            (
                candidate.replacement.as_str(),
                candidate.name.as_deref(),
                candidate.requires_name,
                candidate.creates_pomodoro,
                candidate.line,
                candidate.is_current,
                candidate.child_count,
                candidate.match_count,
            )
        })
        .collect::<Vec<_>>();

    assert_eq!(
        rows,
        vec![
            ("memory", Some("MEMORY"), false, false, Some(2), true, 1, 2),
            ("bugs", Some("BUGS"), false, false, Some(4), false, 0, 1),
            ("", None, true, false, Some(6), false, 0, 1),
            ("", Some("SNAKE_CASE"), true, false, Some(7), false, 0, 1),
        ]
    );
    assert_eq!(candidates[0].time_range.as_deref(), Some("0900-0930"));
    assert!(!candidates[0].placeholder);
    assert!(candidates[2].placeholder);
}

#[test]
fn pomodoro_name_completion_keeps_nameable_rows_for_a_query() {
    let scan = capture_pomodoros::scan(concat!(
        "## Pomodoros\n",
        "- [ ] () — MEMORY\n",
        "- [ ] () — BUGS\n",
        "- [ ] ()\n",
        "- [ ] () — SNAKE_CASE\n",
        "- [x] () — BUGS DONE\n",
    ));

    let candidates = pomodoro_name_candidates_from_entries(&scan.entries, "bu");
    let rows = candidates
        .iter()
        .map(|candidate| {
            (
                candidate.replacement.as_str(),
                candidate.name.as_deref(),
                candidate.requires_name,
                candidate.creates_pomodoro,
            )
        })
        .collect::<Vec<_>>();

    assert_eq!(
        rows,
        vec![
            ("bugs", Some("BUGS"), false, false),
            ("", None, true, false),
            ("", Some("SNAKE_CASE"), true, false),
        ]
    );
}

#[test]
fn pomodoro_name_completion_works_without_a_block_id() {
    let temp = TempDir::new("bob-cli-capture-complete-pomodoro-name");
    write_file(&temp.path().join("dev.md"), "---\ntype: [[area]]\n---\n");
    let day_file = temp.path().join("2026/20260828.md");
    write_file(&day_file, "## Pomodoros\n- [ ] () — BUGS\n- [ ] ()\n");

    let raw = "note @dev:#bu";
    let value = crate::native::env::with_var("BOB_DAY_FILE", &day_file, || {
        result(temp.path(), raw, raw.len())
    });

    assert_eq!(value.context, Some(CompletionContext::PomodoroName));
    assert_eq!(
        value.replacement,
        Replacement {
            start: raw.find('#').expect("hash") + 1,
            end: raw.len(),
        }
    );
    let Candidates::PomodoroName(candidates) = &value.candidates else {
        panic!("expected Pomodoro-name candidates");
    };
    assert_eq!(candidates[0].replacement, "bugs");
    assert_eq!(candidates[0].name.as_deref(), Some("BUGS"));
    assert!(!candidates[0].requires_name);
    assert!(!candidates[0].creates_pomodoro);
    assert_eq!(candidates[1].name.as_deref(), Some("BU"));
    assert!(candidates[1].creates_pomodoro);
    assert_eq!(candidates[1].replacement, "\"BU\"");
    assert!(candidates[2].requires_name);
    assert!(!candidates[2].creates_pomodoro);
}

#[test]
fn pomodoro_name_completion_keeps_today_before_creation_and_utilities() {
    let scan = capture_pomodoros::scan(concat!(
        "## Pomodoros\n",
        "- [ ] (0900-0930) — MEMORY\n",
        "- [ ] () — NETWORK\n",
        "- [ ] ()\n",
        "- [x] () — BUGS\n",
    ));

    let novel = pomodoro_name_candidates_from_scan(&scan, "future");
    assert_eq!(novel[0].replacement, "future");
    assert_eq!(novel[0].name.as_deref(), Some("FUTURE"));
    assert!(novel[0].creates_pomodoro);
    assert!(!novel[0].requires_name);
    assert!(novel[0].pomodoro_ref.is_none());
    assert!(novel[0].line.is_none());
    assert!(novel[0].placeholder);
    assert_eq!(novel[0].child_count, 0);
    assert!(novel.iter().skip(1).any(|row| row.requires_name));
    assert!(novel.iter().skip(1).all(|row| !row.creates_pomodoro));

    let completed_only = pomodoro_name_candidates_from_scan(&scan, "bugs");
    assert_eq!(completed_only[0].replacement, "bugs");
    assert_eq!(completed_only[0].name.as_deref(), Some("BUGS"));
    assert!(completed_only[0].creates_pomodoro);
    assert!(completed_only[0].pomodoro_ref.is_none());

    let substring_only = pomodoro_name_candidates_from_scan(&scan, "work");
    assert_eq!(substring_only[0].replacement, "network");
    assert!(!substring_only[0].creates_pomodoro);
    assert_eq!(substring_only[1].replacement, "work");
    assert_eq!(substring_only[1].name.as_deref(), Some("WORK"));
    assert!(substring_only[1].creates_pomodoro);
    assert!(substring_only[2].requires_name);
}

#[test]
fn pomodoro_name_completion_suppresses_creation_only_for_exact_open_names() {
    let scan = capture_pomodoros::scan(concat!(
        "## Pomodoros\n",
        "- [ ] (0900-0930) — MEMORY\n",
        "- [ ] () — BUGS\n",
        "- [ ] ()\n",
    ));

    for query in ["memory", "MEMORY"] {
        let candidates = pomodoro_name_candidates_from_scan(&scan, query);
        assert!(
            candidates.iter().all(|row| !row.creates_pomodoro),
            "{query}: {candidates:?}"
        );
        assert_eq!(candidates[0].replacement, "memory");
    }
}

#[test]
fn pomodoro_name_completion_skips_creation_for_empty_or_invalid_queries() {
    let scan = capture_pomodoros::scan(concat!(
        "## Pomodoros\n",
        "- [ ] () — MEMORY\n",
        "- [ ] ()\n",
    ));

    let empty = pomodoro_name_candidates_from_scan(&scan, "");
    assert!(!empty.is_empty());
    assert!(empty.iter().all(|row| !row.creates_pomodoro));

    let invalid = pomodoro_name_candidates_from_scan(&scan, "bad_id");
    assert!(invalid.iter().all(|row| !row.creates_pomodoro));
    assert!(invalid.iter().any(|row| row.requires_name));
}

#[test]
fn pomodoro_name_completion_treats_plus_names_as_named_not_nameable() {
    let existing = capture_pomodoros::scan(concat!(
        "## Pomodoros\n",
        "- [ ] () — C++\n",
        "- [ ] ()\n",
        "- [ ] () — SNAKE_CASE\n",
    ));
    let candidates = pomodoro_name_candidates_from_scan(&existing, "c+");
    assert_eq!(candidates[0].replacement, "c++");
    assert_eq!(candidates[0].name.as_deref(), Some("C++"));
    assert!(!candidates[0].requires_name);
    assert!(!candidates[0].creates_pomodoro);
    assert!(candidates.iter().any(|row| row.requires_name));
    assert!(candidates
        .iter()
        .filter(|row| row.replacement == "c++")
        .all(|row| !row.requires_name));

    let novel = capture_pomodoros::scan(concat!(
        "## Pomodoros\n",
        "- [ ] () — MEMORY\n",
        "- [ ] ()\n",
    ));
    let created = pomodoro_name_candidates_from_scan(&novel, "c++");
    assert_eq!(created[0].replacement, "c++");
    assert_eq!(created[0].name.as_deref(), Some("C++"));
    assert!(created[0].creates_pomodoro);
    assert!(!created[0].requires_name);

    let infix = pomodoro_name_candidates_from_scan(&novel, "bob+sase");
    assert_eq!(infix[0].replacement, "bob+sase");
    assert_eq!(infix[0].name.as_deref(), Some("BOB+SASE"));
    assert!(infix[0].creates_pomodoro);
}

#[test]
fn pomodoro_name_completion_skips_creation_when_the_ledger_cannot_place_it() {
    let missing_section = capture_pomodoros::scan("# Day\n");
    assert!(!missing_section.has_section);
    let skipped =
        pomodoro_name_candidates_from_scan(&missing_section, "future");
    assert!(skipped.is_empty());

    let ambiguous = capture_pomodoros::scan(concat!(
        "## Pomodoros\n",
        "- [ ] (0900-0930) — MEMORY\n",
        "- [ ] (1000-1030) — BUGS\n",
        "- [ ] ()\n",
    ));
    let candidates = pomodoro_name_candidates_from_scan(&ambiguous, "future");
    assert!(candidates.iter().all(|row| !row.creates_pomodoro));
    assert!(candidates.iter().any(|row| row.requires_name));

    let named_still_wins =
        pomodoro_name_candidates_from_scan(&ambiguous, "mem");
    assert_eq!(named_still_wins[0].replacement, "memory");
    assert!(!named_still_wins[0].creates_pomodoro);
}

#[test]
fn pomodoro_name_completion_missing_daily_note_warns() {
    let temp = TempDir::new("bob-cli-capture-complete-pomodoro-name-missing");
    let missing_day = temp.path().join("2026/20260828.md");

    let (candidates, warnings) =
        pomodoro_name_candidates_at(&missing_day, "").expect("warning success");

    assert_eq!(candidates.len(), 0);
    assert_eq!(warnings.len(), 1);
    assert!(warnings[0].contains("does not exist"));

    let sectionless = temp.path().join("2026/20260829.md");
    write_file(&sectionless, "# Day\n");
    let (candidates, warnings) =
        pomodoro_name_candidates_at(&sectionless, "").expect("warning success");

    assert_eq!(candidates.len(), 0);
    assert_eq!(warnings.len(), 1);
    assert!(warnings[0].contains("no Pomodoros section"));
}

/// The plan's worked-example ledger: a completed PLAN session, open
/// BUGS and DEEP WORK placeholders, and one unnamed placeholder.

#[test]
fn pomodoro_start_name_lists_start_again_and_name_it_rows() {
    let scan = named_start_ledger();
    let candidates = pomodoro_start_name_candidates_from_scan(&scan, "");
    let rows = candidates
        .iter()
        .map(|candidate| {
            (
                candidate.replacement.as_str(),
                candidate.name.as_deref(),
                candidate.requires_name,
                candidate.creates_pomodoro,
                candidate.state,
                candidate.time_range.as_deref(),
                candidate.next_up,
                candidate.child_count,
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(
        rows,
        vec![
            (
                "bugs",
                Some("BUGS"),
                false,
                false,
                PomodoroState::Open,
                None,
                true,
                1
            ),
            (
                "deep-work",
                Some("DEEP WORK"),
                false,
                false,
                PomodoroState::Open,
                None,
                false,
                2
            ),
            (
                "plan",
                Some("PLAN"),
                false,
                true,
                PomodoroState::Completed,
                Some("0830-0855"),
                false,
                1
            ),
            ("", None, true, false, PomodoroState::Open, None, false, 1),
        ]
    );
    assert_eq!(candidates[0].line, Some(7));
    assert_eq!(candidates[1].line, Some(9));
    assert_eq!(candidates[2].line, Some(5));
    assert!(candidates[2].pomodoro_ref.is_some());
    assert!(candidates.iter().filter(|row| row.next_up).count() == 1);
    assert!(
        candidates.iter().all(|row| !row.creates_pomodoro
            || row.state == PomodoroState::Completed),
        "{candidates:?}"
    );
}

#[test]
fn pomodoro_start_name_filters_and_creates_by_query() {
    let scan = named_start_ledger();

    let prefix = pomodoro_start_name_candidates_from_scan(&scan, "de");
    let prefix_rows = prefix
        .iter()
        .map(|candidate| candidate.replacement.as_str())
        .collect::<Vec<_>>();
    assert_eq!(prefix_rows, vec!["deep-work", "\"DE\"", ""]);

    let completed = pomodoro_start_name_candidates_from_scan(&scan, "pl");
    let completed_rows = completed
        .iter()
        .map(|candidate| candidate.replacement.as_str())
        .collect::<Vec<_>>();
    assert_eq!(completed_rows, vec!["plan", "\"PL\"", ""]);
    assert!(completed[0].creates_pomodoro);
    assert_eq!(completed[0].state, PomodoroState::Completed);

    let novel = pomodoro_start_name_candidates_from_scan(&scan, "rev");
    let novel_rows = novel
        .iter()
        .map(|candidate| candidate.replacement.as_str())
        .collect::<Vec<_>>();
    assert_eq!(novel_rows, vec!["rev", ""]);
    assert_eq!(novel[0].name.as_deref(), Some("REV"));
    assert!(novel[0].creates_pomodoro);
    assert!(novel[0].pomodoro_ref.is_none());
    assert!(novel[0].line.is_none());
    assert!(!novel[0].next_up);
}

#[test]
fn pomodoro_start_name_rows_carry_no_budget_preview() {
    // The type cap is gone: create and again rows no longer preview a
    // themes budget.
    let scan = named_start_ledger();
    let candidates = pomodoro_start_name_candidates_from_scan(&scan, "");
    let again = candidates
        .iter()
        .find(|row| row.replacement == "plan")
        .expect("again PLAN row");
    assert!(again.creates_pomodoro);
    let novel = pomodoro_start_name_candidates_from_scan(&scan, "fresh");
    let created = novel
        .iter()
        .find(|row| row.replacement == "fresh")
        .expect("new FRESH row");
    assert!(created.creates_pomodoro);
    let value = serde_json::to_value(&candidates).expect("candidates JSON");
    for row in value.as_array().expect("candidate array") {
        assert!(row.get("plan_themes_after").is_none());
        assert!(row.get("plan_themes_cap").is_none());
    }
}

#[test]
fn pomodoro_start_name_puts_the_running_entry_last() {
    let scan = capture_pomodoros::scan(concat!(
        "## Pomodoros\n",
        "- [x] (**0830-0855** [t:: 25m]) — PLAN\n",
        "- [ ] (**0840-0905** [t:: 25m]) — BUGS\n",
        "  - [[sase#^deep-fix]]\n",
        "- [ ] () — DEEP WORK\n",
        "  - [[bob#^outline]]\n",
        "  - [[bob#^draft]]\n",
        "- [ ] ()\n",
        "  - [[bob#^inbox-zero]]\n",
    ));
    let candidates = pomodoro_start_name_candidates_from_scan(&scan, "");
    let rows = candidates
        .iter()
        .map(|candidate| {
            (
                candidate.replacement.as_str(),
                candidate.name.as_deref(),
                candidate.requires_name,
                candidate.creates_pomodoro,
                candidate.state,
                candidate.time_range.as_deref(),
                candidate.next_up,
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(
        rows,
        vec![
            (
                "deep-work",
                Some("DEEP WORK"),
                false,
                false,
                PomodoroState::Open,
                None,
                true
            ),
            (
                "plan",
                Some("PLAN"),
                false,
                true,
                PomodoroState::Completed,
                Some("0830-0855"),
                false
            ),
            ("", None, true, false, PomodoroState::Open, None, false),
            (
                "bugs",
                Some("BUGS"),
                false,
                false,
                PomodoroState::Open,
                Some("0840-0905"),
                false
            ),
        ]
    );
}

#[test]
fn pomodoro_start_name_matches_pomodoro_name_warnings() {
    let temp = TempDir::new("bob-cli-capture-complete-start-name-warns");
    let missing_day = temp.path().join("2026/20260828.md");
    let (candidates, warnings) =
        pomodoro_start_name_candidates_at(&missing_day, "")
            .expect("warning success");
    assert_eq!(candidates.len(), 0);
    assert_eq!(warnings.len(), 1);
    assert!(warnings[0].contains("does not exist"));

    let sectionless = temp.path().join("2026/20260829.md");
    write_file(&sectionless, "# Day\n");
    let (candidates, warnings) =
        pomodoro_start_name_candidates_at(&sectionless, "")
            .expect("warning success");
    assert_eq!(candidates.len(), 0);
    assert_eq!(warnings.len(), 1);
    assert!(warnings[0].contains("no Pomodoros section"));

    // Several open timed entries leave no place for a new session.
    let ambiguous = capture_pomodoros::scan(concat!(
        "## Pomodoros\n",
        "- [ ] (0900-0930) — MEMORY\n",
        "- [ ] (1000-1030) — BUGS\n",
        "- [ ] ()\n",
    ));
    let candidates =
        pomodoro_start_name_candidates_from_scan(&ambiguous, "future");
    assert!(
        candidates.iter().all(|row| row.replacement != "future"),
        "{candidates:?}"
    );
}

#[test]
fn pomodoro_start_name_human_labels_cover_every_row_kind() {
    let scan = named_start_ledger();
    let candidates = pomodoro_start_name_candidates_from_scan(&scan, "");
    assert_eq!(
        candidate_lines(
            &Candidates::PomodoroName(candidates),
            Some(CompletionContext::PomodoroStartName),
        ),
        vec![
            ("BUGS".to_string(), "next up · 1 link".to_string()),
            ("DEEP WORK".to_string(), "planned · 2 links".to_string()),
            ("PLAN".to_string(), "again · last 0830-0855".to_string()),
            ("unnamed".to_string(), "name it · 1 link".to_string()),
        ]
    );

    let novel = pomodoro_start_name_candidates_from_scan(&scan, "rev");
    assert_eq!(
        candidate_lines(
            &Candidates::PomodoroName(novel),
            Some(CompletionContext::PomodoroStartName),
        )[0],
        ("REV".to_string(), "new session".to_string()),
    );

    let running = capture_pomodoros::scan(concat!(
        "## Pomodoros\n",
        "- [ ] (0840-0905) — BUGS\n",
        "- [ ] () — DEEP WORK\n",
        "- [ ] ()\n",
    ));
    let running_candidates =
        pomodoro_start_name_candidates_from_scan(&running, "");
    let lines = candidate_lines(
        &Candidates::PomodoroName(running_candidates),
        Some(CompletionContext::PomodoroStartName),
    );
    assert_eq!(
        lines.last().expect("running row"),
        &("BUGS".to_string(), "running 0840-0905".to_string()),
    );
}

#[test]
fn pomodoro_name_candidates_omit_next_up() {
    // The shared `pomodoro_name` context never sets `next_up`, so its
    // JSON stays byte-identical now that the field exists.
    let scan = named_start_ledger();
    let candidates = pomodoro_name_candidates_from_scan(&scan, "");
    let json = serde_json::to_string(&candidates).expect("json");
    assert!(!json.contains("next_up"), "{json}");
}

#[test]
fn pomodoro_name_human_rows_include_time_and_badges() {
    let scan = capture_pomodoros::scan(concat!(
        "## Pomodoros\n",
        "- [ ] (0900-0930) — MEMORY\n",
        "- [ ] () — MEMORY\n",
        "- [ ] ()\n",
    ));
    let candidates = Candidates::PomodoroName(
        pomodoro_name_candidates_from_entries(&scan.entries, ""),
    );

    let lines =
        candidate_lines(&candidates, Some(CompletionContext::PomodoroName));

    assert_eq!(lines[0].0, "MEMORY");
    assert_eq!(lines[0].1, "memory  0900-0930  current 2 matches");
    assert_eq!(lines[1].0, "unnamed");
    assert_eq!(lines[1].1, "-  planned  name it");
}

#[test]
fn pomodoro_name_human_rows_badge_creation() {
    let scan = capture_pomodoros::scan(concat!(
        "## Pomodoros\n",
        "- [ ] (0900-0930) — MEMORY\n",
        "- [ ] ()\n",
    ));
    let candidates = Candidates::PomodoroName(
        pomodoro_name_candidates_from_scan(&scan, "future"),
    );
    let lines =
        candidate_lines(&candidates, Some(CompletionContext::PomodoroName));

    assert_eq!(lines[0].0, "FUTURE");
    assert_eq!(lines[0].1, "future  planned  create");
    assert!(lines.iter().any(|line| line.1.contains("name it")));
}

#[test]
fn pomodoro_creation_json_omits_ref_and_keeps_schema_version() {
    let scan = capture_pomodoros::scan(
        "## Pomodoros\n- [ ] (1205-1230) — MEMORY\n- [ ] ()\n",
    );
    let creation = pomodoro_name_candidates_from_scan(&scan, "future")
        .into_iter()
        .find(|row| row.creates_pomodoro)
        .expect("creation row");
    let json = serde_json::to_value(CaptureCompleteResult {
        ok: true,
        schema_version: SCHEMA_VERSION,
        grammar: capture_language::GRAMMAR_REVISION,
        cursor: 10,
        replacement: Replacement { start: 9, end: 10 },
        context: Some(CompletionContext::PomodoroName),
        initial_selection: None,
        subject: None,
        target: None,
        candidates: Candidates::PomodoroName(vec![creation]),
        theme_suggestion: None,
        r#override: None,
        block_id: None,
        warnings: Vec::new(),
        query: None,
        owner: None,
        relation: None,
        relation_cancel: None,
        relation_complete: None,
        relation_merge: None,
        relation_successor: None,
        picker: None,
        hidden_count: None,
        one_off: None,
        project_task_move: None,
    })
    .expect("creation json");

    assert_eq!(json["schema_version"], 1);
    assert_eq!(json["candidates"][0]["replacement"], "future");
    assert_eq!(json["candidates"][0]["name"], "FUTURE");
    assert_eq!(json["candidates"][0]["creates_pomodoro"], true);
    assert_eq!(json["candidates"][0]["requires_name"], false);
    assert_eq!(json["candidates"][0]["placeholder"], true);
    assert!(json["candidates"][0].get("ref").is_none());
    assert!(json["candidates"][0].get("line").is_none());
}

#[test]
fn bare_start_name_replacements_prefix_hash_when_the_slug_cannot_be_bare() {
    let scan = capture_pomodoros::scan(concat!(
        "## Pomodoros\n",
        "- [ ] () — 2FA\n",
        "- [ ] () — X2\n",
        "- [ ] () — BUGS\n",
    ));
    let items = pomodoro_start_name_candidates_from_scan(&scan, "");
    let mut candidates = Candidates::PomodoroName(items);
    apply_bare_start_name_replacements(&mut candidates);
    let Candidates::PomodoroName(items) = candidates else {
        panic!("expected pomodoro-name candidates");
    };
    let replacements: Vec<&str> =
        items.iter().map(|row| row.replacement.as_str()).collect();
    assert!(replacements.contains(&"#2fa"), "{replacements:?}");
    assert!(replacements.contains(&"#x2"), "{replacements:?}");
    assert!(replacements.contains(&"bugs"), "{replacements:?}");
    assert!(!replacements.contains(&"2fa"), "{replacements:?}");
    assert!(!replacements.contains(&"x2"), "{replacements:?}");
}

#[test]
fn pomodoro_start_name_orders_repeated_completed_names_newest_first() {
    // Ledger order A, B, A: ALPHA is the most recent session, so its
    // again row must precede BETA's in both JSON and shell rows.
    let ledger = concat!(
        "## Pomodoros\n",
        "- [x] (**0800-0825** [t:: 25m]) — ALPHA\n",
        "- [x] (**0830-0855** [t:: 25m]) — BETA\n",
        "- [x] (**0900-0925** [t:: 25m]) — ALPHA\n",
    );
    let scan = capture_pomodoros::scan(ledger);
    let candidates = pomodoro_start_name_candidates_from_scan(&scan, "");
    let json = serde_json::to_value(CaptureCompleteResult {
        ok: true,
        schema_version: SCHEMA_VERSION,
        grammar: capture_language::GRAMMAR_REVISION,
        cursor: 2,
        replacement: Replacement { start: 0, end: 2 },
        context: Some(CompletionContext::PomodoroStartName),
        initial_selection: None,
        subject: None,
        target: None,
        candidates: Candidates::PomodoroName(candidates.clone()),
        theme_suggestion: None,
        r#override: None,
        block_id: None,
        warnings: Vec::new(),
        query: None,
        owner: None,
        relation: None,
        relation_cancel: None,
        relation_complete: None,
        relation_merge: None,
        relation_successor: None,
        picker: None,
        hidden_count: None,
        one_off: None,
        project_task_move: None,
    })
    .expect("start-name json");
    let names = json["candidates"]
        .as_array()
        .expect("candidates array")
        .iter()
        .filter_map(|row| row.get("name").and_then(serde_json::Value::as_str))
        .collect::<Vec<_>>();
    let alpha = names
        .iter()
        .position(|name| *name == "ALPHA")
        .expect("ALPHA row");
    let beta = names
        .iter()
        .position(|name| *name == "BETA")
        .expect("BETA row");
    assert!(alpha < beta, "ALPHA newest-first before BETA: {names:?}");

    let lines = candidate_lines(
        &Candidates::PomodoroName(candidates),
        Some(CompletionContext::PomodoroStartName),
    );
    let presented = lines
        .iter()
        .map(|(name, _)| name.as_str())
        .collect::<Vec<_>>();
    let alpha = presented
        .iter()
        .position(|name| *name == "ALPHA")
        .expect("ALPHA shell row");
    let beta = presented
        .iter()
        .position(|name| *name == "BETA")
        .expect("BETA shell row");
    assert!(
        alpha < beta,
        "ALPHA newest-first before BETA: {presented:?}"
    );
}
