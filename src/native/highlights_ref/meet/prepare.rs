//! Read-only meeting-note filing preparation.
//!
//! This module resolves a meeting, notes, parent, identity, and destinations
//! into one immutable input for later write routes. It does not acquire
//! locks, create files, render PDFs, or launch subprocesses.

use std::{
    collections::BTreeSet,
    ffi::OsStr,
    fs,
    path::{Path, PathBuf},
};

use chrono::{Datelike, NaiveDate, NaiveDateTime};
use sha2::{Digest, Sha256};

use crate::native::{
    collect_done::{block_ids_in_markdown, ArchivePairIndex, SourceLookup},
    config as bob_config,
    highlights_ref::{
        recapture::preflight_for_meeting_note,
        stamp::{
            plan_default_target, DefaultTargetIdentity, TargetPlan,
            TargetWorkflow,
        },
        Config,
    },
    meetings::{
        auto_match, collect_window_rows, content_words, event_ref_for,
        filed_notes_index, meeting_parent, normalize, parse_event_token,
        read_notes, recent_meetings, resolve_event, AutoMatch, EventToken,
        FiledNotes, MeetingNotes, MeetingParent, MeetingRow, NotesError,
        NotesInput, NotesSource, RecentMeeting, ResolveError,
    },
    parent_notes::{
        parent_candidates, resolve_parent, ParentError, ResolvedParent,
    },
    ref_tasks::allocate_ref_block_id,
    task_dependencies::format::canonical_link,
    task_root::{parse_task_root, TaskRootState},
};

use super::model::{
    EffectiveMeetingsConfig, ExistingReferenceSelection, GuardedRootEdit,
    MatchEvidence, MatchMethod, MeetingCandidate, MeetingListReport,
    MeetingRowReport, ParentChoice, PrepareError, PreparedFiling,
    PreparedParent, TranscriptDisposition,
};

/// How the caller selected the event. A picker feeds its selected EVENT
/// token and the candidate evidence back through the same validation path.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) enum MeetingSelection {
    #[default]
    Auto,
    Token(String),
    Picked {
        token: String,
        evidence: Vec<String>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct MeetingPrepareOptions {
    pub selection: MeetingSelection,
    pub day: Option<NaiveDate>,
    pub window_days: Option<u32>,
    pub parent: Option<String>,
    pub producer: Option<String>,
    pub status: Option<String>,
    pub title: Option<String>,
    pub include_transcript: bool,
    pub overwrite: bool,
    pub expected_sha256: Option<String>,
    pub edited: bool,
}

/// Prepare a new filing or replacement using the exact notes snapshot the
/// caller read. Hash validation happens before resolution or any expensive
/// work, and later routes receive this same text and original-byte digest.
pub(crate) fn prepare_filing(
    config: &Config,
    input: &NotesInput,
    options: &MeetingPrepareOptions,
) -> Result<PreparedFiling, PrepareError> {
    if let Some(expected) = options.expected_sha256.as_deref()
        && !expected.eq_ignore_ascii_case(&input.sha256)
    {
        return Err(PrepareError::new(
            "notes_changed",
            "meeting notes changed after they were selected",
            1,
        )
        .with_hint("read the notes again and retry"));
    }

    let base_config = load_meetings_config()?;
    let window_days =
        effective_window(options.window_days, base_config.window_days)?;
    let now = crate::native::env::current_datetime();
    let (meeting, match_evidence) = choose_meeting(
        &config.bob_dir,
        &options.selection,
        &input.text,
        window_days,
        options.day,
        now,
    )?;
    let block_id = meeting.block_id.clone().ok_or_else(|| {
        PrepareError::new(
            "no_meeting",
            format!(
                "`{}` has no `^id`; notes link to the meeting by id",
                meeting.title
            ),
            2,
        )
        .with_hint("add a block id to the Event Task, for example `^planning`")
    })?;

    let mut warnings = Vec::new();
    if meeting.status == '-' {
        warnings.push(format!(
            "the selected event `{}` is cancelled",
            meeting.title
        ));
    }
    if meeting.starts_at > now {
        warnings.push(format!(
            "the selected event `{}` has not started",
            meeting.title
        ));
    }

    let filed = find_filed_reference_with_config(config, &meeting);
    if let Some(filed) = filed.as_ref()
        && !options.overwrite
    {
        let mut error = PrepareError::new(
            "already_filed",
            format!("meeting notes are already filed as {}", filed.note),
            2,
        );
        error.existing = Some(filed.clone());
        error.hint = Some(
            "pass --overwrite to replace this occurrence's notes".to_string(),
        );
        return Err(error);
    }

    let existing = match filed {
        Some(filed) => {
            let Some(overwrite) =
                preflight_for_meeting_note(config, &filed.note)
            else {
                let mut error = PrepareError::new(
                    "overwrite_unavailable",
                    format!(
                        "{} is filed for this meeting but is not a live indexed meetings reference",
                        filed.note
                    ),
                    1,
                );
                error.existing = Some(filed);
                error.hint = Some(
                    "run `bob ref scan` to repair the reference index before replacing it"
                        .to_string(),
                );
                return Err(error);
            };
            Some(ExistingReferenceSelection { filed, overwrite })
        }
        None => None,
    };

    let parent = select_parent(
        &config.bob_dir,
        &meeting,
        options.parent.as_deref(),
        existing.as_ref(),
    )?;
    let relation_label =
        crate::native::highlights_ref::meeting_birth::meeting_relation_label()
            .map_err(|error| {
                PrepareError::new("invalid_config", error.to_string(), 1)
            })?;
    let producer = match options.producer.as_deref() {
        Some(value) => {
            let trimmed = value.trim();
            if trimmed.is_empty() {
                return Err(PrepareError::new(
                    "invalid_producer",
                    "producer cannot be empty",
                    1,
                ));
            }
            Some(trimmed.to_string())
        }
        None => base_config.producer.clone(),
    };
    let status_explicit = options.status.is_some();
    let status = if let Some(status) = options.status.as_deref() {
        normalize_status(status)?
    } else if let Some(existing) = existing.as_ref() {
        status_from_mark(&existing.filed.status)
            .unwrap_or_else(|| base_config.status.clone())
    } else {
        base_config.status.clone()
    };
    let title_explicit = options
        .title
        .as_deref()
        .is_some_and(|title| !title.trim().is_empty());
    let title = match options.title.as_deref() {
        Some(title) if !title.trim().is_empty() => title.trim().to_string(),
        Some(_) => {
            return Err(PrepareError::new(
                "invalid_title",
                "meeting title cannot be empty",
                1,
            ));
        }
        None => {
            if let Some(existing) = existing.as_ref() {
                live_reference_title(config, existing)
                    .unwrap_or_else(|| derived_title(&meeting))
            } else {
                derived_title(&meeting)
            }
        }
    };

    let (mut notes, transcript) =
        prepared_notes(&input.text, options.include_transcript);
    warnings.extend(notes.warnings.iter().cloned());

    let (stem, target, note_path) = match existing.as_ref() {
        Some(existing) => {
            let selected =
                existing.overwrite.selection.family.selected_member();
            let path = PathBuf::from(&selected.path);
            let stem = path
                .file_stem()
                .and_then(OsStr::to_str)
                .ok_or_else(|| {
                    PrepareError::new(
                        "overwrite_unavailable",
                        format!(
                            "could not determine the stem for {}",
                            selected.path
                        ),
                        1,
                    )
                })?
                .to_string();
            let pdf = existing
                .overwrite
                .source_pdf
                .as_deref()
                .map(|path| config.bob_dir.join(path))
                .unwrap_or_else(|| {
                    config.lib_dir.join("meetings").join(format!("{stem}.pdf"))
                });
            let note_path = config.bob_dir.join(&path);
            let target = TargetPlan {
                target: pdf,
                sidecar: note_path.clone(),
                workflow: TargetWorkflow::Library,
                stem: stem.clone(),
                renamed_from: None,
            };
            (stem, target, note_path)
        }
        None => {
            let base_stem = derive_meeting_stem(&meeting);
            let target = plan_default_target(
                config,
                OsStr::new(&base_stem),
                "meetings",
                false,
                &DefaultTargetIdentity::AlwaysDifferent,
            )
            .map_err(|error| {
                PrepareError::new("target_unavailable", error.to_string(), 1)
            })?;
            let note_path = config
                .ref_dir
                .join("meetings")
                .join(format!("{}.md", target.stem));
            (target.stem.clone(), target, note_path)
        }
    };

    let task_block_id = if let Some(existing) = existing.as_ref() {
        existing
            .filed
            .task
            .rsplit_once("#^")
            .map(|(_, id)| id.to_string())
            .filter(|id| !id.is_empty())
            .unwrap_or(preview_task_block_id(config, &parent.route, &stem)?)
    } else {
        preview_task_block_id(config, &parent.route, &stem)?
    };

    let guarded_root_edit = if options.parent.is_some()
        && (meeting.root.is_none() || meeting.root_is_inbox)
    {
        Some(guard_root_edit(config, &meeting, &block_id, &parent.route)?)
    } else {
        if options.parent.is_some()
            && meeting.root.is_some()
            && !meeting.root_is_inbox
            && resolve_parent(
                &config.bob_dir,
                meeting.root.as_deref().unwrap_or(""),
            )
            .is_err()
        {
            warnings.push(
                "the event root was not changed because its existing value is not a missing or inbox root"
                    .to_string(),
            );
        }
        None
    };

    if !options.include_transcript {
        notes.transcript = None;
    }
    let effective_config = EffectiveMeetingsConfig {
        producer,
        relation_label,
        status: status.clone(),
        window_days,
    };

    // The event relation is rendered relative to the task's destination,
    // matching the first-filing route and its later birth reconciliation.
    let relation_target =
        canonical_event_link(config, &meeting, &block_id, &parent.route);

    Ok(PreparedFiling {
        vault_dir: config.bob_dir.clone(),
        meeting,
        notes,
        input_sha256: input.sha256.clone(),
        match_evidence,
        parent,
        guarded_root_edit,
        effective_config,
        status,
        status_explicit,
        title,
        title_explicit,
        stem,
        target,
        note_path,
        task_block_id,
        relation_target,
        existing,
        include_transcript: options.include_transcript,
        transcript,
        edited: options.edited,
        warnings,
    })
}

/// Read one source once and then prepare from its exact-byte digest.
pub(crate) fn prepare_filing_from_source(
    config: &Config,
    source: NotesSource<'_>,
    options: &MeetingPrepareOptions,
) -> Result<PreparedFiling, PrepareError> {
    let input = read_notes(source).map_err(notes_error)?;
    prepare_filing(config, &input, options)
}

/// Prepare list rows without touching clipboard/stdin or creating locks.
pub(crate) fn prepare_list(
    config: &Config,
    window_days: Option<u32>,
    day: Option<NaiveDate>,
) -> Result<MeetingListReport, PrepareError> {
    let base_config = load_meetings_config()?;
    let window_days = effective_window(window_days, base_config.window_days)?;
    let now = crate::native::env::current_datetime();
    let mut rows: Vec<RecentMeeting> = if day.is_none() {
        recent_meetings(&config.bob_dir, window_days, now)
    } else {
        let filed = filed_notes_index(&config.bob_dir);
        collect_window_rows(&config.bob_dir, window_days, day)
            .into_iter()
            .filter(|row| row.starts_at <= now && row.status != '-')
            .map(|meeting| {
                let notes = filed_for_meeting(&filed, &meeting);
                let event_ref = meeting
                    .block_id
                    .as_deref()
                    .and_then(|id| event_ref_for(&meeting.path, id));
                RecentMeeting {
                    meeting,
                    notes,
                    event_ref,
                }
            })
            .collect()
    };
    if let Some(day) = day {
        rows.retain(|row| row.meeting.date == day);
    }
    rows.sort_by(|left, right| {
        (
            right.meeting.date,
            right.meeting.starts_at,
            right.meeting.line,
        )
            .cmp(&(
                left.meeting.date,
                left.meeting.starts_at,
                left.meeting.line,
            ))
    });
    Ok(MeetingListReport {
        meetings: rows
            .into_iter()
            .map(|row| MeetingRowReport::from_row(&row.meeting, row.notes))
            .collect(),
    })
}

fn load_meetings_config(
) -> Result<crate::native::config::MeetingsConfig, PrepareError> {
    let path = bob_config::config_path();
    bob_config::load_meetings_config(&path).map_err(|error| {
        PrepareError::new("invalid_config", error.to_string(), 1)
    })
}

fn effective_window(
    requested: Option<u32>,
    configured: u32,
) -> Result<u32, PrepareError> {
    let days = requested.unwrap_or(configured);
    if (1..=365).contains(&days) {
        Ok(days)
    } else {
        Err(PrepareError::new(
            "invalid_window",
            "window must be an integer from 1 to 365 days",
            1,
        ))
    }
}

fn normalize_status(status: &str) -> Result<String, PrepareError> {
    let normalized = status.trim().to_ascii_lowercase();
    if ["next", "ready", "wip", "read"].contains(&normalized.as_str()) {
        Ok(normalized)
    } else {
        Err(PrepareError::new(
            "invalid_status",
            format!(
                "status must be next, ready, wip, or read (got {status:?})"
            ),
            1,
        ))
    }
}

fn status_from_mark(mark: &str) -> Option<String> {
    match mark {
        "*" => Some("next".to_string()),
        " " => Some("ready".to_string()),
        "/" => Some("wip".to_string()),
        "x" | "X" => Some("read".to_string()),
        _ => None,
    }
}

fn choose_meeting(
    vault: &Path,
    selection: &MeetingSelection,
    notes: &str,
    window_days: u32,
    day: Option<NaiveDate>,
    now: NaiveDateTime,
) -> Result<(MeetingRow, MatchEvidence), PrepareError> {
    match selection {
        MeetingSelection::Auto => {
            let rows: Vec<MeetingRow> =
                collect_window_rows(vault, window_days, day)
                    .into_iter()
                    .filter(|row| row.starts_at <= now && row.status != '-')
                    .collect();
            match auto_match(notes, &rows) {
                AutoMatch::Selected {
                    row_index,
                    evidence,
                    ..
                } => {
                    let meeting = rows.get(row_index).cloned().ok_or_else(|| {
                        PrepareError::new(
                            "no_meeting",
                            "automatic meeting selection returned no candidate",
                            2,
                        )
                    })?;
                    Ok((
                        meeting,
                        MatchEvidence {
                            method: MatchMethod::Auto,
                            evidence: vec![evidence],
                        },
                    ))
                }
                AutoMatch::Unresolved { ranked } if ranked.is_empty() => {
                    Err(PrepareError::new(
                        "no_meeting",
                        "no started, non-cancelled meetings are available in this window",
                        2,
                    )
                    .with_hint("use --day, --window, or an explicit EVENT token"))
                }
                AutoMatch::Unresolved { ranked } => {
                    let candidates = ranked
                        .into_iter()
                        .filter_map(|candidate| {
                            let meeting = rows.get(candidate.row_index)?.clone();
                            Some(MeetingCandidate {
                                detail: candidate_detail(&meeting),
                                meeting,
                                evidence: vec![candidate.evidence],
                            })
                        })
                        .collect();
                    Err(PrepareError {
                        code: "ambiguous_meeting".to_string(),
                        message: "notes do not identify one meeting strongly enough".to_string(),
                        hint: Some("choose one of the evidenced meetings".to_string()),
                        root: None,
                        candidates,
                        parent_candidates: Vec::new(),
                        existing: None,
                        exit_code: 2,
                    })
                }
            }
        }
        MeetingSelection::Token(token) => {
            resolve_token(vault, token, window_days, day, None)
        }
        MeetingSelection::Picked { token, evidence } => {
            resolve_token(vault, token, window_days, day, Some(evidence))
        }
    }
}

fn resolve_token(
    vault: &Path,
    raw: &str,
    window_days: u32,
    day: Option<NaiveDate>,
    picked_evidence: Option<&[String]>,
) -> Result<(MeetingRow, MatchEvidence), PrepareError> {
    let token = parse_event_token(raw);
    let method = if picked_evidence.is_some() {
        MatchMethod::Picked
    } else {
        match &token {
            EventToken::Explicit { .. } | EventToken::Url { .. } => {
                MatchMethod::Explicit
            }
            EventToken::Query { .. } => MatchMethod::Query,
        }
    };
    let meeting = resolve_event(vault, &token, window_days, day)
        .map_err(resolve_error)?;
    let evidence = if let Some(evidence) = picked_evidence {
        if evidence.is_empty() {
            vec![candidate_detail(&meeting)]
        } else {
            evidence.to_vec()
        }
    } else {
        match &token {
            EventToken::Explicit { .. } => {
                vec![format!(
                    "explicit EVENT token resolved to {}",
                    candidate_detail(&meeting)
                )]
            }
            EventToken::Url { .. } => vec![format!(
                "Google event identity {} resolved to {}",
                meeting.event_id.as_deref().unwrap_or("unknown"),
                candidate_detail(&meeting)
            )],
            EventToken::Query { words } => vec![format!(
                "title query '{}' matched {}",
                words.join(" "),
                meeting.title
            )],
        }
    };
    Ok((meeting, MatchEvidence { method, evidence }))
}

fn resolve_error(error: ResolveError) -> PrepareError {
    match error {
        ResolveError::NoMeeting { message, hint } => {
            PrepareError::new("no_meeting", message, 2).with_hint(hint)
        }
        ResolveError::MissingId { message, hint } => {
            PrepareError::new("no_meeting", message, 2).with_hint(hint)
        }
        ResolveError::Ambiguous {
            message,
            candidates,
        } => PrepareError {
            code: "ambiguous_meeting".to_string(),
            message,
            hint: Some("choose one of the evidenced meetings".to_string()),
            root: None,
            candidates: candidates
                .into_iter()
                .map(|candidate| MeetingCandidate {
                    meeting: candidate.meeting,
                    evidence: vec![candidate.detail.clone()],
                    detail: candidate.detail,
                })
                .collect(),
            parent_candidates: Vec::new(),
            existing: None,
            exit_code: 2,
        },
    }
}

fn select_parent(
    vault: &Path,
    meeting: &MeetingRow,
    explicit: Option<&str>,
    existing: Option<&ExistingReferenceSelection>,
) -> Result<PreparedParent, PrepareError> {
    if explicit.is_none()
        && let Some(existing) = existing
        && let Some(route) = existing
            .overwrite
            .selection
            .defaults
            .parent
            .as_deref()
            .or_else(|| locator_parent(&existing.filed.task))
    {
        return Ok(existing_parent(vault, route));
    }

    match meeting_parent(vault, meeting, explicit) {
        Ok(MeetingParent::Ready(parent)) => {
            Ok(parent_from_resolved(parent, explicit.is_some()))
        }
        Ok(MeetingParent::NeedsParent { root, candidates }) => {
            Err(parent_required(
                root,
                "meeting has no eligible filing parent",
                candidates.into_iter().map(ParentChoice::from).collect(),
            ))
        }
        Err(error) => Err(parent_error(vault, meeting, error)),
    }
}

fn parent_from_resolved(
    parent: ResolvedParent,
    explicit: bool,
) -> PreparedParent {
    let kind = parent.kind_name().to_string();
    PreparedParent {
        route: parent.route,
        kind,
        label: parent.label,
        explicit,
    }
}

fn existing_parent(vault: &Path, route: &str) -> PreparedParent {
    if let Ok(parent) = resolve_parent(vault, route) {
        return parent_from_resolved(parent, false);
    }
    let path = Path::new(route);
    let label = path
        .file_name()
        .and_then(OsStr::to_str)
        .unwrap_or(route)
        .to_string();
    let kind = if route.starts_with("done/") {
        "archive"
    } else {
        "existing"
    };
    PreparedParent {
        route: route.trim_end_matches(".md").to_string(),
        kind: kind.to_string(),
        label,
        explicit: false,
    }
}

fn parent_error(
    vault: &Path,
    meeting: &MeetingRow,
    error: ParentError,
) -> PrepareError {
    let candidates = parent_candidates(vault)
        .into_iter()
        .map(ParentChoice::from)
        .collect();
    let mut required =
        parent_required(meeting.root.clone(), error.message(), candidates);
    required.hint =
        Some("choose a valid area or project with --parent".to_string());
    required
}

fn parent_required(
    root: Option<String>,
    message: impl Into<String>,
    candidates: Vec<ParentChoice>,
) -> PrepareError {
    PrepareError {
        code: "needs_parent".to_string(),
        message: message.into(),
        hint: Some("choose a filing parent with --parent".to_string()),
        root,
        candidates: Vec::new(),
        parent_candidates: candidates,
        existing: None,
        exit_code: 2,
    }
}

fn locator_parent(task: &str) -> Option<&str> {
    task.rsplit_once("#^")
        .map(|(parent, _)| parent)
        .filter(|parent| !parent.is_empty())
}

fn filed_for_meeting(
    filed: &crate::native::meetings::FiledNotesIndex,
    meeting: &MeetingRow,
) -> Option<FiledNotes> {
    meeting
        .event_id
        .as_deref()
        .and_then(|event_id| filed.by_event_id.get(event_id))
        .or_else(|| {
            meeting.block_id.as_ref().and_then(|id| {
                filed.by_target.get(&(meeting.path.clone(), id.clone()))
            })
        })
        .cloned()
}

fn prepared_notes(
    text: &str,
    include_transcript: bool,
) -> (MeetingNotes, TranscriptDisposition) {
    let mut notes = normalize(text);
    let transcript_present = notes
        .transcript
        .as_deref()
        .is_some_and(|transcript| !transcript.trim().is_empty());
    let disposition = if !transcript_present {
        TranscriptDisposition::None
    } else if include_transcript {
        TranscriptDisposition::Kept
    } else {
        TranscriptDisposition::Omitted
    };
    if !include_transcript {
        notes.transcript = None;
    }
    (notes, disposition)
}

fn derive_meeting_stem(meeting: &MeetingRow) -> String {
    let words: Vec<String> = content_words(&meeting.title)
        .into_iter()
        .filter_map(|word| {
            let ascii: String = word
                .chars()
                .filter(|character| character.is_ascii_alphanumeric())
                .flat_map(char::to_lowercase)
                .collect();
            (!ascii.is_empty()).then_some(ascii)
        })
        .take(6)
        .collect();
    let tail = if words.is_empty() {
        "meeting".to_string()
    } else {
        words.join("_")
    };
    format!("{}_{}", meeting.date.format("%y%m%d"), tail)
}

fn live_reference_title(
    config: &Config,
    existing: &ExistingReferenceSelection,
) -> Option<String> {
    let path = config.bob_dir.join(&existing.filed.note);
    let note = super::super::frontmatter::read_note(&path).ok()?;
    if let Some(title) = note
        .frontmatter_value("title")
        .and_then(|value| value.as_string().map(str::to_string))
    {
        let trimmed = title.trim();
        if !trimmed.is_empty() {
            return Some(trimmed.to_string());
        }
    }
    for line in note.body.lines() {
        let trimmed = line.trim();
        if let Some(heading) = trimmed.strip_prefix("# ") {
            let heading = heading.trim();
            if !heading.is_empty() {
                return Some(heading.to_string());
            }
        }
    }
    None
}

fn derived_title(meeting: &MeetingRow) -> String {
    format!(
        "{} — Meeting Notes ({} {}, {}–{})",
        meeting.title,
        weekday_short(meeting.date),
        meeting.date.format("%Y-%m-%d"),
        meeting.starts_at.format("%H:%M"),
        meeting.ends_at.format("%H:%M"),
    )
}

fn weekday_short(date: NaiveDate) -> &'static str {
    match date.weekday() {
        chrono::Weekday::Mon => "Mon",
        chrono::Weekday::Tue => "Tue",
        chrono::Weekday::Wed => "Wed",
        chrono::Weekday::Thu => "Thu",
        chrono::Weekday::Fri => "Fri",
        chrono::Weekday::Sat => "Sat",
        chrono::Weekday::Sun => "Sun",
    }
}

fn preview_task_block_id(
    config: &Config,
    parent_route: &str,
    stem: &str,
) -> Result<String, PrepareError> {
    let destination = PathBuf::from(format!("{parent_route}.md"));
    let destination_abs = config.bob_dir.join(&destination);
    let destination_contents = match fs::read_to_string(&destination_abs) {
        Ok(contents) => contents,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            String::new()
        }
        Err(error) => {
            return Err(PrepareError::new(
                "task_destination_unreadable",
                format!(
                    "could not read {}: {error}",
                    destination_abs.display()
                ),
                1,
            ));
        }
    };
    let pairs =
        ArchivePairIndex::read_vault(&config.bob_dir).map_err(|error| {
            PrepareError::new(
                "task_destination_unreadable",
                format!("could not inspect task archives: {error}"),
                1,
            )
        })?;
    let pairs = pairs.with_contents(&destination, &destination_contents);
    let archive = match pairs.for_source(&destination) {
        SourceLookup::Bound(pair) => {
            read_archive_contents(config, &pair.archive)?
        }
        SourceLookup::Missing { canonical_archive } => {
            if let Some(path) = canonical_archive {
                read_optional_archive(config, &path)?
            } else {
                None
            }
        }
        SourceLookup::Refused {
            reason, related, ..
        } => {
            return Err(PrepareError::new(
                "task_destination_unavailable",
                format!(
                    "cannot safely plan a task id for {}: {reason} ({})",
                    destination_abs.display(),
                    related
                        .iter()
                        .map(|path| path.display().to_string())
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
                1,
            ));
        }
    };
    let mut taken: BTreeSet<String> =
        block_ids_in_markdown(&destination_contents)
            .into_iter()
            .collect();
    if let Some(archive) = archive {
        taken.extend(block_ids_in_markdown(&archive));
    }
    Ok(allocate_ref_block_id(stem, &|id| taken.contains(id)))
}

fn read_archive_contents(
    config: &Config,
    relative: &Path,
) -> Result<Option<String>, PrepareError> {
    read_optional_archive(config, relative)
}

fn read_optional_archive(
    config: &Config,
    relative: &Path,
) -> Result<Option<String>, PrepareError> {
    let path = config.bob_dir.join(relative);
    match fs::read_to_string(&path) {
        Ok(contents) => Ok(Some(contents)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(PrepareError::new(
            "task_destination_unavailable",
            format!("could not read {}: {error}", path.display()),
            1,
        )),
    }
}

fn guard_root_edit(
    config: &Config,
    meeting: &MeetingRow,
    block_id: &str,
    destination: &str,
) -> Result<GuardedRootEdit, PrepareError> {
    let path = config.bob_dir.join(&meeting.path);
    let bytes = fs::read(&path).map_err(|error| {
        PrepareError::new(
            "event_changed",
            format!(
                "could not read the event note {}: {error}",
                path.display()
            ),
            1,
        )
    })?;
    let text = String::from_utf8(bytes.clone()).map_err(|_| {
        PrepareError::new(
            "event_changed",
            format!("the event note {} is not valid UTF-8", path.display()),
            1,
        )
    })?;
    let expected_line = text
        .lines()
        .nth(meeting.line.saturating_sub(1))
        .ok_or_else(|| {
            PrepareError::new(
                "event_changed",
                "the selected Event Task moved while it was being prepared",
                1,
            )
        })?;
    match parse_task_root(expected_line) {
        TaskRootState::None => {}
        TaskRootState::Set { .. } if meeting.root_is_inbox => {}
        TaskRootState::Set { .. } => {
            return Err(PrepareError::new(
                "event_changed",
                "the selected event root changed while it was being prepared",
                1,
            ));
        }
        TaskRootState::Repair { .. } => {
            return Err(PrepareError::new(
                "event_root_invalid",
                "the event has a malformed root field and cannot be re-rooted safely",
                1,
            ));
        }
    }
    Ok(GuardedRootEdit {
        path: meeting.path.clone(),
        line: meeting.line,
        block_id: block_id.to_string(),
        event_id: meeting.event_id.clone(),
        from: meeting.root.clone(),
        to: destination.to_string(),
        expected_line: expected_line.to_string(),
        expected_file_sha256: hex::encode(Sha256::digest(&bytes)),
    })
}

fn canonical_event_link(
    config: &Config,
    meeting: &MeetingRow,
    block_id: &str,
    parent_route: &str,
) -> String {
    let index = crate::native::meetings::vault_index(&config.bob_dir);
    canonical_link(
        &meeting.path,
        block_id,
        &PathBuf::from(format!("{parent_route}.md")),
        &index,
    )
}

fn find_filed_reference_with_config(
    config: &Config,
    meeting: &MeetingRow,
) -> Option<FiledNotes> {
    let filed = filed_notes_index(&config.bob_dir);
    filed_for_meeting(&filed, meeting)
}

fn notes_error(error: NotesError) -> PrepareError {
    PrepareError::new(error.code(), error.to_string(), 1)
}

fn candidate_detail(meeting: &MeetingRow) -> String {
    match meeting
        .block_id
        .as_deref()
        .and_then(|id| event_ref_for(&meeting.path, id))
    {
        Some(event_ref) => {
            format!("{event_ref} · {} {}", meeting.time_range, meeting.title)
        }
        None => format!("{} {} (no ^id)", meeting.date, meeting.title),
    }
}
