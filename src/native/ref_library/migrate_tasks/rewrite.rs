//! Vault-wide link and dependency-id rewrite preview.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use super::plan::{AmbiguousLink, FileRewrite, PossibleWrapper};
use crate::native::ref_tasks::is_open_mark;

/// Excluded vault prefixes for the graph rewrite.
fn is_excluded(rel: &str) -> bool {
    if rel.starts_with('.') {
        return true;
    }
    for comp in rel.split('/') {
        if comp.starts_with('.') {
            return true;
        }
    }
    if rel.contains("_generated/")
        || rel.starts_with("_generated/")
        || rel.contains("_templates/")
        || rel.starts_with("_templates/")
        || rel.contains("_conflicts/")
        || rel.starts_with("_conflicts/")
    {
        return true;
    }
    if rel.starts_with("lib/") || rel.contains("/lib/") {
        // `lib/` first component only; keep it simple: any `lib/` prefix.
        if rel.starts_with("lib/") {
            return true;
        }
    }
    if rel.starts_with("xlib/") {
        return true;
    }
    if crate::native::ref_tasks::is_conflict_copy_name(
        rel.rsplit('/').next().unwrap_or(rel),
    ) {
        return true;
    }
    false
}

fn vault_markdown_files(bob_dir: &Path) -> Vec<String> {
    let mut out = Vec::new();
    let mut stack = vec![bob_dir.to_path_buf()];
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
                let forward = rel.to_string_lossy().replace('\\', "/");
                if is_excluded(&forward) {
                    continue;
                }
                out.push(forward);
            }
        }
    }
    out.sort();
    out
}

/// Preview rewrites with preview ids.
///
/// `id_map`: ref_note -> (parent_route, new_block_id).
/// `dep_map`: old_dep_id -> new_dep_id.
/// `stem_counts`: lowercase stem -> note count (for bare-stem ambiguity).
pub(crate) fn preview_rewrites(
    bob_dir: &Path,
    id_map: &BTreeMap<String, (String, String)>,
    dep_map: &BTreeMap<String, String>,
    stem_counts: &BTreeMap<String, usize>,
) -> (
    BTreeMap<String, FileRewrite>,
    Vec<AmbiguousLink>,
    Vec<PossibleWrapper>,
) {
    let mut rewrites: BTreeMap<String, FileRewrite> = BTreeMap::new();
    let mut ambiguous = Vec::new();
    let mut wrappers = Vec::new();

    // Build stem -> single ref_note for bare-stem resolution.
    let mut stem_to_ref: BTreeMap<String, String> = BTreeMap::new();
    for ref_note in id_map.keys() {
        let stem = Path::new(ref_note)
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        if stem_counts.get(&stem).copied().unwrap_or(0) == 1 {
            stem_to_ref.insert(stem, ref_note.clone());
        }
    }

    // Reverse: normalized target path -> ref_note.
    let mut path_to_ref: BTreeMap<String, String> = BTreeMap::new();
    for ref_note in id_map.keys() {
        path_to_ref.insert(ref_note.to_ascii_lowercase(), ref_note.clone());
        // Without md.
        let no_md = ref_note
            .strip_suffix(".md")
            .or_else(|| ref_note.strip_suffix(".MD"))
            .unwrap_or(ref_note);
        path_to_ref.insert(no_md.to_ascii_lowercase(), ref_note.clone());
        // Without ref/ prefix? Path-qualified targets are `ref/...`.
        // Keep as is; resolution handles both via target_to_markdown_path.
    }

    for file in vault_markdown_files(bob_dir) {
        let abs = bob_dir.join(&file);
        let Ok(bytes) = std::fs::read(&abs) else {
            continue;
        };
        let Ok(contents) = String::from_utf8(bytes) else {
            continue;
        };
        // Prefilter: files with `^ref` or an old dep id.
        let has_caret = contents.contains("^ref");
        let has_dep = dep_map.keys().any(|old| contents.contains(old));
        if !has_caret && !has_dep {
            continue;
        }
        let lines: Vec<&str> = contents.lines().collect();
        let pomodoro_range =
            crate::native::pomodoro::pomodoros_section_range(&lines);

        for (idx, line) in lines.iter().enumerate() {
            // Links on this line.
            let spans =
                crate::native::task_dependencies::raw_wikilink_spans(line);
            for span in &spans {
                let inside = &line[span.open + 2..span.end - 2];
                let Some((target, block_id)) =
                    crate::native::task_dependencies::parse_block_link_inside(
                        inside,
                    )
                else {
                    continue;
                };
                if block_id != "ref" {
                    continue;
                }
                // Resolve target to a migrated ref.
                let resolved = resolve_link_target(
                    &target,
                    &path_to_ref,
                    &stem_to_ref,
                    stem_counts,
                );
                match resolved {
                    LinkResolution::Migrated(ref_note) => {
                        let entry = rewrites.entry(file.clone()).or_default();
                        entry.links += 1;
                        // Wrapper check: open task line linking migrated ref,
                        // excluding pomodoro links and Depends-On lines.
                        if is_open_task_line(line) {
                            let in_pomodoro = pomodoro_range
                                .as_ref()
                                .is_some_and(|r| r.contains(&idx));
                            let is_dep_line = is_depends_on_line(line);
                            if !in_pomodoro && !is_dep_line {
                                wrappers.push(PossibleWrapper {
                                    file: file.clone(),
                                    line: idx + 1,
                                });
                            }
                        }
                        let _ = ref_note;
                    }
                    LinkResolution::Ambiguous(display) => {
                        ambiguous.push(AmbiguousLink {
                            file: file.clone(),
                            link: display,
                        });
                    }
                    LinkResolution::Other => {}
                }
            }
            // Markdown links [...](target): check for #^ref fragment.
            for md_target in markdown_link_targets(line) {
                if !md_target.contains("#^ref") {
                    continue;
                }
                let (target_part, frag) = split_md_target(&md_target);
                if frag != "^ref" {
                    continue;
                }
                match resolve_link_target(
                    &target_part,
                    &path_to_ref,
                    &stem_to_ref,
                    stem_counts,
                ) {
                    LinkResolution::Migrated(_) => {
                        rewrites.entry(file.clone()).or_default().links += 1;
                    }
                    LinkResolution::Ambiguous(display) => {
                        ambiguous.push(AmbiguousLink {
                            file: file.clone(),
                            link: display,
                        });
                    }
                    LinkResolution::Other => {}
                }
            }
        }
        // Dependency ids on this file.
        if has_dep {
            let count = dep_map
                .keys()
                .map(|old| contents.matches(old).count())
                .sum::<usize>();
            if count > 0 {
                rewrites.entry(file.clone()).or_default().deps += count;
            }
        }
    }

    wrappers.sort_by(|a, b| a.file.cmp(&b.file).then(a.line.cmp(&b.line)));
    ambiguous.sort_by(|a, b| a.file.cmp(&b.file).then(a.link.cmp(&b.link)));
    (rewrites, ambiguous, wrappers)
}

enum LinkResolution {
    Migrated(String),
    Ambiguous(String),
    Other,
}

fn resolve_link_target(
    target: &str,
    path_to_ref: &BTreeMap<String, String>,
    stem_to_ref: &BTreeMap<String, String>,
    stem_counts: &BTreeMap<String, usize>,
) -> LinkResolution {
    let t = target.trim();
    if t.is_empty() {
        return LinkResolution::Other;
    }
    if t.contains('/') {
        // Path-qualified: normalize to markdown path.
        if let Some(path) =
            crate::native::vault_links::target_to_markdown_path(t)
        {
            let forward = path.to_string_lossy().replace('\\', "/");
            if let Some(ref_note) =
                path_to_ref.get(&forward.to_ascii_lowercase())
            {
                return LinkResolution::Migrated(ref_note.clone());
            }
            // Case-insensitive fallback already covered by lowercased map.
            return LinkResolution::Other;
        }
        return LinkResolution::Other;
    }
    // Bare stem: only when exactly one ref note has that stem.
    let lower = t
        .strip_suffix(".md")
        .or_else(|| t.strip_suffix(".MD"))
        .unwrap_or(t)
        .to_ascii_lowercase();
    if let Some(ref_note) = stem_to_ref.get(&lower) {
        return LinkResolution::Migrated(ref_note.clone());
    }
    if stem_counts.get(&lower).copied().unwrap_or(0) > 1 {
        return LinkResolution::Ambiguous(target.trim().to_string());
    }
    LinkResolution::Other
}

/// Apply rewrites to one file's contents with final ids.
///
/// Returns `(new_contents, links_rewritten, deps_rewritten)`.
pub(crate) fn apply_rewrites_to_contents(
    contents: &str,
    file: &str,
    bob_dir: &Path,
    final_map: &BTreeMap<String, (String, String)>,
    dep_map: &BTreeMap<String, String>,
    stem_counts: &BTreeMap<String, usize>,
) -> (String, usize, usize) {
    // Build lookup helpers identical to preview.
    let mut stem_to_ref: BTreeMap<String, String> = BTreeMap::new();
    for ref_note in final_map.keys() {
        let stem = Path::new(ref_note)
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        if stem_counts.get(&stem).copied().unwrap_or(0) == 1 {
            stem_to_ref.insert(stem, ref_note.clone());
        }
    }
    let mut path_to_ref: BTreeMap<String, String> = BTreeMap::new();
    for ref_note in final_map.keys() {
        path_to_ref.insert(ref_note.to_ascii_lowercase(), ref_note.clone());
        let no_md = ref_note
            .strip_suffix(".md")
            .or_else(|| ref_note.strip_suffix(".MD"))
            .unwrap_or(ref_note);
        path_to_ref.insert(no_md.to_ascii_lowercase(), ref_note.clone());
    }

    let mut link_count = 0usize;
    let lines: Vec<&str> = contents.lines().collect();
    let mut new_lines: Vec<String> = Vec::with_capacity(lines.len());
    for line in &lines {
        let mut out = line.to_string();
        // Wikilinks.
        let spans = crate::native::task_dependencies::raw_wikilink_spans(&out);
        // Apply back-to-front so offsets stay valid.
        let mut edits: Vec<(usize, usize, String)> = Vec::new();
        for span in &spans {
            let inside = &out[span.open + 2..span.end - 2];
            let Some((target, block_id)) =
                crate::native::task_dependencies::parse_block_link_inside(
                    inside,
                )
            else {
                continue;
            };
            if block_id != "ref" {
                continue;
            }
            let resolved = resolve_link_target(
                &target,
                &path_to_ref,
                &stem_to_ref,
                stem_counts,
            );
            let LinkResolution::Migrated(ref_note) = resolved else {
                continue;
            };
            let Some((route, new_id)) = final_map.get(&ref_note) else {
                continue;
            };
            // Rebuild inside: `<route>#^<new>` + optional `|alias`.
            let alias = inside.split_once('|').map(|(_, a)| a);
            let new_inside = match alias {
                Some(a) => format!("{route}#^{new_id}|{a}"),
                None => format!("{route}#^{new_id}"),
            };
            edits.push((span.open + 2, span.end - 2, new_inside));
        }
        for (s, e, replacement) in edits.into_iter().rev() {
            out = format!("{}{}{}", &out[..s], replacement, &out[e..]);
        }
        if out != *line {
            // Count wikilink edits on this line.
            link_count += 1;
        }
        // Markdown links: rewrite target part when fragment is ^ref.
        let (md_out, md_n) =
            rewrite_markdown_links(&out, &path_to_ref, &stem_to_ref, final_map);
        out = md_out;
        link_count += md_n;
        new_lines.push(out);
    }
    let mut joined = new_lines.join("\n");
    // Preserve trailing newline.
    if contents.ends_with('\n') {
        joined.push('\n');
    } else if contents.ends_with("\r\n") {
        joined.push_str("\r\n");
    }
    // Dependency ids.
    let (with_deps, dep_count) = rewrite_dependency_ids(&joined, dep_map);
    let _ = file;
    let _ = bob_dir;
    (with_deps, link_count, dep_count)
}

fn markdown_link_targets(line: &str) -> Vec<String> {
    let mut out = Vec::new();
    let bytes = line.as_bytes();
    let mut i = 0usize;
    while i < bytes.len() {
        if bytes[i] == b']' && bytes.get(i + 1) == Some(&b'(') {
            let start = i + 2;
            if let Some(end_rel) = line[start..].find(')') {
                out.push(line[start..start + end_rel].to_string());
                i = start + end_rel + 1;
                continue;
            }
            break;
        }
        i += 1;
    }
    out
}

fn split_md_target(target: &str) -> (String, String) {
    match target.find('#') {
        Some(pos) => (
            target[..pos].trim().to_string(),
            target[pos + 1..].trim().to_string(),
        ),
        None => (target.trim().to_string(), String::new()),
    }
}

fn rewrite_markdown_links(
    line: &str,
    path_to_ref: &BTreeMap<String, String>,
    stem_to_ref: &BTreeMap<String, String>,
    final_map: &BTreeMap<String, (String, String)>,
) -> (String, usize) {
    // Stem counts are not needed for apply (ambiguous links stay); pass
    // an empty map so only unique stems resolve.
    let empty_counts: BTreeMap<String, usize> = BTreeMap::new();
    let _ = &empty_counts;
    let mut out = line.to_string();
    let mut count = 0usize;
    // Find [...](...) occurrences and rewrite back-to-front.
    let mut occurrences: Vec<(usize, usize, String, String)> = Vec::new();
    let bytes = out.as_bytes();
    let mut i = 0usize;
    while i < bytes.len() {
        if bytes[i] == b']' && bytes.get(i + 1) == Some(&b'(') {
            let start = i + 2;
            if let Some(end_rel) = out[start..].find(')') {
                let target = out[start..start + end_rel].to_string();
                if target.contains("#^ref") {
                    let (tpart, frag) = split_md_target(&target);
                    if frag == "^ref" {
                        if let LinkResolution::Migrated(ref_note) =
                            resolve_link_target(
                                &tpart,
                                path_to_ref,
                                stem_to_ref,
                                &empty_counts,
                            )
                        {
                            if let Some((route, new_id)) =
                                final_map.get(&ref_note)
                            {
                                occurrences.push((
                                    start,
                                    start + end_rel,
                                    target,
                                    format!("{route}#^{new_id}"),
                                ));
                            }
                        }
                    }
                }
                i = start + end_rel + 1;
                continue;
            }
            break;
        }
        i += 1;
    }
    for (s, e, _, replacement) in occurrences.into_iter().rev() {
        out = format!("{}{}{}", &out[..s], replacement, &out[e..]);
        count += 1;
    }
    (out, count)
}

fn rewrite_dependency_ids(
    contents: &str,
    dep_map: &BTreeMap<String, String>,
) -> (String, usize) {
    if dep_map.is_empty() {
        return (contents.to_string(), 0);
    }
    let mut out = contents.to_string();
    let mut total = 0usize;
    for (old, new) in dep_map {
        // Match `[id:: old]` / `[dependsOn:: old]` with flexible spacing.
        // Simple substring replacement inside dependency fields is safe
        // because old ids are path-derived and unique.
        let before = out.clone();
        out = replace_dep_value(&out, old, new);
        if out != before {
            total += before.matches(old).count();
        }
    }
    (out, total)
}

fn replace_dep_value(contents: &str, old: &str, new: &str) -> String {
    // Replace only inside [id:: ...] / [dependsOn:: ...] fields.
    let mut result = String::with_capacity(contents.len());
    let mut rest = contents;
    loop {
        // Find next [id:: or [dependsOn::
        let id_pos = rest.find("[id::");
        let dep_pos = rest.find("[dependsOn::");
        let next = match (id_pos, dep_pos) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (Some(a), None) => Some(a),
            (None, Some(b)) => Some(b),
            (None, None) => None,
        };
        let Some(pos) = next else {
            result.push_str(rest);
            break;
        };
        // Find closing ].
        let after_open = &rest[pos..];
        let Some(close_rel) = after_open.find(']') else {
            result.push_str(rest);
            break;
        };
        result.push_str(&rest[..pos]);
        let field = &rest[pos..pos + close_rel + 1];
        if field.contains(old) {
            result.push_str(&field.replace(old, new));
        } else {
            result.push_str(field);
        }
        rest = &rest[pos + close_rel + 1..];
    }
    // Also handle (id:: ...) paren form.
    let mut second = result.clone();
    for open in ["(id::", "(dependsOn::"] {
        if second.contains(open) && second.contains(old) {
            second = second.replace(old, new);
        }
    }
    second
}

fn is_open_task_line(line: &str) -> bool {
    let stripped = strip_blockquote(line);
    let t = stripped.trim_start_matches([' ', '\t']);
    let after = t
        .strip_prefix("- ")
        .or_else(|| t.strip_prefix("* "))
        .or_else(|| t.strip_prefix("+ "));
    let Some(after) = after else {
        return false;
    };
    let Some(bracket) = after.strip_prefix('[') else {
        return false;
    };
    let Some(mark) = bracket.chars().next() else {
        return false;
    };
    let rest = &bracket[mark.len_utf8()..];
    if !rest.starts_with(']') {
        return false;
    }
    is_open_mark(mark)
}

fn is_depends_on_line(line: &str) -> bool {
    // A child bullet whose text starts with `Depends-On`.
    let stripped = strip_blockquote(line);
    let t = stripped.trim_start();
    let after = t
        .strip_prefix("- ")
        .or_else(|| t.strip_prefix("* "))
        .or_else(|| t.strip_prefix("+ "));
    let Some(after) = after else {
        return false;
    };
    // Skip checkbox `[m] `.
    let after = if after.starts_with('[') {
        if let Some(close) = after.find(']') {
            after[close + 1..].trim_start()
        } else {
            after
        }
    } else {
        after
    };
    after.starts_with("Depends-On")
}

fn strip_blockquote(mut line: &str) -> &str {
    loop {
        let spaces = line.bytes().take_while(|b| *b == b' ').count();
        if spaces > 3 || line.as_bytes().get(spaces) != Some(&b'>') {
            return line;
        }
        line = &line[spaces + 1..];
        if let Some(rest) = line.strip_prefix(' ') {
            line = rest;
        }
    }
}

/// Vault-relative markdown paths for dependency rewriting (test helper).
#[allow(dead_code)]
pub(crate) fn vault_paths_for_test(paths: Vec<PathBuf>) -> BTreeSet<String> {
    paths
        .into_iter()
        .map(|p| p.to_string_lossy().replace('\\', "/"))
        .collect()
}
