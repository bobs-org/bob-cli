//! Bank row and day-heading grammar, plus canonical rendering.

use chrono::{Datelike, NaiveDate};

use crate::native::collect_done::trailing_block_id_in_line;

const WEEKDAYS: [&str; 7] = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];
const MONTHS_ABBR: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct",
    "Nov", "Dec",
];
const MONTHS_FULL: [&str; 12] = [
    "January",
    "February",
    "March",
    "April",
    "May",
    "June",
    "July",
    "August",
    "September",
    "October",
    "November",
    "December",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct YearMonth {
    pub(crate) year: i32,
    pub(crate) month: u32,
}

impl YearMonth {
    pub(crate) fn parse(value: &str) -> Option<Self> {
        let (year, month) = value.split_once('-')?;
        if year.len() != 4 || month.len() != 2 {
            return None;
        }
        let year = year.parse().ok()?;
        let month = month.parse().ok()?;
        if !(1..=12).contains(&month) {
            return None;
        }
        Some(Self { year, month })
    }

    pub(crate) fn contains(self, date: NaiveDate) -> bool {
        date.year() == self.year && date.month() == self.month
    }

    pub(crate) fn label(self) -> String {
        let name = MONTHS_FULL[(self.month as usize).saturating_sub(1)];
        format!("{name} {}", self.year)
    }

    pub(crate) fn as_str(self) -> String {
        format!("{:04}-{:02}", self.year, self.month)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DayHeadingKind {
    Canonical,
    MonthDay,
    Iso,
    IsoWeekday,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DayHeading {
    pub(crate) date: NaiveDate,
    pub(crate) kind: DayHeadingKind,
    pub(crate) weekday_mismatch: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LineKind<'a> {
    NotARow,
    Stub,
    AmountNoCheckbox,
    Checkbox { status: char, body: &'a str },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum AmountLead<'a> {
    NotAmount,
    Stub,
    Unparsed,
    Amount { cents: i64, rest: &'a str },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ParsedRow {
    pub(crate) status: char,
    pub(crate) acknowledged: bool,
    pub(crate) cents: i64,
    pub(crate) payee: String,
    pub(crate) category: Option<String>,
    pub(crate) account: Option<String>,
    pub(crate) links: Vec<String>,
    pub(crate) extra_fields: Vec<(String, String)>,
    pub(crate) flagged: bool,
    pub(crate) note: Option<String>,
    pub(crate) id: Option<String>,
    pub(crate) has_task_tag: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum RowParse {
    Stub,
    Unparsed,
    Parsed(ParsedRow),
}

pub(crate) fn weekday_abbr(date: NaiveDate) -> &'static str {
    WEEKDAYS[date.weekday().num_days_from_monday() as usize]
}

pub(crate) fn month_abbr(month: u32) -> &'static str {
    MONTHS_ABBR[(month as usize).saturating_sub(1).min(11)]
}

pub(crate) fn render_day_heading(date: NaiveDate) -> String {
    format!(
        "## {} {} {}",
        weekday_abbr(date),
        month_abbr(date.month()),
        date.day()
    )
}

pub(crate) fn render_month_title(month: YearMonth) -> String {
    format!("# 🏦 {}", month.label())
}

/// Canonical amount: thousands commas, two decimals, `+` on money in.
pub(crate) fn render_amount(cents: i64) -> String {
    let sign = if cents > 0 { "+" } else { "" };
    let abs = cents.unsigned_abs();
    let dollars = abs / 100;
    let remainder = abs % 100;
    format!("{sign}${}.{:02}", group_thousands(dollars), remainder)
}

fn group_thousands(value: u64) -> String {
    let raw = value.to_string();
    let mut out = String::new();
    for (index, character) in raw.chars().enumerate() {
        if index > 0 && (raw.len() - index).is_multiple_of(3) {
            out.push(',');
        }
        out.push(character);
    }
    out
}

#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn render_row(
    waiting: bool,
    cents: i64,
    payee: &str,
    category: Option<&str>,
    account: Option<&str>,
    links: &[String],
    id: Option<&str>,
) -> String {
    let box_mark = if waiting { " " } else { "x" };
    let mut line = format!("- [{box_mark}] {} {payee}", render_amount(cents));
    if let Some(category) = category {
        line.push_str(&format!(" [cat:: {category}]"));
    }
    if let Some(account) = account {
        line.push_str(&format!(" [acct:: {account}]"));
    }
    for link in links {
        line.push(' ');
        line.push_str(link);
    }
    if let Some(id) = id {
        line.push_str(&format!(" ^bank-{id}"));
    }
    line
}

pub(crate) fn classify_line(line: &str) -> LineKind<'_> {
    if line.starts_with([' ', '\t']) {
        return LineKind::NotARow;
    }
    let Some(after_dash) = line.strip_prefix("- ") else {
        return LineKind::NotARow;
    };
    if let Some(after_open) = after_dash.strip_prefix('[') {
        let mut chars = after_open.chars();
        let Some(status) = chars.next() else {
            return LineKind::NotARow;
        };
        let rest = chars.as_str();
        let Some(body) = rest.strip_prefix("] ") else {
            if let Some(body) = rest.strip_prefix(']') {
                return classify_checkbox(status, body.trim_start());
            }
            return LineKind::NotARow;
        };
        return classify_checkbox(status, body);
    }
    match parse_amount_lead(after_dash) {
        AmountLead::NotAmount => LineKind::NotARow,
        AmountLead::Stub | AmountLead::Unparsed | AmountLead::Amount { .. } => {
            LineKind::AmountNoCheckbox
        }
    }
}

fn classify_checkbox<'a>(status: char, body: &'a str) -> LineKind<'a> {
    match parse_amount_lead(body) {
        AmountLead::NotAmount => LineKind::NotARow,
        AmountLead::Stub => LineKind::Stub,
        AmountLead::Unparsed | AmountLead::Amount { .. } => {
            LineKind::Checkbox { status, body }
        }
    }
}

pub(crate) fn parse_row(status: char, body: &str) -> RowParse {
    match parse_amount_lead(body) {
        AmountLead::NotAmount => RowParse::Unparsed,
        AmountLead::Stub => RowParse::Stub,
        AmountLead::Unparsed => RowParse::Unparsed,
        AmountLead::Amount { cents, rest } => {
            RowParse::Parsed(parse_tail(status, cents, rest))
        }
    }
}

pub(crate) fn parse_amount_lead(body: &str) -> AmountLead<'_> {
    let mut rest = body.trim_start();
    let bold = rest.starts_with("**");
    if bold {
        rest = &rest[2..];
    }
    let (sign, after_sign) = match rest.chars().next() {
        Some('+') => (1i64, &rest[1..]),
        Some('-') => (-1i64, &rest[1..]),
        Some('−') => (-1i64, &rest['−'.len_utf8()..]),
        _ => (-1i64, rest),
    };
    if !after_sign.starts_with('$') {
        return AmountLead::NotAmount;
    }
    rest = &after_sign[1..];
    if rest.starts_with("**") {
        rest = &rest[2..];
    }
    if rest.trim_start().is_empty() {
        return AmountLead::Stub;
    }
    let Some((cents_abs, after_number)) = parse_cents_unsigned(rest) else {
        return AmountLead::Unparsed;
    };
    rest = after_number;
    if bold {
        rest = rest.strip_prefix("**").unwrap_or(rest);
    }
    let cents = sign * cents_abs as i64;
    AmountLead::Amount { cents, rest }
}

fn parse_cents_unsigned(input: &str) -> Option<(u64, &str)> {
    let bytes = input.as_bytes();
    if bytes.is_empty() || !bytes[0].is_ascii_digit() {
        return None;
    }
    let mut index = 0;
    while index < bytes.len()
        && (bytes[index].is_ascii_digit() || bytes[index] == b',')
    {
        index += 1;
    }
    let integer = &input[..index];
    if integer.contains(',') {
        let parts: Vec<&str> = integer.split(',').collect();
        if parts
            .first()
            .is_none_or(|part| !(1..=3).contains(&part.len()))
            || parts.iter().skip(1).any(|part| part.len() != 3)
            || parts.iter().any(|part| part.is_empty())
        {
            return None;
        }
    }
    let dollars: u64 = integer.replace(',', "").parse().ok()?;
    let mut rest = &input[index..];
    let mut cents = dollars.checked_mul(100)?;
    if let Some(after_dot) = rest.strip_prefix('.') {
        let digit_count =
            after_dot.bytes().take_while(u8::is_ascii_digit).count();
        if digit_count == 0 || digit_count > 2 {
            return None;
        }
        let frac: u64 = after_dot[..digit_count].parse().ok()?;
        let scale = if digit_count == 1 { 10 } else { 1 };
        cents = cents.checked_add(frac * scale)?;
        rest = &after_dot[digit_count..];
    }
    Some((cents, rest))
}

fn parse_tail(status: char, cents: i64, rest: &str) -> ParsedRow {
    let (without_id, bank_id) = split_trailing_bank_id(rest);
    let payee_end = payee_end(without_id);
    let payee = without_id[..payee_end].trim().to_string();
    let tail = parse_row_tail(without_id[payee_end..].trim_start());
    let has_task_tag = has_task_token(without_id);
    ParsedRow {
        status,
        acknowledged: matches!(status, 'x' | 'X'),
        cents,
        payee,
        category: tail.category,
        account: tail.account,
        links: tail.links,
        extra_fields: tail.extra_fields,
        flagged: tail.flagged,
        note: tail.note,
        id: bank_id,
        has_task_tag,
    }
}

fn has_task_token(text: &str) -> bool {
    text.split_whitespace().any(|token| token == "#task")
}

fn split_trailing_bank_id(text: &str) -> (&str, Option<String>) {
    let Some(id) = trailing_block_id_in_line(text) else {
        return (text, None);
    };
    let trimmed = text.trim_end();
    let Some(caret) = trimmed.rfind('^') else {
        return (text, None);
    };
    let without = trimmed[..caret].trim_end();
    if let Some(hex) = id.strip_prefix("bank-")
        && !hex.is_empty()
        && hex.bytes().all(|byte| byte.is_ascii_hexdigit())
    {
        (without, Some(hex.to_ascii_lowercase()))
    } else {
        (without, None)
    }
}

fn payee_end(text: &str) -> usize {
    let mut end = text.len();
    let markers = [" [", "[[", "—", "--", "⚑", "!!"];
    for marker in markers {
        if let Some(index) = text.find(marker) {
            end = end.min(index);
        }
    }
    end
}

struct RowTail {
    category: Option<String>,
    account: Option<String>,
    links: Vec<String>,
    extra_fields: Vec<(String, String)>,
    flagged: bool,
    note: Option<String>,
}

fn parse_row_tail(tail: &str) -> RowTail {
    let mut category = None;
    let mut account = None;
    let mut links = Vec::new();
    let mut extra_fields = Vec::new();
    let mut flagged = false;
    let mut note_start = None;
    let mut index = 0;
    let bytes = tail.as_bytes();
    while index < tail.len() {
        if tail[index..].starts_with("[[")
            && let Some(close) = tail[index + 2..].find("]]")
        {
            let end = index + 2 + close + 2;
            links.push(tail[index..end].to_string());
            index = end;
            continue;
        }
        if tail[index..].starts_with('[')
            && let Some(field) = parse_inline_field(&tail[index..])
        {
            match field.key.as_str() {
                "cat" => category = Some(field.value),
                "acct" => account = Some(field.value),
                _ => extra_fields.push((field.key, field.value)),
            }
            index += field.consumed;
            continue;
        }
        if tail[index..].starts_with('⚑') {
            flagged = true;
            if note_start.is_none() {
                note_start = Some(index + '⚑'.len_utf8());
            }
            index += '⚑'.len_utf8();
            continue;
        }
        if tail[index..].starts_with("!!") {
            flagged = true;
            if note_start.is_none() {
                note_start = Some(index + 2);
            }
            index += 2;
            continue;
        }
        if tail[index..].starts_with('—') {
            if note_start.is_none() {
                note_start = Some(index + '—'.len_utf8());
            }
            index += '—'.len_utf8();
            continue;
        }
        if tail[index..].starts_with("--")
            && (index + 2 == tail.len()
                || bytes.get(index + 2).is_none_or(|byte| *byte != b'-'))
        {
            if note_start.is_none() {
                note_start = Some(index + 2);
            }
            index += 2;
            continue;
        }
        index += tail[index..].chars().next().map_or(1, char::len_utf8);
    }
    let note = note_start.and_then(|start| {
        let raw = strip_markers_fields_links(tail[start..].trim());
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_string())
        }
    });
    RowTail {
        category,
        account,
        links,
        extra_fields,
        flagged,
        note,
    }
}

struct InlineField {
    key: String,
    value: String,
    consumed: usize,
}

fn parse_inline_field(text: &str) -> Option<InlineField> {
    let rest = text.strip_prefix('[')?;
    let colon = rest.find("::")?;
    let key = rest[..colon].trim();
    if key.is_empty()
        || !key.chars().next().is_some_and(|c| c.is_ascii_alphabetic())
        || !key
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-'))
    {
        return None;
    }
    let after_key = &rest[colon + 2..];
    let close = after_key.find(']')?;
    let value = after_key[..close].trim().to_string();
    Some(InlineField {
        key: key.to_string(),
        value,
        consumed: 1 + colon + 2 + close + 1,
    })
}

fn strip_markers_fields_links(text: &str) -> String {
    let mut out = String::new();
    let mut index = 0;
    while index < text.len() {
        if text[index..].starts_with("[[")
            && let Some(close) = text[index + 2..].find("]]")
        {
            index += 2 + close + 2;
            continue;
        }
        if text[index..].starts_with('[')
            && let Some(field) = parse_inline_field(&text[index..])
        {
            index += field.consumed;
            continue;
        }
        if text[index..].starts_with('⚑') {
            index += '⚑'.len_utf8();
            continue;
        }
        if text[index..].starts_with("!!") {
            index += 2;
            continue;
        }
        if text[index..].starts_with('—') {
            index += '—'.len_utf8();
            continue;
        }
        if text[index..].starts_with("--") {
            index += 2;
            continue;
        }
        let ch = text[index..].chars().next().unwrap();
        out.push(ch);
        index += ch.len_utf8();
    }
    out
}

pub(crate) fn parse_day_heading(
    title: &str,
    month: YearMonth,
) -> Option<DayHeading> {
    let title = title.trim();
    if let Some(heading) = parse_iso_heading(title, month) {
        return Some(heading);
    }
    if let Some(heading) = parse_canonical_heading(title, month) {
        return Some(heading);
    }
    parse_month_day_heading(title, month)
}

fn parse_iso_heading(title: &str, month: YearMonth) -> Option<DayHeading> {
    let mut parts = title.split_whitespace();
    let iso = parts.next()?;
    let date = NaiveDate::parse_from_str(iso, "%Y-%m-%d").ok()?;
    // Validate trailing tokens before the out-of-month early return, mirroring
    // the in-month branch: at most one token, so `## 2025-12-31 year-end
    // review` is prose, not a day heading.
    let trailing = parts.next();
    if parts.next().is_some() {
        return None;
    }
    if !month.contains(date) {
        return match trailing {
            None => Some(DayHeading {
                date,
                kind: DayHeadingKind::Iso,
                weekday_mismatch: false,
            }),
            Some(weekday) => {
                let expected = weekday_abbr(date);
                Some(DayHeading {
                    date,
                    kind: DayHeadingKind::IsoWeekday,
                    weekday_mismatch: weekday != expected,
                })
            }
        };
    }
    match trailing {
        None => Some(DayHeading {
            date,
            kind: DayHeadingKind::Iso,
            weekday_mismatch: false,
        }),
        Some(weekday) => {
            let expected = weekday_abbr(date);
            Some(DayHeading {
                date,
                kind: DayHeadingKind::IsoWeekday,
                weekday_mismatch: weekday != expected,
            })
        }
    }
}

fn parse_canonical_heading(
    title: &str,
    month: YearMonth,
) -> Option<DayHeading> {
    let mut parts = title.split_whitespace();
    let weekday = parts.next()?;
    let month_name = parts.next()?;
    let day = parts.next()?;
    if parts.next().is_some() {
        return None;
    }
    if !WEEKDAYS.contains(&weekday) {
        return None;
    }
    let month_num = month_index(month_name)?;
    let day: u32 = day.parse().ok()?;
    let date = NaiveDate::from_ymd_opt(month.year, month_num, day)?;
    Some(DayHeading {
        date,
        kind: DayHeadingKind::Canonical,
        weekday_mismatch: weekday != weekday_abbr(date),
    })
}

fn parse_month_day_heading(
    title: &str,
    month: YearMonth,
) -> Option<DayHeading> {
    let mut parts = title.split_whitespace();
    let month_name = parts.next()?;
    let day = parts.next()?;
    if parts.next().is_some() {
        return None;
    }
    let month_num = month_index(month_name)?;
    if month_num != month.month {
        let day: u32 = day.parse().ok()?;
        let date = NaiveDate::from_ymd_opt(month.year, month_num, day)?;
        return Some(DayHeading {
            date,
            kind: DayHeadingKind::MonthDay,
            weekday_mismatch: false,
        });
    }
    let day: u32 = day.parse().ok()?;
    let date = NaiveDate::from_ymd_opt(month.year, month.month, day)?;
    Some(DayHeading {
        date,
        kind: DayHeadingKind::MonthDay,
        weekday_mismatch: false,
    })
}

fn month_index(name: &str) -> Option<u32> {
    MONTHS_ABBR
        .iter()
        .position(|month| *month == name)
        .map(|index| index as u32 + 1)
}

/// True when a `##` title looks like a day heading even if the date is
/// impossible. Used for `bank_day_invalid` on headings the parser rejects.
pub(crate) fn looks_like_day_heading(title: &str) -> bool {
    let title = title.trim();
    let tokens: Vec<&str> = title.split_whitespace().collect();
    if tokens.is_empty() {
        return false;
    }
    if looks_like_iso_date(tokens[0]) {
        return tokens.len() <= 2;
    }
    if tokens.len() == 3
        && WEEKDAYS.contains(&tokens[0])
        && month_index(tokens[1]).is_some()
        && tokens[2].chars().all(|c| c.is_ascii_digit())
    {
        return true;
    }
    tokens.len() == 2
        && month_index(tokens[0]).is_some()
        && tokens[1].chars().all(|c| c.is_ascii_digit())
}

fn looks_like_iso_date(value: &str) -> bool {
    let bytes = value.as_bytes();
    bytes.len() == 10
        && bytes[4] == b'-'
        && bytes[7] == b'-'
        && bytes.iter().enumerate().all(|(index, byte)| {
            if index == 4 || index == 7 {
                true
            } else {
                byte.is_ascii_digit()
            }
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn month() -> YearMonth {
        YearMonth {
            year: 2026,
            month: 11,
        }
    }

    #[test]
    fn plain_and_bold_amounts_parse_to_cents() {
        match parse_amount_lead("$13.71 Smash") {
            AmountLead::Amount { cents, rest } => {
                assert_eq!(cents, -1371);
                assert_eq!(rest.trim_start(), "Smash");
            }
            other => panic!("{other:?}"),
        }
        match parse_amount_lead("**$13.71** Smash") {
            AmountLead::Amount { cents, .. } => assert_eq!(cents, -1371),
            other => panic!("{other:?}"),
        }
        match parse_amount_lead("+$1,250.00 Payroll") {
            AmountLead::Amount { cents, .. } => assert_eq!(cents, 125000),
            other => panic!("{other:?}"),
        }
        match parse_amount_lead("−$4.25 fee") {
            AmountLead::Amount { cents, .. } => assert_eq!(cents, -425),
            other => panic!("{other:?}"),
        }
        match parse_amount_lead("$0.10 a") {
            AmountLead::Amount { cents, .. } => assert_eq!(cents, -10),
            other => panic!("{other:?}"),
        }
        match parse_amount_lead("$0.20 b") {
            AmountLead::Amount { cents, .. } => assert_eq!(cents, -20),
            other => panic!("{other:?}"),
        }
        match parse_amount_lead("$1.234 bad") {
            AmountLead::Unparsed => {}
            other => panic!("{other:?}"),
        }
        assert!(matches!(
            parse_amount_lead("3 things"),
            AmountLead::NotAmount
        ));
        assert!(matches!(parse_amount_lead("$"), AmountLead::Stub));
        assert!(matches!(parse_amount_lead("$   "), AmountLead::Stub));
    }

    #[test]
    fn flags_notes_and_ascii_spellings() {
        let LineKind::Checkbox { status, body } =
            classify_line("- [x] $13.71 Smashburger !! -- with Kelly")
        else {
            panic!("expected checkbox");
        };
        let RowParse::Parsed(row) = parse_row(status, body) else {
            panic!("expected row");
        };
        assert!(row.flagged);
        assert_eq!(row.note.as_deref(), Some("with Kelly"));
        assert_eq!(row.payee, "Smashburger");
    }

    #[test]
    fn empty_stub_is_ignored() {
        assert!(matches!(classify_line("- [x] $"), LineKind::Stub));
        assert!(matches!(classify_line("- [ ] $"), LineKind::Stub));
        assert!(matches!(parse_row('x', "$"), RowParse::Stub));
    }

    #[test]
    fn every_heading_spelling_parses() {
        let date = NaiveDate::from_ymd_opt(2026, 11, 12).unwrap();
        for title in ["Thu Nov 12", "Nov 12", "2026-11-12", "2026-11-12 Thu"] {
            let heading = parse_day_heading(title, month()).unwrap();
            assert_eq!(heading.date, date, "{title}");
            assert!(!heading.weekday_mismatch, "{title}");
        }
        let wrong = parse_day_heading("2026-11-12 Wed", month()).unwrap();
        assert!(wrong.weekday_mismatch);
        assert!(parse_day_heading("Notes", month()).is_none());
        // Trailing junk disqualifies ISO headings in and out of the month.
        assert!(parse_day_heading("2026-11-12 trip retro", month()).is_none());
        assert!(
            parse_day_heading("2025-12-31 year-end review", month()).is_none()
        );
        // A bare out-of-month ISO date still parses as a day heading so the
        // checker can report `bank_day_invalid` for it.
        let outside = parse_day_heading("2025-12-31", month()).unwrap();
        assert_eq!(
            outside.date,
            NaiveDate::from_ymd_opt(2025, 12, 31).unwrap()
        );
        let outside_weekday =
            parse_day_heading("2025-12-31 Wed", month()).unwrap();
        assert!(!outside_weekday.weekday_mismatch);
        assert!(looks_like_day_heading("Thu Nov 31"));
        assert!(looks_like_day_heading("2026-11-32"));
        assert!(looks_like_day_heading("Nov 32"));
        assert!(!looks_like_day_heading("Notes"));
    }

    #[test]
    fn canonical_amount_and_heading_render() {
        assert_eq!(render_amount(-6120), "$61.20");
        assert_eq!(render_amount(125000), "+$1,250.00");
        let date = NaiveDate::from_ymd_opt(2026, 11, 12).unwrap();
        assert_eq!(render_day_heading(date), "## Thu Nov 12");
        assert_eq!(
            render_row(
                true,
                -6120,
                "ShopRite",
                Some("groceries"),
                None,
                &[],
                None,
            ),
            "- [ ] $61.20 ShopRite [cat:: groceries]"
        );
        assert_eq!(
            render_row(
                false,
                125000,
                "Payroll",
                Some("income"),
                None,
                &[],
                Some("1c0e77aa"),
            ),
            "- [x] +$1,250.00 Payroll [cat:: income] ^bank-1c0e77aa"
        );
    }

    #[test]
    fn historical_total_lines_are_not_rows() {
        assert!(matches!(classify_line("total: 12.00"), LineKind::NotARow));
        assert!(matches!(classify_line("- total: 12.00"), LineKind::NotARow));
        assert!(matches!(classify_line("- [ ] 3 things"), LineKind::NotARow));
    }
}
