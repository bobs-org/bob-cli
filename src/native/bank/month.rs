//! Month-note model, canonical template, and byte-preserving planners.

use chrono::{NaiveDate, Utc};

use crate::native::articles::split_physical_lines;
use crate::native::markdown::{atx_heading, fenced_lines};
use crate::native::projects::{
    frontmatter_has_type, frontmatter_top_level_value, parse_frontmatter,
};

use super::config::month_from_filename;
use super::grammar::{
    classify_line, parse_day_heading, parse_row, render_day_heading,
    render_month_title, DayHeading, LineKind, ParsedRow, RowParse, YearMonth,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LocatedRow {
    pub(crate) line: usize,
    pub(crate) kind: LocatedRowKind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum LocatedRowKind {
    Parsed(ParsedRow),
    Unparsed,
    NoCheckbox,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DaySection {
    pub(crate) date: NaiveDate,
    pub(crate) heading: DayHeading,
    pub(crate) heading_line: usize,
    pub(crate) heading_text: String,
    pub(crate) rows: Vec<LocatedRow>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct MonthNote {
    pub(crate) path: String,
    pub(crate) month: YearMonth,
    pub(crate) frontmatter_month: Option<YearMonth>,
    pub(crate) type_is_bank_month: bool,
    pub(crate) days: Vec<DaySection>,
    pub(crate) outside_rows: Vec<LocatedRow>,
    pub(crate) other_headings: Vec<(usize, String)>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ByteEdit {
    pub(crate) start: usize,
    pub(crate) end: usize,
    pub(crate) insert: String,
}

impl ByteEdit {
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn apply(&self, text: &str) -> String {
        let mut out = String::with_capacity(text.len() + self.insert.len());
        out.push_str(&text[..self.start]);
        out.push_str(&self.insert);
        out.push_str(&text[self.end..]);
        out
    }
}

struct PhysicalDoc<'a> {
    bom: bool,
    lines: Vec<(&'a str, &'a str)>,
    default_ending: &'static str,
}

impl<'a> PhysicalDoc<'a> {
    fn parse(text: &'a str) -> Self {
        let (bom, rest) = match text.strip_prefix('\u{feff}') {
            Some(rest) => (true, rest),
            None => (false, text),
        };
        let lines = split_physical_lines(rest);
        let default_ending = lines
            .iter()
            .find_map(|(_, ending)| match *ending {
                "\r\n" => Some("\r\n"),
                "\n" => Some("\n"),
                _ => None,
            })
            .unwrap_or("\n");
        Self {
            bom,
            lines,
            default_ending,
        }
    }

    fn contents(&self) -> Vec<&str> {
        self.lines.iter().map(|(content, _)| *content).collect()
    }

    fn byte_offset(&self, line_index: usize) -> usize {
        let mut offset = if self.bom { "\u{feff}".len() } else { 0 };
        for (content, ending) in self.lines.iter().take(line_index) {
            offset += content.len() + ending.len();
        }
        offset
    }

    fn line_end_offset(&self, line_index: usize) -> usize {
        let (content, ending) = self.lines[line_index];
        self.byte_offset(line_index) + content.len() + ending.len()
    }
}

pub(crate) fn parse_month_note(
    path: &str,
    contents: &str,
    filename_month: YearMonth,
) -> MonthNote {
    let doc = PhysicalDoc::parse(contents);
    let logical = doc.contents();
    let frontmatter = parse_frontmatter(contents);
    let frontmatter_month = frontmatter.as_ref().and_then(|fm| {
        frontmatter_top_level_value(fm, "month")
            .and_then(|value| YearMonth::parse(trim_quotes(value)))
    });
    let type_is_bank_month = frontmatter
        .as_ref()
        .is_some_and(|fm| frontmatter_has_type(fm, "[[bank_month]]"));
    let frontmatter_end =
        crate::native::markdown::strictly_closed_frontmatter_end(&logical);
    let scan_start = frontmatter_end.map_or(0, |end| end + 1);
    let fenced = fenced_lines(&logical, scan_start..logical.len());

    let mut days = Vec::new();
    let mut outside_rows = Vec::new();
    let mut other_headings = Vec::new();
    let mut current: Option<usize> = None;

    for (index, line) in logical.iter().enumerate() {
        if frontmatter_end.is_some_and(|end| index <= end)
            || fenced.contains(&index)
        {
            continue;
        }
        let line_number = index + 1;
        if let Some((level, title)) = atx_heading(line)
            && level == 2
        {
            if let Some(heading) = parse_day_heading(title, filename_month) {
                days.push(DaySection {
                    date: heading.date,
                    heading,
                    heading_line: line_number,
                    heading_text: (*line).to_owned(),
                    rows: Vec::new(),
                });
                current = Some(days.len() - 1);
            } else {
                other_headings.push((line_number, title.to_owned()));
                current = None;
            }
            continue;
        }
        match classify_line(line) {
            LineKind::NotARow | LineKind::Stub => {}
            LineKind::AmountNoCheckbox => {
                let row = LocatedRow {
                    line: line_number,
                    kind: LocatedRowKind::NoCheckbox,
                };
                push_row(&mut days, &mut outside_rows, current, row);
            }
            LineKind::Checkbox { status, body } => {
                let kind = match parse_row(status, body) {
                    RowParse::Stub => continue,
                    RowParse::Unparsed => LocatedRowKind::Unparsed,
                    RowParse::Parsed(row) => LocatedRowKind::Parsed(row),
                };
                let row = LocatedRow {
                    line: line_number,
                    kind,
                };
                push_row(&mut days, &mut outside_rows, current, row);
            }
        }
    }

    MonthNote {
        path: path.to_owned(),
        month: filename_month,
        frontmatter_month,
        type_is_bank_month,
        days,
        outside_rows,
        other_headings,
    }
}

fn trim_quotes(value: &str) -> &str {
    let trimmed = value.trim();
    trimmed
        .strip_prefix('"')
        .and_then(|value| value.strip_suffix('"'))
        .or_else(|| {
            trimmed
                .strip_prefix('\'')
                .and_then(|value| value.strip_suffix('\''))
        })
        .unwrap_or(trimmed)
        .trim()
}

fn push_row(
    days: &mut [DaySection],
    outside: &mut Vec<LocatedRow>,
    current: Option<usize>,
    row: LocatedRow,
) {
    if let Some(index) = current {
        days[index].rows.push(row);
    } else {
        outside.push(row);
    }
}

#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn canonical_template(
    month: YearMonth,
    created: &str,
    first_day: NaiveDate,
) -> String {
    format!(
        "---\nparent: \"[[bank]]\"\ntype: \"[[bank_month]]\"\nmonth: {}\ncreated: {created}\n---\n\n{}\n\n```bob-bank\nview: month\n```\n\n{}\n",
        month.as_str(),
        render_month_title(month),
        render_day_heading(first_day)
    )
}

#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn now_created_stamp() -> String {
    Utc::now().format("%Y-%m-%dT%H:%M:%S+00:00").to_string()
}

/// Insert a missing day heading in date order, or after the fence.
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn plan_insert_day(
    text: &str,
    month: YearMonth,
    date: NaiveDate,
) -> Option<ByteEdit> {
    let parsed = parse_month_note("bank/month.md", text, month);
    if parsed.days.iter().any(|day| day.date == date) {
        return None;
    }
    let doc = PhysicalDoc::parse(text);
    let ending = doc.default_ending;
    let heading = render_day_heading(date);
    let insert_after = insert_after_line(&parsed, date);
    let at = match insert_after {
        Some(line) => doc.line_end_offset(line - 1),
        None => after_fence_or_title(&doc),
    };
    let before_blank = needs_blank_before(text, at);
    let mut insert = String::new();
    if before_blank {
        insert.push_str(ending);
    }
    insert.push_str(&heading);
    insert.push_str(ending);
    insert.push_str(ending);
    Some(ByteEdit {
        start: at,
        end: at,
        insert,
    })
}

fn insert_after_line(parsed: &MonthNote, date: NaiveDate) -> Option<usize> {
    let mut last_before = None;
    for day in &parsed.days {
        if day.date < date {
            last_before = Some(day_end_line(day));
        }
    }
    last_before
}

fn day_end_line(day: &DaySection) -> usize {
    day.rows
        .last()
        .map(|row| row.line)
        .unwrap_or(day.heading_line)
}

fn after_fence_or_title(doc: &PhysicalDoc<'_>) -> usize {
    let logical = doc.contents();
    let fenced = fenced_lines(&logical, 0..logical.len());
    if let Some(last_fence) = fenced.iter().max().copied() {
        return doc.line_end_offset(last_fence);
    }
    if let Some(index) = logical
        .iter()
        .position(|line| atx_heading(line).is_some_and(|(level, _)| level == 1))
    {
        return doc.line_end_offset(index);
    }
    let frontmatter_end =
        crate::native::markdown::strictly_closed_frontmatter_end(&logical);
    if let Some(end) = frontmatter_end {
        return doc.line_end_offset(end);
    }
    if doc.lines.is_empty() {
        if doc.bom {
            "\u{feff}".len()
        } else {
            0
        }
    } else {
        doc.line_end_offset(doc.lines.len() - 1)
    }
}

fn needs_blank_before(text: &str, at: usize) -> bool {
    if at == 0 {
        return false;
    }
    let before = &text[..at];
    !before.ends_with("\n\n")
        && !before.ends_with("\r\n\r\n")
        && before.ends_with('\n')
}

/// Append a canonical row under a day, after that day's last row.
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn plan_append_row(
    text: &str,
    month: YearMonth,
    date: NaiveDate,
    row: &str,
) -> Option<ByteEdit> {
    let parsed = parse_month_note("bank/month.md", text, month);
    let day = parsed.days.iter().find(|day| day.date == date)?;
    let doc = PhysicalDoc::parse(text);
    let ending = doc.default_ending;
    let after_line = day_end_line(day);
    let at = doc.line_end_offset(after_line - 1);
    let mut insert = String::new();
    insert.push_str(row);
    if !row.ends_with('\n') {
        insert.push_str(ending);
    }
    Some(ByteEdit {
        start: at,
        end: at,
        insert,
    })
}

/// Tick `[ ]` to `[x]` on a waiting row.
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn plan_tick_row(text: &str, line: usize) -> Option<ByteEdit> {
    let doc = PhysicalDoc::parse(text);
    let index = line.checked_sub(1)?;
    let (content, _) = doc.lines.get(index)?;
    let after = content.strip_prefix("- [ ]")?;
    let start = doc.byte_offset(index);
    Some(ByteEdit {
        start,
        end: start + content.len(),
        insert: format!("- [x]{after}"),
    })
}

/// Append ` ^bank-<hex>` to a typed row.
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn plan_append_id(
    text: &str,
    line: usize,
    id: &str,
) -> Option<ByteEdit> {
    let doc = PhysicalDoc::parse(text);
    let index = line.checked_sub(1)?;
    let (content, _) = doc.lines.get(index)?;
    if content.contains(&format!("^bank-{id}"))
        || content
            .split_whitespace()
            .last()
            .is_some_and(|token| token.starts_with("^bank-"))
    {
        return None;
    }
    let start = doc.byte_offset(index);
    Some(ByteEdit {
        start,
        end: start + content.len(),
        insert: format!("{content} ^bank-{id}"),
    })
}

pub(crate) fn filename_month_for(path: &std::path::Path) -> Option<YearMonth> {
    month_from_filename(path)
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

    fn sample() -> String {
        canonical_template(
            month(),
            "2026-11-12T07:30:00-05:00",
            NaiveDate::from_ymd_opt(2026, 11, 12).unwrap(),
        )
    }

    #[test]
    fn template_matches_contract_shape() {
        let stamp = now_created_stamp();
        assert!(stamp.contains('T'));
        let text = sample();
        assert!(text.contains("type: \"[[bank_month]]\""));
        assert!(text.contains("month: 2026-11"));
        assert!(text.contains("# 🏦 November 2026"));
        assert!(text.contains("```bob-bank"));
        assert!(text.contains("## Thu Nov 12"));
    }

    #[test]
    fn insert_day_preserves_bom_and_crlf() {
        let date = NaiveDate::from_ymd_opt(2026, 11, 13).unwrap();
        let text = format!("\u{feff}{}", sample().replace('\n', "\r\n"));
        let edit = plan_insert_day(&text, month(), date).unwrap();
        let after = edit.apply(&text);
        assert!(after.starts_with('\u{feff}'));
        assert!(after.contains("\r\n## Fri Nov 13\r\n"));
        assert!(after.contains("## Thu Nov 12"));
    }

    #[test]
    fn append_tick_and_id_are_byte_stable() {
        let mut text = sample();
        text.push_str("\n- [ ] $61.20 ShopRite\n");
        let date = NaiveDate::from_ymd_opt(2026, 11, 12).unwrap();
        let edit = plan_append_row(
            &text,
            month(),
            date,
            "- [ ] +$1,250.00 Payroll [cat:: income]",
        )
        .unwrap();
        let text = edit.apply(&text);
        assert!(text.contains("ShopRite"));
        assert!(text.contains("Payroll"));

        let parsed = parse_month_note("bank/2026-11.md", &text, month());
        let shop = parsed.days[0]
            .rows
            .iter()
            .find(|row| {
                matches!(
                    &row.kind,
                    LocatedRowKind::Parsed(parsed) if parsed.payee == "ShopRite"
                )
            })
            .unwrap();
        let edit = plan_tick_row(&text, shop.line).unwrap();
        let text = edit.apply(&text);
        assert!(text.contains("- [x] $61.20 ShopRite"));

        let edit = plan_append_id(&text, shop.line, "2b91c0e4").unwrap();
        let text = edit.apply(&text);
        assert!(text.contains("^bank-2b91c0e4"));
    }

    #[test]
    fn crlf_round_trip_keeps_other_bytes() {
        let text = sample().replace('\n', "\r\n");
        let date = NaiveDate::from_ymd_opt(2026, 11, 10).unwrap();
        let edit = plan_insert_day(&text, month(), date).unwrap();
        let after = edit.apply(&text);
        assert!(!after.contains("\n##") || after.contains("\r\n##"));
        assert!(after.contains("\r\n"));
        assert!(!after.contains("\n\n") || after.contains("\r\n\r\n"));
    }
}
