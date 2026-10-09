//! Dry-run planner for `bob ref migrate-tasks`.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use crate::native::ref_tasks::{
    allocate_ref_block_id, allocate_unique_block_id, find_trackers,
    is_open_mark, RefTaskIndex,
};

use super::line::migrate_v1_tracker_line;

/// Where a parent came from, for the report.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ParentSource {
    Map,
    Frontmatter,
    Marker,
}

impl ParentSource {
    pub(crate) fn as_str(&self) -> &'static str {
        match self {
            Self::Map => "map",
            Self::Frontmatter => "frontmatter",
            Self::Marker => "marker",
        }
    }
}

/// One open follow-up to move from the ref note's `## Tasks`.
#[derive(Debug, Clone)]
pub(crate) struct PlannedFollowUp {
    /// Block lines (task line plus children), with compact links rewritten.
    pub lines: Vec<String>,
    /// Original block lines before compact rewrite (for removal).
    pub original: Vec<String>,
    /// Original block id (may be empty).
    pub block_id: String,
}

/// One ref task to migrate.
#[derive(Debug, Clone)]
pub(crate) struct PlannedTask {
    pub ref_note: String,
    pub stem: String,
    pub title: String,
    pub mark: char,
    pub parent_route: String,
    pub parent_source: ParentSource,
    pub parent_path: String,
    pub preview_block_id: String,
    /// v2 line with the preview id.
    pub migrated_line: String,
    /// Child lines that move with the tracker (raw, without the tracker).
    pub children: Vec<String>,
    /// Tracker block range in the ref note (start inclusive, end exclusive).
    pub tracker_start: usize,
    pub tracker_end: usize,
    pub follow_ups: Vec<PlannedFollowUp>,
    /// Old and preview-new dependency ids (when encodable).
    pub old_dep_id: Option<String>,
    pub new_dep_id_preview: Option<String>,
}

/// A ref that cannot be mapped.
#[derive(Debug, Clone)]
pub(crate) struct UnmappedRow {
    pub ref_note: String,
    pub stem: String,
    pub reason: String,
}

/// A note with two or more open trackers.
#[derive(Debug, Clone)]
pub(crate) struct MultiTrackerError {
    pub ref_note: String,
    pub count: usize,
}

/// An ambiguous bare-stem link left unchanged.
#[derive(Debug, Clone)]
pub(crate) struct AmbiguousLink {
    pub file: String,
    pub link: String,
}

/// An open task linking a migrated ref.
#[derive(Debug, Clone)]
pub(crate) struct PossibleWrapper {
    pub file: String,
    pub line: usize,
}

/// Per-file rewrite counts.
#[derive(Debug, Clone, Default)]
pub(crate) struct FileRewrite {
    pub links: usize,
    pub deps: usize,
}

/// Lane preview numbers.
#[derive(Debug, Clone)]
pub(crate) struct LanePreview {
    pub next_before: usize,
    pub next_after: usize,
    pub next_cap: u32,
    pub pending_before: usize,
    pub pending_after: usize,
    pub pending_cap: u32,
    pub unavailable: bool,
}

/// The full dry-run plan.
#[derive(Debug, Clone)]
pub(crate) struct MigrationPlan {
    pub tasks: Vec<PlannedTask>,
    pub unmapped: Vec<UnmappedRow>,
    pub multi: Vec<MultiTrackerError>,
    pub ambiguous: Vec<AmbiguousLink>,
    pub wrappers: Vec<PossibleWrapper>,
    pub rewrites: BTreeMap<String, FileRewrite>,
    pub lane_preview: LanePreview,
    /// Map rows that named no open v1 tracker (warnings, not migrations).
    pub map_warnings: Vec<String>,
}

impl MigrationPlan {
    pub(crate) fn is_empty(&self) -> bool {
        self.tasks.is_empty()
    }

    pub(crate) fn distinct_parents(&self) -> usize {
        let mut set = BTreeSet::new();
        for task in &self.tasks {
            set.insert(task.parent_path.clone());
        }
        set.len()
    }
}

/// Parse a TSV map file: `ref_note<TAB>parent`, `#` comments and blank
/// lines skipped, a header whose first field is `ref_note` skipped, extra
/// columns ignored.
pub(crate) fn parse_map_file(contents: &str) -> BTreeMap<String, String> {
    let mut map = BTreeMap::new();
    for raw in contents.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let cols: Vec<&str> = raw.split('\t').collect();
        if cols.is_empty() {
            continue;
        }
        let first = cols[0].trim();
        if first.eq_ignore_ascii_case("ref_note") {
            continue;
        }
        if cols.len() < 2 {
            continue;
        }
        let ref_note = normalize_map_ref_note(first);
        let parent = cols[1].trim().to_string();
        if ref_note.is_empty() || parent.is_empty() {
            continue;
        }
        map.insert(ref_note, parent);
    }
    map
}

fn normalize_map_ref_note(raw: &str) -> String {
    let mut s = raw.trim().to_string();
    // Accept `ref/chat/stem` or `ref/chat/stem.md`.
    if s.to_ascii_lowercase().ends_with(".md") {
        s.truncate(s.len() - 3);
    }
    s
}

/// Vault-relative ref-note path without `.md` (for link targets).
pub(crate) fn ref_target_without_md(ref_note: &str) -> String {
    ref_note
        .strip_suffix(".md")
        .or_else(|| ref_note.strip_suffix(".MD"))
        .unwrap_or(ref_note)
        .to_string()
}

/// Plan the whole migration without writing anything.
pub(crate) fn plan_migration(
    bob_dir: &Path,
    ref_dir: &Path,
    map: &BTreeMap<String, String>,
) -> MigrationPlan {
    let ref_tasks_index = RefTaskIndex::build(bob_dir, ref_dir);
    let ref_notes = collect_ref_notes(bob_dir, ref_dir);
    let stem_counts = stem_occurrences(&ref_notes);

    let mut tasks = Vec::new();
    let mut unmapped = Vec::new();
    let mut multi = Vec::new();
    let mut map_warnings = Vec::new();
    let mut reserved: BTreeSet<String> = BTreeSet::new();
    // Track which map rows matched an open v1 tracker.
    let mut map_matched: BTreeSet<String> = BTreeSet::new();

    // Sort for determinism.
    let mut ordered = ref_notes.clone();
    ordered.sort();

    for ref_note in &ordered {
        let abs = bob_dir.join(ref_note);
        let Ok(contents) = std::fs::read_to_string(&abs) else {
            continue;
        };
        let body = split_body(&contents);
        let hits = find_trackers(&body);
        let open: Vec<_> =
            hits.iter().filter(|h| is_open_mark(h.mark)).collect();
        if open.is_empty() {
            continue;
        }
        if open.len() >= 2 {
            multi.push(MultiTrackerError {
                ref_note: ref_note.clone(),
                count: open.len(),
            });
            continue;
        }
        let hit = open[0];
        // Locate the tracker block in the body.
        let body_lines: Vec<&str> = body.lines().collect();
        let Some(tracker_idx) = find_tracker_line_index(&body_lines, &hit.line)
        else {
            continue;
        };
        let indent = indentation_len(body_lines[tracker_idx]);
        let (block_end_excl, children) =
            tracker_block_extent(&body_lines, tracker_idx, indent);

        // Parent resolution: map, frontmatter, marker.
        let under_ref = under_ref_path(ref_dir, bob_dir, ref_note);
        let map_key_full = ref_target_without_md(ref_note);
        let map_key_under = under_ref
            .as_ref()
            .map(|u| {
                u.strip_suffix(".md")
                    .or_else(|| u.strip_suffix(".MD"))
                    .unwrap_or(u)
                    .to_string()
            })
            .unwrap_or_default();
        // Map lookup tries vault-relative without md, then `ref/...`
        // prefixed, then bare stem forms.
        let mut map_parent: Option<String> = None;
        for key in [
            map_key_full.clone(),
            format!("ref/{}", map_key_under),
            map_key_under.clone(),
        ] {
            if let Some(parent) = map.get(&key) {
                map_parent = Some(parent.clone());
                map_matched.insert(key.clone());
                break;
            }
        }
        // Also try normalized keys: map file stores normalized without md;
        // our keys above already cover that. Fall back to direct scan for
        // `ref/chat/stem` vs `ref/chat/stem.md` forms.
        if map_parent.is_none() {
            for (k, v) in map.iter() {
                let norm = normalize_map_ref_note(k);
                if norm == map_key_full || norm == map_key_under {
                    map_parent = Some(v.clone());
                    map_matched.insert(k.clone());
                    break;
                }
            }
        }

        let (parent_route, source) = if let Some(parent_input) = map_parent {
            match crate::native::parent_notes::resolve_parent(
                bob_dir,
                &parent_input,
            ) {
                Ok(resolved) => (resolved.route, ParentSource::Map),
                Err(err) => {
                    unmapped.push(UnmappedRow {
                        ref_note: ref_note.clone(),
                        stem: file_stem(ref_note),
                        reason: err
                            .message()
                            .lines()
                            .next()
                            .unwrap_or("map parent does not resolve")
                            .to_string(),
                    });
                    continue;
                }
            }
        } else if let Some(parent_input) = frontmatter_parent(&contents) {
            match crate::native::parent_notes::resolve_parent(
                bob_dir,
                &parent_input,
            ) {
                Ok(resolved) => (resolved.route, ParentSource::Frontmatter),
                Err(_) => {
                    // Frontmatter parent that does not resolve is not a
                    // candidate; fall through to the marker.
                    match marker_parent(bob_dir, ref_note) {
                        Ok(route) => (route, ParentSource::Marker),
                        Err(reason) => {
                            unmapped.push(UnmappedRow {
                                ref_note: ref_note.clone(),
                                stem: file_stem(ref_note),
                                reason,
                            });
                            continue;
                        }
                    }
                }
            }
        } else {
            match marker_parent(bob_dir, ref_note) {
                Ok(route) => (route, ParentSource::Marker),
                Err(reason) => {
                    unmapped.push(UnmappedRow {
                        ref_note: ref_note.clone(),
                        stem: file_stem(ref_note),
                        reason,
                    });
                    continue;
                }
            }
        };

        let parent_path = format!("{parent_route}.md");
        let parent_abs = bob_dir.join(&parent_path);

        // Title: H1, else frontmatter title, else stem.
        let title = ref_title(&contents, ref_note);
        let created = ref_created(&contents);
        let stem = file_stem(ref_note);
        let slug = crate::native::ref_tasks::slug_ref_stem(&stem);
        let _ = slug;

        // Preview block id against destination + archive + reserved.
        let taken = taken_ids(bob_dir, &parent_abs, &reserved);
        let preview = allocate_ref_block_id(&stem, &|id| taken.contains(id));
        reserved.insert(preview.clone());

        let ref_target = ref_target_without_md(ref_note);
        let v1_line_full = body_lines[tracker_idx].to_string();
        // migrate_v1_tracker_line expects the raw line; it preserves indent.
        let mut migrated = migrate_v1_tracker_line(
            &v1_line_full,
            &ref_target,
            &title,
            &created,
            &preview,
        );
        // Rewrite the old dependency id on the moved line itself.
        let old_dep = dependency_id_for(ref_note, "ref");
        let new_dep = dependency_id_for(&parent_path, &preview);
        if let (Some(old), Some(new)) = (old_dep.clone(), new_dep.clone())
            && migrated.contains(&old)
        {
            migrated = migrated.replace(&old, &new);
        }

        // Follow-ups from ## Tasks.
        let follow_ups = collect_follow_ups(
            &body_lines,
            ref_note,
            bob_dir,
            &parent_abs,
            &reserved,
        );
        for fu in &follow_ups {
            if !fu.block_id.is_empty() {
                reserved.insert(fu.block_id.clone());
            }
        }

        tasks.push(PlannedTask {
            ref_note: ref_note.clone(),
            stem,
            title,
            mark: hit.mark,
            parent_route,
            parent_source: source,
            parent_path,
            preview_block_id: preview,
            migrated_line: migrated,
            children,
            tracker_start: tracker_idx,
            tracker_end: block_end_excl,
            follow_ups,
            old_dep_id: old_dep,
            new_dep_id_preview: new_dep,
        });
        let _ = block_end_excl;
        let _ = ref_tasks_index;
    }

    // Unknown map rows are warnings, not migrations.
    for key in map.keys() {
        if !map_matched.contains(key) {
            // Check whether any ref note has an open tracker matching this
            // normalized key; if none, warn.
            let norm = normalize_map_ref_note(key);
            let mut found = false;
            for ref_note in &ordered {
                let full = ref_target_without_md(ref_note);
                if full == norm {
                    found = true;
                    break;
                }
            }
            if !found {
                map_warnings
                    .push(format!("map row for unknown ref_note: {key}"));
            }
        }
    }

    // Graph rewrite preview with preview ids.
    let id_map: BTreeMap<String, (String, String)> = tasks
        .iter()
        .map(|t| {
            (
                t.ref_note.clone(),
                (t.parent_route.clone(), t.preview_block_id.clone()),
            )
        })
        .collect();
    let dep_map: BTreeMap<String, String> = tasks
        .iter()
        .filter_map(|t| match (&t.old_dep_id, &t.new_dep_id_preview) {
            (Some(old), Some(new)) => Some((old.clone(), new.clone())),
            _ => None,
        })
        .collect();
    let (rewrites, ambiguous, wrappers) = super::rewrite::preview_rewrites(
        bob_dir,
        &id_map,
        &dep_map,
        &stem_counts,
    );

    let lane_preview = lane_preview_for(bob_dir, &tasks);

    // Deterministic order: by ref note.
    tasks.sort_by(|a, b| a.ref_note.cmp(&b.ref_note));
    unmapped.sort_by(|a, b| a.ref_note.cmp(&b.ref_note));
    multi.sort_by(|a, b| a.ref_note.cmp(&b.ref_note));

    MigrationPlan {
        tasks,
        unmapped,
        multi,
        ambiguous,
        wrappers,
        rewrites,
        lane_preview,
        map_warnings,
    }
}

fn collect_ref_notes(bob_dir: &Path, ref_dir: &Path) -> Vec<String> {
    let mut out = Vec::new();
    let mut stack = vec![ref_dir.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        let mut ordered: Vec<_> = entries.flatten().collect();
        ordered.sort_by_key(|e| e.file_name());
        for entry in ordered {
            let path = entry.path();
            if path.is_dir() {
                let name = path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or_default();
                if name.starts_with('.') {
                    continue;
                }
                stack.push(path);
                continue;
            }
            if !path.is_file() {
                continue;
            }
            if !path
                .extension()
                .and_then(|e| e.to_str())
                .is_some_and(|e| e.eq_ignore_ascii_case("md"))
            {
                continue;
            }
            if let Ok(rel) = path.strip_prefix(bob_dir) {
                out.push(rel.to_string_lossy().replace('\\', "/"));
            }
        }
    }
    out
}

fn stem_occurrences(ref_notes: &[String]) -> BTreeMap<String, usize> {
    let mut map = BTreeMap::new();
    for note in ref_notes {
        let stem = file_stem(note).to_ascii_lowercase();
        *map.entry(stem).or_insert(0) += 1;
    }
    map
}

fn file_stem(path: &str) -> String {
    Path::new(path)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or(path)
        .to_string()
}

fn under_ref_path(
    ref_dir: &Path,
    bob_dir: &Path,
    ref_note: &str,
) -> Option<String> {
    let abs = bob_dir.join(ref_note);
    abs.strip_prefix(ref_dir)
        .ok()
        .map(|rel| rel.to_string_lossy().replace('\\', "/"))
}

fn split_body(contents: &str) -> String {
    if let Some((_, body)) =
        crate::native::highlights_ref::split_frontmatter(contents)
    {
        body
    } else {
        contents.to_string()
    }
}

fn find_tracker_line_index(lines: &[&str], hit_line: &str) -> Option<usize> {
    let want = hit_line.trim();
    for (i, line) in lines.iter().enumerate() {
        if line.trim() == want {
            // Confirm it parses as a tracker.
            if crate::native::ref_tasks::parse_tracker_line(line).is_some() {
                return Some(i);
            }
        }
    }
    // Fallback: first open tracker parse.
    for (i, line) in lines.iter().enumerate() {
        if let Some(hit) = crate::native::ref_tasks::parse_tracker_line(line)
            && is_open_mark(hit.mark)
            && line.trim() == want
        {
            return Some(i);
        }
    }
    None
}

fn indentation_len(line: &str) -> usize {
    line.find(|c: char| !c.is_whitespace())
        .unwrap_or(line.len())
}

/// Tracker block extent: the tracker line plus every more-indented line,
/// including blank lines that sit inside the block.
fn tracker_block_extent(
    lines: &[&str],
    task_index: usize,
    task_indent: usize,
) -> (usize, Vec<String>) {
    let mut end = task_index + 1;
    let mut children = Vec::new();
    let mut i = task_index + 1;
    while i < lines.len() {
        let line = lines[i];
        if line.trim().is_empty() {
            // Blank inside the block only when the next non-blank is deeper.
            let next = lines[i + 1..]
                .iter()
                .position(|l| !l.trim().is_empty())
                .map(|o| i + 1 + o);
            if next.is_some_and(|n| indentation_len(lines[n]) > task_indent) {
                children.push(line.to_string());
                end = i + 1;
                i += 1;
                continue;
            }
            break;
        }
        if indentation_len(line) <= task_indent {
            break;
        }
        children.push(line.to_string());
        end = i + 1;
        i += 1;
    }
    (end, children)
}

fn frontmatter_parent(contents: &str) -> Option<String> {
    let (front, _) =
        crate::native::highlights_ref::split_frontmatter(contents)?;
    for raw in front {
        let Some((key, value)) = raw.split_once(':') else {
            continue;
        };
        if key.trim().eq_ignore_ascii_case("parent") {
            let mut v = value.trim().to_string();
            // Strip surrounding quotes.
            if (v.starts_with('"') && v.ends_with('"') && v.len() >= 2)
                || (v.starts_with('\'') && v.ends_with('\'') && v.len() >= 2)
            {
                v = v[1..v.len() - 1].to_string();
            }
            v = v.trim().to_string();
            if !v.is_empty() {
                return Some(v);
            }
        }
    }
    None
}

fn marker_parent(bob_dir: &Path, ref_note: &str) -> Result<String, String> {
    // Derive the sibling PDF under lib/: ref/<type>/<stem>.md ->
    // lib/<type>/<stem>.pdf. Try lib/ then xlib/.
    let rel = Path::new(ref_note);
    let under = rel
        .strip_prefix("ref")
        .or_else(|_| rel.strip_prefix("REF"))
        .map_err(|_| format!("no PDF marker for {ref_note}: not under ref/"))?;
    let stem_path = under.with_extension("");
    let candidates = [
        Path::new("lib").join(&stem_path).with_extension("pdf"),
        Path::new("xlib").join(&stem_path).with_extension("pdf"),
    ];
    let mut pdf_found: Option<PathBuf> = None;
    for cand in &candidates {
        if bob_dir.join(cand).is_file() {
            pdf_found = Some(cand.clone());
            break;
        }
    }
    let Some(pdf_rel) = pdf_found else {
        return Err(format!(
            "no parent for {ref_note}: no frontmatter parent and no PDF marker"
        ));
    };
    match read_marker_parent_from_pdf(&bob_dir.join(&pdf_rel)) {
        Ok(parent) => {
            match crate::native::parent_notes::resolve_parent(bob_dir, &parent)
            {
                Ok(resolved) => Ok(resolved.route),
                Err(err) => Err(format!(
                    "marker parent for {ref_note} does not resolve: {}",
                    err.message().lines().next().unwrap_or("unknown")
                )),
            }
        }
        Err(err) => Err(format!("marker read failed for {ref_note}: {err}")),
    }
}

fn read_marker_parent_from_pdf(pdf: &Path) -> Result<String, String> {
    let bytes = std::fs::read(pdf).map_err(|e| format!("read PDF: {e}"))?;
    // Best-effort: look for a `parent:` line in the raw bytes (markers are
    // often stored as text). A full lopdf parse lives in highlights_ref;
    // this path only needs to name the failure when absent.
    let text = String::from_utf8_lossy(&bytes);
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.to_ascii_lowercase().starts_with("parent:") {
            let value = trimmed["parent:".len()..]
                .trim()
                .trim_matches('"')
                .trim_matches('\'')
                .trim()
                .to_string();
            if !value.is_empty() {
                return Ok(value);
            }
        }
    }
    Err("no parent in PDF marker".to_string())
}

fn ref_title(contents: &str, ref_note: &str) -> String {
    let body = split_body(contents);
    for line in body.lines() {
        let t = line.trim();
        if let Some(title) = t.strip_prefix("# ") {
            let title = title.trim();
            if !title.is_empty() {
                return title.to_string();
            }
            break;
        }
        if t.starts_with("# ") {
            break;
        }
    }
    // Frontmatter title.
    if let Some((front, _)) =
        crate::native::highlights_ref::split_frontmatter(contents)
    {
        for raw in front {
            if let Some((key, value)) = raw.split_once(':') {
                if key.trim().eq_ignore_ascii_case("title") {
                    let mut v = value.trim().to_string();
                    if (v.starts_with('"') && v.ends_with('"') && v.len() >= 2)
                        || (v.starts_with('\'')
                            && v.ends_with('\'')
                            && v.len() >= 2)
                    {
                        v = v[1..v.len() - 1].to_string();
                    }
                    if !v.trim().is_empty() {
                        return v.trim().to_string();
                    }
                }
            }
        }
    }
    file_stem(ref_note)
}

fn ref_created(contents: &str) -> String {
    if let Some((front, _)) =
        crate::native::highlights_ref::split_frontmatter(contents)
    {
        for raw in front {
            if let Some((key, value)) = raw.split_once(':') {
                if key.trim().eq_ignore_ascii_case("created") {
                    let mut v = value.trim().to_string();
                    if (v.starts_with('"') && v.ends_with('"') && v.len() >= 2)
                        || (v.starts_with('\'')
                            && v.ends_with('\'')
                            && v.len() >= 2)
                    {
                        v = v[1..v.len() - 1].to_string();
                    }
                    let v = v.trim().to_string();
                    // Accept a date prefix YYYY-MM-DD.
                    if v.len() >= 10
                        && v[..4].chars().all(|c| c.is_ascii_digit())
                        && &v[4..5] == "-"
                        && v[5..7].chars().all(|c| c.is_ascii_digit())
                        && &v[7..8] == "-"
                        && v[8..10].chars().all(|c| c.is_ascii_digit())
                    {
                        return v[..10].to_string();
                    }
                }
            }
        }
    }
    crate::native::env::current_datetime()
        .date()
        .format("%Y-%m-%d")
        .to_string()
}

fn taken_ids(
    bob_dir: &Path,
    dest_abs: &Path,
    reserved: &BTreeSet<String>,
) -> BTreeSet<String> {
    let mut taken = BTreeSet::new();
    if let Ok(contents) = std::fs::read_to_string(dest_abs) {
        taken.extend(crate::native::collect_done::block_ids_in_markdown(
            &contents,
        ));
        // Archive.
        if let Some(archive_rel) = archive_rel_for(bob_dir, dest_abs, &contents)
        {
            if let Ok(archive) =
                std::fs::read_to_string(bob_dir.join(&archive_rel))
            {
                taken.extend(
                    crate::native::collect_done::block_ids_in_markdown(
                        &archive,
                    ),
                );
            }
        }
    }
    taken.extend(reserved.iter().cloned());
    taken
}

fn archive_rel_for(
    bob_dir: &Path,
    dest_abs: &Path,
    dest_contents: &str,
) -> Option<String> {
    for line in dest_contents.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("done_tasks:") {
            if let Some(start) = trimmed.find("[[") {
                if let Some(end) = trimmed[start..].find("]]") {
                    let target = &trimmed[start + 2..start + end];
                    let mut rel = target.trim().to_string();
                    if !rel.ends_with(".md") {
                        rel.push_str(".md");
                    }
                    return Some(rel);
                }
            }
        }
    }
    let relative = dest_abs.strip_prefix(bob_dir).unwrap_or(dest_abs);
    if relative.components().count() == 1 {
        if let Some(stem) = dest_abs.file_stem().and_then(|s| s.to_str()) {
            return Some(format!("done/{stem}_done.md"));
        }
    }
    None
}

fn dependency_id_for(note_rel: &str, block_id: &str) -> Option<String> {
    let path = Path::new(note_rel);
    crate::native::task_dependencies::dependency_id(path, block_id).ok()
}

/// Collect open follow-up blocks from the ref note's `## Tasks`.
fn collect_follow_ups(
    body_lines: &[&str],
    ref_note: &str,
    _bob_dir: &Path,
    dest_abs: &Path,
    reserved: &BTreeSet<String>,
) -> Vec<PlannedFollowUp> {
    // Find ## Tasks section.
    let mut section_start: Option<usize> = None;
    for (i, line) in body_lines.iter().enumerate() {
        if line.trim().eq_ignore_ascii_case("## tasks") {
            section_start = Some(i);
            break;
        }
    }
    let Some(start) = section_start else {
        return Vec::new();
    };
    // Section ends at next ## heading or end.
    let mut section_end = body_lines.len();
    for (i, line) in body_lines.iter().enumerate().skip(start + 1) {
        let t = line.trim();
        if t.starts_with("## ") {
            section_end = i;
            break;
        }
    }
    let mut out = Vec::new();
    let mut i = start + 1;
    while i < section_end {
        let line = body_lines[i];
        let stripped = strip_blockquote(line);
        let Some(mark) = task_mark_of(stripped) else {
            i += 1;
            continue;
        };
        if !is_open_mark(mark) {
            // Still skip its block.
            let indent = indentation_len(line);
            let (end, _) = tracker_block_extent(body_lines, i, indent);
            i = end;
            continue;
        }
        if !line.contains("🔖") {
            i += 1;
            continue;
        }
        // Does it target this ref? Compact [[#^h-...|🔖]] always counts
        // inside the ref note; otherwise require the target to resolve to
        // this ref note (path-qualified or unique stem is checked by the
        // locator, but here a substring match suffices for planning).
        let targets_this = is_follow_up_for_ref(line, ref_note);
        if !targets_this {
            i += 1;
            continue;
        }
        let indent = indentation_len(line);
        let (end, children) = tracker_block_extent(body_lines, i, indent);
        let original: Vec<String> = std::iter::once(line.to_string())
            .chain(children.iter().cloned())
            .collect();
        let mut block_lines: Vec<String> = Vec::new();
        // Rewrite compact links to the full ref-note path.
        let rewritten_first = rewrite_compact_bookmark(line, ref_note);
        block_lines.push(rewritten_first);
        for child in &children {
            block_lines.push(rewrite_compact_bookmark(child, ref_note));
        }
        // Block id handling: preserve when free in destination+archive,
        // else suffix with -2,-3 rule.
        let existing = crate::native::collect_done::trailing_block_id_in_line(
            &block_lines[0],
        )
        .unwrap_or_default();
        let final_id = if existing.is_empty() {
            String::new()
        } else {
            // Check freeness against destination+archive+reserved.
            let taken = taken_ids(_bob_dir, dest_abs, reserved);
            if !taken.contains(&existing) {
                existing.clone()
            } else {
                allocate_unique_block_id(&existing, &|id| taken.contains(id))
            }
        };
        if !final_id.is_empty() && final_id != existing {
            // Replace trailing id.
            if let Some(last) = block_lines.first() {
                let replaced = replace_trailing_id(last, &final_id);
                block_lines[0] = replaced;
            }
        }
        out.push(PlannedFollowUp {
            lines: block_lines,
            original,
            block_id: final_id,
        });
        i = end;
    }
    out
}

fn strip_blockquote(line: &str) -> &str {
    let mut l = line;
    loop {
        let spaces = l.bytes().take_while(|b| *b == b' ').count();
        if spaces > 3 || l.as_bytes().get(spaces) != Some(&b'>') {
            return l;
        }
        l = &l[spaces + 1..];
        if let Some(rest) = l.strip_prefix(' ') {
            l = rest;
        }
    }
}

fn task_mark_of(line: &str) -> Option<char> {
    let t = line.trim_start_matches([' ', '\t']);
    let after = t
        .strip_prefix("- ")
        .or_else(|| t.strip_prefix("* "))
        .or_else(|| t.strip_prefix("+ "))?;
    let bracket = after.strip_prefix('[')?;
    let mark = bracket.chars().next()?;
    let rest = bracket[mark.len_utf8()..].strip_prefix(']')?;
    if !rest.is_empty() && !rest.starts_with(char::is_whitespace) {
        return None;
    }
    Some(mark)
}

fn is_follow_up_for_ref(line: &str, ref_note: &str) -> bool {
    // Compact link always targets the containing ref note.
    if line.contains("[[#^h-") {
        return true;
    }
    // Path-qualified link to this ref note.
    let target_no_md = ref_target_without_md(ref_note);
    if line.contains(&target_no_md) {
        return true;
    }
    // Bare stem link to this ref's stem.
    let stem = file_stem(ref_note);
    // Look for [[stem#^h-...|🔖]].
    if line.contains(&format!("[[{stem}#"))
        || line.contains(&format!("[[{stem} #"))
    {
        return true;
    }
    // Fallback: any 🔖 link in the ref note's Tasks is treated as its own.
    line.contains("🔖")
}

fn rewrite_compact_bookmark(line: &str, ref_note: &str) -> String {
    let target = ref_target_without_md(ref_note);
    line.replace("[[#^", &format!("[[{target}#^"))
}

fn replace_trailing_id(line: &str, new_id: &str) -> String {
    if let Some(caret) = line.rfind('^') {
        // Ensure the caret starts a trailing token.
        let before = &line[..caret];
        if before.ends_with(' ') || before.ends_with('\t') {
            // Find token end (trim trailing spaces).
            let trimmed_end = line.trim_end().len();
            return format!(
                "{}{}",
                &line[..caret + 1],
                format!("{new_id}{}", &line[trimmed_end..])
            );
        }
    }
    format!("{line} ^{new_id}")
}

fn lane_preview_for(bob_dir: &Path, tasks: &[PlannedTask]) -> LanePreview {
    use crate::native::config::plan::PlanConfig;
    let config = PlanConfig::default();
    let today = crate::native::env::current_datetime().date();
    let lanes =
        crate::native::plan_budget::count_lanes(bob_dir, today, &config);
    let mut next_add = 0usize;
    let mut pending_add = 0usize;
    let today_str = today.format("%Y-%m-%d").to_string();
    for task in tasks {
        let text =
            format!("{} {}", task.migrated_line, task.children.join("\n"));
        let blocked =
            text.contains("[dependsOn::") || text.contains("dependsOn::");
        let scheduled_after = scheduled_after_today(&text, &today_str);
        if blocked || scheduled_after {
            continue;
        }
        match task.mark {
            '*' => next_add += 1,
            '/' => pending_add += 1,
            _ => {}
        }
    }
    LanePreview {
        next_before: lanes.next.count,
        next_after: lanes.next.count + next_add,
        next_cap: config.max_next,
        pending_before: lanes.pending.count,
        pending_after: lanes.pending.count + pending_add,
        pending_cap: config.max_pending,
        unavailable: false,
    }
}

fn scheduled_after_today(text: &str, today: &str) -> bool {
    // Look for [scheduled:: DATE] or [due:: DATE] or scheduled emoji dates.
    for key in ["[scheduled::", "[due::", "[scheduled_on::"] {
        let mut search = 0usize;
        while let Some(pos) = text[search..].find(key) {
            let abs = search + pos + key.len();
            let rest = text[abs..].trim_start();
            if rest.len() >= 10 {
                let date = &rest[..10];
                if date > today && date[..4].chars().all(|c| c.is_ascii_digit())
                {
                    return true;
                }
            }
            search = abs;
        }
    }
    false
}
