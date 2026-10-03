//! Vault-wide prerequisite discovery for the `&` picker.
//!
//! A read-only scanner that lists every task in the vault — open and
//! closed, hidden and archived — annotated with its exact vault-relative
//! note identity, a short display locator, a stable picker group, and
//! the ID-assignment metadata the explicit Add block ID flow needs. The
//! `:` picker pool (`capture_link_tasks`) is untouched: this catalog
//! scans every task-bearing note (untyped root notes, nested folders,
//! ref notes, terminal projects, daily notes, and history), not just
//! routable capture targets, and it never requires today's ledger file.
//!
//! Note identities resolve through the same basename semantics as vault
//! links (an explicit relative path first, otherwise a unique
//! case-insensitive basename), without the lowercasing route parser, so
//! nested, case-sensitive, and quoted paths round-trip exactly. Ranking
//! reuses the shared tiered AND-term matcher from
//! [`capture_link_tasks::rank_fields`].

use std::{
    collections::{HashMap, HashSet},
    ffi::OsStr,
    fs,
    path::{Component, Path, PathBuf},
};

use super::{
    capture_block_ids, capture_tasks, is_always_excluded_note_directory_name,
    note_tasks::{self, TaskStatusType},
    task_dependencies,
    vault_links::NoteIndex,
};

/// One prerequisite candidate: any task in the vault that the `&`
/// picker can name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DependencyTask {
    /// Exact vault-relative note path including extension
    /// (`projects/foo.md`), case and Unicode preserved. Never a
    /// lowercased route.
    pub(crate) note_path: String,
    /// Short human display form: the basename when it is unique in the
    /// vault, otherwise the full relative path without `.md`.
    pub(crate) locator: String,
    pub(crate) block_id: Option<String>,
    pub(crate) block_id_suggestions: Vec<String>,
    pub(crate) status_symbol: char,
    pub(crate) status_name: String,
    pub(crate) status_type: TaskStatusType,
    pub(crate) text: String,
    pub(crate) section: Option<String>,
    pub(crate) depth: usize,
    pub(crate) line: usize,
    pub(crate) task_ref: String,
    /// A `#hide` task: subdued in the picker and ordered last within
    /// its section.
    pub(crate) hidden: bool,
    /// `false` for Done/Cancelled (and other terminal) tasks, which
    /// list in the separate completed-history section as non-blocking.
    pub(crate) open: bool,
    /// The task's block ID occurs more than once in its own note, so
    /// no replacement can name it unambiguously until the note is
    /// repaired.
    pub(crate) duplicate_id: bool,
}

/// Discovery output: canonically ordered candidates plus bounded
/// warnings. Unreadable notes yield a warning and never destroy the
/// good results.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DependencyTaskResult {
    pub(crate) tasks: Vec<DependencyTask>,
    pub(crate) warnings: Vec<String>,
    /// Basename index over the discovered note paths, for exact-path
    /// and unique-basename resolution without re-walking the vault.
    pub(crate) index: NoteIndex,
    /// Lowercased file stem to every sorted vault-relative display
    /// path carrying it, for ambiguous-basename diagnostics.
    pub(crate) basenames: HashMap<String, Vec<String>>,
}

/// Scan every task-bearing vault note for prerequisite candidates.
///
/// Included: untyped root notes, nested folders, ref notes, terminal
/// projects, daily notes, hidden tasks, and completed/cancelled/archive
/// history. Excluded: dot-directories, `_templates`, `_generated`,
/// `_conflicts` (plus the other always-excluded names), fenced code,
/// and non-task block anchors (the task scan only yields real task
/// lines, and the shared global filter still applies). Symlinked
/// directories are never descended; unreadable notes warn and continue.
pub(crate) fn discover(bob_dir: &Path) -> DependencyTaskResult {
    let mut warnings = Vec::new();
    let mut relative_paths = Vec::new();
    collect_note_paths(bob_dir, bob_dir, &mut relative_paths, &mut warnings);
    relative_paths.sort();
    let index = NoteIndex::from_paths(relative_paths.iter().cloned());
    let settings = note_tasks::read_settings(bob_dir);
    let mut tasks = Vec::new();
    for relative in &relative_paths {
        let absolute = bob_dir.join(relative);
        let contents = match fs::read_to_string(&absolute) {
            Ok(contents) => contents,
            Err(error) => {
                warnings.push(bounded_warning(format!(
                    "failed to read {}: {error}",
                    display_path(relative)
                )));
                continue;
            }
        };
        let scan = note_tasks::scan(&contents, &settings);
        let used_ids = super::collect_done::block_ids_in_markdown(&contents);
        let used: HashSet<&str> = used_ids.iter().map(String::as_str).collect();
        let lines: Vec<&str> = contents.lines().collect();
        for task in scan.tasks() {
            let raw_line =
                lines.get(task.line_index).copied().unwrap_or_default();
            let duplicate_id = task.block_id.as_deref().is_some_and(|id| {
                matches!(
                    scan.by_block_id(id),
                    note_tasks::BlockIdLookup::Duplicate(_)
                )
            });
            let open = task.status_type.is_open();
            let note_path = display_path(relative);
            tasks.push(DependencyTask {
                locator: locator_for(&index, relative),
                note_path,
                block_id: task.block_id.clone(),
                block_id_suggestions: if task.block_id.is_some() {
                    Vec::new()
                } else {
                    capture_block_ids::suggest_ids_with_used(
                        &task.description,
                        ':',
                        &used,
                    )
                },
                status_symbol: task.status_symbol,
                status_name: task.status_name.clone(),
                status_type: task.status_type,
                text: task.description.clone(),
                section: task.section.clone(),
                depth: capture_tasks::indentation_depth(&task.indentation),
                line: task.line_index + 1,
                task_ref: task.task_ref(),
                hidden: has_hide_tag(raw_line),
                open,
                duplicate_id,
            });
        }
    }
    let mut basenames: HashMap<String, Vec<String>> = HashMap::new();
    for relative in &relative_paths {
        if let Some(stem) = relative.file_stem().and_then(OsStr::to_str) {
            basenames
                .entry(stem.to_lowercase())
                .or_default()
                .push(display_path(relative));
        }
    }
    for paths in basenames.values_mut() {
        paths.sort();
    }
    DependencyTaskResult {
        tasks,
        warnings,
        index,
        basenames,
    }
}

fn collect_note_paths(
    root: &Path,
    directory: &Path,
    paths: &mut Vec<PathBuf>,
    warnings: &mut Vec<String>,
) {
    let entries = match fs::read_dir(directory) {
        Ok(entries) => {
            let mut entries =
                entries.filter_map(Result::ok).collect::<Vec<_>>();
            entries.sort_by_key(|entry| entry.path());
            entries
        }
        Err(error) => {
            warnings.push(bounded_warning(format!(
                "failed to read directory {}: {error}",
                display_path(directory.strip_prefix(root).unwrap_or(directory))
            )));
            return;
        }
    };
    for entry in entries {
        let path = entry.path();
        let file_type = match entry.file_type() {
            Ok(file_type) => file_type,
            Err(error) => {
                warnings.push(bounded_warning(format!(
                    "skipping {}: {error}",
                    display_path(path.strip_prefix(root).unwrap_or(&path))
                )));
                continue;
            }
        };
        // `file_type` never follows symlinks, so symlinked directories
        // are skipped here rather than descended (or escaped through).
        if file_type.is_dir() {
            if entry
                .file_name()
                .to_str()
                .is_some_and(|name| name.starts_with('.'))
                || is_always_excluded_note_directory_name(&entry.file_name())
            {
                continue;
            }
            collect_note_paths(root, &path, paths, warnings);
        } else if file_type.is_file()
            && path
                .extension()
                .and_then(OsStr::to_str)
                .is_some_and(|extension| extension.eq_ignore_ascii_case("md"))
            && let Ok(relative) = path.strip_prefix(root)
        {
            paths.push(relative.to_path_buf());
        }
    }
}

fn display_path(relative: &Path) -> String {
    relative.to_string_lossy().replace('\\', "/")
}

fn bounded_warning(message: String) -> String {
    super::capture_active_tasks::bounded_warning(message)
}

/// Whether a raw task line carries the `#hide` tag as its own token:
/// near-matches such as `#hidden` or `#hideaway` do not count.
fn has_hide_tag(raw_line: &str) -> bool {
    raw_line.split_whitespace().any(|token| token == "#hide")
}

/// Short display locator for a note path: the file stem when that
/// basename is unique in the vault (case-insensitive, like link
/// resolution), otherwise the full vault-relative path without `.md`.
pub(crate) fn locator_for(index: &NoteIndex, path: &Path) -> String {
    let without_extension = path_without_extension(path);
    if let Some(stem) = path.file_stem().and_then(OsStr::to_str)
        && index.resolve(None, stem).as_deref() == Some(path)
    {
        return stem.to_string();
    }
    without_extension
}

fn path_without_extension(path: &Path) -> String {
    let mut without_extension = path.to_path_buf();
    without_extension.set_extension("");
    without_extension
        .components()
        .filter_map(|component| {
            component.as_os_str().to_str().map(str::to_string)
        })
        .collect::<Vec<_>>()
        .join("/")
}

/// Whether a locator must be quoted in a `&"note":block-id`
/// replacement: any whitespace, `"`, `:`, `&`-leading sigil confusion,
/// or Markdown/code-sensitive byte that the unquoted run would still
/// scan but a reader could mis-split. Quoted replacements always
/// round-trip; unquoted ones stay readable.
pub(crate) fn needs_quoting(note: &str) -> bool {
    note.is_empty()
        || note.bytes().any(|byte| {
            matches!(
                byte,
                b' ' | b'\t'
                    | b'\n'
                    | b'\r'
                    | b'"'
                    | b':'
                    | b'['
                    | b']'
                    | b'|'
                    | b'`'
                    | b'\\'
            )
        })
}

/// Escape a locator for a quoted replacement: only `\"` and `\\` are
/// escapes inside the quoted component.
pub(crate) fn escape_quoted_note(note: &str) -> String {
    let mut escaped = String::with_capacity(note.len());
    for character in note.chars() {
        if matches!(character, '"' | '\\') {
            escaped.push('\\');
        }
        escaped.push(character);
    }
    escaped
}

/// Bob-authored replacement an accept inserts for an identified task
/// (`&note:block-id`, or `&"Note":block-id` when the locator needs
/// quoting). Callers must never insert an empty string: ID-less and
/// guarded rows resolve through the explicit Add block ID flow instead.
pub(crate) fn replacement_for(
    index: &NoteIndex,
    path: &Path,
    block_id: &str,
) -> String {
    let locator = locator_for(index, path);
    if needs_quoting(&locator) {
        format!("&\"{}\":{block_id}", escape_quoted_note(&locator))
    } else {
        format!("&{locator}:{block_id}")
    }
}

/// Why a typed note identity cannot resolve to a vault note.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum LocatorError {
    /// A usage-shaped diagnostic the picker surfaces without a scan.
    Usage(String),
    /// The note resolved but escapes the vault (or is unreadable).
    Io(String),
}

/// Resolve a typed `&note` identity (quotes already removed, escapes
/// decoded, case and Unicode preserved) to its exact vault-relative
/// path with extension.
///
/// An explicit relative path (`projects/foo`) resolves first and only
/// as that exact file; a bare basename resolves only when it names one
/// vault note (case-insensitive, like wikilink resolution). Traversal
/// (`.`, `..`), absolute paths, empty components, backslashes, and
/// symlink escapes are rejected rather than normalized, and the
/// lowercasing route parser is never consulted.
pub(crate) fn resolve_dependency_note(
    bob_dir: &Path,
    discovered: &DependencyTaskResult,
    typed: &str,
) -> Result<PathBuf, LocatorError> {
    if typed.is_empty() {
        return Err(LocatorError::Usage(
            "empty note identity; use &note:block-id".to_string(),
        ));
    }
    if typed.contains('\\') {
        return Err(LocatorError::Usage(format!(
            "invalid note identity {typed:?}: backslashes are not path separators"
        )));
    }
    let path = Path::new(typed);
    if path.is_absolute() {
        return Err(LocatorError::Usage(format!(
            "invalid note identity {typed:?}: absolute paths escape the vault"
        )));
    }
    for component in path.components() {
        match component {
            Component::Normal(_) => {}
            Component::CurDir | Component::ParentDir => {
                return Err(LocatorError::Usage(format!(
                    "invalid note identity {typed:?}: `.` and `..` escape the vault"
                )));
            }
            _ => {
                return Err(LocatorError::Usage(format!(
                    "invalid note identity {typed:?}: absolute paths escape the vault"
                )));
            }
        }
    }
    let has_separator = typed.contains('/');
    if typed.split('/').any(str::is_empty) {
        return Err(LocatorError::Usage(format!(
            "invalid note identity {typed:?}: empty path components are not notes"
        )));
    }
    // Extensionless, like routes: `foo` and `foo.md` name the same
    // note, but an explicit `.md` is honored, not doubled.
    let with_extension = if typed.rsplit('/').next().is_some_and(|file| {
        file.rsplit('.')
            .next()
            .is_some_and(|extension| extension.eq_ignore_ascii_case("md"))
    }) {
        typed.to_string()
    } else {
        format!("{typed}.md")
    };
    if has_separator {
        let candidate = PathBuf::from(&with_extension);
        if !bob_dir.join(&candidate).is_file() {
            return Err(LocatorError::Usage(format!("no such note: {typed}")));
        }
        reject_symlink_escape(bob_dir, &candidate)?;
        return Ok(candidate);
    }
    match discovered.index.resolve(None, typed) {
        Some(resolved) => {
            reject_symlink_escape(bob_dir, &resolved)?;
            Ok(resolved)
        }
        None => Err(LocatorError::Usage(no_such_note(discovered, typed))),
    }
}

/// Refuse a resolved note that escapes the vault through a symlink:
/// the canonical locations of both the vault root and the joined
/// target must nest.
fn reject_symlink_escape(
    bob_dir: &Path,
    relative: &Path,
) -> Result<(), LocatorError> {
    let root = bob_dir.canonicalize().map_err(|error| {
        LocatorError::Io(format!("read vault {}: {error}", bob_dir.display()))
    })?;
    let target = bob_dir.join(relative).canonicalize().map_err(|error| {
        LocatorError::Io(format!(
            "read note {}: {error}",
            display_path(relative)
        ))
    })?;
    if target.starts_with(&root) {
        return Ok(());
    }
    Err(LocatorError::Usage(format!(
        "invalid note identity {}: escapes the vault through a symlink",
        display_path(relative)
    )))
}

/// Targeted missing/ambiguous diagnostics: a duplicate basename must
/// be disambiguated with its full relative path.
fn no_such_note(discovered: &DependencyTaskResult, typed: &str) -> String {
    let stem = typed
        .strip_suffix(".md")
        .or_else(|| typed.strip_suffix(".MD"))
        .unwrap_or(typed);
    if let Some(paths) = discovered.basenames.get(&stem.to_lowercase())
        && paths.len() > 1
    {
        return format!(
            "ambiguous note {typed:?}: matches {}; use the full relative path",
            paths.join(", ")
        );
    }
    format!("no such note: {typed}")
}

/// Raw searchable fields for a dependency task: cleaned description,
/// `locator:block-id` and block ID (identified tasks only), locator,
/// full note path, and section heading. The shared tiered matcher
/// scores these exactly like the `:` picker's fields.
pub(crate) fn dependency_search_fields(task: &DependencyTask) -> Vec<String> {
    let mut fields = Vec::new();
    if let Some(block_id) = &task.block_id {
        fields.push(format!("{}:{block_id}", task.locator));
        fields.push(block_id.clone());
    }
    fields.push(task.text.clone());
    fields.push(task.locator.clone());
    fields.push(task.note_path.clone());
    if let Some(section) = &task.section {
        fields.push(section.clone());
    }
    fields
}

/// Stable picker group for a task: `in_this_note`, `in_progress`,
/// `next`, `open`, or `completed`. Hidden tasks share their section's
/// group; the client subdues them from the row flag.
pub(crate) fn group_for(
    task: &DependencyTask,
    owner_note: Option<&str>,
) -> &'static str {
    if !task.open {
        "completed"
    } else if owner_note.is_some_and(|owner| owner == task.note_path) {
        "in_this_note"
    } else if task.status_symbol == '/' {
        "in_progress"
    } else if task.status_symbol == '*' {
        "next"
    } else {
        "open"
    }
}

/// Canonical empty-query order key for a task: same-note open tasks
/// in document order, then In Progress (`/`), Next (`*`), other open
/// tasks grouped by note, then completed/cancelled history; hidden
/// tasks order last within their section. Stable path/line order
/// breaks every remaining tie, and ranked queries keep this order for
/// equal scores.
fn canonical_key(
    task: &DependencyTask,
    owner_note: Option<&str>,
) -> (u8, u8, String, usize) {
    let section: u8 = if !task.open {
        4
    } else if owner_note.is_some_and(|owner| owner == task.note_path) {
        0
    } else if task.status_symbol == '/' {
        1
    } else if task.status_symbol == '*' {
        2
    } else {
        3
    };
    let hidden = u8::from(task.hidden);
    if section == 0 {
        (section, hidden, String::new(), task.line)
    } else {
        (section, hidden, task.note_path.clone(), task.line)
    }
}

/// Order catalog tasks for the picker: query matches first (open
/// matches by score, then completed-history matches by score, ties
/// keep canonical order), or the plain canonical order for an empty
/// query. Completed history always trails open matches so searches
/// still find it without burying actionable rows.
pub(crate) fn order_for_picker<'a>(
    tasks: &'a [DependencyTask],
    query: &str,
    owner_note: Option<&str>,
) -> Vec<&'a DependencyTask> {
    let mut scored: Vec<(&'a DependencyTask, u32)> = tasks
        .iter()
        .filter_map(|task| {
            super::capture_link_tasks::match_score(
                &dependency_search_fields(task),
                query,
            )
            .map(|score| (task, score))
        })
        .collect();
    // Score dominates; the canonical key breaks ties; the open flag
    // keeps completed history behind open matches at any score. The
    // sort is stable, so full ties keep catalog (path/line) order.
    scored.sort_by(|left, right| {
        let (left_task, left_score) = left;
        let (right_task, right_score) = right;
        u8::from(!left_task.open)
            .cmp(&u8::from(!right_task.open))
            .then_with(|| right_score.cmp(left_score))
            .then_with(|| {
                canonical_key(left_task, owner_note)
                    .cmp(&canonical_key(right_task, owner_note))
            })
    });
    scored.into_iter().map(|(task, _)| task).collect()
}

/// Prerequisites already on the dependent that an accept must not
/// duplicate: the managed Depends-On line of the existing dependent
/// task (resolved against the vault index, same-note links included)
/// plus every `&note:block-id` already typed in the draft item. Each
/// entry is an exact `(note_path, block_id)` pair.
pub(crate) fn already_present_prerequisites(
    bob_dir: &Path,
    discovered: &DependencyTaskResult,
    dependent_note: Option<&Path>,
    dependent_block_id: Option<&str>,
    typed: &[(String, String)],
) -> HashSet<(String, String)> {
    let mut present = HashSet::new();
    for (note, block_id) in typed {
        if let Ok(resolved) = resolve_dependency_note(bob_dir, discovered, note)
        {
            present.insert((display_path(&resolved), block_id.clone()));
        }
    }
    if let (Some(note), Some(block_id)) = (dependent_note, dependent_block_id) {
        present.extend(depends_on_prerequisites(
            bob_dir, discovered, note, block_id,
        ));
    }
    present
}

/// Exact `(note_path, block_id)` pairs on one existing task's managed
/// Depends-On child line. Unresolvable link targets are skipped: only
/// provable duplicates are reported.
fn depends_on_prerequisites(
    bob_dir: &Path,
    discovered: &DependencyTaskResult,
    dependent_note: &Path,
    dependent_block_id: &str,
) -> HashSet<(String, String)> {
    let mut present = HashSet::new();
    let Ok(contents) = fs::read_to_string(bob_dir.join(dependent_note)) else {
        return present;
    };
    let settings = note_tasks::read_settings(bob_dir);
    let scan = note_tasks::scan(&contents, &settings);
    let task = scan
        .tasks()
        .iter()
        .find(|task| task.block_id.as_deref() == Some(dependent_block_id));
    let Some(task) = task else {
        return present;
    };
    let lines: Vec<&str> = contents.lines().collect();
    let fenced = super::markdown::fenced_lines(&lines, 0..lines.len());
    let Some(child) = task_dependencies::parse::dependency_child_of(
        &lines,
        &fenced,
        task.line_index,
    ) else {
        return present;
    };
    let task_dependencies::parse::DependencyLine::Accepted { links, .. } =
        &child.parsed
    else {
        return present;
    };
    for link in links {
        let resolved = if link.target.is_empty() {
            Some(dependent_note.to_path_buf())
        } else {
            discovered.index.resolve(Some(dependent_note), &link.target)
        };
        if let Some(resolved) = resolved {
            present.insert((display_path(&resolved), link.block_id.clone()));
        }
    }
    present
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn write_file(path: &Path, contents: &str) {
        fs::create_dir_all(path.parent().expect("parent")).expect("parent");
        fs::write(path, contents).expect("write fixture note");
    }

    fn vault_fixture() -> tempfile::TempDir {
        let temp = tempfile::tempdir().expect("temporary vault");
        write_file(
            &temp
                .path()
                .join(".obsidian/plugins/obsidian-tasks-plugin/data.json"),
            r##"{
              "globalFilter": "#task",
              "statusSettings": {
                "coreStatuses": [
                  {"symbol":" ","name":"Todo","type":"TODO"},
                  {"symbol":"x","name":"Done","type":"DONE"},
                  {"symbol":"/","name":"In Progress","type":"IN_PROGRESS"},
                  {"symbol":"*","name":"Next","type":"ON_HOLD"},
                  {"symbol":"?","name":"Blocked","type":"ON_HOLD"}
                ]
              }
            }"##,
        );
        temp
    }

    #[test]
    fn quoted_locator_round_trips_through_replacement() {
        let index = NoteIndex::from_paths(vec![
            PathBuf::from("Shopping List.md"),
            PathBuf::from("foo.md"),
        ]);
        assert!(needs_quoting("Shopping List"));
        assert!(!needs_quoting("projects/foo"));
        assert_eq!(
            replacement_for(&index, Path::new("Shopping List.md"), "bar"),
            "&\"Shopping List\":bar"
        );
        assert_eq!(
            replacement_for(&index, Path::new("foo.md"), "bar"),
            "&foo:bar"
        );
    }

    #[test]
    fn unique_basename_resolves_before_ambiguous_paths() {
        let temp = vault_fixture();
        write_file(&temp.path().join("Solo.md"), "- [ ] #task Solo ^solo\n");
        write_file(&temp.path().join("a/Dup.md"), "- [ ] #task Dup a ^dup-a\n");
        write_file(&temp.path().join("b/Dup.md"), "- [ ] #task Dup b ^dup-b\n");
        let discovered = discover(temp.path());
        assert_eq!(
            resolve_dependency_note(temp.path(), &discovered, "Solo"),
            Ok(PathBuf::from("Solo.md"))
        );
        assert!(resolve_dependency_note(temp.path(), &discovered, "Missing")
            .is_err());
        assert!(
            resolve_dependency_note(temp.path(), &discovered, "../escape")
                .is_err()
        );
        assert!(
            resolve_dependency_note(temp.path(), &discovered, "/abs").is_err()
        );
        let ambiguous =
            resolve_dependency_note(temp.path(), &discovered, "Dup");
        assert!(ambiguous.is_err());
        if let Err(LocatorError::Usage(message)) = ambiguous {
            assert!(message.contains("ambiguous"), "{message}");
        } else {
            panic!("expected an ambiguous usage diagnostic");
        }
        assert_eq!(
            resolve_dependency_note(temp.path(), &discovered, "a/Dup"),
            Ok(PathBuf::from("a/Dup.md"))
        );
    }

    #[test]
    fn hidden_tasks_detect_only_the_exact_tag() {
        assert!(has_hide_tag("- [ ] #task Quiet #hide ^quiet"));
        assert!(!has_hide_tag("- [ ] #task Loud #hidden ^loud"));
        assert!(!has_hide_tag("- [ ] #task Den #hideaway ^den"));
    }

    fn catalog_fixture() -> tempfile::TempDir {
        let temp = vault_fixture();
        write_file(
            &temp.path().join("home.md"),
            concat!(
                "- [ ] #task Buy milk ^milk\n",
                "- [/] #task Cook dinner ^dinner\n",
                "- [ ] #task No id yet\n",
                "- [ ] #task Hidden chore #hide ^hidden-chore\n",
                "- [x] #task Done deed ^done-deed\n",
                "```\n",
                "- [ ] #task Fenced ^fenced\n",
                "```\n",
                "Plain ^plain-anchor\n",
            ),
        );
        write_file(
            &temp.path().join("projects/alpha.md"),
            "- [*] #task Alpha next ^alpha\n",
        );
        write_file(
            &temp.path().join("ref/beta.md"),
            "- [ ] #task Beta ref ^beta\n",
        );
        write_file(
            &temp.path().join("2026/20260930.md"),
            "- [/] #task Today task ^today-task\n",
        );
        write_file(
            &temp.path().join("done/old.md"),
            "- [x] #task Old kept ^kept\n",
        );
        write_file(
            &temp.path().join("twin.md"),
            concat!(
                "- [ ] #task First twin ^twin\n",
                "- [ ] #task Second twin ^twin\n",
            ),
        );
        write_file(
            &temp.path().join("_templates/t.md"),
            "- [ ] #task Tmpl ^tmpl\n",
        );
        write_file(&temp.path().join(".hidden/h.md"), "- [ ] #task Hid ^hid\n");
        temp
    }

    fn texts<'a>(tasks: &[&'a DependencyTask]) -> Vec<&'a str> {
        tasks.iter().map(|task| task.text.as_str()).collect()
    }

    #[test]
    fn catalog_covers_every_task_bearing_note_kind() {
        let temp = catalog_fixture();
        let discovered = discover(temp.path());
        let ordered = order_for_picker(&discovered.tasks, "", Some("home.md"));
        let all = texts(&ordered);
        // Untyped root, nested, ref, daily, archive, hidden, closed,
        // and ID-less tasks are all present.
        for present in [
            "Buy milk",
            "Cook dinner",
            "No id yet",
            "Hidden chore #hide",
            "Done deed",
            "Alpha next",
            "Beta ref",
            "Today task",
            "Old kept",
            "First twin",
            "Second twin",
        ] {
            assert!(all.contains(&present), "{all:?}");
        }
        // Fenced code, non-task anchors, and excluded directories are
        // never candidates.
        for absent in ["Fenced", "Tmpl", "Hid"] {
            assert!(!all.contains(&absent), "{all:?}");
        }
        assert!(discovered.warnings.is_empty());
    }

    #[test]
    fn empty_query_orders_same_note_lanes_history() {
        let temp = catalog_fixture();
        let discovered = discover(temp.path());
        let ordered = order_for_picker(&discovered.tasks, "", Some("home.md"));
        let all = texts(&ordered);
        // Same-note open tasks in document order come first, even
        // ahead of the In Progress lane elsewhere.
        assert_eq!(
            &all[..4],
            &["Buy milk", "Cook dinner", "No id yet", "Hidden chore #hide"],
            "{all:?}"
        );
        // In Progress, then Next, then other open notes.
        assert_eq!(all[4], "Today task");
        assert_eq!(all[5], "Alpha next");
        // Completed history trails every open task.
        let first_closed = all
            .iter()
            .position(|text| ["Done deed", "Old kept"].contains(text))
            .expect("history listed");
        assert!(first_closed > 6, "{all:?}");
        // Duplicate IDs flag the guard without dropping the rows.
        let twins: Vec<&&DependencyTask> = ordered
            .iter()
            .filter(|task| task.block_id.as_deref() == Some("twin"))
            .collect();
        assert_eq!(twins.len(), 2);
        assert!(twins.iter().all(|task| task.duplicate_id));
    }

    #[test]
    fn ranking_shares_the_tiered_matcher_vectors() {
        let temp = catalog_fixture();
        let discovered = discover(temp.path());
        // Field prefix outranks substring: `mil` matches "Buy milk"
        // at a word prefix, never the unrelated notes.
        let ordered = order_for_picker(&discovered.tasks, "mil", None);
        assert_eq!(texts(&ordered), vec!["Buy milk"]);
        // `locator:block-id` qualifies like the `:` picker's
        // `route:block-id` field.
        let ordered = order_for_picker(&discovered.tasks, "home:mil", None);
        assert_eq!(texts(&ordered), vec!["Buy milk"]);
        // History stays findable through search.
        let ordered = order_for_picker(&discovered.tasks, "kept", None);
        assert_eq!(texts(&ordered), vec!["Old kept"]);
    }

    #[test]
    fn groups_follow_the_picker_sections() {
        let temp = catalog_fixture();
        let discovered = discover(temp.path());
        let group = |text: &str| {
            discovered
                .tasks
                .iter()
                .find(|task| task.text == text)
                .map(|task| group_for(task, Some("home.md")))
                .unwrap_or("missing")
        };
        assert_eq!(group("Buy milk"), "in_this_note");
        assert_eq!(group("Cook dinner"), "in_this_note");
        assert_eq!(group("Today task"), "in_progress");
        assert_eq!(group("Alpha next"), "next");
        assert_eq!(group("Beta ref"), "open");
        assert_eq!(group("Done deed"), "completed");
        assert_eq!(group("Old kept"), "completed");
    }
}
