#![allow(clippy::type_complexity)]
//! Reverse a relation label: every accepted `S —OLD→ T` becomes `T —NEW→ S`.
//!
//! One previewed transaction and one revertable commit, with accurate
//! mirrors, strict refusals, and verified undo (`docs/task-relations.md`
//! §16 RV10).

use std::{
    collections::{BTreeMap, BTreeSet, HashMap, HashSet},
    ffi::OsString,
    fs,
    path::{Path, PathBuf},
    rc::Rc,
};

use clap::ArgMatches;
use serde::Serialize;
use serde_json::json;

use crate::native::{
    capture_block_ids::mint_block_id,
    capture_dependency_tasks,
    collect_done::block_ids_in_markdown,
    config as bob_config, env as bob_env,
    markdown::fenced_lines,
    ob,
    relation_labels::{self, journal, Catalog},
    task_dependencies::{
        blocks::{has_task_tag, BlocksEdit},
        format::{canonical_link, has_final_newline, note_line_ending},
        is_archive_path,
    },
    task_relations::{
        label::LabelError,
        parse::{
            linked_from_children_of, linked_from_label_spans,
            links_children_of, links_label_spans,
        },
        plan::{plan_linked_from_line, plan_links_edit, LinksEdit},
        LinkedFromLine, LinksLine,
    },
    task_status_hooks,
    vault_links::{target_to_markdown_path, NoteIndex},
};

const COMMAND_NAME: &str = "bob relations rename";
const SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OutputFormat {
    Human,
    Json,
}

impl OutputFormat {
    fn from_matches(matches: &ArgMatches) -> Self {
        match matches
            .get_one::<String>("format")
            .map(String::as_str)
            .unwrap_or("human")
        {
            "json" => Self::Json,
            _ => Self::Human,
        }
    }
}

#[derive(Debug, Clone)]
struct ReverseFailure {
    code: &'static str,
    message: String,
    hint: Option<String>,
}

impl ReverseFailure {
    fn new(
        code: &'static str,
        message: impl Into<String>,
        hint: Option<impl Into<String>>,
    ) -> Self {
        Self {
            code,
            message: message.into(),
            hint: hint.map(Into::into),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
struct ReverseSide {
    key: String,
    name: String,
}

#[derive(Debug, Clone, Serialize)]
struct LineChange {
    line: usize,
    kind: &'static str,
    before: String,
    after: String,
}

#[derive(Debug, Clone, Serialize)]
struct FilePlanView {
    path: String,
    links: usize,
    mirrors: usize,
    lines: Vec<LineChange>,
}

#[derive(Debug, Clone, Serialize)]
struct ReverseCounts {
    links: usize,
    notes: usize,
    mirrors: usize,
    done_links: usize,
    done_notes: usize,
}

#[derive(Debug, Clone, Serialize)]
struct StateChange {
    hidden_before: bool,
    hidden_after: bool,
    moved: bool,
    note: String,
}

#[derive(Debug, Clone, Serialize)]
struct InverseChange {
    before: Option<String>,
    after: Option<String>,
    note: String,
}

#[derive(Debug, Clone, Serialize)]
struct SyncInfo {
    pre: String,
    post: String,
}

#[derive(Debug, Clone, Serialize)]
struct MoveEndpoint {
    path: String,
    line: usize,
    block_id: Option<String>,
    text: String,
}

#[derive(Debug, Clone, Serialize)]
struct ReverseMove {
    source: MoveEndpoint,
    target: MoveEndpoint,
    from_label: String,
    to_label: String,
    action: &'static str,
}

#[derive(Debug, Clone, Serialize)]
struct MintedBlock {
    path: String,
    line: usize,
    block_id: String,
}

#[derive(Debug, Clone)]
struct WorkingFile {
    relative: PathBuf,
    path_str: String,
    contents: String,
    ending: &'static str,
    final_newline: bool,
    is_archive: bool,
    is_previous_daily: bool,
    done: bool,
    changes: Vec<LineChange>,
}

#[derive(Debug, Clone, Serialize)]
struct ReverseReport {
    schema_version: u32,
    ok: bool,
    mode: String,
    changed: bool,
    reversed: bool,
    from: ReverseSide,
    to: ReverseSide,
    merge: bool,
    moves: Vec<ReverseMove>,
    minted_block_ids: Vec<MintedBlock>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    stay_open: Vec<MoveEndpoint>,
    files: Vec<FilePlanView>,
    counts: ReverseCounts,
    state: StateChange,
    inverse: InverseChange,
    replacement: String,
    hide_replacement: String,
    commit: Option<serde_json::Value>,
    sync: SyncInfo,
    warnings: Vec<String>,
}

#[derive(Debug, Clone)]
struct FilePlan {
    path: String,
    links: usize,
    mirrors: usize,
    done: bool,
    lines: Vec<LineChange>,
    before_contents: String,
    after_contents: String,
}

#[derive(Debug, Clone)]
struct PreparedReverse {
    old_key: String,
    new_key: String,
    new_name: String,
    via: String,
    files: Vec<FilePlan>,
    moves: Vec<ReverseMove>,
    minted: Vec<MintedBlock>,
    total_entries: usize,
    merged_count: usize,
    report: ReverseReport,
}

pub(crate) fn run_reverse_with_matches(matches: &ArgMatches) -> i32 {
    let format = OutputFormat::from_matches(matches);
    if matches.get_flag("dry-run") {
        return match prepare_from_matches(matches) {
            Ok(prepared) => {
                print_success(&prepared.report, format);
                0
            }
            Err(failure) => print_failure(format, failure),
        };
    }
    run_live(matches, format)
}

fn prepare_from_matches(
    matches: &ArgMatches,
) -> Result<PreparedReverse, ReverseFailure> {
    let vault = matches
        .get_one::<OsString>("bob-dir")
        .map(PathBuf::from)
        .map(|path| bob_env::expand_tilde(&path))
        .unwrap_or_else(bob_env::bob_dir);
    let old_raw = matches
        .get_one::<String>("old")
        .map(String::as_str)
        .unwrap_or("");
    let new_raw = matches
        .get_one::<String>("new")
        .map(String::as_str)
        .unwrap_or("");
    let old_name = normalize_label(old_raw, "OLD")?;
    let new_name = normalize_label(new_raw, "NEW")?;
    let via = matches
        .get_one::<String>("via")
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
        .unwrap_or("bob-cli")
        .to_string();
    plan_reverse(
        &vault,
        &old_name,
        &new_name,
        matches.get_flag("merge"),
        &via,
        matches.get_flag("dry-run"),
    )
}

fn load_close_policy() -> bob_config::RelationsConfig {
    match bob_config::load_relations_config(&bob_config::config_path()) {
        Ok(loaded) => loaded,
        Err(error) => bob_config::RelationsConfig {
            linked_from_lines: true,
            linked_from_mentions: false,
            cancel_labels: Vec::new(),
            complete_labels: Vec::new(),
            warnings: vec![format!(
                "invalid relations config: {error}; closing nothing"
            )],
        },
    }
}

fn normalize_label(raw: &str, which: &str) -> Result<String, ReverseFailure> {
    crate::native::task_relations::label::normalize_relation_label(raw).map_err(
        |error| {
            let detail = match error {
                LabelError::ReservedMention => {
                    "💬 is reserved for text mentions"
                }
                LabelError::Empty => "must not be empty",
                LabelError::TooLong => "must be at most 40 characters",
                LabelError::ForbiddenCharacter => {
                    "contains a forbidden character"
                }
                LabelError::InvalidStart => {
                    "cannot begin with a middle dot or comma"
                }
            };
            ReverseFailure::new(
                "invalid_label",
                format!("invalid relation label {which}: {detail}"),
                Some("choose a label accepted by bob relations hide"),
            )
        },
    )
}

fn display_relative(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

fn is_done_path(path: &Path) -> bool {
    path.components()
        .any(|component| component.as_os_str() == "done")
}

fn should_preserve_mention(
    source_path: Option<PathBuf>,
    source_id: &str,
    relation_sources: &BTreeSet<(PathBuf, String)>,
) -> bool {
    source_path.is_none_or(|path| {
        !relation_sources.contains(&(path, source_id.to_string()))
    })
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
struct VaultFile {
    relative: PathBuf,
    path_str: String,
    contents: String,
    lines: Vec<String>,
    line_bodies: Vec<String>,
    fenced: BTreeSet<usize>,
    scan: crate::native::note_tasks::NoteTaskScan,
    used_ids: HashSet<String>,
    block_to_tasks: HashMap<String, Vec<usize>>,
    ending: &'static str,
    final_newline: bool,
    is_previous_daily: bool,
    is_archive: bool,
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
struct RawEdge {
    source_file: usize,
    source_task: usize,
    source_line: usize,
    source_block: Option<String>,
    source_text: String,
    target_raw: String,
    target_block: String,
    target_entry_identity: String,
    from_label_display: String,
    links_line: usize,
}

fn plan_reverse(
    vault: &Path,
    old_name: &str,
    new_name: &str,
    merge: bool,
    via: &str,
    dry_run: bool,
) -> Result<PreparedReverse, ReverseFailure> {
    let old_key = relation_labels::relation_label_key(old_name);
    let new_key = relation_labels::relation_label_key(new_name);
    relation_labels::invalidate_scan(vault);
    let catalog = Catalog::load(vault).map_err(|error| {
        ReverseFailure::new(
            "io",
            format!("read relation catalog in {}: {error}", vault.display()),
            None::<String>,
        )
    })?;
    let (mut paths, walk_warnings) =
        capture_dependency_tasks::vault_note_paths(vault);
    if !walk_warnings.is_empty() {
        return Err(ReverseFailure::new(
            "malformed_links_line",
            format!(
                "could not scan every note in the vault: {}",
                walk_warnings.join("; ")
            ),
            Some("resolve the note-walk warning before retrying the rename"),
        ));
    }
    paths.sort();
    let note_index = NoteIndex::from_paths(paths.iter().cloned());
    let close_policy = load_close_policy();
    let mut warnings = catalog.warnings().to_vec();
    warnings.extend(close_policy.warnings.iter().cloned());

    let Some(old_record) = catalog.get(&old_key) else {
        return Err(ReverseFailure::new(
            "unknown_label",
            format!("unknown relation label: {old_name}"),
            Some("list labels with bob relations -a"),
        ));
    };
    // RV10: existing-NEW-key merge requirement applies, including the
    // same-key case. Ordinary rename refuses OLD==NEW as unchanged;
    // reverse permits it as a pure reversal when --merge arms it.
    if catalog.get(&new_key).is_some() && !merge {
        return Err(ReverseFailure::new(
            "label_exists",
            format!(
                "{new_name} is already a relation label; pass --merge to reverse {old_name} into it"
            ),
            Some("re-run with --merge to reverse the labels together"),
        ));
    }
    // No OLD edges is also unknown_label on a second run after a
    // non-symmetric apply: the catalog no longer contains OLD.
    // The check above already covers it, but keep the shape explicit.

    let settings = crate::native::note_tasks::read_settings(vault);
    let previous_daily = previous_daily_relative(vault, &paths);
    let mut files: Vec<VaultFile> = Vec::new();
    for relative in &paths {
        let absolute = vault.join(relative);
        let path_str = display_relative(relative);
        let contents = fs::read_to_string(&absolute).map_err(|error| {
            ReverseFailure::new(
                "malformed_links_line",
                format!(
                    "could not read note {path_str}: {error}; rename cannot confirm full coverage"
                ),
                Some("restore access to the note and retry"),
            )
        })?;
        let ending = note_line_ending(&contents);
        let final_newline = has_final_newline(&contents);
        let logical: Vec<String> = contents
            .split_inclusive('\n')
            .map(|segment| {
                let without_lf = segment.strip_suffix('\n').unwrap_or(segment);
                without_lf
                    .strip_suffix('\r')
                    .unwrap_or(without_lf)
                    .to_string()
            })
            .collect();
        // Handle empty file: split_inclusive yields nothing; keep one
        // empty logical line only when contents non-empty. For empty
        // contents keep zero lines; helpers cope.
        let bodies: Vec<String> = logical.clone();
        let body_refs: Vec<&str> = bodies.iter().map(String::as_str).collect();
        let fenced = fenced_lines(&body_refs, 0..body_refs.len());
        let scan = crate::native::note_tasks::scan(&contents, &settings);
        let used_vec = block_ids_in_markdown(&contents);
        let used_ids: HashSet<String> = used_vec.into_iter().collect();
        // Also track every task block id for duplicate detection.
        let mut block_to_tasks: HashMap<String, Vec<usize>> = HashMap::new();
        for (task_index, task) in scan.tasks().iter().enumerate() {
            if let Some(id) = &task.block_id {
                block_to_tasks
                    .entry(id.clone())
                    .or_default()
                    .push(task_index);
            }
        }
        let is_archive = is_archive_path(relative);
        let is_previous_daily =
            previous_daily.as_ref().is_some_and(|prev| prev == relative);
        files.push(VaultFile {
            relative: relative.clone(),
            path_str,
            contents,
            lines: logical,
            line_bodies: bodies,
            fenced,
            scan,
            used_ids: used_ids.clone(),
            block_to_tasks,
            ending,
            final_newline,
            is_previous_daily,
            is_archive,
        });
    }
    // Map relative path -> file index for fast lookup.
    let mut file_by_path: HashMap<PathBuf, usize> = HashMap::new();
    for (index, file) in files.iter().enumerate() {
        file_by_path.insert(file.relative.clone(), index);
    }

    // Collect OLD edges; refuse malformed matching lines.
    let mut raw_edges: Vec<RawEdge> = Vec::new();
    let mut total_entries = 0usize;
    for (file_index, file) in files.iter().enumerate() {
        let refs: Vec<&str> =
            file.line_bodies.iter().map(String::as_str).collect();
        for task in file.scan.tasks() {
            let task_line = task.line_index;
            for child in links_children_of(&refs, &file.fenced, task_line) {
                let raw_line = file
                    .line_bodies
                    .get(child.line_index)
                    .map(String::as_str)
                    .unwrap_or("");
                match &child.parsed {
                    LinksLine::Accepted { entries, .. } => {
                        total_entries =
                            total_entries.saturating_add(entries.len());
                        for entry in entries {
                            if relation_labels::relation_label_key(
                                entry.label.as_deref().unwrap_or(""),
                            ) == old_key
                            {
                                raw_edges.push(RawEdge {
                                    source_file: file_index,
                                    source_task: file
                                        .scan
                                        .tasks()
                                        .iter()
                                        .position(|t| {
                                            t.line_index == task.line_index
                                                && t.digest == task.digest
                                        })
                                        .unwrap_or(0),
                                    source_line: task.line_index,
                                    source_block: task.block_id.clone(),
                                    source_text: task.description.clone(),
                                    target_raw: entry.target.clone(),
                                    target_block: entry.block_id.clone(),
                                    target_entry_identity: entry
                                        .canonical_text(),
                                    from_label_display: entry
                                        .label
                                        .clone()
                                        .unwrap_or_default(),
                                    links_line: child.line_index,
                                });
                            }
                        }
                        let _ = raw_line;
                    }
                    LinksLine::Malformed
                        if line_might_name(raw_line, &old_key) =>
                    {
                        return Err(malformed_line_failure(
                            &file.path_str,
                            child.line_index,
                            "LINKS",
                            old_name,
                        ));
                    }
                    LinksLine::Malformed
                    | LinksLine::Empty
                    | LinksLine::NotALine => {}
                }
            }
            // Matching malformed LINKED FROM lines also refuse: they may
            // name OLD and losing them would under-cover the vault.
            for child in linked_from_children_of(&refs, &file.fenced, task_line)
            {
                let raw_line = file
                    .line_bodies
                    .get(child.line_index)
                    .map(String::as_str)
                    .unwrap_or("");
                match &child.parsed {
                    LinkedFromLine::Accepted { .. } => {}
                    LinkedFromLine::Malformed
                        if line_might_name(raw_line, &old_key) =>
                    {
                        return Err(malformed_line_failure(
                            &file.path_str,
                            child.line_index,
                            "LINKED FROM",
                            old_name,
                        ));
                    }
                    LinkedFromLine::Malformed
                    | LinkedFromLine::Empty
                    | LinkedFromLine::NotALine => {}
                }
            }
        }
    }

    if raw_edges.is_empty() {
        // No OLD edges: if the catalog still lists OLD (e.g. journal-only
        // or zero-use after a partial state), report unchanged; otherwise
        // the earlier unknown_label already fired. Keep the ordinary
        // rename's unchanged shape for an empty move set.
        return Err(ReverseFailure::new(
            "unchanged",
            format!("{old_name} has no links to reverse"),
            Some("choose a label with links to reverse"),
        ));
    }

    // Resolve every edge before any writes.
    #[derive(Debug, Clone)]
    #[allow(dead_code)]
    struct ResolvedEdge {
        raw: RawEdge,
        target_file: usize,
        target_task: usize,
        target_line: usize,
        target_block: String,
        target_text: String,
    }
    let mut resolved: Vec<ResolvedEdge> = Vec::new();
    for raw in &raw_edges {
        let source_file = &files[raw.source_file];
        let resolved_path = resolve_target_path(
            &source_file.relative,
            &raw.target_raw,
            &note_index,
            &files,
            &file_by_path,
        );
        let Some(target_path) = resolved_path else {
            return Err(unresolved_target_failure(
                &source_file.path_str,
                raw.source_line + 1,
                &raw.target_block,
                &raw.target_raw,
            ));
        };
        let Some(&target_file_index) = file_by_path.get(&target_path) else {
            return Err(unresolved_target_failure(
                &source_file.path_str,
                raw.source_line + 1,
                &raw.target_block,
                &raw.target_raw,
            ));
        };
        let target_file = &files[target_file_index];
        let candidates = target_file.block_to_tasks.get(&raw.target_block);
        let task_index = match candidates {
            Some(list) if list.len() == 1 => list[0],
            _ => {
                return Err(unresolved_target_failure(
                    &source_file.path_str,
                    raw.source_line + 1,
                    &raw.target_block,
                    &raw.target_raw,
                ));
            }
        };
        let target_task = &target_file.scan.tasks()[task_index];
        // Self endpoints refuse before any writes.
        if target_file.relative == source_file.relative {
            if let Some(source_id) = &raw.source_block {
                if source_id == &raw.target_block {
                    return Err(ReverseFailure::new(
                        "unresolved_target",
                        format!(
                            "{}:{} links to its own task ^{}; a reverse rename never writes a self-link",
                            source_file.path_str,
                            raw.source_line + 1,
                            raw.target_block
                        ),
                        Some("relabel or unlink the self-link, then retry"),
                    ));
                }
            } else {
                // Source without an id cannot be named by a block link,
                // so same-note target with any id is not self. Fall
                // through; the mint below gives S an identity and the
                // new T —NEW→ S edge is well-formed.
            }
            // Same-task check via line: entry on S's own task line that
            // resolves to S itself (after mint S would be the target).
            // Detect by resolving the target block to the source line.
            if target_task.line_index == raw.source_line {
                return Err(ReverseFailure::new(
                    "unresolved_target",
                    format!(
                        "{}:{} links to its own task ^{}; a reverse rename never writes a self-link",
                        source_file.path_str,
                        raw.source_line + 1,
                        raw.target_block
                    ),
                    Some("relabel or unlink the self-link, then retry"),
                ));
            }
        }
        // Duplicate source ids cannot be canonically linked back.
        if let Some(source_id) = &raw.source_block {
            #[allow(clippy::collapsible_if)]
            if let Some(list) = source_file.block_to_tasks.get(source_id) {
                if list.len() != 1 {
                    return Err(unresolved_target_failure(
                        &source_file.path_str,
                        raw.source_line + 1,
                        source_id,
                        &raw.target_raw,
                    ));
                }
            }
            // Source id must belong to a task (it does: it is the owner),
            // but also ensure no non-task duplicate shadows it. The
            // used set includes non-task anchors; a duplicate anchor with
            // the same id elsewhere in the note makes the back-link
            // ambiguous for readers. Refuse rather than under-cover.
            let occurrences =
                block_id_occurrences_in_file(&source_file.contents, source_id);
            if occurrences != 1 {
                return Err(unresolved_target_failure(
                    &source_file.path_str,
                    raw.source_line + 1,
                    source_id,
                    &raw.target_raw,
                ));
            }
        }
        // Malformed affected destination lines refuse: if T's LINKS lines
        // are malformed we cannot safely append there.
        {
            let refs: Vec<&str> =
                target_file.line_bodies.iter().map(String::as_str).collect();
            for child in links_children_of(
                &refs,
                &target_file.fenced,
                target_task.line_index,
            ) {
                if matches!(child.parsed, LinksLine::Malformed) {
                    return Err(ReverseFailure::new(
                        "malformed_links_line",
                        format!(
                            "malformed LINKS line {}:{} would receive a reversed link; repair it and retry",
                            target_file.path_str,
                            child.line_index + 1
                        ),
                        Some("repair the malformed relation line and retry the rename"),
                    ));
                }
            }
        }
        resolved.push(ResolvedEdge {
            target_file: target_file_index,
            target_task: task_index,
            target_line: target_task.line_index,
            target_block: raw.target_block.clone(),
            target_text: target_task.description.clone(),
            raw: raw.clone(),
        });
    }

    // Mint writable sources without an id; refuse archived/previous-daily.
    // Deterministic path/line/entry order; reuse one mint per source task.
    let mut minted_map: BTreeMap<(usize, usize), String> = BTreeMap::new();
    let mut minted_list: Vec<MintedBlock> = Vec::new();
    // Per-file used sets grow as we mint.
    let mut used_per_file: Vec<HashSet<String>> =
        files.iter().map(|file| file.used_ids.clone()).collect();
    // Sort resolved for deterministic minting.
    resolved.sort_by(|left, right| {
        (
            files[left.raw.source_file].path_str.clone(),
            left.raw.source_line,
            left.raw.target_block.clone(),
        )
            .cmp(&(
                files[right.raw.source_file].path_str.clone(),
                right.raw.source_line,
                right.raw.target_block.clone(),
            ))
    });
    for edge in &resolved {
        if edge.raw.source_block.is_some() {
            continue;
        }
        let key = (edge.raw.source_file, edge.raw.source_task);
        if minted_map.contains_key(&key) {
            continue;
        }
        let file = &files[edge.raw.source_file];
        if file.is_archive || file.is_previous_daily {
            return Err(ReverseFailure::new(
                "source_needs_block_id",
                format!(
                    "{}:{} links with {old_name} but has no block id and lives in {}; mint an id on a writable note first",
                    file.path_str,
                    edge.raw.source_line + 1,
                    if file.is_archive {
                        "done/"
                    } else {
                        "the previous daily note"
                    }
                ),
                Some("mint an id on a writable source or repair the archived source"),
            ));
        }
        let task = &file.scan.tasks()[edge.raw.source_task];
        let minted = mint_block_id(
            &title_for_mint(&task.description),
            &used_per_file[edge.raw.source_file]
                .iter()
                .map(String::as_str)
                .collect::<HashSet<_>>(),
        );
        // Collision-free within the note (mint_block_id guarantees it
        // against the used set we passed) and vault-wide best effort:
        // ensure no other file already uses it as a task id.
        let mut candidate = minted;
        let mut suffix = 2u32;
        while vault_uses_block_id(
            &files,
            &candidate,
            Some(edge.raw.source_file),
        ) {
            candidate = format!("task-{suffix}");
            suffix += 1;
            if suffix > 10000 {
                break;
            }
        }
        minted_map.insert(key, candidate.clone());
        used_per_file[edge.raw.source_file].insert(candidate.clone());
        minted_list.push(MintedBlock {
            path: file.path_str.clone(),
            line: edge.raw.source_line + 1,
            block_id: candidate,
        });
    }
    minted_list.sort_by(|left, right| {
        (&left.path, left.line).cmp(&(&right.path, right.line))
    });

    // Final source identities (minted where needed).
    let source_identity = |edge: &ResolvedEdge| -> (PathBuf, String) {
        let file = &files[edge.raw.source_file];
        let id = edge
            .raw
            .source_block
            .clone()
            .or_else(|| {
                minted_map
                    .get(&(edge.raw.source_file, edge.raw.source_task))
                    .cloned()
            })
            .expect("reverse source has an id after minting");
        (file.relative.clone(), id)
    };
    let target_identity = |edge: &ResolvedEdge| -> (PathBuf, String) {
        (
            files[edge.target_file].relative.clone(),
            edge.target_block.clone(),
        )
    };

    // Preflight reverse collisions against resolved identities.
    // Build the set of existing NEW-key opposite edges (for merge) and
    // other-label opposite edges (for reverse_conflict).
    // Map (owner task identity) -> existing entries (identity of linked
    // task, label key).
    let mut existing_by_owner: HashMap<
        (PathBuf, String),
        Vec<(PathBuf, String, String)>,
    > = HashMap::new();
    for file in files.iter() {
        let refs: Vec<&str> =
            file.line_bodies.iter().map(String::as_str).collect();
        for task in file.scan.tasks() {
            let Some(task_id) = &task.block_id else {
                continue;
            };
            // Skip duplicate-id tasks: they are already refused as
            // endpoints; do not let them shadow collision checks.
            if file
                .block_to_tasks
                .get(task_id)
                .is_some_and(|list| list.len() != 1)
            {
                continue;
            }
            let owner = (file.relative.clone(), task_id.clone());
            for child in links_children_of(&refs, &file.fenced, task.line_index)
            {
                let LinksLine::Accepted { entries, .. } = &child.parsed else {
                    continue;
                };
                for entry in entries {
                    let Some(resolved_path) = resolve_target_path(
                        &file.relative,
                        &entry.target,
                        &note_index,
                        &files,
                        &file_by_path,
                    ) else {
                        continue;
                    };
                    let Some(&linked_file) = file_by_path.get(&resolved_path)
                    else {
                        continue;
                    };
                    // Only certain links participate: the linked task must
                    // resolve to exactly one task.
                    let linked_tasks =
                        files[linked_file].block_to_tasks.get(&entry.block_id);
                    if !matches!(linked_tasks, Some(list) if list.len() == 1) {
                        continue;
                    }
                    let label_key = relation_labels::relation_label_key(
                        entry.label.as_deref().unwrap_or(""),
                    );
                    existing_by_owner.entry(owner.clone()).or_default().push((
                        resolved_path,
                        entry.block_id.clone(),
                        label_key,
                    ));
                }
            }
        }
    }

    #[derive(Debug, Clone)]
    struct PlannedMove {
        edge_index: usize,
        action: &'static str,
    }
    let mut planned: Vec<PlannedMove> = Vec::new();
    let mut merged_count = 0usize;
    // For deterministic conflict reporting, check in sorted order.
    for (edge_index, edge) in resolved.iter().enumerate() {
        let source_id = source_identity(edge);
        let target_id = target_identity(edge);
        // No self-link writes (already refused above, but re-check after
        // minting in case a minted source collides with its target).
        if source_id == target_id {
            let source_file = &files[edge.raw.source_file];
            return Err(ReverseFailure::new(
                "unresolved_target",
                format!(
                    "{}:{} would reverse onto itself; a reverse rename never writes a self-link",
                    source_file.path_str,
                    edge.raw.source_line + 1
                ),
                Some("relabel or unlink the self-link, then retry"),
            ));
        }
        // Does T already link S?
        let opposite = existing_by_owner.get(&target_id);
        let mut has_new = false;
        let mut conflicts: Vec<(PathBuf, String)> = Vec::new();
        if let Some(entries) = opposite {
            for (linked_path, linked_id, label_key) in entries {
                if (linked_path, linked_id) == (&source_id.0, &source_id.1) {
                    if label_key == &new_key {
                        has_new = true;
                    } else {
                        conflicts
                            .push((linked_path.clone(), label_key.clone()));
                    }
                }
            }
        }
        // Also consider other moves in the same set that add the same
        // opposite edge: two moves that would both add T —NEW→ S are a
        // duplicate identity write unless one merges. Deduplicate here:
        // the first move adds it, later identical moves merge.
        // (Original scan order is deterministic, so this is stable.)
        if !has_new && !conflicts.is_empty() {
            let source_file = &files[edge.raw.source_file];
            let target_file = &files[edge.target_file];
            let pairs = conflicts
                .iter()
                .map(|(path, label)| {
                    format!(
                        "{} links {} with a different label",
                        display_relative(path),
                        label
                    )
                })
                .collect::<Vec<_>>()
                .join("; ");
            return Err(ReverseFailure::new(
                "reverse_conflict",
                format!(
                    "{}:{} reverses onto {}:{} which already links back with another label ({pairs})",
                    source_file.path_str,
                    edge.raw.source_line + 1,
                    target_file.path_str,
                    edge.target_line + 1,
                ),
                Some("relabel or unlink the existing edge, then retry"),
            ));
        }
        if has_new {
            planned.push(PlannedMove {
                edge_index,
                action: "merged",
            });
            merged_count += 1;
        } else {
            planned.push(PlannedMove {
                edge_index,
                action: "moved",
            });
        }
    }

    // Aggregate per-task LINKS edits.
    // removals: (file, owner task index) -> set of canonical identities to remove
    // additions: (file, owner task index) -> vec of (link text, label)
    let mut removals: BTreeMap<(usize, usize), BTreeSet<String>> =
        BTreeMap::new();
    let mut additions: BTreeMap<(usize, usize), Vec<(String, Option<String>)>> =
        BTreeMap::new();
    let mut moves_report: Vec<ReverseMove> = Vec::new();
    for plan in &planned {
        let edge = &resolved[plan.edge_index];
        let source_id = source_identity(edge);
        let target_id = target_identity(edge);
        let source_file = &files[edge.raw.source_file];
        let target_file = &files[edge.target_file];
        let source_final_id = source_id.1.clone();
        let target_final_id = target_id.1.clone();
        // Source task text comes from the scan; target text likewise.
        let source_task = &source_file.scan.tasks()[edge.raw.source_task];
        let target_task = &target_file.scan.tasks()[edge.target_task];
        let source_endpoint = MoveEndpoint {
            path: source_file.path_str.clone(),
            line: edge.raw.source_line + 1,
            block_id: Some(source_final_id.clone()),
            text: source_task.description.clone(),
        };
        let target_endpoint = MoveEndpoint {
            path: target_file.path_str.clone(),
            line: edge.target_line + 1,
            block_id: Some(target_final_id.clone()),
            text: target_task.description.clone(),
        };
        moves_report.push(ReverseMove {
            source: source_endpoint,
            target: target_endpoint,
            from_label: old_record.name.clone(),
            to_label: new_name.to_string(),
            action: plan.action,
        });
        // Removal from S (always, even when merged: S's OLD entry drops).
        removals
            .entry((edge.raw.source_file, edge.raw.source_task))
            .or_default()
            .insert(edge.raw.target_entry_identity.clone());
        if plan.action == "moved" {
            // Addition to T: NEW [[canonical link to S]].
            let link = canonical_link(
                &source_id.0,
                &source_final_id,
                &target_id.0,
                &note_index,
            );
            additions
                .entry((edge.target_file, edge.target_task))
                .or_default()
                .push((link, Some(new_name.to_string())));
        }
    }
    moves_report.sort_by(|left, right| {
        (
            &left.source.path,
            left.source.line,
            &left.target.path,
            left.target.line,
        )
            .cmp(&(
                &right.source.path,
                right.source.line,
                &right.target.path,
                right.target.line,
            ))
    });

    // Stage working copies: mints, then LINKS, then mirrors.
    let mut working: Vec<WorkingFile> = files
        .iter()
        .map(|file| WorkingFile {
            relative: file.relative.clone(),
            path_str: file.path_str.clone(),
            contents: file.contents.clone(),
            ending: file.ending,
            final_newline: file.final_newline,
            is_archive: file.is_archive,
            is_previous_daily: file.is_previous_daily,
            done: is_done_path(&file.relative),
            changes: Vec::new(),
        })
        .collect();

    // Mints first: one line rewrite per source task.
    for ((file_index, task_index), minted_id) in &minted_map {
        let original_line =
            files[*file_index].scan.tasks()[*task_index].line_index;
        let before =
            logical_line_at(&working[*file_index].contents, original_line)
                .unwrap_or_default();
        let after = format!("{} ^{minted_id}", before.trim_end());
        working[*file_index].contents = replace_logical_line(
            &working[*file_index].contents,
            working[*file_index].ending,
            working[*file_index].final_newline,
            original_line,
            &after,
        );
        working[*file_index].changes.push(LineChange {
            line: original_line + 1,
            kind: "mint",
            before,
            after,
        });
    }

    // LINKS edits second, owner by owner with explicit reparse between
    // stages so inserts/removals never shift later owners onto the wrong
    // task. Owners are processed in deterministic file/task order.
    let mut owners: BTreeSet<(usize, usize)> = BTreeSet::new();
    for key in removals.keys() {
        owners.insert(*key);
    }
    for key in additions.keys() {
        owners.insert(*key);
    }
    let mut links_changed_entries = 0usize;
    for (file_index, task_index) in owners {
        // Find the owner's current line via its final block id.
        let final_id =
            owner_final_id(&files, &minted_map, file_index, task_index);
        let current_line = find_task_line(
            &working[file_index].contents,
            &settings,
            final_id.as_deref(),
            file_index,
            task_index,
            &files,
        );
        let Some(owner_line) = current_line else {
            return Err(ReverseFailure::new(
                "verify_failed",
                format!(
                    "could not locate {} task for LINKS planning",
                    working[file_index].path_str
                ),
                Some("retry the rename"),
            ));
        };
        let refs_owned: Vec<String> =
            logical_lines_owned(&working[file_index].contents);
        let refs: Vec<&str> = refs_owned.iter().map(String::as_str).collect();
        let fenced = fenced_lines(&refs, 0..refs.len());
        let mut edit = LinksEdit {
            add: Vec::new(),
            remove: BTreeSet::new(),
            relabel: BTreeMap::new(),
        };
        if let Some(remove) = removals.get(&(file_index, task_index)) {
            edit.remove.extend(remove.iter().cloned());
        }
        if let Some(add) = additions.get(&(file_index, task_index)) {
            edit.add.extend(add.iter().cloned());
        }
        match plan_links_edit(&refs, &fenced, owner_line, &edit) {
            crate::native::task_relations::plan::LinksLinePlan::Noop => {}
            crate::native::task_relations::plan::LinksLinePlan::Malformed => {
                return Err(ReverseFailure::new(
                    "malformed_links_line",
                    format!(
                        "malformed LINKS line {}:{} would receive a reversed link; repair it and retry",
                        working[file_index].path_str,
                        owner_line + 1
                    ),
                    Some(
                        "repair the malformed relation line and retry the rename",
                    ),
                ));
            }
            crate::native::task_relations::plan::LinksLinePlan::InvalidLabel(
                error,
            ) => {
                return Err(ReverseFailure::new(
                    "invalid_label",
                    format!("invalid relation label: {error:?}"),
                    Some("choose a valid NEW label"),
                ));
            }
            crate::native::task_relations::plan::LinksLinePlan::Edits {
                edits,
                report,
            } => {
                links_changed_entries += report.added.len()
                    + report.removed.len()
                    + report.relabeled.len();
                let before_lines = refs_owned.clone();
                apply_blocks_edits(
                    &mut working[file_index],
                    edits,
                );
                // Record truthful previews for inserted/removed lines.
                // Re-diff the affected range minimally: compare before and
                // after logical lines and record every changed index. The
                // slot helpers keep edits local, so a full diff is small
                // and exact.
                let after_owned = logical_lines_owned(
                    &working[file_index].contents,
                );
                record_line_diff(
                    &mut working[file_index].changes,
                    &before_lines,
                    &after_owned,
                    "links",
                );
            }
        }
    }

    // Mirrors third, from the final graph via rescan.
    let mut mirrors_changed_entries = 0usize;
    let mirrors_enabled = close_policy.linked_from_lines;
    if mirrors_enabled {
        // Rescan working files for final edges.
        let rescanned = rescan_working(&working, &settings);
        // Final edges by target identity.
        let mut final_by_target: BTreeMap<
            (PathBuf, String),
            Vec<(PathBuf, String, Option<String>)>,
        > = BTreeMap::new();
        for (file_index, scan) in rescanned.iter().enumerate() {
            let relative = &working[file_index].relative;
            let refs_owned = logical_lines_owned(&working[file_index].contents);
            let refs: Vec<&str> =
                refs_owned.iter().map(String::as_str).collect();
            let fenced = fenced_lines(&refs, 0..refs.len());
            for task in scan.tasks() {
                let Some(task_id) = &task.block_id else {
                    continue;
                };
                for child in links_children_of(&refs, &fenced, task.line_index)
                {
                    let LinksLine::Accepted { entries, .. } = &child.parsed
                    else {
                        continue;
                    };
                    for entry in entries {
                        let Some(resolved_path) = resolve_target_path(
                            relative,
                            &entry.target,
                            &note_index,
                            &files,
                            &file_by_path,
                        ) else {
                            continue;
                        };
                        final_by_target
                            .entry((resolved_path, entry.block_id.clone()))
                            .or_default()
                            .push((
                                relative.clone(),
                                task_id.clone(),
                                entry.label.clone(),
                            ));
                    }
                }
            }
        }
        // Affected owners for mirrors: every source and target that is
        // writable.
        let mut mirror_owners: BTreeSet<(usize, usize, String)> =
            BTreeSet::new();
        for plan in &planned {
            let edge = &resolved[plan.edge_index];
            for (file_index, task_index) in [
                (edge.raw.source_file, edge.raw.source_task),
                (edge.target_file, edge.target_task),
            ] {
                if working[file_index].is_archive
                    || working[file_index].is_previous_daily
                {
                    continue;
                }
                let final_id =
                    owner_final_id(&files, &minted_map, file_index, task_index);
                if let Some(id) = final_id {
                    mirror_owners.insert((file_index, task_index, id));
                }
            }
        }
        for (file_index, _task_index, owner_id) in mirror_owners {
            let current_line = find_task_line(
                &working[file_index].contents,
                &settings,
                Some(&owner_id),
                file_index,
                _task_index,
                &files,
            );
            let Some(owner_line) = current_line else {
                continue;
            };
            let refs_owned = logical_lines_owned(&working[file_index].contents);
            let refs: Vec<&str> =
                refs_owned.iter().map(String::as_str).collect();
            let fenced = fenced_lines(&refs, 0..refs.len());
            // Existing LINKED FROM children for uncertainty preservation.
            let children = linked_from_children_of(&refs, &fenced, owner_line);
            if children
                .iter()
                .any(|child| matches!(child.parsed, LinkedFromLine::Malformed))
            {
                // Malformed affected destination mirrors refuse instead of
                // losing information.
                return Err(ReverseFailure::new(
                    "malformed_links_line",
                    format!(
                        "malformed LINKED FROM line {}:{} would change; repair it and retry",
                        working[file_index].path_str,
                        owner_line + 1
                    ),
                    Some(
                        "repair the malformed relation line and retry the rename",
                    ),
                ));
            }
            // Desired from the final graph.
            let owner_key =
                (working[file_index].relative.clone(), owner_id.clone());
            let mut desired: BTreeSet<(String, Option<String>)> =
                BTreeSet::new();
            let mut relation_sources: BTreeSet<(PathBuf, String)> =
                BTreeSet::new();
            if let Some(sources) = final_by_target.get(&owner_key) {
                for (source_path, source_id, label) in sources {
                    relation_sources
                        .insert((source_path.clone(), source_id.clone()));
                    let link = canonical_link(
                        source_path,
                        source_id,
                        &owner_key.0,
                        &note_index,
                    );
                    desired.insert((link, label.clone()));
                }
            }
            // Preserve uncertain existing mirrors: entries that do not
            // resolve to exactly one task.
            for child in &children {
                let LinkedFromLine::Accepted { entries, .. } = &child.parsed
                else {
                    continue;
                };
                for entry in entries {
                    if entry.kind
                        == crate::native::task_relations::RelationEntryKind::Mention
                    {
                        let source_path = resolve_target_path(
                            &working[file_index].relative,
                            &entry.target,
                            &note_index,
                            &files,
                            &file_by_path,
                        );
                        if should_preserve_mention(
                            source_path,
                            &entry.block_id,
                            &relation_sources,
                        ) {
                            desired.insert((
                                entry.canonical_text(),
                                entry.storage_label(),
                            ));
                        }
                        continue;
                    }
                    let resolved_path = resolve_target_path(
                        &working[file_index].relative,
                        &entry.target,
                        &note_index,
                        &files,
                        &file_by_path,
                    );
                    let certain = resolved_path
                        .as_ref()
                        .and_then(|path| file_by_path.get(path))
                        .and_then(|index| {
                            files[*index].block_to_tasks.get(&entry.block_id)
                        })
                        .is_some_and(|list| list.len() == 1);
                    if !certain {
                        desired.insert((
                            entry.canonical_text(),
                            entry.storage_label(),
                        ));
                    }
                }
            }
            match plan_linked_from_line(
                &refs,
                &fenced,
                owner_line,
                &desired,
            ) {
                crate::native::task_dependencies::blocks::BlocksLinePlan::Noop => {}
                crate::native::task_dependencies::blocks::BlocksLinePlan::Malformed => {
                    return Err(ReverseFailure::new(
                        "malformed_links_line",
                        format!(
                            "malformed LINKED FROM line {}:{} would change; repair it and retry",
                            working[file_index].path_str,
                            owner_line + 1
                        ),
                        Some(
                            "repair the malformed relation line and retry the rename",
                        ),
                    ));
                }
                crate::native::task_dependencies::blocks::BlocksLinePlan::Edits {
                    edits,
                    report,
                } => {
                    mirrors_changed_entries +=
                        report.added.len() + report.removed.len();
                    let before_lines = refs_owned.clone();
                    apply_blocks_edits(
                        &mut working[file_index],
                        edits
                            .into_iter()
                            .map(|edit| match edit {
                                crate::native::task_dependencies::blocks::BlocksEdit::Insert { line_index, text } => {
                                    BlocksEdit::Insert { line_index, text }
                                }
                                crate::native::task_dependencies::blocks::BlocksEdit::Replace { line_index, text } => {
                                    BlocksEdit::Replace { line_index, text }
                                }
                                crate::native::task_dependencies::blocks::BlocksEdit::Remove { line_index } => {
                                    BlocksEdit::Remove { line_index }
                                }
                            })
                            .collect(),
                    );
                    let after_owned = logical_lines_owned(
                        &working[file_index].contents,
                    );
                    record_line_diff(
                        &mut working[file_index].changes,
                        &before_lines,
                        &after_owned,
                        "linked_from",
                    );
                }
            }
        }
    }

    // Build file plans in deterministic path order.
    let mut file_plans: Vec<FilePlan> = Vec::new();
    let mut ordered: Vec<usize> = (0..working.len()).collect();
    ordered.sort_by(|left, right| {
        working[*left].path_str.cmp(&working[*right].path_str)
    });
    for index in ordered {
        let work = &working[index];
        let original = &files[index].contents;
        if work.contents == *original && work.changes.is_empty() {
            continue;
        }
        let mut changes = work.changes.clone();
        changes.sort_by_key(|change| change.line);
        let links = removals.keys().filter(|(file, _)| *file == index).count()
            + additions.keys().filter(|(file, _)| *file == index).count();
        // Count LINKS moves touching this file for the view; mirrors are
        // counted from linked_from changes below.
        let mirrors = changes
            .iter()
            .filter(|change| change.kind == "linked_from")
            .count();
        file_plans.push(FilePlan {
            path: work.path_str.clone(),
            links,
            mirrors,
            done: work.done,
            lines: changes,
            before_contents: original.clone(),
            after_contents: work.contents.clone(),
        });
    }
    file_plans.sort_by(|left, right| left.path.cmp(&right.path));

    let total_links = moves_report.len();
    let done_links = moves_report
        .iter()
        .filter(|mv| mv.source.path.split('/').any(|part| part == "done"))
        .count();
    let done_notes = file_plans.iter().filter(|file| file.done).count();
    let _ = links_changed_entries;
    let _ = mirrors_changed_entries;

    // Hidden-state transfer, inverse, and closing-label warnings mirror
    // the ordinary rename.
    let old_display = old_record.name.clone();
    let hidden_before = catalog.is_hidden(&old_key);
    let source_has_state = catalog.has_explicit_state(&old_key);
    let target_has_state = catalog.has_explicit_state(&new_key);
    let hidden_after = if old_key == new_key {
        hidden_before
    } else if target_has_state {
        catalog.is_hidden(&new_key)
    } else {
        hidden_before
    };
    let moved = old_key != new_key && source_has_state && !target_has_state;
    let state_note = if moved && hidden_before {
        format!("hidden state moves from {old_display} to {new_name}")
    } else if moved {
        format!("visible state moves from {old_display} to {new_name}")
    } else if hidden_before && old_key != new_key {
        format!(
            "{old_display} stays hidden; {new_name} already has explicit state"
        )
    } else if hidden_before {
        format!("{new_name} remains hidden")
    } else {
        format!("{new_name} is not hidden")
    };
    let before_inverse =
        crate::native::task_relations::label::inverse_phrase(&old_display)
            .map(str::to_string);
    let after_inverse =
        crate::native::task_relations::label::inverse_phrase(new_name)
            .map(str::to_string);
    let inverse_note = if before_inverse == after_inverse {
        "inverse phrase is unchanged".to_string()
    } else {
        format!(
            "inverse phrase changes from {} to {}",
            before_inverse.as_deref().unwrap_or("none"),
            after_inverse.as_deref().unwrap_or("none"),
        )
    };
    if close_policy.is_cancel_label(&old_key) {
        warnings.push(format!(
            "“{old_display}” is a cancel label in config.yml — update relations.cancel_labels"
        ));
    }
    if new_key != old_key && close_policy.is_cancel_label(&new_key) {
        warnings.push(format!(
            "“{new_name}” is a cancel label in config.yml — update relations.cancel_labels"
        ));
    }
    if close_policy.is_complete_label(&old_key) {
        warnings.push(format!(
            "“{old_display}” is a complete label in config.yml · a rename never completes anything — check relations.complete_labels"
        ));
    }
    if new_key != old_key && close_policy.is_complete_label(&new_key) {
        warnings.push(format!(
            "“{new_name}” is a complete label in config.yml · a rename never completes anything — check relations.complete_labels"
        ));
    }
    // Same-key closing-label warning when the key itself closes: the
    // two branches above skip the NEW side for old==new, so add it here.
    if old_key == new_key
        && (close_policy.is_cancel_label(&new_key)
            || close_policy.is_complete_label(&new_key))
    {
        if close_policy.is_cancel_label(&new_key)
            && !warnings.iter().any(|warning| warning.contains(new_name))
        {
            warnings.push(format!(
                "“{new_name}” is a cancel label in config.yml — update relations.cancel_labels"
            ));
        }
        if close_policy.is_complete_label(&new_key)
            && !warnings.iter().any(|warning| warning.contains(new_name))
        {
            warnings.push(format!(
                "“{new_name}” is a complete label in config.yml · a rename never completes anything — check relations.complete_labels"
            ));
        }
    }

    let is_closing_new = close_policy.is_cancel_label(&new_key)
        || close_policy.is_complete_label(&new_key);
    let mut stay_open: Vec<MoveEndpoint> = Vec::new();
    if is_closing_new {
        // New sources are the original targets. List the open ones that
        // keep their status: a rename never closes.
        for plan in &planned {
            let edge = &resolved[plan.edge_index];
            let target_task =
                &files[edge.target_file].scan.tasks()[edge.target_task];
            if target_task.status_symbol != 'x'
                && target_task.status_symbol != '-'
            {
                // Find the matching move's target endpoint.
                if let Some(mv) = moves_report.iter().find(|mv| {
                    mv.source.path == files[edge.raw.source_file].path_str
                        && mv.source.line == edge.raw.source_line + 1
                        && mv.target.path == files[edge.target_file].path_str
                        && mv.target.line == edge.target_line + 1
                }) {
                    stay_open.push(mv.target.clone());
                }
            }
        }
        stay_open.sort_by(|left, right| {
            (&left.path, left.line).cmp(&(&right.path, right.line))
        });
    }
    let report = ReverseReport {
        schema_version: SCHEMA_VERSION,
        ok: true,
        mode: if dry_run { "dry_run" } else { "write" }.to_string(),
        changed: true,
        reversed: true,
        from: ReverseSide {
            key: old_key.clone(),
            name: old_display,
        },
        to: ReverseSide {
            key: new_key.clone(),
            name: new_name.to_string(),
        },
        merge,
        moves: moves_report.clone(),
        minted_block_ids: minted_list.clone(),
        stay_open,
        files: file_plans
            .iter()
            .map(|file| FilePlanView {
                path: file.path.clone(),
                links: file.links,
                mirrors: file.mirrors,
                lines: file.lines.clone(),
            })
            .collect(),
        counts: ReverseCounts {
            links: total_links,
            notes: file_plans.len(),
            mirrors: mirrors_changed_entries,
            done_links,
            done_notes,
        },
        state: StateChange {
            hidden_before,
            hidden_after,
            moved,
            note: state_note,
        },
        inverse: InverseChange {
            before: before_inverse,
            after: after_inverse,
            note: inverse_note,
        },
        replacement:
            crate::native::task_relations::label::relation_label_quoted_form(
                new_name,
            ),
        hide_replacement:
            crate::native::task_relations::label::relation_label_hide_form(
                new_name,
            ),
        commit: None,
        sync: SyncInfo {
            pre: "skipped".to_string(),
            post: if dry_run { "not_run" } else { "pending" }.to_string(),
        },
        warnings,
    };
    Ok(PreparedReverse {
        old_key,
        new_key,
        new_name: new_name.to_string(),
        via: via.to_string(),
        files: file_plans,
        moves: moves_report,
        minted: minted_list,
        total_entries,
        merged_count,
        report,
    })
}

fn previous_daily_relative(vault: &Path, paths: &[PathBuf]) -> Option<PathBuf> {
    let day_file = crate::native::pomodoro::day_file_for(vault);
    let anchor = task_status_hooks::daily_anchor_date(
        &day_file,
        bob_env::current_datetime().date(),
    );
    let mut previous: Option<(chrono::NaiveDate, PathBuf)> = None;
    for relative in paths {
        let Some(date) = task_status_hooks::canonical_daily_date(relative)
        else {
            continue;
        };
        if date < anchor && previous.as_ref().is_none_or(|(day, _)| date > *day)
        {
            previous = Some((date, relative.clone()));
        }
    }
    previous.map(|(_, path)| path)
}

fn resolve_target_path(
    source: &Path,
    target: &str,
    index: &NoteIndex,
    _files: &[VaultFile],
    by_path: &HashMap<PathBuf, usize>,
) -> Option<PathBuf> {
    if target.is_empty() {
        return Some(source.to_path_buf());
    }
    let candidate = target_to_markdown_path(target)?;
    if is_archive_path(&candidate) {
        if by_path.contains_key(&candidate) {
            return Some(candidate);
        }
        // Archive targets must be explicit; never fall back to a
        // basename match that could resolve the wrong history file.
        return None;
    }
    index.resolve(Some(source), target)
}

fn line_might_name(line: &str, old_key: &str) -> bool {
    relation_labels::relation_label_key(line).contains(old_key)
}

fn malformed_line_failure(
    path: &str,
    line_index: usize,
    kind: &str,
    old_name: &str,
) -> ReverseFailure {
    ReverseFailure::new(
        "malformed_links_line",
        format!(
            "malformed {kind} line {path}:{} may contain relation label {old_name}",
            line_index + 1
        ),
        Some("repair the malformed relation line and retry the rename"),
    )
}

fn unresolved_target_failure(
    path: &str,
    line: usize,
    block_id: &str,
    raw: &str,
) -> ReverseFailure {
    let target = if raw.is_empty() {
        format!("#^{block_id}")
    } else {
        format!("{raw}#^{block_id}")
    };
    ReverseFailure::new(
        "unresolved_target",
        format!(
            "{path}:{line} links {target} which does not resolve to one task"
        ),
        Some("repair the link so it names one #task, then retry"),
    )
}

fn block_id_occurrences_in_file(contents: &str, id: &str) -> usize {
    let _needle = format!("#{id}");
    // Count trailing block-id occurrences that name this exact id.
    // block_ids_in_markdown returns every trailing id; count exact matches.
    block_ids_in_markdown(contents)
        .iter()
        .filter(|found| found.as_str() == id)
        .count()
        .max(
            contents
                .match_indices(&format!("^{id}"))
                .filter(|(start, _)| {
                    start
                        .checked_sub(1)
                        .and_then(|prev| contents.as_bytes().get(prev))
                        .is_none_or(|byte| byte.is_ascii_whitespace())
                })
                .count()
                .min(1),
        )
}

fn vault_uses_block_id(
    files: &[VaultFile],
    id: &str,
    except: Option<usize>,
) -> bool {
    for (index, file) in files.iter().enumerate() {
        if Some(index) == except {
            continue;
        }
        if file.block_to_tasks.contains_key(id) {
            return true;
        }
    }
    false
}

fn title_for_mint(description: &str) -> String {
    let mut state = description;
    for _ in 0..20 {
        if let Some((start, _, _)) = trailing_dataview_field(state) {
            state = state[..start].trim_end();
            continue;
        }
        if let Some(start) = trailing_task_tag_start(state) {
            state = state[..start].trim_end();
            continue;
        }
        break;
    }
    state.to_string()
}

fn trailing_dataview_field(text: &str) -> Option<(usize, &str, &str)> {
    let trimmed = text.trim_end();
    let start = trimmed.rfind('[')?;
    let field = &trimmed[start..];
    if field.starts_with('[') && field.ends_with(']') && field.contains("::") {
        Some((start, "", field))
    } else {
        None
    }
}

fn trailing_task_tag_start(text: &str) -> Option<usize> {
    let trimmed = text.trim_end();
    for tag in ["#task", "#ref", "#prj_is_blocked"] {
        if let Some(start) = trimmed.rfind(tag)
            && (trimmed[start..].trim() == tag
                || trimmed[start..].starts_with(&format!("{tag} ")))
        {
            // Only strip when the tag is trailing.
            let rest = trimmed[start + tag.len()..].trim();
            if rest.is_empty() {
                return Some(start);
            }
        }
    }
    None
}

fn logical_lines_owned(contents: &str) -> Vec<String> {
    if contents.is_empty() {
        return Vec::new();
    }
    contents
        .split_inclusive('\n')
        .map(|segment| {
            let without_lf = segment.strip_suffix('\n').unwrap_or(segment);
            without_lf
                .strip_suffix('\r')
                .unwrap_or(without_lf)
                .to_string()
        })
        .collect()
}

fn logical_line_at(contents: &str, index: usize) -> Option<String> {
    logical_lines_owned(contents).get(index).cloned()
}

fn replace_logical_line(
    contents: &str,
    ending: &str,
    final_newline: bool,
    index: usize,
    new_line: &str,
) -> String {
    let mut lines = logical_lines_owned(contents);
    if index >= lines.len() {
        return contents.to_string();
    }
    lines[index] = new_line.to_string();
    let joined = lines.join(ending);
    if final_newline {
        format!("{joined}{ending}")
    } else {
        joined
    }
}

fn apply_blocks_edits(working: &mut WorkingFile, edits: Vec<BlocksEdit>) {
    let mut lines = logical_lines_owned(&working.contents);
    let mut ordered = edits;
    ordered.sort_by(|left, right| {
        let left_index = match left {
            BlocksEdit::Insert { line_index, .. }
            | BlocksEdit::Replace { line_index, .. }
            | BlocksEdit::Remove { line_index } => *line_index,
        };
        let right_index = match right {
            BlocksEdit::Insert { line_index, .. }
            | BlocksEdit::Replace { line_index, .. }
            | BlocksEdit::Remove { line_index } => *line_index,
        };
        right_index
            .cmp(&left_index)
            .then_with(|| rank_edit(left).cmp(&rank_edit(right)))
    });
    for edit in ordered {
        match edit {
            BlocksEdit::Replace { line_index, text } => {
                if line_index < lines.len() {
                    lines[line_index] = text;
                }
            }
            BlocksEdit::Remove { line_index } => {
                if line_index < lines.len() {
                    lines.remove(line_index);
                }
            }
            BlocksEdit::Insert { line_index, text } => {
                let index = line_index.min(lines.len());
                lines.insert(index, text);
            }
        }
    }
    let joined = lines.join(working.ending);
    working.contents = if working.final_newline {
        format!("{joined}{}", working.ending)
    } else {
        joined
    };
}

fn rank_edit(edit: &BlocksEdit) -> u8 {
    match edit {
        BlocksEdit::Remove { .. } => 0,
        BlocksEdit::Replace { .. } => 1,
        BlocksEdit::Insert { .. } => 2,
    }
}

fn record_line_diff(
    changes: &mut Vec<LineChange>,
    before: &[String],
    after: &[String],
    kind: &'static str,
) {
    let max = before.len().max(after.len());
    for index in 0..max {
        let left = before.get(index).map(String::as_str).unwrap_or("");
        let right = after.get(index).map(String::as_str).unwrap_or("");
        if left != right {
            changes.push(LineChange {
                line: index + 1,
                kind,
                before: left.to_string(),
                after: right.to_string(),
            });
        }
    }
}

fn owner_final_id(
    files: &[VaultFile],
    minted: &BTreeMap<(usize, usize), String>,
    file_index: usize,
    task_index: usize,
) -> Option<String> {
    if let Some(id) = &files[file_index].scan.tasks().get(task_index)?.block_id
    {
        return Some(id.clone());
    }
    minted.get(&(file_index, task_index)).cloned()
}

fn find_task_line(
    contents: &str,
    settings: &crate::native::note_tasks::NoteTaskSettings,
    block_id: Option<&str>,
    _file_index: usize,
    _task_index: usize,
    _files: &[VaultFile],
) -> Option<usize> {
    let scan = crate::native::note_tasks::scan(contents, settings);
    if let Some(id) = block_id {
        let mut found = None;
        for task in scan.tasks() {
            if task.block_id.as_deref() == Some(id) {
                if found.is_some() {
                    return None;
                }
                found = Some(task.line_index);
            }
        }
        if found.is_some() {
            return found;
        }
    }
    // Fallback: original line still valid when no shifts occurred.
    let original = _files[_file_index]
        .scan
        .tasks()
        .get(_task_index)?
        .line_index;
    let current_tasks = scan.tasks();
    if current_tasks.iter().any(|task| task.line_index == original) {
        return Some(original);
    }
    None
}

fn rescan_working(
    working: &[WorkingFile],
    settings: &crate::native::note_tasks::NoteTaskSettings,
) -> Vec<crate::native::note_tasks::NoteTaskScan> {
    working
        .iter()
        .map(|file| crate::native::note_tasks::scan(&file.contents, settings))
        .collect()
}

fn run_live(matches: &ArgMatches, format: OutputFormat) -> i32 {
    let child_env = ob::child_env();
    let vault = matches
        .get_one::<OsString>("bob-dir")
        .map(PathBuf::from)
        .map(|path| bob_env::expand_tilde(&path))
        .unwrap_or_else(bob_env::bob_dir);
    let offline = matches.get_flag("offline");
    let json_mode = format == OutputFormat::Json;
    let outcome = crate::native::vault_rewrite::run(
        &vault,
        &child_env,
        COMMAND_NAME,
        "relation label reverses",
        offline,
        json_mode,
        || {
            let prepared =
                prepare_from_matches(matches).map_err(|failure| {
                    crate::native::vault_rewrite::TxnFailure::new(
                        failure.code,
                        failure.message,
                        failure.hint,
                    )
                })?;
            let inner = Rc::new(prepared);
            let files = inner
                .files
                .iter()
                .map(|file| crate::native::vault_rewrite::FileRewrite {
                    path: file.path.clone(),
                    before_contents: file.before_contents.clone(),
                    after_contents: file.after_contents.clone(),
                })
                .collect::<Vec<_>>();
            let subject = format!(
                "bob relations rename: {} → {} (reversed)",
                inner.report.from.name, inner.new_name
            );
            let post_inner = Rc::clone(&inner);
            let post_vault = vault.clone();
            let verify_inner = Rc::clone(&inner);
            let verify_vault = vault.clone();
            let invalidate_vault = vault.clone();
            Ok(crate::native::vault_rewrite::ReadyPlan {
                files,
                subject,
                post_write: Box::new(move || {
                    append_rename_event(&post_vault, &post_inner)
                        .map(|path| vec![path])
                }),
                verify: Box::new(move || {
                    verify_reversed(&verify_vault, &verify_inner)
                }),
                on_committed: Box::new(move || {
                    relation_labels::invalidate_scan(&invalidate_vault);
                }),
                context: inner,
            })
        },
    );
    match outcome {
        Ok(outcome) => {
            let mut report = outcome.context.report.clone();
            report.mode = "write".to_string();
            report.sync.pre = outcome.sync_pre.to_string();
            report.sync.post = outcome.sync_post.to_string();
            report.commit = Some(json!({
                "sha": outcome.sha,
                "subject": outcome.subject,
            }));
            if let Some(dirty) = outcome.dirty_warning {
                report.warnings.push(dirty);
            }
            if let Some(warning) = outcome.post_sync_warning {
                report.warnings.push(warning);
            }
            print_success(&report, format);
            0
        }
        Err(failure) => print_failure(
            format,
            ReverseFailure::new(failure.code, failure.message, failure.hint),
        ),
    }
}

fn append_rename_event(
    vault: &Path,
    prepared: &PreparedReverse,
) -> Result<PathBuf, String> {
    let host = journal::host_label();
    let ts = journal::event_timestamp();
    let payload = journal::EventPayload::Rename {
        from: prepared.old_key.clone(),
        to: prepared.new_key.clone(),
        from_name: prepared.report.from.name.clone(),
        to_name: prepared.new_name.clone(),
    };
    let path = journal::host_journal_path(vault, &host);
    journal::append_event_at(vault, &host, &payload, &prepared.via, ts)
        .map_err(|error| {
            format!("could not append relation_label.rename: {error}")
        })?;
    Ok(path)
}

fn verify_reversed(
    vault: &Path,
    prepared: &PreparedReverse,
) -> Result<(), String> {
    let (paths, _) = capture_dependency_tasks::vault_note_paths(vault);
    let planned = prepared
        .files
        .iter()
        .map(|file| (file.path.as_str(), file))
        .collect::<BTreeMap<_, _>>();
    let note_index = NoteIndex::from_paths(paths.iter().cloned());
    let settings = crate::native::note_tasks::read_settings(vault);
    let mut total_entries = 0usize;
    let mut old_remaining = 0usize;
    for relative in &paths {
        let path = display_relative(relative);
        let absolute = vault.join(relative);
        let contents = fs::read_to_string(&absolute)
            .map_err(|error| format!("reread {path}: {error}"))?;
        if let Some(file) = planned.get(path.as_str())
            && contents != file.after_contents
        {
            return Err(format!("{path} changed during the rename"));
        }
        let body_refs_owned = logical_lines_owned(&contents);
        let body_refs: Vec<&str> =
            body_refs_owned.iter().map(String::as_str).collect();
        let fenced = fenced_lines(&body_refs, 0..body_refs.len());
        for (task_line, line) in body_refs.iter().enumerate() {
            if fenced.contains(&task_line) || !has_task_tag(line) {
                continue;
            }
            for child in links_children_of(&body_refs, &fenced, task_line) {
                let raw = body_refs[child.line_index];
                match child.parsed {
                    LinksLine::Accepted { entries, .. } => {
                        total_entries =
                            total_entries.saturating_add(entries.len());
                        if prepared.old_key != prepared.new_key
                            && let Some(spans) = links_label_spans(raw)
                        {
                            for span in spans {
                                if relation_labels::relation_label_key(
                                    &span.label,
                                ) == prepared.old_key
                                {
                                    old_remaining += 1;
                                }
                            }
                        }
                        if let Some(spans) = links_label_spans(raw) {
                            let _ = spans;
                        }
                    }
                    LinksLine::Malformed
                        if line_might_name(raw, &prepared.old_key) =>
                    {
                        return Err(format!(
                            "malformed LINKS line remains in {path}:{}",
                            child.line_index + 1
                        ));
                    }
                    LinksLine::Malformed
                    | LinksLine::Empty
                    | LinksLine::NotALine => {}
                }
            }
            for child in linked_from_children_of(&body_refs, &fenced, task_line)
            {
                let raw = body_refs[child.line_index];
                match child.parsed {
                    LinkedFromLine::Accepted { .. } => {
                        if let Some(spans) = linked_from_label_spans(raw)
                            && prepared.old_key != prepared.new_key
                        {
                            for span in spans {
                                if relation_labels::relation_label_key(
                                    &span.label,
                                ) == prepared.old_key
                                {
                                    // OLD may remain in read-only
                                    // mirrors (done/ + previous daily
                                    // are never rewritten). Only fail
                                    // when the mirror is writable.
                                    let writable = !is_done_path(relative)
                                        && previous_daily_relative(
                                            vault, &paths,
                                        )
                                        .as_ref()
                                        .is_none_or(|prev| prev != relative);
                                    if writable {
                                        old_remaining += 1;
                                    }
                                }
                            }
                        }
                    }
                    LinkedFromLine::Malformed
                        if line_might_name(raw, &prepared.old_key) =>
                    {
                        return Err(format!(
                            "malformed LINKED FROM line remains in {path}:{}",
                            child.line_index + 1
                        ));
                    }
                    LinkedFromLine::Malformed
                    | LinkedFromLine::Empty
                    | LinkedFromLine::NotALine => {}
                }
            }
        }
        // Minted ids must be present.
        for minted in &prepared.minted {
            if minted.path == path
                && !contents.contains(&format!("^{}", minted.block_id))
            {
                return Err(format!(
                    "minted block id ^{} missing in {}",
                    minted.block_id, path
                ));
            }
        }
    }
    if old_remaining != 0 {
        return Err(format!(
            "{old_remaining} old relation labels remain after the reverse"
        ));
    }
    let expected = prepared.total_entries.saturating_sub(prepared.merged_count);
    if total_entries != expected {
        return Err(format!(
            "LINKS entry count changed (was {expected}, now {total_entries})"
        ));
    }
    // Verify the final directed graph contains every planned move.
    verify_final_graph(vault, prepared, &paths, &note_index, &settings)?;
    Ok(())
}

fn verify_final_graph(
    vault: &Path,
    prepared: &PreparedReverse,
    paths: &[PathBuf],
    note_index: &NoteIndex,
    settings: &crate::native::note_tasks::NoteTaskSettings,
) -> Result<(), String> {
    // Build final edges by rescan.
    let mut by_owner: BTreeMap<
        (PathBuf, String),
        Vec<(PathBuf, String, String)>,
    > = BTreeMap::new();
    let by_path: HashMap<PathBuf, usize> = paths
        .iter()
        .enumerate()
        .map(|(index, path)| (path.clone(), index))
        .collect();
    for relative in paths {
        let contents =
            fs::read_to_string(vault.join(relative)).map_err(|error| {
                format!("reread {}: {error}", display_relative(relative))
            })?;
        let scan = crate::native::note_tasks::scan(&contents, settings);
        let owned = logical_lines_owned(&contents);
        let refs: Vec<&str> = owned.iter().map(String::as_str).collect();
        let fenced = fenced_lines(&refs, 0..refs.len());
        for task in scan.tasks() {
            let Some(task_id) = &task.block_id else {
                continue;
            };
            for child in links_children_of(&refs, &fenced, task.line_index) {
                let LinksLine::Accepted { entries, .. } = &child.parsed else {
                    continue;
                };
                for entry in entries {
                    let label_key = relation_labels::relation_label_key(
                        entry.label.as_deref().unwrap_or(""),
                    );
                    if label_key != prepared.new_key {
                        continue;
                    }
                    // Resolve like the planner (archive-aware).
                    let resolved = if entry.target.is_empty() {
                        Some(relative.clone())
                    } else {
                        let Some(candidate) =
                            target_to_markdown_path(&entry.target)
                        else {
                            continue;
                        };
                        if is_archive_path(&candidate) {
                            by_path
                                .contains_key(&candidate)
                                .then_some(candidate)
                        } else {
                            note_index.resolve(Some(relative), &entry.target)
                        }
                    };
                    let Some(target_path) = resolved else {
                        continue;
                    };
                    by_owner
                        .entry((relative.clone(), task_id.clone()))
                        .or_default()
                        .push((target_path, entry.block_id.clone(), label_key));
                }
            }
        }
    }
    // Every moved edge must appear as T —NEW→ S in the final graph.
    for mv in &prepared.moves {
        if mv.action == "merged" {
            continue;
        }
        let target_path = PathBuf::from(&mv.target.path);
        let target_id = mv.target.block_id.clone().unwrap_or_default();
        let source_path = PathBuf::from(&mv.source.path);
        let source_id = mv.source.block_id.clone().unwrap_or_default();
        let entries = by_owner
            .get(&(target_path.clone(), target_id.clone()))
            .cloned()
            .unwrap_or_default();
        let found = entries.iter().any(|(linked_path, linked_id, _)| {
            linked_path == &source_path && linked_id == &source_id
        });
        if !found {
            return Err(format!(
                "reversed edge missing: {} —{}→ {}",
                mv.target.path, mv.to_label, mv.source.path
            ));
        }
    }
    Ok(())
}

fn print_success(report: &ReverseReport, format: OutputFormat) {
    match format {
        OutputFormat::Json => match serde_json::to_string(report) {
            Ok(text) => println!("{text}"),
            Err(error) => {
                eprintln!("{COMMAND_NAME}: failed to serialize JSON: {error}")
            }
        },
        OutputFormat::Human => print_human(report),
    }
}

fn print_human(report: &ReverseReport) {
    let styler = crate::native::style::Styler::detect();
    let dry = report.mode == "dry_run";
    println!(
        "{} rename {} → {} and reverse {} link{} ({} in done/){}",
        styler.success_prefix(dry),
        report.from.name,
        report.to.name,
        report.counts.links,
        if report.counts.links == 1 { "" } else { "s" },
        report.counts.done_links,
        if dry { " (dry run)" } else { "" },
    );
    // Group moves by original source note, in deterministic order.
    let mut by_source: BTreeMap<&str, Vec<&ReverseMove>> = BTreeMap::new();
    for mv in &report.moves {
        by_source
            .entry(mv.source.path.as_str())
            .or_default()
            .push(mv);
    }
    for (source_path, moves) in by_source {
        println!("  {source_path}:");
        for mv in moves {
            let icon = if mv.action == "merged" { "═" } else { "⇄" };
            println!(
                "    {} {} —{}→ {}  {}:{}",
                icon,
                mv.source.text,
                mv.to_label,
                mv.target.text,
                mv.target.path,
                mv.target.line,
            );
            if mv.action == "merged" {
                println!("      merged: kept existing link");
            }
        }
    }
    if !report.minted_block_ids.is_empty() {
        println!("  minted block ids:");
        for minted in &report.minted_block_ids {
            println!(
                "    {}:{} ^{}",
                minted.path, minted.line, minted.block_id
            );
        }
    }
    let merged = report
        .moves
        .iter()
        .filter(|mv| mv.action == "merged")
        .count();
    if merged > 0 {
        println!(
            "  merged {merged} link{}",
            if merged == 1 { "" } else { "s" }
        );
    }
    println!(
        "  mirrors {} changed in {} notes; statuses none change",
        report.counts.mirrors, report.counts.notes,
    );
    if !report.stay_open.is_empty() {
        println!(
            "  stay open  {} new source{} keep{} their status — a rename never closes",
            report.stay_open.len(),
            if report.stay_open.len() == 1 { "" } else { "s" },
            if report.stay_open.len() == 1 { "s" } else { "" },
        );
        for endpoint in &report.stay_open {
            println!(
                "    {}:{} {}",
                endpoint.path, endpoint.line, endpoint.text
            );
        }
    }
    println!("  state: {}", report.state.note);
    println!("  {}", report.inverse.note);
    if let Some(commit) = &report.commit {
        let sha = commit
            .get("sha")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("(unknown)");
        let subject = commit
            .get("subject")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("bob relations rename");
        println!("  committed {sha} {subject}");
        println!("  undo: git revert {sha}");
    }
    if report.mode == "write" && report.sync.post == "failed" {
        println!("  sync pending");
    } else if report.mode == "write" {
        println!("  renamed locally; sync {}", report.sync.post);
    }
    for warning in &report.warnings {
        eprintln!("{}: {warning}", styler.warning_prefix());
    }
    // Closing-label stay-open row: list affected open new sources.
    // The planner never changes statuses; this row is informational.
    // (Computed from the move set: every target becomes a new source.)
    let _ = report.moves.len();
}

fn print_failure(format: OutputFormat, failure: ReverseFailure) -> i32 {
    let exit = if failure.code == "usage" { 2 } else { 1 };
    match format {
        OutputFormat::Json => println!(
            "{}",
            json!({
                "schema_version": SCHEMA_VERSION,
                "ok": false,
                "code": failure.code,
                "message": failure.message,
            })
        ),
        OutputFormat::Human => {
            eprintln!("{COMMAND_NAME}: {}", failure.message);
            if let Some(hint) = failure.hint {
                eprintln!("hint: {hint}");
            }
        }
    }
    exit
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reverse_line_diff_records_inserts_and_removals() {
        let mut changes = Vec::new();
        record_line_diff(
            &mut changes,
            &["a".to_string(), "b".to_string()],
            &["a".to_string(), "c".to_string(), "b".to_string()],
            "links",
        );
        assert_eq!(changes.len(), 2);
    }

    #[test]
    fn relation_on_same_source_target_pair_supersedes_mention_mirror() {
        let relation_sources = BTreeSet::from([(
            PathBuf::from("source.md"),
            "source-task".to_string(),
        )]);

        assert!(!should_preserve_mention(
            Some(PathBuf::from("source.md")),
            "source-task",
            &relation_sources,
        ));
        assert!(should_preserve_mention(
            Some(PathBuf::from("other.md")),
            "source-task",
            &relation_sources,
        ));
        assert!(should_preserve_mention(
            None,
            "source-task",
            &relation_sources,
        ));
    }
}
