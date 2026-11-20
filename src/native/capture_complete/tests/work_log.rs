use super::super::model::Candidates;
use super::{result, write_file, TempDir};
use crate::native::capture_language::{self, CompletionContext};

fn work_log_vault() -> TempDir {
    let temp = TempDir::new("bob-cli-capture-complete-work-log");
    write_file(
        &temp.path().join("task.md"),
        concat!(
            "- [ ] Task ^task\n",
            "  - 🛠️ **WORK LOG**\n",
            "    - *2026-11-18 09:00* — auth retry\n",
            "    - *2026-11-19 08:00* — auth config cleanup\n",
            "    - *2026-11-19 08:15* — auth timeout recovery\n",
            "    - *2026-11-19 08:30* — auth retry\n",
        ),
    );
    temp
}

#[test]
fn work_log_completion_uses_recent_work_phrases_without_selection() {
    let temp = work_log_vault();
    let raw = "x auth";
    let menu = result(temp.path(), raw, raw.len());

    assert_eq!(menu.context, Some(CompletionContext::WorkLog));
    assert_eq!(menu.query.as_deref(), Some("auth"));
    assert_eq!(menu.initial_selection, Some("none"));
    assert_eq!(
        menu.replacement,
        super::super::model::Replacement {
            start: 2,
            end: raw.len(),
        }
    );
    let Candidates::WorkLog(rows) = menu.candidates else {
        panic!("expected Work Log phrase candidates");
    };
    assert_eq!(rows.len(), 3);
    assert_eq!(rows[0].text, "auth retry");
    assert_eq!(rows[0].replacement, "auth retry");
    assert_eq!(rows[0].uses, 2);
    assert_eq!(rows[0].last_used, "2026-11-19 08:30");
    assert_eq!(
        rows[0].matched,
        Some(crate::native::log_history::MatchKind::Prefix)
    );
}

#[test]
fn work_log_completion_waits_for_two_typed_characters() {
    let temp = work_log_vault();
    for raw in ["x a", "x a "] {
        let menu = result(temp.path(), raw, raw.len());
        assert_eq!(menu.context, None, "{raw:?}");
        assert!(menu.candidates.is_empty(), "{raw:?}");
    }
}

#[test]
fn work_log_completion_queries_only_text_before_the_cursor() {
    let temp = work_log_vault();
    let raw = "x auth retry";
    let cursor = raw.find("au").expect("au") + "au".len();
    let field = capture_language::work_log_completion_field_at(raw, cursor)
        .expect("missing Work Log field");
    assert_eq!(field.context, CompletionContext::WorkLog);
    assert_eq!(field.query, "au");
    assert_eq!(field.replacement, (2, raw.len()));
    let menu = result(temp.path(), raw, cursor);
    assert_eq!(menu.context, Some(CompletionContext::WorkLog));
    assert_eq!(menu.query.as_deref(), Some("au"));
    assert_eq!(
        menu.replacement,
        super::super::model::Replacement {
            start: 2,
            end: raw.len(),
        }
    );
}

#[test]
fn work_log_completion_drops_the_exact_current_summary() {
    let temp = work_log_vault();
    let raw = "x Auth  Retry";
    let menu = result(temp.path(), raw, raw.len());
    assert_eq!(menu.context, None, "{menu:?}");
    assert!(menu.candidates.is_empty(), "{menu:?}");
    if let Candidates::WorkLog(rows) = &menu.candidates {
        assert!(rows.iter().all(|row| row.key != "auth retry"), "{rows:?}");
    }
}

#[test]
fn work_log_completion_caps_at_eight_rows() {
    let temp = TempDir::new("bob-cli-capture-complete-work-log-cap");
    let mut log = String::from("- [ ] Task ^task\n  - 🛠️ **WORK LOG**\n");
    for n in 1..=12 {
        log.push_str(&format!(
            "    - *2026-11-19 08:{n:02}* — auth phrase {n}\n"
        ));
    }
    write_file(&temp.path().join("task.md"), &log);
    let raw = "x auth";
    let menu = result(temp.path(), raw, raw.len());
    assert_eq!(menu.context, Some(CompletionContext::WorkLog));
    let Candidates::WorkLog(rows) = menu.candidates else {
        panic!("expected Work Log phrase candidates");
    };
    assert_eq!(rows.len(), 8);
}

#[test]
fn work_log_completion_ignores_summaries_with_wikilinks() {
    let temp = work_log_vault();
    let raw = "x auth [[retry]]";
    assert!(
        capture_language::work_log_completion_field_at(raw, raw.len())
            .is_none(),
        "{raw:?}"
    );
    let menu = result(temp.path(), raw, raw.len());
    assert_ne!(menu.context, Some(CompletionContext::WorkLog), "{menu:?}");
}

#[test]
fn inline_bullet_and_past_summaries_replace_only_the_entry_text() {
    for raw in [
        "x wired the lexer",
        "x\n- 1 wired the lexer",
        "x=3=1 wired the lexer",
    ] {
        let summary = "wired the lexer";
        let start = raw.find(summary).expect("summary start");
        let field =
            capture_language::work_log_completion_field_at(raw, raw.len())
                .unwrap_or_else(|| {
                    panic!("missing Work Log field for {raw:?}")
                });
        assert_eq!(field.context, CompletionContext::WorkLog, "{raw:?}");
        assert_eq!(field.query, summary, "{raw:?}");
        assert_eq!(
            field.replacement,
            (start, start + summary.len()),
            "{raw:?}"
        );
    }
}
