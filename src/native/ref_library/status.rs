//! Tracker detection, status precedence, and reading-state mapping.
//!
//! A tracker is a list task line whose whitespace tokens include `^ref`, read
//! outside fenced code and outside the managed region; the PDF wikilink is
//! optional here (unlike the sync writer's strict task). The checkbox mark
//! maps through the `highlights_ref` mark seams, and frontmatter aliases
//! through its deprecated-status seam. A single Blocked `[?]` tracker is
//! status-neutral: it defers to the normalized frontmatter status (open
//! statuses stay as is with source `ref_task:[?]`; terminal ones target
//! `ready` through the usual pending/conflict comparison).
use super::frontmatter::ParsedFrontmatter;
use super::row::Diagnostic;
use crate::native::highlights_ref::{
    is_known_ref_task_mark, normalize_deprecated_status_str,
    ref_task_mark_target_status,
};
pub(crate) use crate::native::ref_tasks::TrackerHit;

/// The decided status plus its evidence.
#[derive(Debug, Clone)]
pub(crate) struct StatusOutcome {
    pub status: Option<String>,
    pub status_sync: &'static str,
    pub frontmatter_status: Option<String>,
    /// True when the note carries exactly one `^ref` tracker and its
    /// mark is Blocked `[?]`: an overlay on the reading lane, never a
    /// status of its own.
    pub blocked: bool,
    pub has_usable_tracker: bool,
    pub tracker: Option<TrackerHit>,
    pub reading_state: &'static str,
    pub reading_state_source: String,
    pub diagnostics: Vec<Diagnostic>,
}

/// Rank a reading state for primary-match ordering: the most-advanced order
/// from the plan (`finished`, `started`, `queued`, `dropped`, `unknown`).
pub(crate) fn reading_state_rank(state: &str) -> u8 {
    match state {
        "finished" => 0,
        "started" => 1,
        "queued" => 2,
        "dropped" => 3,
        _ => 4,
    }
}

/// Decide the effective status and derived reading state for one note.
pub(crate) fn decide_status(
    hits: &[TrackerHit],
    front: &ParsedFrontmatter,
) -> StatusOutcome {
    let front_raw = front
        .get_str("status")
        .map(str::trim)
        .filter(|s| !s.is_empty());
    let front_norm = front_raw.map(normalize_deprecated_status_str);
    let base_norm = base_status(front);
    let legacy_raw = front.get_str("legacy_status");

    // Exactly one tracker with a status-aware target is usable. A `[?]`
    // tracker defers to the normalized frontmatter status, so it is usable
    // only with an open or terminal frontmatter status; otherwise it falls
    // through to the frontmatter path without an `unknown_ref_mark`.
    let single_target = if hits.len() == 1 {
        ref_task_mark_target_status(hits[0].mark, front_norm)
    } else {
        None
    };
    let usable = hits.len() == 1 && single_target.is_some();
    let blocked = hits.len() == 1 && hits[0].mark == '?';
    let mut diagnostics = Vec::new();
    if hits.len() > 1 {
        diagnostics.push(Diagnostic::new(
            "multiple_ref_trackers",
            format!(
                "{} ^ref tracker lines; using frontmatter status",
                hits.len()
            ),
        ));
    }
    if hits.len() == 1 && !is_known_ref_task_mark(hits[0].mark) {
        diagnostics.push(Diagnostic::new(
            "unknown_ref_mark",
            format!("unsupported ^ref checkbox mark [{}]", hits[0].mark),
        ));
    }

    if usable {
        let tracker_status = single_target.expect("usable mark");
        let tracker = Some(hits[0].clone());
        if front_norm == Some(tracker_status) {
            let (state, source) =
                modern_reading_state(tracker_status, Some(&hits[0]), None);
            return StatusOutcome {
                status: Some(tracker_status.to_string()),
                status_sync: "ok",
                frontmatter_status: None,
                blocked,
                has_usable_tracker: true,
                tracker,
                reading_state: state,
                reading_state_source: source,
                diagnostics,
            };
        }
        let front_at_base =
            base_norm.is_none() || front_norm == base_norm.as_deref();
        if front_at_base {
            let (state, source) =
                modern_reading_state(tracker_status, Some(&hits[0]), None);
            return StatusOutcome {
                status: Some(tracker_status.to_string()),
                status_sync: "pending",
                frontmatter_status: front_norm.map(str::to_string),
                blocked,
                has_usable_tracker: true,
                tracker,
                reading_state: state,
                reading_state_source: source,
                diagnostics,
            };
        }
        diagnostics.push(Diagnostic::new(
            "status_conflict",
            format!(
                "tracker targets {} but frontmatter moved to {}",
                tracker_status,
                front_norm.unwrap_or("missing"),
            ),
        ));
        return StatusOutcome {
            status: Some("conflict".to_string()),
            status_sync: "conflict",
            frontmatter_status: front_norm.map(str::to_string),
            blocked,
            has_usable_tracker: true,
            tracker,
            reading_state: "unknown",
            reading_state_source: format!(
                "conflict:ref_task=[{}],frontmatter={}",
                hits[0].mark,
                front_norm.unwrap_or("missing"),
            ),
            diagnostics,
        };
    }

    // No usable tracker: the normalized frontmatter status decides.
    match front_norm {
        None => StatusOutcome {
            status: None,
            status_sync: "ok",
            frontmatter_status: None,
            blocked,
            has_usable_tracker: false,
            tracker: None,
            reading_state: "unknown",
            reading_state_source: "none".to_string(),
            diagnostics,
        },
        Some("legacy") => {
            // A book with folded chapters derives its reading state
            // from them; a book without chapters stays unknown.
            if legacy_raw
                .map(|value| value.trim().eq_ignore_ascii_case("book"))
                .unwrap_or(false)
            {
                let chapters = front.get_all("legacy_chapter_statuses");
                if !chapters.is_empty() {
                    let (state, source) = book_reading_state(&chapters);
                    return StatusOutcome {
                        status: Some("legacy".to_string()),
                        status_sync: "ok",
                        frontmatter_status: None,
                        blocked,
                        has_usable_tracker: false,
                        tracker: None,
                        reading_state: state,
                        reading_state_source: source,
                        diagnostics,
                    };
                }
            }
            let (state, source) = legacy_reading_state(legacy_raw);
            StatusOutcome {
                status: Some("legacy".to_string()),
                status_sync: "ok",
                frontmatter_status: None,
                blocked,
                has_usable_tracker: false,
                tracker: None,
                reading_state: state,
                reading_state_source: source,
                diagnostics,
            }
        }
        Some(status) => {
            let (state, source) =
                modern_reading_state(status, None, front.get_str("status"));
            StatusOutcome {
                status: Some(status.to_string()),
                status_sync: "ok",
                frontmatter_status: None,
                blocked,
                has_usable_tracker: false,
                tracker: None,
                reading_state: state,
                reading_state_source: source,
                diagnostics,
            }
        }
    }
}

/// Modern reading state from a status, with its evidence string.
fn modern_reading_state(
    status: &str,
    tracker: Option<&TrackerHit>,
    frontmatter_raw: Option<&str>,
) -> (&'static str, String) {
    let state = match status {
        "ready" | "next" => "queued",
        "wip" => "started",
        "read" => "finished",
        "abandoned" => "dropped",
        _ => "unknown",
    };
    let source = match tracker {
        Some(hit) => format!("ref_task:[{}]", hit.mark),
        None => {
            format!("frontmatter:{}", frontmatter_raw.unwrap_or(status).trim())
        }
    };
    (state, source)
}

/// Legacy reading state from the raw `legacy_status` value.
fn legacy_reading_state(raw: Option<&str>) -> (&'static str, String) {
    let folded = raw
        .map(|value| value.trim().to_lowercase())
        .unwrap_or_default();
    let state = map_legacy_status(&folded);
    let source = match raw.map(str::trim).filter(|s| !s.is_empty()) {
        Some(value) => format!("legacy_status:{value}"),
        None => "legacy_status:missing".to_string(),
    };
    (state, source)
}

/// One folded lowercase legacy status mapped to its reading state.
fn map_legacy_status(folded: &str) -> &'static str {
    match folded {
        "unread" => "queued",
        "collect_fleeting_notes" => "started",
        "read" | "review_fleeting_notes" | "review_lit_notes" => "finished",
        "abandoned" => "dropped",
        _ => "unknown",
    }
}

/// A legacy book's derived reading state from its chapters' raw
/// statuses: finished when every known chapter is finished, started
/// when any known chapter is started or finished, queued when known
/// chapters remain, dropped when any chapter was dropped, and unknown
/// otherwise.
fn book_reading_state(chapters: &[String]) -> (&'static str, String) {
    let mapped: Vec<&str> = chapters
        .iter()
        .map(|status| map_legacy_status(&status.trim().to_lowercase()))
        .collect();
    let total = mapped.len();
    let finished = mapped.iter().filter(|state| **state == "finished").count();
    let known: Vec<&&str> = mapped
        .iter()
        .filter(|state| matches!(**state, "finished" | "started" | "queued"))
        .collect();
    let state = if !known.is_empty()
        && known.iter().all(|state| **state == "finished")
    {
        "finished"
    } else if known
        .iter()
        .any(|state| matches!(**state, "started" | "finished"))
    {
        "started"
    } else if !known.is_empty() {
        "queued"
    } else if mapped.contains(&"dropped") {
        "dropped"
    } else {
        "unknown"
    };
    (state, format!("legacy_chapters:{finished}/{total}"))
}

/// The stored base `status` from the `highlights_marker_base` JSON, aliased
/// like frontmatter. Unparsable or missing bases behave as no base.
fn base_status(front: &ParsedFrontmatter) -> Option<String> {
    let raw = front.get_str("highlights_marker_base")?;
    let json: serde_json::Value = serde_json::from_str(raw).ok()?;
    let status = json.get("status")?.as_str()?;
    Some(normalize_deprecated_status_str(status.trim()).to_string())
}

#[cfg(test)]
pub(crate) use crate::native::ref_tasks::{
    find_trackers, managed_region_line_range, parse_tracker_line,
};

#[cfg(test)]
mod tests {
    use super::*;

    fn book_front(chapters: &[&str]) -> ParsedFrontmatter {
        let mut raw = vec![
            "status: legacy".to_string(),
            "legacy_status: book".to_string(),
            "legacy_chapter_statuses:".to_string(),
        ];
        for chapter in chapters {
            raw.push(format!("  - \"{chapter}\""));
        }
        ParsedFrontmatter::parse(&raw)
    }

    fn derived_state(chapters: &[&str]) -> (String, String) {
        let front = book_front(chapters);
        let outcome = decide_status(&[], &front);
        assert_eq!(outcome.status.as_deref(), Some("legacy"));
        (
            outcome.reading_state.to_string(),
            outcome.reading_state_source.clone(),
        )
    }

    #[test]
    fn book_derivation_covers_every_branch() {
        // Every known chapter finished.
        assert_eq!(
            derived_state(&["read", "review_fleeting_notes"]),
            ("finished".to_string(), "legacy_chapters:2/2".to_string())
        );
        // A started chapter alongside finished ones.
        assert_eq!(
            derived_state(&["read", "collect_fleeting_notes"]),
            ("started".to_string(), "legacy_chapters:1/2".to_string())
        );
        // A queued chapter alongside a finished one is still started.
        assert_eq!(
            derived_state(&["read", "unread"]),
            ("started".to_string(), "legacy_chapters:1/2".to_string())
        );
        // Only queued chapters remain.
        assert_eq!(
            derived_state(&["unread", "unread"]),
            ("queued".to_string(), "legacy_chapters:0/2".to_string())
        );
        // Unknown and dropped chapters are set aside first.
        assert_eq!(
            derived_state(&["unread", "abandoned", "book"]),
            ("queued".to_string(), "legacy_chapters:0/3".to_string())
        );
        // No known chapter, but one was dropped.
        assert_eq!(
            derived_state(&["abandoned", "book"]),
            ("dropped".to_string(), "legacy_chapters:0/2".to_string())
        );
        // Nothing known and nothing dropped.
        assert_eq!(
            derived_state(&["book", "missing"]),
            ("unknown".to_string(), "legacy_chapters:0/2".to_string())
        );
    }

    #[test]
    fn book_without_chapters_stays_unknown() {
        let front = ParsedFrontmatter::parse(&[
            "status: legacy".to_string(),
            "legacy_status: book".to_string(),
        ]);
        let outcome = decide_status(&[], &front);
        assert_eq!(outcome.reading_state, "unknown");
        assert_eq!(
            outcome.reading_state_source,
            "legacy_status:book".to_string()
        );
    }

    #[test]
    fn non_book_legacy_ignores_chapter_statuses() {
        let mut raw = vec![
            "status: legacy".to_string(),
            "legacy_status: read".to_string(),
            "legacy_chapter_statuses:".to_string(),
        ];
        raw.push("  - \"unread\"".to_string());
        let outcome = decide_status(&[], &ParsedFrontmatter::parse(&raw));
        assert_eq!(outcome.reading_state, "finished");
        assert_eq!(
            outcome.reading_state_source,
            "legacy_status:read".to_string()
        );
    }
}
