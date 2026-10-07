//! Tracker detection, status precedence, and reading-state mapping.
//!
//! A tracker is a list task line whose whitespace tokens include `^ref`, read
//! outside fenced code and outside the managed region; the PDF wikilink is
//! optional here (unlike the sync writer's strict task). The checkbox mark
//! maps through the `highlights_ref` mark seam, and frontmatter aliases
//! through its deprecated-status seam.
use super::frontmatter::ParsedFrontmatter;
use super::row::Diagnostic;
use crate::native::highlights_ref::{
    normalize_deprecated_status_str, ref_task_mark_status, MANAGED_BODY_BEGIN,
    MANAGED_BODY_END,
};
use crate::native::markdown::fenced_lines;

/// One `^ref` tracker candidate found in the body.
#[derive(Debug, Clone)]
pub(crate) struct TrackerHit {
    pub mark: char,
    pub line: String,
}

/// The decided status plus its evidence.
#[derive(Debug, Clone)]
pub(crate) struct StatusOutcome {
    pub status: Option<String>,
    pub status_sync: &'static str,
    pub frontmatter_status: Option<String>,
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
    body: &str,
    front: &ParsedFrontmatter,
) -> StatusOutcome {
    let hits = find_trackers(body);
    let front_raw = front
        .get_str("status")
        .map(str::trim)
        .filter(|s| !s.is_empty());
    let front_norm = front_raw.map(normalize_deprecated_status_str);
    let base_norm = base_status(front);
    let legacy_raw = front.get_str("legacy_status");

    // Exactly one tracker with a known mark is usable.
    let usable =
        hits.len() == 1 && ref_task_mark_status(hits[0].mark).is_some();
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
    if hits.len() == 1 && ref_task_mark_status(hits[0].mark).is_none() {
        diagnostics.push(Diagnostic::new(
            "unknown_ref_mark",
            format!("unsupported ^ref checkbox mark [{}]", hits[0].mark),
        ));
    }

    if usable {
        let tracker_status =
            ref_task_mark_status(hits[0].mark).expect("usable mark");
        let tracker = Some(hits[0].clone());
        if front_norm == Some(tracker_status) {
            let (state, source) =
                modern_reading_state(tracker_status, Some(&hits[0]), None);
            return StatusOutcome {
                status: Some(tracker_status.to_string()),
                status_sync: "ok",
                frontmatter_status: None,
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
            has_usable_tracker: false,
            tracker: None,
            reading_state: "unknown",
            reading_state_source: "none".to_string(),
            diagnostics,
        },
        Some("legacy") => {
            let (state, source) = legacy_reading_state(legacy_raw);
            StatusOutcome {
                status: Some("legacy".to_string()),
                status_sync: "ok",
                frontmatter_status: None,
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
    let state = match folded.as_str() {
        "unread" => "queued",
        "collect_fleeting_notes" => "started",
        "read" | "review_fleeting_notes" | "review_lit_notes" => "finished",
        "abandoned" => "dropped",
        _ => "unknown",
    };
    let source = match raw.map(str::trim).filter(|s| !s.is_empty()) {
        Some(value) => format!("legacy_status:{value}"),
        None => "legacy_status:missing".to_string(),
    };
    (state, source)
}

/// The stored base `status` from the `highlights_marker_base` JSON, aliased
/// like frontmatter. Unparsable or missing bases behave as no base.
fn base_status(front: &ParsedFrontmatter) -> Option<String> {
    let raw = front.get_str("highlights_marker_base")?;
    let json: serde_json::Value = serde_json::from_str(raw).ok()?;
    let status = json.get("status")?.as_str()?;
    Some(normalize_deprecated_status_str(status.trim()).to_string())
}

/// All `^ref` tracker candidates: list task lines carrying the token, outside
/// fenced code and outside the managed region.
fn find_trackers(body: &str) -> Vec<TrackerHit> {
    let lines: Vec<&str> = body.lines().collect();
    let fenced = fenced_lines(&lines, 0..lines.len());
    let region = managed_region_line_range(&lines);
    lines
        .iter()
        .enumerate()
        .filter(|(index, _)| {
            !fenced.contains(index)
                && region
                    .is_none_or(|(begin, end)| *index < begin || *index > end)
        })
        .filter_map(|(_, line)| parse_tracker_line(line))
        .collect()
}

/// The managed-region line range (inclusive) when exactly one begin marker is
/// followed by its end marker; anything broken reads as no region.
fn managed_region_line_range(lines: &[&str]) -> Option<(usize, usize)> {
    let begin = lines
        .iter()
        .position(|line| line.contains(MANAGED_BODY_BEGIN))?;
    if lines
        .iter()
        .filter(|line| line.contains(MANAGED_BODY_BEGIN))
        .count()
        > 1
    {
        return None;
    }
    let end = lines[begin..]
        .iter()
        .position(|line| line.contains(MANAGED_BODY_END))
        .map(|offset| begin + offset)?;
    if lines
        .iter()
        .filter(|line| line.contains(MANAGED_BODY_END))
        .count()
        > 1
    {
        return None;
    }
    Some((begin, end))
}

/// A loose `^ref` tracker line: an unordered list task checkbox of any single
/// mark whose whitespace tokens include `^ref`.
fn parse_tracker_line(line: &str) -> Option<TrackerHit> {
    let trimmed = line.trim_start();
    let after_marker = trimmed
        .strip_prefix("- ")
        .or_else(|| trimmed.strip_prefix("* "))
        .or_else(|| trimmed.strip_prefix("+ "))?;
    let bracket = after_marker.strip_prefix('[')?;
    let mark = bracket.chars().next()?;
    let rest = bracket[mark.len_utf8()..].strip_prefix(']')?;
    if !rest.is_empty() && !rest.starts_with(char::is_whitespace) {
        return None;
    }
    if !line.split_whitespace().any(|token| token == "^ref") {
        return None;
    }
    Some(TrackerHit {
        mark,
        line: line.trim().to_string(),
    })
}
