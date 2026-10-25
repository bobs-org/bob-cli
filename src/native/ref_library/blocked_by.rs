//! Blocked reasons for `bob ref` rows, read from the selected task line.
//!
//! Never runs reconcile. `local_dependency` resolves `[dependsOn::]` ids
//! against open Tasks-status carriers outside `done/`; `local_schedule`
//! uses `date > today`; `project` is the `#prj_is_blocked` tag proxy.
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use super::row::{BlockedPrerequisite, BlockedReason, RefRow};
use crate::native::capture_dependency_tasks::{
    has_prj_is_blocked_tag, vault_note_paths,
};
use crate::native::env as bob_env;
use crate::native::note_tasks::{self, NoteTaskSettings};
use crate::native::ref_tasks::{is_conflict_copy_name, RefTaskIndex};
use crate::native::task_dependencies::inline_code_spans;
use crate::native::task_fields::{self, parse_strict_calendar_date};

const PRIORITY_VALUES: [&str; 5] =
    ["highest", "high", "medium", "low", "lowest"];

/// Fill `priority`/`scheduled` on each task view and `blocked_by` on
/// every row. Dependency ids resolve only when a blocked row names some.
pub(super) fn apply_blocked_by(
    bob_dir: &Path,
    ref_tasks: &RefTaskIndex,
    rows: &mut [RefRow],
) {
    let today = bob_env::current_datetime().date();
    let mut wanted = BTreeSet::new();
    for row in rows.iter_mut() {
        let line = row.task_line.as_deref();
        if let Some(task) = row.task.as_mut() {
            task.priority = line.and_then(priority_value);
            task.scheduled = line.and_then(scheduled_value);
        }
        if !row.blocked {
            row.blocked_by.clear();
            continue;
        }
        let Some(line) = line else {
            row.blocked_by.clear();
            continue;
        };
        for id in depends_on_ids(line) {
            wanted.insert(id);
        }
        row.blocked_by = line_reasons_without_deps(line, row, today);
    }
    if wanted.is_empty() {
        return;
    }
    let settings = note_tasks::read_settings(bob_dir);
    let carriers = collect_open_id_carriers_wanted(bob_dir, &wanted, &settings);
    let titles: BTreeMap<String, String> = rows
        .iter()
        .map(|row| (row.path.clone(), row.title.clone()))
        .collect();
    for row in rows.iter_mut() {
        if !row.blocked {
            continue;
        }
        let Some(line) = row.task_line.as_deref() else {
            continue;
        };
        let ids = depends_on_ids(line);
        if ids.is_empty() {
            continue;
        }
        let prerequisites =
            prerequisites_for(&ids, &carriers, ref_tasks, &titles);
        if prerequisites.is_empty() {
            continue;
        }
        row.blocked_by
            .insert(0, BlockedReason::LocalDependency { prerequisites });
    }
}

/// Human suffix for a blocked list row, or `None` when the row is not blocked.
pub(super) fn human_reason_suffix(row: &RefRow) -> Option<String> {
    if !row.blocked {
        return None;
    }
    if row.blocked_by.is_empty() {
        return Some("⏸ blocked".to_string());
    }
    let parts = row.blocked_by.iter().map(format_reason).collect::<Vec<_>>();
    Some(parts.join(" · "))
}

fn format_reason(reason: &BlockedReason) -> String {
    match reason {
        BlockedReason::LocalDependency { prerequisites } => {
            let title = prerequisites
                .first()
                .map(|item| item.title.as_str())
                .unwrap_or("");
            if prerequisites.len() > 1 {
                format!("⛓ after {title} +{}", prerequisites.len() - 1)
            } else {
                format!("⛓ after {title}")
            }
        }
        BlockedReason::LocalSchedule { date } => {
            format!("💤 until {date}")
        }
        BlockedReason::Project { project, .. } => {
            let stem = Path::new(project)
                .file_stem()
                .and_then(|stem| stem.to_str())
                .unwrap_or(project);
            format!("🧱 {stem} blocked")
        }
    }
}

fn line_reasons_without_deps(
    line: &str,
    row: &RefRow,
    today: chrono::NaiveDate,
) -> Vec<BlockedReason> {
    let mut reasons = Vec::new();
    if let Some(date) = scheduled_value(line)
        && let Some(parsed) = parse_strict_calendar_date(&date)
        && parsed > today
    {
        reasons.push(BlockedReason::LocalSchedule { date });
    }
    if has_project_block(line) {
        let project = row
            .task
            .as_ref()
            .map(|task| task.path.clone())
            .unwrap_or_else(|| row.path.clone());
        let stem = project
            .strip_suffix(".md")
            .or_else(|| project.strip_suffix(".MD"))
            .unwrap_or(&project)
            .to_string();
        reasons.push(BlockedReason::Project {
            project,
            link: format!("[[{stem}]]"),
        });
    }
    reasons
}

fn priority_value(line: &str) -> Option<String> {
    fields_outside_code(line, "priority")
        .into_iter()
        .find(|value| PRIORITY_VALUES.contains(&value.as_str()))
}

fn scheduled_value(line: &str) -> Option<String> {
    fields_outside_code(line, "scheduled")
        .into_iter()
        .find_map(|value| parse_strict_calendar_date(&value).map(|_| value))
}

#[cfg(test)]
pub(super) fn depends_on_ids_for_test(line: &str) -> Vec<String> {
    depends_on_ids(line)
}

fn depends_on_ids(line: &str) -> Vec<String> {
    let mut ids = Vec::new();
    for value in fields_outside_code(line, "dependsOn") {
        for part in value.split(',') {
            let id = part.trim_matches(' ');
            if valid_task_identity(id) {
                ids.push(id.to_string());
            }
        }
    }
    ids
}

fn has_project_block(line: &str) -> bool {
    has_prj_is_blocked_tag(&mask_inline_code(line))
}

fn fields_outside_code(line: &str, key: &str) -> Vec<String> {
    let code = inline_code_spans(line);
    task_fields::inline_fields(line, key)
        .into_iter()
        .filter(|field| {
            !code
                .iter()
                .any(|span| field.start < span.end && field.end > span.start)
        })
        .map(|field| field.value)
        .collect()
}

fn mask_inline_code(line: &str) -> String {
    let mut masked = line.to_string();
    for span in inline_code_spans(line).into_iter().rev() {
        let width = span.end.saturating_sub(span.start);
        masked.replace_range(span, &" ".repeat(width));
    }
    masked
}

fn valid_task_identity(value: &str) -> bool {
    !value.is_empty()
        && value.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-')
        })
}

#[derive(Debug, Clone)]
struct IdCarrier {
    path: String,
    block_id: Option<String>,
    mark: char,
    title: String,
    line_index: usize,
    is_ref_task: bool,
}

fn prerequisites_for(
    ids: &[String],
    carriers: &BTreeMap<String, IdCarrier>,
    ref_tasks: &RefTaskIndex,
    titles: &BTreeMap<String, String>,
) -> Vec<BlockedPrerequisite> {
    let mut out = Vec::new();
    let mut seen = BTreeSet::new();
    for id in ids {
        if !seen.insert(id) {
            continue;
        }
        let Some(carrier) = carriers.get(id) else {
            continue;
        };
        let ref_path = ref_tasks
            .ref_note_for_task(&carrier.path, carrier.line_index)
            .map(str::to_string);
        let title = ref_path
            .as_deref()
            .and_then(|path| titles.get(path).cloned())
            .unwrap_or_else(|| carrier.title.clone());
        let stem = carrier
            .path
            .strip_suffix(".md")
            .or_else(|| carrier.path.strip_suffix(".MD"))
            .unwrap_or(&carrier.path);
        let link = match &carrier.block_id {
            Some(block_id) => format!("[[{stem}#^{block_id}]]"),
            None => format!("[[{stem}]]"),
        };
        out.push(BlockedPrerequisite {
            id: id.clone(),
            title,
            path: carrier.path.clone(),
            block_id: carrier.block_id.clone(),
            link,
            mark: carrier.mark,
            ref_path,
        });
    }
    out
}

/// Extra prefiltered pass: read only notes that may carry a wanted `[id::]`.
fn collect_open_id_carriers_wanted(
    bob_dir: &Path,
    wanted: &BTreeSet<String>,
    settings: &NoteTaskSettings,
) -> BTreeMap<String, IdCarrier> {
    collect_open_id_carriers(bob_dir, Some(wanted), settings)
}

/// Additive-style pass: read every note that carries any `id::` field.
#[cfg(test)]
fn collect_open_id_carriers_all(
    bob_dir: &Path,
    settings: &NoteTaskSettings,
) -> BTreeMap<String, IdCarrier> {
    collect_open_id_carriers(bob_dir, None, settings)
}

fn collect_open_id_carriers(
    bob_dir: &Path,
    wanted: Option<&BTreeSet<String>>,
    settings: &NoteTaskSettings,
) -> BTreeMap<String, IdCarrier> {
    let (paths, _) = vault_note_paths(bob_dir);
    let pairs: Vec<(PathBuf, PathBuf)> = paths
        .into_iter()
        .filter(|rel| keep_id_note(rel))
        .map(|rel| (bob_dir.join(&rel), rel))
        .collect();
    let contents = read_id_notes_parallel(&pairs, wanted);
    let mut best: BTreeMap<String, IdCarrier> = BTreeMap::new();
    for (rel, text) in contents {
        let forward = rel.to_string_lossy().replace('\\', "/");
        let scan = note_tasks::scan(&text, settings);
        let lines: Vec<&str> = text.lines().collect();
        for task in scan.tasks() {
            if !settings.status_types.contains_key(&task.status_symbol)
                || !task.status_type.is_open()
            {
                continue;
            }
            let raw = lines.get(task.line_index).copied().unwrap_or_default();
            let Some(id) = task_id_value(raw) else {
                continue;
            };
            if wanted.is_some_and(|set| !set.contains(&id)) {
                continue;
            }
            let carrier = IdCarrier {
                path: forward.clone(),
                block_id: task.block_id.clone(),
                mark: task.status_symbol,
                title: task.description.clone(),
                line_index: task.line_index,
                is_ref_task: task.task_kind == Some("ref"),
            };
            best.entry(id)
                .and_modify(|existing| {
                    if prefer_carrier(&carrier, existing) {
                        *existing = carrier.clone();
                    }
                })
                .or_insert(carrier);
        }
    }
    best
}

fn keep_id_note(rel: &Path) -> bool {
    let forward = rel.to_string_lossy().replace('\\', "/");
    let first = forward.split('/').next().unwrap_or("");
    if first == "lib" || first == "xlib" || first == "done" {
        return false;
    }
    if let Some(name) = rel.file_name().and_then(|name| name.to_str())
        && is_conflict_copy_name(name)
    {
        return false;
    }
    true
}

fn task_id_value(line: &str) -> Option<String> {
    fields_outside_code(line, "id")
        .into_iter()
        .rev()
        .find(|value| valid_task_identity(value))
}

fn prefer_carrier(new: &IdCarrier, old: &IdCarrier) -> bool {
    match (new.is_ref_task, old.is_ref_task) {
        (true, false) => true,
        (false, true) => false,
        _ => (&new.path, new.line_index) < (&old.path, old.line_index),
    }
}

fn read_id_notes_parallel(
    pairs: &[(PathBuf, PathBuf)],
    wanted: Option<&BTreeSet<String>>,
) -> Vec<(PathBuf, String)> {
    if pairs.is_empty() {
        return Vec::new();
    }
    let worker_count = std::thread::available_parallelism()
        .map(|count| count.get())
        .unwrap_or(4)
        .min(pairs.len())
        .max(1);
    if worker_count <= 1 {
        return pairs
            .iter()
            .filter_map(|pair| read_id_note(pair, wanted))
            .collect();
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
                        if let Some(entry) = read_id_note(pair, wanted) {
                            local.push(entry);
                        }
                    }
                    local
                })
            })
            .collect();
        handles
            .into_iter()
            .map(|handle| handle.join().expect("id-carrier worker panicked"))
            .collect()
    });
    collected.into_iter().flatten().collect()
}

fn read_id_note(
    pair: &(PathBuf, PathBuf),
    wanted: Option<&BTreeSet<String>>,
) -> Option<(PathBuf, String)> {
    let bytes = std::fs::read(&pair.0).ok()?;
    if !file_may_carry_id(&bytes, wanted) {
        return None;
    }
    let text = String::from_utf8(bytes).ok()?;
    Some((pair.1.clone(), text))
}

fn file_may_carry_id(bytes: &[u8], wanted: Option<&BTreeSet<String>>) -> bool {
    if memchr::memmem::find(bytes, b"id::").is_none() {
        return false;
    }
    let Some(ids) = wanted else {
        return true;
    };
    ids.iter()
        .any(|id| memchr::memmem::find(bytes, id.as_bytes()).is_some())
}

/// Time both id-resolution strategies on `bob_dir` for the wanted set.
#[cfg(test)]
pub(super) fn measure_id_resolution(
    bob_dir: &Path,
    wanted: &BTreeSet<String>,
) -> (std::time::Duration, std::time::Duration, usize, usize) {
    let settings = note_tasks::read_settings(bob_dir);
    let start = std::time::Instant::now();
    let extra = collect_open_id_carriers_wanted(bob_dir, wanted, &settings);
    let extra_time = start.elapsed();
    let start = std::time::Instant::now();
    let additive = collect_open_id_carriers_all(bob_dir, &settings);
    let additive_time = start.elapsed();
    (extra_time, additive_time, extra.len(), additive.len())
}

#[cfg(test)]
mod tests {
    use super::super::row::{RefIdentity, RefTaskView};
    use super::*;
    use chrono::NaiveDate;

    #[test]
    fn priority_reads_known_values_and_ignores_code() {
        assert_eq!(
            priority_value("- [ ] A [priority:: high] #task"),
            Some("high".to_string())
        );
        assert_eq!(priority_value("- [ ] A [priority:: bogus] #task"), None);
        assert_eq!(priority_value("- [ ] A `[priority:: high]` #task"), None);
        assert_eq!(
            priority_value(
                "- [ ] A `[priority:: high]` [priority:: low] #task"
            ),
            Some("low".to_string())
        );
        for value in PRIORITY_VALUES {
            let line = format!("- [ ] A [priority:: {value}] #task");
            assert_eq!(priority_value(&line).as_deref(), Some(value));
        }
    }

    #[test]
    fn scheduled_requires_a_strict_date_outside_code() {
        assert_eq!(
            scheduled_value("- [?] A [scheduled:: 2026-10-23] #task"),
            Some("2026-10-23".to_string())
        );
        assert_eq!(
            scheduled_value("- [?] A [scheduled:: tomorrow] #task"),
            None
        );
        assert_eq!(
            scheduled_value("- [?] A `[scheduled:: 2026-10-23]` #task"),
            None
        );
    }

    #[test]
    fn depends_on_ids_skip_code_and_keep_order() {
        let line =
            "- [?] A [dependsOn:: first, second] `[dependsOn:: coded]` #task";
        assert_eq!(
            depends_on_ids(line),
            vec!["first".to_string(), "second".to_string()]
        );
        assert!(depends_on_ids("- [?] A `[dependsOn:: only]` #task").is_empty());
    }

    #[test]
    fn project_tag_is_exact_and_ignores_code() {
        assert!(has_project_block("- [?] A #prj_is_blocked #task ^ref-a"));
        assert!(!has_project_block("- [?] A #prj_is_blocked_extra #task"));
        assert!(!has_project_block("- [?] A `#prj_is_blocked` #task ^ref-a"));
        assert!(!has_project_block(
            "- [?] A `keep #prj_is_blocked hidden` #task"
        ));
    }

    #[test]
    fn future_schedule_is_a_reason_today_is_not() {
        let today = NaiveDate::from_ymd_opt(2026, 10, 6).expect("date");
        let row = sample_row(true);
        let future = line_reasons_without_deps(
            "- [?] A [scheduled:: 2026-10-23] #task",
            &row,
            today,
        );
        assert_eq!(
            future,
            vec![BlockedReason::LocalSchedule {
                date: "2026-10-23".to_string()
            }]
        );
        let today_line = line_reasons_without_deps(
            "- [?] A [scheduled:: 2026-10-06] #task",
            &row,
            today,
        );
        assert!(today_line.is_empty());
    }

    #[test]
    fn human_suffix_joins_reasons_in_order() {
        let mut row = sample_row(true);
        row.blocked_by = vec![
            BlockedReason::LocalDependency {
                prerequisites: vec![
                    sample_prereq("a", "First"),
                    sample_prereq("b", "Second"),
                ],
            },
            BlockedReason::LocalSchedule {
                date: "2026-10-23".to_string(),
            },
            BlockedReason::Project {
                project: "sase_post_blog.md".to_string(),
                link: "[[sase_post_blog]]".to_string(),
            },
        ];
        assert_eq!(
            human_reason_suffix(&row).as_deref(),
            Some("⛓ after First +1 · 💤 until 2026-10-23 · 🧱 sase_post_blog blocked")
        );
        row.blocked_by.clear();
        assert_eq!(human_reason_suffix(&row).as_deref(), Some("⏸ blocked"));
        row.blocked = false;
        assert_eq!(human_reason_suffix(&row), None);
    }

    fn sample_row(blocked: bool) -> RefRow {
        RefRow {
            path: "ref/papers/a.md".to_string(),
            link: "[[ref/papers/a]]".to_string(),
            title: "A".to_string(),
            marker_id: None,
            origin: "external".to_string(),
            ref_type: Some("papers".to_string()),
            era: "modern".to_string(),
            status: Some("ready".to_string()),
            status_sync: "ok".to_string(),
            frontmatter_status: None,
            legacy_status: None,
            blocked,
            blocked_by: Vec::new(),
            reading_state: "queued".to_string(),
            reading_state_source: "ref_task:[?]".to_string(),
            parent: Some("sase".to_string()),
            task: Some(RefTaskView {
                path: "sase.md".to_string(),
                block_id: Some("ref-a".to_string()),
                link: "[[sase#^ref-a]]".to_string(),
                mark: '?',
                archived: false,
                priority: None,
                scheduled: None,
            }),
            urls: Vec::new(),
            earlier: Vec::new(),
            reading: None,
            identity: RefIdentity {
                keys: Vec::new(),
                arxiv: None,
                doi: None,
            },
            author: None,
            published: None,
            captured: None,
            added: None,
            added_source: None,
            added_at: None,
            finished: None,
            finished_source: None,
            source_pdf: None,
            audio: None,
            annotation_count: 0,
            comment_count: 0,
            snapshot: None,
            research_ref: None,
            superseded_by: None,
            diagnostics: Vec::new(),
            family: "ref/papers/a.md".to_string(),
            has_later: false,
            id: None,
            source_block: None,
            source_path: None,
            source_id: None,
            source_blocks: Vec::new(),
            v2: true,
            task_line: None,
        }
    }

    fn sample_prereq(id: &str, title: &str) -> BlockedPrerequisite {
        BlockedPrerequisite {
            id: id.to_string(),
            title: title.to_string(),
            path: "sase.md".to_string(),
            block_id: Some(id.to_string()),
            link: format!("[[sase#^{id}]]"),
            mark: ' ',
            ref_path: None,
        }
    }
}
