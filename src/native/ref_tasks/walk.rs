//! The vault walk and prefilter.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use super::dates;
use super::line::{
    has_caret_ref, has_ref_token, is_embed_task_line, is_task_line,
    parse_follow_up_task, strip_blockquote_prefix, task_mark, OrphanRefTask,
    RefFollowUp,
};
use super::select::{select_for_ref, RefTaskSelection};
use super::v1::TrackerHit;

/// One located v2 reading task.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LocatedRefTask {
    pub path: String,
    // Reserved for phase ref-sync-v2.
    #[allow(dead_code)]
    pub line_index: usize,
    // Reserved for phase ref-sync-v2.
    #[allow(dead_code)]
    pub line: String,
    pub mark: char,
    pub block_id: Option<String>,
    pub archived: bool,
    pub residence: Option<String>,
    pub closed_on: Option<String>,
    pub in_capture_target: bool,
}

/// The built index: candidates and follow-ups keyed by ref-note path.
#[derive(Debug, Clone)]
pub(crate) struct RefTaskIndex {
    candidates: BTreeMap<String, Vec<LocatedRefTask>>,
    follow_ups: BTreeMap<String, Vec<RefFollowUp>>,
    orphans: Vec<OrphanRefTask>,
    warnings: Vec<String>,
}

/// True when a file name is a conflict copy.
pub(crate) fn is_conflict_copy_name(name: &str) -> bool {
    name.contains(" (conflict")
        || name.contains(" (Conflicted copy")
        || name.contains(".sync-conflict-")
}

impl RefTaskIndex {
    /// Build the index over `(bob_dir, ref_dir)`. Never errors; problems
    /// become `warnings`.
    pub(crate) fn build(bob_dir: &Path, ref_dir: &Path) -> Self {
        let mut warnings = Vec::new();
        let ref_rel = match ref_dir.strip_prefix(bob_dir) {
            Ok(rel) => rel.to_string_lossy().replace('\\', "/"),
            Err(_) => {
                warnings.push(
                    "ref dir is outside the vault; reading tasks were not located"
                        .to_string(),
                );
                return Self {
                    candidates: BTreeMap::new(),
                    follow_ups: BTreeMap::new(),
                    orphans: Vec::new(),
                    warnings,
                };
            }
        };
        let ref_prefix = if ref_rel.is_empty() {
            String::new()
        } else {
            format!("{}/", ref_rel.trim_end_matches('/'))
        };

        let (mut paths, mut walk_warnings) =
            crate::native::capture_dependency_tasks::vault_note_paths(bob_dir);
        warnings.append(&mut walk_warnings);
        // Drop lib/xlib first components and conflict copies.
        paths.retain(|p| {
            let forward = p.to_string_lossy().replace('\\', "/");
            let first = forward.split('/').next().unwrap_or("");
            if first == "lib" || first == "xlib" {
                return false;
            }
            if let Some(name) = p.file_name().and_then(|n| n.to_str())
                && is_conflict_copy_name(name)
            {
                return false;
            }
            true
        });

        // Ref-note set: walked paths under <ref_rel>/, skipping *.assets.
        let mut ref_notes: Vec<String> = Vec::new();
        for p in &paths {
            let forward = p.to_string_lossy().replace('\\', "/");
            if !ref_prefix.is_empty() && !forward.starts_with(&ref_prefix) {
                continue;
            }
            let under = if ref_prefix.is_empty() {
                forward.clone()
            } else {
                forward[ref_prefix.len()..].to_string()
            };
            if under.is_empty() {
                continue;
            }
            // Skip any path with a *.assets directory component.
            let has_assets = Path::new(&forward).components().any(|c| {
                c.as_os_str()
                    .to_str()
                    .is_some_and(|s| s.ends_with(".assets"))
            });
            if has_assets {
                continue;
            }
            ref_notes.push(forward);
        }
        ref_notes.sort();

        // Case-insensitive stem map.
        let mut stem_map: HashMap<String, Vec<String>> = HashMap::new();
        for note in &ref_notes {
            if let Some(stem) =
                Path::new(note).file_stem().and_then(|s| s.to_str())
            {
                stem_map
                    .entry(stem.to_ascii_lowercase())
                    .or_default()
                    .push(note.clone());
            }
        }
        for paths in stem_map.values_mut() {
            paths.sort();
        }
        let ref_set: std::collections::BTreeSet<String> =
            ref_notes.iter().cloned().collect();

        // Read + prefilter in parallel, merge back into path order.
        let pairs: Vec<(PathBuf, PathBuf)> = paths
            .iter()
            .map(|rel| (bob_dir.join(rel), rel.clone()))
            .collect();
        let contents_map = read_prefiltered_parallel(&pairs);

        // Order pairs in path order for determinism.
        let mut ordered: Vec<(PathBuf, String)> =
            contents_map.into_iter().collect();
        ordered.sort_by(|a, b| a.0.cmp(&b.0));

        // Lazy capture routes.
        let mut routes: Option<std::collections::BTreeSet<String>> = None;
        let mut routes_for = |need: bool,
                              bob_dir: &Path|
         -> std::collections::BTreeSet<String> {
            if !need {
                return std::collections::BTreeSet::new();
            }
            if routes.is_none() {
                let report =
                    crate::native::capture_targets::scan_capture_targets(
                        bob_dir,
                    );
                routes = Some(
                    report
                        .targets
                        .iter()
                        .map(|t| t.route.to_ascii_lowercase())
                        .collect(),
                );
            }
            routes.clone().unwrap_or_default()
        };

        // First pass: find whether any candidate lives in a root file, to
        // decide laziness. We compute contents first, then check root.
        let mut candidates: BTreeMap<String, Vec<LocatedRefTask>> =
            BTreeMap::new();
        let mut follow_ups: BTreeMap<String, Vec<RefFollowUp>> =
            BTreeMap::new();
        let mut orphans: Vec<OrphanRefTask> = Vec::new();

        // Pre-scan for root candidates to trigger lazy routes once.
        let mut need_routes = false;
        for (rel, _contents) in &ordered {
            let forward = rel.to_string_lossy().replace('\\', "/");
            if !forward.contains('/') {
                // Root file could hold a candidate; check cheaply later.
                need_routes = true;
                break;
            }
        }
        let route_set = if need_routes {
            routes_for(true, bob_dir)
        } else {
            std::collections::BTreeSet::new()
        };

        for (rel, contents) in &ordered {
            let forward = rel.to_string_lossy().replace('\\', "/");
            let lines: Vec<&str> = contents.lines().collect();
            let front_end =
                crate::native::markdown::strictly_closed_frontmatter_end(
                    &lines,
                );
            let fenced =
                crate::native::markdown::fenced_lines(&lines, 0..lines.len());
            let archived = forward.starts_with("done/");
            let is_root = !forward.contains('/');
            let file_stem = Path::new(&forward)
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("")
                .to_string();
            // done/ residence from frontmatter parent.
            let done_residence = if archived {
                done_parent_stem(contents)
            } else {
                None
            };
            for (line_index, raw) in lines.iter().enumerate() {
                if front_end.is_some_and(|end| line_index <= end) {
                    continue;
                }
                if fenced.contains(&line_index) {
                    continue;
                }
                let stripped = strip_blockquote_prefix(raw);
                let Some(mark) = task_mark(stripped) else {
                    continue;
                };
                // Follow-ups on any task line.
                for (target_part, frag) in follow_up_links(stripped) {
                    if frag.is_empty() || !frag.starts_with("^h-") {
                        continue;
                    }
                    // Resolve target part the same way as candidates.
                    if let Some(key) = resolve_target_to_ref(
                        &target_part,
                        &ref_prefix,
                        &ref_set,
                        &stem_map,
                    ) {
                        // Same-note links never attributed elsewhere.
                        if target_part.is_empty() {
                            continue;
                        }
                        if key == forward {
                            continue;
                        }
                        // Only when the line's file is not that ref note.
                        if forward == key {
                            continue;
                        }
                        if let Some((_, text, block_id)) =
                            parse_follow_up_task(stripped)
                        {
                            follow_ups.entry(key).or_default().push(RefFollowUp {
                                path: forward.clone(),
                                line_index,
                                mark,
                                text,
                                block_id: if block_id.is_empty() {
                                    // Fall back to trailing block id.
                                    crate::native::collect_done::trailing_block_id_in_line(
                                        stripped,
                                    )
                                    .unwrap_or_default()
                                } else {
                                    block_id
                                },
                            });
                        }
                    }
                }
                // v2 candidate?
                if !is_task_line(stripped) {
                    continue;
                }
                if is_embed_task_line(stripped) {
                    continue;
                }
                // Managed embed lines are ignored (they are not tasks).
                if stripped.trim_start().starts_with("![[") {
                    continue;
                }
                if has_caret_ref(stripped) {
                    continue;
                }
                if !has_ref_token(stripped) {
                    continue;
                }
                // Resolve first wikilink.
                let spans =
                    crate::native::task_dependencies::raw_wikilink_spans(
                        stripped,
                    );
                let Some(first) = spans.first() else {
                    orphans.push(OrphanRefTask {
                        path: forward.clone(),
                        line_index,
                        reason: "no_link".to_string(),
                        target: String::new(),
                    });
                    continue;
                };
                let inside = &stripped[first.open + 2..first.end - 2];
                let target_raw = inside
                    .split('|')
                    .next()
                    .unwrap_or("")
                    .split('#')
                    .next()
                    .unwrap_or("")
                    .trim()
                    .to_string();
                if target_raw.contains('/') || target_raw.contains('\\') {
                    // Path-qualified.
                    let normalized =
                        crate::native::vault_links::target_to_markdown_path(
                            &target_raw,
                        );
                    let Some(norm) = normalized else {
                        orphans.push(OrphanRefTask {
                            path: forward.clone(),
                            line_index,
                            reason: "not_a_ref_link".to_string(),
                            target: target_raw.clone(),
                        });
                        continue;
                    };
                    let norm_forward =
                        norm.to_string_lossy().replace('\\', "/");
                    if !ref_prefix.is_empty()
                        && !norm_forward.starts_with(&ref_prefix)
                    {
                        orphans.push(OrphanRefTask {
                            path: forward.clone(),
                            line_index,
                            reason: "not_a_ref_link".to_string(),
                            target: target_raw.clone(),
                        });
                        continue;
                    }
                    // Under ref dir.
                    if ref_set.contains(&norm_forward) {
                        push_candidate(
                            &mut candidates,
                            &norm_forward,
                            &forward,
                            line_index,
                            raw,
                            mark,
                            archived,
                            is_root,
                            &file_stem,
                            &route_set,
                            &done_residence,
                        );
                    } else {
                        // Case-insensitive match against ref-note set.
                        let lowered = norm_forward.to_ascii_lowercase();
                        let found = ref_set
                            .iter()
                            .find(|p| p.to_ascii_lowercase() == lowered)
                            .cloned();
                        if let Some(key) = found {
                            push_candidate(
                                &mut candidates,
                                &key,
                                &forward,
                                line_index,
                                raw,
                                mark,
                                archived,
                                is_root,
                                &file_stem,
                                &route_set,
                                &done_residence,
                            );
                        } else {
                            // Keyed for adoption + orphan.
                            push_candidate(
                                &mut candidates,
                                &norm_forward,
                                &forward,
                                line_index,
                                raw,
                                mark,
                                archived,
                                is_root,
                                &file_stem,
                                &route_set,
                                &done_residence,
                            );
                            orphans.push(OrphanRefTask {
                                path: forward.clone(),
                                line_index,
                                reason: "no_ref_note".to_string(),
                                target: target_raw.clone(),
                            });
                        }
                    }
                } else if target_raw.is_empty() {
                    orphans.push(OrphanRefTask {
                        path: forward.clone(),
                        line_index,
                        reason: "no_link".to_string(),
                        target: String::new(),
                    });
                } else {
                    // Bare stem.
                    let lowered = target_raw.to_ascii_lowercase();
                    match stem_map.get(&lowered) {
                        Some(paths) if paths.len() == 1 => {
                            push_candidate(
                                &mut candidates,
                                &paths[0],
                                &forward,
                                line_index,
                                raw,
                                mark,
                                archived,
                                is_root,
                                &file_stem,
                                &route_set,
                                &done_residence,
                            );
                        }
                        Some(paths) => {
                            orphans.push(OrphanRefTask {
                                path: forward.clone(),
                                line_index,
                                reason: "ambiguous_stem".to_string(),
                                target: format!(
                                    "{} ({})",
                                    target_raw,
                                    paths.join(", ")
                                ),
                            });
                        }
                        None => {
                            orphans.push(OrphanRefTask {
                                path: forward.clone(),
                                line_index,
                                reason: "no_ref_note".to_string(),
                                target: target_raw.clone(),
                            });
                        }
                    }
                }
            }
        }

        // Sort candidates and follow-ups deterministically.
        for tasks in candidates.values_mut() {
            tasks.sort_by(|a, b| {
                a.path.cmp(&b.path).then(a.line_index.cmp(&b.line_index))
            });
        }
        for tasks in follow_ups.values_mut() {
            tasks.sort_by(|a, b| {
                a.path.cmp(&b.path).then(a.line_index.cmp(&b.line_index))
            });
        }
        orphans.sort_by(|a, b| {
            a.path.cmp(&b.path).then(a.line_index.cmp(&b.line_index))
        });

        Self {
            candidates,
            follow_ups,
            orphans,
            warnings,
        }
    }

    /// This ref's v2 candidates.
    pub(crate) fn candidates(&self, ref_path: &str) -> &[LocatedRefTask] {
        self.candidates
            .get(ref_path)
            .map(Vec::as_slice)
            .unwrap_or(&[])
    }

    /// Follow-ups attributed to this ref from elsewhere.
    pub(crate) fn follow_ups(&self, ref_path: &str) -> &[RefFollowUp] {
        self.follow_ups
            .get(ref_path)
            .map(Vec::as_slice)
            .unwrap_or(&[])
    }

    /// Every orphan task line.
    pub(crate) fn orphans(&self) -> &[OrphanRefTask] {
        &self.orphans
    }

    /// Walk problems.
    pub(crate) fn warnings(&self) -> &[String] {
        &self.warnings
    }

    /// Pure selection for one ref note.
    pub(crate) fn select(
        &self,
        ref_path: &str,
        v1_hits: &[TrackerHit],
    ) -> RefTaskSelection {
        select_for_ref(self.candidates(ref_path), v1_hits)
    }

    /// The empty selection for a path with no candidates and no hits
    /// (used by `migrate_zorg` and unreadable notes).
    pub(crate) fn build_empty_selection() -> RefTaskSelection {
        crate::native::ref_tasks::RefTaskSelection::empty()
    }
}

#[allow(clippy::too_many_arguments)]
fn push_candidate(
    candidates: &mut BTreeMap<String, Vec<LocatedRefTask>>,
    key: &str,
    file: &str,
    line_index: usize,
    raw: &str,
    mark: char,
    archived: bool,
    is_root: bool,
    file_stem: &str,
    routes: &std::collections::BTreeSet<String>,
    done_residence: &Option<String>,
) {
    let raw_line = (*raw).to_string();
    let (residence, in_capture_target) = if is_root {
        let in_target = routes.contains(&file_stem.to_ascii_lowercase());
        (Some(file_stem.to_string()), in_target)
    } else if archived {
        (done_residence.clone(), false)
    } else {
        (None, false)
    };
    let block_id =
        crate::native::collect_done::trailing_block_id_in_line(&raw_line);
    let closed_on = dates::close_date(&raw_line);
    candidates
        .entry(key.to_string())
        .or_default()
        .push(LocatedRefTask {
            path: file.to_string(),
            line_index,
            line: raw_line.trim_end_matches(['\n', '\r']).to_string(),
            mark,
            block_id,
            archived,
            residence,
            closed_on,
            in_capture_target,
        });
}

fn done_parent_stem(contents: &str) -> Option<String> {
    // Read from the already-loaded contents with `collect_done`'s
    // `archive_parent_target` (promoted to `pub(crate)` with a re-export).
    let target = crate::native::collect_done::archive_parent_target(contents)?;
    // Stem: last path segment without .md.
    let last = target.rsplit('/').next().unwrap_or(&target);
    let stem = last
        .strip_suffix(".md")
        .or_else(|| last.strip_suffix(".MD"))
        .unwrap_or(last);
    (!stem.is_empty()).then(|| stem.to_string())
}

/// Every `[[target|🔖]]` link on the line whose alias is exactly `🔖`,
/// as `(target_part, fragment_after_#)`.
fn follow_up_links(line: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for span in crate::native::task_dependencies::raw_wikilink_spans(line) {
        let inside = &line[span.open + 2..span.end - 2];
        let mut parts = inside.split('|');
        let target_frag = parts.next().unwrap_or("").trim();
        let alias = parts.next().unwrap_or("").trim();
        // Alias must be exactly 🔖 (no extra aliases considered).
        if alias != "🔖" {
            continue;
        }
        let (target, frag) = match target_frag.find('#') {
            Some(pos) => (
                target_frag[..pos].trim().to_string(),
                target_frag[pos + 1..].trim().to_string(),
            ),
            None => (target_frag.trim().to_string(), String::new()),
        };
        // Fragment must be #^h-…
        if !frag.starts_with("^h-") {
            continue;
        }
        out.push((target, frag));
    }
    out
}

fn resolve_target_to_ref(
    target_part: &str,
    ref_prefix: &str,
    ref_set: &std::collections::BTreeSet<String>,
    stem_map: &HashMap<String, Vec<String>>,
) -> Option<String> {
    if target_part.is_empty() {
        return None;
    }
    if target_part.contains('/') || target_part.contains('\\') {
        let normalized =
            crate::native::vault_links::target_to_markdown_path(target_part)?;
        let forward = normalized.to_string_lossy().replace('\\', "/");
        if !ref_prefix.is_empty() && !forward.starts_with(ref_prefix) {
            return None;
        }
        if ref_set.contains(&forward) {
            return Some(forward);
        }
        let lowered = forward.to_ascii_lowercase();
        if let Some(found) =
            ref_set.iter().find(|p| p.to_ascii_lowercase() == lowered)
        {
            return Some(found.clone());
        }
        return None;
    }
    let lowered = target_part.to_ascii_lowercase();
    match stem_map.get(&lowered) {
        Some(paths) if paths.len() == 1 => Some(paths[0].clone()),
        _ => None,
    }
}

fn contains_ref_marker(bytes: &[u8]) -> bool {
    // 🔖 bytes.
    if memchr::memmem::find(bytes, "🔖".as_bytes()).is_some() {
        return true;
    }
    // ASCII case-insensitive #ref / ^ref.
    let mut i = 0;
    while i + 4 <= bytes.len() {
        let b = bytes[i];
        if b == b'#' || b == b'^' {
            if bytes[i + 1].eq_ignore_ascii_case(&b'r')
                && bytes[i + 2].eq_ignore_ascii_case(&b'e')
                && bytes[i + 3].eq_ignore_ascii_case(&b'f')
            {
                return true;
            }
        }
        i += 1;
    }
    false
}

fn read_prefiltered_one(
    pair: &(PathBuf, PathBuf),
) -> Option<(PathBuf, String)> {
    let bytes = std::fs::read(&pair.0).ok()?;
    if !contains_ref_marker(&bytes) {
        return None;
    }
    let text = String::from_utf8(bytes).ok()?;
    Some((pair.1.clone(), text))
}

fn read_prefiltered_parallel(
    pairs: &[(PathBuf, PathBuf)],
) -> Vec<(PathBuf, String)> {
    if pairs.is_empty() {
        return Vec::new();
    }
    let worker_count = std::thread::available_parallelism()
        .map(|p| p.get())
        .unwrap_or(4)
        .min(pairs.len())
        .max(1);
    if worker_count <= 1 {
        return pairs.iter().filter_map(read_prefiltered_one).collect();
    }
    let next = AtomicUsize::new(0);
    let collected: Vec<Vec<(PathBuf, String)>> = std::thread::scope(|scope| {
        let handles: Vec<_> = (0..worker_count)
            .map(|_| {
                scope.spawn(|| {
                    let mut local = Vec::new();
                    loop {
                        let index = next.fetch_add(1, Ordering::Relaxed);
                        let Some(pair) = pairs.get(index) else {
                            break;
                        };
                        if let Some(entry) = read_prefiltered_one(pair) {
                            local.push(entry);
                        }
                    }
                    local
                })
            })
            .collect();
        handles
            .into_iter()
            .map(|h| h.join().expect("snapshot worker panicked"))
            .collect()
    });
    collected.into_iter().flatten().collect()
}
