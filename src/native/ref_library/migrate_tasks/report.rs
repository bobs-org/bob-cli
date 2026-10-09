//! Human, JSON, and TSV reports for `bob ref migrate-tasks`.

use std::collections::BTreeMap;

use serde::Serialize;

use super::super::output::{generated_at, REF_SCHEMA_VERSION};
use super::plan::MigrationPlan;

/// Which run produced a report.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ReportMode {
    DryRun,
    Write { sha: String, subject: String },
}

impl ReportMode {
    pub(crate) fn dry_run() -> Self {
        Self::DryRun
    }

    pub(crate) fn write(sha: &str, subject: &str) -> Self {
        Self::Write {
            sha: sha.to_string(),
            subject: subject.to_string(),
        }
    }

    fn headline_verb(&self) -> &'static str {
        match self {
            Self::DryRun => "dry run",
            Self::Write { .. } => "write",
        }
    }

    fn json_mode(&self) -> &'static str {
        match self {
            Self::DryRun => "dry_run",
            Self::Write { .. } => "write",
        }
    }
}

/// Render the human report.
pub(crate) fn render_human(plan: &MigrationPlan, mode: &ReportMode) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "{} open ref tasks → {} notes ({})\n",
        plan.tasks.len(),
        plan.distinct_parents(),
        mode.headline_verb(),
    ));
    out.push_str("\ntasks:\n");
    if plan.tasks.is_empty() {
        out.push_str("  (none)\n");
    }
    for task in &plan.tasks {
        out.push_str(&format!(
            "  {} [{}] → {} ^{} ({})\n",
            task.stem,
            task.mark,
            task.parent_route,
            task.preview_block_id,
            task.parent_source.as_str(),
        ));
    }
    out.push_str("\nlinks and dependency ids to rewrite, by file:\n");
    if plan.rewrites.is_empty() {
        out.push_str("  (none)\n");
    }
    let mut files: Vec<_> = plan.rewrites.iter().collect();
    files.sort_by(|a, b| a.0.cmp(b.0));
    for (file, counts) in files {
        out.push_str(&format!(
            "  {file}: {} links, {} ids\n",
            counts.links, counts.deps
        ));
    }
    out.push_str("\npossible wrappers:\n");
    if plan.wrappers.is_empty() {
        out.push_str("  (none)\n");
    }
    for w in &plan.wrappers {
        out.push_str(&format!("  {}:{}\n", w.file, w.line));
    }
    out.push_str("\nambiguous links:\n");
    if plan.ambiguous.is_empty() {
        out.push_str("  (none)\n");
    }
    for a in &plan.ambiguous {
        out.push_str(&format!("  {} {}\n", a.file, a.link));
    }
    out.push_str("\nunmapped refs:\n");
    if plan.unmapped.is_empty() {
        out.push_str("  (none)\n");
    }
    for u in &plan.unmapped {
        out.push_str(&format!("  {}: {}\n", u.ref_note, u.reason));
    }
    out.push_str("\nmultiple-tracker errors:\n");
    if plan.multi.is_empty() {
        out.push_str("  (none)\n");
    }
    for m in &plan.multi {
        out.push_str(&format!("  {}: {} open trackers\n", m.ref_note, m.count));
    }
    if !plan.map_warnings.is_empty() {
        out.push_str("\nmap warnings:\n");
        for w in &plan.map_warnings {
            out.push_str(&format!("  {w}\n"));
        }
    }
    out.push_str("\nlane preview:\n");
    if plan.lane_preview.unavailable {
        out.push_str("  lane preview unavailable\n");
    } else {
        out.push_str(&format!(
            "  Next {} → {} / {}\n",
            plan.lane_preview.next_before,
            plan.lane_preview.next_after,
            plan.lane_preview.next_cap,
        ));
        out.push_str(&format!(
            "  Pending {} → {} / {}\n",
            plan.lane_preview.pending_before,
            plan.lane_preview.pending_after,
            plan.lane_preview.pending_cap,
        ));
    }
    if let ReportMode::Write { sha, subject } = mode {
        out.push_str(&format!("\ncommitted {} {subject}\n", short_sha(sha)));
    }
    out
}

fn short_sha(sha: &str) -> String {
    sha.chars().take(7).collect()
}

/// Render TSV: `ref_note<TAB>parent<TAB>status<TAB>title`, parent empty when
/// unmapped.
pub(crate) fn render_tsv(plan: &MigrationPlan) -> String {
    let mut out = String::new();
    for task in &plan.tasks {
        out.push_str(&format!(
            "{}\t{}\tmapped\t{}\n",
            task.ref_note, task.parent_route, task.title
        ));
    }
    for u in &plan.unmapped {
        out.push_str(&format!("{}\t\tunmapped\t\n", u.ref_note));
    }
    for m in &plan.multi {
        out.push_str(&format!("{}\t\terror\t\n", m.ref_note));
    }
    out
}

/// The JSON envelope.
#[derive(Debug, Clone, Serialize)]
struct ReportEnvelope {
    ok: bool,
    schema_version: u32,
    command: &'static str,
    generated_at: String,
    mode: &'static str,
    summary: ReportSummary,
    tasks: Vec<TaskRow>,
    unmapped: Vec<UnmappedRow>,
    ambiguous_links: Vec<AmbiguousRow>,
    possible_wrappers: Vec<WrapperRow>,
    rewrites: BTreeMap<String, FileCounts>,
    lane_preview: LanePreviewJson,
    commit: Option<CommitInfo>,
}

#[derive(Debug, Clone, Serialize)]
struct ReportSummary {
    tasks: usize,
    notes: usize,
    unmapped: usize,
    ambiguous: usize,
    wrappers: usize,
}

#[derive(Debug, Clone, Serialize)]
struct TaskRow {
    ref_note: String,
    stem: String,
    mark: String,
    parent: String,
    parent_source: String,
    block_id: String,
    title: String,
}

#[derive(Debug, Clone, Serialize)]
struct UnmappedRow {
    ref_note: String,
    reason: String,
}

#[derive(Debug, Clone, Serialize)]
struct AmbiguousRow {
    file: String,
    link: String,
}

#[derive(Debug, Clone, Serialize)]
struct WrapperRow {
    file: String,
    line: usize,
}

#[derive(Debug, Clone, Serialize)]
struct FileCounts {
    links: usize,
    deps: usize,
}

#[derive(Debug, Clone, Serialize)]
struct LanePreviewJson {
    next_before: usize,
    next_after: usize,
    next_cap: u32,
    pending_before: usize,
    pending_after: usize,
    pending_cap: u32,
    unavailable: bool,
}

#[derive(Debug, Clone, Serialize)]
struct CommitInfo {
    sha: String,
    subject: String,
    paths: Vec<String>,
}

/// Print the JSON envelope as compact one-line JSON.
pub(crate) fn print_json(
    plan: &MigrationPlan,
    mode: &ReportMode,
    commit_paths: Option<(String, String, Vec<String>)>,
) {
    let commit = match mode {
        ReportMode::DryRun => None,
        ReportMode::Write { .. } => {
            commit_paths.map(|(sha, subject, paths)| CommitInfo {
                sha,
                subject,
                paths,
            })
        }
    };
    let envelope = ReportEnvelope {
        ok: true,
        schema_version: REF_SCHEMA_VERSION,
        command: "ref migrate-tasks",
        generated_at: generated_at(),
        mode: mode.json_mode(),
        summary: ReportSummary {
            tasks: plan.tasks.len(),
            notes: plan.distinct_parents(),
            unmapped: plan.unmapped.len(),
            ambiguous: plan.ambiguous.len(),
            wrappers: plan.wrappers.len(),
        },
        tasks: plan
            .tasks
            .iter()
            .map(|t| TaskRow {
                ref_note: t.ref_note.clone(),
                stem: t.stem.clone(),
                mark: t.mark.to_string(),
                parent: t.parent_route.clone(),
                parent_source: t.parent_source.as_str().to_string(),
                block_id: t.preview_block_id.clone(),
                title: t.title.clone(),
            })
            .collect(),
        unmapped: plan
            .unmapped
            .iter()
            .map(|u| UnmappedRow {
                ref_note: u.ref_note.clone(),
                reason: u.reason.clone(),
            })
            .collect(),
        ambiguous_links: plan
            .ambiguous
            .iter()
            .map(|a| AmbiguousRow {
                file: a.file.clone(),
                link: a.link.clone(),
            })
            .collect(),
        possible_wrappers: plan
            .wrappers
            .iter()
            .map(|w| WrapperRow {
                file: w.file.clone(),
                line: w.line,
            })
            .collect(),
        rewrites: plan
            .rewrites
            .iter()
            .map(|(file, counts)| {
                (
                    file.clone(),
                    FileCounts {
                        links: counts.links,
                        deps: counts.deps,
                    },
                )
            })
            .collect(),
        lane_preview: LanePreviewJson {
            next_before: plan.lane_preview.next_before,
            next_after: plan.lane_preview.next_after,
            next_cap: plan.lane_preview.next_cap,
            pending_before: plan.lane_preview.pending_before,
            pending_after: plan.lane_preview.pending_after,
            pending_cap: plan.lane_preview.pending_cap,
            unavailable: plan.lane_preview.unavailable,
        },
        commit,
    };
    println!(
        "{}",
        serde_json::to_string(&envelope).expect("migrate-tasks serializes")
    );
}

/// Unused noop printer kept for API symmetry; `write.rs` renders the
/// empty-write envelope inline so `commit` stays `null`.
#[allow(dead_code)]
pub(crate) fn print_noop_json(_plan: &MigrationPlan) {}
