//! Coalesce credit-neutral runs of closed continuous same-type Pomodoros.
//!
//! Only canonical, child-bearing closed rows in the selected daily ledger are
//! candidates. Adjacent rows coalesce when their clock, normalized component
//! sequence, child indentation, and touching time bounds agree, the name is
//! not MEET, and both task and theme attribution remain byte-for-byte equal
//! in minutes. The pass rescans to a fixed point so a newly coalesced run can
//! make a larger credit-neutral run possible. Rewrites preserve kept line
//! terminators and child bytes; the planner is idempotent.

use std::{collections::BTreeMap, ops::Range, path::Path};

use crate::native::{
    capture::{session_clock, SessionClock},
    capture_pomodoros,
    plan_budget::{normalize_component, split_components},
    pomodoro as native_pomodoro,
    pomodoro_homes::{
        closed_session_task_credit, counted_session_links, split_minutes,
        HomeResolver, HomeSource,
    },
    pomodoro_themes::routing,
};

use super::{
    logical_line, logical_lines, scan_pomodoros, CoalescedPomodoro,
    CoalescedPomodoroSource, PomodoroEntry,
};

#[derive(Debug)]
pub(crate) struct CoalesceOutcome {
    pub(crate) contents: String,
    pub(crate) coalesced: Vec<CoalescedPomodoro>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Clock {
    Worked,
    Off,
}

impl Clock {
    fn label(self) -> &'static str {
        match self {
            Self::Worked => "worked",
            Self::Off => "off",
        }
    }

    fn key(self) -> &'static str {
        match self {
            Self::Worked => "t",
            Self::Off => "off",
        }
    }
}

#[derive(Debug, Clone)]
struct Header {
    start: String,
    end: String,
    start_minutes: u16,
    end_minutes: u16,
    clock: Clock,
    minutes: u32,
    name: String,
    components: Vec<String>,
    parenthetical: Range<usize>,
}

#[derive(Debug, Clone)]
struct Candidate {
    entry_position: usize,
    line_index: usize,
    end_line: usize,
    header: String,
    parenthetical: Range<usize>,
    start: String,
    end: String,
    start_minutes: u16,
    end_minutes: u16,
    clock: Clock,
    minutes: u32,
    name: String,
    components: Vec<String>,
    child_indentation: String,
    children: Vec<String>,
    credit: BTreeMap<String, u32>,
    sources: Vec<CoalescedPomodoroSource>,
}

#[derive(Debug)]
struct Run {
    members: Vec<Candidate>,
    merged: Candidate,
}

struct PassOutcome {
    contents: String,
    lineage: BTreeMap<usize, Vec<CoalescedPomodoroSource>>,
    coalesced: Vec<CoalescedPomodoro>,
    changed: bool,
}

pub(crate) fn plan_coalesce<S: HomeSource>(
    daily_contents: &str,
    daily_path: &Path,
    resolver: &mut HomeResolver<S>,
) -> CoalesceOutcome {
    let mut contents = daily_contents.to_string();
    let mut lineage = BTreeMap::new();
    loop {
        let pass = coalesce_pass(&contents, daily_path, resolver, &lineage);
        if !pass.changed {
            return CoalesceOutcome {
                contents,
                coalesced: pass.coalesced,
            };
        }
        contents = pass.contents;
        lineage = pass.lineage;
    }
}

fn coalesce_pass<S: HomeSource>(
    contents: &str,
    daily_path: &Path,
    resolver: &mut HomeResolver<S>,
    lineage: &BTreeMap<usize, Vec<CoalescedPomodoroSource>>,
) -> PassOutcome {
    let lines = logical_lines(contents);
    let Some(section) = native_pomodoro::pomodoros_section_range(&lines) else {
        return unchanged(contents, lineage);
    };
    let model = scan_pomodoros(&lines, section);
    let candidates = model
        .entries
        .iter()
        .enumerate()
        .map(|(position, entry)| {
            candidate_for_entry(
                contents, &lines, position, entry, lineage, daily_path,
                resolver,
            )
        })
        .collect::<Vec<_>>();

    let mut runs = BTreeMap::<usize, Run>::new();
    let mut position = 0;
    while position < model.entries.len() {
        let Some(first) = candidates[position].clone() else {
            position += 1;
            continue;
        };
        let mut members = vec![first];
        let mut merged = None;
        let mut next_position = position + 1;
        while next_position < model.entries.len() {
            let Some(next) = candidates[next_position].clone() else {
                break;
            };
            let last = members.last().expect("run has a first member");
            if !can_join(last, &next, &lines) {
                break;
            }
            let mut extended = members.clone();
            extended.push(next);
            let Some(credit_neutral) =
                merge_candidate(&extended, daily_path, resolver)
            else {
                break;
            };
            members = extended;
            merged = Some(credit_neutral);
            next_position += 1;
        }
        if let Some(merged) = merged {
            runs.insert(position, Run { members, merged });
            position = next_position;
        } else {
            position += 1;
        }
    }

    if runs.is_empty() {
        let coalesced = candidates
            .into_iter()
            .flatten()
            .filter(|candidate| candidate.sources.len() > 1)
            .map(report_for)
            .collect();
        return PassOutcome {
            contents: contents.to_string(),
            lineage: lineage.clone(),
            coalesced,
            changed: false,
        };
    }

    rewrite_runs(contents, &lines, &model.entries, &candidates, &runs)
}

fn unchanged(
    contents: &str,
    lineage: &BTreeMap<usize, Vec<CoalescedPomodoroSource>>,
) -> PassOutcome {
    PassOutcome {
        contents: contents.to_string(),
        lineage: lineage.clone(),
        coalesced: Vec::new(),
        changed: false,
    }
}

fn candidate_for_entry<S: HomeSource>(
    contents: &str,
    lines: &[&str],
    entry_position: usize,
    entry: &PomodoroEntry,
    lineage: &BTreeMap<usize, Vec<CoalescedPomodoroSource>>,
    daily_path: &Path,
    resolver: &HomeResolver<S>,
) -> Option<Candidate> {
    if !entry.completed || !entry.has_child {
        return None;
    }
    let child_indentation = entry.child_indentation.clone()?;
    let header_line = *lines.get(entry.line_index)?;
    let header = parse_header(header_line)?;
    if routing::is_meet_name(Some(&header.name)) {
        return None;
    }
    let links = counted_session_links(contents, entry.line_index);
    let credit = closed_session_task_credit(
        &links,
        header.minutes,
        daily_path,
        resolver,
    )?;
    let children = lines
        .get(entry.line_index + 1..entry.end_line)?
        .iter()
        .map(|line| (*line).to_string())
        .collect::<Vec<_>>();
    let span = format!("{}-{}", header.start, header.end);
    let sources =
        lineage.get(&entry.line_index).cloned().unwrap_or_else(|| {
            vec![CoalescedPomodoroSource {
                span,
                minutes: header.minutes,
            }]
        });
    Some(Candidate {
        entry_position,
        line_index: entry.line_index,
        end_line: entry.end_line,
        header: header_line.to_string(),
        parenthetical: header.parenthetical,
        start: header.start,
        end: header.end,
        start_minutes: header.start_minutes,
        end_minutes: header.end_minutes,
        clock: header.clock,
        minutes: header.minutes,
        name: header.name,
        components: header.components,
        child_indentation,
        children,
        credit,
        sources,
    })
}

fn parse_header(line: &str) -> Option<Header> {
    let task_prefix_len =
        if line.starts_with("- [x] ") || line.starts_with("- [X] ") {
            6
        } else {
            return None;
        };
    native_pomodoro::completed_ledger_task(line)?;
    let body = line.get(task_prefix_len..)?;
    if !body.starts_with("(**") {
        return None;
    }
    let mut cursor = 3;
    let start = take_digits(body, &mut cursor, 4)?;
    expect(body, &mut cursor, "-")?;
    let end = take_digits(body, &mut cursor, 4)?;
    expect(body, &mut cursor, "** [")?;
    let (clock, clock_key) = if body.get(cursor..)?.starts_with("t:: ") {
        (Clock::Worked, "t:: ")
    } else if body.get(cursor..)?.starts_with("off:: ") {
        (Clock::Off, "off:: ")
    } else {
        return None;
    };
    cursor += clock_key.len();
    let minutes_text = take_one_or_more_digits(body, &mut cursor)?;
    expect(body, &mut cursor, "m])")?;
    let parenthetical = task_prefix_len..task_prefix_len + cursor;
    if !body.get(cursor..)?.starts_with(" — ") {
        return None;
    }
    let name = capture_pomodoros::parse_name_tail(&body[cursor..])?;
    if trailing_block_id(&name) {
        return None;
    }
    let components = split_components(&name)
        .into_iter()
        .map(|component| normalize_component(&component))
        .collect::<Vec<_>>();
    if components.is_empty() {
        return None;
    }

    let start_minutes = parse_hhmm(&start)?;
    let end_minutes = parse_hhmm(&end)?;
    if start_minutes > end_minutes {
        return None;
    }
    let minutes = minutes_text.parse::<u32>().ok()?;
    let (_, parsed_start, parsed_end) = native_pomodoro::task_time_range(body)?;
    if parsed_start != start || parsed_end != end {
        return None;
    }
    match session_clock(line) {
        SessionClock::Worked { minutes: parsed }
            if clock == Clock::Worked && parsed == u64::from(minutes) => {}
        SessionClock::OffClock {
            minutes: parsed,
            stamped: true,
        } if clock == Clock::Off && parsed == u64::from(minutes) => {}
        _ => return None,
    }
    Some(Header {
        start,
        end,
        start_minutes,
        end_minutes,
        clock,
        minutes,
        name,
        components,
        parenthetical,
    })
}

fn take_digits(text: &str, cursor: &mut usize, count: usize) -> Option<String> {
    let end = cursor.checked_add(count)?;
    let digits = text.get(*cursor..end)?;
    if !digits.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    *cursor = end;
    Some(digits.to_string())
}

fn take_one_or_more_digits(text: &str, cursor: &mut usize) -> Option<String> {
    let start = *cursor;
    while text.as_bytes().get(*cursor).is_some_and(u8::is_ascii_digit) {
        *cursor += 1;
    }
    (start < *cursor).then(|| text[start..*cursor].to_string())
}

fn expect(text: &str, cursor: &mut usize, expected: &str) -> Option<()> {
    if !text.get(*cursor..)?.starts_with(expected) {
        return None;
    }
    *cursor += expected.len();
    Some(())
}

fn parse_hhmm(text: &str) -> Option<u16> {
    let hour = text.get(..2)?.parse::<u16>().ok()?;
    let minute = text.get(2..4)?.parse::<u16>().ok()?;
    (hour < 24 && minute < 60).then_some(hour * 60 + minute)
}

fn trailing_block_id(name: &str) -> bool {
    let Some(token) = name.split_whitespace().last() else {
        return false;
    };
    token.strip_prefix('^').is_some_and(|block_id| {
        !block_id.is_empty()
            && block_id.bytes().all(|byte| {
                byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_')
            })
    })
}

fn can_join(left: &Candidate, right: &Candidate, lines: &[&str]) -> bool {
    left.entry_position + 1 == right.entry_position
        && left.end_line <= right.line_index
        && lines[left.end_line..right.line_index]
            .iter()
            .all(|line| line.trim().is_empty())
        && left.end_minutes == right.start_minutes
        && left.clock == right.clock
        && left.components == right.components
        && left.child_indentation == right.child_indentation
}

fn merge_candidate<S: HomeSource>(
    members: &[Candidate],
    daily_path: &Path,
    resolver: &HomeResolver<S>,
) -> Option<Candidate> {
    let first = members.first()?;
    let last = members.last()?;
    let minutes = members
        .iter()
        .try_fold(0_u32, |sum, member| sum.checked_add(member.minutes))?;
    let parenthetical = format!(
        "(**{}-{}** [{}:: {}m])",
        first.start,
        last.end,
        first.clock.key(),
        minutes
    );
    let mut header = first.header.clone();
    header.replace_range(first.parenthetical.clone(), &parenthetical);
    let children = members
        .iter()
        .flat_map(|member| member.children.iter().cloned())
        .collect::<Vec<_>>();
    let mut synthetic_lines = Vec::with_capacity(children.len() + 1);
    synthetic_lines.push(header.clone());
    synthetic_lines.extend(children.iter().cloned());
    let synthetic = synthetic_lines.join("\n");
    let links = counted_session_links(&synthetic, 0);
    let merged_credit =
        closed_session_task_credit(&links, minutes, daily_path, resolver)?;
    let mut member_credit = BTreeMap::<String, u32>::new();
    for member in members {
        for (identity, value) in &member.credit {
            let sum = member_credit.entry(identity.clone()).or_default();
            *sum = sum.checked_add(*value)?;
        }
    }
    if member_credit != merged_credit
        || !theme_credit_matches(members, minutes, first.components.len())
    {
        return None;
    }
    let sources = members
        .iter()
        .flat_map(|member| member.sources.iter().cloned())
        .collect();
    Some(Candidate {
        entry_position: first.entry_position,
        line_index: first.line_index,
        end_line: last.end_line,
        header,
        parenthetical: first.parenthetical.clone(),
        start: first.start.clone(),
        end: last.end.clone(),
        start_minutes: first.start_minutes,
        end_minutes: last.end_minutes,
        clock: first.clock,
        minutes,
        name: first.name.clone(),
        components: first.components.clone(),
        child_indentation: first.child_indentation.clone(),
        children,
        credit: merged_credit,
        sources,
    })
}

fn theme_credit_matches(
    members: &[Candidate],
    total_minutes: u32,
    component_count: usize,
) -> bool {
    for count in 1..=component_count {
        let mut summed = vec![0_u32; count];
        for member in members {
            for (total, share) in
                summed.iter_mut().zip(split_minutes(member.minutes, count))
            {
                let Some(next) = total.checked_add(share) else {
                    return false;
                };
                *total = next;
            }
        }
        if summed != split_minutes(total_minutes, count) {
            return false;
        }
    }
    true
}

fn rewrite_runs(
    contents: &str,
    lines: &[&str],
    entries: &[PomodoroEntry],
    candidates: &[Option<Candidate>],
    runs: &BTreeMap<usize, Run>,
) -> PassOutcome {
    let segments = contents.split_inclusive('\n').collect::<Vec<_>>();
    let mut output = String::with_capacity(contents.len());
    let mut lineage = BTreeMap::new();
    let mut line_index = 0;
    let mut entry_position = 0;
    let mut output_line = 0;

    while line_index < segments.len() {
        if entry_position < entries.len()
            && entries[entry_position].line_index == line_index
        {
            if let Some(run) = runs.get(&entry_position) {
                let first =
                    run.members.first().expect("coalesced run is nonempty");
                let last =
                    run.members.last().expect("coalesced run is nonempty");
                lineage.insert(output_line, run.merged.sources.clone());
                let original_segment = segments[line_index];
                let original_line = logical_line(original_segment);
                let mut rewritten = original_line.to_string();
                let parenthetical = format!(
                    "(**{}-{}** [{}:: {}m])",
                    run.merged.start,
                    run.merged.end,
                    run.merged.clock.key(),
                    run.merged.minutes
                );
                rewritten
                    .replace_range(first.parenthetical.clone(), &parenthetical);
                output.push_str(&rewritten);
                output.push_str(line_terminator(original_segment));
                output_line += 1;

                for member in &run.members {
                    let entry = &entries[member.entry_position];
                    for segment in
                        &segments[entry.line_index + 1..entry.end_line]
                    {
                        output.push_str(segment);
                        output_line += 1;
                    }
                }
                line_index = entries[last.entry_position].end_line;
                entry_position = last.entry_position + 1;
                continue;
            }

            if let Some(candidate) = &candidates[entry_position] {
                lineage.insert(output_line, candidate.sources.clone());
            }
            output.push_str(segments[line_index]);
            output_line += 1;
            line_index += 1;
            entry_position += 1;
            continue;
        }

        output.push_str(segments[line_index]);
        output_line += 1;
        line_index += 1;
    }

    let _ = lines;
    PassOutcome {
        contents: output,
        lineage,
        coalesced: Vec::new(),
        changed: true,
    }
}

fn line_terminator(segment: &str) -> &str {
    if segment.ends_with("\r\n") {
        "\r\n"
    } else if segment.ends_with('\n') {
        "\n"
    } else {
        ""
    }
}

fn report_for(candidate: Candidate) -> CoalescedPomodoro {
    CoalescedPomodoro {
        name: candidate.name,
        span: format!("{}-{}", candidate.start, candidate.end),
        clock: candidate.clock.label().to_string(),
        minutes: candidate.minutes,
        sources: candidate.sources,
    }
}

#[cfg(test)]
mod tests {
    use std::{collections::BTreeMap, path::PathBuf};

    use crate::native::{
        pomodoro_homes::{HomeResolver, IndexedHomeSource},
        vault_links::NoteIndex,
    };

    use super::plan_coalesce;

    fn plan(contents: &str, notes: &[(&str, &str)]) -> super::CoalesceOutcome {
        let notes = notes
            .iter()
            .map(|(path, contents)| {
                (PathBuf::from(path), (*contents).to_string())
            })
            .collect::<BTreeMap<_, _>>();
        let index = NoteIndex::from_paths(notes.keys().cloned());
        let source = IndexedHomeSource::new(&index, &notes);
        let mut resolver = HomeResolver::new(source);
        plan_coalesce(
            contents,
            PathBuf::from("2026/20261105.md").as_path(),
            &mut resolver,
        )
    }

    #[test]
    fn coalesces_credit_neutral_rows_and_preserves_children_in_order() {
        let input = concat!(
            "## Pomodoros\n",
            "- [x] (**0840-0845** [t:: 5m]) — FIX\n",
            "  - 🍅 [[bob#^a]]\n",
            "- [x] (**0845-0850** [t:: 5m]) — fix\n",
            "  - 🍅 [[bob#^b]]\n",
            "- [x] (**0850-0855** [t:: 5m]) — FIX\n",
            "  - 🍅 [[bob#^c]]\n",
        );
        let outcome =
            plan(input, &[("bob.md", "- [ ] A ^a\n- [ ] B ^b\n- [ ] C ^c\n")]);
        assert_eq!(
            outcome.contents,
            concat!(
                "## Pomodoros\n",
                "- [x] (**0840-0855** [t:: 15m]) — FIX\n",
                "  - 🍅 [[bob#^a]]\n",
                "  - 🍅 [[bob#^b]]\n",
                "  - 🍅 [[bob#^c]]\n",
            )
        );
        assert_eq!(outcome.coalesced.len(), 1);
        assert_eq!(outcome.coalesced[0].span, "0840-0855");
        assert_eq!(outcome.coalesced[0].name, "FIX");
        assert_eq!(outcome.coalesced[0].clock, "worked");
        assert_eq!(outcome.coalesced[0].minutes, 15);
        assert_eq!(outcome.coalesced[0].sources.len(), 3);
    }

    #[test]
    fn exact_task_guard_refuses_a_join_that_changes_task_shares() {
        let input = concat!(
            "## Pomodoros\n",
            "- [x] (**0820-0825** [t:: 5m]) — FIX\n",
            "  - [[bob#^x]]\n",
            "- [x] (**0825-0840** [t:: 15m]) — FIX\n",
            "  - [[bob#^y]]\n",
            "  - [[bob#^z]]\n",
        );
        let outcome =
            plan(input, &[("bob.md", "- [ ] X ^x\n- [ ] Y ^y\n- [ ] Z ^z\n")]);
        assert_eq!(outcome.contents, input);
        assert!(outcome.coalesced.is_empty());
    }

    #[test]
    fn repeated_task_dedupes_and_unlinked_sessions_coalesce() {
        let repeated = concat!(
            "## Pomodoros\n",
            "- [x] (**0900-0905** [t:: 5m]) — FIX\n",
            "  - [[bob#^x]]\n",
            "- [x] (**0905-0910** [t:: 5m]) — FIX\n",
            "  - [[bob.md#^x]]\n",
        );
        let outcome = plan(repeated, &[("bob.md", "- [ ] X ^x\n")]);
        assert!(outcome.contents.contains("(**0900-0910** [t:: 10m])"));

        let unlinked = concat!(
            "## Pomodoros\n",
            "- [x] (**0900-0905** [off:: 5m]) — FIX\n",
            "  - note A\n",
            "- [x] (**0905-0910** [off:: 5m]) — FIX\n",
            "  - note B\n",
        );
        let outcome = plan(unlinked, &[]);
        assert!(outcome.contents.contains("(**0900-0910** [off:: 10m])"));
        assert_eq!(outcome.coalesced[0].clock, "off");
    }

    #[test]
    fn component_guard_and_fixed_point_are_exact_and_idempotent() {
        let theme = concat!(
            "## Pomodoros\n",
            "- [x] (**0900-0905** [t:: 5m]) — PLAN + BUILD\n",
            "  - [[bob#^x]]\n",
            "- [x] (**0905-0910** [t:: 5m]) — PLAN + BUILD\n",
            "  - [[bob#^x]]\n",
        );
        assert_eq!(plan(theme, &[("bob.md", "- [ ] X ^x\n")]).contents, theme);

        let exact_theme = theme
            .replace("[t:: 5m]", "[t:: 4m]")
            .replace("0900-0905", "0900-0904")
            .replace("0905-0910", "0904-0908");
        let exact = plan(&exact_theme, &[("bob.md", "- [ ] X ^x\n")]);
        assert!(exact.contents.contains("(**0900-0908** [t:: 8m])"));

        let fixed_point = concat!(
            "## Pomodoros\n",
            "- [x] (**0800-0810** [t:: 10m]) — FIX\n",
            "  - [[bob#^x]]\n",
            "  - [[bob#^y]]\n",
            "- [x] (**0810-0815** [t:: 5m]) — FIX\n",
            "  - [[bob#^x]]\n",
            "- [x] (**0815-0820** [t:: 5m]) — FIX\n",
            "  - [[bob#^y]]\n",
        );
        let notes = [("bob.md", "- [ ] X ^x\n- [ ] Y ^y\n")];
        let once = plan(fixed_point, &notes);
        assert!(once.contents.contains("(**0800-0820** [t:: 20m])"));
        assert_eq!(once.coalesced[0].sources.len(), 3);
        let twice = plan(&once.contents, &notes);
        assert_eq!(twice.contents, once.contents);
        assert!(twice.coalesced.is_empty());
    }

    #[test]
    fn rejects_gaps_types_clock_meet_open_legacy_ids_midnight_and_indent_mix() {
        let cases = [
            (
                "- [x] (**0900-0905** [t:: 5m]) — FIX\n  - [[bob#^x]]\n\n- [x] (**0906-0911** [t:: 5m]) — FIX\n  - [[bob#^y]]\n",
                "gap",
            ),
            (
                "- [x] (**0900-0905** [t:: 5m]) — FIX\n  - [[bob#^x]]\n- [x] (**0905-0910** [t:: 5m]) — CHORE\n  - [[bob#^y]]\n",
                "types",
            ),
            (
                "- [x] (**0900-0905** [t:: 5m]) — FIX\n  - [[bob#^x]]\n- [x] (**0905-0910** [off:: 5m]) — FIX\n  - [[bob#^y]]\n",
                "clock",
            ),
            (
                "- [x] (**0900-0905** [off:: 5m]) — MEET\n  - [[bob#^x]]\n- [x] (**0905-0910** [off:: 5m]) — MEET\n  - [[bob#^y]]\n",
                "meet",
            ),
            (
                "- [ ] (**0900-0905** [t:: 5m]) — FIX\n  - [[bob#^x]]\n- [x] (**0905-0910** [t:: 5m]) — FIX\n  - [[bob#^y]]\n",
                "open",
            ),
            (
                "- [x] (**0900-0905**) — FIX\n  - [[bob#^x]]\n- [x] (**0905-0910**) — FIX\n  - [[bob#^y]]\n",
                "legacy",
            ),
            (
                "- [x] (**0900-0905** [t:: 5m]) — FIX ^block\n  - [[bob#^x]]\n- [x] (**0905-0910** [t:: 5m]) — FIX\n  - [[bob#^y]]\n",
                "block id",
            ),
            (
                "- [x] (**2355-0005** [t:: 10m]) — FIX\n  - [[bob#^x]]\n- [x] (**0005-0010** [t:: 5m]) — FIX\n  - [[bob#^y]]\n",
                "midnight",
            ),
            (
                "- [x] (**0900-0905** [t:: 5m]) — FIX\n  - [[bob#^x]]\n- [x] (**0905-0910** [t:: 5m]) — FIX\n\t- [[bob#^y]]\n",
                "indentation",
            ),
        ];
        for (rows, label) in cases {
            let input = format!("## Pomodoros\n{rows}");
            let outcome =
                plan(&input, &[("bob.md", "- [ ] X ^x\n- [ ] Y ^y\n")]);
            assert_eq!(outcome.contents, input, "case: {label}");
            assert!(outcome.coalesced.is_empty(), "case: {label}");
        }
    }

    #[test]
    fn preserves_crlf_and_child_interior_blank_lines() {
        let input = concat!(
            "## Pomodoros\r\n",
            "- [x] (**0900-0905** [t:: 5m]) — FIX\r\n",
            "  - [[bob#^x]]\r\n",
            "  - note\r\n",
            "\r\n",
            "  - continuation\r\n",
            "- [x] (**0905-0910** [t:: 5m]) — FIX\r\n",
            "  - [[bob#^x]]\r\n",
        );
        let outcome = plan(input, &[("bob.md", "- [ ] X ^x\n")]);
        assert!(outcome.contents.contains("\r\n"));
        assert!(!outcome.contents.replace("\r\n", "").contains('\n'));
        assert!(outcome
            .contents
            .contains("  - note\r\n\r\n  - continuation\r\n"));
        assert!(outcome.contents.contains("(**0900-0910** [t:: 10m])"));
    }
}
