//! Staged dependency writes for `&note:block-id` captures.
//!
//! The writer merges managed Depends-On lines and derived effects (line
//! links, `[dependsOn::]` / `[id::]` fields, immediate Blocked status,
//! prerequisite promotion, freshness) through the capture batch planner.
//! Every edit resolves against the batch's current staged text, so a
//! target-ID change cannot overwrite a newly inserted child, and every
//! task is re-looked-up by block ID immediately before its note is
//! staged. Commit keeps the existing temporary-file/rollback contract;
//! a failed item aborts planning before anything is written.

use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    ffi::OsStr,
    sync::{
        atomic::{AtomicUsize, Ordering},
        OnceLock,
    },
};

use super::*;
use crate::native::{
    capture_dependency_tasks::{self, DependencyTaskResult, LocatorError},
    capture_language::{
        ParsedCaptureText, ParsedDependency, ParsedDependencyTarget,
    },
    freshness::placement::stamp_fresh,
    task_dependencies::{
        self, format as dependency_format, parse as dependency_parse,
    },
    task_status_hooks,
    vault_links::NoteIndex,
};

/// Batch-scoped dependency writer state: the vault catalog, task
/// settings, and the previous-daily guard, shared by every item in one
/// batch.
pub(super) struct DependencyContext {
    bob_dir: PathBuf,
    pub(super) today: NaiveDate,
    pub(super) anchor: NaiveDate,
    /// Lazily initialised vault state: `new` performs no I/O, so plain
    /// captures never touch the vault. The full task scan runs only for
    /// `&` dependency writes; `!` note resolution uses the walk-only
    /// catalog; `!` recovery uses the prefiltered dependents snapshot.
    settings: OnceLock<note_tasks::NoteTaskSettings>,
    discovered: OnceLock<DependencyTaskResult>,
    catalog: OnceLock<NoteCatalog>,
    previous_daily: OnceLock<Option<PathBuf>>,
    dependents: OnceLock<DependentsSnapshot>,
}

impl DependencyContext {
    pub(super) fn new(bob_dir: &Path, today: NaiveDate) -> Self {
        // No I/O here: the day-file path is pure path math plus
        // environment, and the anchor parses its file name. Every vault
        // read below waits for first use.
        let day_file = pomodoro::day_file_for(bob_dir);
        let anchor = task_status_hooks::daily_anchor_date(&day_file, today);
        Self {
            bob_dir: bob_dir.to_path_buf(),
            today,
            anchor,
            settings: OnceLock::new(),
            discovered: OnceLock::new(),
            catalog: OnceLock::new(),
            previous_daily: OnceLock::new(),
            dependents: OnceLock::new(),
        }
    }

    /// Task settings, read once per batch on first dependency use.
    pub(super) fn settings(&self) -> &note_tasks::NoteTaskSettings {
        self.settings
            .get_or_init(|| note_tasks::read_settings(&self.bob_dir))
    }

    /// Full prerequisite scan, warmed only by `&` dependency writes.
    /// `!` resolution and recovery never touch it.
    pub(super) fn ensure_discovered(&self) -> &DependencyTaskResult {
        self.discovered
            .get_or_init(|| capture_dependency_tasks::discover(&self.bob_dir))
    }

    /// Walk-only note catalog for `!`/`&` resolution: no note contents
    /// are read.
    pub(super) fn catalog(&self) -> &NoteCatalog {
        self.catalog
            .get_or_init(|| NoteCatalog::walk(&self.bob_dir))
    }

    /// The latest daily note before `anchor`, resolved lazily on first
    /// `&` target-ID projection.
    pub(super) fn previous_daily(&self) -> Option<&Path> {
        self.previous_daily
            .get_or_init(|| {
                // A missing or unreadable vault root lists no daily
                // notes; the requested read or write still fails
                // decisively downstream.
                previous_daily_absolute(&self.bob_dir, self.anchor)
                    .ok()
                    .flatten()
            })
            .as_deref()
    }

    /// Batch-scoped prefiltered dependents snapshot for `!` recovery,
    /// built once per batch and borrowed afterwards.
    pub(super) fn dependents_snapshot(&self) -> &DependentsSnapshot {
        self.dependents
            .get_or_init(|| DependentsSnapshot::build(&self.bob_dir))
    }
}

/// Walk-only note catalog for `!`/`&` resolution: the basename index
/// plus ambiguity diagnostics over the same path set [`discover`]
/// walks, without reading any note contents.
pub(super) struct NoteCatalog {
    pub(super) index: NoteIndex,
    pub(super) basenames: HashMap<String, Vec<String>>,
}

impl NoteCatalog {
    fn walk(bob_dir: &Path) -> Self {
        let (relative_paths, _) =
            capture_dependency_tasks::vault_note_paths(bob_dir);
        let index = NoteIndex::from_paths(relative_paths.iter().cloned());
        // Basename diagnostics mirror `discover` exactly (same walk,
        // same display form, same sort).
        let mut basenames: HashMap<String, Vec<String>> = HashMap::new();
        for relative in &relative_paths {
            if let Some(stem) = relative.file_stem().and_then(OsStr::to_str) {
                basenames
                    .entry(stem.to_lowercase())
                    .or_default()
                    .push(capture_dependency_tasks::display_path(relative));
            }
        }
        for paths in basenames.values_mut() {
            paths.sort();
        }
        Self { index, basenames }
    }
}

/// Batch-scoped dependents snapshot for `!` recovery: the prefiltered
/// on-disk contents built once per batch and borrowed afterwards. Only
/// notes whose bytes contain `dependsOn` or `id::` are kept; a note is
/// excluded only when it contributes no dependency edge and no task
/// identity to `task_dependency_states`.
pub(super) struct DependentsSnapshot {
    base: BTreeMap<PathBuf, String>,
    /// False when the vault walk itself failed. Recovery then finds
    /// nothing; `capture_complete` reports the lookup as unavailable.
    pub(super) available: bool,
}

impl DependentsSnapshot {
    fn build(bob_dir: &Path) -> Self {
        // The walk runs on the calling thread. Every env-dependent path
        // is resolved here, before fanning out: `bob_env` overrides are
        // thread-local, so nothing env-dependent may resolve inside the
        // scoped workers below (they only read bytes and match
        // substrings).
        let files = match task_status_hooks::markdown_files(bob_dir) {
            Ok(files) => files,
            Err(_) => {
                return Self {
                    base: BTreeMap::new(),
                    available: false,
                };
            }
        };
        let mut pairs = Vec::with_capacity(files.len());
        for absolute in files {
            let Ok(relative) =
                absolute.strip_prefix(bob_dir).map(Path::to_path_buf)
            else {
                continue;
            };
            pairs.push((absolute, relative));
        }
        Self {
            base: read_prefiltered_parallel(&pairs),
            available: true,
        }
    }

    /// Base contents with one batch item's staged overlay applied:
    /// staged text replaces the disk version (or, for a staged deletion,
    /// removes it), and staged text is prefiltered the same way. The
    /// base map is borrowed, never cloned. The result is sorted by
    /// vault-relative path, like the full-vault read it replaces.
    pub(super) fn overlaid(
        &self,
        staged: BTreeMap<PathBuf, Option<String>>,
    ) -> Vec<(PathBuf, String)> {
        let mut snapshot: Vec<(PathBuf, String)> =
            Vec::with_capacity(self.base.len() + staged.len());
        for (relative, text) in &self.base {
            if staged.contains_key(relative) {
                continue;
            }
            snapshot.push((relative.clone(), text.clone()));
        }
        for (relative, contents) in staged {
            let Some(text) = contents else {
                continue;
            };
            if contains_dependency_markers(text.as_bytes()) {
                snapshot.push((relative, text));
            }
        }
        snapshot.sort_by(|left, right| left.0.cmp(&right.0));
        snapshot
    }
}

/// Byte prefilter for dependency relevance: a note whose bytes contain
/// neither `dependsOn` nor `id::` holds no parsed `dependsOn` field and
/// no `[id::]`/`(id::)` identity (`task_metadata` only recognizes those
/// exact keys), so recovery and `task_dependency_states` cannot observe
/// it. Matching is deliberately substring-wide: a prose or fenced-code
/// match only keeps the note, never changes its parse.
fn contains_dependency_markers(bytes: &[u8]) -> bool {
    memchr::memmem::find(bytes, b"dependsOn").is_some()
        || memchr::memmem::find(bytes, b"id::").is_some()
}

/// One walked note read through the prefilter: raw bytes first (so an
/// invalid-UTF-8 note that matches still skips cleanly), then kept only
/// when marker-positive and valid UTF-8.
fn read_prefiltered_one(
    pair: &(PathBuf, PathBuf),
) -> Option<(PathBuf, String)> {
    let bytes = std::fs::read(&pair.0).ok()?;
    if !contains_dependency_markers(&bytes) {
        return None;
    }
    let text = String::from_utf8(bytes).ok()?;
    Some((pair.1.clone(), text))
}

/// Read every walked note on scoped worker threads, keeping only
/// marker-positive notes. Mirrors the atomic work-queue shape of
/// `highlights_ref::sync::plan_pdfs`: workers pull indices off a shared
/// counter and the merge restores vault order, so output is
/// deterministic.
fn read_prefiltered_parallel(
    pairs: &[(PathBuf, PathBuf)],
) -> BTreeMap<PathBuf, String> {
    if pairs.is_empty() {
        return BTreeMap::new();
    }
    let worker_count = std::thread::available_parallelism()
        .map(|parallelism| parallelism.get())
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
            .map(|handle| handle.join().expect("snapshot worker panicked"))
            .collect()
    });
    collected.into_iter().flatten().collect()
}

/// The latest daily note before `anchor`, mirroring the hooks' previous
/// daily snapshot without requiring it to exist.
fn previous_daily_absolute(
    bob_dir: &Path,
    anchor: NaiveDate,
) -> Result<Option<PathBuf>, String> {
    let files = task_status_hooks::markdown_files(bob_dir)
        .map_err(|error| format!("list vault notes: {error}"))?;
    Ok(files
        .iter()
        .filter_map(|path| {
            let relative = path.strip_prefix(bob_dir).ok()?;
            let date = task_status_hooks::canonical_daily_date(relative)?;
            (date < anchor).then_some((date, path.clone()))
        })
        .max_by_key(|(date, _)| *date)
        .map(|(_, path)| path))
}

/// Vault-relative display path (`a/b.md`) for diagnostics and JSON.
pub(super) fn display_relative(path: &Path) -> String {
    path.components()
        .filter_map(|component| {
            component.as_os_str().to_str().map(str::to_string)
        })
        .collect::<Vec<_>>()
        .join("/")
}

/// Absolute path for a vault-relative note.
fn absolute_note(bob_dir: &Path, relative: &Path) -> PathBuf {
    bob_dir.join(relative)
}

/// Current text of a note: staged contents when the batch already
/// touched it, otherwise what is on disk.
pub(super) fn current_note_text(
    planner: &mut CaptureBatchPlanner,
    absolute: &Path,
) -> Result<String, CaptureError> {
    if let Some(staged) = planner.current_contents(absolute)? {
        return Ok(staged);
    }
    std::fs::read_to_string(absolute).map_err(|error| {
        CaptureError::io(format!("read note {}: {error}", absolute.display()))
    })
}

/// Split a note into editable lines, preserving the note's line ending
/// and final-newline state on rejoin.
struct EditableNote {
    lines: Vec<String>,
    ending: &'static str,
    final_newline: bool,
}

impl EditableNote {
    fn new(contents: &str) -> Self {
        Self {
            lines: contents.lines().map(str::to_string).collect(),
            ending: dependency_format::note_line_ending(contents),
            final_newline: dependency_format::has_final_newline(contents),
        }
    }

    fn join(&self) -> String {
        let mut joined = self.lines.join(self.ending);
        if self.final_newline && !self.lines.is_empty() {
            joined.push_str(self.ending);
        }
        joined
    }
}

/// Indent for a new managed child: reuse the first direct child's
/// indent, otherwise the parent's indent plus one tab.
fn first_child_indent(note: &EditableNote, task_at: usize) -> String {
    let view: Vec<&str> = note.lines.iter().map(String::as_str).collect();
    let Some(first) = view.get(task_at) else {
        return "\t".to_string();
    };
    let source_indent = task_status_hooks::leading_indentation_width(first);
    for line_index in task_at + 1..view.len() {
        let line = view[line_index];
        if line.trim().is_empty() {
            continue;
        }
        if task_status_hooks::leading_indentation_width(line) <= source_indent {
            break;
        }
        if task_status_hooks::nearest_parent_list_item(&view, line_index)
            != Some(task_at)
        {
            continue;
        }
        let len = line
            .bytes()
            .take_while(|byte| matches!(byte, b' ' | b'\t'))
            .count();
        return line[..len].to_string();
    }
    dependency_format::child_indent_for_parent(
        task_indent(view.get(task_at).copied().unwrap_or_default()),
        None,
    )
}

/// Insertion slot for a new managed child: first except after an
/// existing Cancel Log, which keeps the first slot.
fn cancel_log_aware_slot(note: &EditableNote, task_at: usize) -> usize {
    let view: Vec<&str> = note.lines.iter().map(String::as_str).collect();
    let Some(first) = view.get(task_at) else {
        return task_at + 1;
    };
    let source_indent = task_status_hooks::leading_indentation_width(first);
    for line_index in task_at + 1..view.len() {
        let line = view[line_index];
        if line.trim().is_empty() {
            continue;
        }
        if task_status_hooks::leading_indentation_width(line) <= source_indent {
            break;
        }
        if task_status_hooks::nearest_parent_list_item(&view, line_index)
            != Some(task_at)
        {
            continue;
        }
        if task_status_hooks::is_cancel_log_line(line) {
            return line_index + 1;
        }
        return task_at + 1;
    }
    task_at + 1
}

/// Replace one task line's status box (`- [X]`) with `replacement`.
/// The first `[...]` group on a scanned task line is its status box.
fn replace_status_char(line: &str, replacement: char) -> Option<String> {
    let open = line.find('[')?;
    let close = line[open + 1..].find(']')?;
    if close != 1 {
        return None;
    }
    let mut rebuilt = line.to_string();
    rebuilt.replace_range(open + 1..open + 2, &replacement.to_string());
    Some(rebuilt)
}

/// Numeric lane for promotion: Ready 0, Next 1, In Progress 2. Blocked,
/// custom, and unrecognized statuses never promote.
fn lane_for_status(status: char) -> Option<u8> {
    match status {
        ' ' => Some(0),
        '*' => Some(1),
        '/' => Some(2),
        _ => None,
    }
}

fn lane_checkbox(lane: u8) -> char {
    match lane {
        0 => ' ',
        1 => '*',
        _ => '/',
    }
}

/// Note index over on-disk notes plus batch-staged new notes, so a
/// prerequisite in a note created earlier in the same batch resolves
/// and canonical links stay shortest-unambiguous.
fn staged_note_index(
    ctx: &DependencyContext,
    planner: &CaptureBatchPlanner,
    bob_dir: &Path,
) -> NoteIndex {
    let mut paths: BTreeSet<PathBuf> = ctx
        .catalog()
        .index
        .relative_paths()
        .map(Path::to_path_buf)
        .collect();
    for (path, _) in planner.staged_snapshot() {
        if let Ok(relative) = path.strip_prefix(bob_dir)
            && relative
                .extension()
                .is_some_and(|extension| extension.eq_ignore_ascii_case("md"))
        {
            paths.insert(relative.to_path_buf());
        }
    }
    NoteIndex::from_paths(paths)
}

/// Resolve a typed `&note` identity against the on-disk catalog, with a
/// staged fallback for notes this batch created but has not committed.
/// Returns the vault-relative path with extension.
pub(super) fn resolve_prerequisite_note(
    ctx: &DependencyContext,
    planner: &CaptureBatchPlanner,
    bob_dir: &Path,
    typed: &str,
) -> Result<PathBuf, CaptureError> {
    let catalog = ctx.catalog();
    match capture_dependency_tasks::resolve_dependency_note_in(
        bob_dir,
        &catalog.index,
        &catalog.basenames,
        typed,
    ) {
        Ok(relative) => Ok(relative),
        Err(LocatorError::Io(message)) => Err(CaptureError::io(message)),
        Err(LocatorError::Usage(message)) => {
            // A batch-created note is not on disk yet: accept an exact
            // staged path or a unique staged basename before refusing.
            if message.starts_with("no such note")
                && let Some(relative) =
                    resolve_staged_note(planner, bob_dir, typed)
            {
                return Ok(relative);
            }
            Err(CaptureError::usage(message))
        }
    }
}

/// Staged-only counterpart to basename/exact resolution for notes this
/// batch created: exact relative paths win, otherwise a unique
/// case-insensitive basename.
pub(super) fn resolve_staged_note(
    planner: &CaptureBatchPlanner,
    bob_dir: &Path,
    typed: &str,
) -> Option<PathBuf> {
    let with_extension = if typed
        .rsplit('/')
        .next()
        .is_some_and(|file| file.contains('.'))
    {
        typed.to_string()
    } else {
        format!("{typed}.md")
    };
    let mut staged: Vec<PathBuf> = Vec::new();
    for (path, _) in planner.staged_snapshot() {
        let Ok(relative) = path.strip_prefix(bob_dir) else {
            continue;
        };
        if !relative
            .extension()
            .is_some_and(|extension| extension.eq_ignore_ascii_case("md"))
        {
            continue;
        }
        staged.push(relative.to_path_buf());
    }
    if staged.contains(&PathBuf::from(&with_extension)) {
        return Some(PathBuf::from(&with_extension));
    }
    if typed.contains('/') {
        return None;
    }
    let mut matches = staged
        .into_iter()
        .filter(|relative| {
            relative
                .file_stem()
                .and_then(|stem| stem.to_str())
                .is_some_and(|stem| stem.eq_ignore_ascii_case(typed))
        })
        .collect::<Vec<_>>();
    matches.sort();
    matches.dedup();
    (matches.len() == 1).then(|| matches.remove(0))
}

/// One resolved prerequisite: its staged note identity plus the vault
/// id its Depends-On link projects.
struct ResolvedPrerequisite {
    /// Vault-relative note path with extension.
    relative: PathBuf,
    block_id: String,
    /// Existing `[id::]`, canonical id, or archive id: the
    /// `[dependsOn::]` value for this prerequisite.
    projected_id: String,
    /// Canonical wikilink text for the merged managed line.
    link_text: String,
    status_symbol: char,
    status_name: String,
    text: String,
    open: bool,
}

/// Look a task up by block ID in staged contents, with capture-shaped
/// diagnostics. `route_label` names the note for messages.
pub(super) fn lookup_staged_task<'a>(
    scan: &'a note_tasks::NoteTaskScan,
    route_label: &str,
    block_id: &str,
) -> Result<&'a note_tasks::NoteTask, CaptureError> {
    match scan.by_block_id(block_id) {
        note_tasks::BlockIdLookup::Found(task) => Ok(task),
        note_tasks::BlockIdLookup::NotATask {
            line_index,
            excerpt,
        } => Err(CaptureError::io(format!(
            "^{block_id} in {route_label} is not a task (line {}: {excerpt})",
            line_index + 1
        ))),
        note_tasks::BlockIdLookup::Duplicate(count) => {
            Err(CaptureError::io(format!(
                "block ID ^{block_id} appears {count} times in {route_label}; make it unique before capturing"
            )))
        }
        note_tasks::BlockIdLookup::Missing => {
            let choices =
                format!("run 'bob capture-tasks -r {route_label}' to list task block IDs");
            let message = match scan.suggest_block_id(block_id) {
                Some(suggestion) => format!(
                    "no task with block ID ^{block_id} in {route_label}; did you mean ^{suggestion}? ({choices})"
                ),
                None => format!(
                    "no task with block ID ^{block_id} in {route_label} ({choices})"
                ),
            };
            Err(CaptureError::io(message))
        }
    }
}

/// One note the writer changed as a side effect (target-ID stamp or
/// promotion): carried for `dependency_target` task blocks.
struct TargetTouch {
    absolute: PathBuf,
    relative: PathBuf,
    block_id: String,
    line: usize,
}

/// Project one prerequisite target to its vault id: the existing valid
/// `[id::]` when one is present, otherwise the canonical id (stamping
/// the target line, unless it lives in the previous daily snapshot or
/// its path cannot encode, which both refuse with a precise message).
/// Returns the id plus the staged line when the target was stamped.
fn project_prerequisite_id(
    ctx: &DependencyContext,
    planner: &mut CaptureBatchPlanner,
    relative: &Path,
    absolute: &Path,
    block_id: &str,
    dependent_desc: &str,
) -> Result<(String, Option<usize>), CaptureError> {
    let contents = current_note_text(planner, absolute)?;
    let lines: Vec<&str> = contents.lines().collect();
    let scan = note_tasks::scan(&contents, ctx.settings());
    let route_label = display_relative(relative);
    let task = lookup_staged_task(&scan, &route_label, block_id)?;
    let task_line = lines.get(task.line_index).copied().unwrap_or_default();
    if let Some(existing) =
        task_status_hooks::task_metadata(task_line, Some(block_id)).task_id
    {
        return Ok((existing, None));
    }
    if task_dependencies::is_archive_path(relative) {
        // Archive history is never stamped: without an `[id::]` the
        // target cannot project.
        if let Ok(canonical) =
            task_dependencies::dependency_id(relative, block_id)
        {
            return Ok((canonical, None));
        }
        return Err(CaptureError::usage(format!(
            "cannot depend on {route_label}^{block_id} for {dependent_desc}: it lives in archive history without an [id::] and archive notes are never stamped"
        )));
    }
    if ctx.previous_daily() == Some(absolute) {
        return Err(CaptureError::usage(format!(
            "cannot depend on {route_label}^{block_id} for {dependent_desc}: it lives in the previous daily note without an [id::], and the previous daily snapshot is never stamped (add the [id::] by editing that note first)"
        )));
    }
    let canonical =
        task_dependencies::dependency_id(relative, block_id).map_err(
            |_| {
                CaptureError::usage(format!(
                    "cannot depend on {route_label}^{block_id} for {dependent_desc}: it has no [id::] and its path cannot encode one (add a valid [id::] to that task first)"
                ))
            },
        )?;
    // Revalidate the task line against current staged text before
    // stamping: the batch may have edited this note since resolution.
    let fresh = current_note_text(planner, absolute)?;
    let fresh_scan = note_tasks::scan(&fresh, ctx.settings());
    let fresh_task = lookup_staged_task(&fresh_scan, &route_label, block_id)?;
    if fresh_task.digest != task.digest {
        return Err(CaptureError::io(format!(
            "task ^{block_id} in {route_label} changed while planning {dependent_desc}; refusing to overwrite it"
        )));
    }
    let fresh_lines: Vec<&str> = fresh.lines().collect();
    let current_line = fresh_lines
        .get(fresh_task.line_index)
        .copied()
        .unwrap_or_default();
    let metadata =
        task_status_hooks::task_metadata(current_line, Some(block_id));
    let stamped = task_status_hooks::set_task_fields(
        current_line,
        Some(&canonical),
        &metadata.depends_on,
    );
    if stamped != current_line {
        let mut note = EditableNote::new(&fresh);
        note.lines[fresh_task.line_index] = stamped;
        planner.stage(absolute, note.join())?;
        return Ok((canonical, Some(fresh_task.line_index)));
    }
    Ok((canonical, None))
}

/// Shortest link text for a resolved prerequisite, mirroring the hooks'
/// archive rule: archive targets keep the explicit `done/` path while
/// every other target uses the shortest unambiguous form.
fn prerequisite_link_text(
    index: &NoteIndex,
    dependent_relative: &Path,
    relative: &Path,
    block_id: &str,
) -> String {
    if task_dependencies::is_archive_path(relative) {
        let mut without_extension = relative.to_path_buf();
        without_extension.set_extension("");
        let text = without_extension
            .components()
            .filter_map(|component| {
                component.as_os_str().to_str().map(str::to_string)
            })
            .collect::<Vec<_>>()
            .join("/");
        return format!("[[{text}#^{block_id}]]");
    }
    dependency_format::canonical_link(
        relative,
        block_id,
        dependent_relative,
        index,
    )
}

/// Resolve every typed modifier to a validated prerequisite, in typed
/// order. Repeats of one (note, block) pair are idempotent markers, not
/// errors; missing, duplicate-ID, and non-task targets refuse with a
/// repairable diagnostic. Returns the de-duplicated prerequisites, the
/// typed-repeat count, and every target-ID stamp staged along the way.
fn resolve_prerequisites(
    ctx: &DependencyContext,
    planner: &mut CaptureBatchPlanner,
    bob_dir: &Path,
    index: &NoteIndex,
    dependent_relative: &Path,
    dependent_block: Option<&str>,
    deps: &[ParsedDependency],
    dependent_desc: &str,
) -> Result<(Vec<ResolvedPrerequisite>, usize, Vec<TargetTouch>), CaptureError>
{
    let mut resolved = Vec::new();
    let mut seen: BTreeSet<(String, String)> = BTreeSet::new();
    let mut repeats = 0usize;
    let mut touches = Vec::new();
    for dep in deps {
        let relative =
            resolve_prerequisite_note(ctx, planner, bob_dir, &dep.note)?;
        let absolute = absolute_note(bob_dir, &relative);
        let identity = (display_relative(&relative), dep.block_id.clone());
        if !seen.insert(identity.clone()) {
            repeats += 1;
            continue;
        }
        if Some(relative.as_path()) == Some(dependent_relative)
            && dependent_block == Some(dep.block_id.as_str())
        {
            return Err(CaptureError::usage(format!(
                "task dependency on itself: '{raw}' names the dependent task; a task cannot depend on itself",
                raw = dep.raw
            )));
        }
        let (projected_id, stamped_line) = project_prerequisite_id(
            ctx,
            planner,
            &relative,
            &absolute,
            &dep.block_id,
            dependent_desc,
        )?;
        if let Some(line) = stamped_line {
            touches.push(TargetTouch {
                absolute: absolute.clone(),
                relative: relative.clone(),
                block_id: dep.block_id.clone(),
                line,
            });
        }
        // Re-read the (possibly just stamped) target for status/text.
        let contents = current_note_text(planner, &absolute)?;
        let scan = note_tasks::scan(&contents, ctx.settings());
        let route_label = display_relative(&relative);
        let task = lookup_staged_task(&scan, &route_label, &dep.block_id)?;
        resolved.push(ResolvedPrerequisite {
            link_text: prerequisite_link_text(
                index,
                dependent_relative,
                &relative,
                &dep.block_id,
            ),
            relative,
            block_id: dep.block_id.clone(),
            projected_id,
            status_symbol: task.status_symbol,
            status_name: task.status_name.clone(),
            text: task.description.clone(),
            open: task.status_type.is_open(),
        });
    }
    Ok((resolved, repeats, touches))
}

/// Vault-wide dependency edges from managed lines and covered legacy
/// children, resolved against staged contents: `(note, block)` pairs
/// with the prerequisite pairs their lines name. Unresolvable links
/// contribute no edges (R4 keeps them verbatim; they never block).
struct DependencyGraph {
    edges: BTreeMap<(String, String), BTreeSet<(String, String)>>,
    /// Live openness per identity, from the same staged scans.
    open: HashMap<(String, String), bool>,
    /// Scheduled date per identity, for the future-schedule guard.
    scheduled: HashMap<(String, String), Option<NaiveDate>>,
    /// Status symbol per identity, for promotion lanes.
    status: HashMap<(String, String), char>,
}

impl DependencyGraph {
    fn build(
        ctx: &DependencyContext,
        planner: &mut CaptureBatchPlanner,
        bob_dir: &Path,
        index: &NoteIndex,
    ) -> Result<Self, CaptureError> {
        let mut graph = Self {
            edges: BTreeMap::new(),
            open: HashMap::new(),
            scheduled: HashMap::new(),
            status: HashMap::new(),
        };
        let mut rels: BTreeSet<PathBuf> = ctx
            .catalog()
            .index
            .relative_paths()
            .map(Path::to_path_buf)
            .collect();
        for (path, _) in planner.staged_snapshot() {
            if let Ok(relative) = path.strip_prefix(bob_dir)
                && relative.extension().is_some_and(|extension| {
                    extension.eq_ignore_ascii_case("md")
                })
            {
                rels.insert(relative.to_path_buf());
            }
        }
        // One staged scan per note; unreadable notes are skipped like
        // discovery skips them (a requested write target still fails
        // decisively in its own resolution path).
        let mut scans: BTreeMap<
            PathBuf,
            (Vec<String>, note_tasks::NoteTaskScan),
        > = BTreeMap::new();
        for relative in &rels {
            let absolute = absolute_note(bob_dir, relative);
            let contents = match planner.current_contents(&absolute)? {
                Some(staged) => staged,
                None => match std::fs::read_to_string(&absolute) {
                    Ok(disk) => disk,
                    Err(_) => continue,
                },
            };
            let scan = note_tasks::scan(&contents, ctx.settings());
            scans.insert(
                relative.clone(),
                (contents.lines().map(str::to_string).collect(), scan),
            );
        }
        for (relative, (lines, scan)) in &scans {
            for task in scan.tasks() {
                let Some(block_id) = task.block_id.as_deref() else {
                    continue;
                };
                let identity =
                    (display_relative(relative), block_id.to_string());
                graph
                    .open
                    .insert(identity.clone(), task.status_type.is_open());
                graph.status.insert(identity.clone(), task.status_symbol);
                let task_line = lines
                    .get(task.line_index)
                    .map(String::as_str)
                    .unwrap_or_default();
                graph.scheduled.insert(
                    identity.clone(),
                    task_status_hooks::task_metadata(task_line, Some(block_id))
                        .scheduled,
                );
                let line_refs: Vec<&str> =
                    lines.iter().map(String::as_str).collect();
                if line_refs.is_empty() {
                    continue;
                }
                let fenced = task_status_hooks::fenced_lines(
                    &line_refs,
                    0..line_refs.len(),
                );
                let Some(child) = dependency_parse::dependency_child_of(
                    &line_refs,
                    &fenced,
                    task.line_index,
                ) else {
                    continue;
                };
                let dependency_parse::DependencyLine::Accepted {
                    links, ..
                } = &child.parsed
                else {
                    continue;
                };
                for link in links {
                    let Some(target_rel) =
                        index.resolve(Some(relative), link.target.trim())
                    else {
                        continue;
                    };
                    if task_dependencies::is_archive_path(&target_rel) {
                        continue;
                    }
                    let target_key =
                        (display_relative(&target_rel), link.block_id.clone());
                    let target_known = scans.get(&target_rel).is_some_and(
                        |(_, target_scan)| {
                            matches!(
                                target_scan.by_block_id(&link.block_id),
                                note_tasks::BlockIdLookup::Found(_)
                            )
                        },
                    );
                    if target_known {
                        graph
                            .edges
                            .entry(identity.clone())
                            .or_default()
                            .insert(target_key);
                    }
                }
            }
        }
        Ok(graph)
    }

    /// Whether `from` reaches `to` following `edges` (plus `extra`).
    fn reaches(
        &self,
        from: &(String, String),
        to: &(String, String),
        extra: &BTreeMap<(String, String), BTreeSet<(String, String)>>,
    ) -> Option<Vec<(String, String)>> {
        let mut visited: BTreeSet<(String, String)> = BTreeSet::new();
        let mut stack: Vec<((String, String), Vec<(String, String)>)> =
            vec![(from.clone(), vec![from.clone()])];
        while let Some((node, path)) = stack.pop() {
            if node == *to && path.len() > 1 {
                return Some(path);
            }
            if !visited.insert(node.clone()) {
                continue;
            }
            let mut next: BTreeSet<(String, String)> =
                self.edges.get(&node).cloned().unwrap_or_default();
            if let Some(more) = extra.get(&node) {
                next.extend(more.iter().cloned());
            }
            for target in next {
                if visited.contains(&target) {
                    continue;
                }
                let mut cyl = path.clone();
                cyl.push(target.clone());
                stack.push((target, cyl));
            }
        }
        None
    }
}

/// Format one graph identity for cycle diagnostics.
fn format_identity(identity: &(String, String)) -> String {
    format!("{}^{}", identity.0, identity.1)
}

/// Leading whitespace (spaces/tabs) of a task line: the indent reused
/// for a new managed child.
fn task_indent(line: &str) -> &str {
    let len = line
        .bytes()
        .take_while(|byte| matches!(byte, b' ' | b'\t'))
        .count();
    &line[..len]
}

/// Every direct-child Depends-On line under `task_line`, in document
/// order: the managed-line slot plus any duplicate that refuses the
/// write. Mirrors the single-match [`dependency_child_of`] walk while
/// collecting every match.
fn managed_children(
    lines: &[&str],
    fenced: &BTreeSet<usize>,
    task_line: usize,
) -> Vec<(usize, dependency_parse::DependencyLine)> {
    let mut found = Vec::new();
    let Some(first) = lines.get(task_line) else {
        return found;
    };
    let source_indent = task_status_hooks::leading_indentation_width(first);
    for line_index in task_line + 1..lines.len() {
        let line = lines[line_index];
        if line.trim().is_empty() {
            continue;
        }
        if fenced.contains(&line_index) {
            continue;
        }
        if task_status_hooks::leading_indentation_width(line) <= source_indent {
            break;
        }
        if task_status_hooks::nearest_parent_list_item(lines, line_index)
            != Some(task_line)
        {
            continue;
        }
        match dependency_parse::parse_dependency_line(line) {
            dependency_parse::DependencyLine::NotALine => continue,
            parsed => found.push((line_index, parsed)),
        }
    }
    found
}

/// Direct-child legacy task links under `task_line`, in document order,
/// with their line indices.
fn legacy_children(
    lines: &[&str],
    fenced: &BTreeSet<usize>,
    task_line: usize,
    managed_at: Option<usize>,
) -> Vec<(usize, task_dependencies::legacy::LegacyChildLink)> {
    let mut found = Vec::new();
    let Some(first) = lines.get(task_line) else {
        return found;
    };
    let source_indent = task_status_hooks::leading_indentation_width(first);
    for line_index in task_line + 1..lines.len() {
        if Some(line_index) == managed_at {
            continue;
        }
        let line = lines[line_index];
        if line.trim().is_empty() {
            continue;
        }
        if fenced.contains(&line_index) {
            continue;
        }
        if task_status_hooks::leading_indentation_width(line) <= source_indent {
            break;
        }
        if task_status_hooks::nearest_parent_list_item(lines, line_index)
            != Some(task_line)
        {
            continue;
        }
        if let Some(legacy) = task_dependencies::legacy_child_reference(line) {
            found.push((line_index, legacy));
        }
    }
    found
}

/// Whether a legacy child's resolved target is already managed by the
/// dependent's field: its `[id::]`, canonical id, or same-note bare
/// block id (mirrors the hooks' R8 field gate).
fn legacy_covered(
    field_ids: &[String],
    target_id: Option<&str>,
    target_rel: &Path,
    block_id: &str,
    dependent_rel: &Path,
) -> bool {
    if let Some(task_id) = target_id
        && field_ids.iter().any(|id| id == task_id)
    {
        return true;
    }
    if let Ok(canonical) =
        task_dependencies::dependency_id(target_rel, block_id)
        && field_ids.iter().any(|id| id == &canonical)
    {
        return true;
    }
    target_rel == dependent_rel && field_ids.iter().any(|id| id == block_id)
}

/// Reject any *new* edge that closes a dependency cycle, including
/// cycles introduced across two items of one draft. Pre-existing
/// unrelated graph problems never fail a no-op repeat: with no new
/// edges this is a no-op.
fn reject_new_cycles(
    graph: &DependencyGraph,
    dependent: &(String, String),
    existing: &BTreeSet<(String, String)>,
    merged: &[(String, String)],
) -> Result<(), CaptureError> {
    let mut extra: BTreeMap<(String, String), BTreeSet<(String, String)>> =
        BTreeMap::new();
    let mut new_edges = Vec::new();
    for target in merged {
        if existing.contains(target) {
            continue;
        }
        extra
            .entry(dependent.clone())
            .or_default()
            .insert(target.clone());
        new_edges.push(target);
    }
    if new_edges.is_empty() {
        return Ok(());
    }
    for target in new_edges {
        if target == dependent {
            return Err(CaptureError::usage(format!(
                "task dependency on itself: {} cannot depend on itself",
                format_identity(dependent)
            )));
        }
        if let Some(path) = graph.reaches(target, dependent, &extra) {
            let mut readable: Vec<String> =
                path.iter().map(format_identity).collect();
            readable.push(format_identity(dependent));
            return Err(CaptureError::usage(format!(
                "task dependency cycle: {} (a task cannot depend on itself, directly or transitively)",
                readable.join(" -> ")
            )));
        }
    }
    Ok(())
}

/// Planned dependency effects for one item: the JSON summary plus the
/// final task-block touches (dependent first, then changed targets).
pub(super) struct PlannedDependencyUpdate {
    pub(super) summary: DependencyUpdateJson,
    pub(super) dependent_ref: TaskBlockRef,
    pub(super) target_refs: Vec<TaskBlockRef>,
}

fn prerequisite_json(prereq: &ResolvedPrerequisite) -> PrerequisiteJson {
    PrerequisiteJson {
        note: display_relative(&prereq.relative),
        block_id: prereq.block_id.clone(),
        link: prereq.link_text.clone(),
        status_symbol: prereq.status_symbol,
        status_name: prereq.status_name.clone(),
        text: prereq.text.clone(),
        open: prereq.open,
    }
}

fn target_block_ref(touch: &TargetTouch) -> TaskBlockRef {
    let mut without_extension = touch.relative.clone();
    without_extension.set_extension("");
    TaskBlockRef {
        target: touch.absolute.clone(),
        relative_target: display_relative(&touch.relative),
        route: display_relative(&without_extension),
        line: touch.line,
        block_id: Some(touch.block_id.clone()),
        role: TaskBlockRole::DependencyTarget,
    }
}

/// Raise open prerequisites to at least the dependent's lane (Ready 0,
/// Next 1, In Progress 2), transitively along staged edges. Blocked,
/// future-scheduled, closed, and unrecognized statuses are respected
/// and never rewritten; lanes are never lowered.
#[allow(clippy::too_many_arguments)]
fn plan_promotions(
    ctx: &DependencyContext,
    planner: &mut CaptureBatchPlanner,
    bob_dir: &Path,
    graph: &DependencyGraph,
    extra_edges: &BTreeMap<(String, String), BTreeSet<(String, String)>>,
    dependent_lane: u8,
    seeds: &[(String, String)],
    dependent_desc: &str,
) -> Result<Vec<TargetTouch>, CaptureError> {
    let mut desired: BTreeMap<(String, String), u8> = BTreeMap::new();
    let mut queue: Vec<(String, String)> = Vec::new();
    for seed in seeds {
        desired.insert(seed.clone(), dependent_lane);
        queue.push(seed.clone());
    }
    while let Some(node) = queue.pop() {
        let lane = desired[&node];
        let mut next: BTreeSet<(String, String)> =
            graph.edges.get(&node).cloned().unwrap_or_default();
        if let Some(more) = extra_edges.get(&node) {
            next.extend(more.iter().cloned());
        }
        for target in next {
            if target == node {
                continue;
            }
            if !graph.open.get(&target).copied().unwrap_or(false) {
                continue;
            }
            if graph
                .scheduled
                .get(&target)
                .copied()
                .flatten()
                .is_some_and(|scheduled| scheduled > ctx.anchor)
            {
                continue;
            }
            let current = graph.status.get(&target).copied().unwrap_or(' ');
            let Some(current_lane) = lane_for_status(current) else {
                continue;
            };
            if current_lane >= lane {
                continue;
            }
            let should = desired.get(&target).is_none_or(|known| *known < lane);
            if should {
                desired.insert(target.clone(), lane);
                queue.push(target);
            }
        }
    }
    let mut touches = Vec::new();
    for (identity, lane) in desired {
        let current = graph.status.get(&identity).copied().unwrap_or(' ');
        if lane_for_status(current).is_some_and(|known| known >= lane) {
            continue;
        }
        let (note_display, block_id) = (&identity.0, identity.1.as_str());
        let relative = PathBuf::from(note_display);
        let absolute = absolute_note(bob_dir, &relative);
        // Preimage check: the staged task must still carry the status
        // the promotion was decided from.
        let contents = current_note_text(planner, &absolute)?;
        let scan = note_tasks::scan(&contents, ctx.settings());
        let task = lookup_staged_task(&scan, note_display, block_id)?;
        if task.status_symbol != current {
            return Err(CaptureError::io(format!(
                "task ^{block_id} in {note_display} changed while planning {dependent_desc}; refusing to overwrite it"
            )));
        }
        let lines: Vec<&str> = contents.lines().collect();
        let line = lines.get(task.line_index).copied().unwrap_or_default();
        let Some(rebuilt) = replace_status_char(line, lane_checkbox(lane))
        else {
            return Err(CaptureError::io(format!(
                "task ^{block_id} in {note_display} has no status box to promote for {dependent_desc}"
            )));
        };
        if rebuilt != line {
            let mut note = EditableNote::new(&contents);
            note.lines[task.line_index] = rebuilt;
            planner.stage(&absolute, note.join())?;
            touches.push(TargetTouch {
                absolute,
                relative,
                block_id: block_id.to_string(),
                line: task.line_index,
            });
        }
    }
    Ok(touches)
}

/// Find the vault task carrying one `[id::]` field value: the unique
/// `(note, block)` pair whose task line holds it. Ambiguous, missing,
/// or ID-less matches do not adopt.
fn find_task_by_field_id(
    ctx: &DependencyContext,
    planner: &mut CaptureBatchPlanner,
    bob_dir: &Path,
    field_id: &str,
) -> Result<Option<(PathBuf, String)>, CaptureError> {
    let mut rels: BTreeSet<PathBuf> = ctx
        .catalog()
        .index
        .relative_paths()
        .map(Path::to_path_buf)
        .collect();
    for (path, _) in planner.staged_snapshot() {
        if let Ok(relative) = path.strip_prefix(bob_dir)
            && relative
                .extension()
                .is_some_and(|extension| extension.eq_ignore_ascii_case("md"))
        {
            rels.insert(relative.to_path_buf());
        }
    }
    let mut matches = Vec::new();
    for relative in &rels {
        let absolute = absolute_note(bob_dir, relative);
        let contents = match planner.current_contents(&absolute)? {
            Some(staged) => staged,
            None => match std::fs::read_to_string(&absolute) {
                Ok(disk) => disk,
                Err(_) => continue,
            },
        };
        let scan = note_tasks::scan(&contents, ctx.settings());
        for task in scan.tasks() {
            let Some(block_id) = task.block_id.as_deref() else {
                continue;
            };
            let lines: Vec<&str> = contents.lines().collect();
            let line = lines.get(task.line_index).copied().unwrap_or_default();
            if task_status_hooks::task_metadata(line, Some(block_id))
                .task_id
                .as_deref()
                == Some(field_id)
            {
                matches.push((relative.clone(), block_id.to_string()));
            }
        }
    }
    matches.sort();
    matches.dedup();
    Ok((matches.len() == 1).then(|| matches.remove(0)))
}

/// One merged Depends-On member: its identity, canonical link text,
/// and projected field id (`None` for kept-verbatim links that never
/// project).
struct MergedMember {
    identity: (String, String),
    link_text: String,
    projected_id: Option<String>,
}

/// What one managed-line link resolves to against staged contents.
enum LinkIdentity {
    /// A resolved task with its existing `[id::]` when one is present.
    Task { id: Option<String> },
    /// An archive-history task with its existing `[id::]` if any.
    /// Archive notes are never stamped.
    Archive { id: Option<String> },
    /// The note resolves but carries no such task (or cannot be read):
    /// kept verbatim, never blocking, like the hooks' R4.
    Unresolved,
    /// The block exists but is not a task: kept verbatim.
    NonTask,
    /// The link names the dependent itself: kept verbatim here; new
    /// self-edges refuse through the cycle check.
    SelfTarget,
}

/// Resolve one managed-line link against staged contents.
fn read_identity_for_link(
    ctx: &DependencyContext,
    planner: &mut CaptureBatchPlanner,
    target_rel: &Path,
    absolute: &Path,
    block_id: &str,
    dependent: &(String, String),
) -> LinkIdentity {
    if (display_relative(target_rel), block_id.to_string()) == *dependent {
        return LinkIdentity::SelfTarget;
    }
    let Some(contents) = planner_staged_or_disk(planner, absolute) else {
        return LinkIdentity::Unresolved;
    };
    let scan = note_tasks::scan(&contents, ctx.settings());
    match scan.by_block_id(block_id) {
        note_tasks::BlockIdLookup::Found(task) => {
            let lines: Vec<&str> = contents.lines().collect();
            let line = lines.get(task.line_index).copied().unwrap_or_default();
            let id =
                task_status_hooks::task_metadata(line, Some(block_id)).task_id;
            if task_dependencies::is_archive_path(target_rel) {
                LinkIdentity::Archive { id }
            } else {
                LinkIdentity::Task { id }
            }
        }
        note_tasks::BlockIdLookup::NotATask { .. } => LinkIdentity::NonTask,
        note_tasks::BlockIdLookup::Duplicate(_)
        | note_tasks::BlockIdLookup::Missing => LinkIdentity::Unresolved,
    }
}

/// Staged contents when the batch touched the note, else what is on
/// disk; `None` when neither is readable.
fn planner_staged_or_disk(
    planner: &mut CaptureBatchPlanner,
    absolute: &Path,
) -> Option<String> {
    if let Ok(Some(staged)) = planner.current_contents(absolute) {
        return Some(staged);
    }
    std::fs::read_to_string(absolute).ok()
}

/// The vault id an archive target projects without ever stamping:
/// its `[id::]`, else the canonical id, else `None`.
fn archive_projected_id(
    ctx: &DependencyContext,
    planner: &mut CaptureBatchPlanner,
    target_rel: &Path,
    absolute: &Path,
    block_id: &str,
) -> Option<String> {
    let contents = planner_staged_or_disk(planner, absolute)?;
    let scan = note_tasks::scan(&contents, ctx.settings());
    let task = match scan.by_block_id(block_id) {
        note_tasks::BlockIdLookup::Found(task) => task,
        _ => return None,
    };
    let lines: Vec<&str> = contents.lines().collect();
    let line = lines.get(task.line_index).copied().unwrap_or_default();
    if let Some(existing) =
        task_status_hooks::task_metadata(line, Some(block_id)).task_id
    {
        return Some(existing);
    }
    task_dependencies::dependency_id(target_rel, block_id).ok()
}

/// Verbatim `[[...]]` text of one managed-line link, sliced from the
/// original child line so aliases, embeds, and strikes round-trip.
fn verbatim_link(
    child_line: &str,
    link_index: usize,
    link: &task_dependencies::parse::DependencyLink,
) -> String {
    let spans = task_dependencies::block_link_spans(child_line);
    spans.get(link_index).map_or_else(
        || format!("[[{}#^{}]]", link.target, link.block_id),
        |span| child_line[span.full_start..span.full_end].to_string(),
    )
}

/// Merge new prerequisites into an existing dependent's managed line:
/// existing links keep order, covered legacy children fold, field-only
/// dependencies adopt, and typed modifiers append. Returns the merged
/// members in line order, the pre-existing identity set (for cycle and
/// already-present checks), whether an unresolvable link was kept
/// verbatim, and the folded legacy line removals. Every side-effect
/// stamp staged along the way is pushed to `touches`.
#[allow(clippy::too_many_arguments)]
fn merge_existing_members(
    ctx: &DependencyContext,
    planner: &mut CaptureBatchPlanner,
    bob_dir: &Path,
    index: &NoteIndex,
    dep_rel: &Path,
    dependent: &(String, String),
    dep_desc: &str,
    managed_text: Option<&str>,
    existing_links: &[task_dependencies::parse::DependencyLink],
    legacy: &[(usize, task_dependencies::legacy::LegacyChildLink)],
    field_ids: &[String],
    prereqs: &[ResolvedPrerequisite],
    warnings: &mut Vec<String>,
    touches: &mut Vec<TargetTouch>,
) -> Result<
    (
        Vec<MergedMember>,
        BTreeSet<(String, String)>,
        bool,
        Vec<usize>,
    ),
    CaptureError,
> {
    let mut merged: Vec<MergedMember> = Vec::new();
    let mut known: BTreeSet<(String, String)> = BTreeSet::new();
    let mut accounted_ids: BTreeSet<String> = BTreeSet::new();
    let mut kept_unresolved = false;
    let child_text = managed_text.unwrap_or_default();
    // Existing line links keep order; resolved links canonicalize like
    // the hooks rewrite, while unresolvable content keeps its text.
    for (link_index, link) in existing_links.iter().enumerate() {
        let target = link.target.trim();
        let Some(target_rel) = index.resolve(Some(dep_rel), target) else {
            kept_unresolved = true;
            merged.push(MergedMember {
                identity: (target.to_string(), link.block_id.clone()),
                link_text: verbatim_link(child_text, link_index, link),
                projected_id: None,
            });
            continue;
        };
        let absolute = absolute_note(bob_dir, &target_rel);
        match read_identity_for_link(
            ctx,
            planner,
            &target_rel,
            &absolute,
            &link.block_id,
            dependent,
        ) {
            LinkIdentity::Task { id } => {
                let id = match id {
                    Some(id) => id,
                    None => {
                        let (stamped, line) = project_prerequisite_id(
                            ctx,
                            planner,
                            &target_rel,
                            &absolute,
                            &link.block_id,
                            dep_desc,
                        )?;
                        if let Some(line) = line {
                            touches.push(TargetTouch {
                                absolute: absolute.clone(),
                                relative: target_rel.clone(),
                                block_id: link.block_id.clone(),
                                line,
                            });
                        }
                        stamped
                    }
                };
                let identity =
                    (display_relative(&target_rel), link.block_id.clone());
                known.insert(identity.clone());
                accounted_ids.insert(id.clone());
                merged.push(MergedMember {
                    identity,
                    link_text: prerequisite_link_text(
                        index,
                        dep_rel,
                        &target_rel,
                        &link.block_id,
                    ),
                    projected_id: Some(id),
                });
            }
            LinkIdentity::Archive { id } => {
                let identity =
                    (display_relative(&target_rel), link.block_id.clone());
                known.insert(identity.clone());
                let projected = match id {
                    Some(id) => {
                        accounted_ids.insert(id.clone());
                        Some(id)
                    }
                    None => archive_projected_id(
                        ctx,
                        planner,
                        &target_rel,
                        &absolute,
                        &link.block_id,
                    ),
                };
                if let Some(id) = &projected {
                    accounted_ids.insert(id.clone());
                }
                merged.push(MergedMember {
                    identity,
                    link_text: prerequisite_link_text(
                        index,
                        dep_rel,
                        &target_rel,
                        &link.block_id,
                    ),
                    projected_id: projected,
                });
            }
            LinkIdentity::Unresolved => {
                kept_unresolved = true;
                merged.push(MergedMember {
                    identity: (
                        display_relative(&target_rel),
                        link.block_id.clone(),
                    ),
                    link_text: verbatim_link(child_text, link_index, link),
                    projected_id: None,
                });
            }
            LinkIdentity::NonTask | LinkIdentity::SelfTarget => {
                // Pre-existing content the hooks keeps verbatim: never
                // projected, never dropped by this write.
                let identity =
                    (display_relative(&target_rel), link.block_id.clone());
                known.insert(identity.clone());
                merged.push(MergedMember {
                    identity,
                    link_text: verbatim_link(child_text, link_index, link),
                    projected_id: None,
                });
            }
        }
    }
    // Covered legacy children fold into the line; uncovered ones are
    // content and stay. Unresolvable legacy targets refuse rather than
    // lose the relationship.
    let mut remove_lines = Vec::new();
    for (line_index, legacy) in legacy {
        let target = legacy.target.trim();
        let Some(target_rel) = index.resolve(Some(dep_rel), target) else {
            return Err(CaptureError::usage(format!(
                "cannot merge dependencies for {dep_desc}: legacy task link '{target}#^{block}' did not resolve uniquely; repair or remove that child line, then capture again",
                block = legacy.block_id
            )));
        };
        let absolute = absolute_note(bob_dir, &target_rel);
        let identity = read_identity_for_link(
            ctx,
            planner,
            &target_rel,
            &absolute,
            &legacy.block_id,
            dependent,
        );
        let cover_id = match &identity {
            LinkIdentity::Task { id } => id.clone().or_else(|| {
                task_dependencies::dependency_id(&target_rel, &legacy.block_id)
                    .ok()
            }),
            LinkIdentity::Archive { id } => id.clone().or_else(|| {
                task_dependencies::dependency_id(&target_rel, &legacy.block_id)
                    .ok()
            }),
            LinkIdentity::Unresolved
            | LinkIdentity::NonTask
            | LinkIdentity::SelfTarget => None,
        };
        if !legacy_covered(
            field_ids,
            cover_id.as_deref(),
            &target_rel,
            &legacy.block_id,
            dep_rel,
        ) {
            continue;
        }
        let projected = match identity {
            LinkIdentity::Task { id } => match id {
                Some(id) => Some(id),
                None => {
                    let (stamped, line) = project_prerequisite_id(
                        ctx,
                        planner,
                        &target_rel,
                        &absolute,
                        &legacy.block_id,
                        dep_desc,
                    )?;
                    if let Some(line) = line {
                        touches.push(TargetTouch {
                            absolute: absolute.clone(),
                            relative: target_rel.clone(),
                            block_id: legacy.block_id.clone(),
                            line,
                        });
                    }
                    Some(stamped)
                }
            },
            LinkIdentity::Archive { id } => match id {
                Some(id) => Some(id),
                None => archive_projected_id(
                    ctx,
                    planner,
                    &target_rel,
                    &absolute,
                    &legacy.block_id,
                ),
            },
            LinkIdentity::Unresolved
            | LinkIdentity::NonTask
            | LinkIdentity::SelfTarget => None,
        };
        let Some(id) = projected else {
            return Err(CaptureError::usage(format!(
                "cannot merge dependencies for {dep_desc}: legacy task link '{target}#^{block}' has no usable [id::]; add one to that task, then capture again",
                block = legacy.block_id
            )));
        };
        let identity = (display_relative(&target_rel), legacy.block_id.clone());
        if known.insert(identity.clone()) {
            accounted_ids.insert(id.clone());
            merged.push(MergedMember {
                identity,
                link_text: prerequisite_link_text(
                    index,
                    dep_rel,
                    &target_rel,
                    &legacy.block_id,
                ),
                projected_id: Some(id),
            });
        }
        remove_lines.push(*line_index);
    }
    // Field-only dependencies adopt before typed modifiers are added:
    // unaccounted field ids resolve vault-wide, staying in field order.
    let mut stale_ids = Vec::new();
    for field_id in field_ids {
        if accounted_ids.contains(field_id) {
            continue;
        }
        match find_task_by_field_id(ctx, planner, bob_dir, field_id)? {
            Some((target_rel, block_id)) => {
                let identity =
                    (display_relative(&target_rel), block_id.clone());
                if known.insert(identity.clone()) {
                    accounted_ids.insert(field_id.clone());
                    merged.push(MergedMember {
                        link_text: prerequisite_link_text(
                            index,
                            dep_rel,
                            &target_rel,
                            &block_id,
                        ),
                        identity,
                        projected_id: Some(field_id.clone()),
                    });
                }
            }
            None => stale_ids.push(field_id.clone()),
        }
    }
    // Typed modifiers append; repeats and already-present links are
    // idempotent, not toggles and not reorder operations. Snapshot the
    // pre-existing set first: cycle and already-present checks must see
    // the line as this capture found it, not as it appends to it.
    let pre_existing = known.clone();
    for prereq in prereqs {
        let identity =
            (display_relative(&prereq.relative), prereq.block_id.clone());
        if known.insert(identity.clone()) {
            accounted_ids.insert(prereq.projected_id.clone());
            merged.push(MergedMember {
                identity,
                link_text: prereq.link_text.clone(),
                projected_id: Some(prereq.projected_id.clone()),
            });
        }
    }
    // Stale field ids drop like the hooks' R1 projection, except while
    // an unhealable link remains, when they stay on as heal breadcrumbs.
    let mut dropped = Vec::new();
    if !kept_unresolved {
        for stale in stale_ids {
            if merged.iter().any(|member| {
                member.projected_id.as_deref() == Some(stale.as_str())
            }) {
                continue;
            }
            dropped.push(stale);
        }
    }
    if !dropped.is_empty() {
        warnings.push(format!(
            "dropped unaccounted field ids {}",
            dropped.join(", ")
        ));
    }
    Ok((merged, pre_existing, kept_unresolved, remove_lines))
}

/// Apply a dependency-only capture to an explicitly selected existing
/// task: merge its managed Depends-On line and derived effects through
/// staged writes with validation, then report the final preview.
///
/// The dependent must be an open task; completed dependents never
/// reopen. Repeats are idempotent. A later failing batch item aborts
/// planning before anything is committed, so no partial dependency
/// state can land.
#[allow(clippy::too_many_arguments)]
pub(super) fn plan_existing_task_dependencies(
    ctx: &mut DependencyContext,
    planner: &mut CaptureBatchPlanner,
    bob_dir: &Path,
    dependent_route: &str,
    dependent_block_id: &str,
    deps: &[ParsedDependency],
    warnings: &mut Vec<String>,
) -> Result<PlannedDependencyUpdate, CaptureError> {
    let dep_rel = PathBuf::from(format!("{dependent_route}.md"));
    let dep_abs = absolute_note(bob_dir, &dep_rel);
    let dep_desc =
        format!("^{dependent_block_id} in {}", display_relative(&dep_rel));
    let dependent =
        (display_relative(&dep_rel), dependent_block_id.to_string());
    // `&` writes plan against the dependency pool: warm the full vault
    // scan now so every resolution below sees it.
    ctx.ensure_discovered();
    let index = staged_note_index(ctx, planner, bob_dir);
    // Resolve prerequisites first: every target-ID stamp lands before
    // the dependent's own edits resolve, so same-note writes compose.
    let (prereqs, repeats, mut touches) = resolve_prerequisites(
        ctx,
        planner,
        bob_dir,
        &index,
        &dep_rel,
        Some(dependent_block_id),
        deps,
        &dep_desc,
    )?;
    let contents = current_note_text(planner, &dep_abs)?;
    let scan = note_tasks::scan(&contents, ctx.settings());
    let task = lookup_staged_task(&scan, dependent_route, dependent_block_id)?;
    if task.status_type.is_terminal() || !task.status_type.is_open() {
        return Err(CaptureError::io(format!(
            "task ^{dependent_block_id} in {dependent_route}.md is {}; dependencies cannot reopen a completed task",
            task.status_name
        )));
    }
    let dependent_lane = lane_for_status(task.status_symbol);
    let dependent_text = task.description.clone();
    let dependent_line_index = task.line_index;
    let dependent_digest = task.digest.clone();
    let lines: Vec<&str> = contents.lines().collect();
    let task_line = lines
        .get(dependent_line_index)
        .copied()
        .unwrap_or_default()
        .to_string();
    let metadata =
        task_status_hooks::task_metadata(&task_line, Some(dependent_block_id));
    let fenced = task_status_hooks::fenced_lines(&lines, 0..lines.len());
    let managed = managed_children(&lines, &fenced, dependent_line_index);
    if managed.len() > 1 {
        return Err(CaptureError::usage(format!(
            "cannot merge dependencies for {dep_desc}: {count} managed Depends-On lines; keep one canonical '- ⛓️ **DEPENDS ON:** ...' child line, then capture again",
            count = managed.len()
        )));
    }
    let (managed_at, existing_links, managed_text) = match managed.first() {
        None => (None, Vec::new(), None),
        Some((line_index, parsed)) => match parsed {
            dependency_parse::DependencyLine::Accepted { links, .. } => (
                Some(*line_index),
                links.clone(),
                lines.get(*line_index).copied(),
            ),
            dependency_parse::DependencyLine::Empty => (
                Some(*line_index),
                Vec::new(),
                lines.get(*line_index).copied(),
            ),
            dependency_parse::DependencyLine::Malformed => {
                return Err(CaptureError::usage(format!(
                    "cannot merge dependencies for {dep_desc}: the managed Depends-On line (line {}) is malformed; repair it to a single '- ⛓️ **DEPENDS ON:** [[...]]' child line or remove it, then capture again",
                    line_index + 1
                )));
            }
            dependency_parse::DependencyLine::NotALine => {
                (None, Vec::new(), None)
            }
        },
    };
    let legacy_all =
        legacy_children(&lines, &fenced, dependent_line_index, managed_at);
    let (merged, pre_existing, kept_unresolved, remove_lines) =
        merge_existing_members(
            ctx,
            planner,
            bob_dir,
            &index,
            &dep_rel,
            &dependent,
            &dep_desc,
            managed_text,
            &existing_links,
            &legacy_all,
            &metadata.depends_on,
            &prereqs,
            warnings,
            &mut touches,
        )?;
    // New edges refuse self-dependencies and fresh cycles; no-op
    // repeats skip validation so unrelated graph problems never fail
    // them.
    let graph = DependencyGraph::build(ctx, planner, bob_dir, &index)?;
    let merged_identities: Vec<(String, String)> = merged
        .iter()
        .map(|member| member.identity.clone())
        .collect();
    reject_new_cycles(&graph, &dependent, &pre_existing, &merged_identities)?;
    let mut extra: BTreeMap<(String, String), BTreeSet<(String, String)>> =
        BTreeMap::new();
    for target in &merged_identities {
        if !pre_existing.contains(target) {
            extra
                .entry(dependent.clone())
                .or_default()
                .insert(target.clone());
        }
    }
    // Openness comes from staged truth: closed, archive, and
    // unresolvable prerequisites never block.
    let mut open_count = 0usize;
    for member in &merged {
        if member.projected_id.is_none() {
            continue;
        }
        if graph.open.get(&member.identity).copied().unwrap_or(false) {
            open_count += 1;
        }
    }
    // Re-resolve the dependent against current staged text: same-note
    // target stamps above may have shifted it.
    let fresh = current_note_text(planner, &dep_abs)?;
    let fresh_scan = note_tasks::scan(&fresh, ctx.settings());
    let fresh_task =
        lookup_staged_task(&fresh_scan, dependent_route, dependent_block_id)?;
    if fresh_task.digest != dependent_digest {
        return Err(CaptureError::io(format!(
            "task ^{dependent_block_id} in {dependent_route}.md changed while planning {dep_desc}; refusing to overwrite it"
        )));
    }
    // Target-ID stamps rewrite lines in place, so pre-stamp indices
    // still address the fresh staged text.
    let mut note = EditableNote::new(&fresh);
    let fresh_at = fresh_task.line_index;
    let link_texts: Vec<String> = merged
        .iter()
        .map(|member| member.link_text.clone())
        .collect();
    // Merge into one canonical child line, first except after an
    // existing Cancel Log; preserve indentation, endings, and
    // unrelated content.
    let child_changed;
    if let Some(managed_idx) = managed_at {
        let original = note.lines.get(managed_idx).cloned().unwrap_or_default();
        let rebuilt =
            task_status_hooks::rebuild_child_line(&original, &link_texts);
        child_changed = rebuilt != original;
        if child_changed {
            note.lines[managed_idx] = rebuilt;
        }
        let mut folded = remove_lines.clone();
        folded.sort();
        folded.dedup();
        folded.reverse();
        for line_index in folded {
            if line_index < note.lines.len() {
                note.lines.remove(line_index);
            }
        }
    } else {
        let slot = cancel_log_aware_slot(&note, fresh_at);
        let indent = first_child_indent(&note, fresh_at);
        let new_child =
            dependency_format::format_dependency_line(&indent, &link_texts);
        note.lines.insert(slot, new_child);
        child_changed = true;
        let mut folded = remove_lines.clone();
        folded.sort();
        folded.dedup();
        folded.reverse();
        for mut line_index in folded {
            if line_index >= slot {
                line_index += 1;
            }
            if line_index < note.lines.len() {
                note.lines.remove(line_index);
            }
        }
    }
    // Derive fields and status on the dependent line. Freshness stamps
    // only when this gesture actually edits the dependent; the stamp
    // lands before fields so placement stays after freshness and
    // before `^block-id`.
    let projected: Vec<String> = merged
        .iter()
        .filter_map(|member| member.projected_id.clone())
        .collect();
    let mut kept_field = metadata.depends_on.clone();
    if !kept_unresolved {
        kept_field.clear();
    }
    for id in &projected {
        if !kept_field.contains(id) {
            kept_field.push(id.clone());
        }
    }
    let own_id = metadata.task_id.or_else(|| {
        task_dependencies::dependency_id(&dep_rel, dependent_block_id).ok()
    });
    let joined = note.join();
    let joined_scan = note_tasks::scan(&joined, ctx.settings());
    let joined_task =
        lookup_staged_task(&joined_scan, dependent_route, dependent_block_id)?;
    let current_line = note
        .lines
        .get(joined_task.line_index)
        .cloned()
        .unwrap_or_default();
    let mut status_char = fresh_task.status_symbol;
    let mut status_name = fresh_task.status_name.clone();
    let mut status_changed = false;
    if open_count > 0 && status_char != '?' {
        task_status_hooks::validate_blocked_status(ctx.settings())
            .map_err(|error| CaptureError::io(error.message().to_string()))?;
        status_char = '?';
        status_name = "Blocked".to_string();
        status_changed = true;
    }
    // Fields first (dry): the stamp decision needs to know whether
    // this gesture actually edits the dependent, and the stamp itself
    // must not count as the edit.
    let uncommitted_fields = task_status_hooks::set_task_fields(
        &current_line,
        own_id.as_deref(),
        &kept_field,
    );
    let fields_changed = uncommitted_fields != current_line;
    let stamped_base = if child_changed || fields_changed || status_changed {
        let stamped = stamp_fresh(&current_line, ctx.today);
        if stamped.refused.is_none() {
            stamped.line
        } else {
            current_line.clone()
        }
    } else {
        current_line.clone()
    };
    let mut next_line = task_status_hooks::set_task_fields(
        &stamped_base,
        own_id.as_deref(),
        &kept_field,
    );
    if status_changed
        && let Some(rebuilt) = replace_status_char(&next_line, status_char)
    {
        next_line = rebuilt;
    }
    note.lines[joined_task.line_index] = next_line.clone();
    planner.stage(&dep_abs, note.join())?;
    // Promote open prerequisites to the dependent's pre-capture lane.
    if let Some(lane) = dependent_lane
        && lane > 0
    {
        let seeds: Vec<(String, String)> = merged_identities.clone();
        let promoted = plan_promotions(
            ctx, planner, bob_dir, &graph, &extra, lane, &seeds, &dep_desc,
        )?;
        touches.extend(promoted);
    }
    let added = merged_identities
        .iter()
        .filter(|identity| !pre_existing.contains(*identity))
        .count();
    let typed_known: BTreeSet<(String, String)> = prereqs
        .iter()
        .map(|prereq| {
            (display_relative(&prereq.relative), prereq.block_id.clone())
        })
        .collect();
    // Already-present tallies typed modifiers the merge did not need
    // to add (pre-existing links plus in-draft repeats); `added` counts
    // merged members that are new to the line.
    let already_present =
        typed_known.intersection(&pre_existing).count() + repeats;
    let _ = kept_unresolved;
    let summary = DependencyUpdateJson {
        dependent_note: display_relative(&dep_rel),
        dependent_block_id: Some(dependent_block_id.to_string()),
        dependent_text,
        new_task: false,
        added,
        already_present,
        open_prerequisites: open_count,
        prerequisites: prereqs.iter().map(prerequisite_json).collect(),
        dependent_status: status_char,
        dependent_status_name: status_name,
        status_changed,
    };
    let final_contents = current_note_text(planner, &dep_abs)?;
    let final_scan = note_tasks::scan(&final_contents, ctx.settings());
    let final_task =
        lookup_staged_task(&final_scan, dependent_route, dependent_block_id)?;
    Ok(PlannedDependencyUpdate {
        summary,
        dependent_ref: TaskBlockRef {
            target: dep_abs,
            relative_target: display_relative(&dep_rel),
            route: dependent_route.to_string(),
            line: final_task.line_index,
            block_id: Some(dependent_block_id.to_string()),
            role: TaskBlockRole::Dependency,
        },
        target_refs: touches
            .into_iter()
            .map(|touch| target_block_ref(&touch))
            .collect(),
    })
}

/// Augmented new-task block: the dependency-ready task line plus its
/// managed Depends-On child (already indented), computed before
/// insertion so duplicate bodies in one batch each land correctly.
#[derive(Debug, Clone)]
pub(super) struct AugmentedNewTask {
    pub(super) task_line: String,
    pub(super) dep_child: String,
}

/// Plan dependencies for a task this capture creates: resolve and
/// validate every prerequisite (staging target-ID preparation and
/// promotion), then derive the augmented task line and managed child.
/// Prerequisite edits stage immediately; the dependent block inserts
/// through the normal capture path afterwards.
#[allow(clippy::too_many_arguments)]
pub(super) fn plan_new_task_dependencies(
    ctx: &mut DependencyContext,
    planner: &mut CaptureBatchPlanner,
    bob_dir: &Path,
    dependent_route: Option<&str>,
    new_block_id: Option<&str>,
    capture_line: &str,
    capture_status: char,
    child_indent: Option<&str>,
    deps: &[ParsedDependency],
    warnings: &mut Vec<String>,
) -> Result<(AugmentedNewTask, PlannedDependencyUpdateParts), CaptureError> {
    let dep_rel = relative_target(dependent_route);
    let dep_desc = match new_block_id {
        Some(block_id) => {
            format!("^{block_id} in {}", display_relative(&dep_rel))
        }
        None => format!("new task in {}", display_relative(&dep_rel)),
    };
    // `&` writes plan against the dependency pool: warm the full vault
    // scan now so every resolution below sees it.
    ctx.ensure_discovered();
    let index = staged_note_index(ctx, planner, bob_dir);
    let (prereqs, repeats, mut touches) = resolve_prerequisites(
        ctx,
        planner,
        bob_dir,
        &index,
        &dep_rel,
        new_block_id,
        deps,
        &dep_desc,
    )?;
    let graph = DependencyGraph::build(ctx, planner, bob_dir, &index)?;
    // The merged set is exactly the typed set for a new task; cycle
    // validation still applies when the new task carries a block ID
    // (an ID-less task cannot be referenced, so it cannot cycle).
    let merged_identities: Vec<(String, String)> = prereqs
        .iter()
        .map(|prereq| {
            (display_relative(&prereq.relative), prereq.block_id.clone())
        })
        .collect();
    let dependent = (
        display_relative(&dep_rel),
        new_block_id.unwrap_or_default().to_string(),
    );
    if new_block_id.is_some() {
        reject_new_cycles(
            &graph,
            &dependent,
            &BTreeSet::new(),
            &merged_identities,
        )?;
    }
    let mut extra: BTreeMap<(String, String), BTreeSet<(String, String)>> =
        BTreeMap::new();
    if new_block_id.is_some() {
        extra.insert(
            dependent.clone(),
            merged_identities.iter().cloned().collect(),
        );
    }
    let open_count = prereqs.iter().filter(|prereq| prereq.open).count();
    let mut status_char = capture_status;
    let mut status_name = status_name_for(capture_status).to_string();
    let mut status_changed = false;
    if open_count > 0 && status_char != '?' {
        task_status_hooks::validate_blocked_status(ctx.settings())
            .map_err(|error| CaptureError::io(error.message().to_string()))?;
        status_char = '?';
        status_name = "Blocked".to_string();
        status_changed = true;
    }
    let own_id = new_block_id.and_then(|block_id| {
        task_dependencies::dependency_id(&dep_rel, block_id).ok()
    });
    let projected: Vec<String> = prereqs
        .iter()
        .map(|prereq| prereq.projected_id.clone())
        .collect();
    let mut task_line = capture_line.to_string();
    if status_changed
        && let Some(rebuilt) = replace_status_char(&task_line, status_char)
    {
        task_line = rebuilt;
    }
    task_line = task_status_hooks::set_task_fields(
        &task_line,
        own_id.as_deref(),
        &projected,
    );
    let link_texts: Vec<String> = prereqs
        .iter()
        .map(|prereq| prereq.link_text.clone())
        .collect();
    let indent = child_indent.map(str::to_string).unwrap_or_else(|| {
        dependency_format::child_indent_for_parent("", None)
    });
    let dep_child =
        dependency_format::format_dependency_line(&indent, &link_texts);
    // Promote to the new task's pre-Blocked lane (Next for Pomodoro
    // tasks); Ready and scheduled tasks promote nothing.
    if let Some(lane) = lane_for_status(capture_status)
        && lane > 0
    {
        let promoted = plan_promotions(
            ctx,
            planner,
            bob_dir,
            &graph,
            &extra,
            lane,
            &merged_identities,
            &dep_desc,
        )?;
        touches.extend(promoted);
    }
    let _ = warnings;
    let summary = DependencyUpdateJson {
        dependent_note: display_relative(&dep_rel),
        dependent_block_id: new_block_id.map(str::to_string),
        dependent_text: task_description_of(capture_line),
        new_task: true,
        added: prereqs.len(),
        already_present: repeats,
        open_prerequisites: open_count,
        prerequisites: prereqs.iter().map(prerequisite_json).collect(),
        dependent_status: status_char,
        dependent_status_name: status_name,
        status_changed,
    };
    Ok((
        AugmentedNewTask {
            task_line,
            dep_child,
        },
        PlannedDependencyUpdateParts {
            summary,
            target_refs: touches
                .into_iter()
                .map(|touch| target_block_ref(&touch))
                .collect(),
            relative_target: display_relative(&dep_rel),
            route: dependent_route.map(str::to_string).unwrap_or_default(),
            block_id: new_block_id.map(str::to_string),
        },
    ))
}

/// The post-insert half of a new-task dependency plan: everything but
/// the dependent's own task-block ref, which needs its inserted line.
#[derive(Debug, Clone)]
pub(super) struct PlannedDependencyUpdateParts {
    pub(super) summary: DependencyUpdateJson,
    pub(super) target_refs: Vec<TaskBlockRef>,
    pub(super) relative_target: String,
    pub(super) route: String,
    pub(super) block_id: Option<String>,
}

/// Locate a just-inserted new task's final line for its task block:
/// the last full-line match of the augmented task line in staged text.
pub(super) fn locate_new_task_ref(
    planner: &mut CaptureBatchPlanner,
    bob_dir: &Path,
    parts: &PlannedDependencyUpdateParts,
    task_line: &str,
) -> Option<TaskBlockRef> {
    let absolute = bob_dir.join(&parts.relative_target);
    let staged = planner.peek_text(&absolute)?;
    let staged_lines: Vec<&str> = staged.lines().collect();
    let line = staged_lines.iter().rposition(|line| *line == task_line)?;
    Some(TaskBlockRef {
        target: absolute,
        relative_target: parts.relative_target.clone(),
        route: parts.route.clone(),
        line,
        block_id: parts.block_id.clone(),
        role: TaskBlockRole::Dependency,
    })
}

/// Status display name for a fresh capture line's checkbox.
fn status_name_for(status: char) -> &'static str {
    match status {
        ' ' => "Todo",
        '?' => "Blocked",
        '*' => "Next",
        '/' => "In Progress",
        'x' => "Done",
        '-' => "Canceled",
        _ => "Todo",
    }
}

/// Best-effort task text from a formatted capture line for previews.
fn task_description_of(capture_line: &str) -> String {
    let mut text = capture_line.to_string();
    if let Some(start) = text.find("#task ") {
        text = text[start + "#task ".len()..].to_string();
    } else if let Some(dash) = text.find("- ") {
        text = text[dash + 2..].to_string();
    }
    for marker in [" [created::", " [scheduled::", " ^"] {
        if let Some(start) = text.find(marker) {
            text.truncate(start);
        }
    }
    text.trim().to_string()
}

/// Plan one dependency-only item into its existing-task update: no new
/// task, empty child, link toggle, or Pomodoro operation. Reports kind
/// `task_dependency` with the final dependent block and the
/// `dependency_update` preview; it is never labeled a task creation or
/// Pomodoro link.
pub(super) fn plan_dependency_only_item(
    request: &CaptureRequest,
    parsed: &ParsedCaptureText,
    target: &ParsedDependencyTarget,
    planner: &mut CaptureBatchPlanner,
    warnings: &mut Vec<String>,
    ctx: &mut DependencyContext,
) -> Result<PlannedCaptureItem, CaptureError> {
    let (route, block_id) = match target {
        ParsedDependencyTarget {
            route: Some(route),
            block_id: Some(block_id),
            ..
        } => (route.clone(), block_id.clone()),
        _ => {
            return Err(CaptureError::io(
                "dependency invariant failed: existing-task target without route and block ID",
            ));
        }
    };
    let relative_target = relative_target(Some(&route));
    let absolute = request.bob_dir.join(&relative_target);
    // Previous status for the preview comes from the pre-write staged
    // text; the update below re-resolves before staging.
    let previous = current_note_text(planner, &absolute)?;
    let previous_scan = note_tasks::scan(&previous, ctx.settings());
    let previous_task = lookup_staged_task(&previous_scan, &route, &block_id)?;
    let previous_line = previous
        .lines()
        .nth(previous_task.line_index)
        .unwrap_or_default()
        .to_string();
    let update = plan_existing_task_dependencies(
        ctx,
        planner,
        &request.bob_dir,
        &route,
        &block_id,
        &parsed.dependencies,
        warnings,
    )?;
    let summary = update.summary.clone();
    let final_contents = current_note_text(planner, &absolute)?;
    let final_line = final_contents
        .lines()
        .nth(update.dependent_ref.line)
        .unwrap_or_default()
        .to_string();
    let mut task_block_refs = vec![update.dependent_ref];
    task_block_refs.extend(update.target_refs);
    Ok(PlannedCaptureItem {
        result: CaptureItemResult {
            ok: true,
            dry_run: request.dry_run,
            routed: true,
            route: Some(route.clone()),
            route_label: route_label(&route),
            relative_target: relative_target.to_string_lossy().into_owned(),
            target: absolute.display().to_string(),
            text: summary.dependent_text.clone(),
            task_line: final_line,
            kind: "task_dependency",
            created: date_string(ctx.today),
            scheduled: None,
            priority: None,
            priority_label: None,
            placement: Placement::Updated,
            sub_bullets: Vec::new(),
            clip: None,
            schedule_log: None,
            block_id: Some(block_id.clone()),
            day_file: None,
            block_link: Some(format!("[[{route}#^{block_id}]]")),
            pomodoro_link_placement: None,
            parent_line: None,
            parent_text: None,
            parent_section: None,
            parent_status_symbol: None,
            parent_status_name: None,
            toggle_direction: None,
            previous_task_line: Some(previous_line),
            status_symbol: Some(summary.dependent_status),
            status_name: Some(summary.dependent_status_name.clone()),
            previous_status_symbol: Some(previous_task.status_symbol),
            previous_status_name: Some(previous_task.status_name.clone()),
            pomodoro_name: None,
            creates_pomodoro: None,
            pomodoro_already_linked: None,
            removed_pomodoro_links: None,
            removed_scheduled: None,
            pomodoro_selector_unused: None,
            toggle_behavior: None,
            status_changed: Some(summary.status_changed),
            pomodoro_link_action: None,
            pomodoro_link_source: None,
            pomodoro_link_destination: None,
            project_note: None,
            pomodoro_start: None,
            pomodoro_adjust: None,
            pomodoro_shift: None,
            pomodoro_close: None,
            pomodoro_reset: None,
            dependency_update: Some(summary),
            task_complete: None,
            toggle_task_description: None,
            r#ref: None,
        },
        clip_plan: None,
        pomodoro_refs: Vec::new(),
        task_block_refs,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::native::task_complete::recovery::recover_blocked_dependents;
    use std::fs;

    fn write_note(vault: &Path, relative: &str, contents: &str) {
        let path = vault.join(relative);
        fs::create_dir_all(path.parent().expect("parent")).expect("parent");
        fs::write(&path, contents).expect("write fixture note");
    }

    fn file_scans(
        snapshot: &[(PathBuf, String)],
        settings: &note_tasks::NoteTaskSettings,
    ) -> Vec<task_status_hooks::FileScan> {
        snapshot
            .iter()
            .map(|(relative, contents)| task_status_hooks::FileScan {
                path: relative.clone(),
                relative_path: relative.clone(),
                contents: contents.clone(),
                tasks: task_status_hooks::parse_tasks(contents, settings),
            })
            .collect()
    }

    fn normalized_states(
        snapshot: &[(PathBuf, String)],
        settings: &note_tasks::NoteTaskSettings,
    ) -> Vec<(PathBuf, usize, task_status_hooks::TaskDependencyState)> {
        let files = file_scans(snapshot, settings);
        let states =
            task_status_hooks::task_dependency_states(&files, &BTreeSet::new());
        let mut normalized: Vec<_> = states
            .into_iter()
            .map(|((file_index, task_index), state)| {
                (files[file_index].relative_path.clone(), task_index, state)
            })
            .collect();
        normalized
            .sort_by(|left, right| (&left.0, left.1).cmp(&(&right.0, right.1)));
        normalized
    }

    /// The prefiltered snapshot plus staged overlay yields the same
    /// `task_dependency_states` and the same `recover_blocked_dependents`
    /// result as a full vault read: same-note and cross-note edges, a
    /// `(id:: …)` paren form, a staged-new note, and a staged deletion.
    #[test]
    fn prefiltered_snapshot_matches_full_read() {
        let vault = tempfile::tempdir().expect("temp vault");
        let root = vault.path();
        write_note(
            root,
            ".obsidian/plugins/obsidian-tasks-plugin/data.json",
            r##"{
              "globalFilter": "#task",
              "statusSettings": {
                "coreStatuses": [
                  {"symbol":" ","name":"Ready","type":"TODO"},
                  {"symbol":"x","name":"Done","type":"DONE"}
                ],
                "customStatuses": [
                  {"symbol":"*","name":"Next","type":"ON_HOLD"},
                  {"symbol":"?","name":"Blocked","type":"TODO"},
                  {"symbol":"/","name":"In Progress","type":"IN_PROGRESS"},
                  {"symbol":"-","name":"Canceled","type":"CANCELLED"}
                ]
              }
            }"##,
        );
        write_note(
            root,
            "sase.md",
            "- [x] #task Done root [id:: done-root] ^done\n- [?] #task Waiting on root [dependsOn:: done-root] [id:: waiter] ^waiter\n",
        );
        write_note(
            root,
            "travel.md",
            "- [?] #task Cross waiter [dependsOn:: done-root] [id:: cross] ^cross\n",
        );
        write_note(
            root,
            "paren.md",
            "- [x] #task Paren root (id:: paren-root) ^proot\n- [?] #task Paren waiter (dependsOn:: paren-root) (id:: paren-waiter) ^pwaiter\n",
        );
        write_note(
            root,
            "prose.md",
            "# Field notes\nNothing here names a task identity or a prerequisite.\n",
        );
        write_note(
            root,
            "fence.md",
            "# Queries\n```dataview\nTABLE dependsOn, id::mine\n```\n",
        );
        let settings = note_tasks::read_settings(root);
        let today = chrono::NaiveDate::from_ymd_opt(2026, 10, 5).expect("date");
        let completed: BTreeSet<String> = ["done-root", "paren-root"]
            .iter()
            .map(|id| id.to_string())
            .collect();

        // Reference: the old full-vault read (every note, no prefilter)
        // with the same staged overlay applied.
        let mut full: BTreeMap<PathBuf, String> = BTreeMap::new();
        for absolute in
            task_status_hooks::markdown_files(root).expect("walk vault")
        {
            let relative = absolute
                .strip_prefix(root)
                .map(Path::to_path_buf)
                .expect("relative");
            full.insert(
                relative,
                fs::read_to_string(&absolute).expect("read note"),
            );
        }
        let mut staged: BTreeMap<PathBuf, Option<String>> = BTreeMap::new();
        staged.insert(
            PathBuf::from("new.md"),
            Some(
                "- [?] #task Staged waiter [dependsOn:: paren-root] [id:: staged-waiter] ^staged\n"
                    .to_string(),
            ),
        );
        staged.insert(PathBuf::from("travel.md"), None);
        for (relative, contents) in &staged {
            match contents {
                Some(text) => {
                    full.insert(relative.clone(), text.clone());
                }
                None => {
                    full.remove(relative);
                }
            }
        }
        let full_snapshot: Vec<(PathBuf, String)> = full.into_iter().collect();

        // New path: parallel prefiltered base plus staged overlay.
        let base = DependentsSnapshot::build(root);
        assert!(base.available);
        assert!(
            !base.base.contains_key(Path::new("prose.md")),
            "marker-free notes are excluded"
        );
        assert!(
            base.base.contains_key(Path::new("fence.md")),
            "fenced-only markers still keep the note"
        );
        let filtered_snapshot = base.overlaid(staged);

        // The prefiltered snapshot is smaller by design (prose.md is
        // excluded); what must match is every downstream result.
        assert!(
            filtered_snapshot.len() < full_snapshot.len(),
            "prefilter excluded the marker-free note"
        );

        // File indices shift when a note is excluded, so compare
        // states keyed by vault-relative path and in-file task order.
        let full_states = normalized_states(&full_snapshot, &settings);
        let filtered_states = normalized_states(&filtered_snapshot, &settings);
        assert_eq!(filtered_states, full_states);

        let full_recovery = recover_blocked_dependents(
            full_snapshot,
            &completed,
            today,
            &settings,
        );
        let filtered_recovery = recover_blocked_dependents(
            filtered_snapshot,
            &completed,
            today,
            &settings,
        );
        assert_eq!(filtered_recovery, full_recovery);
        let recovered: BTreeSet<&str> = filtered_recovery
            .recovered
            .iter()
            .filter_map(|dependent| dependent.block_id.as_deref())
            .collect();
        assert_eq!(recovered, BTreeSet::from(["waiter", "pwaiter", "staged"]));
    }
}
