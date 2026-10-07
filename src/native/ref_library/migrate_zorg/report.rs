//! Human and JSON reports for `bob ref migrate-zorg` (dry run).

use serde::Serialize;

use super::super::output::{generated_at, REF_SCHEMA_VERSION};
use super::plan::MigrationPlan;

/// Render the human dry-run report: headline, counts by file and status,
/// books, renamed stems, URL gaps, identity hits, already-migrated and
/// skipped records, and the post-write coverage line.
pub(crate) fn render_human(plan: &MigrationPlan) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "bob ref migrate-zorg · dry run · {} records in {} files → {} notes under ref/zorg/\n",
        plan.records,
        plan.by_file.len(),
        plan.notes.len(),
    ));
    out.push_str("\nrecords by status:\n");
    if plan.by_status.is_empty() {
        out.push_str("  (none)\n");
    }
    for (status, count) in &plan.by_status {
        out.push_str(&format!("  {status} {count}\n"));
    }
    out.push_str("\nrecords by file:\n");
    if plan.by_file.is_empty() {
        out.push_str("  (none)\n");
    }
    for (path, count) in &plan.by_file {
        out.push_str(&format!("  {path} {count}\n"));
    }
    let books: Vec<_> = plan.notes.iter().filter(|note| note.is_book).collect();
    out.push_str(&format!("\nbooks ({}):\n", books.len()));
    if books.is_empty() {
        out.push_str("  (none)\n");
    }
    for book in books {
        out.push_str(&format!(
            "  {} · {} · {} chapters · {}\n",
            book.path, book.title, book.chapter_count, book.reading_state,
        ));
    }
    let renamed: Vec<_> = plan
        .notes
        .iter()
        .filter(|note| note.renamed.is_some())
        .collect();
    out.push_str(&format!("\nrenamed stems ({}):\n", renamed.len()));
    if renamed.is_empty() {
        out.push_str("  (none)\n");
    }
    for note in renamed {
        let renamed = note.renamed.as_ref().expect("renamed");
        out.push_str(&format!(
            "  {} → {} (avoids {})\n",
            renamed.from, note.path, renamed.avoids,
        ));
    }
    let no_url: Vec<_> = plan
        .notes
        .iter()
        .filter(|note| note.urls.is_empty())
        .collect();
    out.push_str(&format!("\nnotes without a URL ({}):\n", no_url.len()));
    if no_url.is_empty() {
        out.push_str("  (none)\n");
    }
    for note in no_url {
        out.push_str(&format!("  {}\n", note.path));
    }
    out.push_str(&format!(
        "\nidentity hits ({}):\n",
        plan.identity_hits.len()
    ));
    if plan.identity_hits.is_empty() {
        out.push_str("  (none)\n");
    }
    for hit in &plan.identity_hits {
        out.push_str(&format!(
            "  {} → {} (planned {})\n",
            hit.key, hit.note, hit.planned,
        ));
    }
    out.push_str(&format!("\nalready migrated: {}\n", plan.already_migrated));
    out.push_str(&format!("\nskipped ({}):\n", plan.skipped.len()));
    if plan.skipped.is_empty() {
        out.push_str("  (none)\n");
    }
    for skipped in &plan.skipped {
        out.push_str(&format!(
            "  {}:{} {}\n",
            skipped.path, skipped.line, skipped.reason,
        ));
    }
    out.push_str(&format!(
        "\ncoverage after --write: {} unindexed ({} = skipped)\n",
        plan.skipped.len(),
        plan.skipped.len(),
    ));
    out
}

/// The dry-run JSON envelope: compact one-line JSON, no ANSI.
#[derive(Debug, Clone, Serialize)]
struct ReportEnvelope {
    ok: bool,
    schema_version: u32,
    command: &'static str,
    generated_at: String,
    mode: &'static str,
    summary: ReportSummary,
    by_status: std::collections::BTreeMap<String, usize>,
    by_file: Vec<FileRow>,
    notes: Vec<NoteRow>,
    identity_hits: Vec<HitRow>,
    skipped: Vec<SkippedRow>,
    commit: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize)]
struct ReportSummary {
    records: usize,
    notes: usize,
    chapters: usize,
    already_migrated: usize,
    skipped: usize,
    renamed: usize,
    no_url: usize,
    identity_hits: usize,
}

#[derive(Debug, Clone, Serialize)]
struct FileRow {
    path: String,
    records: usize,
}

#[derive(Debug, Clone, Serialize)]
struct NoteRow {
    path: String,
    title: String,
    ref_type: String,
    source_path: String,
    source_block: String,
    source_id: String,
    legacy_status: String,
    reading_state: String,
    urls: Vec<String>,
    chapters: usize,
    renamed: Option<RenamedRow>,
}

#[derive(Debug, Clone, Serialize)]
struct RenamedRow {
    from: String,
    avoids: String,
}

#[derive(Debug, Clone, Serialize)]
struct HitRow {
    key: String,
    planned: String,
    note: String,
}

#[derive(Debug, Clone, Serialize)]
struct SkippedRow {
    path: String,
    line: usize,
    reason: String,
}

/// Print the dry-run JSON envelope as compact one-line JSON.
pub(crate) fn print_json(plan: &MigrationPlan) {
    let envelope = ReportEnvelope {
        ok: true,
        schema_version: REF_SCHEMA_VERSION,
        command: "ref migrate-zorg",
        generated_at: generated_at(),
        mode: "dry_run",
        summary: ReportSummary {
            records: plan.records,
            notes: plan.notes.len(),
            chapters: plan.chapters,
            already_migrated: plan.already_migrated,
            skipped: plan.skipped.len(),
            renamed: plan.renamed(),
            no_url: plan.no_url(),
            identity_hits: plan.identity_hits.len(),
        },
        by_status: plan.by_status.iter().cloned().collect(),
        by_file: plan
            .by_file
            .iter()
            .map(|(path, records)| FileRow {
                path: path.clone(),
                records: *records,
            })
            .collect(),
        notes: plan
            .notes
            .iter()
            .map(|note| NoteRow {
                path: note.path.clone(),
                title: note.title.clone(),
                ref_type: note.ref_type.clone(),
                source_path: note.source_path.clone(),
                source_block: note.source_block.clone(),
                source_id: note.source_id.clone(),
                legacy_status: note.legacy_status.clone(),
                reading_state: note.reading_state.clone(),
                urls: note.urls.clone(),
                chapters: note.chapter_count,
                renamed: note.renamed.clone().map(|renamed| RenamedRow {
                    from: renamed.from,
                    avoids: renamed.avoids,
                }),
            })
            .collect(),
        identity_hits: plan
            .identity_hits
            .iter()
            .map(|hit| HitRow {
                key: hit.key.clone(),
                planned: hit.planned.clone(),
                note: hit.note.clone(),
            })
            .collect(),
        skipped: plan
            .skipped
            .iter()
            .map(|skipped| SkippedRow {
                path: skipped.path.clone(),
                line: skipped.line,
                reason: skipped.reason.to_string(),
            })
            .collect(),
        commit: None,
    };
    println!(
        "{}",
        serde_json::to_string(&envelope)
            .expect("migrate-zorg envelope serializes")
    );
}
