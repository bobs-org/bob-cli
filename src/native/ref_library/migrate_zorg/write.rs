//! `bob ref migrate-zorg --write`: reversible vault write (phase `writer`).
//!
//! `--write` applies exactly what the dry run plans, as one revertible
//! commit. It mirrors `bob task reroll`'s live flow: under `bob_sync.lock`
//! it pre-syncs, re-plans from disk, refuses existing targets, writes new
//! files only, verifies them through the index and coverage, and deletes
//! its own files on any failure. Then one scoped commit and a post-sync.

use std::path::{Path, PathBuf};
use std::time::Duration;

use clap::ArgMatches;

use super::super::output::{print_find_error, Format};
use super::super::{build_index, count_zorg_records, LibraryConfig};
use super::cli::migrate_config_from_matches;
use super::plan::{plan_migration, MigrationPlan, PlannedNote};
use super::report::{print_json, print_noop_json, render_human, ReportMode};
use crate::native::style::Styler;
use crate::native::{ob, vault_sync};

/// Lock budget for `--write`, matching `bob task reroll`'s default.
const LOCK_BUDGET: Duration = Duration::from_secs(60);

/// Run `bob ref migrate-zorg --write`: pre-sync, re-plan, write, verify,
/// commit exactly the new notes, and post-sync.
pub(crate) fn run_migrate_zorg_write(
    matches: &ArgMatches,
    format: Format,
) -> i32 {
    let offline = matches.get_flag("offline");
    let config = migrate_config_from_matches(matches);

    // 1. Hold the shared vault-maintenance lock for the whole write.
    let _lock = {
        let json_mode = format == Format::Json;
        let on_first_wait = move || {
            if json_mode {
                eprintln!("waiting for another vault maintenance run…");
            } else {
                let styler = Styler::detect();
                eprintln!(
                    "  {}",
                    styler.dim("waiting for another vault maintenance run…")
                );
            }
        };
        match ob::acquire_lock_waiting(LOCK_BUDGET, on_first_wait) {
            Ok(lock) => lock,
            Err(error) => {
                return fail(
                    format,
                    "lock",
                    &format!("acquire vault maintenance lock: {error}"),
                    Some("another maintenance run holds the lock; try again"),
                );
            }
        }
    };

    let child_env = ob::child_env();

    // 2. Reversibility depends on git: refuse a non-git vault before
    // touching anything.
    match ob::detect_git_worktree(&config.bob_dir, &child_env) {
        Ok(true) => {}
        Ok(false) => {
            return fail(
                format,
                "not_a_worktree",
                &format!(
                    "vault is not a git worktree ({}); --write needs git so the migration stays revertible",
                    config.bob_dir.display()
                ),
                Some(
                    "run without --write to preview, or initialize git in the vault",
                ),
            );
        }
        Err(error) => {
            return fail(
                format,
                "git_unavailable",
                &format!("could not run git rev-parse: {error}"),
                None,
            );
        }
    }

    // 3. Pre-sync so the migration commit holds only the new files. A
    // failed pre-sync aborts with reroll's `--offline` hint.
    if !offline {
        let cycle =
            vault_sync::run_cycle_with_existing_lock_report(&child_env, true);
        if !cycle.ok {
            return fail(
                format,
                "pre_sync",
                &format!(
                    "pre-sync failed: {}",
                    cycle.error.unwrap_or_else(|| {
                        "vault sync reported failure".to_string()
                    })
                ),
                Some("re-run with --offline to commit locally without syncing"),
            );
        }
    }

    // 4. Re-plan from disk under the lock.
    let index = match build_index(&config) {
        Ok(index) => index,
        Err(error) => {
            return fail(
                format,
                "missing_ref_dir",
                &error.to_string(),
                Some("pass -b/--bob-dir or -r/--ref-dir, or set BOB_DIR"),
            );
        }
    };
    let plan = plan_migration(&config.bob_dir, &config.ref_dir, &index.rows);
    let coverage_before =
        count_zorg_records(&config.bob_dir, &config.ref_dir, &index.rows).total;
    if plan.notes.is_empty() {
        match format {
            Format::Json => print_noop_json(&plan),
            Format::Human | Format::Markdown => println!("nothing to migrate"),
        }
        return 0;
    }

    // 5. Refuse before writing anything when a planned target already
    // exists, or when ref/zorg/ holds uncommitted changes.
    if let Some(note) = plan
        .notes
        .iter()
        .find(|note| config.bob_dir.join(&note.path).exists())
    {
        return fail(
            format,
            "target_exists",
            &format!(
                "refusing --write: planned target already exists: {}",
                note.path
            ),
            Some("move it aside or delete it, then re-run"),
        );
    }
    match zorg_status_clean(&config, &child_env) {
        Ok(clean) => {
            if !clean {
                return fail(
                    format,
                    "dirty_zorg",
                    "refusing --write: ref/zorg/ has uncommitted changes",
                    Some("commit, stash, or discard them, then re-run"),
                );
            }
        }
        Err(message) => {
            return fail(format, "git_status", &message, None);
        }
    }

    // 6. Write each note with create-new semantics.
    let created = match write_planned_notes(&config.bob_dir, &plan.notes) {
        Ok(created) => created,
        Err(message) => {
            return fail(format, "write", &message, None);
        }
    };

    // 7. Verify through the rebuilt index and coverage. On any failure,
    // delete every file this run created and exit without committing.
    if let Err(message) =
        verify_written(&config, &plan, &created, coverage_before)
    {
        cleanup_created(&created);
        return fail(format, "verify", &message, None);
    }

    // 9. Commit exactly the written paths.
    let message = commit_message(&plan);
    let relative = relative_paths(&config.bob_dir, &created.files);
    let sha = match ob::commit_paths(
        &config.bob_dir,
        &child_env,
        &message,
        &relative,
    ) {
        Ok(Some(sha)) => sha,
        Ok(None) => {
            return fail(
                format,
                "commit",
                &format!(
                    "commit notes: nothing to commit after writing {} note(s)",
                    created.files.len()
                ),
                Some("resolve the git failure and re-run"),
            );
        }
        Err(message) => {
            return fail(
                format,
                "commit",
                &format!("commit notes: {message}"),
                Some("resolve the git failure and re-run; notes are already written"),
            );
        }
    };
    let subject = message.lines().next().unwrap_or(&message).to_string();

    // 10. Post-sync. The local commit stands on failure: report it and
    // exit 1, never rolling back good local edits.
    let pushed = if offline {
        false
    } else {
        let cycle =
            vault_sync::run_cycle_with_existing_lock_report(&child_env, true);
        if !cycle.ok || !cycle.conflicts.is_empty() {
            let mut conflicts = cycle.conflicts.clone();
            conflicts.sort();
            let detail = if conflicts.is_empty() {
                cycle.error.unwrap_or_else(|| {
                    "vault sync reported failure".to_string()
                })
            } else {
                format!(
                    "post-sync conflict in {}: the remote copy wins in place",
                    conflicts.join(", ")
                )
            };
            return fail(
                format,
                "post_sync",
                &format!(
                    "post-sync failed ({detail}); committed `{}` locally; background vault-sync will publish it",
                    short_sha(&sha),
                ),
                Some("run bob vault-sync later, or re-run with --offline"),
            );
        }
        // The human report already names the commit below; note the push.
        cycle.pushed
    };

    let mode = ReportMode::write(&sha, &subject);
    match format {
        Format::Json => print_json(&plan, &mode),
        Format::Human | Format::Markdown => {
            print!("{}", render_human(&plan, &mode));
            if pushed {
                println!("pushed to origin/master");
            }
        }
    }
    0
}

/// Report a `--write` failure in the requested format; every failure
/// exits 1.
fn fail(format: Format, code: &str, message: &str, hint: Option<&str>) -> i32 {
    print_find_error(format, "ref migrate-zorg", code, message, hint);
    1
}

/// Whether `git status --porcelain -- <ref-dir>/zorg` is empty.
fn zorg_status_clean(
    config: &LibraryConfig,
    child_env: &ob::ChildEnv,
) -> Result<bool, String> {
    let zorg_dir = config.ref_dir.join("zorg");
    let output = ob::git_command(&config.bob_dir, child_env)
        .arg("status")
        .arg("--porcelain")
        .arg("--")
        .arg(&zorg_dir)
        .output()
        .map_err(|error| {
            format!("could not run git status for ref/zorg/: {error}")
        })?;
    if !output.status.success() {
        return Err(format!(
            "git status for ref/zorg/ failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(output.stdout.iter().all(|byte| byte.is_ascii_whitespace()))
}

/// Files this run created plus ancestor directories that did not exist
/// before the run, so cleanup removes exactly its own output.
#[derive(Debug, Default)]
struct CreatedPaths {
    files: Vec<PathBuf>,
    dirs: Vec<PathBuf>,
}

/// Write every planned note: a temp file in the target directory, fsync,
/// a no-clobber rename, then a re-read that compares bytes. On any
/// failure, delete every file created so far and return the error.
fn write_planned_notes(
    bob_dir: &Path,
    notes: &[PlannedNote],
) -> Result<CreatedPaths, String> {
    let mut created = CreatedPaths::default();
    for note in notes {
        if let Err(message) = write_one_note(bob_dir, note, &mut created) {
            cleanup_created(&created);
            return Err(message);
        }
    }
    Ok(created)
}

/// Write one planned note and record its file plus any directories the
/// run created.
fn write_one_note(
    bob_dir: &Path,
    note: &PlannedNote,
    created: &mut CreatedPaths,
) -> Result<(), String> {
    let target = bob_dir.join(&note.path);
    if target.exists() {
        return Err(format!(
            "refusing --write: planned target already exists: {}",
            note.path
        ));
    }
    let parent = target.parent().ok_or_else(|| {
        format!("planned target has no parent directory: {}", note.path)
    })?;
    // Ancestor directories the run creates, deepest last, so cleanup can
    // prune exactly its own empty directories.
    let mut missing: Vec<PathBuf> = Vec::new();
    let mut cursor = parent;
    loop {
        if cursor.exists() {
            break;
        }
        missing.push(cursor.to_path_buf());
        match cursor.parent() {
            Some(next) if next != cursor => cursor = next,
            _ => break,
        }
        if cursor == bob_dir {
            if !cursor.exists() {
                missing.push(cursor.to_path_buf());
            }
            break;
        }
    }
    std::fs::create_dir_all(parent).map_err(|error| {
        format!("create parent directory {}: {error}", parent.display())
    })?;
    let file_name = target.file_name().ok_or_else(|| {
        format!("planned target has no file name: {}", note.path)
    })?;
    let mut temp_name = std::ffi::OsString::from(".");
    temp_name.push(file_name);
    temp_name.push(format!(".{}.tmp", std::process::id()));
    let temp = target.with_file_name(temp_name);
    let _ = std::fs::remove_file(&temp);
    std::fs::write(&temp, &note.contents).map_err(|error| {
        let _ = std::fs::remove_file(&temp);
        format!("write temporary file {}: {error}", temp.display())
    })?;
    if let Err(error) =
        std::fs::File::open(&temp).and_then(|file| file.sync_all())
    {
        let _ = std::fs::remove_file(&temp);
        return Err(format!("sync temporary file {}: {error}", temp.display()));
    }
    // No-clobber rename: the pre-write refuse already passed, so an
    // existing target here is a concurrent write — fail, never overwrite.
    if target.exists() {
        let _ = std::fs::remove_file(&temp);
        return Err(format!(
            "refusing --write: planned target already exists: {}",
            note.path
        ));
    }
    if let Err(error) = std::fs::rename(&temp, &target) {
        let _ = std::fs::remove_file(&temp);
        return Err(format!("install note {}: {error}", target.display()));
    }
    if let Some(parent) = target.parent()
        && let Err(error) =
            std::fs::File::open(parent).and_then(|file| file.sync_all())
    {
        return Err(format!("sync directory {}: {error}", parent.display()));
    }
    let reread = std::fs::read(&target).map_err(|error| {
        format!("re-read note {}: {error}", target.display())
    })?;
    if reread != note.contents.as_bytes() {
        return Err(format!(
            "re-read note {} differs from the planned bytes",
            target.display()
        ));
    }
    missing.reverse();
    created.dirs.extend(missing);
    created.files.push(target);
    Ok(())
}

/// Verify the written notes through the rebuilt index and coverage:
/// every planned path must yield a row with no `invalid_yaml`,
/// `opaque_url`, or `missing_type` diagnostic, the row must carry the
/// planned reading state, and coverage must drop by exactly
/// `notes + chapters`.
fn verify_written(
    config: &LibraryConfig,
    plan: &MigrationPlan,
    created: &CreatedPaths,
    coverage_before: usize,
) -> Result<(), String> {
    let index = build_index(config)
        .map_err(|error| format!("rebuild the index: {error}"))?;
    for note in &plan.notes {
        let Some(row) = index.rows.iter().find(|row| row.path == note.path)
        else {
            return Err(format!(
                "verify: planned note has no index row: {}",
                note.path
            ));
        };
        let mut bad: Vec<&str> = row
            .diagnostics
            .iter()
            .map(|diagnostic| diagnostic.code.as_str())
            .filter(|code| {
                *code == "invalid_yaml"
                    || *code == "opaque_url"
                    || *code == "missing_type"
            })
            .collect();
        bad.sort_unstable();
        bad.dedup();
        if !bad.is_empty() {
            return Err(format!(
                "verify: {} has unexpected diagnostics: {}",
                note.path,
                bad.join(", ")
            ));
        }
        if row.reading_state != note.reading_state {
            return Err(format!(
                "verify: {} reading state {} != planned {}",
                note.path, row.reading_state, note.reading_state
            ));
        }
    }
    if created.files.len() != plan.notes.len() {
        return Err(format!(
            "verify: wrote {} file(s) for {} planned note(s)",
            created.files.len(),
            plan.notes.len()
        ));
    }
    let coverage_after =
        count_zorg_records(&config.bob_dir, &config.ref_dir, &index.rows).total;
    let expected =
        coverage_before.saturating_sub(plan.notes.len() + plan.chapters);
    if coverage_after != expected {
        return Err(format!(
            "verify: coverage is {coverage_after}, expected {expected} ({coverage_before} - {} notes - {} chapters)",
            plan.notes.len(),
            plan.chapters
        ));
    }
    Ok(())
}

/// Delete every file this run created, plus any directory it created
/// that is now empty. Best-effort: cleanup must never fail the caller.
fn cleanup_created(created: &CreatedPaths) {
    for file in &created.files {
        let _ = std::fs::remove_file(file);
    }
    let mut dirs = created.dirs.clone();
    dirs.sort();
    dirs.dedup();
    dirs.reverse();
    for dir in &dirs {
        let is_empty = std::fs::read_dir(dir)
            .map(|mut entries| entries.next().is_none())
            .unwrap_or(false);
        if is_empty {
            let _ = std::fs::remove_dir(dir);
        }
    }
}

/// Vault-relative display paths for the scoped commit, sorted.
fn relative_paths(bob_dir: &Path, files: &[PathBuf]) -> Vec<PathBuf> {
    let mut relative: Vec<PathBuf> = files
        .iter()
        .map(|path| {
            path.strip_prefix(bob_dir)
                .map(Path::to_path_buf)
                .unwrap_or_else(|_| path.clone())
        })
        .collect();
    relative.sort();
    relative
}

/// The scoped commit message: one subject line plus by-status counts,
/// top files, and the rollback command.
fn commit_message(plan: &MigrationPlan) -> String {
    let subject = format!(
        "bob ref migrate-zorg: {} records into {} notes under ref/zorg",
        plan.records,
        plan.notes.len()
    );
    let status_line = plan
        .by_status
        .iter()
        .map(|(status, count)| format!("{status} {count}"))
        .collect::<Vec<_>>()
        .join(" · ");
    let files_line = plan
        .by_file
        .iter()
        .take(5)
        .map(|(path, count)| format!("{path} {count}"))
        .collect::<Vec<_>>()
        .join(" · ");
    format!(
        "{subject}\n\nby status: {status_line}\ntop files: {files_line}\nrollback: git revert --no-edit <sha>"
    )
}

fn short_sha(sha: &str) -> String {
    sha.chars().take(7).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    const HUB: &str = "- 250411 250411#0p [[read]] #mcp ID::awesome_mcp_servers ^z-250411-0p\n  | LINKS: [[mcp_ref#^z-250411-0p|model_context_protocol]]\n  * file:: [[lib/docs/awesome_mcp_servers.pdf]]\n  * status:: READ\n  * url:: https://example.com/mcp\n";

    fn vault_fixture(hubs: &[(&str, &str)]) -> tempfile::TempDir {
        let temp = tempfile::tempdir().expect("temporary vault");
        let vault = temp.path();
        for dir in ["lib", "ref", "xlib"] {
            std::fs::create_dir_all(vault.join(dir)).expect("create dir");
        }
        for (name, contents) in hubs {
            let path = vault.join(name);
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).expect("create parent");
            }
            std::fs::write(&path, contents).expect("write hub");
        }
        temp
    }

    fn test_config(vault: &Path) -> LibraryConfig {
        LibraryConfig {
            xlib_dir: vault.join("xlib"),
            bob_dir: vault.to_path_buf(),
            ref_dir: vault.join("ref"),
        }
    }

    fn file_snapshot(vault: &Path) -> BTreeMap<String, Vec<u8>> {
        let mut files = BTreeMap::new();
        let mut stack = vec![vault.to_path_buf()];
        while let Some(dir) = stack.pop() {
            let entries = std::fs::read_dir(&dir).expect("read snapshot dir");
            for entry in entries {
                let entry = entry.expect("dir entry");
                let path = entry.path();
                if path.is_dir() {
                    stack.push(path);
                    continue;
                }
                let name = path
                    .strip_prefix(vault)
                    .expect("under vault")
                    .to_string_lossy()
                    .replace('\\', "/");
                files.insert(
                    name,
                    std::fs::read(&path).expect("read snapshot file"),
                );
            }
        }
        files
    }

    #[test]
    fn write_then_verify_round_trip() {
        let temp = vault_fixture(&[("work_ref.md", HUB)]);
        let vault = temp.path();
        let config = test_config(vault);
        let index = build_index(&config).expect("build index");
        let plan =
            plan_migration(&config.bob_dir, &config.ref_dir, &index.rows);
        assert_eq!(plan.records, 1);
        assert_eq!(plan.notes.len(), 1);
        let coverage_before =
            count_zorg_records(&config.bob_dir, &config.ref_dir, &index.rows)
                .total;
        assert_eq!(coverage_before, 1);

        let created =
            write_planned_notes(&config.bob_dir, &plan.notes).expect("write");
        assert_eq!(created.files.len(), 1);
        let target = vault.join(&plan.notes[0].path);
        assert_eq!(
            std::fs::read_to_string(&target).expect("read note"),
            plan.notes[0].contents,
            "written bytes must equal the planned bytes",
        );
        verify_written(&config, &plan, &created, coverage_before)
            .expect("verify");
    }

    #[test]
    fn verify_failure_deletes_created_files_and_prunes_own_dirs() {
        let temp = vault_fixture(&[("work_ref.md", HUB)]);
        let vault = temp.path();
        let config = test_config(vault);
        // A pre-existing empty directory is not the run's output: cleanup
        // must leave it alone while pruning the hub directory it made.
        std::fs::create_dir_all(vault.join("ref/zorg/old"))
            .expect("pre-existing dir");
        let before = file_snapshot(vault);
        let index = build_index(&config).expect("build index");
        let plan =
            plan_migration(&config.bob_dir, &config.ref_dir, &index.rows);
        let coverage_before =
            count_zorg_records(&config.bob_dir, &config.ref_dir, &index.rows)
                .total;
        let created =
            write_planned_notes(&config.bob_dir, &plan.notes).expect("write");
        assert!(vault.join("ref/zorg/work_ref").is_dir());

        // Break the written note so verification fails on diagnostics.
        std::fs::write(
            vault.join(&plan.notes[0].path),
            "---\n: bad: [\n---\n\n# Broken\n",
        )
        .expect("corrupt note");
        let error = verify_written(&config, &plan, &created, coverage_before)
            .expect_err("verify must fail on the broken note");
        assert!(
            error.contains("invalid_yaml"),
            "unexpected verify error: {error}"
        );

        cleanup_created(&created);
        assert_eq!(
            file_snapshot(vault),
            before,
            "cleanup must restore the pre-write tree",
        );
        assert!(
            !vault.join("ref/zorg/work_ref").exists(),
            "the created hub directory is pruned once empty",
        );
        assert!(
            vault.join("ref/zorg/old").is_dir(),
            "a pre-existing directory is never pruned",
        );
    }

    #[test]
    fn write_refuses_pre_existing_target_and_rolls_back_partial() {
        const DEV_HUB: &str = "- 250115#0k [[read]] ID::dev_second ^z-250115-0k\n  * status:: READ\n  * url:: https://example.com/second\n";
        let temp =
            vault_fixture(&[("work_ref.md", HUB), ("dev_ref.md", DEV_HUB)]);
        let vault = temp.path();
        let config = test_config(vault);
        let index = build_index(&config).expect("build index");
        let plan =
            plan_migration(&config.bob_dir, &config.ref_dir, &index.rows);
        assert_eq!(plan.notes.len(), 2);
        // A file that appears at a planned target after planning (the
        // TOCTOU window the pre-write refuse cannot see) must fail the
        // write without clobbering it, rolling back the notes already
        // written in the same run.
        let clashing = plan
            .notes
            .iter()
            .map(|note| note.path.clone())
            .max()
            .expect("planned note");
        let target = vault.join(&clashing);
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent).expect("create parent");
        }
        std::fs::write(&target, "# mine\n").expect("plant clash");
        let before = file_snapshot(vault);
        let error = write_planned_notes(&config.bob_dir, &plan.notes)
            .expect_err("write must refuse the clashing target");
        assert!(
            error.contains("already exists"),
            "unexpected write error: {error}"
        );
        assert_eq!(
            file_snapshot(vault),
            before,
            "a refused write rolls back its own partial files",
        );
    }

    #[test]
    fn commit_message_names_records_notes_and_rollback() {
        let temp = vault_fixture(&[("work_ref.md", HUB)]);
        let vault = temp.path();
        let config = test_config(vault);
        let index = build_index(&config).expect("build index");
        let plan =
            plan_migration(&config.bob_dir, &config.ref_dir, &index.rows);
        let message = commit_message(&plan);
        let mut lines = message.lines();
        assert_eq!(
            lines.next().expect("subject"),
            "bob ref migrate-zorg: 1 records into 1 notes under ref/zorg",
        );
        assert!(
            message.contains("by status: READ 1"),
            "missing by-status line:\n{message}"
        );
        assert!(
            message.contains("top files: work_ref.md 1"),
            "missing top-files line:\n{message}"
        );
        assert!(
            message.contains("rollback: git revert --no-edit <sha>"),
            "missing rollback command:\n{message}"
        );
    }
}
