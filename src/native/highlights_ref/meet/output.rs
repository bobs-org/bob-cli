//! Stable JSON and human reports for internal `ref meet` preparations.

use serde::Serialize;

use crate::native::{
    highlights_ref::{
        meet::model::{
            FilingReport, MatchReport, MeetingListReport, MeetingNotesLink,
            MeetingPrepareReport, MeetingRowReport, NotesReport, ParentChoice,
            RerootReport, TaskPlanReport,
        },
        stamp::TargetWorkflow,
    },
    ref_library::output::{generated_at, REF_SCHEMA_VERSION},
};

use super::model::{MeetingCandidate, PrepareError, PreparedFiling};

#[derive(Serialize)]
struct SuccessEnvelope<'a, T> {
    ok: bool,
    schema_version: u32,
    command: &'static str,
    generated_at: String,
    #[serde(flatten)]
    payload: &'a T,
}

#[derive(Serialize)]
struct ErrorEnvelope<'a> {
    ok: bool,
    schema_version: u32,
    command: &'static str,
    generated_at: String,
    error: ErrorBody<'a>,
    #[serde(skip_serializing_if = "Option::is_none")]
    root: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    candidates: Option<Vec<CandidateReport>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    parent_candidates: Option<&'a [ParentChoice]>,
    #[serde(skip_serializing_if = "Option::is_none")]
    existing: Option<MeetingNotesLink>,
}

#[derive(Serialize)]
struct ErrorBody<'a> {
    code: &'a str,
    message: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    hint: Option<&'a str>,
}

#[derive(Serialize)]
struct CandidateReport {
    meeting: MeetingRowReport,
    detail: String,
    evidence: Vec<String>,
}

/// Render the typed preparation as the public success payload, without the
/// versioned envelope. The later CLI adds its final outcome/commit/queue data.
pub(crate) fn prepare_report(
    prepared: &PreparedFiling,
) -> MeetingPrepareReport {
    let meeting = MeetingRowReport::from_row(
        &prepared.meeting,
        prepared
            .existing
            .as_ref()
            .map(|existing| existing.filed.clone()),
    );
    let pdf_path = match &prepared.target.workflow {
        TargetWorkflow::Intake {
            library_destination,
        } => library_destination,
        TargetWorkflow::Library | TargetWorkflow::External => {
            &prepared.target.target
        }
    };
    let reroot = prepared
        .guarded_root_edit
        .as_ref()
        .map(|edit| RerootReport {
            from: edit.from.clone(),
            to: edit.to.clone(),
        });
    MeetingPrepareReport {
        outcome: "would_file",
        meeting,
        match_report: MatchReport {
            method: prepared.match_evidence.method,
            evidence: prepared.match_evidence.evidence.clone(),
        },
        filing: FilingReport {
            parent: prepared.parent.route.clone(),
            parent_kind: prepared.parent.kind.clone(),
            reroot,
            title: prepared.title.clone(),
            stem: prepared.stem.clone(),
            note: display_vault_path(&prepared.note_path, &prepared.vault_dir),
            pdf: display_vault_path(pdf_path, &prepared.vault_dir),
            task: TaskPlanReport {
                destination: prepared.parent.route.clone(),
                block_id: prepared.task_block_id.clone(),
                status: prepared.status.clone(),
            },
            relation: super::model::RelationReport {
                label: prepared.effective_config.relation_label.clone(),
                target: prepared.relation_target.clone(),
            },
        },
        notes: NotesReport::from_prepared(prepared),
        warnings: prepared.warnings.clone(),
    }
}

/// Serialize a filing preview with the shared versioned ref envelope.
pub(crate) fn prepare_json(prepared: &PreparedFiling) -> String {
    let report = prepare_report(prepared);
    serialize(&SuccessEnvelope {
        ok: true,
        schema_version: REF_SCHEMA_VERSION,
        command: "ref meet",
        generated_at: generated_at(),
        payload: &report,
    })
}

/// Serialize a list with the shared versioned ref envelope.
pub(crate) fn list_json(list: &MeetingListReport) -> String {
    serialize(&SuccessEnvelope {
        ok: true,
        schema_version: REF_SCHEMA_VERSION,
        command: "ref meet",
        generated_at: generated_at(),
        payload: list,
    })
}

/// Serialize a typed preparation failure with its selection context.
pub(crate) fn error_json(error: &PrepareError) -> String {
    let candidates = (!error.candidates.is_empty())
        .then(|| error.candidates.iter().map(candidate_report).collect());
    let parents = (!error.parent_candidates.is_empty())
        .then_some(error.parent_candidates.as_slice());
    let envelope = ErrorEnvelope {
        ok: false,
        schema_version: REF_SCHEMA_VERSION,
        command: "ref meet",
        generated_at: generated_at(),
        error: ErrorBody {
            code: &error.code,
            message: &error.message,
            hint: error.hint.as_deref(),
        },
        root: error.root.as_deref(),
        candidates,
        parent_candidates: parents,
        existing: error.existing.clone().map(MeetingNotesLink::from),
    };
    serialize(&envelope)
}

/// Human preview for the same data used by JSON clients.
pub(crate) fn prepare_human(prepared: &PreparedFiling) -> String {
    let report = prepare_report(prepared);
    let mut lines = vec!["Would file meeting notes".to_string()];
    lines.push(format!(
        "meeting   {} {}–{} · {}",
        report.meeting.date,
        report.meeting.start,
        report.meeting.end,
        report.meeting.title
    ));
    if let Some(event_ref) = report.meeting.event_ref.as_deref() {
        lines.push(format!("EVENT     {event_ref}"));
    }
    lines.push(format!(
        "match     {:?} · {}",
        report.match_report.method,
        report.match_report.evidence.join("; ")
    ));
    lines.push(format!(
        "parent    {} ({})",
        report.filing.parent, report.filing.parent_kind
    ));
    if let Some(reroot) = report.filing.reroot.as_ref() {
        lines.push(format!(
            "event root {} → {}",
            reroot.from.as_deref().unwrap_or("<missing>"),
            reroot.to
        ));
    }
    lines.push(format!("title     {}", report.filing.title));
    lines.push(format!("note      {}", report.filing.note));
    lines.push(format!("pdf       {}", report.filing.pdf));
    lines.push(format!(
        "task      {}#^{} ({})",
        report.filing.task.destination,
        report.filing.task.block_id,
        report.filing.task.status
    ));
    lines.push(format!(
        "relation  {} {}",
        report.filing.relation.label, report.filing.relation.target
    ));
    lines.push(format!(
        "notes     {:?} · sha256 {}",
        report.notes.shape, report.notes.sha256
    ));
    if !report.notes.outline.tldr.is_empty() {
        lines.push(format!("  TL;DR   {}", report.notes.outline.tldr));
    }
    lines.push(format!(
        "  next steps: {} · sections: {} · transcript: {}",
        report.notes.outline.next_steps.len(),
        report.notes.outline.sections.len(),
        transcript_label(report.notes.transcript)
    ));
    for section in &report.notes.outline.sections {
        lines.push(format!(
            "  {}: {} item(s)",
            section.heading.as_deref().unwrap_or("Notes"),
            section.items
        ));
    }
    for warning in &report.warnings {
        lines.push(format!("warning   {warning}"));
    }
    lines.join("\n")
}

/// Human list report. Rows are newest first and the header counts meetings
/// that still need notes.
pub(crate) fn list_human(list: &MeetingListReport) -> String {
    let missing = list
        .meetings
        .iter()
        .filter(|row| row.notes.is_none())
        .count();
    let mut lines = vec![format!("Recent meetings · {missing} need notes")];
    for row in &list.meetings {
        lines.push(format!(
            "{} {}–{} · {}",
            row.date, row.start, row.end, row.title
        ));
        if let Some(event_ref) = row.event_ref.as_deref() {
            lines.push(format!("  EVENT {event_ref}"));
        }
        lines.push(format!(
            "  root {} · {}",
            row.root.as_deref().unwrap_or("<missing>"),
            row.notes
                .as_ref()
                .map(|notes| format!("filed {}", notes.note))
                .unwrap_or_else(|| "missing".to_string())
        ));
    }
    lines.join("\n")
}

/// Human refusal with the same evidence/candidates as the JSON envelope.
pub(crate) fn error_human(error: &PrepareError) -> String {
    let mut lines = vec![format!("error: {}", error.message)];
    if let Some(hint) = error.hint.as_deref() {
        lines.push(format!("hint: {hint}"));
    }
    if let Some(root) = error.root.as_deref() {
        lines.push(format!("root: {root}"));
    }
    for candidate in &error.candidates {
        lines.push(format!("  {}", candidate.detail));
        for evidence in &candidate.evidence {
            lines.push(format!("    {evidence}"));
        }
    }
    for parent in &error.parent_candidates {
        lines.push(format!(
            "  {} · {} · {}",
            parent.route, parent.kind, parent.label
        ));
    }
    if let Some(existing) = error.existing.as_ref() {
        lines.push(format!("existing: {} · {}", existing.note, existing.task));
    }
    lines.join("\n")
}

fn candidate_report(candidate: &MeetingCandidate) -> CandidateReport {
    CandidateReport {
        meeting: MeetingRowReport::from_row(&candidate.meeting, None),
        detail: candidate.detail.clone(),
        evidence: candidate.evidence.clone(),
    }
}

fn display_vault_path(
    path: &std::path::Path,
    vault: &std::path::Path,
) -> String {
    path.strip_prefix(vault)
        .map(|relative| relative.to_string_lossy().replace('\\', "/"))
        .unwrap_or_else(|_| path.to_string_lossy().replace('\\', "/"))
}

fn transcript_label(
    disposition: super::model::TranscriptDisposition,
) -> &'static str {
    match disposition {
        super::model::TranscriptDisposition::Kept => "kept",
        super::model::TranscriptDisposition::Omitted => "omitted",
        super::model::TranscriptDisposition::None => "none",
    }
}

fn serialize(value: &impl Serialize) -> String {
    serde_json::to_string(value).expect("meeting report serializes")
}
