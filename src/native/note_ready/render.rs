//! Human rendering for `bob ready`: the per-note bar overview and
//! the single-note worklist.
//!
//! The contract lives in `docs/plan.md` ("## Ready cap per note").
//! Rendering is pure over [`ScanReport`] so it is unit-testable with
//! color forced on and off. Color never carries state alone:
//! without a TTY or with `NO_COLOR` the glyphs and text are
//! identical, only the ANSI escapes are gone.

use std::fmt::Write as _;

use super::{NoteReady, NoteState, ReadyDetail, ScanReport};
use crate::native::style::{display_width, terminal_width, truncate, Styler};

/// Full bar width in cells; shrinks to fit narrow terminals.
const BAR_WIDTH: usize = 30;
/// Minimum bar width when shrinking to fit.
const MIN_BAR_WIDTH: usize = 10;
/// Longest note-name column before truncation.
const MAX_NAME_WIDTH: usize = 24;

/// Counts beside one worklist note.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct AlsoHere {
    pub(crate) next: u32,
    pub(crate) pending: u32,
    pub(crate) blocked: u32,
    pub(crate) recurring: u32,
}

fn plural(count: u32, singular: &str) -> String {
    if count == 1 {
        format!("1 {singular}")
    } else {
        format!("{count} {singular}s")
    }
}

fn state_label(state: NoteState) -> &'static str {
    match state {
        NoteState::Exempt => "EXEMPT",
        NoteState::Crowded => "CROWDED",
        NoteState::Full => "FULL",
        NoteState::Room => "ROOM",
        NoteState::Empty => "EMPTY",
    }
}

/// `1 new`, `3 rotten`: the non-zero freshness make-up fragments.
fn makeup_fragments(entry: &NoteReady) -> Vec<String> {
    let mut fragments = Vec::new();
    if let Some(make_up) = &entry.make_up {
        if make_up.new > 0 {
            fragments.push(format!("{} new", make_up.new));
        }
        if make_up.rotten > 0 {
            fragments.push(format!("{} rotten", make_up.rotten));
        }
    }
    fragments
}

/// Dim trailing meta for a bar row: kind, parent, make-up,
/// recurring, and the cap source when it is not the default.
fn row_meta(entry: &NoteReady, styler: &Styler) -> String {
    let mut parts = vec![entry.kind.clone()];
    if let Some(parent) = &entry.parent {
        parts.push(format!("parent {parent}"));
    }
    parts.extend(makeup_fragments(entry));
    if entry.recurring > 0 {
        parts.push(format!("\u{21bb} {}", entry.recurring));
    }
    if entry.cap_source.as_str() != "default"
        && let Some(cap) = entry.cap
    {
        parts.push(format!("cap {cap} ({})", entry.cap_source.as_str()));
    }
    if parts.is_empty() {
        String::new()
    } else {
        styler.dim(&parts.join(" \u{b7} "))
    }
}

/// One bar row's cells: one filled cell per task up to `width`, a
/// dim marker right after the cap-th cell (shown only when the row
/// reaches its cap and the cap sits inside the bar), and a trailing
/// ellipsis when tasks overflow the bar. Cells within the cap are
/// cyan; cells past it are red.
fn bar_cells(entry: &NoteReady, width: usize, styler: &Styler) -> String {
    let cap = entry.cap.unwrap_or(u32::MAX) as usize;
    let count = entry.count as usize;
    let shown = count.min(width);
    let mut cells: Vec<String> = (0..shown)
        .map(|index| {
            if index < cap {
                styler.cyan("\u{25a0}")
            } else {
                styler.red("\u{25a0}")
            }
        })
        .collect();
    let overflow = count > shown;
    // The marker sits right after the cap-th cell. It is omitted
    // when the cap is at or past the bar width, and room rows never
    // reach it.
    let marked = matches!(entry.state, NoteState::Crowded | NoteState::Full)
        && cap < width
        && cap <= shown;
    let mut bar = String::new();
    for (index, cell) in cells.drain(..).enumerate() {
        if marked && index == cap {
            bar.push_str(&styler.dim("\u{2502}"));
        }
        bar.push_str(&cell);
    }
    if marked && cap == shown {
        bar.push_str(&styler.dim("\u{2502}"));
    }
    if overflow {
        bar.push('\u{2026}');
    }
    bar
}

/// Display name for a bar row: the stem, or the vault path when two
/// displayed stems collide.
fn display_name(entry: &NoteReady, collide: bool) -> String {
    if collide {
        entry.path.clone()
    } else {
        entry.name.clone()
    }
}

/// Bar rows for one state group, padded into columns: name,
/// right-aligned `n/cap`, bar, excess, meta.
fn bar_rows(
    entries: &[&NoteReady],
    bar_width: usize,
    with_meta: bool,
    styler: &Styler,
) -> Vec<String> {
    use std::collections::HashMap;
    let mut stem_counts: HashMap<&str, usize> = HashMap::new();
    for entry in entries {
        *stem_counts.entry(entry.name.as_str()).or_default() += 1;
    }
    let name_width = entries
        .iter()
        .map(|entry| {
            display_width(&display_name(
                entry,
                stem_counts[entry.name.as_str()] > 1,
            ))
        })
        .max()
        .unwrap_or(0)
        .min(MAX_NAME_WIDTH);
    let meter_width = entries
        .iter()
        .map(|entry| {
            let cap = entry.cap.unwrap_or(entry.count);
            format!("{}/{}", entry.count, cap).len()
        })
        .max()
        .unwrap_or(0);
    entries
        .iter()
        .map(|entry| {
            let collide = stem_counts[entry.name.as_str()] > 1;
            let name = truncate(&display_name(entry, collide), MAX_NAME_WIDTH);
            let name = format!(
                "{name}{}",
                " ".repeat(name_width.saturating_sub(display_width(&name)))
            );
            let cap = entry.cap.unwrap_or(entry.count);
            let meter = format!("{}/{}", entry.count, cap);
            let meter = format!(
                "{}{meter}",
                " ".repeat(meter_width.saturating_sub(meter.len()))
            );
            let bar = bar_cells(entry, bar_width, styler);
            let excess = match entry.state {
                NoteState::Crowded => {
                    styler.red(&format!("+{}", entry.over_by))
                }
                NoteState::Full => styler.dim("full"),
                _ => String::new(),
            };
            let meta = if with_meta {
                row_meta(entry, styler)
            } else {
                String::new()
            };
            let mut row = format!("    {name}  {meter}  {bar}  {excess}");
            if !meta.is_empty() {
                row.push_str(&format!("   {meta}"));
            } else {
                row = row.trim_end().to_string();
            }
            row
        })
        .collect()
}

/// Width of the widest line, ignoring ANSI escapes.
fn text_width(text: &str) -> usize {
    let mut plain = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(char) = chars.next() {
        if char == '\u{1b}' {
            for char in chars.by_ref() {
                if char == 'm' {
                    break;
                }
            }
        } else {
            plain.push(char);
        }
    }
    plain.chars().count()
}

fn fits(output: &str, width: usize) -> bool {
    output.lines().all(|line| text_width(line) <= width)
}

/// Render the overview: header, summary, CROWDED/FULL/ROOM groups,
/// footer, and the make-room hint.
pub(crate) fn render_overview(
    scan: &ScanReport,
    show_all: bool,
    styler: &Styler,
) -> String {
    // Fit the terminal: drop the meta column first, then shrink the
    // bar to at least MIN_BAR_WIDTH cells.
    let mut with_meta = true;
    let mut bar_width = BAR_WIDTH;
    loop {
        let output =
            render_overview_sized(scan, show_all, bar_width, with_meta, styler);
        if fits(&output, terminal_width()) {
            return output;
        }
        if with_meta {
            with_meta = false;
        } else if bar_width > MIN_BAR_WIDTH {
            bar_width = (bar_width.saturating_sub(5)).max(MIN_BAR_WIDTH);
        } else {
            return output;
        }
    }
}

fn render_overview_sized(
    scan: &ScanReport,
    show_all: bool,
    bar_width: usize,
    with_meta: bool,
    styler: &Styler,
) -> String {
    let totals = &scan.report.totals;
    let weekday = scan.today.format("%a").to_string();
    let mut output = String::new();
    let _ = writeln!(
        output,
        "bob ready \u{b7} {weekday} {} \u{b7} cap {} per note{}",
        scan.today.format("%Y-%m-%d"),
        scan.default_cap,
        if scan.default_source.as_str() == "preview" {
            " \u{b7} preview"
        } else {
            ""
        },
    );
    output.push('\n');

    if totals.crowded > 0 {
        let _ = writeln!(
            output,
            "  {} \u{b7} {} over \u{b7} {} full \u{b7} {} ({} \u{b7} {})",
            styler.red(&format!("CROWDED {}", totals.crowded)),
            totals.excess,
            totals.full,
            plural(totals.notes, "note"),
            plural(totals.areas, "area"),
            plural(totals.projects, "project"),
        );
    } else {
        let _ = writeln!(
            output,
            "  {}",
            styler.green("CROWDED 0 \u{2713} \u{b7} every note has room"),
        );
    }

    let by_state = |state: NoteState| -> Vec<&NoteReady> {
        scan.report
            .notes
            .iter()
            .filter(|note| note.state == state)
            .collect()
    };
    let crowded = by_state(NoteState::Crowded);
    let full = by_state(NoteState::Full);
    let room = by_state(NoteState::Room);
    let empty = by_state(NoteState::Empty);
    let exempt: Vec<&NoteReady> = by_state(NoteState::Exempt);

    if !crowded.is_empty() {
        output.push_str(&format!("\n  {}\n", styler.red("CROWDED")));
        for row in bar_rows(&crowded, bar_width, with_meta, styler) {
            output.push_str(&row);
            output.push('\n');
        }
    }
    if !full.is_empty() {
        output.push_str("\n  FULL\n");
        for row in bar_rows(&full, bar_width, with_meta, styler) {
            output.push_str(&row);
            output.push('\n');
        }
    }
    if !room.is_empty() {
        output.push_str("\n  ROOM\n");
        if show_all {
            for row in bar_rows(&room, bar_width, with_meta, styler) {
                output.push_str(&row);
                output.push('\n');
            }
        } else {
            let names: Vec<String> = room
                .iter()
                .map(|entry| format!("{} {}", entry.name, entry.count))
                .collect();
            let _ = writeln!(output, "    {}", names.join(" \u{b7} "));
        }
    }
    if show_all && !empty.is_empty() {
        output.push_str("\n  EMPTY\n");
        let names: Vec<String> =
            empty.iter().map(|entry| entry.name.clone()).collect();
        let _ = writeln!(output, "    {}", names.join(" \u{b7} "));
    }

    // Footer: empty count, the not-capped list, recurring, -a hint.
    let mut footer: Vec<String> = Vec::new();
    if totals.empty > 0 {
        footer.push(format!("{} empty", totals.empty));
    }
    let mut uncapped: Vec<String> = exempt
        .iter()
        .map(|entry| format!("{} {} (ready_cap: off)", entry.name, entry.count))
        .collect();
    if show_all {
        for lint in &scan.report.lints {
            if lint.code == super::LINT_NOTE_READY_IN_TERMINAL_PROJECT {
                uncapped.push(format!("{} ({})", lint.path, lint.code));
            }
        }
    }
    if !uncapped.is_empty() {
        footer.push(format!("not capped: {}", uncapped.join(", ")));
    }
    if totals.recurring > 0 {
        footer.push(format!("\u{21bb} {} recurring", totals.recurring));
    }
    if !footer.is_empty() || !show_all {
        let mut line = format!("  {}", footer.join(" \u{b7} "));
        if !show_all {
            if !footer.is_empty() {
                line.push_str("   ");
            }
            line.push_str("(-a for all)");
        }
        output.push_str(&format!("\n{line}\n"));
    }

    if show_all && !scan.report.lints.is_empty() {
        output.push_str(&format!("\n  {}\n", styler.yellow("LINTS")));
        for lint in &scan.report.lints {
            let _ = writeln!(
                output,
                "    {} {} {}",
                lint.path,
                styler.cyan(&lint.code),
                lint.message,
            );
        }
    }

    // The quickest win: the crowded note with the smallest excess.
    let quickest = crowded.iter().min_by(|a, b| {
        a.over_by.cmp(&b.over_by).then_with(|| a.name.cmp(&b.name))
    });
    if let Some(note) = quickest {
        output.push_str(&format!(
            "\n  Make room \u{2192} bob ready {}\n",
            note.name
        ));
    }
    output.push_str(&format!("  {}\n", ready_gesture_hint()));
    output
}

/// Crowded-note remedies. The Task Card is the only `Ctrl+Shift+P`
/// surface, so the hint always names the card's keys.
fn ready_gesture_hint() -> &'static str {
    "split Ctrl+Shift+N \u{b7} defer Ctrl+Shift+P 1\u{2013}4 \u{b7} drop Ctrl+Shift+P x \u{b7} sequence Ctrl+Shift+P b"
}

/// Freshness label for one worklist row: `new`, `rotten Nd`,
/// `fresh Nd`, or blank when out of scope.
fn freshness_label(task: &ReadyDetail, today: chrono::NaiveDate) -> String {
    match (task.bucket.as_deref(), task.fresh_on) {
        (Some("new"), _) => "new".to_string(),
        (Some("rotten"), Some(fresh)) => {
            format!("rotten {}d", today.signed_duration_since(fresh).num_days())
        }
        (Some("rotten"), None) => "rotten".to_string(),
        (_, Some(fresh)) => {
            format!("fresh {}d", today.signed_duration_since(fresh).num_days())
        }
        _ => String::new(),
    }
}

/// Render the single-note worklist: file-order rows plus the
/// `also here` line and the make-room footer.
pub(crate) fn render_worklist(
    scan: &ScanReport,
    entry: &NoteReady,
    also: &AlsoHere,
    styler: &Styler,
) -> String {
    let mut tasks: Vec<&ReadyDetail> = scan
        .ready_details
        .iter()
        .filter(|task| task.path == entry.path)
        .collect();
    tasks.sort_by_key(|task| task.line);

    let mut output = String::new();
    let makeup = entry
        .make_up
        .map(|make_up| {
            format!(
                "{} ready + {} new + {} rotten",
                make_up.ready, make_up.new, make_up.rotten
            )
        })
        .unwrap_or_default();
    if entry.state == NoteState::Exempt {
        let _ = writeln!(
            output,
            "bob ready \u{b7} {} \u{b7} {} \u{b7} no cap (ready_cap: off) \u{b7} {}",
            entry.name, entry.count, entry.kind,
        );
    } else {
        let cap = entry.cap.unwrap_or(scan.default_cap);
        let state_text = match entry.state {
            NoteState::Crowded => {
                styler.red(&format!("CROWDED +{}", entry.over_by))
            }
            other => state_label(other).to_string(),
        };
        let _ = writeln!(
            output,
            "bob ready \u{b7} {} \u{b7} {}/{} \u{b7} {} \u{b7} {}{}{}",
            entry.name,
            entry.count,
            cap,
            state_text,
            entry.kind,
            entry
                .parent
                .as_ref()
                .map(|parent| format!(" \u{b7} parent {parent}"))
                .unwrap_or_default(),
            if makeup.is_empty() {
                String::new()
            } else {
                format!(" \u{b7} {makeup}")
            },
        );
    }
    output.push('\n');

    let width = terminal_width();
    let index_width = tasks.len().to_string().len().max(1);
    let reference_width = tasks
        .iter()
        .map(|task| format!("{}:{}", task.path, task.line).len())
        .max()
        .unwrap_or(0);
    let fresh_width = tasks
        .iter()
        .map(|task| freshness_label(task, scan.today).len())
        .max()
        .unwrap_or(0);
    for (position, task) in tasks.iter().enumerate() {
        let reference = format!("{}:{}", task.path, task.line);
        let fresh = freshness_label(task, scan.today);
        let block = task
            .block_id
            .as_ref()
            .map(|id| format!(" ^{id}"))
            .unwrap_or_default();
        // Fixed columns plus text plus trailing columns; the text
        // takes whatever the terminal leaves.
        let fixed = 2
            + index_width
            + 2
            + reference_width
            + 2
            + fresh_width
            + 2
            + block.len();
        let text_width = width.saturating_sub(fixed).max(10);
        let mut row = format!(
            "  {:>index_width$}  {reference:<reference_width$}  {:<text_width$}  {fresh:<fresh_width$}{block}",
            position + 1,
            truncate(&task.text, text_width),
            index_width = index_width,
            reference_width = reference_width,
            text_width = text_width,
            fresh_width = fresh_width,
        );
        row = row.trim_end().to_string();
        output.push_str(&row);
        output.push('\n');
    }

    let _ = writeln!(
        output,
        "\n  also here: {} next \u{b7} {} pending \u{b7} {} blocked \u{b7} \u{21bb} {} recurring",
        also.next, also.pending, also.blocked, also.recurring,
    );
    match entry.state {
        NoteState::Crowded => {
            let _ = writeln!(
                output,
                "  Make room for {}: {}",
                entry.over_by,
                ready_gesture_hint(),
            );
        }
        NoteState::Full => {
            output.push_str("  full \u{2014} at cap\n");
        }
        NoteState::Room => {
            let cap = entry.cap.unwrap_or(scan.default_cap);
            let _ = writeln!(output, "  room for {} more", cap - entry.count);
        }
        NoteState::Empty => {
            let cap = entry.cap.unwrap_or(scan.default_cap);
            let _ = writeln!(output, "  room for {} more", cap - entry.count);
        }
        NoteState::Exempt => {
            output.push_str("  no cap (ready_cap: off)\n");
        }
    }
    output
}

#[cfg(test)]
mod tests {
    use super::super::{CapSource, MakeUp, Totals};
    use super::*;

    fn entry(
        name: &str,
        kind: &str,
        count: u32,
        cap: Option<u32>,
        state: NoteState,
    ) -> NoteReady {
        NoteReady {
            path: format!("{name}.md"),
            name: name.to_string(),
            kind: kind.to_string(),
            status: "wip".to_string(),
            parent: None,
            count,
            cap,
            cap_source: CapSource::Default,
            state,
            over_by: count.saturating_sub(cap.unwrap_or(count)),
            make_up: Some(MakeUp {
                ready: count,
                new: 0,
                rotten: 0,
            }),
            recurring: 0,
        }
    }

    fn overview_scan(entries: Vec<NoteReady>) -> ScanReport {
        let mut totals = Totals {
            notes: 0,
            areas: 0,
            projects: 0,
            crowded: 0,
            full: 0,
            room: 0,
            empty: 0,
            exempt: 0,
            counted: 0,
            excess: 0,
            recurring: 0,
        };
        for note in &entries {
            match note.state {
                NoteState::Crowded => {
                    totals.crowded += 1;
                    totals.notes += 1;
                    totals.excess += note.over_by;
                }
                NoteState::Full => {
                    totals.full += 1;
                    totals.notes += 1;
                }
                NoteState::Room => {
                    totals.room += 1;
                    totals.notes += 1;
                }
                NoteState::Empty => {
                    totals.empty += 1;
                    totals.notes += 1;
                }
                NoteState::Exempt => totals.exempt += 1,
            }
            if note.kind == "area" && note.state != NoteState::Exempt {
                totals.areas += 1;
            }
            if note.kind == "project" && note.state != NoteState::Exempt {
                totals.projects += 1;
            }
            totals.counted += note.count;
        }
        ScanReport {
            report: super::super::Report {
                notes: entries,
                totals,
                lints: Vec::new(),
            },
            today: chrono::NaiveDate::from_ymd_opt(2026, 10, 1).unwrap(),
            default_cap: 5,
            default_source: CapSource::Default,
            ready_details: Vec::new(),
            next_counts: Default::default(),
            pending_counts: Default::default(),
            blocked_counts: Default::default(),
        }
    }

    #[test]
    fn overview_names_crowded_notes_with_bars() {
        let scan = overview_scan(vec![
            entry("sase", "project", 7, Some(5), NoteState::Crowded),
            entry("bob", "project", 5, Some(5), NoteState::Full),
            entry("dev", "area", 4, Some(5), NoteState::Room),
        ]);
        let plain = Styler::plain();
        let output = render_overview(&scan, false, &plain);
        assert!(
            output.contains(
                "bob ready \u{b7} Thu 2026-10-01 \u{b7} cap 5 per note"
            ),
            "{output}"
        );
        assert!(
            output.contains("CROWDED 1 \u{b7} 2 over \u{b7} 1 full"),
            "{output}"
        );
        assert!(
            output.contains("sase") && output.contains("7/5"),
            "{output}"
        );
        assert!(output.contains("+2"), "{output}");
        assert!(
            output.contains("Make room \u{2192} bob ready sase"),
            "{output}"
        );
        assert!(
            output.contains(
                "split Ctrl+Shift+N \u{b7} defer Ctrl+Shift+P 1\u{2013}4 \u{b7} drop Ctrl+Shift+P x \u{b7} sequence Ctrl+Shift+P b"
            ),
            "{output}"
        );
        assert!(!output.contains('\u{1b}'), "{output}");
    }

    #[test]
    fn ready_gesture_hint_is_always_the_task_card_keys() {
        let hint = ready_gesture_hint();
        assert_eq!(
            hint,
            "split Ctrl+Shift+N \u{b7} defer Ctrl+Shift+P 1\u{2013}4 \u{b7} drop Ctrl+Shift+P x \u{b7} sequence Ctrl+Shift+P b"
        );
        assert!(!hint.contains("type to search"), "{hint}");
        assert!(
            !hint.contains("sequence / defer / drop Ctrl+Shift+P"),
            "{hint}"
        );
    }

    #[test]
    fn overview_advertises_task_card_keys_on_any_day() {
        for day in [
            chrono::NaiveDate::from_ymd_opt(2026, 10, 1).unwrap(),
            chrono::NaiveDate::from_ymd_opt(2026, 10, 18).unwrap(),
            chrono::NaiveDate::from_ymd_opt(2026, 10, 19).unwrap(),
        ] {
            let mut scan = overview_scan(vec![entry(
                "sase",
                "project",
                7,
                Some(5),
                NoteState::Crowded,
            )]);
            scan.today = day;
            let output = render_overview(&scan, false, &Styler::plain());
            assert!(
                output.contains("defer Ctrl+Shift+P 1\u{2013}4"),
                "{day}: {output}"
            );
            assert!(output.contains("drop Ctrl+Shift+P x"), "{day}: {output}");
            assert!(
                output.contains("sequence Ctrl+Shift+P b"),
                "{day}: {output}"
            );
            assert!(!output.contains("type to search"), "{day}: {output}");
            assert!(
                !output.contains("sequence / defer / drop Ctrl+Shift+P"),
                "{day}: {output}"
            );
        }
    }

    #[test]
    fn overview_all_clear_is_green_without_crowded_group() {
        let scan = overview_scan(vec![entry(
            "dev",
            "area",
            4,
            Some(5),
            NoteState::Room,
        )]);
        let colored = Styler::colored();
        let output = render_overview(&scan, false, &colored);
        assert!(output.contains("CROWDED 0 \u{2713}"), "{output}");
        assert!(!output.contains("CROWDED\n"), "{output}");
        assert!(output.contains('\u{1b}'), "{output}");
    }

    #[test]
    fn crowded_bars_mark_the_cap_and_overflow() {
        let plain = Styler::plain();
        let crowded = entry("sase", "project", 61, Some(5), NoteState::Crowded);
        let bar = bar_cells(&crowded, 30, &plain);
        assert!(bar.contains("\u{2502}"), "{bar}");
        assert!(bar.ends_with('\u{2026}'), "{bar}");
        let full = entry("bob", "project", 5, Some(5), NoteState::Full);
        let bar = bar_cells(&full, 30, &plain);
        assert!(bar.ends_with("\u{2502}"), "{bar}");
    }

    #[test]
    fn worklist_lists_rows_in_file_order() {
        let mut scan = overview_scan(vec![entry(
            "sase_remote",
            "project",
            2,
            Some(5),
            NoteState::Room,
        )]);
        scan.ready_details = vec![
            ReadyDetail {
                path: "sase_remote.md".to_string(),
                line: 38,
                text: "Second".to_string(),
                block_id: None,
                bucket: Some("new".to_string()),
                fresh_on: None,
            },
            ReadyDetail {
                path: "sase_remote.md".to_string(),
                line: 21,
                text: "First".to_string(),
                block_id: Some("machines".to_string()),
                bucket: None,
                fresh_on: Some(
                    chrono::NaiveDate::from_ymd_opt(2026, 9, 30).unwrap(),
                ),
            },
        ];
        let plain = Styler::plain();
        let also = AlsoHere {
            next: 1,
            pending: 1,
            blocked: 2,
            recurring: 0,
        };
        let output =
            render_worklist(&scan, &scan.report.notes[0], &also, &plain);
        let first = output.find("First").unwrap();
        let second = output.find("Second").unwrap();
        assert!(first < second, "{output}");
        assert!(output.contains("sase_remote.md:21"), "{output}");
        assert!(output.contains("^machines"), "{output}");
        assert!(output.contains("fresh 1d"), "{output}");
        assert!(output.contains("also here: 1 next"), "{output}");
        assert!(output.contains("room for 3 more"), "{output}");
    }

    #[test]
    fn crowded_worklist_always_advertises_task_card_hints() {
        let mut scan = overview_scan(vec![entry(
            "sase",
            "project",
            7,
            Some(5),
            NoteState::Crowded,
        )]);
        let plain = Styler::plain();
        let also = AlsoHere {
            next: 0,
            pending: 0,
            blocked: 0,
            recurring: 0,
        };
        let expected = "Make room for 2: split Ctrl+Shift+N \u{b7} defer Ctrl+Shift+P 1\u{2013}4 \u{b7} drop Ctrl+Shift+P x \u{b7} sequence Ctrl+Shift+P b";
        let before =
            render_worklist(&scan, &scan.report.notes[0], &also, &plain);
        assert!(before.contains(expected), "{before}");
        assert!(!before.contains("type to search"), "{before}");
        scan.today = chrono::NaiveDate::from_ymd_opt(2026, 10, 19).unwrap();
        let after =
            render_worklist(&scan, &scan.report.notes[0], &also, &plain);
        assert!(after.contains(expected), "{after}");
    }
}
