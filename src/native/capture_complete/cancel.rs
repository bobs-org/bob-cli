//! Cancel-label cue for `capture-complete` relation labels
//! (`docs/task-relations.md` §17 CL6–CL7).
//!
//! Rows gain `cancels_source` / `configured`, and the response gains
//! `relation_cancel` when at least one cancel label is configured. The
//! source resolves only from the token under the cursor — the item
//! owner for `~`, the `~~` token's task for `~~` — with one note file
//! read per side and no vault scan.

use std::path::Path;

use super::model::{RelationCancel, RelationCancelSource};
use crate::native::{
    capture, capture_language, config,
    note_tasks::{self, NoteTask},
    relation_labels::relation_label_key,
    task_cancel::verdicts::{cancel_verdict, CancelSourceVerdict},
    task_relations::label as relation_label,
};

/// Best-effort cancel-label policy: a broken config means fewer
/// cancels, never a failed completion. Warnings surface through the
/// response instead.
pub(super) fn load_policy() -> config::RelationsConfig {
    match config::load_relations_config(&config::config_path()) {
        Ok(loaded) => loaded,
        Err(error) => config::RelationsConfig {
            linked_from_lines: true,
            cancel_labels: Vec::new(),
            warnings: vec![format!(
                "invalid relations config: {error}; cancelling nothing"
            )],
        },
    }
}

/// The cancel consequence for the `relation_label` token whose label
/// separator sits at `separator`, plus the disabled reason to stamp on
/// every `cancels_source` row when the source cannot be cancelled
/// through this gesture (`recurring` / `new_task`).
pub(super) struct CancelPlan {
    pub(super) response: RelationCancel,
    pub(super) row_disable: Option<String>,
}

pub(super) fn plan_relation_cancel(
    bob_dir: &Path,
    raw_text: &str,
    separator: usize,
    editor_item: Option<&capture_language::EditorItemParse>,
    query: &str,
    policy: &config::RelationsConfig,
) -> Option<CancelPlan> {
    if policy.cancel_labels.is_empty() {
        return None;
    }
    let (_, direction, _) =
        capture_language::relation_label_token_at(raw_text, separator)?;
    let settings = note_tasks::read_settings(bob_dir);
    let source =
        resolve_source(bob_dir, raw_text, separator, direction, editor_item)?;
    let raw_line = source.raw_line.clone();
    let verdict = cancel_verdict(&raw_line, &settings, source.is_new);
    let target_text = resolve_target_text(
        bob_dir,
        raw_text,
        separator,
        direction,
        editor_item,
    );
    let (verdict_str, message) = match verdict {
        CancelSourceVerdict::Cancels { previous_status, project } => {
            let mut message = format!(
                "⊘ Cancels “{}” · {} → Cancelled",
                source.text,
                status_display(&source, previous_status)
            );
            if project {
                let open = source.project_open_tasks.unwrap_or(0);
                message.push_str(&format!(
                    " · its {open} open {} stay open",
                    if open == 1 { "task" } else { "tasks" }
                ));
            }
            ("cancels", message)
        }
        CancelSourceVerdict::AlreadyClosed { .. } => {
            // The `-` symbol is Cancelled in every vault; every other
            // closed symbol keeps its Done wording.
            let message = if source.status_symbol == '-' {
                "Already Cancelled · link only".to_string()
            } else {
                "Stays Done · link only".to_string()
            };
            ("already_closed", message)
        }
        CancelSourceVerdict::Recurring => (
            "recurring",
            format!(
                "“{}” repeats — cancel it with Obsidian Tasks first",
                source.text
            ),
        ),
        CancelSourceVerdict::NewTask => (
            "new_task",
            format!(
                "“{}” is new — it would be born cancelled · to retire “{}”, use ~~",
                source.text,
                target_text.as_deref().unwrap_or("the other task")
            ),
        ),
    };
    let hint = inverse_hint(query, target_text.as_deref(), policy);
    let row_disable = match verdict {
        CancelSourceVerdict::Recurring | CancelSourceVerdict::NewTask => {
            Some(message.clone())
        }
        _ => None,
    };
    Some(CancelPlan {
        response: RelationCancel {
            source: RelationCancelSource {
                path: source.path,
                block_id: source.block_id,
                text: source.text,
                status_symbol: source.status_symbol,
                status_name: source.status_name,
                project_open_tasks: source.project_open_tasks,
            },
            verdict: verdict_str,
            message,
            hint,
        },
        row_disable,
    })
}

/// `“{typed}” never cancels · to retire “{T}”, use ~~ with “{cancel}”`
/// when the typed label is the inverse of a configured cancel label.
fn inverse_hint(
    query: &str,
    target_text: Option<&str>,
    policy: &config::RelationsConfig,
) -> Option<String> {
    let typed = relation_label::normalize_relation_label(query).ok()?;
    let key = relation_label_key(&typed);
    let cancel = policy.cancel_label_for_inverse(&key)?;
    Some(format!(
        "“{typed}” never cancels · to retire “{}”, use ~~ with “{cancel}”",
        target_text.unwrap_or("the other task")
    ))
}

fn status_display(source: &ResolvedSource, previous: char) -> String {
    if source.status_symbol == previous {
        source.status_name.clone()
    } else {
        previous.to_string()
    }
}

struct ResolvedSource {
    path: String,
    block_id: Option<String>,
    text: String,
    status_symbol: char,
    status_name: String,
    project_open_tasks: Option<usize>,
    raw_line: String,
    is_new: bool,
}

/// The `~` / `~~` token behind the label separator: its direction
/// plus the decoded target note and block id. The completion field
/// already classified complete and partial labels alike, so this
/// parses the token slice itself instead of relying on the item's
/// relation rows (partial labels have none).
fn token_target(
    raw_text: &str,
    separator: usize,
) -> Option<(capture_language::RelationDirection, String, String)> {
    let (start, direction, _) =
        capture_language::relation_label_token_at(raw_text, separator)?;
    let token = raw_text.get(start..separator)?;
    let rest = token
        .strip_prefix("~~")
        .or_else(|| token.strip_prefix('~'))?;
    let (note, block) = rest.rsplit_once(':')?;
    let note = note.trim();
    let block = block.trim();
    if note.is_empty() || block.is_empty() {
        return None;
    }
    if !block
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
    {
        return None;
    }
    Some((direction, unquote_note(note), block.to_string()))
}

/// Strip one pair of surrounding double quotes from a typed note
/// component, mirroring the relation scanner's stored identity.
fn unquote_note(note: &str) -> String {
    let quoted =
        note.len() >= 2 && note.starts_with('"') && note.ends_with('"');
    if quoted {
        note[1..note.len() - 1].to_string()
    } else {
        note.to_string()
    }
}

/// Resolve only the token's source: the item owner for `~`, the `~~`
/// token's task for `~~`. One note file read, never a vault scan. An
/// unresolvable source omits `relation_cancel` instead of guessing.
fn resolve_source(
    bob_dir: &Path,
    raw_text: &str,
    separator: usize,
    direction: capture_language::RelationDirection,
    editor_item: Option<&capture_language::EditorItemParse>,
) -> Option<ResolvedSource> {
    let item = editor_item?;
    match direction {
        capture_language::RelationDirection::To => {
            let owner = item.dependency_target.clone()?;
            match owner.kind {
                capture_language::DependencyTargetKind::NewTask => {
                    let route = owner
                        .route
                        .clone()
                        .unwrap_or_else(|| capture::inbox_route().to_string());
                    Some(ResolvedSource {
                        path: capture::route_label(&route),
                        block_id: None,
                        text: item.body.clone(),
                        status_symbol: ' ',
                        status_name: status_name_for(bob_dir, ' '),
                        project_open_tasks: None,
                        raw_line: String::new(),
                        is_new: true,
                    })
                }
                capture_language::DependencyTargetKind::ExistingTask => {
                    let route = owner.route?;
                    let block_id = owner.block_id?;
                    read_task_source(bob_dir, &route, &block_id, false)
                }
            }
        }
        capture_language::RelationDirection::From => {
            let (_, note, block) = token_target(raw_text, separator)?;
            read_task_source(bob_dir, &note, &block, false)
        }
    }
}

/// Resolve the other end's display text for hints: the `~` token's
/// target for `~`, the item owner for `~~`. Best effort; callers fall
/// back to a generic phrase.
fn resolve_target_text(
    bob_dir: &Path,
    raw_text: &str,
    separator: usize,
    direction: capture_language::RelationDirection,
    editor_item: Option<&capture_language::EditorItemParse>,
) -> Option<String> {
    let item = editor_item?;
    match direction {
        capture_language::RelationDirection::To => {
            let (_, note, block) = token_target(raw_text, separator)?;
            read_task_text(bob_dir, &note, &block)
        }
        capture_language::RelationDirection::From => {
            match item.dependency_target.clone()?.kind {
                capture_language::DependencyTargetKind::NewTask => {
                    Some(item.body.clone()).filter(|body| !body.is_empty())
                }
                capture_language::DependencyTargetKind::ExistingTask => {
                    let owner = item.dependency_target.clone()?;
                    read_task_text(bob_dir, &owner.route?, &owner.block_id?)
                }
            }
        }
    }
}

fn read_task_source(
    bob_dir: &Path,
    route_or_note: &str,
    block_id: &str,
    is_new: bool,
) -> Option<ResolvedSource> {
    let relative = note_relative(route_or_note)?;
    let contents = std::fs::read_to_string(bob_dir.join(&relative)).ok()?;
    let settings = note_tasks::read_settings(bob_dir);
    let scan = note_tasks::scan(&contents, &settings);
    let task = task_by_block_id(&scan, block_id)?;
    let raw_line = contents
        .lines()
        .nth(task.line_index)
        .unwrap_or_default()
        .to_string();
    let project = is_project_source(&raw_line, block_id);
    let project_open_tasks =
        project.then(|| scan.open_tasks().count().saturating_sub(1));
    Some(ResolvedSource {
        path: display_relative(&relative),
        block_id: Some(block_id.to_string()),
        text: task.description.clone(),
        status_symbol: task.status_symbol,
        status_name: task.status_name.clone(),
        project_open_tasks,
        raw_line,
        is_new,
    })
}

fn read_task_text(
    bob_dir: &Path,
    route_or_note: &str,
    block_id: &str,
) -> Option<String> {
    let relative = note_relative(route_or_note)?;
    let contents = std::fs::read_to_string(bob_dir.join(&relative)).ok()?;
    let settings = note_tasks::read_settings(bob_dir);
    let scan = note_tasks::scan(&contents, &settings);
    task_by_block_id(&scan, block_id).map(|task| task.description.clone())
}

/// Accept a capture route (`cash`) or a vault-relative note identity
/// (`cash`, `cash.md`) and return the vault-relative path. Only exact
/// matches resolve here; ambiguity omits `relation_cancel`.
fn note_relative(route_or_note: &str) -> Option<std::path::PathBuf> {
    let trimmed = route_or_note.trim().trim_end_matches(".md");
    if trimmed.is_empty()
        || trimmed.contains('\\')
        || trimmed.contains("..")
        || trimmed.starts_with('/')
    {
        return None;
    }
    Some(std::path::PathBuf::from(format!("{trimmed}.md")))
}

fn display_relative(relative: &Path) -> String {
    relative.to_string_lossy().replace('\\', "/")
}

fn task_by_block_id<'a>(
    scan: &'a note_tasks::NoteTaskScan,
    block_id: &str,
) -> Option<&'a NoteTask> {
    match scan.by_block_id(block_id) {
        note_tasks::BlockIdLookup::Found(task) => Some(task),
        _ => None,
    }
}

fn is_project_source(raw_line: &str, block_id: &str) -> bool {
    block_id == "prj" && raw_line.contains("^prj")
}

fn status_name_for(bob_dir: &Path, symbol: char) -> String {
    let settings = note_tasks::read_settings(bob_dir);
    settings
        .status_definitions
        .iter()
        .find(|definition| {
            let mut characters = definition.symbol.chars();
            characters.next() == Some(symbol) && characters.next().is_none()
        })
        .map(|definition| definition.name.clone())
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| {
            if symbol == ' ' {
                "Ready".to_string()
            } else {
                "Unknown".to_string()
            }
        })
}
