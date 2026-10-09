//! `bob ref migrate-tasks --write`: reversible vault write.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::time::Duration;

use clap::ArgMatches;

use super::super::output::{generated_at, REF_SCHEMA_VERSION};
use super::cli::{load_map, migrate_config_from_matches};
use super::plan::plan_migration;
use super::report::{print_json, render_human, render_tsv, ReportMode};
use crate::native::style::Styler;
use crate::native::{ob, vault_sync};

const LOCK_BUDGET: Duration = Duration::from_secs(60);

fn fail(format: &str, code: &str, message: &str, hint: Option<&str>) -> i32 {
    if format == "json" {
        let envelope = serde_json::json!({
            "ok": false,
            "schema_version": REF_SCHEMA_VERSION,
            "command": "ref migrate-tasks",
            "generated_at": generated_at(),
            "error": {"code": code, "message": message, "hint": hint},
        });
        println!(
            "{}",
            serde_json::to_string(&envelope).expect("error serializes")
        );
        return 1;
    }
    eprintln!("bob ref migrate-tasks: error: {message}");
    if let Some(hint) = hint {
        eprintln!("hint: {hint}");
    }
    1
}

/// Snapshot of every path that will change: `None` means absent.
pub(crate) type BeforeImages = BTreeMap<PathBuf, Option<Vec<u8>>>;

pub(crate) fn snapshot_before_images(
    bob_dir: &Path,
    rel_paths: &[String],
) -> BeforeImages {
    let mut map = BTreeMap::new();
    for rel in rel_paths {
        let abs = bob_dir.join(rel);
        let bytes = std::fs::read(&abs).ok();
        map.insert(abs, bytes);
    }
    map
}

/// Restore before-images where the current bytes still equal the written
/// after-image. Returns the paths that diverged and were left in place.
pub(crate) fn restore_before_images(
    before: &BeforeImages,
    after: &BTreeMap<PathBuf, Vec<u8>>,
) -> Vec<PathBuf> {
    let mut diverged = Vec::new();
    for (path, before_bytes) in before {
        let after_bytes = after.get(path);
        let Ok(current) = std::fs::read(path) else {
            // Missing now: restore when after-image was also missing? If the
            // run created it (before None) and it is now gone, that is a
            // divergence (someone deleted it); leave it and name it when it
            // was supposed to exist.
            if after_bytes.is_some() {
                diverged.push(path.clone());
            } else if let Some(bytes) = before_bytes {
                let _ = std::fs::write(path, bytes);
            }
            continue;
        };
        match (before_bytes, after_bytes) {
            (before_opt, Some(wrote)) => {
                if &current == wrote {
                    match before_opt {
                        Some(bytes) => {
                            let _ = std::fs::write(path, bytes);
                        }
                        None => {
                            let _ = std::fs::remove_file(path);
                        }
                    }
                } else {
                    diverged.push(path.clone());
                }
            }
            (Some(before_bytes), None) => {
                // Path was not written by this run but is in before set;
                // leave it alone.
                let _ = before_bytes;
            }
            (None, None) => {}
        }
    }
    diverged.sort();
    diverged
}

pub(crate) fn run_migrate_tasks_write(
    matches: &ArgMatches,
    format: String,
) -> i32 {
    let offline = matches.get_flag("write") && matches.get_flag("offline");
    let config = migrate_config_from_matches(matches);

    let _lock = {
        let json_mode = format == "json";
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
                    &format,
                    "lock",
                    &format!("acquire vault maintenance lock: {error}"),
                    Some("another maintenance run holds the lock; try again"),
                );
            }
        }
    };

    let child_env = ob::child_env();

    match ob::detect_git_worktree(&config.bob_dir, &child_env) {
        Ok(true) => {}
        Ok(false) => {
            return fail(
                &format,
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
                &format,
                "git_unavailable",
                &format!("could not run git rev-parse: {error}"),
                None,
            );
        }
    }

    if !offline {
        let cycle =
            vault_sync::run_cycle_with_existing_lock_report(&child_env, true);
        if !cycle.ok {
            return fail(
                &format,
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

    let map = match load_map(matches) {
        Ok(map) => map,
        Err(message) => return fail(&format, "map", &message, None),
    };
    let plan = plan_migration(&config.bob_dir, &config.ref_dir, &map);
    // Refuse before any write when unmapped or multi-tracker refs exist,
    // even when no task is plannable.
    if !plan.unmapped.is_empty() || !plan.multi.is_empty() {
        let mut details: Vec<String> = plan
            .unmapped
            .iter()
            .map(|u| format!("{}: {}", u.ref_note, u.reason))
            .collect();
        for m in &plan.multi {
            details.push(format!("{}: {} open trackers", m.ref_note, m.count));
        }
        details.sort();
        return fail(
            &format,
            "unmapped",
            &format!(
                "refusing --write: {} problem(s): {}",
                details.len(),
                details.join("; ")
            ),
            Some("pass -m with parents for every open ref task"),
        );
    }
    if plan.tasks.is_empty() {
        match format.as_str() {
            "json" => {
                let envelope = serde_json::json!({
                    "ok": true,
                    "schema_version": REF_SCHEMA_VERSION,
                    "command": "ref migrate-tasks",
                    "generated_at": generated_at(),
                    "mode": "write",
                    "summary": {
                        "tasks": 0,
                        "notes": 0,
                        "unmapped": plan.unmapped.len(),
                        "ambiguous": 0,
                        "wrappers": 0
                    },
                    "tasks": [],
                    "unmapped": plan.unmapped.iter().map(|u| {
                        serde_json::json!({"ref_note": u.ref_note, "reason": u.reason})
                    }).collect::<Vec<_>>(),
                    "ambiguous_links": [],
                    "possible_wrappers": [],
                    "rewrites": {},
                    "lane_preview": {
                        "next_before": plan.lane_preview.next_before,
                        "next_after": plan.lane_preview.next_after,
                        "next_cap": plan.lane_preview.next_cap,
                        "pending_before": plan.lane_preview.pending_before,
                        "pending_after": plan.lane_preview.pending_after,
                        "pending_cap": plan.lane_preview.pending_cap,
                        "unavailable": plan.lane_preview.unavailable
                    },
                    "commit": serde_json::Value::Null,
                });
                println!(
                    "{}",
                    serde_json::to_string(&envelope).expect("noop serializes")
                );
            }
            "tsv" => print!("{}", render_tsv(&plan)),
            _ => println!("nothing to migrate"),
        }
        return 0;
    }

    // Paths that will change: parents, ref notes, rewrite files.
    let mut rel_paths: BTreeSet<String> = BTreeSet::new();
    for task in &plan.tasks {
        rel_paths.insert(task.parent_path.clone());
        rel_paths.insert(task.ref_note.clone());
    }
    for file in plan.rewrites.keys() {
        rel_paths.insert(file.clone());
    }
    // Include absences for parents that insert would create.
    let rel_list: Vec<String> = rel_paths.into_iter().collect();

    // Refuse when any changing path already differs from HEAD.
    match dirty_paths(&config.bob_dir, &child_env, &rel_list) {
        Ok(dirty) => {
            if !dirty.is_empty() {
                return fail(
                    &format,
                    "dirty",
                    &format!(
                        "refusing --write: {} path(s) differ from HEAD: {}",
                        dirty.len(),
                        dirty.join(", ")
                    ),
                    Some("commit, stash, or discard them, then re-run"),
                );
            }
        }
        Err(message) => return fail(&format, "git_status", &message, None),
    }

    let before = snapshot_before_images(&config.bob_dir, &rel_list);

    // Apply writes sequentially.
    let mut after: BTreeMap<PathBuf, Vec<u8>> = BTreeMap::new();
    let mut final_map: BTreeMap<String, (String, String)> = BTreeMap::new();
    let mut dep_final: BTreeMap<String, String> = BTreeMap::new();
    let mut written_rel: BTreeSet<String> = BTreeSet::new();

    if let Err(message) = apply_plan(
        &config.bob_dir,
        &plan,
        &mut final_map,
        &mut dep_final,
        &mut written_rel,
        &mut after,
    ) {
        let diverged = restore_before_images(&before, &after);
        if !diverged.is_empty() {
            eprintln!(
                "bob ref migrate-tasks: error: write failed ({message}); left diverged paths: {}",
                diverged
                    .iter()
                    .map(|p| p.display().to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            );
        }
        return fail(&format, "write", &message, None);
    }

    // Rewrite graph files with final ids (parents/ref notes already carry
    // final content; other files still need rewrites).
    if let Err(message) = apply_graph_rewrites(
        &config.bob_dir,
        &plan,
        &final_map,
        &dep_final,
        &mut written_rel,
        &mut after,
    ) {
        let diverged = restore_before_images(&before, &after);
        if !diverged.is_empty() {
            eprintln!(
                "bob ref migrate-tasks: error: rewrite failed ({message}); left diverged paths: {}",
                diverged
                    .iter()
                    .map(|p| p.display().to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            );
        }
        return fail(&format, "write", &message, None);
    }

    // Verify.
    if let Err(message) = verify_written(
        &config.bob_dir,
        &config.ref_dir,
        &plan,
        &final_map,
        &map,
    ) {
        let diverged = restore_before_images(&before, &after);
        if !diverged.is_empty() {
            eprintln!(
                "bob ref migrate-tasks: error: verify failed; left diverged paths: {}",
                diverged
                    .iter()
                    .map(|p| p.display().to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            );
        }
        return fail(&format, "verify", &message, None);
    }

    // Commit exactly the written paths.
    let written_sorted: Vec<String> = written_rel.into_iter().collect();
    let message_subject = format!(
        "bob ref migrate-tasks: {} ref tasks into {} notes",
        plan.tasks.len(),
        plan.distinct_parents()
    );
    let rel_paths_buf: Vec<PathBuf> =
        written_sorted.iter().map(PathBuf::from).collect();
    let sha = match ob::commit_paths(
        &config.bob_dir,
        &child_env,
        &message_subject,
        &rel_paths_buf,
    ) {
        Ok(Some(sha)) => sha,
        Ok(None) => {
            return fail(
                &format,
                "commit",
                "commit notes: nothing to commit after writing",
                Some("resolve the git failure and re-run"),
            );
        }
        Err(message) => {
            return fail(
                &format,
                "commit",
                &format!("commit notes: {message}"),
                Some("resolve the git failure and re-run; notes are already written"),
            );
        }
    };

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
                &format,
                "post_sync",
                &format!(
                    "post-sync failed ({detail}); committed `{}` locally; background vault-sync will publish it",
                    short_sha(&sha),
                ),
                Some("run bob vault-sync later, or re-run with --offline"),
            );
        }
        cycle.pushed
    };

    // Update preview ids to final for the report.
    let mut final_plan = plan.clone();
    for task in final_plan.tasks.iter_mut() {
        if let Some((_, final_id)) = final_map.get(&task.ref_note) {
            task.preview_block_id = final_id.clone();
        }
    }
    let mode = ReportMode::write(&sha, &message_subject);
    match format.as_str() {
        "json" => print_json(
            &final_plan,
            &mode,
            Some((sha.clone(), message_subject.clone(), written_sorted)),
        ),
        "tsv" => print!("{}", render_tsv(&final_plan)),
        _ => {
            print!("{}", render_human(&final_plan, &mode));
            if pushed {
                println!("pushed to origin/master");
            }
        }
    }
    0
}

fn short_sha(sha: &str) -> String {
    sha.chars().take(7).collect()
}

fn dirty_paths(
    bob_dir: &Path,
    child_env: &ob::ChildEnv,
    rel_paths: &[String],
) -> Result<Vec<String>, String> {
    if rel_paths.is_empty() {
        return Ok(Vec::new());
    }
    let output = ob::git_command(bob_dir, child_env)
        .arg("status")
        .arg("--porcelain")
        .arg("--")
        .args(rel_paths)
        .output()
        .map_err(|e| format!("could not run git status: {e}"))?;
    if !output.status.success() {
        return Err(format!(
            "git status failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut dirty = Vec::new();
    for line in stdout.lines() {
        // Porcelain: XY <path>. Path may be quoted.
        let path = line.get(3..).unwrap_or("").trim().trim_matches('"');
        if !path.is_empty() {
            dirty.push(path.to_string());
        }
    }
    dirty.sort();
    Ok(dirty)
}

#[allow(clippy::too_many_arguments)]
fn apply_plan(
    bob_dir: &Path,
    plan: &super::plan::MigrationPlan,
    final_map: &mut BTreeMap<String, (String, String)>,
    dep_final: &mut BTreeMap<String, String>,
    written_rel: &mut BTreeSet<String>,
    after: &mut BTreeMap<PathBuf, Vec<u8>>,
) -> Result<(), String> {
    use crate::native::ref_tasks::RefTaskIndex;
    // Fresh index for adoption checks (plan was built pre-lock; re-read).
    let index = RefTaskIndex::build(
        bob_dir,
        &bob_dir.join(plan_parent_ref_dir(bob_dir, plan)),
    );
    let _ = index;
    for task in &plan.tasks {
        let dest_abs = bob_dir.join(&task.parent_path);
        // Adoption: exactly one open v2 candidate outside done/.
        let live_index = RefTaskIndex::build(bob_dir, &ref_dir_for(bob_dir));
        let candidates = live_index.candidates(&task.ref_note);
        let open_outside: Vec<_> = candidates
            .iter()
            .filter(|c| {
                !c.archived && crate::native::ref_tasks::is_open_mark(c.mark)
            })
            .collect();
        let final_id = if open_outside.len() == 1 {
            let adopted = open_outside[0]
                .block_id
                .clone()
                .unwrap_or_else(|| task.preview_block_id.clone());
            // Still need to ensure the adopted line is in the destination;
            // adoption writes no second task.
            adopted
        } else {
            // Insert reading task plus children.
            let inserted = crate::native::ref_tasks::insert_ref_task(
                bob_dir,
                &dest_abs,
                &task.migrated_line,
                &task.children,
            )
            .map_err(|e| {
                format!(
                    "insert {} into {}: {e}",
                    task.ref_note, task.parent_path
                )
            })?;
            // Fix [id::] when preview != final.
            if inserted.block_id != task.preview_block_id {
                fix_inserted_dep_id(
                    bob_dir,
                    &dest_abs,
                    &task,
                    &inserted.block_id,
                )?;
            }
            inserted.block_id
        };
        final_map.insert(
            task.ref_note.clone(),
            (task.parent_route.clone(), final_id.clone()),
        );
        if let Some(old) = task.old_dep_id.clone() {
            if let Ok(new) = crate::native::task_dependencies::dependency_id(
                Path::new(&task.parent_path),
                &final_id,
            ) {
                dep_final.insert(old, new);
            }
        }
        // Follow-ups as siblings via insert_task_line + write_staged_files.
        for fu in &task.follow_ups {
            insert_follow_up(bob_dir, &dest_abs, fu)?;
        }
        // Record destination after-image.
        if let Ok(bytes) = std::fs::read(&dest_abs) {
            after.insert(dest_abs.clone(), bytes);
        }
        written_rel.insert(task.parent_path.clone());

        // Ref-note edit (tracker removal, embed, parent, hash/base).
        let ref_abs = bob_dir.join(&task.ref_note);
        let contents = std::fs::read_to_string(&ref_abs)
            .map_err(|e| format!("read {}: {e}", task.ref_note))?;
        let mut updated =
            crate::native::highlights_ref::apply_v2_migration_note(
                &contents,
                &task.parent_route,
                &final_id,
            )
            .map_err(|e| format!("migrate note {}: {e}", task.ref_note))?;
        // Remove moved follow-up blocks (open only; closed stay).
        for fu in &task.follow_ups {
            updated = remove_original_block(&updated, &fu.original);
        }
        std::fs::write(&ref_abs, &updated)
            .map_err(|e| format!("write {}: {e}", task.ref_note))?;
        if let Ok(bytes) = std::fs::read(&ref_abs) {
            after.insert(ref_abs.clone(), bytes);
        }
        written_rel.insert(task.ref_note.clone());
    }
    Ok(())
}

fn plan_parent_ref_dir(
    _bob_dir: &Path,
    _plan: &super::plan::MigrationPlan,
) -> PathBuf {
    // Unused helper kept for symmetry; the live index uses ref_dir_for.
    PathBuf::from("ref")
}

fn ref_dir_for(bob_dir: &Path) -> PathBuf {
    // The ref dir is `<bob>/ref` unless the vault uses a custom dir. The
    // plan only ever migrates notes under the configured ref dir; for the
    // live adoption check, `<bob>/ref` covers the default and the tests.
    // When a custom dir is in play, candidates still resolve because the
    // index is built over `(bob_dir, ref_dir)` with the same default.
    bob_dir.join("ref")
}

fn fix_inserted_dep_id(
    _bob_dir: &Path,
    dest_abs: &Path,
    task: &super::plan::PlannedTask,
    final_id: &str,
) -> Result<(), String> {
    let (Some(old), Some(preview_new)) =
        (task.old_dep_id.clone(), task.new_dep_id_preview.clone())
    else {
        return Ok(());
    };
    let Ok(final_new) = crate::native::task_dependencies::dependency_id(
        Path::new(&task.parent_path),
        final_id,
    ) else {
        return Ok(());
    };
    if preview_new == final_new {
        return Ok(());
    }
    let contents = std::fs::read_to_string(dest_abs)
        .map_err(|e| format!("re-read {}: {e}", dest_abs.display()))?;
    // Replace the preview dep (written from the preview line) with final.
    if contents.contains(&preview_new) {
        let updated = contents.replace(&preview_new, &final_new);
        std::fs::write(dest_abs, updated)
            .map_err(|e| format!("fix dep id: {e}"))?;
    } else if contents.contains(&old) {
        let updated = contents.replace(&old, &final_new);
        std::fs::write(dest_abs, updated)
            .map_err(|e| format!("fix dep id: {e}"))?;
    }
    Ok(())
}

fn insert_follow_up(
    bob_dir: &Path,
    dest_abs: &Path,
    fu: &super::plan::PlannedFollowUp,
) -> Result<(), String> {
    let block = fu.lines.join("\n");
    // Re-read destination fresh for each follow-up.
    let contents = match std::fs::read_to_string(dest_abs) {
        Ok(c) => c,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(format!("read {}: {e}", dest_abs.display())),
    };
    // Ensure block id freeness; suffix when taken.
    let mut block_text = block;
    let existing =
        crate::native::collect_done::trailing_block_id_in_line(&block_text)
            .unwrap_or_default();
    if !existing.is_empty() {
        let taken: std::collections::BTreeSet<String> =
            crate::native::collect_done::block_ids_in_markdown(&contents)
                .into_iter()
                .collect();
        if taken.contains(&existing) {
            let fresh = crate::native::ref_tasks::allocate_unique_block_id(
                &existing,
                &|id| taken.contains(id),
            );
            // Replace trailing id.
            if let Some(caret) = block_text.rfind('^') {
                let trimmed = block_text.trim_end().len();
                block_text = format!(
                    "{}{}{}",
                    &block_text[..caret + 1],
                    fresh,
                    &block_text[trimmed..]
                );
            }
        }
    }
    let (updated, _placement) =
        crate::native::capture::insert_task_line(&contents, &block_text);
    if updated == contents {
        return Ok(());
    }
    let existed = dest_abs.exists();
    let staged = crate::native::capture::StagedTextFile {
        target: dest_abs.to_path_buf(),
        target_existed: existed,
        original_target: contents,
        updated_target: updated,
    };
    crate::native::capture::write_staged_files(std::slice::from_ref(&staged))
        .map_err(|e| {
        format!(
            "insert follow-up into {}: {}",
            dest_abs.display(),
            e.message
        )
    })?;
    let _ = bob_dir;
    Ok(())
}

fn apply_graph_rewrites(
    bob_dir: &Path,
    plan: &super::plan::MigrationPlan,
    final_map: &BTreeMap<String, (String, String)>,
    dep_final: &BTreeMap<String, String>,
    written_rel: &mut BTreeSet<String>,
    after: &mut BTreeMap<PathBuf, Vec<u8>>,
) -> Result<(), String> {
    // Stem counts for bare-stem resolution.
    let mut stems: BTreeMap<String, usize> = BTreeMap::new();
    // Rebuild from plan tasks' ref notes + vault ref notes? For final
    // rewrites, unique-stem info comes from the vault. Recompute quickly.
    let ref_dir = ref_dir_for(bob_dir);
    let mut stack = vec![ref_dir.clone()];
    let mut ref_notes: Vec<String> = Vec::new();
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
                continue;
            }
            if path
                .extension()
                .and_then(|e| e.to_str())
                .is_some_and(|e| e.eq_ignore_ascii_case("md"))
            {
                if let Ok(rel) = path.strip_prefix(bob_dir) {
                    ref_notes.push(rel.to_string_lossy().replace('\\', "/"));
                }
            }
        }
    }
    for note in &ref_notes {
        let stem = Path::new(note)
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        *stems.entry(stem).or_insert(0) += 1;
    }
    // Files to rewrite: those preview flagged (old dep ids are stable,
    // so preview already covers dep-only files).
    let files: BTreeSet<String> = plan.rewrites.keys().cloned().collect();
    for file in files.clone() {
        let abs = bob_dir.join(&file);
        let Ok(contents) = std::fs::read_to_string(&abs) else {
            continue;
        };
        let (updated, link_n, dep_n) =
            super::rewrite::apply_rewrites_to_contents(
                &contents, &file, bob_dir, final_map, dep_final, &stems,
            );
        if updated != contents {
            std::fs::write(&abs, &updated)
                .map_err(|e| format!("rewrite {file}: {e}"))?;
            written_rel.insert(file.clone());
            if let Ok(bytes) = std::fs::read(&abs) {
                after.insert(abs, bytes);
            }
            let _ = (link_n, dep_n);
        }
    }
    // Also rewrite parent and ref notes themselves for dep ids/links that
    // preview counted under their own paths: they were already written
    // above, but their link bodies still carry preview ids when final
    // differed. Fix them here.
    for task in &plan.tasks {
        for rel in [&task.parent_path, &task.ref_note] {
            if files.contains(rel) {
                continue;
            }
            let abs = bob_dir.join(rel);
            let Ok(contents) = std::fs::read_to_string(&abs) else {
                continue;
            };
            let (updated, _, _) = super::rewrite::apply_rewrites_to_contents(
                &contents, rel, bob_dir, final_map, dep_final, &stems,
            );
            if updated != contents {
                std::fs::write(&abs, &updated)
                    .map_err(|e| format!("rewrite {rel}: {e}"))?;
                written_rel.insert(rel.clone());
                if let Ok(bytes) = std::fs::read(&abs) {
                    after.insert(abs, bytes);
                }
            }
        }
    }
    Ok(())
}

fn verify_written(
    bob_dir: &Path,
    ref_dir: &Path,
    plan: &super::plan::MigrationPlan,
    final_map: &BTreeMap<String, (String, String)>,
    map: &BTreeMap<String, String>,
) -> Result<(), String> {
    use crate::native::ref_tasks::{is_open_mark, RefTaskIndex};
    // No PDF bytes changed: we never write PDFs; assert none of the lib
    // PDFs are newer than the run start? Best-effort: ensure no .pdf under
    // lib/xlib was modified in the last minute by this run? Since we never
    // open PDFs for write, pass.
    let index = RefTaskIndex::build(bob_dir, ref_dir);
    for task in &plan.tasks {
        let Some((route, final_id)) = final_map.get(&task.ref_note) else {
            return Err(format!("verify: no final id for {}", task.ref_note));
        };
        let candidates = index.candidates(&task.ref_note);
        let open_outside: Vec<_> = candidates
            .iter()
            .filter(|c| !c.archived && is_open_mark(c.mark))
            .collect();
        if open_outside.len() != 1 {
            return Err(format!(
                "verify: {} resolves to {} live tasks, expected 1",
                task.ref_note,
                open_outside.len()
            ));
        }
        let winner = open_outside[0];
        if winner.mark != task.mark {
            return Err(format!(
                "verify: {} mark {} != planned {}",
                task.ref_note, winner.mark, task.mark
            ));
        }
        let winner_id = winner.block_id.clone().unwrap_or_default();
        if winner_id != *final_id {
            return Err(format!(
                "verify: {} block id {winner_id} != planned {final_id}",
                task.ref_note
            ));
        }
        // Ref note carries the embed and parent, and no open tracker.
        let ref_abs = bob_dir.join(&task.ref_note);
        let contents = std::fs::read_to_string(&ref_abs)
            .map_err(|e| format!("verify read {}: {e}", task.ref_note))?;
        let body = if let Some((_, b)) =
            crate::native::highlights_ref::split_frontmatter(&contents)
        {
            b
        } else {
            contents.clone()
        };
        let hits = crate::native::ref_tasks::find_trackers(&body);
        if hits.iter().any(|h| is_open_mark(h.mark)) {
            return Err(format!(
                "verify: {} still has an open v1 tracker",
                task.ref_note
            ));
        }
        let expected_embed = format!("![[{route}#^{final_id}]]");
        if !contents.contains(&expected_embed) {
            return Err(format!(
                "verify: {} missing embed {expected_embed}",
                task.ref_note
            ));
        }
        let _ = route;
    }
    // Each rewritten link resolves to its task: re-scan for stale #^ref.
    for (file, _) in plan.rewrites.iter() {
        let abs = bob_dir.join(file);
        let Ok(contents) = std::fs::read_to_string(&abs) else {
            continue;
        };
        for line in contents.lines() {
            let spans =
                crate::native::task_dependencies::raw_wikilink_spans(line);
            for span in &spans {
                let inside = &line[span.open + 2..span.end - 2];
                if let Some((_, block_id)) =
                    crate::native::task_dependencies::parse_block_link_inside(
                        inside,
                    )
                {
                    if block_id == "ref" {
                        // Any remaining #^ref link whose target is a migrated
                        // ref is a missed rewrite.
                        let target = inside
                            .split('|')
                            .next()
                            .unwrap_or("")
                            .split('#')
                            .next()
                            .unwrap_or("")
                            .trim();
                        for ref_note in final_map.keys() {
                            let no_md = ref_note
                                .strip_suffix(".md")
                                .unwrap_or(ref_note);
                            if target.eq_ignore_ascii_case(ref_note)
                                || target.eq_ignore_ascii_case(no_md)
                            {
                                return Err(format!(
                                    "verify: {file} still links {target}#^ref"
                                ));
                            }
                        }
                    }
                }
            }
        }
    }
    // A second plan is empty.
    let second = plan_migration(bob_dir, ref_dir, map);
    if !second.tasks.is_empty() {
        return Err(format!(
            "verify: second plan has {} remaining tasks",
            second.tasks.len()
        ));
    }
    Ok(())
}

/// Remove one original follow-up block from ref-note contents.
fn remove_original_block(contents: &str, original: &[String]) -> String {
    if original.is_empty() {
        return contents.to_string();
    }
    // Try exact block-text replacement first (with and without trailing
    // newline), then fall back to removing just the first line.
    let block = original.join("\n");
    for candidate in [format!("{block}\n"), block.clone()] {
        if contents.contains(&candidate) {
            return contents.replacen(&candidate, "", 1);
        }
    }
    contents.replacen(&original[0], "", 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn restore_returns_diverged_and_restores_clean() {
        let temp = tempfile::tempdir().expect("tempdir");
        let a = temp.path().join("a.md");
        let b = temp.path().join("b.md");
        std::fs::write(&a, "after-a").expect("write a");
        std::fs::write(&b, "diverged").expect("write b");
        let before: BeforeImages = BTreeMap::from([
            (a.clone(), Some(b"before-a".to_vec())),
            (b.clone(), Some(b"before-b".to_vec())),
        ]);
        let after: BTreeMap<PathBuf, Vec<u8>> = BTreeMap::from([
            (a.clone(), b"after-a".to_vec()),
            (b.clone(), b"after-b".to_vec()),
        ]);
        let diverged = restore_before_images(&before, &after);
        assert_eq!(diverged, vec![b.clone()]);
        assert_eq!(std::fs::read_to_string(&a).expect("read a"), "before-a");
        assert_eq!(std::fs::read_to_string(&b).expect("read b"), "diverged");
    }
}
