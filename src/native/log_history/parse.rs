use std::{collections::BTreeMap, sync::OnceLock};

use chrono::{NaiveDate, NaiveDateTime};
use serde::{Deserialize, Serialize};

use crate::native::{
    capture::{self, ManagedTaskLogKind},
    markdown,
    task_dependencies::blocks::is_cancel_log,
};

use super::{classify, rank, Excluded, LogKind};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct LogUse {
    pub(crate) kind: LogKind,
    pub(crate) stamp: String,
    pub(crate) text: String,
    pub(crate) key: String,
    pub(crate) last_used: String,
    /// Lexically sortable at the precision of this log kind.
    pub(crate) sort_key: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) struct NoteScan {
    pub(crate) uses: Vec<LogUse>,
    pub(crate) excluded: BTreeMap<LogKind, Excluded>,
    pub(crate) warnings: Vec<String>,
}

pub(crate) fn scan_note(contents: &str, today: NaiveDate) -> NoteScan {
    let spans = capture::line_spans(contents);
    let lines = spans.iter().map(|line| line.text).collect::<Vec<_>>();
    let fenced = markdown::fenced_lines(&lines, 0..lines.len());
    let mut scan = NoteScan::default();
    for marker_index in 0..spans.len() {
        if fenced.contains(&marker_index) {
            continue;
        }
        let Some(kind) = marker_kind(spans[marker_index].text) else {
            continue;
        };
        for child_index in marker_index + 1..spans.len() {
            if capture::nearest_shallower_list_item_parent(&spans, child_index)
                != Some(marker_index)
            {
                continue;
            }
            if fenced.contains(&child_index) {
                continue;
            }
            let Some(body) = capture::list_item_body(spans[child_index].text)
            else {
                continue;
            };
            match parse_entry(kind, body, today) {
                Ok(Some(entry)) => scan.uses.push(entry),
                Ok(None) => {}
                Err(EntryReject::Machine) => {
                    excluded_mut(&mut scan.excluded, kind).machine += 1
                }
                Err(EntryReject::Blank) => {
                    excluded_mut(&mut scan.excluded, kind).blank += 1
                }
                Err(EntryReject::TooLong) => {
                    excluded_mut(&mut scan.excluded, kind).too_long += 1
                }
                Err(EntryReject::Unparsed) => {
                    excluded_mut(&mut scan.excluded, kind).unparsed += 1
                }
            }
        }
    }
    if !scan.warnings.is_empty() {
        scan.warnings.truncate(8);
    }
    scan
}

fn marker_kind(line: &str) -> Option<LogKind> {
    if is_cancel_log(line) {
        return Some(LogKind::Cancel);
    }
    match capture::parse_managed_task_log_marker(line)? {
        ManagedTaskLogKind::Schedule => Some(LogKind::Schedule),
        ManagedTaskLogKind::Work => Some(LogKind::Work),
        ManagedTaskLogKind::Rotten => Some(LogKind::Rotten),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EntryReject {
    Machine,
    Blank,
    TooLong,
    Unparsed,
}

fn parse_entry(
    kind: LogKind,
    entry: &str,
    today: NaiveDate,
) -> Result<Option<LogUse>, EntryReject> {
    let Some((stamp_text, raw_body)) = entry
        .strip_prefix('*')
        .and_then(|rest| rest.split_once("* — "))
    else {
        return Err(EntryReject::Unparsed);
    };
    if stamp_text.contains('*') {
        return Err(EntryReject::Unparsed);
    }
    let body = collapse_whitespace(match kind {
        LogKind::Rotten => strip_rotten_writer_tail(raw_body),
        _ => raw_body,
    });
    if classify::is_machine_text(&body) {
        return Err(EntryReject::Machine);
    }
    if body.is_empty() {
        return Err(EntryReject::Blank);
    }
    if body.chars().count() > 200 {
        return Err(EntryReject::TooLong);
    }
    let (last_used, sort_key) =
        parse_stamp(kind, stamp_text, today).ok_or(EntryReject::Unparsed)?;
    let key = rank::normalize(&body);
    if key.is_empty() {
        return Err(EntryReject::Blank);
    }
    Ok(Some(LogUse {
        kind,
        stamp: stamp_text.to_string(),
        text: body,
        key,
        last_used,
        sort_key,
    }))
}

fn parse_stamp(
    kind: LogKind,
    stamp: &str,
    today: NaiveDate,
) -> Option<(String, String)> {
    match kind {
        LogKind::Work => {
            if let Ok(datetime) =
                NaiveDateTime::parse_from_str(stamp, "%Y-%m-%d %H:%M")
            {
                let value = datetime.format("%Y-%m-%d %H:%M").to_string();
                Some((value.clone(), value))
            } else {
                let date = parse_date(stamp)?;
                Some((
                    format_date(date),
                    format!("{} 00:00", format_date(date)),
                ))
            }
        }
        LogKind::Rotten | LogKind::Cancel => {
            let date = parse_date(stamp)?;
            let value = format_date(date);
            Some((value.clone(), value))
        }
        LogKind::Schedule => {
            let mut dates = stamp.split(" → ");
            let first = parse_date(dates.next()?)?;
            let date = match dates.next() {
                Some(second) if dates.next().is_none() => {
                    first.min(parse_date(second)?)
                }
                None => first,
                _ => return None,
            }
            .min(today);
            let value = format_date(date);
            Some((value.clone(), value))
        }
    }
}

fn parse_date(text: &str) -> Option<NaiveDate> {
    NaiveDate::parse_from_str(text, "%Y-%m-%d").ok()
}

fn format_date(date: NaiveDate) -> String {
    date.format("%Y-%m-%d").to_string()
}

fn collapse_whitespace(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn excluded_mut(
    map: &mut BTreeMap<LogKind, Excluded>,
    kind: LogKind,
) -> &mut Excluded {
    map.entry(kind).or_default()
}

/// Remove only the anchored writer suffixes used by the Rotten log formatters.
pub(crate) fn strip_rotten_writer_tail(body: &str) -> &str {
    static PATTERNS: OnceLock<[regex::Regex; 4]> = OnceLock::new();
    let patterns = PATTERNS.get_or_init(|| {
        [
            regex::Regex::new(r" · kept \d+×(?: in| at) [^ ·]+$")
                .expect("valid pattern"),
            regex::Regex::new(r" · kept \d+×$").expect("valid pattern"),
            regex::Regex::new(r" · kept in Next$").expect("valid pattern"),
            regex::Regex::new(r" · stayed (?:in|at) .+ after \d+ keeps?$")
                .expect("valid pattern"),
        ]
    });
    for pattern in patterns {
        if let Some(found) = pattern.find(body)
            && found.end() == body.len()
        {
            return body[..found.start()].trim_end();
        }
    }
    body
}
