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
    let raw = "x a";
    let menu = result(temp.path(), raw, raw.len());

    assert_eq!(menu.context, Some(CompletionContext::WorkLog));
    assert_eq!(menu.query.as_deref(), Some("a"));
    assert_eq!(menu.initial_selection, Some("none"));
    assert!(menu.candidates.is_empty());
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
