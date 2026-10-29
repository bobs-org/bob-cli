//! Placement conformance vectors P1–P18 from `docs/freshness.md`,
//! plus parse-invariance through both Rust parsers and the hooks
//! preservation test. D = `2026-10-08`.

use chrono::NaiveDate;

use super::{
    mark_unblocked, prune_unblocked, read_freshness, set_refresh, stamp_fresh,
    stamp_fresh_preserve_keeps, tasks_suffix_start, Refusal, UnblockScope,
};
use crate::native::config::freshness::FreshnessConfig;

fn day() -> NaiveDate {
    NaiveDate::from_ymd_opt(2026, 10, 8).expect("valid D")
}

fn date(year: i32, month: u32, day: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(year, month, day).expect("valid date")
}

fn stamp(line: &str) -> super::Stamp {
    stamp_fresh(line, day())
}

#[test]
fn p01_bare_appends_fresh() {
    let stamped = stamp("- [ ] #task Buy milk");
    assert_eq!(stamped.line, "- [ ] #task Buy milk [fresh:: 2026-10-08]");
    assert!(stamped.changed);
    assert_eq!(stamped.refused, None);
}

#[test]
fn p02_created_suffix() {
    let stamped = stamp("- [ ] #task Buy milk [created::2026-09-29]");
    assert_eq!(
        stamped.line,
        "- [ ] #task Buy milk [fresh:: 2026-10-08] [created::2026-09-29]"
    );
    assert!(stamped.changed);
}

#[test]
fn p03_block_id_only() {
    let stamped = stamp("- [ ] #task Pick up Abby ^pickup");
    assert_eq!(
        stamped.line,
        "- [ ] #task Pick up Abby [fresh:: 2026-10-08] ^pickup"
    );
    assert!(stamped.changed);
}

#[test]
fn p04_interleaved_suffix() {
    let stamped = stamp(
        "- [ ] #task Plan trip [created::2026-08-26] #hide [priority:: high] ^trip",
    );
    assert_eq!(
        stamped.line,
        "- [ ] #task Plan trip [fresh:: 2026-10-08] [created::2026-08-26] #hide [priority:: high] ^trip"
    );
    assert!(stamped.changed);
}

#[test]
fn p05_unknown_field_stays_left() {
    let stamped = stamp(
        "- [ ] #task Read X [[#^h-8bac|🔖]] [h:: e629] [created::2026-08-28]",
    );
    assert_eq!(
        stamped.line,
        "- [ ] #task Read X [[#^h-8bac|🔖]] [h:: e629] [fresh:: 2026-10-08] [created::2026-08-28]"
    );
    assert!(stamped.changed);
}

#[test]
fn p06_spacing_head_collapses_suffix_untouched() {
    let stamped = stamp(
        "- [ ] #task Rahway  [created:: 2026-07-15]  [scheduled:: 2026-08-10] ^rahway",
    );
    assert_eq!(
        stamped.line,
        "- [ ] #task Rahway [fresh:: 2026-10-08] [created:: 2026-07-15]  [scheduled:: 2026-08-10] ^rahway"
    );
    assert!(stamped.changed);
}

#[test]
fn p07_restamp_replaces_date() {
    let stamped = stamp(
        "- [ ] #task Buy milk [fresh:: 2026-10-01] [created::2026-09-29]",
    );
    assert_eq!(
        stamped.line,
        "- [ ] #task Buy milk [fresh:: 2026-10-08] [created::2026-09-29]"
    );
    assert!(stamped.changed);
}

#[test]
fn p08_same_day_is_noop() {
    let line =
        "- [ ] #task Buy milk [fresh:: 2026-10-08] [created::2026-09-29]";
    let stamped = stamp(line);
    assert_eq!(stamped.line, line);
    assert!(!stamped.changed);
    assert_eq!(stamped.refused, None);
}

#[test]
fn p09_misplaced_is_repaired() {
    let input =
        "- [ ] #task Buy milk [created::2026-09-29] [fresh:: 2026-10-01]";
    let read = read_freshness(input, day());
    assert_eq!(read.fresh, Some(date(2026, 10, 1)));
    assert!(read.lints.contains(&"fresh_misplaced".to_string()));
    let stamped = stamp(input);
    assert_eq!(
        stamped.line,
        "- [ ] #task Buy milk [fresh:: 2026-10-08] [created::2026-09-29]"
    );
    assert!(stamped.changed);
}

#[test]
fn p10_duplicates_collapse_and_use_latest() {
    let input = "- [ ] #task A [fresh:: 2026-09-01] B [fresh:: 2026-09-20] [created::2026-09-01]";
    let read = read_freshness(input, day());
    assert_eq!(read.fresh, Some(date(2026, 9, 20)));
    assert!(read.lints.contains(&"fresh_duplicate".to_string()));
    let stamped = stamp(input);
    assert_eq!(
        stamped.line,
        "- [ ] #task A B [fresh:: 2026-10-08] [created::2026-09-01]"
    );
    assert!(stamped.changed);
}

#[test]
fn p11_refresh_follows_fresh() {
    let stamped = stamp(
        "- [ ] #task Rename queue input [refresh:: 14] [created::2026-09-10] [priority:: low]",
    );
    assert_eq!(
        stamped.line,
        "- [ ] #task Rename queue input [fresh:: 2026-10-08] [refresh:: 14] [created::2026-09-10] [priority:: low]"
    );
    assert!(stamped.changed);
    let read = read_freshness(&stamped.line, day());
    assert_eq!(read.refresh, Some(14));
}

#[test]
fn p12_set_and_clear_refresh() {
    let base =
        "- [ ] #task Buy milk [fresh:: 2026-10-08] [created::2026-09-29]";
    let set = set_refresh(base, Some(30), day());
    assert_eq!(
        set.line,
        "- [ ] #task Buy milk [fresh:: 2026-10-08] [refresh:: 30] [created::2026-09-29]"
    );
    assert!(set.changed);
    let cleared = set_refresh(&set.line, None, day());
    assert_eq!(cleared.line, base);
}

#[test]
fn p13_recurring_refused() {
    let input =
        "- [ ] #task Water plants [repeat:: every week] [created::2026-09-01]";
    let stamped = stamp(input);
    assert_eq!(stamped.line, input);
    assert!(!stamped.changed);
    assert_eq!(stamped.refused, Some(Refusal::Recurring));
}

#[test]
fn p14_global_filter_floor() {
    let first = stamp("- [ ] #task #hide ^x");
    assert_eq!(first.line, "- [ ] #task [fresh:: 2026-10-08] #hide ^x");
    let second = stamp("- [ ] #task #prj Ship it #hide ^prj");
    assert_eq!(
        second.line,
        "- [ ] #task #prj Ship it [fresh:: 2026-10-08] #hide ^prj"
    );
    // The `#task` token itself is never treated as suffix content.
    assert_eq!(
        tasks_suffix_start("- [ ] #task #hide ^x"),
        "- [ ] #task ".len()
    );
}

#[test]
fn p15_parenthesized_field() {
    let stamped = stamp("- [ ] #task Call mom (created:: 2026-09-01)");
    assert_eq!(
        stamped.line,
        "- [ ] #task Call mom [fresh:: 2026-10-08] (created:: 2026-09-01)"
    );
    assert!(stamped.changed);
}

#[test]
fn p16_indented_blocked() {
    let stamped = stamp(
        "\t- [?] #task Deferred [created::2026-09-01] [scheduled:: 2026-10-20]",
    );
    assert_eq!(
        stamped.line,
        "\t- [?] #task Deferred [fresh:: 2026-10-08] [created::2026-09-01] [scheduled:: 2026-10-20]"
    );
    assert!(stamped.changed);
}

#[test]
fn p17_done_refused() {
    let input = "- [x] #task Old [completion:: 2026-10-01]";
    let stamped = stamp(input);
    assert_eq!(stamped.line, input);
    assert!(!stamped.changed);
    assert_eq!(stamped.refused, Some(Refusal::Closed));
}

#[test]
fn p18_quoted_task_stamps_with_prefix_kept() {
    let stamped = stamp("> - [ ] #task Quoted [created::2026-09-01]");
    assert_eq!(
        stamped.line,
        "> - [ ] #task Quoted [fresh:: 2026-10-08] [created::2026-09-01]"
    );
    assert!(stamped.changed);
    assert_eq!(stamped.refused, None);
}

#[test]
fn p18_nested_quote_stamps() {
    let stamped = stamp(">> - [ ] #task Nested");
    assert_eq!(stamped.line, ">> - [ ] #task Nested [fresh:: 2026-10-08]");
    assert!(stamped.changed);
    assert_eq!(stamped.refused, None);
}

#[test]
fn p18_deeply_indented_quote_is_not_a_task() {
    let line = "    > - [ ] #task Not a quoted task";
    let stamped = stamp(line);
    assert_eq!(stamped.line, line);
    assert_eq!(stamped.refused, Some(Refusal::NotTask));
}

#[test]
fn non_task_lines_are_refused() {
    for line in [
        "Just a bullet",
        "- plain bullet without a checkbox",
        "#task not a task line",
        "- [ ]no space after checkbox",
        "- [  ] #task double status",
    ] {
        let stamped = stamp(line);
        assert_eq!(stamped.line, line, "must leave {line:?} alone");
        assert_eq!(stamped.refused, Some(Refusal::NotTask));
    }
}

#[test]
fn cancelled_is_refused_like_done() {
    let input = "- [-] #task Dropped [created::2026-09-01]";
    let stamped = stamp(input);
    assert_eq!(stamped.line, input);
    assert_eq!(stamped.refused, Some(Refusal::Closed));
}

#[test]
fn pomodoro_ledger_rows_are_refused_byte_for_byte_by_every_stamp_writer() {
    let rows = [
        "- [ ] (**0920-0950** [t:: 30m]) — FOCUS",
        "- [x] (**0920-0950** [t:: 30m]) — FOCUS [fresh:: 2026-10-01]",
        "- [ ] () — PLANNED",
        "- [ ] (09:20 - 09:50) — FOCUS",
        "- [ ] (0920 - 0950) [fresh:: 2026-10-01] [refresh:: 14] [keeps:: 2]",
        "\t- [ ] (**0920-0950**) — INDENTED",
        "> - [ ] (**09:20-09:50**) — QUOTED",
    ];

    for line in rows {
        let operations = [
            stamp_fresh(line, day()),
            set_refresh(line, Some(30), day()),
            set_refresh(line, None, day()),
            stamp_fresh_preserve_keeps(line, day()),
        ];
        for stamped in operations {
            assert_eq!(
                stamped.line, line,
                "ledger row must stay byte-exact: {line:?}"
            );
            assert!(
                !stamped.changed,
                "ledger row must not be changed: {line:?}"
            );
            assert_eq!(stamped.refused, Some(Refusal::NotTask), "{line:?}");
        }
    }
}

#[test]
fn prose_that_mentions_a_time_range_remains_stampable() {
    let line = "- [ ] #task Review 09:20-09:50 before lunch";
    let stamped = stamp(line);
    assert_eq!(
        stamped.line,
        "- [ ] #task Review 09:20-09:50 before lunch [fresh:: 2026-10-08]"
    );
    assert_eq!(stamped.refused, None);
}

#[test]
fn set_refresh_replaces_existing_value() {
    let input = "- [ ] #task A [fresh:: 2026-10-01] [refresh:: 14] [created::2026-09-01]";
    let updated = set_refresh(input, Some(30), day());
    assert_eq!(
        updated.line,
        "- [ ] #task A [fresh:: 2026-10-08] [refresh:: 30] [created::2026-09-01]"
    );
}

/// Every P vector's Tasks fields are identical through both Rust
/// parsers (`parse_details` for Dataview, `task_metadata` for hooks)
/// once misplaced `fresh` / `refresh` fields are set aside: a
/// misplaced stamp (P9) hides the Tasks fields after it — the repair
/// unhides them, which is the point — so the invariant is measured
/// between the fresh-stripped input and the stamped output. For
/// well-placed inputs the stripped input carries the same Tasks fields
/// as the raw input.
#[test]
fn parse_invariance_across_all_vectors() {
    let vectors = [
        "- [ ] #task Buy milk",
        "- [ ] #task Buy milk [created::2026-09-29]",
        "- [ ] #task Pick up Abby ^pickup",
        "- [ ] #task Plan trip [created::2026-08-26] #hide [priority:: high] ^trip",
        "- [ ] #task Read X [[#^h-8bac|🔖]] [h:: e629] [created::2026-08-28]",
        "- [ ] #task Rahway  [created:: 2026-07-15]  [scheduled:: 2026-08-10] ^rahway",
        "- [ ] #task Buy milk [fresh:: 2026-10-01] [created::2026-09-29]",
        "- [ ] #task Buy milk [fresh:: 2026-10-08] [created::2026-09-29]",
        "- [ ] #task Buy milk [created::2026-09-29] [fresh:: 2026-10-01]",
        "- [ ] #task A [fresh:: 2026-09-01] B [fresh:: 2026-09-20] [created::2026-09-01]",
        "- [ ] #task Rename queue input [refresh:: 14] [created::2026-09-10] [priority:: low]",
        "- [ ] #task Water plants [repeat:: every week] [created::2026-09-01]",
        "- [ ] #task #hide ^x",
        "- [ ] #task #prj Ship it #hide ^prj",
        "- [ ] #task Call mom (created:: 2026-09-01)",
        "\t- [?] #task Deferred [created::2026-09-01] [scheduled:: 2026-10-20]",
        "- [x] #task Old [completion:: 2026-10-01]",
        "> - [ ] #task Quoted [created::2026-09-01]",
    ];
    for input in vectors {
        let output = stamp_fresh(input, day()).line;
        let content = without_fresh_fields(input);
        assert_dataview_fields_equal(&content, &output);
        assert_hooks_fields_equal(&content, &output);
    }
}

/// The line with every `fresh` / `refresh` field removed, so the
/// parsers see the underlying Tasks content.
fn without_fresh_fields(line: &str) -> String {
    use crate::native::task_fields::inline_fields;
    let mut ranges = Vec::new();
    for field in inline_fields(line, "fresh") {
        ranges.push((field.start, field.end));
    }
    for field in inline_fields(line, "refresh") {
        ranges.push((field.start, field.end));
    }
    ranges.sort();
    let mut output = String::with_capacity(line.len());
    let mut cursor = 0;
    for (start, end) in ranges {
        output.push_str(&line[cursor..start]);
        cursor = end;
    }
    output.push_str(&line[cursor..]);
    output
}

fn assert_dataview_fields_equal(before: &str, after: &str) {
    use crate::native::dataview::{parse_details, TaskDetails, TaskFormat};
    let parse = |line: &str| -> TaskDetails {
        parse_details(&strip_block_link(line), TaskFormat::Dataview)
    };
    let a = parse(before);
    let b = parse(after);
    assert_eq!(a.priority, b.priority, "priority changed for {before:?}");
    assert_eq!(a.created, b.created, "created changed for {before:?}");
    assert_eq!(a.start, b.start, "start changed for {before:?}");
    assert_eq!(a.scheduled, b.scheduled, "scheduled changed for {before:?}");
    assert_eq!(a.due, b.due, "due changed for {before:?}");
    assert_eq!(a.done, b.done, "done changed for {before:?}");
    assert_eq!(a.cancelled, b.cancelled, "cancelled changed for {before:?}");
    assert_eq!(
        a.recurrence_source, b.recurrence_source,
        "recurrence changed for {before:?}"
    );
    assert_eq!(
        a.on_completion, b.on_completion,
        "onCompletion changed for {before:?}"
    );
    assert_eq!(a.id, b.id, "id changed for {before:?}");
    assert_eq!(
        a.depends_on, b.depends_on,
        "dependsOn changed for {before:?}"
    );
    assert_eq!(a.tags, b.tags, "tags changed for {before:?}");
}

fn assert_hooks_fields_equal(before: &str, after: &str) {
    use crate::native::task_status_hooks::task_metadata;
    let parse = |line: &str| {
        let body = task_body(line);
        let block = trailing_block_id(&body);
        task_metadata(&body, block.as_deref())
    };
    let a = parse(before);
    let b = parse(after);
    assert_eq!(a, b, "hooks fields changed for {before:?}");
}

/// The line without a trailing ` ^id` block link, mirroring
/// `Task::from_line` before `parse_details`.
fn strip_block_link(line: &str) -> String {
    let trimmed = line.trim_end();
    if let Some(block_start) = trailing_block_start(trimmed) {
        return trimmed[..block_start].trim_end().to_string();
    }
    trimmed.to_string()
}

fn trailing_block_start(trimmed: &str) -> Option<usize> {
    let token_start = trimmed.rfind([' ', '\t']).map(|i| i + 1).unwrap_or(0);
    let token = &trimmed[token_start..];
    let id = token.strip_prefix('^')?;
    if id.is_empty()
        || !id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
    {
        return None;
    }
    Some(trimmed.len() - token.len())
}

/// The text after the checkbox, as the hooks parser sees it.
fn task_body(line: &str) -> String {
    let bytes = line.as_bytes();
    let mut index = bytes
        .iter()
        .take_while(|b| matches!(b, b' ' | b'\t'))
        .count();
    // List marker.
    if matches!(bytes.get(index), Some(b'-' | b'*' | b'+')) {
        index += 1;
    } else {
        let digits = bytes[index..]
            .iter()
            .take_while(|b| b.is_ascii_digit())
            .count();
        index += digits + 1;
    }
    while bytes.get(index).is_some_and(u8::is_ascii_whitespace) {
        index += 1;
    }
    // Checkbox.
    let close = line[index..].find(']').expect("task line");
    let after = index + close + 1;
    line[after..].trim_start().to_string()
}

fn trailing_block_id(body: &str) -> Option<String> {
    let token = body.split_whitespace().next_back()?;
    let id = token.strip_prefix('^')?;
    (!id.is_empty()
        && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-'))
    .then(|| id.to_string())
}

/// A stamped `[?]` task with `dependsOn` or a future `scheduled` still
/// parses identically (so hooks still derives Blocked), and a hooks
/// line rewrite (status plus `#prj_is_blocked` / `[unblocked::]`
/// edits) preserves `[fresh:: …]`.
#[test]
fn hooks_blocked_signals_survive_and_checkbox_swap_preserves_fresh() {
    use crate::native::task_status_hooks::task_metadata;

    for input in [
        "- [?] #task Waiting [dependsOn:: abc] [created::2026-09-01]",
        "- [?] #task Deferred [scheduled:: 2026-10-20] [created::2026-09-01]",
    ] {
        let stamped = stamp(input);
        assert_eq!(stamped.refused, None);
        let before_body = task_body(input);
        let after_body = task_body(&stamped.line);
        let before_block = trailing_block_id(&before_body);
        let after_block = trailing_block_id(&after_body);
        assert_eq!(
            task_metadata(&before_body, before_block.as_deref()),
            task_metadata(&after_body, after_block.as_deref()),
            "hooks must still see the Blocked signal in {input:?}"
        );
        // The hooks rewrite the whole line (status plus derived tag
        // and `[unblocked::]` edits): simulate the status swap and show
        // the stamp survives.
        let reopened = stamped.line.replacen("[?]", "[ ]", 1);
        assert!(
            reopened.contains("[fresh:: 2026-10-08]"),
            "checkbox swap must preserve fresh in {input:?}"
        );
        let read = read_freshness(&reopened, day());
        assert_eq!(read.fresh, Some(day()));
    }
}

/// `FreshnessConfig` defaults used across these tests.
#[test]
fn freshness_config_defaults_match_contract() {
    let config = FreshnessConfig::default();
    assert_eq!(config.interval(), 7);
    assert_eq!(config.rotten_daily_budget(), None);
}

/// K1: absence means 0 with no lints.
#[test]
fn keeps_absent_reads_zero() {
    let read =
        read_freshness("- [ ] #task Buy milk [fresh:: 2026-10-01]", day());
    assert_eq!(read.keeps, 0);
    assert!(read.lints.is_empty());
}

/// K2/K10: the first valid value wins; 999 is the ceiling.
#[test]
fn keeps_first_valid_wins_up_to_999() {
    let read = read_freshness("- [ ] #task A [keeps:: 2] B [keeps:: 5]", day());
    assert_eq!(read.keeps, 2);
    assert!(read.lints.contains(&"keeps_duplicate".to_string()));
    let read = read_freshness(
        "- [ ] #task A [fresh:: 2026-10-01] [keeps:: 999]",
        day(),
    );
    assert_eq!(read.keeps, 999);
    assert!(read.lints.is_empty());
}

/// K9: zero, negative, fractional, out-of-range, and non-numeric
/// values are invalid and report 0.
#[test]
fn keeps_invalid_values_report_zero() {
    for value in ["0", "-1", "2.5", "1000", "soon", ""] {
        let line = format!("- [ ] #task A [keeps:: {value}]");
        let read = read_freshness(&line, day());
        assert_eq!(read.keeps, 0, "value {value:?} must read 0");
        assert!(
            read.lints.contains(&"keeps_invalid".to_string()),
            "value {value:?} must lint"
        );
    }
    // An invalid value beside a valid one lints but keeps the first
    // valid value.
    let read =
        read_freshness("- [ ] #task A [keeps:: soon] [keeps:: 3]", day());
    assert_eq!(read.keeps, 3);
    assert!(read.lints.contains(&"keeps_invalid".to_string()));
    assert!(read.lints.contains(&"keeps_duplicate".to_string()));
}

/// K13: the key match is exact and case-sensitive; readers accept
/// bracket and paren field syntax.
#[test]
fn keeps_key_match_is_exact_and_paren_reads() {
    let read = read_freshness(
        "- [ ] #task A [keep:: 2] [Keep:: 2] [keepsx:: 2]",
        day(),
    );
    assert_eq!(read.keeps, 0);
    assert!(read.lints.is_empty());
    let read = read_freshness("- [ ] #task Call mom (keeps:: 2)", day());
    assert_eq!(read.keeps, 2);
    assert!(read.lints.is_empty());
}

/// K8: `keeps` inside the Tasks suffix lints `fresh_misplaced`, and
/// the next stamp repairs it into canonical order.
#[test]
fn keeps_misplaced_lints_and_repairs() {
    let input = "- [ ] #task Buy milk [created:: 2026-09-29] [keeps:: 2]";
    let read = read_freshness(input, day());
    assert_eq!(read.keeps, 2);
    assert!(read.lints.contains(&"fresh_misplaced".to_string()));
    let preserved = stamp_fresh_preserve_keeps(input, day());
    assert_eq!(
        preserved.line,
        "- [ ] #task Buy milk [fresh:: 2026-10-08] [keeps:: 2] [created:: 2026-09-29]"
    );
    assert!(preserved.changed);
}

/// K3: canonical output order is `fresh`, `refresh`, `keeps`, then
/// the Tasks suffix.
#[test]
fn keeps_canonical_order_after_refresh() {
    let preserved = stamp_fresh_preserve_keeps(
        "- [ ] #task Rename queue input [keeps:: 2] [refresh:: 14] [fresh:: 2026-10-01] [created:: 2026-09-10] [priority:: low]",
        day(),
    );
    assert_eq!(
        preserved.line,
        "- [ ] #task Rename queue input [fresh:: 2026-10-08] [refresh:: 14] [keeps:: 2] [created:: 2026-09-10] [priority:: low]"
    );
    assert!(preserved.changed);
}

/// K4: a canonical same-day preserve is a byte-identical no-op.
#[test]
fn keeps_same_day_preserve_is_noop() {
    let line = "- [ ] #task Buy milk [fresh:: 2026-10-08] [keeps:: 2]";
    let preserved = stamp_fresh_preserve_keeps(line, day());
    assert_eq!(preserved.line, line);
    assert!(!preserved.changed);
}

/// K5/K6/K7: every generic human stamp clears `keeps`, even when the
/// stamp date already equals today.
#[test]
fn generic_stamps_clear_keeps_including_same_day() {
    let stamped =
        stamp("- [ ] #task Buy milk [fresh:: 2026-10-08] [keeps:: 2]");
    assert_eq!(stamped.line, "- [ ] #task Buy milk [fresh:: 2026-10-08]");
    assert!(stamped.changed);
    let stamped = stamp("- [ ] #task Buy milk [fresh:: 2026-10-01] [keeps:: 1] [created::2026-09-29]");
    assert_eq!(
        stamped.line,
        "- [ ] #task Buy milk [fresh:: 2026-10-08] [created::2026-09-29]"
    );
    let updated = set_refresh(
        "- [ ] #task Buy milk [fresh:: 2026-10-01] [refresh:: 14] [keeps:: 2]",
        Some(30),
        day(),
    );
    assert_eq!(
        updated.line,
        "- [ ] #task Buy milk [fresh:: 2026-10-08] [refresh:: 30]"
    );
}

/// K11: refusals write nothing, so the streak stays on the line.
#[test]
fn refusals_leave_keeps_untouched() {
    for (input, refusal) in [
        (
            "- [x] #task Old [keeps:: 2] [completion:: 2026-10-01]",
            Refusal::Closed,
        ),
        (
            "- [ ] #task Water plants [repeat:: every week] [keeps:: 2]",
            Refusal::Recurring,
        ),
    ] {
        let stamped = stamp(input);
        assert_eq!(stamped.line, input);
        assert!(!stamped.changed);
        assert_eq!(stamped.refused, Some(refusal));
        assert_eq!(read_freshness(input, day()).keeps, 2);
    }
}

/// The shared parity fixture drives the Rust read/reset/placement
/// cases verbatim (`docs/freshness.md` §10a). Rust has no production
/// increment API; `js`-only vectors are skipped here.
#[test]
fn keeps_fixture_vectors_match_rust() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/freshness_keeps/vectors.json");
    let text = std::fs::read_to_string(&path).expect("read keeps fixture");
    let fixture: serde_json::Value =
        serde_json::from_str(&text).expect("parse keeps fixture");
    assert_eq!(fixture["schema"], 4);
    let today = NaiveDate::parse_from_str(
        fixture["today"].as_str().expect("fixture today"),
        "%Y-%m-%d",
    )
    .expect("valid fixture today");
    assert_eq!(today, day());
    let vectors = fixture["placement"].as_array().expect("placement array");
    assert!(!vectors.is_empty(), "fixture must carry placement vectors");
    for vector in vectors {
        let id = vector["id"].as_str().unwrap_or("?");
        let runners: Vec<&str> = vector["runners"]
            .as_array()
            .expect("runners array")
            .iter()
            .map(|runner| runner.as_str().expect("runner string"))
            .collect();
        if !runners.contains(&"rust") {
            continue;
        }
        let input = vector["input"].as_str().expect("input line");
        match vector["mode"].as_str().expect("vector mode") {
            "read" => {
                let read = read_freshness(input, today);
                assert_eq!(
                    read.keeps,
                    vector["expected_keeps"].as_u64().expect("keeps") as u32,
                    "{id}: keeps"
                );
                let expected: Vec<String> = vector["expected_lints"]
                    .as_array()
                    .expect("lints array")
                    .iter()
                    .map(|lint| lint.as_str().expect("lint string").to_string())
                    .collect();
                assert_eq!(read.lints, expected, "{id}: lints");
            }
            "stamp" | "preserve" => {
                let stamped = if vector["mode"] == "stamp" {
                    stamp_fresh(input, today)
                } else {
                    stamp_fresh_preserve_keeps(input, today)
                };
                if let Some(refused) = vector
                    .get("expected_refused")
                    .and_then(|value| value.as_str())
                {
                    assert_eq!(
                        stamped.refused.map(|refusal| refusal.as_str()),
                        Some(refused),
                        "{id}: refusal"
                    );
                    assert_eq!(stamped.line, input, "{id}: refused line");
                } else {
                    assert_eq!(
                        stamped.line,
                        vector["expected_line"].as_str().expect("line"),
                        "{id}: line"
                    );
                    assert_eq!(
                        stamped.changed,
                        vector["expected_changed"].as_bool().expect("changed"),
                        "{id}: changed"
                    );
                }
            }
            "set_refresh" => {
                let days = vector["refresh"].as_u64().expect("refresh") as u16;
                let updated = set_refresh(input, Some(days), today);
                assert_eq!(
                    updated.line,
                    vector["expected_line"].as_str().expect("line"),
                    "{id}: line"
                );
                assert_eq!(
                    updated.changed,
                    vector["expected_changed"].as_bool().expect("changed"),
                    "{id}: changed"
                );
            }
            mode => panic!("{id}: unknown fixture mode {mode:?}"),
        }
    }
}

/// UB vectors for `[unblocked::]` placement (`docs/freshness.md`
/// §10b). D = `2026-10-08`.
fn unblock_scope() -> UnblockScope {
    UnblockScope {
        recurring: false,
        hidden: false,
        is_daily_note: false,
        is_today: false,
    }
}

#[test]
fn ub1_mark_unblocked_writes_and_replaces_older_value() {
    let marked = mark_unblocked(
        "- [ ] #task Recovered [fresh:: 2026-10-01] [unblocked:: 2026-10-05] [created:: 2026-09-01]",
        day(),
        ' ',
        &unblock_scope(),
    )
    .expect("UB1 writes");
    assert_eq!(
        marked,
        "- [ ] #task Recovered [fresh:: 2026-10-01] [unblocked:: 2026-10-08] [created:: 2026-09-01]"
    );
    // The write preserves the fresh group and never writes fresh.
    let read = read_freshness(&marked, day());
    assert_eq!(read.fresh, Some(date(2026, 10, 1)));
    assert_eq!(read.unblocked, Some(day()));
}

#[test]
fn ub1_mark_unblocked_keeps_refresh_and_keeps_in_order() {
    let marked = mark_unblocked(
        "- [ ] #task Recovered [fresh:: 2026-10-01] [refresh:: 14] [keeps:: 2] [scheduled:: 2026-10-09]",
        day(),
        ' ',
        &unblock_scope(),
    )
    .expect("UB1 writes with a future schedule");
    assert_eq!(
        marked,
        "- [ ] #task Recovered [fresh:: 2026-10-01] [refresh:: 14] [keeps:: 2] [unblocked:: 2026-10-08] [scheduled:: 2026-10-09]"
    );
}

#[test]
fn ub2_mark_unblocked_skips_new_without_fresh() {
    assert_eq!(
        mark_unblocked(
            "- [ ] #task New capture [created:: 2026-09-30]",
            day(),
            ' ',
            &unblock_scope(),
        ),
        None,
        "UB2: no fresh means already due as NEW"
    );
}

#[test]
fn ub3_mark_unblocked_skips_scheduled_tickler() {
    assert_eq!(
        mark_unblocked(
            "- [ ] #task Deferred [fresh:: 2026-10-01] [scheduled:: 2026-10-07]",
            day(),
            ' ',
            &unblock_scope(),
        ),
        None,
        "UB3: already RESURFACED from scheduled"
    );
}

#[test]
fn ub4_mark_unblocked_skips_lane_landings() {
    let line = "- [ ] #task Recovered [fresh:: 2026-10-01]";
    assert_eq!(
        mark_unblocked(line, day(), '*', &unblock_scope()),
        None,
        "UB4: Next landing is covered by the lane walk"
    );
    assert_eq!(
        mark_unblocked(line, day(), '/', &unblock_scope()),
        None,
        "UB4: Pending landing is covered by the lane walk"
    );
}

#[test]
fn ub5_ub6_mark_unblocked_skips_out_of_scope_rows() {
    let line = "- [ ] #task Recovered [fresh:: 2026-10-01]";
    for scope in [
        UnblockScope {
            recurring: true,
            ..unblock_scope()
        },
        UnblockScope {
            hidden: true,
            ..unblock_scope()
        },
        UnblockScope {
            is_daily_note: true,
            ..unblock_scope()
        },
        UnblockScope {
            is_today: true,
            ..unblock_scope()
        },
    ] {
        assert_eq!(
            mark_unblocked(line, day(), ' ', &scope),
            None,
            "UB5/UB6: outside freshness scope"
        );
    }
}

#[test]
fn ub7_stamps_clear_unblocked() {
    let input =
        "- [ ] #task Recovered [fresh:: 2026-10-01] [unblocked:: 2026-10-08] [created:: 2026-09-01]";
    let stamped = stamp(input);
    assert_eq!(
        stamped.line,
        "- [ ] #task Recovered [fresh:: 2026-10-08] [created:: 2026-09-01]"
    );
    assert!(stamped.changed);
    // The seed preserve mode clears it too: it never writes unblocked.
    let preserved = stamp_fresh_preserve_keeps(input, day());
    assert_eq!(
        preserved.line,
        "- [ ] #task Recovered [fresh:: 2026-10-08] [created:: 2026-09-01]"
    );
}

#[test]
fn ub8_prune_drops_unblocked_when_fresh_passes_it() {
    let stale =
        "- [ ] #task Recovered [fresh:: 2026-10-08] [unblocked:: 2026-10-07]";
    assert_eq!(
        prune_unblocked(stale, day()),
        Some("- [ ] #task Recovered [fresh:: 2026-10-08]".to_string()),
        "UB8: fresh > U prunes"
    );
    // The fresh == U tie still resurfaces (UB9): nothing to prune.
    let tie =
        "- [ ] #task Recovered [fresh:: 2026-10-08] [unblocked:: 2026-10-08]";
    assert_eq!(prune_unblocked(tie, day()), None);
}

#[test]
fn ub_read_reports_unblocked_lints_and_latest_wins() {
    let malformed = read_freshness(
        "- [ ] #task Recovered [fresh:: 2026-10-01] [unblocked:: 2026-13-01]",
        day(),
    );
    assert_eq!(malformed.unblocked, None);
    assert!(malformed.lints.contains(&"unblocked_malformed".to_string()));
    let duplicate = read_freshness(
        "- [ ] #task Recovered [fresh:: 2026-10-01] [unblocked:: 2026-10-05] [unblocked:: 2026-10-07]",
        day(),
    );
    assert_eq!(duplicate.unblocked, Some(date(2026, 10, 7)));
    assert!(duplicate.lints.contains(&"unblocked_duplicate".to_string()));
}

/// UB12: a canonical line carries `[unblocked::]` before the Tasks
/// fields, so both Rust parsers still extract `scheduled`,
/// `dependsOn`, and `id` identically before and after the write.
#[test]
fn ub12_unblocked_before_tasks_fields_parses_invariantly() {
    let before = "- [ ] #task Recovered [fresh:: 2026-10-01] [scheduled:: 2026-10-09] [dependsOn:: lane-root] [id:: lane-task] ^lane-task";
    let marked = mark_unblocked(before, day(), ' ', &unblock_scope())
        .expect("UB12 setup writes");
    assert_eq!(
        marked,
        "- [ ] #task Recovered [fresh:: 2026-10-01] [unblocked:: 2026-10-08] [scheduled:: 2026-10-09] [dependsOn:: lane-root] [id:: lane-task] ^lane-task"
    );
    assert_dataview_fields_equal(before, &marked);
    assert_hooks_fields_equal(before, &marked);
}

/// Preservation contract (PT13): stamping keeps a distinctive authored
/// theme field byte-identical and ahead of the fresh/Tasks suffix.
#[test]
fn theme_field_survives_stamp_before_fresh_suffix() {
    let stamped = stamp(
        "- [ ] #task Write the report (theme::  WRITE) [created::2026-09-29] ^write",
    );
    assert_eq!(
        stamped.line,
        "- [ ] #task Write the report (theme::  WRITE) [fresh:: 2026-10-08] [created::2026-09-29] ^write"
    );
    assert!(stamped.changed);
    assert_eq!(stamped.refused, None);
}

/// Preservation contract (PT13): a closed task keeps its theme history;
/// stamping refuses and leaves every byte untouched.
#[test]
fn closed_theme_history_survives_stamp_refusal() {
    let line =
        "- [x] #task Shipped (theme::  WRITE) [completion:: 2026-10-07] ^ship";
    let stamped = stamp(line);
    assert_eq!(stamped.line, line);
    assert!(!stamped.changed);
    assert_eq!(stamped.refused, Some(Refusal::Closed));
}

/// Preservation contract (R11): stamping keeps a distinctive authored
/// root field byte-identical and ahead of the fresh/Tasks suffix.
#[test]
fn root_field_survives_stamp_before_fresh_suffix() {
    let stamped = stamp(
        "- [ ] #task Write the report (root::  sase) [created::2026-09-29] ^write",
    );
    assert_eq!(
        stamped.line,
        "- [ ] #task Write the report (root::  sase) [fresh:: 2026-10-08] [created::2026-09-29] ^write"
    );
    assert!(stamped.changed);
    assert_eq!(stamped.refused, None);
}

#[test]
fn root_field_survives_keep_preserve_and_unblock() {
    let preserved = stamp_fresh_preserve_keeps(
        "- [ ] #task Work (root::  sase) [keeps:: 2] [created:: 2026-09-29] ^t",
        day(),
    );
    assert_eq!(
        preserved.line,
        "- [ ] #task Work (root::  sase) [fresh:: 2026-10-08] [keeps:: 2] [created:: 2026-09-29] ^t"
    );
    let marked = mark_unblocked(
        "- [ ] #task Recovered (root::  sase) [fresh:: 2026-10-01] [scheduled:: 2026-10-09] ^t",
        day(),
        ' ',
        &unblock_scope(),
    )
    .expect("unblock writes");
    assert_eq!(
        marked,
        "- [ ] #task Recovered (root::  sase) [fresh:: 2026-10-01] [unblocked:: 2026-10-08] [scheduled:: 2026-10-09] ^t"
    );
}
