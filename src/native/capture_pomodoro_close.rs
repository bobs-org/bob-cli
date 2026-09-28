//! Pure Pomodoro-close ledger planner: find the running entry, auto-decrement
//! its range, and rewrite today's daily note the way Obsidian's completion
//! does. Linked-task effects and Work Log writes belong to a later phase; this
//! module only exposes those planned targets as data.

#![allow(dead_code)]

use std::{cmp::Reverse, collections::BTreeSet, ops::Range};

use chrono::{NaiveDateTime, Timelike};

use super::{
    capture::{
        adjustment_duration_for_range, format_adjusted_range,
        leading_spaces_or_tabs_len, line_spans, list_item_body,
        list_marker_len, nearest_shallower_list_item_parent,
        parse_adjustment_range, AdjustRange, LineSpan,
    },
    capture_language, capture_pomodoros, markdown, pomodoro,
};

const POMODORO_MARKER: &str = "🍅";
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
            let next = scan.entries.iter().find(|entry| {
                entry.state == capture_pomodoros::PomodoroState::Open
                    && entry.placeholder
            });
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
        if deferred_lines.contains(&index) {
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
            let shifted = later_index - deferred_lines.len();
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

fn target_from_token(line: usize, token: &WikiToken) -> BlockLinkTarget {
    BlockLinkTarget {
        line,
        path_part: token.path_part.clone(),
        block_id: token.block_id.clone(),
        wikilink: token.token.clone(),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MarkerPolicy {
    Strip,
    Completed,
}

fn strip_pomodoro_markers(line: &str) -> String {
    rewrite_markers(line, MarkerPolicy::Strip)
}

fn rewrite_completed_markers(line: &str) -> String {
    rewrite_markers(line, MarkerPolicy::Completed)
}

fn rewrite_markers(line: &str, policy: MarkerPolicy) -> String {
    let tokens = wikilink_tokens(line);
    let strike_spans = strikethrough_inner_spans(line);
    let mut edits = Vec::new();
    for token in &tokens {
        let exact = exact_struck(token, &strike_spans);
        let token_start = if exact {
            token.start.saturating_sub(2)
        } else {
            token.start
        };
        let prefix = pomodoro_marker_prefix(line, token_start);
        let marked = match policy {
            MarkerPolicy::Strip => false,
            MarkerPolicy::Completed => {
                if token.embedded {
                    false
                } else if exact {
                    prefix.count > 0
                } else {
                    true
                }
            }
        };
        let replacement = if marked {
            format!("{POMODORO_MARKER} ")
        } else {
            String::new()
        };
        if (marked && prefix.canonical) || (!marked && prefix.count == 0) {
            continue;
        }
        edits.push((prefix.start, token_start, replacement));
    }
    apply_edits(line, edits)
}

struct MarkerPrefix {
    start: usize,
    count: usize,
    canonical: bool,
}

fn pomodoro_marker_prefix(line: &str, token_start: usize) -> MarkerPrefix {
    let token_start = token_start.min(line.len());
    let mut start = token_start;
    let mut count = 0;
    loop {
        let prefix = &line[..start];
        let without_ws = prefix.trim_end_matches([' ', '\t']);
        if without_ws.len() == prefix.len() {
            break;
        }
        if !without_ws.ends_with(POMODORO_MARKER) {
            break;
        }
        start = without_ws.len() - POMODORO_MARKER.len();
        count += 1;
    }
    MarkerPrefix {
        start,
        count,
        canonical: count == 1 && line.get(start..token_start) == Some("🍅 "),
    }
}

fn apply_edits(line: &str, mut edits: Vec<(usize, usize, String)>) -> String {
    edits.sort_by_key(|right| Reverse(right.0));
    let mut rewritten = line.to_string();
    for (start, end, text) in edits {
        if start <= end && end <= rewritten.len() {
            rewritten.replace_range(start..end, &text);
        }
    }
    rewritten
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct WikiToken {
    start: usize,
    end: usize,
    embedded: bool,
    path_part: String,
    block_id: String,
    token: String,
}

struct ParsedTarget {
    path_part: String,
    block_id: String,
}

fn wikilink_tokens(line: &str) -> Vec<WikiToken> {
    let mut tokens = Vec::new();
    let mut cursor = 0;
    while let Some(relative) = line[cursor..].find("[[") {
        let open = cursor + relative;
        let inner_start = open + 2;
        let Some(relative_close) = line[inner_start..].find("]]") else {
            break;
        };
        let close = inner_start + relative_close + 2;
        let inner = &line[inner_start..inner_start + relative_close];
        let raw_target = inner.split('|').next().unwrap_or("").trim();
        if let Some(parsed) = parse_block_target(raw_target) {
            let embedded = open > 0 && line.as_bytes()[open - 1] == b'!';
            let start = if embedded { open - 1 } else { open };
            tokens.push(WikiToken {
                start,
                end: close,
                embedded,
                path_part: parsed.path_part,
                block_id: parsed.block_id,
                token: line[open..close].to_string(),
            });
        }
        cursor = close;
    }
    tokens
}

fn parse_block_target(raw_target: &str) -> Option<ParsedTarget> {
    let target = normalize_transcluded_link_target(raw_target);
    let marker = target.find("#^")?;
    let raw_path = target[..marker].trim();
    let block_id = target[marker + 2..].trim();
    if !capture_language::is_block_id(block_id)
        || raw_path.contains('#')
        || raw_path.contains('^')
        || is_uri_scheme(raw_path)
    {
        return None;
    }
    Some(ParsedTarget {
        path_part: strip_markdown_extension(raw_path),
        block_id: block_id.to_string(),
    })
}

fn normalize_transcluded_link_target(value: &str) -> String {
    let mut target = strip_wrapping_quotes(value.trim()).to_string();
    if target.starts_with('<') && target.ends_with('>') && target.len() >= 2 {
        target = target[1..target.len() - 1].trim().to_string();
    }
    safe_decode_uri(&target)
}

fn strip_wrapping_quotes(value: &str) -> &str {
    let bytes = value.as_bytes();
    if bytes.len() >= 2
        && ((bytes[0] == b'"' && bytes[bytes.len() - 1] == b'"')
            || (bytes[0] == b'\'' && bytes[bytes.len() - 1] == b'\''))
    {
        value[1..value.len() - 1].trim()
    } else {
        value
    }
}

fn safe_decode_uri(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%'
            && index + 2 < bytes.len()
            && let Some(high) = from_hex(bytes[index + 1])
            && let Some(low) = from_hex(bytes[index + 2])
        {
            out.push((high << 4) | low);
            index += 3;
            continue;
        }
        out.push(bytes[index]);
        index += 1;
    }
    String::from_utf8(out).unwrap_or_else(|_| value.to_string())
}

fn from_hex(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

fn is_uri_scheme(path: &str) -> bool {
    let Some(colon) = path.find(':') else {
        return false;
    };
    let scheme = &path[..colon];
    let mut chars = scheme.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    first.is_ascii_alphabetic()
        && chars.all(|character| {
            character.is_ascii_alphanumeric()
                || matches!(character, '+' | '.' | '-')
        })
}

fn strip_markdown_extension(path: &str) -> String {
    let bytes = path.as_bytes();
    if bytes.len() >= 3 && bytes[bytes.len() - 3..].eq_ignore_ascii_case(b".md")
    {
        path[..path.len() - 3].to_string()
    } else {
        path.to_string()
    }
}

fn strikethrough_inner_spans(line: &str) -> Vec<Range<usize>> {
    let mut delimiters = Vec::new();
    let mut cursor = 0;
    while let Some(relative) = line[cursor..].find("~~") {
        let position = cursor + relative;
        delimiters.push(position);
        cursor = position + 2;
    }
    delimiters
        .chunks_exact(2)
        .map(|pair| (pair[0] + 2)..pair[1])
        .collect()
}

fn range_is_struck(start: usize, end: usize, spans: &[Range<usize>]) -> bool {
    spans
        .iter()
        .any(|span| start >= span.start && end <= span.end)
}

fn exact_struck(token: &WikiToken, spans: &[Range<usize>]) -> bool {
    spans
        .iter()
        .any(|span| span.start == token.start && span.end == token.end)
}

fn trimmed_body_range(line: &str) -> Option<(usize, usize)> {
    let indent = leading_spaces_or_tabs_len(line);
    let after_indent = &line[indent..];
    let marker_len = list_marker_len(after_indent)?;
    let after_marker = &after_indent[marker_len..];
    let whitespace = leading_spaces_or_tabs_len(after_marker);
    if whitespace == 0 {
        return None;
    }
    let body_start = indent + marker_len + whitespace;
    let trimmed = line[body_start..].trim_end_matches([' ', '\t']);
    let body_end = body_start + trimmed.len();
    (body_start < body_end).then_some((body_start, body_end))
}

fn move_only_destination(line: &str) -> Option<String> {
    let (body_start, body_end) = trimmed_body_range(line)?;
    if line.as_bytes().get(body_end - 1) != Some(&b'#') {
        return None;
    }
    let directive = body_end - 1;
    if directive <= body_start {
        return None;
    }
    let plains = wikilink_tokens(line)
        .into_iter()
        .filter(|token| !token.embedded)
        .collect::<Vec<_>>();
    if plains.len() != 1 {
        return None;
    }
    let target = &plains[0];
    if target.start != body_start || target.end != directive {
        return None;
    }
    Some(format!("{}{}", &line[..directive], &line[directive + 1..]))
}

fn bare_plain_link(line: &str) -> Option<WikiToken> {
    let (body_start, body_end) = trimmed_body_range(line)?;
    let plains = wikilink_tokens(line)
        .into_iter()
        .filter(|token| !token.embedded)
        .collect::<Vec<_>>();
    if plains.len() != 1 {
        return None;
    }
    let target = plains.into_iter().next()?;
    (target.start == body_start && target.end == body_end).then_some(target)
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
    for raw in lines.iter().take(end_line + 1).skip(sub_bullet + 1) {
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
        if let Some(parent) = stack.last_mut() {
            parent.nodes.push(WorkLogNode {
                marker,
                body_text,
                children: Vec::new(),
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

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::NaiveDate;

    fn at(hour: u32, minute: u32) -> NaiveDateTime {
        NaiveDate::from_ymd_opt(2026, 9, 28)
            .expect("valid date")
            .and_hms_opt(hour, minute, 0)
            .expect("valid time")
    }

    fn note(lines: &[&str]) -> String {
        let mut text = lines.join("\n");
        text.push('\n');
        text
    }

    fn close_at(contents: &str, hour: u32, minute: u32) -> LedgerClosePlan {
        let entry = find_running_pomodoro(contents).expect("running pomodoro");
        plan_ledger_close(contents, &entry, at(hour, minute))
    }

    fn worked_example() -> String {
        note(&[
            "## Pomodoros",
            "",
            "- [x] (**0830-0855** [t:: 25m]) — PLAN",
            "\t- 🍅 [[bob#^capture-stop]]",
            "- [ ] (**0920-0950** [t:: 30m]) — CAPTURE",
            "\t- [[bob#^capture-stop]]",
            "\t\t- Designed the `=x` grammar",
            "\t\t\t- chose `x` for done",
            "\t\t- Wrote the plan",
            "\t- [[bob#^web-capture]]#",
            "\t- ~~[[sase#^axe-restart]]~~",
            "\t\t- Restarted axe",
            "\t- quick note",
            "- [ ] () — SASE",
            "\t- [[sase#^recovery-panel]]",
        ])
    }

    fn worked_example_closed() -> String {
        note(&[
            "## Pomodoros",
            "",
            "- [x] (**0830-0855** [t:: 25m]) — PLAN",
            "\t- 🍅 [[bob#^capture-stop]]",
            "- [x] (**0920-0940** [t:: 20m]) — CAPTURE",
            "\t- 🍅 [[bob#^capture-stop]]",
            "\t\t- Designed the `=x` grammar",
            "\t\t\t- chose `x` for done",
            "\t\t- Wrote the plan",
            "\t- ~~[[sase#^axe-restart]]~~",
            "\t\t- Restarted axe",
            "\t- quick note",
            "- [ ] () — CAPTURE",
            "\t- [[bob#^capture-stop]]",
            "\t- [[bob#^web-capture]]",
            "- [ ] () — SASE",
            "\t- [[sase#^recovery-panel]]",
        ])
    }

    fn timing_for(line: &str, hour: u32, minute: u32) -> CloseTiming {
        let range = parse_adjustment_range(line).expect("range");
        close_timing(&range, at(hour, minute))
    }

    #[test]
    fn worked_example_ledger_is_byte_for_byte() {
        let plan = close_at(&worked_example(), 9, 37);
        assert_eq!(plan.contents, worked_example_closed());
        let timing = plan.timing.expect("timing");
        assert_eq!(timing.planned_start, 9 * 60 + 20);
        assert_eq!(timing.planned_end, 9 * 60 + 50);
        assert_eq!(timing.planned_duration, 30);
        assert_eq!(timing.closed_end, 9 * 60 + 40);
        assert_eq!(timing.closed_duration, 20);
        assert_eq!(timing.closed_at, 9 * 60 + 37);
        assert_eq!(timing.remaining_minutes, 13);
        assert_eq!(timing.decremented_minutes, 10);
        assert_eq!(
            plan.classified_links
                .iter()
                .map(|link| {
                    (link.line, link.role, link.block_id.as_str(), link.carried)
                })
                .collect::<Vec<_>>(),
            vec![
                (6, LedgerLinkRole::Worked, "capture-stop", true),
                (10, LedgerLinkRole::Deferred, "web-capture", true),
                (11, LedgerLinkRole::Struck, "axe-restart", false),
            ]
        );
        assert_eq!(
            plan.carried_lines,
            vec![
                "\t- [[bob#^capture-stop]]".to_string(),
                "\t- [[bob#^web-capture]]".to_string(),
            ]
        );
        assert_eq!(plan.notes, vec!["quick note".to_string()]);
        let next = plan.next_pomodoro.expect("next");
        assert_eq!(next.line, 13);
        assert_eq!(next.name.as_deref(), Some("CAPTURE"));
        assert!(next.created);
        assert_eq!(
            plan.startable_targets
                .iter()
                .map(|target| target.block_id.as_str())
                .collect::<Vec<_>>(),
            vec!["capture-stop"]
        );
        assert!(plan.embedded_targets.is_empty());
        assert_eq!(plan.sub_bullet_range, 5..13);
        assert_eq!(
            plan.work_log_groups
                .iter()
                .map(|group| group.block_id.as_str())
                .collect::<Vec<_>>(),
            vec!["capture-stop", "axe-restart"]
        );
        assert_eq!(
            plan.work_log_groups[0].descendant_roots,
            vec![
                WorkLogNode {
                    marker: "-".to_string(),
                    body_text: "Designed the `=x` grammar".to_string(),
                    children: vec![WorkLogNode {
                        marker: "-".to_string(),
                        body_text: "chose `x` for done".to_string(),
                        children: Vec::new(),
                    }],
                },
                WorkLogNode {
                    marker: "-".to_string(),
                    body_text: "Wrote the plan".to_string(),
                    children: Vec::new(),
                },
            ]
        );
        assert_eq!(
            plan.work_log_groups[1].descendant_roots,
            vec![WorkLogNode {
                marker: "-".to_string(),
                body_text: "Restarted axe".to_string(),
                children: Vec::new(),
            }]
        );
    }

    #[test]
    fn no_decrement_when_fewer_than_five_minutes_remain() {
        let plan = close_at(&worked_example(), 9, 49);
        assert!(plan.contents.contains("(**0920-0950** [t:: 30m])"));
        let timing = plan.timing.expect("timing");
        assert_eq!(timing.remaining_minutes, 1);
        assert_eq!(timing.decremented_minutes, 0);
        assert_eq!(timing.new_range_text, None);
        assert_eq!(timing.closed_duration, 30);
    }

    #[test]
    fn start_in_the_future_clamps_to_zero_minutes() {
        let timing =
            timing_for("- [ ] (**1000-1030** [t:: 30m]) — FUTURE", 9, 37);
        assert_eq!(timing.closed_duration, 0);
        assert_eq!(timing.closed_end, 10 * 60);
        assert_eq!(
            timing.new_range_text.as_deref(),
            Some("(**1000-1000** [t:: 0m])")
        );
    }

    #[test]
    fn midnight_crossing_range_uses_signed_remaining() {
        let timing =
            timing_for("- [ ] (**2330-0030** [t:: 60m]) — LATE", 23, 50);
        assert_eq!(timing.remaining_minutes, 40);
        assert_eq!(timing.closed_duration, 20);
        assert_eq!(timing.closed_end, 23 * 60 + 50);
        assert_eq!(
            timing.new_range_text.as_deref(),
            Some("(**2330-2350** [t:: 20m])")
        );
    }

    #[test]
    fn unnamed_empty_last_entry_creates_placeholder_and_stub() {
        let contents =
            note(&["## Pomodoros", "- [ ] (**0920-0950** [t:: 30m])"]);
        let plan = close_at(&contents, 9, 37);
        assert_eq!(
            plan.contents,
            note(&[
                "## Pomodoros",
                "- [x] (**0920-0940** [t:: 20m])",
                "- [ ] ()",
                "\t- ",
            ])
        );
        let next = plan.next_pomodoro.expect("next");
        assert!(next.created);
        assert_eq!(next.name, None);
        assert_eq!(next.line, 3);
    }

    #[test]
    fn nothing_carried_with_later_entry_creates_nothing() {
        let contents = note(&[
            "## Pomodoros",
            "- [ ] (**0920-0950** [t:: 30m]) — CAPTURE",
            "- [ ] () — SASE",
        ]);
        let plan = close_at(&contents, 9, 37);
        assert_eq!(
            plan.contents,
            note(&[
                "## Pomodoros",
                "- [x] (**0920-0940** [t:: 20m]) — CAPTURE",
                "- [ ] () — SASE",
            ])
        );
        assert!(plan.carried_lines.is_empty());
        let next = plan.next_pomodoro.expect("next");
        assert!(!next.created);
        assert_eq!(next.name.as_deref(), Some("SASE"));
        assert_eq!(next.line, 3);
    }

    #[test]
    fn deferred_lookalikes_are_not_removed() {
        let lookalikes = [
            "\t- ![[bob#^embedded]]#",
            "\t- ~~[[bob#^struck]]~~#",
            "\t- [[bob#^spaced]] #",
            "\t- [[bob#^double]]##",
            "\t- [[bob#^tagged]] #tag",
            "\t- prose [[bob#^mixed]]#",
        ];
        let mut lines =
            vec!["## Pomodoros", "- [ ] (**0920-0950** [t:: 30m]) — CAPTURE"];
        lines.extend(lookalikes);
        let contents = note(&lines);
        let plan = close_at(&contents, 9, 37);
        assert!(plan.contents.contains("![[bob#^embedded]]#"));
        assert!(plan.contents.contains("~~[[bob#^struck]]~~#"));
        assert!(plan.contents.contains("[[bob#^spaced]] #"));
        assert!(plan.contents.contains("[[bob#^double]]##"));
        assert!(plan.contents.contains("[[bob#^tagged]] #tag"));
        assert!(plan.contents.contains("[[bob#^mixed]]#"));
        assert!(plan
            .classified_links
            .iter()
            .all(|link| link.role != LedgerLinkRole::Deferred));
    }

    #[test]
    fn true_deferred_hash_is_removed_and_carried_without_hash() {
        let contents = note(&[
            "## Pomodoros",
            "- [ ] (**0920-0950** [t:: 30m]) — CAPTURE",
            "\t- [[bob#^web-capture]]#",
        ]);
        let plan = close_at(&contents, 9, 37);
        assert!(
            !plan
                .contents
                .contains("- [x] (**0920-0940** [t:: 20m]) — CAPTURE\n\t- [[bob#^web-capture]]#")
        );
        assert!(plan.contents.contains("\t- [[bob#^web-capture]]\n"));
        assert_eq!(
            plan.carried_lines,
            vec!["\t- [[bob#^web-capture]]".to_string()]
        );
        assert_eq!(plan.classified_links[0].role, LedgerLinkRole::Deferred);
    }

    #[test]
    fn struck_markers_collapse_and_embedded_drop() {
        let contents = note(&[
            "## Pomodoros",
            "- [ ] (**0920-0950** [t:: 30m]) — CAPTURE",
            "\t- ~~[[bob#^unmarked]]~~",
            "\t- 🍅 ~~[[bob#^marked]]~~",
            "\t- 🍅 🍅 [[bob#^collapse]]",
            "\t- 🍅 ![[bob#^embed]]",
            "- [ ] () — LATER",
        ]);
        let plan = close_at(&contents, 9, 37);
        assert!(plan.contents.contains("\t- ~~[[bob#^unmarked]]~~"));
        assert!(plan.contents.contains("\t- 🍅 ~~[[bob#^marked]]~~"));
        assert!(plan.contents.contains("\t- 🍅 [[bob#^collapse]]"));
        assert!(!plan.contents.contains("🍅 🍅 [[bob#^collapse]]"));
        assert!(plan.contents.contains("\t- ![[bob#^embed]]"));
        assert!(!plan.contents.contains("🍅 ![[bob#^embed]]"));
        assert_eq!(
            plan.classified_links
                .iter()
                .map(|link| (link.role, link.block_id.as_str(), link.carried))
                .collect::<Vec<_>>(),
            vec![
                (LedgerLinkRole::Struck, "unmarked", false),
                (LedgerLinkRole::Struck, "marked", false),
                (LedgerLinkRole::Worked, "collapse", true),
                (LedgerLinkRole::Embedded, "embed", false),
            ]
        );
    }

    #[test]
    fn nested_worked_on_links_keep_their_indent_when_carried() {
        let contents = note(&[
            "## Pomodoros",
            "- [ ] (**0920-0950** [t:: 30m]) — CAPTURE",
            "\t- [[bob#^parent]]",
            "\t\t- [[bob#^child]]",
        ]);
        let plan = close_at(&contents, 9, 37);
        assert_eq!(
            plan.carried_lines,
            vec![
                "\t- [[bob#^parent]]".to_string(),
                "\t\t- [[bob#^child]]".to_string(),
            ]
        );
        assert!(plan.contents.contains("\t\t- [[bob#^child]]"));
    }

    #[test]
    fn fenced_lines_are_untouched() {
        let contents = note(&[
            "## Pomodoros",
            "- [ ] (**0920-0950** [t:: 30m]) — CAPTURE",
            "\t- [[bob#^keep]]",
            "```md",
            "\t- [[bob#^fenced]]#",
            "```",
            "- [ ] () — SASE",
        ]);
        let plan = close_at(&contents, 9, 37);
        assert!(plan.contents.contains("```md\n\t- [[bob#^fenced]]#\n```"));
        assert!(plan
            .classified_links
            .iter()
            .all(|link| link.block_id != "fenced"));
    }

    #[test]
    fn range_cuts_at_a_blank_line() {
        let contents = note(&[
            "## Pomodoros",
            "- [ ] (**0920-0950** [t:: 30m]) — CAPTURE",
            "\t- [[bob#^keep]]",
            "",
            "\t- [[bob#^after-blank]]#",
            "- [ ] () — SASE",
        ]);
        let plan = close_at(&contents, 9, 37);
        assert_eq!(plan.sub_bullet_range, 2..3);
        assert!(plan.contents.contains("\t- [[bob#^after-blank]]#"));
        assert!(plan
            .classified_links
            .iter()
            .all(|link| link.block_id != "after-blank"));
    }

    #[test]
    fn deferred_line_leaves_orphaned_children() {
        let contents = note(&[
            "## Pomodoros",
            "- [ ] (**0920-0950** [t:: 30m]) — CAPTURE",
            "\t- [[bob#^keep]]",
            "\t- [[bob#^defer]]#",
            "\t\t- orphan note",
            "\t- [[bob#^other]]",
        ]);
        let plan = close_at(&contents, 9, 37);
        assert_eq!(
            plan.contents,
            note(&[
                "## Pomodoros",
                "- [x] (**0920-0940** [t:: 20m]) — CAPTURE",
                "\t- 🍅 [[bob#^keep]]",
                "\t\t- orphan note",
                "\t- 🍅 [[bob#^other]]",
                "- [ ] () — CAPTURE",
                "\t- [[bob#^keep]]",
                "\t- [[bob#^other]]",
                "\t- [[bob#^defer]]",
            ])
        );
    }

    #[test]
    fn preserves_crlf() {
        let contents = worked_example().replace('\n', "\r\n");
        let plan = close_at(&contents, 9, 37);
        let expected = worked_example_closed().replace('\n', "\r\n");
        assert_eq!(plan.contents, expected);
        assert!(plan.contents.contains("\r\n"));
        assert!(!plan.contents.replace("\r\n", "").contains('\n'));
    }

    #[test]
    fn preserves_missing_final_newline() {
        let mut contents = worked_example();
        assert!(contents.pop() == Some('\n'));
        let plan = close_at(&contents, 9, 37);
        let mut expected = worked_example_closed();
        assert!(expected.pop() == Some('\n'));
        assert_eq!(plan.contents, expected);
        assert!(!plan.contents.ends_with('\n'));
    }

    #[test]
    fn multiple_open_timed_entries_are_an_error() {
        let contents = note(&[
            "## Pomodoros",
            "- [ ] (**0920-0950** [t:: 30m]) — CAPTURE",
            "- [ ] (**1000-1030** [t:: 30m]) — SASE",
        ]);
        let error = find_running_pomodoro(&contents).expect_err("multiple");
        match error {
            FindRunningError::Multiple(entries) => {
                assert_eq!(
                    entries
                        .iter()
                        .map(|entry| (entry.name.as_deref(), entry.line))
                        .collect::<Vec<_>>(),
                    vec![(Some("CAPTURE"), 2), (Some("SASE"), 3)]
                );
            }
            other => panic!("expected multiple, got {other:?}"),
        }
    }

    #[test]
    fn no_open_timed_entry_reports_next_placeholder() {
        let contents = note(&[
            "## Pomodoros",
            "- [x] (**0830-0855** [t:: 25m]) — PLAN",
            "- [ ] () — CAPTURE",
        ]);
        let error = find_running_pomodoro(&contents).expect_err("none");
        match error {
            FindRunningError::NoneRunning {
                next_name,
                next_line,
            } => {
                assert_eq!(next_name.as_deref(), Some("CAPTURE"));
                assert_eq!(next_line, Some(3));
            }
            other => panic!("expected none running, got {other:?}"),
        }
    }

    #[test]
    fn missing_section_is_an_error() {
        let contents = note(&["# Daily", "- [ ] (**0920-0950** [t:: 30m])"]);
        assert_eq!(
            find_running_pomodoro(&contents),
            Err(FindRunningError::NoSection)
        );
    }
}
