//! Detect displaced-plus-foreign-identity rewrites of tracked notes.
//!
//! Vault-sync calls this after `git status` and before staging so a whole-note
//! clobber (the review-walk save-to-the-wrong-file failure) is quarantined
//! before it reaches other hosts.

use std::{
    collections::BTreeSet,
    path::{Component, Path, PathBuf},
    process::Stdio,
};

use super::{
    collect_done, is_always_excluded_note_directory_name,
    ob::{self, ChildEnv},
    projects, task_status_hooks,
};

const MIN_HEAD_DISTINCT_LINES: usize = 5;
const MAX_BLOCK_ID_GREP_NEEDLES: usize = 5;

#[derive(Debug, Clone)]
pub(crate) struct ClobberHit {
    pub path: PathBuf,
    pub matched_note: PathBuf,
    pub evidence: String,
    pub head_bytes: Vec<u8>,
    pub work_bytes: Vec<u8>,
}

pub(crate) fn is_clobber_candidate(code: &str, path: &Path) -> bool {
    is_modified_tracked(code) && is_clobber_scan_path(path)
}

pub(crate) fn detect_clobbers(
    vault: &Path,
    child_env: &ChildEnv,
    candidates: &[PathBuf],
) -> Result<Vec<ClobberHit>, String> {
    let mut hits = Vec::new();
    for path in candidates {
        if let Some(hit) = inspect_candidate(vault, child_env, path)? {
            hits.push(hit);
        }
    }
    hits.sort_by(|left, right| left.path.cmp(&right.path));
    Ok(hits)
}

fn is_modified_tracked(code: &str) -> bool {
    matches!(code, " M" | "M " | "MM")
}

pub(crate) fn is_clobber_scan_path(path: &Path) -> bool {
    if !projects::is_markdown_file(path) || is_canonical_daily_note(path) {
        return false;
    }
    path.components().all(|component| match component {
        Component::Normal(name) => {
            !is_always_excluded_note_directory_name(name)
                && name.to_str() != Some("done")
                && !name.to_str().is_some_and(|text| text.starts_with('.'))
        }
        Component::CurDir | Component::ParentDir => true,
        Component::Prefix(_) | Component::RootDir => true,
    })
}

fn is_canonical_daily_note(path: &Path) -> bool {
    if task_status_hooks::canonical_daily_date(path).is_some() {
        return true;
    }
    let mut components = path.components();
    let Some(Component::Normal(year)) = components.next() else {
        return false;
    };
    let Some(Component::Normal(file)) = components.next() else {
        return false;
    };
    if components.next().is_some() {
        return false;
    }
    let Some(year) = year.to_str() else {
        return false;
    };
    let Some(file) = file.to_str() else {
        return false;
    };
    year.len() == 4
        && year.bytes().all(|byte| byte.is_ascii_digit())
        && file.len() >= 11
        && file.ends_with(".md")
        && file.as_bytes()[..8]
            .iter()
            .all(|byte| byte.is_ascii_digit())
        && file.starts_with(year)
}

fn inspect_candidate(
    vault: &Path,
    child_env: &ChildEnv,
    path: &Path,
) -> Result<Option<ClobberHit>, String> {
    let head_bytes = git_head_blob(vault, child_env, path)?;
    let work_path = vault.join(path);
    let work_bytes = std::fs::read(&work_path).map_err(|error| {
        format!("failed to read {}: {error}", display_path(path))
    })?;
    let Ok(head_text) = std::str::from_utf8(&head_bytes) else {
        return Ok(None);
    };
    let Ok(work_text) = std::str::from_utf8(&work_bytes) else {
        return Ok(None);
    };
    if !is_displaced(head_text, work_text) {
        return Ok(None);
    }

    let Some(matched) =
        find_matching_note(vault, child_env, path, head_text, work_text)?
    else {
        return Ok(None);
    };

    Ok(Some(ClobberHit {
        path: path.to_path_buf(),
        matched_note: matched.path,
        evidence: matched.evidence,
        head_bytes,
        work_bytes,
    }))
}

#[derive(Debug, Clone)]
struct MatchedNote {
    path: PathBuf,
    evidence: String,
}

fn find_matching_note(
    vault: &Path,
    child_env: &ChildEnv,
    path: &Path,
    head_text: &str,
    work_text: &str,
) -> Result<Option<MatchedNote>, String> {
    let mut candidates = BTreeSet::new();
    for needle in grep_needles(head_text, work_text) {
        for found in git_grep_files(vault, child_env, &needle)? {
            if found != path && is_clobber_scan_path(&found) {
                candidates.insert(found);
            }
        }
    }

    let mut best: Option<(u8, PathBuf, String)> = None;
    for candidate in candidates {
        let other_path = vault.join(&candidate);
        let Ok(other_text) = std::fs::read_to_string(&other_path) else {
            continue;
        };
        let Some(identity) = identity_match(head_text, work_text, &other_text)
        else {
            continue;
        };
        let created = created_value(work_text);
        let evidence =
            identity.evidence(created.as_deref(), &display_path(&candidate));
        let score = identity.score();
        let take = match &best {
            None => true,
            Some((best_score, best_path, _)) => {
                score > *best_score
                    || (score == *best_score && candidate < *best_path)
            }
        };
        if take {
            best = Some((score, candidate, evidence));
        }
    }

    Ok(best.map(|(_, path, evidence)| MatchedNote { path, evidence }))
}

fn grep_needles(head_text: &str, work_text: &str) -> Vec<String> {
    let mut needles = Vec::new();
    let head_created = created_value(head_text);
    let work_created = created_value(work_text);
    if work_created != head_created
        && let Some(value) = work_created
    {
        needles.push(value);
    }

    let head_prj = prj_task_line(head_text);
    if let Some(work_prj) = prj_task_line(work_text)
        && Some(work_prj) != head_prj
    {
        needles.push(work_prj.to_string());
    }

    let new_ids = new_task_block_ids(head_text, work_text);
    if new_ids.len() >= 3 {
        for id in new_ids.iter().take(MAX_BLOCK_ID_GREP_NEEDLES) {
            needles.push(format!("^{id}"));
        }
    }

    needles.retain(|needle| !needle.is_empty());
    needles
}

fn is_displaced(head_text: &str, work_text: &str) -> bool {
    let head_lines = distinct_nonblank_lines(head_text);
    if head_lines.len() < MIN_HEAD_DISTINCT_LINES {
        return false;
    }
    let work_lines = distinct_nonblank_lines(work_text);
    let absent = head_lines
        .iter()
        .filter(|line| !work_lines.contains(line.as_str()))
        .count();
    absent * 2 >= head_lines.len()
}

fn distinct_nonblank_lines(text: &str) -> BTreeSet<String> {
    text.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_string)
        .collect()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct IdentityMatch {
    created: bool,
    prj: bool,
    shared_new_ids: usize,
}

impl IdentityMatch {
    fn matches(self) -> bool {
        self.created || self.prj || self.shared_new_ids >= 3
    }

    fn score(self) -> u8 {
        u8::from(self.created)
            + u8::from(self.prj)
            + u8::from(self.shared_new_ids >= 3)
    }

    fn evidence(self, created: Option<&str>, matched: &str) -> String {
        let mut parts = Vec::new();
        if self.created {
            match created {
                Some(value) => parts.push(format!("created {value}")),
                None => parts.push("created".to_string()),
            }
        }
        if self.prj {
            parts.push("^prj line".to_string());
        }
        if self.shared_new_ids >= 3 {
            parts.push(format!(
                "{} task block IDs shared with {matched}",
                self.shared_new_ids
            ));
        }
        parts.join(", ")
    }
}

fn identity_match(
    head_text: &str,
    work_text: &str,
    other_text: &str,
) -> Option<IdentityMatch> {
    let head_created = created_value(head_text);
    let work_created = created_value(work_text);
    let other_created = created_value(other_text);
    let created = work_created.is_some()
        && work_created != head_created
        && work_created == other_created;

    let head_prj = prj_task_line(head_text);
    let work_prj = prj_task_line(work_text);
    let other_prj = prj_task_line(other_text);
    let prj =
        work_prj.is_some() && work_prj != head_prj && work_prj == other_prj;

    let other_ids = task_block_ids(other_text);
    let shared_new_ids = new_task_block_ids(head_text, work_text)
        .into_iter()
        .filter(|id| other_ids.contains(id))
        .count();

    let identity = IdentityMatch {
        created,
        prj,
        shared_new_ids,
    };
    identity.matches().then_some(identity)
}

fn created_value(text: &str) -> Option<String> {
    let frontmatter = projects::parse_frontmatter(text)?;
    projects::frontmatter_value(&frontmatter, "created")
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn prj_task_line(text: &str) -> Option<&str> {
    text.lines().find_map(|line| {
        task_line_body(line)?;
        (collect_done::trailing_block_id_in_line(line).as_deref()
            == Some("prj"))
        .then_some(line.trim())
    })
}

fn new_task_block_ids(head_text: &str, work_text: &str) -> BTreeSet<String> {
    let head_ids = task_block_ids(head_text);
    task_block_ids(work_text)
        .into_iter()
        .filter(|id| !head_ids.contains(id))
        .collect()
}

fn task_block_ids(text: &str) -> BTreeSet<String> {
    text.lines()
        .filter(|line| task_line_body(line).is_some())
        .filter_map(collect_done::trailing_block_id_in_line)
        .filter(|id| id != "prj")
        .collect()
}

fn task_line_body(line: &str) -> Option<&str> {
    let rest = line.trim_start();
    let after_open = rest.strip_prefix("- [")?;
    let mut chars = after_open.chars();
    let status = chars.next()?;
    let after_status = &after_open[status.len_utf8()..];
    after_status.strip_prefix("] ")
}

fn git_head_blob(
    vault: &Path,
    child_env: &ChildEnv,
    path: &Path,
) -> Result<Vec<u8>, String> {
    let spec = format!("HEAD:{}", display_path(path));
    let output = ob::git_command(vault, child_env)
        .args(["cat-file", "-p", &spec])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .map_err(|error| format!("failed to run git cat-file: {error}"))?;
    if output.status.success() {
        Ok(output.stdout)
    } else {
        Err(format!(
            "git cat-file -p {spec} failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ))
    }
}

fn git_grep_files(
    vault: &Path,
    child_env: &ChildEnv,
    needle: &str,
) -> Result<Vec<PathBuf>, String> {
    let output = ob::git_command(vault, child_env)
        .args(["grep", "-F", "-l", "-z", "-I", "--", needle])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .map_err(|error| format!("failed to run git grep: {error}"))?;
    match output.status.code() {
        Some(0) => Ok(output
            .stdout
            .split(|byte| *byte == 0)
            .filter(|record| !record.is_empty())
            .map(|record| {
                PathBuf::from(String::from_utf8_lossy(record).into_owned())
            })
            .collect()),
        Some(1) => Ok(Vec::new()),
        _ => Err(format!(
            "git grep failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )),
    }
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

    const SASE_CREATED: &str = "2026-06-01T03:30:02-04:00";
    const RECOVERY_CREATED: &str = "2026-10-19T19:32:43-0400";

    fn recovery_head() -> String {
        format!(
            "---\ncreated: {RECOVERY_CREATED}\nparent: \"[[sase]]\"\n---\n\
- [ ] #task Failed, killed, or interrupted agents can be recovered in one action without losing work! #prj ^prj\n\
- [ ] #task Recover the session pane ^time-all-of-turn\n\
- [ ] #task Preserve the transcript ^restart-survival\n\
- [ ] #task Keep the worktree ^worktree-keep\n\
- [ ] #task Unique recovery line one ^recovery-one\n\
- [ ] #task Unique recovery line two ^recovery-two\n\
- [ ] #task Unique recovery line three ^recovery-three\n\
- [ ] #task Unique recovery line four ^recovery-four\n"
        )
    }

    fn sase_note() -> String {
        format!(
            "---\ncreated: {SASE_CREATED}\n---\n\
- [ ] #task Get ≥1000 stars #prj ^prj\n\
- [ ] #task Task alpha ^alpha\n\
- [ ] #task Task beta ^beta\n\
- [ ] #task Task gamma ^gamma\n\
- [ ] #task Task delta ^delta\n\
- [ ] #task Task epsilon ^epsilon\n"
        )
    }

    fn clobbered_work(sase: &str) -> String {
        sase.replace(
            "- [ ] #task Task alpha ^alpha\n",
            "- [?] #task Task alpha [scheduled:: 2026-11-19] ^alpha\n\
- 🎲 P2 roll · in 22\n",
        )
    }

    #[test]
    fn incident_shaped_copy_matches_foreign_identity() {
        let head = recovery_head();
        let sase = sase_note();
        let work = clobbered_work(&sase);
        assert!(is_displaced(&head, &work));
        let identity =
            identity_match(&head, &work, &sase).expect("foreign identity");
        assert!(identity.created);
        assert!(identity.prj);
        assert!(identity.shared_new_ids >= 3);
        assert!(identity
            .evidence(Some(SASE_CREATED), "sase.md")
            .contains("created 2026-06-01T03:30:02-04:00, ^prj line"));
    }

    #[test]
    fn task_move_style_transfer_is_not_foreign() {
        let head = recovery_head();
        let mut work = recovery_head();
        work.push_str("- [ ] #task Task alpha ^alpha\n");
        let mut other = sase_note();
        other = other.replace("- [ ] #task Task alpha ^alpha\n", "");
        assert!(identity_match(&head, &work, &other).is_none());
    }

    #[test]
    fn deleting_most_of_a_note_is_not_foreign() {
        let head = recovery_head();
        let work = "---\ncreated: 2026-10-19T19:32:43-0400\n---\nleft\n";
        assert!(is_displaced(&head, work));
        assert!(identity_match(&head, work, &sase_note()).is_none());
    }

    #[test]
    fn notes_under_five_distinct_lines_are_not_displaced() {
        let head = "one\ntwo\nthree\nfour\n";
        let work = "other\n";
        assert!(!is_displaced(head, work));
    }

    #[test]
    fn a_single_duplicated_task_is_not_foreign() {
        let head = recovery_head();
        let mut work = recovery_head();
        work.push_str("- [ ] #task Extra copy ^alpha\n");
        let other = sase_note();
        assert!(identity_match(&head, &work, &other).is_none());
    }

    #[test]
    fn excluded_paths_skip_conflicts_done_daily_and_dot_dirs() {
        assert!(!is_clobber_scan_path(Path::new(
            "_conflicts/sase_agent_recovery.md"
        )));
        assert!(!is_clobber_scan_path(Path::new("done/sase_done.md")));
        assert!(!is_clobber_scan_path(Path::new("2026/20261028.md")));
        assert!(!is_clobber_scan_path(Path::new("2026/20261028-extra.md")));
        assert!(!is_clobber_scan_path(Path::new(".bob/note.md")));
        assert!(!is_clobber_scan_path(Path::new(".trash/note.md")));
        assert!(!is_clobber_scan_path(Path::new("_templates/note.md")));
        assert!(!is_clobber_scan_path(Path::new("note.txt")));
        assert!(is_clobber_scan_path(Path::new("sase_agent_recovery.md")));
        assert!(is_clobber_scan_path(Path::new("area/project.md")));
    }

    #[test]
    fn added_and_deleted_status_codes_are_not_candidates() {
        let path = Path::new("sase_agent_recovery.md");
        assert!(!is_clobber_candidate("A ", path));
        assert!(!is_clobber_candidate("??", path));
        assert!(!is_clobber_candidate(" D", path));
        assert!(!is_clobber_candidate("D ", path));
        assert!(!is_clobber_candidate("R ", path));
        assert!(is_clobber_candidate(" M", path));
        assert!(is_clobber_candidate("M ", path));
        assert!(is_clobber_candidate("MM", path));
    }

    #[test]
    fn grep_needles_cover_created_prj_and_new_ids() {
        let needles =
            grep_needles(&recovery_head(), &clobbered_work(&sase_note()));
        assert!(needles.iter().any(|needle| needle == SASE_CREATED));
        assert!(needles.iter().any(|needle| needle.contains("≥1000 stars")));
        assert!(needles.iter().any(|needle| needle == "^alpha"));
    }
}
