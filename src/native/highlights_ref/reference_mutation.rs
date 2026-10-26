//! Shared lock, precondition, scoped-git, and rollback helpers for
//! reference rename and overwrite.
//!
//! Operation-specific messages and policy stay at the callers. Rename's
//! offline dirty-path warning and overwrite's descendant coverage both
//! consume [`dirty_touched_paths`]; this module does not choose between
//! them.

use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
    process::Command,
    time::Duration,
};

use super::*;
use crate::native::{ob, ref_library::RefRow};

const MAINTENANCE_LOCK_BUDGET: Duration = Duration::from_secs(60);
const SCAN_LOCK_BUDGET: Duration = Duration::from_secs(30);
const LEGACY_REF_DIRS: &[&str] = &["ai", "zorg"];

/// Exclusive maintenance then scan locks, held for one mutation.
pub(crate) struct MutationLocks {
    _maintenance: fs::File,
    _scan: fs::File,
}

/// Failures while taking the mutation lock session or checking the
/// worktree, with operation-specific wrapping left to the caller.
#[derive(Debug)]
pub(crate) enum MutationSessionError {
    Maintenance(ob::LockWaitError),
    NotAWorktree,
    Scan(CommandError),
}

/// Why Git status could not be read for a mutation plan.
#[derive(Debug)]
pub(crate) enum DirtyPathError {
    GitUnavailable(String),
    StatusFailed,
}

/// Acquire `bob_sync.lock` then `scan.lock`, checking that `vault` is a
/// Git worktree between the two so a non-worktree refusal does not hold
/// the scan lock.
pub(crate) fn acquire_mutation_session(
    vault: &Path,
    child_env: &ob::ChildEnv,
    on_maintenance_wait: impl FnOnce(),
) -> std::result::Result<MutationLocks, MutationSessionError> {
    let maintenance =
        ob::acquire_lock_waiting(MAINTENANCE_LOCK_BUDGET, on_maintenance_wait)
            .map_err(MutationSessionError::Maintenance)?;
    if !vault_is_git_worktree(vault, child_env) {
        return Err(MutationSessionError::NotAWorktree);
    }
    let scan = super::acquire_scan_writer_lock_with_timeout(SCAN_LOCK_BUDGET)
        .map_err(MutationSessionError::Scan)?;
    Ok(MutationLocks {
        _maintenance: maintenance,
        _scan: scan,
    })
}

pub(crate) fn vault_is_git_worktree(
    vault: &Path,
    child_env: &ob::ChildEnv,
) -> bool {
    matches!(ob::detect_git_worktree(vault, child_env), Ok(true))
}

pub(crate) fn git_head(
    vault: &Path,
    child_env: &ob::ChildEnv,
) -> std::result::Result<String, String> {
    let output = ob::git_command(vault, child_env)
        .args(["rev-parse", "HEAD"])
        .output()
        .map_err(|error| format!("git rev-parse HEAD: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "git rev-parse HEAD failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

pub(crate) fn latest_commit_for_path(
    vault: &Path,
    child_env: &ob::ChildEnv,
    path: &Path,
) -> std::result::Result<Option<(String, String)>, String> {
    let output = ob::git_command(vault, child_env)
        .args(["log", "-1", "--format=%H%x00%s", "--"])
        .arg(path)
        .output()
        .map_err(|error| format!("git log for {}: {error}", path.display()))?;
    if !output.status.success() {
        return Err(format!(
            "git log for {} failed: {}",
            path.display(),
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    let value = String::from_utf8_lossy(&output.stdout);
    let Some((sha, subject)) = value.trim_end().split_once('\0') else {
        return Ok(None);
    };
    Ok(Some((sha.to_string(), subject.to_string())))
}

pub(crate) fn unstage_paths(
    vault: &Path,
    child_env: &ob::ChildEnv,
    paths: &[PathBuf],
) -> std::result::Result<(), String> {
    let output = ob::git_command(vault, child_env)
        .arg("reset")
        .arg("--quiet")
        .arg("HEAD")
        .arg("--")
        .args(paths)
        .output()
        .map_err(|error| error.to_string())?;
    if output.status.success() {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&output.stderr).trim().to_string())
    }
}

pub(crate) fn committed_despite_error(
    vault: &Path,
    child_env: &ob::ChildEnv,
    old_head: Option<&str>,
    expected_subject: &str,
) -> Option<(String, String)> {
    let (sha, subject) =
        latest_commit_for_path(vault, child_env, Path::new(".")).ok()??;
    (old_head != Some(sha.as_str()) && subject == expected_subject)
        .then_some((sha, subject))
}

/// Dirty Git paths that overlap `touched_paths`.
///
/// Exact matching (rename) requires a porcelain path to equal a touched
/// path. `include_descendants` also matches when one path is under the
/// other, so overwrite can cover sidecar directories and their files.
pub(crate) fn dirty_touched_paths(
    vault: &Path,
    touched_paths: &BTreeSet<PathBuf>,
    include_descendants: bool,
) -> std::result::Result<Vec<PathBuf>, DirtyPathError> {
    if !vault.join(".git").exists() {
        return Ok(Vec::new());
    }
    let output = Command::new("git")
        .arg("-C")
        .arg(vault)
        .args(["status", "--porcelain=v1", "-z", "--untracked-files=all"])
        .output()
        .map_err(|error| DirtyPathError::GitUnavailable(error.to_string()))?;
    if !output.status.success() {
        return Err(DirtyPathError::StatusFailed);
    }
    let mut records = output.stdout.split(|byte| *byte == 0);
    let mut dirty = BTreeSet::new();
    while let Some(record) = records.next() {
        if record.len() < 4 {
            continue;
        }
        let status = String::from_utf8_lossy(&record[..2]);
        let path =
            PathBuf::from(String::from_utf8_lossy(&record[3..]).as_ref());
        dirty.insert(path);
        if (status.contains('R') || status.contains('C'))
            && let Some(previous) = records.next()
        {
            dirty.insert(PathBuf::from(
                String::from_utf8_lossy(previous).as_ref(),
            ));
        }
    }
    let touched = touched_paths
        .iter()
        .map(|path| normalized_path(&vault_path(path)))
        .collect::<BTreeSet<_>>();
    let mut matches = dirty
        .into_iter()
        .filter(|path| {
            let dirty = normalized_path(&vault_path(path));
            dirty_path_matches(&dirty, &touched, include_descendants)
        })
        .collect::<Vec<_>>();
    matches.sort();
    Ok(matches)
}

/// Ignored (and therefore invisible to `git status --untracked-files=all`)
/// files under `touched_paths`. Overwrite preflight merges these into
/// `dirty_paths` so a gitignored file inside a sidecar directory cannot
/// be deleted by `remove_old_sidecar` and then wedge rollback.
pub(crate) fn ignored_paths_under(
    vault: &Path,
    touched_paths: &BTreeSet<PathBuf>,
) -> std::result::Result<Vec<PathBuf>, DirtyPathError> {
    if !vault.join(".git").exists() || touched_paths.is_empty() {
        return Ok(Vec::new());
    }
    let output = Command::new("git")
        .arg("-C")
        .arg(vault)
        .args([
            "ls-files",
            "-z",
            "--others",
            "--ignored",
            "--exclude-standard",
            "--",
        ])
        .args(touched_paths)
        .output()
        .map_err(|error| DirtyPathError::GitUnavailable(error.to_string()))?;
    if !output.status.success() {
        return Err(DirtyPathError::StatusFailed);
    }
    let mut ignored = output
        .stdout
        .split(|byte| *byte == 0)
        .filter(|record| !record.is_empty())
        .map(|record| PathBuf::from(String::from_utf8_lossy(record).as_ref()))
        .collect::<Vec<_>>();
    ignored.sort();
    ignored.dedup();
    Ok(ignored)
}

fn dirty_path_matches(
    dirty: &str,
    touched: &BTreeSet<String>,
    include_descendants: bool,
) -> bool {
    if touched.contains(dirty) {
        return true;
    }
    include_descendants
        && touched
            .iter()
            .any(|path| path_covers(path, dirty) || path_covers(dirty, path))
}

fn path_covers(ancestor: &str, descendant: &str) -> bool {
    descendant == ancestor
        || descendant.starts_with(ancestor)
            && descendant.as_bytes().get(ancestor.len()) == Some(&b'/')
}

pub(crate) fn is_legacy(row: &RefRow, configured_ref_dir: &Path) -> bool {
    if row.era == "legacy" || row.source_pdf.is_none() {
        return true;
    }
    let path = Path::new(&row.path);
    let Some(relative) = path
        .strip_prefix(configured_ref_dir.file_name().unwrap_or_default())
        .ok()
    else {
        return false;
    };
    relative
        .components()
        .next()
        .and_then(|part| part.as_os_str().to_str())
        .is_some_and(|name| LEGACY_REF_DIRS.contains(&name))
}

pub(crate) fn intake_query(config: &Config, query: &str) -> bool {
    let value = crate::native::env::expand_tilde(Path::new(query));
    let intake_root = &config.xlib_dir;
    if value.is_absolute() && value.starts_with(intake_root) {
        return true;
    }
    let under_vault = if value.is_absolute() {
        value
            .strip_prefix(&config.bob_dir)
            .ok()
            .map(Path::to_path_buf)
    } else {
        Some(value.to_path_buf())
    };
    let Some(relative) = under_vault else {
        return false;
    };
    let xlib_relative = intake_root
        .strip_prefix(&config.bob_dir)
        .unwrap_or(Path::new("xlib"));
    relative.starts_with(xlib_relative)
}

pub(crate) fn has_managed_highlights(contents: &str) -> bool {
    let body = split_frontmatter(contents)
        .map(|(_, body)| body)
        .unwrap_or_else(|| contents.to_string());
    let parts = split_note_body(&body);
    let Some(region) = parts.region else {
        return false;
    };
    let parsed = parse_managed_region(&region);
    parsed
        .blocks
        .iter()
        .any(|block| block.block_id.starts_with("h-"))
        || parsed.removed.iter().any(|id| id.starts_with("h-"))
}

pub(crate) fn is_in_sync(config: &Config, pdf_relative: &Path) -> bool {
    is_in_sync_with(config, pdf_relative, false)
}

pub(crate) fn is_in_sync_for_overwrite(
    config: &Config,
    pdf_relative: &Path,
) -> bool {
    is_in_sync_with(config, pdf_relative, true)
}

fn is_in_sync_with(
    config: &Config,
    pdf_relative: &Path,
    ignore_unannotated_sidecar: bool,
) -> bool {
    let pdf = config.bob_dir.join(pdf_relative);
    let ctx = ScanContext::build(config);
    let Ok(mut plan) = plan_pdf_sync(
        config,
        &pdf,
        SyncOptions {
            dry_run: true,
            write_pdf: false,
            prefer: None,
            ignore_unannotated_sidecar,
        },
        &ctx,
    ) else {
        return false;
    };
    let mut plans = [&mut plan];
    if finalize_annotation_task_plans(config, &mut plans, &ctx).is_err() {
        return false;
    }
    reserve_preview_ids(config, &mut plans, &ctx);
    let reading_unchanged_or_adopted =
        plan.reading_task_plan.as_ref().is_some_and(|reading| {
            reading.action == ReadingTaskAction::NoWrite
                && (plan.reading_located.is_some()
                    || reading.kind == ReadingTaskKind::Birth)
        });
    plan.stable_note_action == "none"
        && !plan.marker_write_needed
        && reading_unchanged_or_adopted
}

/// Reverse `from → to` moves, last first. A destination that is already
/// gone while its source exists is treated as already rolled back.
pub(crate) fn rollback_moves(moves: &[(PathBuf, PathBuf)]) -> Vec<String> {
    let mut failures = Vec::new();
    for (from, to) in moves.iter().rev() {
        if !to.exists() {
            if from.exists() {
                continue;
            }
            failures.push(format!(
                "{} and {} are both missing",
                from.display(),
                to.display()
            ));
            continue;
        }
        if from.exists() {
            failures.push(format!(
                "{} exists; left {} in place",
                from.display(),
                to.display()
            ));
            continue;
        }
        if let Err(error) = super::io::rename_no_replace(to, from) {
            failures.push(format!(
                "could not move {} back to {}: {error}",
                to.display(),
                from.display()
            ));
        }
    }
    failures
}

/// Restore `path` to `before` when current bytes still equal `after` or
/// `before`. Concurrent changes are left in place. `kind` is interpolated
/// into the caller-facing failure text (`PDF` for rename).
pub(crate) fn restore_bytes_if_uncontended(
    path: &Path,
    before: &[u8],
    after: Option<&[u8]>,
    kind: &str,
) -> Option<String> {
    match fs::read(path) {
        Ok(current)
            if after.is_some_and(|after| after == current.as_slice())
                || current == before =>
        {
            if current != before {
                if let Err(error) = atomic_write_bytes(path, before) {
                    return Some(format!(
                        "could not restore {kind} bytes at {}: {error}",
                        path.display()
                    ));
                }
            }
            None
        }
        Ok(_) => Some(format!(
            "left concurrently changed {kind} {} in place",
            path.display()
        )),
        Err(error) => Some(format!(
            "could not read {kind} {} before restore: {error}",
            path.display()
        )),
    }
}

pub(crate) fn atomic_write_bytes(
    path: &Path,
    contents: &[u8],
) -> std::io::Result<()> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let name = path.file_name().unwrap_or_default().to_string_lossy();
    let temporary =
        parent.join(format!(".{name}.bob-rollback-{}.tmp", std::process::id()));
    let _ = fs::remove_file(&temporary);
    fs::write(&temporary, contents)?;
    fs::File::open(&temporary)?.sync_all()?;
    fs::rename(&temporary, path)?;
    if let Ok(directory) = fs::File::open(parent) {
        directory.sync_all()?;
    }
    Ok(())
}

pub(crate) fn vault_path(path: &Path) -> String {
    path.components()
        .map(|component| component.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

pub(crate) fn normalized_path(path: &str) -> String {
    path.replace('\\', "/")
        .trim_start_matches("./")
        .to_ascii_lowercase()
}

pub(crate) fn display_paths(paths: &[PathBuf]) -> String {
    paths
        .iter()
        .map(|path| vault_path(path))
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    fn init_repo(vault: &Path) {
        let run = |args: &[&str]| {
            let status = Command::new("git")
                .arg("-C")
                .arg(vault)
                .args(args)
                .status()
                .expect("run git");
            assert!(status.success(), "git {args:?} failed");
        };
        run(&["init"]);
        run(&["config", "user.email", "test@example.com"]);
        run(&["config", "user.name", "Test"]);
        run(&["config", "commit.gpgsign", "false"]);
    }

    fn test_config(vault: &Path) -> Config {
        Config {
            bob_dir: vault.to_path_buf(),
            lib_dir: vault.join("lib"),
            ref_dir: vault.join("ref"),
            xlib_dir: vault.join("xlib"),
        }
    }

    fn legacy_row(path: &str, era: &str, source_pdf: Option<&str>) -> RefRow {
        RefRow {
            path: path.into(),
            link: String::new(),
            title: String::new(),
            marker_id: None,
            origin: String::new(),
            ref_type: None,
            era: era.into(),
            status: None,
            status_sync: String::new(),
            frontmatter_status: None,
            legacy_status: None,
            reading_state: String::new(),
            reading_state_source: String::new(),
            blocked: false,
            blocked_by: Vec::new(),
            parent: None,
            task: None,
            urls: Vec::new(),
            earlier: Vec::new(),
            reading: None,
            identity: crate::native::ref_library::RefIdentity {
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
            source_pdf: source_pdf.map(str::to_string),
            audio: None,
            annotation_count: 0,
            comment_count: 0,
            snapshot: None,
            research_ref: None,
            superseded_by: None,
            diagnostics: Vec::new(),
            family: path.into(),
            has_later: false,
            id: None,
            source_block: None,
            source_path: None,
            source_id: None,
            source_blocks: Vec::new(),
            v2: false,
            task_line: None,
        }
    }

    #[test]
    fn vault_path_joins_components_with_slashes() {
        assert_eq!(
            vault_path(Path::new("ref/blogs/old.md")),
            "ref/blogs/old.md"
        );
    }

    #[test]
    fn display_paths_joins_vault_paths() {
        assert_eq!(
            display_paths(&[
                PathBuf::from("ref/blogs/old.md"),
                PathBuf::from("lib/blogs/old.pdf"),
            ]),
            "ref/blogs/old.md, lib/blogs/old.pdf"
        );
    }

    #[test]
    fn legacy_paths_are_recognized_by_ref_type() {
        let row =
            legacy_row("ref/ai/book.md", "modern", Some("lib/ai/book.pdf"));
        assert!(is_legacy(&row, Path::new("/vault/ref")));
        let zorg =
            legacy_row("ref/zorg/book.md", "modern", Some("lib/zorg/book.pdf"));
        assert!(is_legacy(&zorg, Path::new("/vault/ref")));
        let modern = legacy_row(
            "ref/blogs/book.md",
            "modern",
            Some("lib/blogs/book.pdf"),
        );
        assert!(!is_legacy(&modern, Path::new("/vault/ref")));
        let era = legacy_row(
            "ref/blogs/book.md",
            "legacy",
            Some("lib/blogs/book.pdf"),
        );
        assert!(is_legacy(&era, Path::new("/vault/ref")));
        let no_pdf = legacy_row("ref/blogs/book.md", "modern", None);
        assert!(is_legacy(&no_pdf, Path::new("/vault/ref")));
    }

    #[test]
    fn intake_query_matches_xlib_absolute_and_relative_paths() {
        let temp = tempfile::tempdir().expect("scratch vault");
        let vault = temp.path();
        let config = test_config(vault);
        assert!(intake_query(
            &config,
            &vault.join("xlib/blogs/intake.pdf").to_string_lossy()
        ));
        assert!(intake_query(&config, "xlib/blogs/intake.pdf"));
        assert!(!intake_query(&config, "ref/blogs/filed.md"));
        assert!(!intake_query(
            &config,
            &vault.join("lib/blogs/filed.pdf").to_string_lossy()
        ));
    }

    #[test]
    fn managed_highlight_inspection_sees_live_and_removed_blocks() {
        let live = concat!(
            "---\nid: annotated\n---\n",
            "<!-- highlights:begin -->\n\n",
            "### Page 1\n\n> [!quote] A quote\n^h-0123456789abcdef\n\n",
            "<!-- highlights:end -->\n",
        );
        assert!(has_managed_highlights(live));

        let removed = concat!(
            "---\nid: annotated\n---\n",
            "<!-- highlights:begin -->\n\n",
            "### Removed highlights\n\n",
            "> [!warning] Removed highlight This annotation is no longer present ",
            "in the Highlights sidecar.\n",
            "^h-0123456789abcdef\n\n",
            "<!-- highlights:end -->\n",
        );
        assert!(has_managed_highlights(removed));

        let skeleton = concat!(
            "---\nid: plain\n---\n",
            "# Title\n\n![[sase#^ref-plain]]\n",
        );
        assert!(!has_managed_highlights(skeleton));
    }

    #[test]
    fn dirty_touched_paths_exact_ignores_descendants() {
        let temp = tempfile::tempdir().expect("scratch vault");
        let vault = temp.path();
        fs::create_dir_all(vault.join("lib/blogs/old.textbundle")).unwrap();
        fs::write(vault.join("lib/blogs/old.pdf"), b"pdf").unwrap();
        fs::write(vault.join("lib/blogs/old.textbundle/text.md"), "sidecar\n")
            .unwrap();
        init_repo(vault);
        let add = Command::new("git")
            .arg("-C")
            .arg(vault)
            .args(["add", "-A"])
            .status()
            .expect("git add");
        assert!(add.success());
        let commit = Command::new("git")
            .arg("-C")
            .arg(vault)
            .args(["commit", "-q", "-m", "fixture"])
            .status()
            .expect("git commit");
        assert!(commit.success());
        fs::write(vault.join("lib/blogs/old.textbundle/text.md"), "dirty\n")
            .unwrap();

        let touched = BTreeSet::from([PathBuf::from("lib/blogs/old.pdf")]);
        let exact =
            dirty_touched_paths(vault, &touched, false).expect("exact dirty");
        assert!(exact.is_empty());

        let tree_touched =
            BTreeSet::from([PathBuf::from("lib/blogs/old.textbundle")]);
        let descendants = dirty_touched_paths(vault, &tree_touched, true)
            .expect("tree dirty");
        assert_eq!(
            descendants,
            [PathBuf::from("lib/blogs/old.textbundle/text.md")]
        );
        let exact_tree = dirty_touched_paths(vault, &tree_touched, false)
            .expect("exact tree");
        assert!(exact_tree.is_empty());
    }

    #[test]
    fn ignored_paths_under_lists_gitignored_sidecar_files() {
        let temp = tempfile::tempdir().expect("scratch vault");
        let vault = temp.path();
        fs::create_dir_all(vault.join("lib/blogs/old.textbundle")).unwrap();
        fs::write(vault.join("lib/blogs/old.pdf"), b"pdf").unwrap();
        fs::write(vault.join("lib/blogs/old.textbundle/text.md"), "sidecar\n")
            .unwrap();
        fs::write(vault.join(".gitignore"), "*.tmp\n").unwrap();
        init_repo(vault);
        let add = Command::new("git")
            .arg("-C")
            .arg(vault)
            .args(["add", "-A"])
            .status()
            .expect("git add");
        assert!(add.success());
        let commit = Command::new("git")
            .arg("-C")
            .arg(vault)
            .args(["commit", "-q", "-m", "fixture"])
            .status()
            .expect("git commit");
        assert!(commit.success());
        fs::write(
            vault.join("lib/blogs/old.textbundle/secret.tmp"),
            "ignored\n",
        )
        .unwrap();

        let touched =
            BTreeSet::from([PathBuf::from("lib/blogs/old.textbundle")]);
        let dirty = dirty_touched_paths(vault, &touched, true).expect("dirty");
        assert!(
            dirty.is_empty(),
            "git status does not report ignored files: {dirty:?}"
        );
        let ignored = ignored_paths_under(vault, &touched).expect("ignored");
        assert_eq!(
            ignored,
            [PathBuf::from("lib/blogs/old.textbundle/secret.tmp")]
        );
    }

    #[test]
    fn dirty_touched_paths_without_git_are_empty() {
        let temp = tempfile::tempdir().expect("scratch vault");
        let touched = BTreeSet::from([PathBuf::from("ref/blogs/old.md")]);
        let dirty =
            dirty_touched_paths(temp.path(), &touched, true).expect("no git");
        assert!(dirty.is_empty());
    }

    #[test]
    fn rollback_moves_reverses_successful_renames() {
        let temp = tempfile::tempdir().expect("scratch vault");
        let vault = temp.path();
        fs::create_dir_all(vault.join("lib/blogs")).unwrap();
        let from = vault.join("lib/blogs/old.pdf");
        let to = vault.join("lib/blogs/new.pdf");
        fs::write(&from, b"pdf").unwrap();
        super::super::io::rename_no_replace(&from, &to).unwrap();
        let failures = rollback_moves(&[(from.clone(), to.clone())]);
        assert!(failures.is_empty());
        assert_eq!(fs::read(&from).unwrap(), b"pdf");
        assert!(!to.exists());
    }

    #[test]
    fn rollback_moves_reports_missing_and_occupied_sources() {
        let temp = tempfile::tempdir().expect("scratch vault");
        let vault = temp.path();
        let missing_from = vault.join("missing-from");
        let missing_to = vault.join("missing-to");
        let occupied_from = vault.join("occupied-from");
        let occupied_to = vault.join("occupied-to");
        fs::write(&occupied_from, b"from").unwrap();
        fs::write(&occupied_to, b"to").unwrap();
        let failures = rollback_moves(&[
            (missing_from.clone(), missing_to.clone()),
            (occupied_from.clone(), occupied_to.clone()),
        ]);
        assert_eq!(failures.len(), 2);
        assert!(failures
            .iter()
            .any(|line| line.contains("are both missing")));
        assert!(failures.iter().any(|line| line.contains("exists; left")));
    }

    #[test]
    fn restore_bytes_restores_uncontended_after_image() {
        let temp = tempfile::tempdir().expect("scratch vault");
        let path = temp.path().join("doc.pdf");
        fs::write(&path, b"after").unwrap();
        let failure = restore_bytes_if_uncontended(
            &path,
            b"before",
            Some(b"after"),
            "PDF",
        );
        assert!(failure.is_none());
        assert_eq!(fs::read(&path).unwrap(), b"before");
    }

    #[test]
    fn restore_bytes_leaves_concurrent_changes() {
        let temp = tempfile::tempdir().expect("scratch vault");
        let path = temp.path().join("doc.pdf");
        fs::write(&path, b"other").unwrap();
        let failure = restore_bytes_if_uncontended(
            &path,
            b"before",
            Some(b"after"),
            "PDF",
        )
        .expect("concurrent");
        assert!(failure.contains("left concurrently changed PDF"));
        assert_eq!(fs::read(&path).unwrap(), b"other");
    }

    #[test]
    fn git_head_and_committed_despite_error_read_the_latest_subject() {
        let temp = tempfile::tempdir().expect("scratch vault");
        let vault = temp.path();
        fs::write(vault.join("note.md"), "one\n").unwrap();
        init_repo(vault);
        let add = Command::new("git")
            .arg("-C")
            .arg(vault)
            .args(["add", "-A"])
            .status()
            .expect("git add");
        assert!(add.success());
        let commit = Command::new("git")
            .arg("-C")
            .arg(vault)
            .args(["commit", "-q", "-m", "bob ref rename: old → new"])
            .status()
            .expect("git commit");
        assert!(commit.success());
        let env = ob::child_env();
        let head = git_head(vault, &env).expect("head");
        assert!(!head.is_empty());
        let latest = latest_commit_for_path(vault, &env, Path::new("note.md"))
            .expect("log")
            .expect("commit");
        assert_eq!(latest.0, head);
        assert_eq!(latest.1, "bob ref rename: old → new");
        assert!(committed_despite_error(
            vault,
            &env,
            Some("not-the-head"),
            "bob ref rename: old → new",
        )
        .is_some());
        assert!(committed_despite_error(
            vault,
            &env,
            Some(head.as_str()),
            "bob ref rename: old → new",
        )
        .is_none());
        assert!(vault_is_git_worktree(vault, &env));
        let other = tempfile::tempdir().expect("non-git path");
        assert!(!vault_is_git_worktree(other.path(), &env));
    }

    #[test]
    fn unstage_paths_clears_only_the_listed_paths() {
        let temp = tempfile::tempdir().expect("scratch vault");
        let vault = temp.path();
        fs::write(vault.join("keep.md"), "keep\n").unwrap();
        fs::write(vault.join("drop.md"), "drop\n").unwrap();
        init_repo(vault);
        let add = Command::new("git")
            .arg("-C")
            .arg(vault)
            .args(["add", "-A"])
            .status()
            .expect("git add");
        assert!(add.success());
        let commit = Command::new("git")
            .arg("-C")
            .arg(vault)
            .args(["commit", "-q", "-m", "fixture"])
            .status()
            .expect("git commit");
        assert!(commit.success());
        fs::write(vault.join("keep.md"), "keep-changed\n").unwrap();
        fs::write(vault.join("drop.md"), "drop-changed\n").unwrap();
        let add_again = Command::new("git")
            .arg("-C")
            .arg(vault)
            .args(["add", "keep.md", "drop.md"])
            .status()
            .expect("git add dirty");
        assert!(add_again.success());
        unstage_paths(vault, &ob::child_env(), &[PathBuf::from("drop.md")])
            .expect("unstage");
        let cached = Command::new("git")
            .arg("-C")
            .arg(vault)
            .args(["diff", "--cached", "--name-only"])
            .output()
            .expect("git diff");
        assert_eq!(String::from_utf8_lossy(&cached.stdout).trim(), "keep.md");
    }
}
