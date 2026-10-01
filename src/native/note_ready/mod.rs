//! Per-note Ready cap: pure evaluator on the freshness snapshot.
//!
//! The contract lives in `docs/plan.md` ("## Ready cap per note").
//! `bob-ledger-tools` mirrors it in JavaScript; both sides run the
//! R1–R14 vectors verbatim.

mod scan;

pub(crate) use scan::{default_cap_source, scan_note_ready, ScanError};

/// Lint when `ready_cap` is present but not `1–999`, `off`, or `false`.
pub(crate) const LINT_NOTE_READY_CAP_INVALID: &str = "note_ready_cap_invalid";
/// Lint when a done/canceled project still holds counted rows.
pub(crate) const LINT_NOTE_READY_IN_TERMINAL_PROJECT: &str =
    "note_ready_in_terminal_project";

/// Where a note's cap came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CapSource {
    Note,
    Config,
    Default,
    Preview,
}

impl CapSource {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Note => "note",
            Self::Config => "config",
            Self::Default => "default",
            Self::Preview => "preview",
        }
    }
}

/// Per-note Ready state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum NoteState {
    Exempt,
    Crowded,
    Full,
    Room,
    Empty,
}

impl NoteState {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Exempt => "exempt",
            Self::Crowded => "crowded",
            Self::Full => "full",
            Self::Room => "room",
            Self::Empty => "empty",
        }
    }

    fn rank(self) -> u8 {
        match self {
            Self::Crowded => 0,
            Self::Full => 1,
            Self::Room => 2,
            Self::Empty => 3,
            Self::Exempt => 4,
        }
    }
}

/// Freshness make-up of a note's counted rows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct MakeUp {
    pub(crate) ready: u32,
    pub(crate) new: u32,
    pub(crate) rotten: u32,
}

/// One per-note entry. Field names match CLI JSON and
/// `api.noteReady`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct NoteReady {
    pub(crate) path: String,
    pub(crate) name: String,
    pub(crate) kind: String,
    pub(crate) status: String,
    pub(crate) parent: Option<String>,
    pub(crate) count: u32,
    pub(crate) cap: Option<u32>,
    pub(crate) cap_source: CapSource,
    pub(crate) state: NoteState,
    pub(crate) over_by: u32,
    pub(crate) make_up: Option<MakeUp>,
    pub(crate) recurring: u32,
}

/// One note lint, emitted once per note, never once per row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct NoteLint {
    pub(crate) code: String,
    pub(crate) path: String,
    pub(crate) message: String,
}

/// Whole-vault totals.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Totals {
    /// Eligible, non-exempt notes.
    pub(crate) notes: u32,
    pub(crate) areas: u32,
    pub(crate) projects: u32,
    pub(crate) crowded: u32,
    pub(crate) full: u32,
    pub(crate) room: u32,
    pub(crate) empty: u32,
    pub(crate) exempt: u32,
    /// Σ count over capped notes.
    pub(crate) counted: u32,
    /// Σ max(0, count − cap) over capped notes.
    pub(crate) excess: u32,
    pub(crate) recurring: u32,
}

/// Evaluated per-note report.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Report {
    pub(crate) notes: Vec<NoteReady>,
    pub(crate) totals: Totals,
    pub(crate) lints: Vec<NoteLint>,
}

/// One typed note input for the pure evaluator.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct NoteInput {
    pub(crate) path: String,
    pub(crate) name: String,
    pub(crate) kind: String,
    pub(crate) status: String,
    pub(crate) is_area: bool,
    pub(crate) is_terminal: bool,
    pub(crate) parent: Option<String>,
    pub(crate) ready_cap_raw: Option<String>,
}

/// One Ready-lane row input for the pure evaluator. Callers pass
/// only [`crate::native::dataview::READY_QUERY`] rows; the lane
/// predicate is already applied.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LaneRow {
    pub(crate) path: String,
    pub(crate) is_recurring: bool,
    pub(crate) block_id: Option<String>,
    /// Freshness bucket: `Some("new")`, `Some("rotten")`
    /// (resurfaced folds into rotten), or `None` for ready/fresh
    /// and out-of-scope rows (Today, daily). `None` for every row
    /// plus `freshness_available: false` gives `make_up: None`.
    pub(crate) bucket: Option<String>,
}

impl LaneRow {
    pub(crate) fn counted(&self) -> bool {
        !self.is_recurring && self.block_id.as_deref() != Some("prj")
    }

    pub(crate) fn recurring_only(&self) -> bool {
        self.is_recurring && self.block_id.as_deref() != Some("prj")
    }
}

/// Parse a raw `ready_cap` frontmatter value.
///
/// Returns `(cap, source, exempt, invalid)`: `cap` is `None` for
/// exempt notes; `invalid` is true when the value was present but
/// not `1–999`, `off`, or `false`, in which case the caller falls
/// back to the default and emits
/// [`LINT_NOTE_READY_CAP_INVALID`].
pub(crate) fn parse_ready_cap(
    raw: Option<&str>,
    default_cap: u32,
    default_source: CapSource,
) -> (Option<u32>, CapSource, bool, bool) {
    let Some(raw) = raw else {
        return (Some(default_cap), default_source, false, false);
    };
    let trimmed = raw.trim().trim_matches(['"', '\'']).trim();
    if trimmed.is_empty() {
        return (Some(default_cap), default_source, false, false);
    }
    if trimmed.eq_ignore_ascii_case("off")
        || trimmed.eq_ignore_ascii_case("false")
    {
        return (None, CapSource::Note, true, false);
    }
    if let Ok(number) = trimmed.parse::<i64>()
        && (1..=999).contains(&number)
    {
        let cap = u32::try_from(number).unwrap_or(default_cap);
        return (Some(cap), CapSource::Note, false, false);
    }
    (Some(default_cap), default_source, false, true)
}

/// Pure per-note Ready evaluation: no I/O, unit-testable.
///
/// `rows` holds only Ready-lane rows. Rows whose path matches no
/// input note are ignored (daily, untyped, excluded paths). Terminal
/// projects are not capped: they contribute no entry, and a
/// [`LINT_NOTE_READY_IN_TERMINAL_PROJECT`] is emitted once when
/// they still hold counted rows.
pub(crate) fn evaluate(
    notes: &[NoteInput],
    rows: &[LaneRow],
    default_cap: u32,
    default_source: CapSource,
    freshness_available: bool,
) -> Report {
    use std::collections::HashMap;

    let mut by_path: HashMap<&str, Vec<&LaneRow>> = HashMap::new();
    for row in rows {
        by_path.entry(row.path.as_str()).or_default().push(row);
    }

    let mut entries = Vec::new();
    let mut lints = Vec::new();

    for note in notes {
        let empty: Vec<&LaneRow> = Vec::new();
        let note_rows = by_path.get(note.path.as_str()).unwrap_or(&empty);

        let counted_rows: Vec<&&LaneRow> =
            note_rows.iter().filter(|row| row.counted()).collect();
        let count = counted_rows.len() as u32;
        let recurring =
            note_rows.iter().filter(|row| row.recurring_only()).count() as u32;

        // Terminal projects are never capped.
        if !note.is_area && note.is_terminal {
            if count > 0 {
                lints.push(NoteLint {
                    code: LINT_NOTE_READY_IN_TERMINAL_PROJECT.to_string(),
                    path: note.path.clone(),
                    message: format!(
                        "project {} is {} but still holds {count} ready {}",
                        note.name,
                        note.status,
                        if count == 1 { "task" } else { "tasks" },
                    ),
                });
            }
            continue;
        }

        let (cap, source, exempt, invalid) = parse_ready_cap(
            note.ready_cap_raw.as_deref(),
            default_cap,
            default_source,
        );
        if invalid {
            lints.push(NoteLint {
                code: LINT_NOTE_READY_CAP_INVALID.to_string(),
                path: note.path.clone(),
                message: format!(
                    "ready_cap {:?} in {} is not 1–999 or off; using {default_cap}",
                    note.ready_cap_raw.as_deref().unwrap_or(""),
                    note.name,
                ),
            });
        }

        let (state, over_by) = if exempt {
            (NoteState::Exempt, 0)
        } else {
            let cap_value = cap.unwrap_or(default_cap);
            if count > cap_value {
                (NoteState::Crowded, count - cap_value)
            } else if count == cap_value {
                (NoteState::Full, 0)
            } else if count > 0 {
                (NoteState::Room, 0)
            } else {
                (NoteState::Empty, 0)
            }
        };

        let make_up = if freshness_available {
            let mut new = 0;
            let mut rotten = 0;
            for row in &counted_rows {
                match row.bucket.as_deref() {
                    Some("new") => new += 1,
                    Some("rotten") => rotten += 1,
                    _ => {}
                }
            }
            let ready = count.saturating_sub(new).saturating_sub(rotten);
            Some(MakeUp { ready, new, rotten })
        } else {
            None
        };

        entries.push(NoteReady {
            path: note.path.clone(),
            name: note.name.clone(),
            kind: note.kind.clone(),
            status: note.status.clone(),
            parent: note.parent.clone(),
            count,
            cap,
            cap_source: source,
            state,
            over_by,
            make_up,
            recurring,
        });
    }

    entries.sort_by(|a, b| {
        a.state
            .rank()
            .cmp(&b.state.rank())
            .then_with(|| b.over_by.cmp(&a.over_by))
            .then_with(|| b.count.cmp(&a.count))
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
            .then_with(|| a.path.cmp(&b.path))
    });

    let mut totals = Totals {
        notes: 0,
        areas: 0,
        projects: 0,
        crowded: 0,
        full: 0,
        room: 0,
        empty: 0,
        exempt: 0,
        counted: 0,
        excess: 0,
        recurring: 0,
    };
    for entry in &entries {
        totals.recurring += entry.recurring;
        if entry.state == NoteState::Exempt {
            totals.exempt += 1;
            continue;
        }
        totals.notes += 1;
        if entry.kind == "area" {
            totals.areas += 1;
        } else {
            totals.projects += 1;
        }
        totals.counted += entry.count;
        totals.excess += entry.over_by;
        match entry.state {
            NoteState::Crowded => totals.crowded += 1,
            NoteState::Full => totals.full += 1,
            NoteState::Room => totals.room += 1,
            NoteState::Empty => totals.empty += 1,
            NoteState::Exempt => {}
        }
    }

    lints.sort_by(|a, b| a.path.cmp(&b.path).then(a.code.cmp(&b.code)));

    Report {
        notes: entries,
        totals,
        lints,
    }
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
