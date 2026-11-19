use std::{
    fs,
    path::{Path, PathBuf},
};

use chrono::{Datelike, NaiveDate};

use crate::native::periods::{
    calendar::{PeriodId, PeriodState},
    labels,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum NoteSubject {
    Day(NaiveDate),
    Period(PeriodId),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SubjectInfo {
    pub(crate) subject: NoteSubject,
    pub(crate) id: String,
    pub(crate) kind: &'static str,
    pub(crate) label: String,
    pub(crate) path: PathBuf,
    pub(crate) parent: Option<NoteSubject>,
    pub(crate) start: Option<NaiveDate>,
    pub(crate) end: Option<NaiveDate>,
    pub(crate) state: &'static str,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ReadNoteError {
    pub(crate) code: &'static str,
    pub(crate) path: PathBuf,
    pub(crate) message: String,
    pub(crate) hint: String,
}

/// Read a resolved canonical note without creating it or interpreting its
/// fields. Missing notes carry the approved day/period-specific recovery hint.
pub(crate) fn read_existing(
    vault: &Path,
    info: &SubjectInfo,
) -> Result<String, ReadNoteError> {
    let path = vault.join(&info.path);
    fs::read_to_string(&path).map_err(|error| {
        let missing = error.kind() == std::io::ErrorKind::NotFound;
        let hint = match (missing, info.subject) {
            (true, NoteSubject::Day(_)) => {
                "Open the day in Obsidian; Templater creates daily notes."
                    .to_owned()
            }
            (true, NoteSubject::Period(id)) => {
                format!(
                    "Run `bob periods ensure {id}` before reading its fields."
                )
            }
            (false, _) => String::new(),
        };
        ReadNoteError {
            code: if missing {
                "missing_note"
            } else {
                "note_read_failed"
            },
            path: info.path.clone(),
            message: error.to_string(),
            hint,
        }
    })
}

impl NoteSubject {
    /// Resolve a user-facing id. `today` is explicit so callers can use the
    /// BOB_NOW-aware clock once and pass the same date through a projection.
    pub(crate) fn resolve(
        text: &str,
        today: NaiveDate,
    ) -> Result<SubjectInfo, String> {
        let raw = text.trim();
        if raw.eq_ignore_ascii_case("today") {
            return Ok(day_info(today, today));
        }
        if raw.eq_ignore_ascii_case("yesterday")
            || raw.eq_ignore_ascii_case("last-day")
        {
            let day = today.pred_opt().ok_or_else(|| invalid_subject(text))?;
            return Ok(day_info(day, today));
        }
        if let Some(day) = parse_date_argument(raw) {
            return Ok(day_info(day, today));
        }
        if !raw.is_ascii() {
            return Err(invalid_subject(text));
        }
        let id = PeriodId::resolve(raw, today)?;
        Ok(period_info(id, today))
    }

    /// Recognize only canonical note paths. A notebook whose basename looks
    /// like a date is deliberately not accepted.
    pub(crate) fn from_canonical_note(
        path: &str,
        today: NaiveDate,
    ) -> Option<SubjectInfo> {
        let normalized = path.trim_start_matches("./").replace('\\', "/");
        let no_md = normalized.strip_suffix(".md").unwrap_or(&normalized);
        let components: Vec<_> = no_md.split('/').collect();
        if components.len() == 2
            && components[0].len() == 4
            && components[0].bytes().all(|byte| byte.is_ascii_digit())
            && components[1].len() == 8
            && components[1].bytes().all(|byte| byte.is_ascii_digit())
        {
            let date = parse_compact_date(components[1])?;
            if date.format("%Y").to_string() == components[0] {
                return Some(day_info(date, today));
            }
        }
        let candidate = match components.as_slice() {
            [year, stem] if year.len() == 4 && stem.starts_with(year) => {
                format!("{year}/{stem}")
            }
            [stem] => (*stem).to_owned(),
            _ => return None,
        };
        let id = PeriodId::parse(&candidate).ok()?;
        if id == PeriodId::AllTime
            && !matches!(components.as_slice(), ["all_time"])
        {
            return None;
        }
        let canonical = id.path().to_string_lossy().replace('\\', "/");
        let requested = if normalized.ends_with(".md") {
            normalized
        } else {
            format!("{normalized}.md")
        };
        (requested == canonical).then(|| period_info(id, today))
    }

    pub(crate) fn day_path(date: NaiveDate) -> PathBuf {
        PathBuf::from(format!(
            "{}/{:04}{:02}{:02}.md",
            date.year(),
            date.year(),
            date.month(),
            date.day()
        ))
    }
}

fn parse_date_argument(text: &str) -> Option<NaiveDate> {
    if text.len() == 10 {
        let date = NaiveDate::parse_from_str(text, "%Y-%m-%d").ok()?;
        return (date.format("%Y-%m-%d").to_string() == text).then_some(date);
    }
    if text.len() == 8 && text.bytes().all(|byte| byte.is_ascii_digit()) {
        return parse_compact_date(text);
    }
    if text.ends_with(".md") || text.contains('/') || text.contains('\\') {
        let normalized = text.replace('\\', "/");
        let stem = normalized.strip_suffix(".md").unwrap_or(&normalized);
        let parts: Vec<_> = stem.split('/').collect();
        if let [year, compact] = parts.as_slice()
            && year.len() == 4
            && compact.len() == 8
            && compact.starts_with(year)
        {
            return parse_compact_date(compact);
        }
    }
    None
}

fn parse_compact_date(text: &str) -> Option<NaiveDate> {
    if text.len() != 8 || !text.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    let date = NaiveDate::parse_from_str(text, "%Y%m%d").ok()?;
    (date.format("%Y%m%d").to_string() == text).then_some(date)
}

fn day_info(day: NaiveDate, today: NaiveDate) -> SubjectInfo {
    let state = if day < today {
        "closed"
    } else if day > today {
        "future"
    } else {
        "current"
    };
    let week = PeriodId::week_for_date(day);
    SubjectInfo {
        subject: NoteSubject::Day(day),
        id: day.format("%Y-%m-%d").to_string(),
        kind: "day",
        label: day.format("%a %b %-d, %Y").to_string(),
        path: NoteSubject::day_path(day),
        parent: Some(NoteSubject::Period(week)),
        start: Some(day),
        end: Some(day),
        state,
    }
}

fn period_info(id: PeriodId, today: NaiveDate) -> SubjectInfo {
    let state = match id.state(today) {
        PeriodState::Closed => "closed",
        PeriodState::Current => "current",
        PeriodState::Future => "future",
    };
    let (kind, label) = match id {
        PeriodId::Week { .. } => ("week", labels::title(id)),
        PeriodId::Quarter { .. } => ("quarter", labels::title(id)),
        PeriodId::Year(_) => ("year", labels::title(id)),
        PeriodId::AllTime => ("all_time", "All time".to_owned()),
    };
    SubjectInfo {
        subject: NoteSubject::Period(id),
        id: id.to_string(),
        kind,
        label,
        path: id.path(),
        parent: id.parent().map(NoteSubject::Period),
        start: id.start(),
        end: id.end(),
        state,
    }
}

fn invalid_subject(text: &str) -> String {
    format!("invalid day or period subject {text:?}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(year: i32, month: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(year, month, day).unwrap()
    }

    #[test]
    fn resolves_day_aliases_and_canonical_paths_strictly() {
        let today = d(2026, 1, 1);
        for alias in [
            "2026-01-01",
            "20260101",
            "2026/20260101",
            "2026/20260101.md",
        ] {
            let resolved = NoteSubject::resolve(alias, today).unwrap();
            assert_eq!(resolved.subject, NoteSubject::Day(today));
            assert_eq!(resolved.path, PathBuf::from("2026/20260101.md"));
        }
        assert_eq!(
            NoteSubject::resolve("today", today).unwrap().subject,
            NoteSubject::Day(today)
        );
        assert_eq!(
            NoteSubject::resolve("yesterday", today).unwrap().subject,
            NoteSubject::Day(d(2025, 12, 31))
        );
        assert!(NoteSubject::resolve("2026-02-30", today).is_err());
        assert!(NoteSubject::resolve("☃.md", today).is_err());
        assert_eq!(
            NoteSubject::from_canonical_note("2026/20260101.md", today)
                .unwrap()
                .subject,
            NoteSubject::Day(today)
        );
        assert!(NoteSubject::from_canonical_note(
            "notebooks/20260101.md",
            today
        )
        .is_none());
    }

    #[test]
    fn period_parent_and_iso_week_53_paths_follow_the_calendar() {
        let today = d(2027, 1, 3);
        let resolved = NoteSubject::resolve("2020-W53", today).unwrap();
        assert_eq!(resolved.path, PathBuf::from("2020/2020-W53.md"));
        assert_eq!(resolved.start, Some(d(2020, 12, 28)));
        let day = NoteSubject::resolve("2021-01-01", today).unwrap();
        assert_eq!(
            day.parent,
            Some(NoteSubject::Period(PeriodId::Week {
                year: 2020,
                week: 53
            }))
        );
        assert!(NoteSubject::resolve("2021-W53", today).is_err());
    }

    #[test]
    fn existing_note_read_reports_recovery_hints_without_creating_notes() {
        let today = d(2026, 1, 1);
        let vault = tempfile::tempdir().expect("vault");
        let day = NoteSubject::resolve("today", today).expect("day");
        let missing = read_existing(vault.path(), &day).unwrap_err();
        assert_eq!(missing.code, "missing_note");
        assert!(missing.hint.contains("Open the day in Obsidian"));
        assert!(!vault.path().join(&day.path).exists());

        let period = NoteSubject::resolve("2026-W01", today).expect("week");
        let missing = read_existing(vault.path(), &period).unwrap_err();
        assert_eq!(missing.code, "missing_note");
        assert!(missing.hint.contains("bob periods ensure 2026-W01"));
        assert!(!vault.path().join(&period.path).exists());

        fs::create_dir_all(vault.path().join("2026")).expect("year dir");
        fs::write(vault.path().join(&day.path), "day bytes")
            .expect("write day");
        assert_eq!(read_existing(vault.path(), &day).unwrap(), "day bytes");
    }
}
