//! Report task block IDs removed by this host's local vault changes.
//!
//! This guard never edits task notes and never rejects a sync. Findings stay
//! pending for a short settle window so multi-step writers can finish before
//! vault-sync logs a confirmed loss.

use std::{
    collections::{BTreeMap, BTreeSet},
    ffi::OsStr,
    path::{Component, Path, PathBuf},
    process::Stdio,
    time::Duration,
};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use super::{
    collect_done, is_always_excluded_note_directory_name,
    ob::{self, ChildEnv},
    projects,
};

const SETTLE_WINDOW: Duration = Duration::from_secs(30);
const ID_PREFIX: &str = "^";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct PendingTaskLoss {
    pub id: String,
    pub path: String,
    pub kind: String,
    pub open: bool,
    pub task: String,
    pub base_commit: String,
    pub line: usize,
    pub first_seen: String,
    #[serde(default)]
    pub inbound_links: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct TaskLossRecord {
    pub id: String,
    pub path: String,
    pub kind: String,
    pub task: String,
    pub inbound_links: Vec<String>,
    pub base_commit: String,
    pub restore_hint: String,
    pub detected_at: String,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct Scan {
    pub pending: Vec<PendingTaskLoss>,
    pub confirmed: Vec<TaskLossRecord>,
    pub would_report: Vec<PendingTaskLoss>,
}

#[derive(Debug, Clone)]
struct TaskLine {
    id: String,
    open: bool,
    text: String,
    signature: String,
    line: usize,
}

#[derive(Debug, Clone)]
struct Candidate {
    id: String,
    path: String,
    kind: String,
    open: bool,
    task: String,
    base_commit: String,
    line: usize,
    inbound_links: Vec<String>,
}

pub(crate) fn scan(
    vault: &Path,
    child_env: &ChildEnv,
    old_pending: &[PendingTaskLoss],
    recent: &[TaskLossRecord],
    now: &str,
) -> Result<Scan, String> {
    let base_commit = sync_base(vault, child_env)?;
    let candidates =
        detect_candidates(vault, child_env, &base_commit, old_pending)?;
    let mut result = Scan::default();

    for candidate in candidates {
        if recent.iter().any(|record| {
            record.id == candidate.id
                && record.path == candidate.path
                && record.base_commit == candidate.base_commit
        }) {
            continue;
        }
        let previous = old_pending.iter().find(|pending| {
            pending.id == candidate.id
                && pending.path == candidate.path
                && pending.base_commit == candidate.base_commit
        });
        let first_seen = previous
            .map(|pending| pending.first_seen.clone())
            .unwrap_or_else(|| now.to_string());
        let pending = PendingTaskLoss {
            id: candidate.id.clone(),
            path: candidate.path.clone(),
            kind: candidate.kind.clone(),
            open: candidate.open,
            task: candidate.task.clone(),
            base_commit: candidate.base_commit.clone(),
            line: candidate.line,
            first_seen,
            inbound_links: candidate.inbound_links.clone(),
        };
        if settled(&pending.first_seen, now) {
            let hint = restore_hint(
                vault,
                &candidate.base_commit,
                &candidate.path,
                candidate.line,
            );
            result.confirmed.push(TaskLossRecord {
                id: candidate.id,
                path: candidate.path,
                kind: candidate.kind,
                task: candidate.task,
                inbound_links: candidate.inbound_links,
                base_commit: candidate.base_commit.clone(),
                restore_hint: hint,
                detected_at: now.to_string(),
            });
        } else {
            result.pending.push(pending.clone());
            result.would_report.push(pending);
        }
    }

    result.pending.sort_by(|left, right| {
        (&left.path, &left.id).cmp(&(&right.path, &right.id))
    });
    result.confirmed.sort_by(|left, right| {
        (&left.path, &left.id).cmp(&(&right.path, &right.id))
    });
    result.would_report.sort_by(|left, right| {
        (&left.path, &left.id).cmp(&(&right.path, &right.id))
    });
    Ok(result)
}

fn sync_base(vault: &Path, child_env: &ChildEnv) -> Result<String, String> {
    for revision in ["refs/remotes/origin/master", "HEAD"] {
        let output = ob::git_command(vault, child_env)
            .args(["rev-parse", "--verify", "--quiet", revision])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .map_err(|error| format!("failed to run git rev-parse: {error}"))?;
        if output.status.success() {
            return Ok(String::from_utf8_lossy(&output.stdout)
                .trim()
                .to_string());
        }
        if output.status.code() != Some(1) {
            return Err(format!(
                "git rev-parse {revision} failed: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            ));
        }
    }
    Err("could not resolve origin/master or HEAD".to_string())
}

fn detect_candidates(
    vault: &Path,
    child_env: &ChildEnv,
    base_commit: &str,
    old_pending: &[PendingTaskLoss],
) -> Result<Vec<Candidate>, String> {
    let changed = changed_markdown_paths(vault, child_env, base_commit)?;
    let mut raw = BTreeMap::<(String, String), (TaskLine, String)>::new();
    let mut current_signatures = BTreeSet::new();
    for path in changed {
        if !is_scan_path(&path) {
            continue;
        }
        let Some(base_text) = git_blob(vault, child_env, base_commit, &path)?
        else {
            continue;
        };
        let Ok(base_text) = String::from_utf8(base_text) else {
            continue;
        };
        let current_text =
            std::fs::read_to_string(vault.join(&path)).unwrap_or_default();
        let current_tasks = task_lines(&current_text);
        let current_ids = current_tasks
            .iter()
            .map(|task| task.id.clone())
            .collect::<BTreeSet<_>>();
        current_signatures.extend(
            current_tasks
                .into_iter()
                .map(|task| (task.id, task.signature)),
        );
        let display = display_path(&path);
        for task in task_lines(&base_text) {
            if !current_ids.contains(&task.id) {
                raw.entry((task.id.clone(), display.clone()))
                    .or_insert((task, base_commit.to_string()));
            }
        }
    }
    // Recheck prior candidates even after the first cycle committed and
    // pushed their removal. The current worktree is still the settle-window
    // authority; requiring the old ID to remain in today's diff would lose
    // the candidate as soon as origin/master advanced.
    for pending in old_pending {
        let path = (pending.id.clone(), pending.path.clone());
        raw.entry(path).or_insert_with(|| {
            (
                TaskLine {
                    id: pending.id.clone(),
                    open: pending.open,
                    text: pending.task.clone(),
                    signature: task_signature(&pending.task, &pending.id),
                    line: pending.line,
                },
                pending.base_commit.clone(),
            )
        });
    }
    if raw.is_empty() {
        return Ok(Vec::new());
    }

    let ids = raw
        .keys()
        .map(|(id, _)| id.clone())
        .collect::<BTreeSet<_>>();
    let search_terms = ids
        .iter()
        .map(|id| format!("{ID_PREFIX}{id}"))
        .chain(raw.values().map(|(task, _)| task.signature.clone()))
        .filter(|term| !term.is_empty())
        .collect::<BTreeSet<_>>();
    let mut current_task_ids = BTreeSet::new();
    let mut current_signatures = current_signatures;
    let mut inbound = BTreeMap::<String, BTreeSet<String>>::new();
    for (path, contents) in
        current_files_with_needles(vault, child_env, &search_terms)?
    {
        if !is_scan_path(&path) {
            continue;
        }
        let display = display_path(&path);
        for task in task_lines(&contents) {
            if ids.contains(&task.id) {
                current_task_ids.insert(task.id.clone());
            }
            current_signatures.insert((task.id, task.signature));
        }
        for id in task_links(&contents, &ids) {
            if ids.contains(&id) {
                inbound.entry(id).or_default().insert(display.clone());
            }
        }
    }

    let mut candidates = Vec::new();
    for ((id, path), (task, source_base)) in raw {
        // A task moved or archived elsewhere in the worktree is not lost.
        if current_task_ids.contains(&id) {
            continue;
        }
        let links = inbound
            .get(&id)
            .map(|paths| paths.iter().cloned().collect::<Vec<_>>())
            .unwrap_or_default();
        // A same-prose task whose ID changed, with no stale inbound links,
        // is an intentional hand rename rather than a vanished open task.
        if links.is_empty()
            && current_signatures.iter().any(|(other_id, signature)| {
                other_id != &id && signature == &task.signature
            })
        {
            continue;
        }
        let kind = if !links.is_empty() {
            "linked"
        } else if task.open {
            "unlinked"
        } else {
            continue;
        };
        candidates.push(Candidate {
            id,
            path,
            kind: kind.to_string(),
            open: task.open,
            task: task.text,
            base_commit: source_base,
            line: task.line,
            inbound_links: links,
        });
    }
    Ok(candidates)
}

fn changed_markdown_paths(
    vault: &Path,
    child_env: &ChildEnv,
    base_commit: &str,
) -> Result<Vec<PathBuf>, String> {
    let output = ob::git_command(vault, child_env)
        .args([
            "diff",
            "--no-renames",
            "--name-only",
            "-z",
            base_commit,
            "--",
            "*.md",
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .map_err(|error| format!("failed to run git diff: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "git diff against {base_commit} failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(output
        .stdout
        .split(|byte| *byte == 0)
        .filter(|path| !path.is_empty())
        .map(|path| PathBuf::from(String::from_utf8_lossy(path).into_owned()))
        .collect())
}

fn current_files_with_needles(
    vault: &Path,
    child_env: &ChildEnv,
    search_terms: &BTreeSet<String>,
) -> Result<Vec<(PathBuf, String)>, String> {
    let mut command = ob::git_command(vault, child_env);
    command.args(["grep", "-F", "-l", "-z", "-I"]);
    for term in search_terms {
        command.arg("-e").arg(term);
    }
    let output = command
        .arg("--")
        .arg("*.md")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .map_err(|error| format!("failed to run git grep: {error}"))?;
    match output.status.code() {
        Some(0) | Some(1) => {}
        _ => {
            return Err(format!(
                "git grep failed: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            ));
        }
    }
    let mut paths = output
        .stdout
        .split(|byte| *byte == 0)
        .filter(|path| !path.is_empty())
        .map(|path| PathBuf::from(String::from_utf8_lossy(path).into_owned()))
        .collect::<BTreeSet<_>>();

    // git grep intentionally searches tracked worktree files. Include the
    // usually small set of new Markdown files as destinations for moved tasks.
    let untracked = ob::git_command(vault, child_env)
        .args([
            "ls-files",
            "--others",
            "--exclude-standard",
            "-z",
            "--",
            "*.md",
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .map_err(|error| {
            format!("failed to list untracked Markdown: {error}")
        })?;
    if !untracked.status.success() {
        return Err(format!(
            "git ls-files for untracked Markdown failed: {}",
            String::from_utf8_lossy(&untracked.stderr).trim()
        ));
    }
    for path in untracked
        .stdout
        .split(|byte| *byte == 0)
        .filter(|path| !path.is_empty())
        .map(|path| PathBuf::from(String::from_utf8_lossy(path).into_owned()))
    {
        if !is_scan_path(&path) {
            continue;
        }
        let Ok(contents) = std::fs::read_to_string(vault.join(&path)) else {
            continue;
        };
        if search_terms.iter().any(|term| contents.contains(term)) {
            paths.insert(path);
        }
    }

    let mut files = Vec::new();
    for path in paths {
        if !is_scan_path(&path) {
            continue;
        }
        let Ok(contents) = std::fs::read_to_string(vault.join(&path)) else {
            continue;
        };
        files.push((path, contents));
    }
    Ok(files)
}

fn git_blob(
    vault: &Path,
    child_env: &ChildEnv,
    revision: &str,
    path: &Path,
) -> Result<Option<Vec<u8>>, String> {
    let spec = format!("{revision}:{}", display_path(path));
    let output = ob::git_command(vault, child_env)
        .args(["cat-file", "-p", &spec])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .map_err(|error| format!("failed to run git cat-file: {error}"))?;
    if output.status.success() {
        Ok(Some(output.stdout))
    } else if String::from_utf8_lossy(&output.stderr).contains("does not exist")
        || String::from_utf8_lossy(&output.stderr).contains("path '")
    {
        Ok(None)
    } else {
        Err(format!(
            "git cat-file {spec} failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ))
    }
}

fn task_lines(contents: &str) -> Vec<TaskLine> {
    let mut result = Vec::new();
    for (line_number, line) in visible_lines(contents) {
        let Some((status, _body)) = checkbox(line) else {
            continue;
        };
        let Some(id) = collect_done::trailing_block_id_in_line(line) else {
            continue;
        };
        let signature = task_signature(line.trim(), &id);
        result.push(TaskLine {
            id,
            open: matches!(status, ' ' | '/' | '*' | '?'),
            text: line.trim().to_string(),
            signature,
            line: line_number,
        });
    }
    result
}

fn task_signature(line: &str, id: &str) -> String {
    line.trim_end()
        .strip_suffix(&format!("^{id}"))
        .unwrap_or(line.trim_end())
        .trim_end()
        .to_string()
}

fn checkbox(line: &str) -> Option<(char, &str)> {
    let rest = line.trim_start();
    let marker_end = rest
        .char_indices()
        .next()
        .filter(|(_, ch)| matches!(ch, '-' | '*' | '+'))?
        .0
        + 1;
    let after_marker = rest.get(marker_end..)?;
    if !after_marker.starts_with(char::is_whitespace) {
        return None;
    }
    let body = after_marker.trim_start();
    let body = body.strip_prefix('[')?;
    let status = body.chars().next()?;
    let rest = body.get(status.len_utf8()..)?.strip_prefix(']')?;
    if !rest.is_empty() && !rest.starts_with(char::is_whitespace) {
        return None;
    }
    Some((status, rest.trim_start()))
}

fn visible_lines(contents: &str) -> Vec<(usize, &str)> {
    let mut visible = Vec::new();
    let mut in_frontmatter = false;
    let mut frontmatter_seen_content = false;
    let mut fence: Option<(char, usize)> = None;
    for (index, line) in contents.lines().enumerate() {
        let trimmed = line.trim_start();
        if !frontmatter_seen_content && trimmed == "---" {
            in_frontmatter = !in_frontmatter;
            frontmatter_seen_content = true;
            continue;
        }
        if in_frontmatter {
            if trimmed == "---" || trimmed == "..." {
                in_frontmatter = false;
            }
            continue;
        }
        frontmatter_seen_content = true;
        if let Some((fence_char, width)) = fence {
            let count =
                trimmed.chars().take_while(|ch| *ch == fence_char).count();
            if count >= width {
                fence = None;
            }
            continue;
        }
        let Some(fence_char @ ('`' | '~')) = trimmed.chars().next() else {
            visible.push((index + 1, line));
            continue;
        };
        let width = trimmed.chars().take_while(|ch| *ch == fence_char).count();
        if width >= 3 {
            fence = Some((fence_char, width));
            continue;
        }
        visible.push((index + 1, line));
    }
    visible
}

fn task_links(contents: &str, ids: &BTreeSet<String>) -> Vec<String> {
    let mut links = Vec::new();
    for (_, line) in visible_lines(contents) {
        let mut rest = line;
        while let Some(start) = rest.find("[[") {
            rest = &rest[start + 2..];
            let Some(end) = rest.find("]]") else {
                break;
            };
            let inside = &rest[..end];
            let target = inside.split('|').next().unwrap_or(inside);
            if let Some(anchor) = target.find("#^") {
                let id_start = anchor + 2;
                let id_len = target[id_start..]
                    .bytes()
                    .take_while(|byte| {
                        byte.is_ascii_alphanumeric() || *byte == b'-'
                    })
                    .count();
                if id_len > 0 {
                    let id = target[id_start..id_start + id_len].to_string();
                    if ids.contains(&id) {
                        links.push(id);
                    }
                }
            }
            rest = &rest[end + 2..];
        }
    }
    links
}

fn settled(first_seen: &str, now: &str) -> bool {
    let (Ok(first), Ok(now)) = (
        DateTime::parse_from_rfc3339(first_seen),
        DateTime::parse_from_rfc3339(now),
    ) else {
        return false;
    };
    now.with_timezone(&Utc)
        .signed_duration_since(first.with_timezone(&Utc))
        >= chrono::TimeDelta::from_std(SETTLE_WINDOW).unwrap_or_default()
}

fn restore_hint(vault: &Path, commit: &str, path: &str, line: usize) -> String {
    format!(
        "git -C {} show {}:{} | sed -n '{}p'",
        shell_quote(&vault.to_string_lossy()),
        commit,
        shell_quote(path),
        line
    )
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

fn is_scan_path(path: &Path) -> bool {
    if !projects::is_markdown_file(path) {
        return false;
    }
    path.components().all(|component| match component {
        Component::Normal(name) => {
            !is_always_excluded_note_directory_name(name)
                && name != OsStr::new("lib")
                && name != OsStr::new("_conflicts")
        }
        Component::CurDir | Component::ParentDir => true,
        Component::Prefix(_) | Component::RootDir => true,
    })
}

fn display_path(path: &Path) -> String {
    path.components()
        .map(|component| component.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_task_checkbox_states_and_trailing_ids() {
        let lines = task_lines(
            "- [ ] #task Open ^open\n- [/] #task Working ^working\n\
- [*] #task Next ^next\n- [?] #task Question ^question\n\
- [x] #task Closed ^closed\nordinary ^not-task\n",
        );
        assert_eq!(lines.len(), 5);
        assert!(lines[..4].iter().all(|line| line.open));
        assert!(!lines[4].open);
        assert_eq!(lines[0].line, 1);
    }

    #[test]
    fn ignores_task_looking_lines_in_frontmatter_and_fences() {
        let lines = task_lines(
            "---\nexample: '- [ ] #task frontmatter ^front'\n---\n\
```md\n- [ ] #task example ^example\n```\n\
- [ ] #task Real task ^real\n",
        );
        assert_eq!(
            lines
                .iter()
                .map(|line| line.id.as_str())
                .collect::<Vec<_>>(),
            ["real"]
        );
    }

    #[test]
    fn task_links_match_same_note_and_other_note_targets_exactly() {
        let ids = ["alpha".to_string(), "alpha-long".to_string()]
            .into_iter()
            .collect();
        let found = task_links(
            "[[#^alpha]] [[bob#^alpha-long|task]] [[bob#^alpha-extra]]",
            &ids,
        );
        assert_eq!(found.len(), 2);
        assert_eq!(found[0], "alpha");
        assert_eq!(found[1], "alpha-long");
    }

    #[test]
    fn changing_only_a_task_block_id_keeps_its_signature() {
        let old = task_lines("- [ ] #task Rename this task ^old-id\n");
        let new = task_lines("- [ ] #task Rename this task ^new-id\n");
        assert_eq!(old[0].signature, new[0].signature);
    }

    #[test]
    fn settle_window_requires_a_full_thirty_seconds() {
        assert!(!settled("2026-11-09T12:00:00Z", "2026-11-09T12:00:29Z"));
        assert!(settled("2026-11-09T12:00:00Z", "2026-11-09T12:00:30Z"));
    }

    #[test]
    fn loss_scan_paths_skip_internal_areas_but_include_done_archives() {
        assert!(!is_scan_path(Path::new("_conflicts/sync_conflicts.md")));
        assert!(!is_scan_path(Path::new("lib/generated.md")));
        assert!(!is_scan_path(Path::new(".bob/internal.md")));
        assert!(is_scan_path(Path::new("done/archive.md")));
        assert!(is_scan_path(Path::new("area/project.md")));
    }
}
