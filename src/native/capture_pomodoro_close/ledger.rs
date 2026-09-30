//! Running-Pomodoro discovery and ledger close planning.

use std::{collections::BTreeSet, ops::Range};

use chrono::{NaiveDateTime, Timelike};

use super::super::{
    capture::{
        adjustment_duration_for_range, format_adjusted_range,
        leading_spaces_or_tabs_len, line_spans, list_item_body,
        list_marker_len, nearest_shallower_list_item_parent,
        parse_adjustment_range, AdjustRange, LineSpan,
    },
    capture_pomodoros, markdown, pomodoro,
};
use super::links::{move_only_destination, rewrite_completed_markers};
use super::{
    bare_plain_link, dropped_plain_link, range_is_struck,
    strikethrough_inner_spans, strip_pomodoro_markers, wikilink_tokens,
    WikiToken,
};

const PLACEHOLDER_LINE: &str = "- [ ] ()";
const EMPTY_SUB_BULLET: &str = "\t- ";
const HALF_DAY_MINUTES: i64 = 720;
const DAY_MINUTES: i64 = 1440;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RunningPomodoro {
    pub line: usize,
    pub name: Option<String>,
    pub time_range: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct NamedPomodoro {
    pub name: Option<String>,
    pub line: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum FindRunningError {
    NoSection,
    NoneRunning {
        next_name: Option<String>,
        next_line: Option<usize>,
    },
    Multiple(Vec<NamedPomodoro>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CloseTiming {
    pub planned_start: u64,
    pub planned_end: u64,
    pub planned_duration: u64,
    pub closed_start: u64,
    pub closed_end: u64,
    pub closed_duration: u64,
    pub closed_at: u64,
    pub remaining_minutes: i64,
    pub decremented_minutes: u64,
    pub new_range_text: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LedgerLinkRole {
    Worked,
    Mentioned,
    Deferred,
    Dropped,
    Struck,
    Embedded,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ClassifiedLink {
    pub line: usize,
    pub role: LedgerLinkRole,
    pub raw_target: String,
    pub path_part: String,
    pub block_id: String,
    pub carried: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BlockLinkTarget {
    pub line: usize,
    pub path_part: String,
    pub block_id: String,
    pub wikilink: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct NextPomodoro {
    pub line: usize,
    pub name: Option<String>,
    pub time_range: Option<String>,
    pub created: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct WorkLogNode {
    pub marker: String,
    pub body_text: String,
    pub children: Vec<WorkLogNode>,
    /// 1-based day-note line this root was collected from (`None` for
    /// nested nodes). Typed close entries claim dated strings by it.
    pub source_line: Option<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct WorkLogNoteGroup {
    pub source_line: usize,
    pub path_part: String,
    pub block_id: String,
    pub descendant_roots: Vec<WorkLogNode>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LedgerClosePlan {
    pub contents: String,
    pub timing: Option<CloseTiming>,
    pub unparseable_range: bool,
    pub classified_links: Vec<ClassifiedLink>,
    pub carried_lines: Vec<String>,
    pub notes: Vec<String>,
    pub next_pomodoro: Option<NextPomodoro>,
    pub startable_targets: Vec<BlockLinkTarget>,
    pub embedded_targets: Vec<BlockLinkTarget>,
    pub work_log_groups: Vec<WorkLogNoteGroup>,
    pub sub_bullet_range: Range<usize>,
}

pub(crate) fn find_running_pomodoro(
    contents: &str,
) -> Result<RunningPomodoro, FindRunningError> {
    let scan = capture_pomodoros::scan(contents);
    if !scan.has_section {
        return Err(FindRunningError::NoSection);
    }
    let timed_open = scan
        .entries
        .iter()
        .filter(|entry| {
            entry.state == capture_pomodoros::PomodoroState::Open
                && entry.time_range.is_some()
        })
        .collect::<Vec<_>>();
    match timed_open.as_slice() {
        [entry] => Ok(RunningPomodoro {
            line: entry.line,
            name: entry.name.clone(),
            time_range: entry.time_range.clone().unwrap_or_default(),
        }),
        [] => {
            let next = capture_pomodoros::next_future_pomodoro(&scan);
            Err(FindRunningError::NoneRunning {
                next_name: next.and_then(|entry| entry.name.clone()),
                next_line: next.map(|entry| entry.line),
            })
        }
        many => Err(FindRunningError::Multiple(
            many.iter()
                .map(|entry| NamedPomodoro {
                    name: entry.name.clone(),
                    line: entry.line,
                })
                .collect(),
        )),
    }
}

pub(crate) fn close_timing(
    range: &AdjustRange,
    now: NaiveDateTime,
) -> CloseTiming {
    let closed_at = u64::from(now.hour()) * 60 + u64::from(now.minute());
    let duration = adjustment_duration_for_range(range);
    let remaining = signed_minute_delta(range.end_minutes, closed_at);
    let mut timing = CloseTiming {
        planned_start: range.start_minutes,
        planned_end: range.end_minutes,
        planned_duration: duration,
        closed_start: range.start_minutes,
        closed_end: range.end_minutes,
        closed_duration: duration,
        closed_at,
        remaining_minutes: remaining,
        decremented_minutes: 0,
        new_range_text: None,
    };
    if remaining < 5 {
        return timing;
    }
    let Some(delta) = remaining
        .checked_div(5)
        .and_then(|units| units.checked_mul(5))
    else {
        return timing;
    };
    let Ok(delta) = u64::try_from(delta) else {
        return timing;
    };
    let new_duration = duration.saturating_sub(delta);
    let Some(new_end_total) = range.start_minutes.checked_add(new_duration)
    else {
        return timing;
    };
    let new_end = new_end_total % 1440;
    let Ok(new_end_u16) = u16::try_from(new_end) else {
        return timing;
    };
    timing.closed_end = new_end;
    timing.closed_duration = new_duration;
    timing.decremented_minutes = duration.saturating_sub(new_duration);
    timing.new_range_text = Some(format_adjusted_range(
        range.start_minutes,
        new_end_u16,
        new_duration,
        &range.metadata,
    ));
    timing
}

pub(crate) fn sub_bullet_range(
    lines: &[&str],
    entry_index: usize,
) -> Range<usize> {
    let section_end = pomodoro::pomodoros_section_range(lines)
        .map(|section| section.end)
        .unwrap_or(lines.len());
    let start = entry_index + 1;
    let mut end = start;
    while end < section_end && end < lines.len() {
        let line = lines[end];
        if line.trim().is_empty() || !is_indented_list_line(line) {
            break;
        }
        end += 1;
    }
    start..end
}

pub(crate) fn plan_ledger_close(
    contents: &str,
    entry: &RunningPomodoro,
    now: NaiveDateTime,
) -> LedgerClosePlan {
    let spans = line_spans(contents);
    let line_text = spans.iter().map(|line| line.text).collect::<Vec<_>>();
    let entry_index = entry.line.saturating_sub(1);
    let range = sub_bullet_range(&line_text, entry_index);
    let fenced = markdown::fenced_lines(&line_text, 0..line_text.len());
    let classified = classify_sub_bullets(&line_text, &range, &fenced);
    let work_log_groups =
        collect_work_log_note_groups(&spans, entry_index, &range, &fenced);
    let notes = classified
        .iter()
        .filter(|bullet| {
            bullet.kind == BulletKind::Note
                && !fenced.contains(&bullet.line)
                && nearest_shallower_list_item_parent(&spans, bullet.line)
                    == Some(entry_index)
                && wikilink_tokens(&bullet.stripped).is_empty()
        })
        .filter_map(|bullet| {
            let body = list_item_body(&bullet.stripped).map(str::trim)?;
            (!body.is_empty()).then(|| body.to_string())
        })
        .collect::<Vec<_>>();

    let entry_line = line_text.get(entry_index).copied().unwrap_or("");
    let parsed_range = parse_adjustment_range(entry_line);
    let (timing, unparseable_range) = match parsed_range.as_ref() {
        Some(parsed) => (Some(close_timing(parsed, now)), false),
        None => (None, true),
    };

    let mut classified_links = Vec::new();
    let mut startable_targets = Vec::new();
    let mut embedded_targets = Vec::new();
    for bullet in &classified {
        collect_links_from_bullet(
            bullet,
            &mut classified_links,
            &mut startable_targets,
            &mut embedded_targets,
        );
    }

    let mut carried_lines = Vec::new();
    for bullet in classified
        .iter()
        .filter(|bullet| bullet.kind == BulletKind::WorkedOn)
    {
        carried_lines.push(bullet.stripped.clone());
    }
    let mut deferred_lines = BTreeSet::new();
    for bullet in classified
        .iter()
        .filter(|bullet| bullet.kind == BulletKind::Deferred)
    {
        deferred_lines.insert(bullet.line);
        if let Some(destination) = &bullet.deferred_destination {
            carried_lines.push(destination.clone());
        }
    }
    // Dropped links are removed from the closed session like deferred
    // ones, except nothing is carried: the `~` marker never reaches the
    // day note.
    let mut dropped_lines = BTreeSet::new();
    for bullet in classified
        .iter()
        .filter(|bullet| bullet.kind == BulletKind::Dropped)
    {
        dropped_lines.insert(bullet.line);
    }
    let removed_lines: BTreeSet<usize> =
        deferred_lines.union(&dropped_lines).copied().collect();

    let later_entry = find_next_pomodoro_line(&line_text, entry_index);
    let should_create = !carried_lines.is_empty() || later_entry.is_none();

    let mut output_lines = Vec::new();
    for (index, line) in line_text.iter().enumerate() {
        if index >= range.start {
            break;
        }
        if index == entry_index {
            output_lines.push(close_entry_line(line, timing.as_ref()));
        } else {
            output_lines.push((*line).to_string());
        }
    }
    for index in range.clone() {
        let line = line_text[index];
        if removed_lines.contains(&index) {
            continue;
        }
        if fenced.contains(&index) {
            output_lines.push(line.to_string());
            continue;
        }
        output_lines.push(rewrite_completed_markers(line));
    }

    let mut created_next = None;
    if should_create {
        let placeholder = match entry.name.as_deref() {
            Some(name) => {
                capture_pomodoros::format_named_placeholder_line(name)
            }
            None => PLACEHOLDER_LINE.to_string(),
        };
        let placeholder_index = output_lines.len();
        output_lines.push(placeholder);
        if carried_lines.is_empty() {
            output_lines.push(EMPTY_SUB_BULLET.to_string());
        } else {
            output_lines.extend(carried_lines.iter().cloned());
        }
        created_next = Some(NextPomodoro {
            line: placeholder_index + 1,
            name: entry.name.clone(),
            time_range: None,
            created: true,
        });
    }
    output_lines.extend(
        line_text[range.end..]
            .iter()
            .map(|line| (*line).to_string()),
    );

    let next_pomodoro = created_next.or_else(|| {
        later_entry.map(|later_index| {
            let shifted = later_index - removed_lines.len();
            let line = line_text[later_index];
            NextPomodoro {
                line: shifted + 1,
                name: pomodoro_line_name(line),
                time_range: pomodoro_line_time_range(line),
                created: false,
            }
        })
    });

    let ending = line_ending(contents);
    let had_final_newline = contents.ends_with('\n');
    let mut rebuilt = output_lines.join(ending);
    if had_final_newline {
        rebuilt.push_str(ending);
    }

    LedgerClosePlan {
        contents: rebuilt,
        timing,
        unparseable_range,
        classified_links,
        carried_lines,
        notes,
        next_pomodoro,
        startable_targets,
        embedded_targets,
        work_log_groups,
        sub_bullet_range: range,
    }
}

fn signed_minute_delta(end_minutes: u64, now_minutes: u64) -> i64 {
    let delta = end_minutes as i64 - now_minutes as i64;
    let mut wrapped = delta.rem_euclid(DAY_MINUTES);
    if wrapped > HALF_DAY_MINUTES {
        wrapped -= DAY_MINUTES;
    }
    wrapped
}

fn line_ending(contents: &str) -> &'static str {
    if contents.contains("\r\n") {
        "\r\n"
    } else {
        "\n"
    }
}

fn is_indented_list_line(line: &str) -> bool {
    let indent = leading_spaces_or_tabs_len(line);
    if indent == 0 {
        return false;
    }
    let after_indent = &line[indent..];
    let Some(marker_len) = list_marker_len(after_indent) else {
        return false;
    };
    let rest = &after_indent[marker_len..];
    rest.is_empty() || rest.starts_with([' ', '\t'])
}

fn is_top_level_task_line(line: &str) -> bool {
    if leading_spaces_or_tabs_len(line) > 0 {
        return false;
    }
    let Some(marker_len) = list_marker_len(line) else {
        return false;
    };
    let rest = &line[marker_len..];
    let ws = leading_spaces_or_tabs_len(rest);
    if ws == 0 {
        return false;
    }
    let after = &rest[ws..];
    let Some(inner) = after.strip_prefix('[') else {
        return false;
    };
    let mut chars = inner.chars();
    let Some(status) = chars.next() else {
        return false;
    };
    status != ']' && chars.as_str().starts_with(']')
}

fn find_next_pomodoro_line(
    lines: &[&str],
    entry_index: usize,
) -> Option<usize> {
    let section_end = pomodoro::pomodoros_section_range(lines)
        .map(|section| section.end)
        .unwrap_or(lines.len());
    ((entry_index + 1)..section_end)
        .find(|&index| is_top_level_task_line(lines[index]))
}

fn pomodoro_line_name(line: &str) -> Option<String> {
    let body = pomodoro::open_ledger_task(line)
        .or_else(|| pomodoro::completed_ledger_task(line))?;
    let remaining = match body.find(')') {
        Some(close) => &body[close + 1..],
        None => body,
    };
    capture_pomodoros::parse_name_tail(remaining)
}

fn pomodoro_line_time_range(line: &str) -> Option<String> {
    parse_adjustment_range(line).map(|range| {
        format!(
            "{:02}{:02}-{:02}{:02}",
            range.start_minutes / 60,
            range.start_minutes % 60,
            range.end_minutes / 60,
            range.end_minutes % 60
        )
    })
}

fn close_entry_line(line: &str, timing: Option<&CloseTiming>) -> String {
    let mut updated = line.to_string();
    if let Some(timing) = timing
        && let Some(new_range) = &timing.new_range_text
        && let Some(range) = parse_adjustment_range(line)
    {
        updated = format!(
            "{}{}{}",
            &line[..range.start_ch],
            new_range,
            &line[range.end_ch..]
        );
    }
    close_entry_checkbox(&updated)
}

fn close_entry_checkbox(line: &str) -> String {
    let Some(rest) = line.strip_prefix("- [") else {
        return line.to_string();
    };
    if rest.starts_with(" ]") {
        format!("- [x{}", &rest[1..])
    } else {
        line.to_string()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BulletKind {
    Fenced,
    Embedded,
    Deferred,
    Dropped,
    WorkedOn,
    Note,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ClassifiedBullet {
    line: usize,
    kind: BulletKind,
    stripped: String,
    deferred_destination: Option<String>,
    startable: bool,
}

fn classify_sub_bullets(
    lines: &[&str],
    range: &Range<usize>,
    fenced: &BTreeSet<usize>,
) -> Vec<ClassifiedBullet> {
    let mut bullets = Vec::new();
    for index in range.clone() {
        let original = lines[index];
        let stripped = strip_pomodoro_markers(original);
        if fenced.contains(&index) {
            bullets.push(ClassifiedBullet {
                line: index,
                kind: BulletKind::Fenced,
                stripped,
                deferred_destination: None,
                startable: false,
            });
            continue;
        }
        let tokens = wikilink_tokens(&stripped);
        if tokens.iter().any(|token| token.embedded) {
            bullets.push(ClassifiedBullet {
                line: index,
                kind: BulletKind::Embedded,
                stripped,
                deferred_destination: None,
                startable: false,
            });
            continue;
        }
        if let Some(destination) = move_only_destination(&stripped) {
            bullets.push(ClassifiedBullet {
                line: index,
                kind: BulletKind::Deferred,
                stripped,
                deferred_destination: Some(destination),
                startable: false,
            });
            continue;
        }
        if dropped_plain_link(&stripped).is_some() {
            bullets.push(ClassifiedBullet {
                line: index,
                kind: BulletKind::Dropped,
                stripped,
                deferred_destination: None,
                startable: false,
            });
            continue;
        }
        let strike_spans = strikethrough_inner_spans(&stripped);
        let has_unstruck_plain = tokens.iter().any(|token| {
            !token.embedded
                && !range_is_struck(token.start, token.end, &strike_spans)
        });
        if has_unstruck_plain {
            let startable = bare_plain_link(&stripped).is_some();
            bullets.push(ClassifiedBullet {
                line: index,
                kind: BulletKind::WorkedOn,
                stripped,
                deferred_destination: None,
                startable,
            });
            continue;
        }
        bullets.push(ClassifiedBullet {
            line: index,
            kind: BulletKind::Note,
            stripped,
            deferred_destination: None,
            startable: false,
        });
    }
    bullets
}

fn collect_links_from_bullet(
    bullet: &ClassifiedBullet,
    classified_links: &mut Vec<ClassifiedLink>,
    startable_targets: &mut Vec<BlockLinkTarget>,
    embedded_targets: &mut Vec<BlockLinkTarget>,
) {
    let tokens = wikilink_tokens(&bullet.stripped);
    let pre_image_line = bullet.line + 1;
    match bullet.kind {
        BulletKind::Fenced => {}
        BulletKind::Embedded => {
            for token in tokens.iter().filter(|token| token.embedded) {
                classified_links.push(link_from_token(
                    pre_image_line,
                    token,
                    LedgerLinkRole::Embedded,
                    false,
                ));
                embedded_targets.push(target_from_token(pre_image_line, token));
            }
        }
        BulletKind::Deferred => {
            if let Some(token) = tokens.iter().find(|token| !token.embedded) {
                classified_links.push(link_from_token(
                    pre_image_line,
                    token,
                    LedgerLinkRole::Deferred,
                    true,
                ));
            }
        }
        BulletKind::Dropped => {
            // Removed from the closed session: reported, but never
            // carried to the next placeholder and never started.
            if let Some(token) = dropped_plain_link(&bullet.stripped) {
                classified_links.push(link_from_token(
                    pre_image_line,
                    &token,
                    LedgerLinkRole::Dropped,
                    false,
                ));
            }
        }
        BulletKind::WorkedOn => {
            let strike_spans = strikethrough_inner_spans(&bullet.stripped);
            let role = if bullet.startable {
                LedgerLinkRole::Worked
            } else {
                LedgerLinkRole::Mentioned
            };
            for token in tokens.iter().filter(|token| {
                !token.embedded
                    && !range_is_struck(token.start, token.end, &strike_spans)
            }) {
                classified_links.push(link_from_token(
                    pre_image_line,
                    token,
                    role,
                    true,
                ));
                if bullet.startable {
                    startable_targets
                        .push(target_from_token(pre_image_line, token));
                }
            }
        }
        BulletKind::Note => {
            let strike_spans = strikethrough_inner_spans(&bullet.stripped);
            for token in tokens.iter().filter(|token| {
                !token.embedded
                    && range_is_struck(token.start, token.end, &strike_spans)
            }) {
                classified_links.push(link_from_token(
                    pre_image_line,
                    token,
                    LedgerLinkRole::Struck,
                    false,
                ));
            }
        }
    }
}

fn link_from_token(
    line: usize,
    token: &WikiToken,
    role: LedgerLinkRole,
    carried: bool,
) -> ClassifiedLink {
    ClassifiedLink {
        line,
        role,
        raw_target: token.token.clone(),
        path_part: token.path_part.clone(),
        block_id: token.block_id.clone(),
        carried,
    }
}

pub(super) fn target_from_token(
    line: usize,
    token: &WikiToken,
) -> BlockLinkTarget {
    BlockLinkTarget {
        line,
        path_part: token.path_part.clone(),
        block_id: token.block_id.clone(),
        wikilink: token.token.clone(),
    }
}

fn work_log_task_link_target(line: &str) -> Option<WikiToken> {
    let stripped = strip_pomodoro_markers(line);
    let indent = leading_spaces_or_tabs_len(&stripped);
    let after_indent = &stripped[indent..];
    let marker_len = list_marker_len(after_indent)?;
    let after_marker = &after_indent[marker_len..];
    let whitespace = leading_spaces_or_tabs_len(after_marker);
    if whitespace == 0 {
        return None;
    }
    let mut start = indent + marker_len + whitespace;
    let mut end = stripped.len();
    while end > start && matches!(stripped.as_bytes()[end - 1], b' ' | b'\t') {
        end -= 1;
    }
    while start < end && matches!(stripped.as_bytes()[start], b' ' | b'\t') {
        start += 1;
    }
    if start >= end {
        return None;
    }
    if stripped.as_bytes()[end - 1] == b'#' {
        end -= 1;
        while end > start
            && matches!(stripped.as_bytes()[end - 1], b' ' | b'\t')
        {
            end -= 1;
        }
    }
    if start >= end {
        return None;
    }
    if end - start >= 4
        && stripped[start..].starts_with("~~")
        && stripped[..end].ends_with("~~")
    {
        start += 2;
        end -= 2;
        while end > start
            && matches!(stripped.as_bytes()[end - 1], b' ' | b'\t')
        {
            end -= 1;
        }
        while start < end && matches!(stripped.as_bytes()[start], b' ' | b'\t')
        {
            start += 1;
        }
    }
    if start >= end {
        return None;
    }
    let mut tokens = wikilink_tokens(&stripped);
    if tokens.len() != 1 {
        return None;
    }
    let token = tokens.pop()?;
    (token.start == start && token.end == end).then_some(token)
}

fn child_block_end_line(lines: &[&str], parent: usize) -> usize {
    let parent_indent = leading_spaces_or_tabs_len(lines[parent]);
    let mut end = parent;
    for (index, line) in lines.iter().enumerate().skip(parent + 1) {
        if line.trim().is_empty() {
            continue;
        }
        if leading_spaces_or_tabs_len(line) > parent_indent {
            end = index;
            continue;
        }
        break;
    }
    end
}

fn parse_list_prefix(line: &str) -> (String, String) {
    let indent = leading_spaces_or_tabs_len(line);
    let after = &line[indent..];
    if let Some(marker_len) = list_marker_len(after) {
        let marker = after[..marker_len].to_string();
        let rest = after[marker_len..].trim();
        return (marker, rest.to_string());
    }
    ("-".to_string(), line.trim().to_string())
}

fn collect_descendant_tree(
    lines: &[&str],
    sub_bullet: usize,
    end_line: usize,
) -> Vec<WorkLogNode> {
    struct Frame {
        indent: usize,
        nodes: Vec<WorkLogNode>,
    }
    let root_indent = leading_spaces_or_tabs_len(lines[sub_bullet]);
    let mut stack = vec![Frame {
        indent: root_indent,
        nodes: Vec::new(),
    }];
    for (offset, raw) in lines
        .iter()
        .enumerate()
        .take(end_line + 1)
        .skip(sub_bullet + 1)
    {
        if raw.trim().is_empty() {
            continue;
        }
        let indent = leading_spaces_or_tabs_len(raw);
        while stack.len() > 1
            && stack.last().is_some_and(|frame| frame.indent >= indent)
        {
            let frame = stack.pop().expect("stack len > 1");
            if let Some(parent) = stack.last_mut()
                && let Some(node) = parent.nodes.last_mut()
            {
                node.children = frame.nodes;
            }
        }
        if stack.first().is_some_and(|frame| indent <= frame.indent) {
            continue;
        }
        let (marker, body_text) = parse_list_prefix(raw);
        // Only depth-one roots carry a source line; nested nodes never do.
        let source_line = (stack.len() == 1).then_some(offset + 1);
        if let Some(parent) = stack.last_mut() {
            parent.nodes.push(WorkLogNode {
                marker,
                body_text,
                children: Vec::new(),
                source_line,
            });
        }
        stack.push(Frame {
            indent,
            nodes: Vec::new(),
        });
    }
    while stack.len() > 1 {
        let frame = stack.pop().expect("stack len > 1");
        if let Some(parent) = stack.last_mut()
            && let Some(node) = parent.nodes.last_mut()
        {
            node.children = frame.nodes;
        }
    }
    stack.pop().map(|frame| frame.nodes).unwrap_or_default()
}

fn collect_work_log_note_groups(
    spans: &[LineSpan<'_>],
    entry_index: usize,
    range: &Range<usize>,
    fenced: &BTreeSet<usize>,
) -> Vec<WorkLogNoteGroup> {
    let lines = spans.iter().map(|span| span.text).collect::<Vec<_>>();
    let mut groups = Vec::new();
    for index in range.clone() {
        if fenced.contains(&index) || lines[index].trim().is_empty() {
            continue;
        }
        if nearest_shallower_list_item_parent(spans, index) != Some(entry_index)
        {
            continue;
        }
        let Some(target) = work_log_task_link_target(lines[index]) else {
            continue;
        };
        let end_line = child_block_end_line(&lines, index);
        let descendant_roots = collect_descendant_tree(&lines, index, end_line);
        if descendant_roots.is_empty() {
            continue;
        }
        groups.push(WorkLogNoteGroup {
            source_line: index + 1,
            path_part: target.path_part,
            block_id: target.block_id,
            descendant_roots,
        });
    }
    groups
}
