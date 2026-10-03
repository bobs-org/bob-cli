//! Canonical placement for `[fresh:: YYYY-MM-DD]`, `[refresh:: N]`,
//! and `[keeps:: N]`.
//!
//! This is the Rust half of the one-helper-per-language rule owned by
//! `docs/freshness.md`. The JavaScript mirror is
//! `api.freshness.stampLine` / `setRefreshLine` / `keepLine` in
//! bob-ledger-tools. Both sides run the placement conformance vectors
//! in that doc verbatim. Rust reads, clears, and reports `keeps`; it
//! has no increment path — the sole increment helper is JavaScript
//! `api.freshness.keepLine`.
//!
//! The vault uses Tasks' Dataview format, and both Rust parsers read
//! fields from the end of the line and stop at the first key they do
//! not recognize. Appending `fresh` at the end would therefore hide
//! `created`, `priority`, `scheduled`, `id` and `dependsOn`. The helper
//! inserts `fresh` (and `refresh`) immediately before the trailing
//! Tasks suffix instead, and never changes the suffix bytes themselves
//! except to remove a misplaced `fresh` / `refresh`.

use chrono::NaiveDate;

use super::super::task_fields::{
    format_calendar_date, inline_fields, parse_strict_calendar_date,
};

/// Tasks keys recognized at the end of a Dataview task line, in the
/// order `docs/freshness.md` pins them.
const TASKS_KEYS: &[&str] = &[
    "priority",
    "start",
    "created",
    "scheduled",
    "due",
    "completion",
    "cancelled",
    "repeat",
    "onCompletion",
    "id",
    "dependsOn",
];

/// Why a line was left unchanged.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Refusal {
    /// Not a task line (`- [x] …` shape).
    NotTask,
    /// A `repeat` field: recurrence resurfaces the task, and Tasks
    /// would copy the stamp into the next occurrence.
    Recurring,
    /// A done or cancelled task, which callers must not stamp.
    Closed,
}

impl Refusal {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::NotTask => "not_task",
            Self::Recurring => "recurring",
            Self::Closed => "closed",
        }
    }
}

/// The result of stamping a line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Stamp {
    pub(crate) line: String,
    pub(crate) changed: bool,
    pub(crate) refused: Option<Refusal>,
}

/// What `read_freshness` found on one line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FreshRead {
    /// The latest valid `fresh` date not after `today`, if any.
    /// A future date is treated as none (plus a lint).
    pub(crate) fresh: Option<NaiveDate>,
    /// The first valid `[refresh:: N]` (1–365), if any.
    pub(crate) refresh: Option<u16>,
    /// The first valid `[keeps:: N]` (1–999) as a semantic count.
    /// Absence — or no valid value — means 0, and writers omit zero.
    pub(crate) keeps: u32,
    /// Lint codes in first-seen order.
    pub(crate) lints: Vec<String>,
}

/// Stamp `line` with `date`, keeping the first valid existing
/// `[refresh:: N]` if there is one.
///
/// Every generic human stamp clears `keeps`: the line is rewritten
/// without any `keeps` field, even when `date` already equals today.
/// Refusals (not a task, recurring, done/cancelled) return the line
/// unchanged with a reason. A line already stamped with `date` in
/// canonical position — and with no `keeps` to clear — is
/// byte-identical with `changed: false`.
pub(crate) fn stamp_fresh(line: &str, date: NaiveDate) -> Stamp {
    stamp_inner(line, date, RefreshEdit::Keep, KeepsEdit::Clear)
}

/// Stamp `line` with `date` while preserving the valid `keeps`
/// semantic value.
///
/// This is the private preserve-mode primitive for the cutover seed
/// only (`seed.rs::stamp_change`): the seed must never turn into a
/// resetter when the default stamp clears keeps. The preserved count
/// is canonicalized (first valid value, re-emitted in canonical
/// position and order); absent or invalid stays omitted.
pub(crate) fn stamp_fresh_preserve_keeps(line: &str, date: NaiveDate) -> Stamp {
    stamp_inner(line, date, RefreshEdit::Keep, KeepsEdit::Preserve)
}

/// Set or clear `[refresh:: N]` and stamp with `date`.
///
/// `Some(days)` must hold 1–365; out-of-range values clear the field
/// instead of writing an invalid one. `None` removes the field.
/// Like every generic stamp, this clears `keeps`.
// P12 vector helper: only the conformance tests exercise refresh edits.
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn set_refresh(
    line: &str,
    days: Option<u16>,
    date: NaiveDate,
) -> Stamp {
    let edit = match days {
        Some(days) if (1..=365).contains(&days) => RefreshEdit::Set(days),
        Some(_) => RefreshEdit::Clear,
        None => RefreshEdit::Clear,
    };
    stamp_inner(line, date, edit, KeepsEdit::Clear)
}

#[derive(Debug, Clone, Copy)]
enum RefreshEdit {
    Keep,
    // P12 vector variants: only set_refresh (test-only) constructs these.
    #[cfg_attr(not(test), allow(dead_code))]
    Set(u16),
    #[cfg_attr(not(test), allow(dead_code))]
    Clear,
}

/// What a stamp does with `keeps`: generic stamps clear it;
/// only the seed preserve-mode keeps the valid semantic value.
#[derive(Debug, Clone, Copy)]
enum KeepsEdit {
    Clear,
    Preserve,
}

fn stamp_inner(
    line: &str,
    date: NaiveDate,
    refresh_edit: RefreshEdit,
    keeps_edit: KeepsEdit,
) -> Stamp {
    let Some(status) = task_status(line) else {
        return Stamp {
            line: line.to_string(),
            changed: false,
            refused: Some(Refusal::NotTask),
        };
    };
    if is_closed_status(status) {
        return Stamp {
            line: line.to_string(),
            changed: false,
            refused: Some(Refusal::Closed),
        };
    }
    if has_repeat_field(line) {
        return Stamp {
            line: line.to_string(),
            changed: false,
            refused: Some(Refusal::Recurring),
        };
    }

    let kept_refresh = match refresh_edit {
        RefreshEdit::Keep => first_valid_refresh(line),
        RefreshEdit::Set(days) => Some(days),
        RefreshEdit::Clear => None,
    };
    let kept_keeps = match keeps_edit {
        KeepsEdit::Clear => None,
        KeepsEdit::Preserve => first_valid_keeps(line),
    };

    let date_text = format_calendar_date(date);
    let output =
        rebuild_without_fields(line, &date_text, kept_refresh, kept_keeps);
    let changed = output != line;
    Stamp {
        line: output,
        changed,
        refused: None,
    }
}

/// Read the `fresh` / `refresh` / `keeps` fields on `line`.
///
/// - Malformed `fresh` values report `fresh_malformed`.
/// - `fresh` dates after `today` report `fresh_future` and are treated
///   as none.
/// - More than one `fresh` field reports `fresh_duplicate`; the latest
///   valid date wins.
/// - A `fresh` / `refresh` / `keeps` field inside the Tasks suffix
///   reports `fresh_misplaced`; the next stamp repairs it.
/// - An invalid `[refresh:: N]` reports `refresh_invalid` and falls
///   through.
/// - `keeps` is a decimal integer 1–999; absence — or no valid value
///   — means 0 and writers omit zero. The first valid value wins; an
///   invalid value reports `keeps_invalid`, and more than one `keeps`
///   field reports `keeps_duplicate`.
pub(crate) fn read_freshness(line: &str, today: NaiveDate) -> FreshRead {
    let fresh_fields = inline_fields(line, "fresh");
    let refresh_fields = inline_fields(line, "refresh");
    let keeps_fields = inline_fields(line, "keeps");
    let mut lints = Vec::new();

    let mut best: Option<NaiveDate> = None;
    let mut seen_valid = 0;
    for field in &fresh_fields {
        match parse_strict_calendar_date(&field.value) {
            Some(date) if date > today => {
                push_lint(&mut lints, "fresh_future");
            }
            Some(date) => {
                seen_valid += 1;
                if best.is_none_or(|current| date > current) {
                    best = Some(date);
                }
            }
            None => {
                push_lint(&mut lints, "fresh_malformed");
            }
        }
    }
    let _ = seen_valid;
    if fresh_fields.len() > 1 {
        push_lint(&mut lints, "fresh_duplicate");
    }

    let mut refresh = None;
    let mut refresh_invalid_seen = false;
    for field in &refresh_fields {
        match parse_refresh_value(&field.value) {
            Some(days) => {
                if refresh.is_none() {
                    refresh = Some(days);
                }
            }
            None => {
                refresh_invalid_seen = true;
            }
        }
    }
    if refresh_invalid_seen {
        push_lint(&mut lints, "refresh_invalid");
    }

    let mut keeps = None;
    let mut keeps_invalid_seen = false;
    for field in &keeps_fields {
        match parse_keeps_value(&field.value) {
            Some(count) => {
                if keeps.is_none() {
                    keeps = Some(count);
                }
            }
            None => {
                keeps_invalid_seen = true;
            }
        }
    }
    if keeps_invalid_seen {
        push_lint(&mut lints, "keeps_invalid");
    }
    if keeps_fields.len() > 1 {
        push_lint(&mut lints, "keeps_duplicate");
    }

    if has_misplaced_field(line) {
        push_lint(&mut lints, "fresh_misplaced");
    }

    FreshRead {
        fresh: best,
        refresh,
        keeps: keeps.unwrap_or(0),
        lints,
    }
}

/// Byte offset where the trailing Tasks suffix starts.
///
/// The suffix starts at the leftmost Tasks element (a Tasks-key field,
/// a trailing tag, or `^id`) of the run scanned from the end of the
/// line. `fresh` / `refresh` / `keeps` fields extend the run but are
/// not part of the suffix. `keeps` is a run-extending non-Tasks key:
/// it is never added to the Tasks key registry. Returns `trimmed_len`
/// when there is no suffix.
pub(crate) fn tasks_suffix_start(line: &str) -> usize {
    suffix_start_inner(line).unwrap_or_else(|| line.trim_end().len())
}

fn suffix_start_inner(line: &str) -> Option<usize> {
    let floor = scan_floor(line)?;
    let trimmed_len = line.trim_end().len();
    let mut cursor = trimmed_len;
    let mut leftmost: Option<usize> = None;

    // An optional trailing ` ^id` block link opens the run.
    if let Some(block_start) = trailing_block_start(&line[..cursor])
        && block_start >= floor
    {
        leftmost = Some(block_start);
        cursor = block_start;
        cursor = trim_end_to(&line[..cursor], cursor);
    }

    loop {
        if cursor <= floor {
            break;
        }
        let slice = &line[..cursor];
        let trimmed = slice.trim_end();
        let end = trimmed.len();
        if end <= floor {
            break;
        }
        // A trailing tag continues the run and is a suffix element.
        if let Some(tag_start) = trailing_tag_start(trimmed, floor) {
            leftmost = Some(tag_start);
            cursor = tag_start;
            cursor = trim_end_to(&line[..cursor], cursor);
            continue;
        }
        // A trailing inline field continues the run. Tasks keys are
        // suffix elements; fresh/refresh/keeps only extend the run.
        let Some((field_start, key)) = trailing_field_key(trimmed) else {
            break;
        };
        if field_start < floor {
            break;
        }
        if TASKS_KEYS.contains(&key.as_str()) {
            leftmost = Some(field_start);
            cursor = field_start;
            cursor = trim_end_to(&line[..cursor], cursor);
            continue;
        }
        if key == "fresh" || key == "refresh" || key == "keeps" {
            cursor = field_start;
            cursor = trim_end_to(&line[..cursor], cursor);
            continue;
        }
        break;
    }

    leftmost
}

/// Rebuild `line` with `fresh:: date_text` (plus `refresh` and
/// `keeps` when kept) immediately before the Tasks suffix.
///
/// Output order is `fresh`, optional `refresh`, optional `keeps`,
/// then the existing Tasks suffix, tags, and block ID.
fn rebuild_without_fields(
    line: &str,
    date_text: &str,
    kept_refresh: Option<u16>,
    kept_keeps: Option<u32>,
) -> String {
    let suffix_start = tasks_suffix_start(line);
    let trimmed_len = line.trim_end().len();
    let suffix_start = suffix_start.min(trimmed_len);
    let head_raw = &line[..suffix_start];
    let suffix_raw = &line[suffix_start..trimmed_len];

    let head_clean = remove_fields(head_raw);
    let suffix_clean = remove_fields(suffix_raw);

    let head = head_clean.trim_end();
    let suffix = suffix_clean.trim().trim_start_matches([' ', '\t']);

    let mut output = String::with_capacity(line.len() + 24);
    output.push_str(head);
    output.push(' ');
    output.push_str("[fresh:: ");
    output.push_str(date_text);
    output.push(']');
    if let Some(days) = kept_refresh {
        output.push_str(" [refresh:: ");
        output.push_str(&days.to_string());
        output.push(']');
    }
    if let Some(count) = kept_keeps {
        output.push_str(" [keeps:: ");
        output.push_str(&count.to_string());
        output.push(']');
    }
    if !suffix.is_empty() {
        output.push(' ');
        output.push_str(suffix);
    }
    output
}

/// Remove every `fresh`, `refresh`, and `keeps` field from `text`,
/// collapsing the whitespace each removal leaves to a single space.
fn remove_fields(text: &str) -> String {
    let mut ranges = Vec::new();
    for field in inline_fields(text, "fresh") {
        ranges.push((field.start, field.end));
    }
    for field in inline_fields(text, "refresh") {
        ranges.push((field.start, field.end));
    }
    for field in inline_fields(text, "keeps") {
        ranges.push((field.start, field.end));
    }
    if ranges.is_empty() {
        return text.to_string();
    }
    ranges.sort();

    let mut output = String::new();
    let mut cursor = 0;
    for (start, end) in ranges {
        let before = &text[cursor..start];
        // Collapse whitespace around the removal to one gap.
        let before_trimmed = before.trim_end_matches([' ', '\t']);
        output.push_str(before_trimmed);
        // Skip whitespace after the field.
        let mut next = end;
        while text[next..]
            .chars()
            .next()
            .is_some_and(|c| c == ' ' || c == '\t')
        {
            next += 1;
        }
        if !output.is_empty() && next < text.len() {
            output.push(' ');
        }
        cursor = next;
    }
    output.push_str(&text[cursor..]);
    output
}

/// Drop leading `>` quote markers the way the vault scanner does
/// (`strip_blockquote_prefixes` in `dataview::tasks`): up to three
/// spaces, `>`, one optional space, repeated. Both Rust parsers read
/// quoted tasks, so the stamper must recognize the same lines the
/// seed scans — otherwise the cutover refuses on the first quoted
/// task in the vault.
fn strip_blockquote_prefix(mut line: &str) -> &str {
    loop {
        let spaces = line.bytes().take_while(|byte| *byte == b' ').count();
        if spaces > 3 || line.as_bytes().get(spaces) != Some(&b'>') {
            return line;
        }
        line = &line[spaces + 1..];
        if let Some(rest) = line.strip_prefix(' ') {
            line = rest;
        }
    }
}

/// The checkbox status on a task line, if `line` is a task line.
fn task_status(line: &str) -> Option<char> {
    let line = strip_blockquote_prefix(line);
    let bytes = line.as_bytes();
    let mut index = bytes
        .iter()
        .take_while(|byte| matches!(byte, b' ' | b'\t'))
        .count();
    index = after_list_marker(line, index)?;
    while bytes.get(index).is_some_and(u8::is_ascii_whitespace) {
        index += 1;
    }
    if bytes.get(index) != Some(&b'[') {
        return None;
    }
    let close = line[index + 1..].find(']')?;
    let status_text = &line[index + 1..index + 1 + close];
    let mut chars = status_text.chars();
    let status = chars.next()?;
    if chars.next().is_some() {
        return None;
    }
    let after = &line[index + 1 + close + 1..];
    if !after.is_empty() && !after.starts_with(char::is_whitespace) {
        return None;
    }
    Some(status)
}

fn after_list_marker(line: &str, index: usize) -> Option<usize> {
    let bytes = line.as_bytes();
    if matches!(bytes.get(index), Some(b'-' | b'*' | b'+')) {
        return bytes
            .get(index + 1)
            .is_some_and(u8::is_ascii_whitespace)
            .then_some(index + 1);
    }
    let digits = bytes[index..]
        .iter()
        .take_while(|byte| byte.is_ascii_digit())
        .count();
    if digits == 0 || !matches!(bytes.get(index + digits), Some(b'.' | b')')) {
        return None;
    }
    bytes
        .get(index + digits + 1)
        .is_some_and(u8::is_ascii_whitespace)
        .then_some(index + digits + 1)
}

/// Done (`x`/`X`) or cancelled (`-`) checkbox symbols.
fn is_closed_status(status: char) -> bool {
    matches!(status, 'x' | 'X' | '-')
}

fn has_repeat_field(line: &str) -> bool {
    !inline_fields(line, "repeat").is_empty()
}

fn first_valid_refresh(line: &str) -> Option<u16> {
    inline_fields(line, "refresh")
        .iter()
        .find_map(|field| parse_refresh_value(&field.value))
}

fn first_valid_keeps(line: &str) -> Option<u32> {
    inline_fields(line, "keeps")
        .iter()
        .find_map(|field| parse_keeps_value(&field.value))
}

/// A `[keeps:: N]` value: a decimal integer 1–999, nothing else.
/// Absence means 0 and writers omit zero; increment saturates at 999
/// on the JavaScript side (Rust has no increment path).
fn parse_keeps_value(value: &str) -> Option<u32> {
    let trimmed = value.trim();
    if trimmed.is_empty() || !trimmed.bytes().all(|byte| byte.is_ascii_digit())
    {
        return None;
    }
    let number: i64 = trimmed.parse().ok()?;
    if (1..=999).contains(&number) {
        Some(number as u32)
    } else {
        None
    }
}

/// A `[refresh:: N]` value: an integer 1–365, nothing else.
fn parse_refresh_value(value: &str) -> Option<u16> {
    let trimmed = value.trim();
    if trimmed.is_empty() || trimmed.len() > 3 {
        // 1–365 is at most 3 digits; longer can only be invalid.
        // Fall through to the strict parse which rejects it.
    }
    let number: i64 = trimmed.parse().ok()?;
    if (1..=365).contains(&number) {
        Some(number as u16)
    } else {
        None
    }
}

/// Whether any `fresh` / `refresh` / `keeps` field sits at or after
/// the suffix start (inside the Tasks suffix run).
fn has_misplaced_field(line: &str) -> bool {
    let start = tasks_suffix_start(line);
    let trimmed_len = line.trim_end().len();
    if start >= trimmed_len {
        return false;
    }
    inline_fields(line, "fresh")
        .iter()
        .chain(inline_fields(line, "refresh").iter())
        .chain(inline_fields(line, "keeps").iter())
        .any(|field| field.start >= start)
}

/// The scan floor: the start of the task body, or the end of a
/// leading `#task` global-filter token. The suffix scan never moves
/// left of it.
fn scan_floor(line: &str) -> Option<usize> {
    // Quote markers are detection-only: strip them, then shift the
    // floor back into the original line's coordinates.
    let stripped = strip_blockquote_prefix(line);
    let offset = line.len() - stripped.len();
    let line = stripped;
    let bytes = line.as_bytes();
    let mut index = bytes
        .iter()
        .take_while(|byte| matches!(byte, b' ' | b'\t'))
        .count();
    index = after_list_marker(line, index)?;
    while bytes.get(index).is_some_and(u8::is_ascii_whitespace) {
        index += 1;
    }
    // Skip `[x]`.
    let close = line[index..].find(']')?;
    let mut body_start = index + close + 1;
    while line[body_start..].starts_with([' ', '\t']) {
        body_start += 1;
    }
    let body = &line[body_start..];
    if body == "#task"
        || body.starts_with("#task ")
        || body.starts_with("#task\t")
    {
        Some(offset + body_start + "#task".len())
    } else {
        Some(offset + body_start)
    }
}

fn trim_end_to(text: &str, mut cursor: usize) -> usize {
    cursor = cursor.min(text.len());
    while cursor > 0 && text[..cursor].ends_with([' ', '\t']) {
        cursor -= 1;
    }
    cursor
}

/// Start of a trailing ` ^id` block link, if `text` ends with one.
/// The id is ASCII alphanumeric plus `-`, matching `BLOCK_LINK`.
fn trailing_block_start(text: &str) -> Option<usize> {
    let trimmed = text.trim_end();
    let token_start = trimmed
        .rfind([' ', '\t'])
        .map(|index| index + 1)
        .unwrap_or(0);
    let token = &trimmed[token_start..];
    let id = token.strip_prefix('^')?;
    if id.is_empty()
        || !id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
    {
        return None;
    }
    // Require whitespace before the `^` (or start of the scanned
    // slice, which only happens for degenerate lines).
    if token_start > 0 {
        let before = trimmed[..token_start].chars().next_back()?;
        if before != ' ' && before != '\t' {
            return None;
        }
    }
    // Map back to the untrimmed text: the token ends at the trim end.
    let end = text.trim_end().len();
    Some(end - token.len())
}

/// Start of a trailing `#tag`, if `trimmed` (already right-trimmed)
/// ends with one starting at or after `floor`.
fn trailing_tag_start(trimmed: &str, floor: usize) -> Option<usize> {
    let token_start = trimmed
        .rfind([' ', '\t'])
        .map(|index| index + 1)
        .unwrap_or(0);
    let start = token_start.max(floor);
    let token = trimmed.get(start..)?;
    // The last token must start with `#` exactly at a token boundary:
    // either at the floor or right after whitespace.
    if !token.starts_with('#') {
        return None;
    }
    if start > floor {
        let before = trimmed[..start].chars().next_back()?;
        if before != ' ' && before != '\t' {
            return None;
        }
    }
    let value = token.strip_prefix('#')?;
    if value.is_empty()
        || value
            .chars()
            .any(|c| c.is_whitespace() || "!@#$%^&*(),.?\":{}|<>".contains(c))
    {
        return None;
    }
    Some(start)
}

/// The `(start, key)` of a trailing `[k:: v]` / `(k:: v)` field, in
/// `trailing_inline_field` grammar: the key must equal its trim.
fn trailing_field_key(trimmed: &str) -> Option<(usize, String)> {
    let mut end = trimmed.len();
    if trimmed[..end].ends_with(',') {
        end -= 1;
        end = trimmed[..end].trim_end().len();
    }
    let close = trimmed[..end].chars().next_back()?;
    let open = match close {
        ']' => '[',
        ')' => '(',
        _ => return None,
    };
    let without_close = &trimmed[..end - close.len_utf8()];
    let start = without_close.rfind(open)?;
    let inner = without_close[start + open.len_utf8()..].trim();
    let (key, _) = inner.split_once("::")?;
    (key == key.trim()).then(|| (start, key.trim().to_string()))
}

fn push_lint(lints: &mut Vec<String>, code: &str) {
    if !lints.iter().any(|lint| lint == code) {
        lints.push(code.to_string());
    }
}

#[cfg(test)]
#[path = "placement_tests.rs"]
mod placement_tests;
