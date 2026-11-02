use std::path::{Path, PathBuf};

use serde::Serialize;

use super::{HomeResolver, HomeSource, NoteKind, NoteOffClock};
use crate::native::{
    clock_tags::{line_clock_tag, ClockTag},
    collect_done::trailing_block_id_in_line,
    task_root::TaskRootState,
    task_status_hooks::canonical_daily_date,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[allow(dead_code)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ClockSource {
    OnTag,
    OffTag,
    Note,
    InvalidNote,
    Default,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ClockResolution {
    pub(crate) off_clock: bool,
    pub(crate) source: ClockSource,
    pub(crate) note: Option<String>,
}

impl ClockResolution {
    fn tagged(tag: &ClockTag) -> Option<Self> {
        if tag.is_on() {
            Some(Self {
                off_clock: false,
                source: ClockSource::OnTag,
                note: None,
            })
        } else if tag.explicit_off() {
            Some(Self {
                off_clock: true,
                source: ClockSource::OffTag,
                note: None,
            })
        } else {
            None
        }
    }

    fn default() -> Self {
        Self {
            off_clock: false,
            source: ClockSource::Default,
            note: None,
        }
    }

    pub(crate) fn tag_only(tag: &ClockTag) -> Self {
        Self::tagged(tag).unwrap_or_else(Self::default)
    }
}

/// Resolve a task's effective clock from its explicit tag, then the nearest
/// area/project note default on the shared PT13 root walk.
#[allow(dead_code)]
pub(crate) fn effective_clock<S: HomeSource>(
    resolver: &mut HomeResolver<S>,
    residence_path: &Path,
    line: &str,
) -> ClockResolution {
    let tag = line_clock_tag(line);
    if let Some(resolution) = ClockResolution::tagged(&tag) {
        return resolution;
    }

    let root_start = resolver.root_walk_start(residence_path, line);
    if root_start.is_none() {
        lint_invalid_root_if_indexed(resolver, residence_path, line);
    }
    if root_start.is_none() && canonical_daily_date(residence_path).is_some() {
        return ClockResolution::default();
    }
    let start = root_start.as_deref().unwrap_or(residence_path);
    resolve_note_default(resolver, start)
}

fn resolve_note_default<S: HomeSource>(
    resolver: &mut HomeResolver<S>,
    start: &Path,
) -> ClockResolution {
    let home = resolver.resolve_note(start);
    for path_string in home.residence_chain {
        let path = PathBuf::from(&path_string);
        let Some(facts) = resolver.facts(&path) else {
            continue;
        };
        let Some(value) = facts.offclock else {
            continue;
        };
        let raw = match &value {
            NoteOffClock::True => "true",
            NoteOffClock::False => "false",
            NoteOffClock::Invalid(raw) => raw,
        };
        if !matches!(facts.kind, NoteKind::Area | NoteKind::Project) {
            resolver.lint_note_offclock_ignored(&path, raw);
            continue;
        }
        match value {
            NoteOffClock::True => {
                return ClockResolution {
                    off_clock: true,
                    source: ClockSource::Note,
                    note: Some(path_string),
                };
            }
            NoteOffClock::False => {
                return ClockResolution {
                    off_clock: false,
                    source: ClockSource::Note,
                    note: Some(path_string),
                };
            }
            NoteOffClock::Invalid(raw) => {
                resolver.lint_note_offclock_invalid(&path, &raw);
                return ClockResolution {
                    off_clock: false,
                    source: ClockSource::InvalidNote,
                    note: Some(path_string),
                };
            }
        }
    }
    ClockResolution::default()
}

/// A task root that cannot start the walk falls back to its residence and
/// keeps the existing root-field health report. The indexed task-root map
/// supplies the source line number without changing the resolver contract.
fn lint_invalid_root_if_indexed<S: HomeSource>(
    resolver: &mut HomeResolver<S>,
    residence_path: &Path,
    line: &str,
) {
    let Some(block_id) = trailing_block_id_in_line(line) else {
        return;
    };
    let Some(fact) = resolver
        .facts(residence_path)
        .and_then(|facts| facts.task_roots.get(&block_id).cloned())
    else {
        return;
    };
    if matches!(fact.state, TaskRootState::None) {
        return;
    }
    let _ = resolver.home_for_task(residence_path, fact.line_number, line);
}
