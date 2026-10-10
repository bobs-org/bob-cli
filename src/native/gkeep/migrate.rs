//! `bob gkeep migrate-markers`: offline marker cleanup.
//!
//! Removes supported `%%gkeep:…%%` markers from task Markdown while
//! preserving import evidence in `.bob/gkeep/imports/`. Needs no
//! credentials, adapter, Keep snapshot, or network access, and does not
//! require a configured target note. Operates on eligible vault task
//! blocks, including completed tasks and tasks moved to other notes,
//! using existing walker exclusions.

use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, File, OpenOptions},
    io,
    path::{Path, PathBuf},
};

use fs2::FileExt;
use serde_json::json;

use super::{
    imports::{self, EntryState, ImportEntry, TransactionFile},
    ledger::parse_markers,
    render::parse_source_url,
    ui,
};
use super::{GkeepError, MigrateArgs};
use crate::native::{env as bob_env, note_tasks, ob, style::Styler};

fn pull_lock_path() -> PathBuf {
    bob_env::bob_cli_state_dir().join("gkeep").join("pull.lock")
}

/// Whether `dir` or any ancestor contains a `.git` entry.
fn has_git_ancestor(dir: &Path) -> bool {
    let mut current = Some(dir);
    while let Some(path) = current {
        let dot_git = path.join(".git");
        if dot_git.is_dir() || dot_git.is_file() {
            return true;
        }
        current = path.parent();
    }
    false
}

fn acquire_pull_lock() -> Result<File, GkeepError> {
    let path = pull_lock_path();
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
        && let Err(error) = fs::create_dir_all(parent)
    {
        return Err(GkeepError::runtime(
            "lock",
            format!("create pull lock dir {}: {error}", parent.display()),
        ));
    }
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(&path)
        .map_err(|error| {
            GkeepError::runtime(
                "lock",
                format!("open pull lock {}: {error}", path.display()),
            )
        })?;
    match file.try_lock_exclusive() {
        Ok(()) => Ok(file),
        Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
            Err(GkeepError::runtime(
                "lock",
                "another bob gkeep pull is already running".to_string(),
            ))
        }
        Err(error) => Err(GkeepError::runtime(
            "lock",
            format!("lock {}: {error}", path.display()),
        )),
    }
}

/// One recognized marker occurrence.
#[derive(Debug, Clone)]
struct MarkerHit {
    line_index: usize,
    id: String,
    fp: String,
    kind: MarkerKind,
    /// Byte range of the marker token within the line (for Source).
    token_start: usize,
    token_end: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MarkerKind {
    Standalone,
    Source,
}

/// Per-file migration plan.
#[derive(Debug)]
struct FilePlan {
    rel: String,
    path: PathBuf,
    before_bytes: Vec<u8>,
    /// Line ending: `\r\n` when the file uses CRLF, else `\n`.
    crlf: bool,
    has_final_newline: bool,
    perms: fs::Permissions,
    /// Recognized hits grouped by task block.
    hits: Vec<MarkerHit>,
    /// Task-block line ranges for ambiguity detection.
    blocks: Vec<(usize, usize)>,
    /// Human-readable skips (ambiguous, malformed outside positions).
    skips: Vec<String>,
}

pub(crate) fn run(args: &MigrateArgs) -> i32 {
    let is_json = args.format.is_json();
    let styler = Styler::detect();
    let format_name = args.error_format();

    // Dry runs take no locks, write nothing, create no metadata.
    let _pull_guard = if args.dry_run {
        None
    } else {
        match acquire_pull_lock() {
            Ok(guard) => Some(guard),
            Err(error) => {
                return ui::report_error(
                    "migrate-markers",
                    &error,
                    format_name,
                );
            }
        }
    };

    let bob_dir = args.bob_dir();
    // Offline: no config, adapter, snapshot, or target requirement.

    // Malformed store files fail before any mutation.
    let store_files = match imports::read_all(&bob_dir) {
        Ok(files) => files,
        Err(error) => {
            return ui::report_error(
                "migrate-markers",
                &GkeepError::runtime(
                    "vault",
                    format!("read the gkeep import store: {error}"),
                ),
                format_name,
            );
        }
    };
    let verified: BTreeSet<(String, String)> =
        imports::verified_pairs(&store_files).into_iter().collect();

    let mut file_plans = match scan_candidates(&bob_dir) {
        Ok(plans) => plans,
        Err(error) => {
            return ui::report_error("migrate-markers", &error, format_name)
        }
    };
    // Stable order for deterministic output and commits.
    file_plans.sort_by(|a, b| a.rel.cmp(&b.rel));

    if args.dry_run {
        return print_report(args, &file_plans, None, &[], &styler, true);
    }

    // Real migration takes the shared maintenance lock in the same order
    // as pull (pull lock already held, then vault lock).
    let _vault_guard = {
        let waiting = format!(
            "  {}",
            styler.dim("waiting for another vault maintenance run…")
        );
        let json_mode = is_json;
        let on_first_wait = move || {
            if json_mode {
                eprintln!("waiting for another vault maintenance run…");
            } else {
                eprintln!("{waiting}");
            }
        };
        match ob::acquire_lock_waiting(
            std::time::Duration::from_secs(60),
            on_first_wait,
        ) {
            Ok(guard) => guard,
            Err(error) => {
                return ui::report_error(
                    "migrate-markers",
                    &GkeepError::runtime(
                        "lock",
                        format!("acquire vault maintenance lock: {error}"),
                    ),
                    format_name,
                );
            }
        }
    };

    // Re-scan under the vault lock so newly synced markers cannot be
    // bypassed; dry-run produced no writes.
    file_plans = match scan_candidates(&bob_dir) {
        Ok(plans) => {
            let mut plans = plans;
            plans.sort_by(|a, b| a.rel.cmp(&b.rel));
            plans
        }
        Err(error) => {
            return ui::report_error("migrate-markers", &error, format_name)
        }
    };

    let mut changed_rel: Vec<PathBuf> = Vec::new();
    let mut tx_rel_paths: Vec<PathBuf> = Vec::new();
    let mut failures: Vec<(String, String)> = Vec::new();
    let mut removed_total = 0usize;

    for plan in &file_plans {
        if plan.hits.is_empty() {
            continue;
        }
        // Group hits by task block; ambiguous ownership (distinct
        // (id, fp) in one block) is retained and reported.
        let mut block_hits: BTreeMap<usize, Vec<&MarkerHit>> = BTreeMap::new();
        for hit in &plan.hits {
            let block_index = plan
                .blocks
                .iter()
                .position(|(start, end)| {
                    hit.line_index >= *start && hit.line_index < *end
                })
                .unwrap_or(usize::MAX);
            block_hits.entry(block_index).or_default().push(hit);
        }
        let mut file_ok_hits: Vec<&MarkerHit> = Vec::new();
        for hits in block_hits.values() {
            let mut distinct = BTreeSet::new();
            for hit in hits {
                distinct.insert((hit.id.clone(), hit.fp.clone()));
            }
            if distinct.len() > 1 {
                continue;
            }
            file_ok_hits.extend(hits.iter().copied());
        }
        if file_ok_hits.is_empty() {
            continue;
        }
        // Persist import evidence before removing its only marker, with
        // the old marker as the import proof. Do not reconstruct the
        // Keep fingerprint from a user-edited task. Skip receipts that
        // already exist (safe rerun after a crash before cleanup).
        let settings = note_tasks::read_settings(&bob_dir);
        let after_contents = match apply_cleanup(plan, &file_ok_hits) {
            Some(text) => text,
            None => {
                failures.push((
                    plan.rel.clone(),
                    "failed to render the cleaned note".to_string(),
                ));
                continue;
            }
        };
        let after_sha = imports::sha256_hex(after_contents.as_bytes());
        // Build verified receipts for hits lacking them.
        let mut new_entries: Vec<ImportEntry> = Vec::new();
        // Need after-cleanup block digests per hit: compute task blocks
        // in the cleaned file.
        let cleaned_blocks = task_block_map(&after_contents, &settings);
        for hit in &file_ok_hits {
            if verified.contains(&(hit.id.clone(), hit.fp.clone())) {
                continue;
            }
            // URL from the task description (💡 link) when present; the
            // adapter id stays in `id`, never recovered from the URL.
            let url = source_url_for_hit(plan, hit);
            // Initial block digest: digest of the cleaned task block
            // containing this hit's line (marker-free).
            let block_digest = cleaned_blocks
                .get(&hit.line_index)
                .cloned()
                .unwrap_or_else(|| "0".repeat(64));
            // Original relative path: where the marker was found.
            new_entries.push(ImportEntry {
                id: hit.id.clone(),
                fp: hit.fp.clone(),
                url,
                path: plan.rel.clone(),
                block_digest,
                intended: None,
                state: EntryState::Verified,
                dest_digest: Some(after_sha.clone()),
            });
        }
        // Deduplicate identical new proofs (same id/fp/path/digest).
        {
            let mut seen = BTreeSet::new();
            new_entries.retain(|entry| {
                seen.insert((
                    entry.id.clone(),
                    entry.fp.clone(),
                    entry.path.clone(),
                    entry.block_digest.clone(),
                ))
            });
        }
        let had_new = !new_entries.is_empty();
        let tx_path_opt = if !had_new {
            None
        } else {
            let tx = TransactionFile {
                schema_version: imports::SCHEMA_VERSION,
                transaction_id: imports::new_transaction_id(),
                destination: plan.rel.clone(),
                before_sha256: imports::sha256_hex(&plan.before_bytes),
                after_sha256: Some(after_sha.clone()),
                baseline_counts: BTreeMap::new(),
                entries: std::mem::take(&mut new_entries),
            };
            match imports::persist_prepared(&bob_dir, &tx) {
                Ok(path) => {
                    let rel = path
                        .strip_prefix(&bob_dir)
                        .map(|relative| relative.to_path_buf())
                        .unwrap_or_else(|_| {
                            PathBuf::from(imports::IMPORTS_DIR)
                                .join(path.file_name().unwrap_or_default())
                        });
                    tx_rel_paths.push(rel);
                    Some(path)
                }
                Err(error) => {
                    // Record-persistence failure leaves the marker
                    // recoverable: do not touch the note.
                    failures.push((
                        plan.rel.clone(),
                        format!("persist import evidence: {error}"),
                    ));
                    None
                }
            }
        };
        if had_new && tx_path_opt.is_none() {
            continue;
        }
        // CAS-protected write of the cleaned note. A race leaves the
        // marker recoverable (evidence already persisted; rerun cleans).
        match install_cleaned(plan, &after_contents) {
            Ok(true) => {
                removed_total += file_ok_hits.len();
                changed_rel.push(PathBuf::from(&plan.rel));
            }
            Ok(false) => {
                failures.push((
                    plan.rel.clone(),
                    "the note changed during migration; evidence retained, marker untouched".to_string(),
                ));
            }
            Err(error) => {
                failures.push((plan.rel.clone(), error));
            }
        }
    }

    // Commit only the changed notes and generated records.
    let mut commit_sha: Option<String> = None;
    if !args.no_commit && (!changed_rel.is_empty() || !tx_rel_paths.is_empty())
    {
        let child_env = ob::child_env();
        let worktree = match ob::detect_git_worktree(&bob_dir, &child_env) {
            Ok(inside) => inside,
            Err(error) => {
                if has_git_ancestor(&bob_dir) {
                    return ui::report_error(
                        "migrate-markers",
                        &GkeepError::runtime(
                            "commit",
                            format!("detect the vault Git worktree: {error}"),
                        ),
                        format_name,
                    );
                }
                false
            }
        };
        if worktree {
            let mut paths = changed_rel.clone();
            paths.extend(tx_rel_paths.clone());
            paths.sort();
            paths.dedup();
            if let Err(message) =
                imports::preflight_trackable(&bob_dir, &child_env, &paths)
            {
                return ui::report_error(
                    "migrate-markers",
                    &GkeepError::runtime("commit", message),
                    format_name,
                );
            }
            let message = format!(
                "bob gkeep migrate-markers: {removed_total} markers from Google Keep"
            );
            match ob::commit_paths(&bob_dir, &child_env, &message, &paths) {
                Ok(sha) => commit_sha = sha,
                Err(error) => {
                    failures.push((
                        "commit".to_string(),
                        format!("commit the vault: {error}"),
                    ));
                }
            }
        }
    }

    if !failures.is_empty() {
        print_report(
            args,
            &file_plans,
            commit_sha.clone(),
            &failures,
            &styler,
            false,
        );
        return 1;
    }
    print_report(args, &file_plans, commit_sha, &[], &styler, false)
}

/// Vault-relative scan of eligible `.md` files for recognized markers.
fn scan_candidates(bob_dir: &Path) -> Result<Vec<FilePlan>, GkeepError> {
    let mut plans = Vec::new();
    visit_dir(bob_dir, bob_dir, &mut plans)?;
    Ok(plans)
}

fn visit_dir(
    dir: &Path,
    bob_dir: &Path,
    out: &mut Vec<FilePlan>,
) -> Result<(), GkeepError> {
    let mut names: Vec<PathBuf> = Vec::new();
    let entries = fs::read_dir(dir).map_err(|error| {
        GkeepError::runtime("vault", format!("scan the vault: {error}"))
    })?;
    for entry in entries {
        let entry = entry.map_err(|error| {
            GkeepError::runtime("vault", format!("scan the vault: {error}"))
        })?;
        let file_type = entry.file_type().map_err(|error| {
            GkeepError::runtime("vault", format!("scan the vault: {error}"))
        })?;
        if file_type.is_dir() {
            if crate::native::is_always_excluded_note_directory_name(
                &entry.file_name(),
            ) {
                continue;
            }
            // Never descend into the import store itself.
            if entry.path() == imports::imports_dir(bob_dir) {
                continue;
            }
            visit_dir(&entry.path(), bob_dir, out)?;
        } else if file_type.is_file() {
            names.push(entry.path());
        }
    }
    names.sort();
    for path in names {
        if path.extension().is_none_or(|ext| ext != "md") {
            continue;
        }
        // Skip the import store files themselves.
        if path.starts_with(imports::imports_dir(bob_dir)) {
            continue;
        }
        if let Some(plan) = scan_file(bob_dir, &path)? {
            out.push(plan);
        }
    }
    Ok(())
}

/// Scan one file for recognized markers inside top-level task blocks.
fn scan_file(
    bob_dir: &Path,
    path: &Path,
) -> Result<Option<FilePlan>, GkeepError> {
    let before_bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(_) => {
            // Unreadable files are skipped, not fatal.
            return Ok(None);
        }
    };
    let text = match String::from_utf8(before_bytes.clone()) {
        Ok(text) => text,
        Err(_) => return Ok(None),
    };
    let crlf = text.contains("\r\n");
    let has_final_newline = text.ends_with('\n');
    let settings = note_tasks::read_settings(bob_dir);
    let scan = note_tasks::scan(&text, &settings);
    // Top-level task block extents (any status, including completed).
    let tops: Vec<usize> = scan
        .tasks()
        .iter()
        .filter(|task| task.indentation.is_empty())
        .map(|task| task.line_index)
        .collect();
    let lines: Vec<String> = text
        .replace("\r\n", "\n")
        .split('\n')
        .map(str::to_string)
        .collect();
    // Note: `split('\n')` on a trailing-newline file yields a final empty
    // element; line indices from `note_tasks` count real lines only.
    let mut blocks: Vec<(usize, usize)> = Vec::new();
    for (i, line_index) in tops.iter().enumerate() {
        let end = if i + 1 < tops.len() {
            tops[i + 1]
        } else {
            lines.len()
        };
        blocks.push((*line_index, end));
    }
    let mut hits = Vec::new();
    for (line_number, line) in lines.iter().enumerate() {
        // Only lines inside a top-level task block are eligible.
        if !blocks
            .iter()
            .any(|(start, end)| line_number >= *start && line_number < *end)
        {
            continue;
        }
        // Standalone indented marker-only continuation: the trimmed line
        // is exactly one valid marker.
        let trimmed = line.trim();
        if !line.is_empty()
            && line.starts_with([' ', '\t'])
            && parse_markers(trimmed).len() == 1
            && trimmed == marker_token(&parse_markers(trimmed)[0])
        {
            let (id, fp) = parse_markers(trimmed)[0].clone();
            hits.push(MarkerHit {
                line_index: line_number,
                id,
                fp,
                kind: MarkerKind::Standalone,
                token_start: 0,
                token_end: line.len(),
            });
            continue;
        }
        // Older `- Source: … %%gkeep:…%%` child: an indented list child
        // whose trimmed form starts with `- Source` and holds exactly one
        // valid marker token. Remove only the token and its generated
        // separator space, preserving the source link and other text.
        if let Some(hit) = source_hit(line, line_number) {
            hits.push(hit);
        }
    }
    if hits.is_empty() {
        return Ok(None);
    }
    let rel = rel_path(bob_dir, path);
    let perms = fs::metadata(path)
        .map_err(|error| {
            GkeepError::runtime(
                "vault",
                format!("stat {}: {error}", path.display()),
            )
        })?
        .permissions();
    Ok(Some(FilePlan {
        rel,
        path: path.to_path_buf(),
        before_bytes,
        crlf,
        has_final_newline,
        perms,
        hits,
        blocks,
        skips: Vec::new(),
    }))
}

/// The canonical marker token for `(id, fp)` (for standalone compare).
fn marker_token((id, fp): &(String, String)) -> String {
    super::ledger::format_marker(id, fp)
}

fn source_hit(line: &str, line_index: usize) -> Option<MarkerHit> {
    let trimmed = line.trim_start();
    if !(trimmed.starts_with("- Source")
        || trimmed.starts_with("* Source")
        || trimmed.starts_with("+ Source"))
    {
        return None;
    }
    let markers = parse_markers(line);
    if markers.len() != 1 {
        return None;
    }
    // Locate the exact token bytes for surgical removal.
    let token = marker_token(&markers[0]);
    let start = line.find(&token)?;
    // Require the generated separator: exactly the token, optionally
    // preceded by one space which is also removed.
    let token_start = if start > 0 && line.as_bytes()[start - 1] == b' ' {
        start - 1
    } else {
        start
    };
    Some(MarkerHit {
        line_index,
        id: markers[0].0.clone(),
        fp: markers[0].1.clone(),
        kind: MarkerKind::Source,
        token_start,
        token_end: start + token.len(),
    })
}

fn rel_path(bob_dir: &Path, path: &Path) -> String {
    path.strip_prefix(bob_dir)
        .map(|relative| relative.to_string_lossy().replace('\\', "/"))
        .unwrap_or_else(|_| path.to_string_lossy().replace('\\', "/"))
}

/// Map task-block start lines to cleaned block digests (helper for
/// receipts). Computed by the caller after cleanup; here for dry-run
/// display only the count matters.
fn task_block_map(
    contents: &str,
    settings: &note_tasks::NoteTaskSettings,
) -> BTreeMap<usize, String> {
    let scan = note_tasks::scan(contents, settings);
    let lines: Vec<&str> = contents
        .split_terminator('\n')
        .map(|line| line.strip_suffix('\r').unwrap_or(line))
        .collect();
    let mut map = BTreeMap::new();
    for task in scan
        .tasks()
        .iter()
        .filter(|task| task.indentation.is_empty())
    {
        let mut block = vec![lines[task.line_index].to_string()];
        let mut index = task.line_index + 1;
        while index < lines.len() {
            if lines[index].trim().is_empty() {
                break;
            }
            if !lines[index].starts_with([' ', '\t']) {
                break;
            }
            block.push(lines[index].to_string());
            index += 1;
        }
        map.insert(task.line_index, imports::block_digest(&block.join("\n")));
    }
    map
}

/// The 💡 source URL for a hit's task, when the task carries the exact
/// emitted link shape. Never creates history on its own.
fn source_url_for_hit(plan: &FilePlan, hit: &MarkerHit) -> Option<String> {
    let text = String::from_utf8_lossy(&plan.before_bytes).into_owned();
    let normalized = text.replace("\r\n", "\n");
    // Find the task block start at or before the hit line.
    let lines: Vec<&str> = normalized.lines().collect();
    // Search upward for the owning top-level task line: the nearest
    // preceding non-indented task line.
    let mut task_line = hit.line_index;
    while task_line > 0 {
        if !lines[task_line].starts_with([' ', '\t'])
            && lines[task_line].trim_start().starts_with("- [")
        {
            break;
        }
        if task_line == 0 {
            break;
        }
        task_line -= 1;
    }
    let description = lines.get(task_line).unwrap_or(&"").to_string();
    // The description from the raw line still carries the checkbox; the
    // shared parser looks for the 💡 shape anywhere in it.
    parse_source_url(&description).and_then(|encoded| {
        // Decode percent-encoding back to the raw Keep URL for storage.
        Some(percent_decode(&encoded).unwrap_or(encoded))
    })
}

fn percent_decode(encoded: &str) -> Option<String> {
    let bytes = encoded.as_bytes();
    let mut raw = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            let digits = encoded.get(index + 1..index + 3)?;
            raw.push(u8::from_str_radix(digits, 16).ok()?);
            index += 3;
        } else {
            raw.push(bytes[index]);
            index += 1;
        }
    }
    String::from_utf8(raw).ok()
}

/// Render the cleaned file contents for `plan`, removing only `hits`.
/// Returns `None` when the rendering fails. Preserves task state, text,
/// links, children, indentation, CRLF/LF, final newline, and permissions
/// (permissions applied by the installer, not here).
fn apply_cleanup(plan: &FilePlan, hits: &[&MarkerHit]) -> Option<String> {
    let text = String::from_utf8(plan.before_bytes.clone()).ok()?;
    let ending = if plan.crlf { "\r\n" } else { "\n" };
    // Work on LF lines, rejoin with the original ending.
    let mut lines: Vec<String> = text
        .replace("\r\n", "\n")
        .split('\n')
        .map(str::to_string)
        .collect();
    // Trailing-newline split yields a final empty element; keep it so the
    // final newline is preserved exactly.
    let mut by_line: BTreeMap<usize, Vec<&MarkerHit>> = BTreeMap::new();
    for hit in hits {
        by_line.entry(hit.line_index).or_default().push(*hit);
    }
    // Delete standalone lines entirely (in reverse so indices hold).
    let mut standalone: Vec<usize> = by_line
        .iter()
        .filter(|(_, group)| {
            group.iter().any(|hit| hit.kind == MarkerKind::Standalone)
        })
        .map(|(index, _)| *index)
        .collect();
    standalone.sort_unstable_by(|a, b| b.cmp(a));
    for index in standalone {
        if index < lines.len() {
            lines.remove(index);
        }
        by_line.remove(&index);
    }
    // Surgical Source-token removal.
    for (index, group) in &by_line {
        for hit in group.iter().filter(|hit| hit.kind == MarkerKind::Source) {
            if *index >= lines.len() {
                continue;
            }
            let line = lines[*index].clone();
            if hit.token_end > line.len() || hit.token_start > hit.token_end {
                continue;
            }
            let mut cleaned = line.clone();
            cleaned.replace_range(hit.token_start..hit.token_end, "");
            // Trim a single leftover trailing space left by the generated
            // separator when the marker ended the line.
            if cleaned.ends_with(' ')
                && !line[..hit.token_start].ends_with("  ")
            {
                cleaned.pop();
            }
            lines[*index] = cleaned;
        }
    }
    let mut out = lines.join("\n");
    // `split('\n')` round-trips the final newline via the trailing empty
    // element; `join` already restores it. Convert endings last.
    if plan.crlf {
        out = out.replace('\n', ending);
    }
    Some(out)
}

/// CAS-protected install of cleaned contents. Returns `Ok(true)` when the
/// file was replaced, `Ok(false)` when the file changed under us (marker
/// left recoverable).
fn install_cleaned(
    plan: &FilePlan,
    after_contents: &str,
) -> Result<bool, String> {
    let parent = plan.path.parent().unwrap_or_else(|| Path::new("."));
    let file_name = plan
        .path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "note.md".to_string());
    let tmp =
        parent.join(format!(".{file_name}.tmp.migrate.{}", std::process::id()));
    fs::write(&tmp, after_contents)
        .map_err(|error| format!("write {}: {error}", plan.path.display()))?;
    fs::set_permissions(&tmp, plan.perms.clone())
        .map_err(|error| format!("chmod {}: {error}", plan.path.display()))?;
    File::open(&tmp)
        .and_then(|file| file.sync_all())
        .map_err(|error| format!("sync {}: {error}", plan.path.display()))?;
    // CAS: re-read before rename; an intervening edit aborts this file.
    let current = fs::read(&plan.path)
        .map_err(|error| format!("re-read {}: {error}", plan.path.display()))?;
    if current != plan.before_bytes {
        let _ = fs::remove_file(&tmp);
        return Ok(false);
    }
    fs::rename(&tmp, &plan.path)
        .map_err(|error| format!("rename {}: {error}", plan.path.display()))?;
    File::open(parent)
        .and_then(|file| file.sync_all())
        .map_err(|error| format!("sync dir {}: {error}", parent.display()))?;
    Ok(true)
}

#[allow(clippy::too_many_arguments)]
fn print_report(
    args: &MigrateArgs,
    plans: &[FilePlan],
    commit: Option<String>,
    failures: &[(String, String)],
    styler: &Styler,
    dry_run: bool,
) -> i32 {
    let is_json = args.format.is_json();
    // Per-file removals (recognized, non-ambiguous hits) and skips.
    let mut files_json = Vec::new();
    let mut removed = 0usize;
    let mut skipped = 0usize;
    for plan in plans {
        if plan.hits.is_empty() {
            continue;
        }
        // Ambiguous blocks are skips, not removals.
        let mut block_ids: BTreeMap<usize, BTreeSet<(String, String)>> =
            BTreeMap::new();
        for hit in &plan.hits {
            let block_index = plan
                .blocks
                .iter()
                .position(|(start, end)| {
                    hit.line_index >= *start && hit.line_index < *end
                })
                .unwrap_or(usize::MAX);
            block_ids
                .entry(block_index)
                .or_default()
                .insert((hit.id.clone(), hit.fp.clone()));
        }
        let mut file_removed = 0usize;
        let mut file_skipped = 0usize;
        for ids in block_ids.values() {
            if ids.len() > 1 {
                file_skipped += 1;
            } else {
                file_removed += 1;
            }
        }
        // Hits counted per marker occurrence for the summary.
        let mut distinct_blocks = BTreeMap::new();
        for hit in &plan.hits {
            distinct_blocks
                .entry((hit.id.clone(), hit.fp.clone()))
                .or_insert(0);
        }
        let _ = distinct_blocks;
        removed += plan
            .hits
            .iter()
            .filter(|hit| {
                let block_index = plan
                    .blocks
                    .iter()
                    .position(|(start, end)| {
                        hit.line_index >= *start && hit.line_index < *end
                    })
                    .unwrap_or(usize::MAX);
                block_ids
                    .get(&block_index)
                    .map(|ids| ids.len() == 1)
                    .unwrap_or(false)
            })
            .count();
        skipped += file_skipped;
        files_json.push(json!({
            "path": plan.rel,
            "removed": file_removed,
            "skipped": file_skipped,
        }));
    }
    let ok = failures.is_empty();
    if is_json {
        if args.quiet && ok {
            return 0;
        }
        let document = json!({
            "schema_version": 1,
            "ok": ok,
            "dry_run": dry_run,
            "files": files_json,
            "summary": {"removed": removed, "skipped": skipped, "failed": failures.len()},
            "commit": commit,
            "failures": failures.iter().map(|(path, message)| json!({"path": path, "message": message})).collect::<Vec<_>>(),
        });
        println!("{document}");
        return if ok { 0 } else { 1 };
    }
    if args.quiet {
        if ok {
            return 0;
        }
        for (path, message) in failures {
            eprintln!("bob gkeep migrate-markers: {path}: {message}");
        }
        return 1;
    }
    if dry_run {
        println!(
            "gkeep migrate-markers --dry-run · {} marker(s) in {} file(s)",
            plans.iter().map(|plan| plan.hits.len()).sum::<usize>(),
            files_json.len()
        );
        for plan in plans {
            if plan.hits.is_empty() {
                continue;
            }
            println!("  · {}: {} marker(s)", plan.rel, plan.hits.len());
            for hit in &plan.hits {
                println!(
                    "    {}:{} {}:{}",
                    plan.rel,
                    hit.line_index + 1,
                    hit.id,
                    hit.fp
                );
            }
        }
        println!(
            "{} dry run: no writes, no metadata",
            styler.success_prefix(true)
        );
        return 0;
    }
    if removed == 0 && failures.is_empty() {
        println!(
            "{} nothing to migrate · markers already clean",
            styler.success_prefix(false)
        );
        return 0;
    }
    println!("gkeep migrate-markers · {removed} removed · {skipped} skipped");
    for plan in plans {
        if plan.hits.is_empty() {
            continue;
        }
        println!("  · {}: {} marker(s)", plan.rel, plan.hits.len());
    }
    for (path, message) in failures {
        eprintln!("bob gkeep migrate-markers: {path}: {message}");
    }
    if let Some(sha) = commit {
        let short: String = sha.chars().take(7).collect();
        println!("{} vault commit {short}", styler.success_prefix(false));
    }
    if ok {
        0
    } else {
        1
    }
}
