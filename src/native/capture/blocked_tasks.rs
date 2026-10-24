//! `*` blocked-task target resolution, A8 guards, and execute passes.

use super::*;
use crate::native::{
    capture_language::ParsedDependency,
    collect_done::trailing_block_id_in_line,
    task_dependencies::{self, parse as dependency_parse},
    task_status_hooks,
};
use std::collections::BTreeSet;

/// Vault-relative path for a typed `*note` identity: append `.md`
/// unless the last segment already has an extension.
fn typed_note_as_relative(typed: &str) -> PathBuf {
    if typed
        .rsplit('/')
        .next()
        .is_some_and(|file| file.contains('.'))
    {
        PathBuf::from(typed)
    } else {
        PathBuf::from(format!("{typed}.md"))
    }
}

fn star_token(typed: &str, block_id: &str) -> String {
    format!("*{typed}:{block_id}")
}

/// A8 path refusals: daily notes, `done/`, `_templates` /
/// `_generated` / `_conflicts`, and dot-directories. Checked against
/// the typed spelling before catalog resolve so skipped dirs still
/// teach, and again against the resolved path.
fn refuse_blocked_task_path(
    typed: &str,
    block_id: &str,
    relative: &Path,
) -> Result<(), CaptureError> {
    let display = display_relative(relative);
    let star = star_token(typed, block_id);
    if task_status_hooks::canonical_daily_date(relative).is_some() {
        return Err(CaptureError::usage(format!(
            "cannot block {star}: {display} is a daily note; capture cannot block tasks there"
        )));
    }
    if task_dependencies::is_archive_path(relative) {
        return Err(CaptureError::usage(format!(
            "cannot block {star}: {display} is under done/; archived tasks cannot be blocked by capture"
        )));
    }
    for component in relative.components() {
        let os_name = component.as_os_str();
        let Some(name) = os_name.to_str() else {
            continue;
        };
        if crate::native::is_always_excluded_note_directory_name(os_name)
            && matches!(name, "_templates" | "_generated" | "_conflicts")
        {
            return Err(CaptureError::usage(format!(
                "cannot block {star}: {display} is under {name}/, which is not an editable blocked-task target"
            )));
        }
        if name.starts_with('.') {
            return Err(CaptureError::usage(format!(
                "cannot block {star}: {display} is in a dot-directory; capture cannot block tasks there"
            )));
        }
    }
    Ok(())
}

fn block_id_in_fenced_code(contents: &str, block_id: &str) -> bool {
    let lines: Vec<&str> = contents.lines().collect();
    let fenced = task_status_hooks::fenced_lines(&lines, 0..lines.len());
    fenced.iter().any(|line_index| {
        lines.get(*line_index).is_some_and(|line| {
            trailing_block_id_in_line(line).as_deref() == Some(block_id)
        })
    })
}

/// Resolve a typed `*note:block-id` like `&note`, then enforce every
/// A8 refusal with a teaching error that names `*`.
pub(super) fn resolve_blocked_task_target(
    ctx: &mut DependencyContext,
    planner: &mut CaptureBatchPlanner,
    bob_dir: &Path,
    typed_note: &str,
    block_id: &str,
    owner: Option<&DependentRef>,
) -> Result<DependentRef, CaptureError> {
    let star = star_token(typed_note, block_id);
    refuse_blocked_task_path(
        typed_note,
        block_id,
        &typed_note_as_relative(typed_note),
    )?;
    let relative =
        resolve_prerequisite_note(ctx, planner, bob_dir, typed_note)?;
    refuse_blocked_task_path(typed_note, block_id, &relative)?;
    let absolute = absolute_note(bob_dir, &relative);
    let contents = current_note_text(planner, &absolute)?;
    let scan = note_tasks::scan(&contents, ctx.settings());
    let label = display_relative(&relative);
    let task = match lookup_staged_task(&scan, &label, block_id) {
        Ok(task) => task,
        Err(error) => {
            if block_id_in_fenced_code(&contents, block_id) {
                return Err(CaptureError::usage(format!(
                    "cannot block {star}: ^{block_id} lives inside a fenced code block and is not an editable task"
                )));
            }
            return Err(error);
        }
    };
    let task_line_index = task.line_index;
    let task_status_type = task.status_type;
    let task_status_name = task.status_name.clone();
    if task_status_type.is_terminal() || !task_status_type.is_open() {
        return Err(CaptureError::usage(format!(
            "cannot block {star}: the task is {task_status_name}; capture cannot block a completed task"
        )));
    }
    if let Some(owner) = owner
        && display_relative(&owner.relative) == label
        && owner.block_id == block_id
    {
        return Err(CaptureError::usage(format!(
            "cannot block {star}: that is the owner task itself; a task cannot block itself"
        )));
    }
    let lines: Vec<&str> = contents.lines().collect();
    let fenced = task_status_hooks::fenced_lines(&lines, 0..lines.len());
    let managed = managed_children(&lines, &fenced, task_line_index);
    if managed.len() > 1 {
        return Err(CaptureError::usage(format!(
            "cannot block {star}: {count} managed Depends-On lines; keep one canonical '- ⛓️ **DEPENDS ON:** ...' child line, then capture again",
            count = managed.len()
        )));
    }
    if let Some((line_index, dependency_parse::DependencyLine::Malformed)) =
        managed.first()
    {
        return Err(CaptureError::usage(format!(
            "cannot block {star}: the managed Depends-On line (line {}) is malformed; repair it to a single '- ⛓️ **DEPENDS ON:** [[...]]' child line or remove it, then capture again",
            line_index + 1
        )));
    }
    if is_prj_dependent(block_id)
        && let Some(owner) = owner
    {
        let index =
            super::dependencies::staged_note_index(ctx, planner, bob_dir);
        let dep_desc = format!("^{block_id} in {label}");
        refuse_internal_prj_targets(
            &relative,
            &dep_desc,
            &[ResolvedPrerequisite::identity(
                owner.relative.clone(),
                owner.block_id.clone(),
            )],
            &index,
            ctx.settings(),
            |owner_relative| {
                let owner_absolute = absolute_note(bob_dir, owner_relative);
                planner
                    .current_contents(&owner_absolute)
                    .ok()
                    .flatten()
                    .or_else(|| std::fs::read_to_string(owner_absolute).ok())
            },
        )
        .map_err(|error| {
            CaptureError::usage(format!(
                "cannot block {star}: {}",
                error.message
            ))
        })?;
    }
    Ok(DependentRef {
        relative,
        block_id: block_id.to_string(),
    })
}

/// N as a prerequisite of T: the existing-dependent writer resolves
/// `note` vault-wide, including notes this batch just created.
fn blocker_as_dependency(owner: &DependentRef) -> ParsedDependency {
    let note = route_without_extension(&owner.relative);
    ParsedDependency {
        raw: format!("&{note}:{}", owner.block_id),
        note,
        block_id: owner.block_id.clone(),
        quoted: false,
    }
}

/// Fold `&` then `*` summaries so the singular field equals the first
/// entry and `&`-only items still carry the array.
pub(super) fn fold_dependency_updates(
    updates: Vec<DependencyUpdateJson>,
) -> (Option<DependencyUpdateJson>, Vec<DependencyUpdateJson>) {
    (updates.first().cloned(), updates)
}

pub(super) fn absorb_dependency_write(
    update: PlannedDependencyUpdate,
    summaries: &mut Vec<DependencyUpdateJson>,
    refs: &mut Vec<TaskBlockRef>,
    pomodoro_refs: &mut Vec<PomodoroBlockRef>,
    blocks_line_updates: &mut Vec<BlocksLineUpdateJson>,
) {
    refs.push(update.dependent_ref);
    refs.extend(update.target_refs);
    pomodoro_refs.extend(update.pomodoro_refs);
    blocks_line_updates.extend(update.blocks.updates);
    summaries.push(update.summary);
}

/// Refuse a Done or Canceled `*` owner before any blocked-task write.
pub(super) fn refuse_closed_star_owner(
    ctx: &DependencyContext,
    planner: &mut CaptureBatchPlanner,
    bob_dir: &Path,
    owner: &DependentRef,
) -> Result<(), CaptureError> {
    let absolute = absolute_note(bob_dir, &owner.relative);
    let contents = current_note_text(planner, &absolute)?;
    let scan = note_tasks::scan(&contents, ctx.settings());
    let label = display_relative(&owner.relative);
    let task = lookup_staged_task(&scan, &label, &owner.block_id)?;
    if task.status_type.is_terminal() || !task.status_type.is_open() {
        return Err(CaptureError::usage(format!(
            "task ^{} in {label} is {}; a finished task cannot block another task (reopen it first)",
            owner.block_id, task.status_name,
        )));
    }
    Ok(())
}

/// One existing-dependent writer pass per typed `*` target, in order.
/// Repeats are idempotent `already_present` no-ops. Relocate N by ID
/// after each pass so later targets see lane/`[id::]` edits.
pub(super) fn plan_blocked_task_writes(
    ctx: &mut DependencyContext,
    planner: &mut CaptureBatchPlanner,
    bob_dir: &Path,
    owner: &DependentRef,
    blocked_tasks: &[ParsedDependency],
    warnings: &mut Vec<CaptureWarning>,
    options: ExistingDependencyOptions,
) -> Result<Vec<PlannedDependencyUpdate>, CaptureError> {
    refuse_closed_star_owner(ctx, planner, bob_dir, owner)?;
    let mut out = Vec::with_capacity(blocked_tasks.len());
    let mut seen = BTreeSet::new();
    for typed in blocked_tasks {
        let target = resolve_blocked_task_target(
            ctx,
            planner,
            bob_dir,
            &typed.note,
            &typed.block_id,
            Some(owner),
        )?;
        let identity =
            (display_relative(&target.relative), target.block_id.clone());
        if !seen.insert(identity) {
            continue;
        }
        let prereq = blocker_as_dependency(owner);
        let update = plan_existing_task_dependencies(
            ctx,
            planner,
            bob_dir,
            &target,
            &[prereq],
            warnings,
            options,
        )?;
        out.push(update);
        let _ = locate_task_by_block_id(
            ctx,
            planner,
            bob_dir,
            &owner.relative,
            &owner.block_id,
        )?;
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::native::capture_language::ParsedDependency;
    use crate::native::env::TestEnvGuard;
    use std::fs;

    fn today() -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 10, 14).expect("date")
    }

    fn write_note(root: &Path, relative: &str, contents: &str) {
        let path = root.join(relative);
        fs::create_dir_all(path.parent().expect("parent")).expect("parent");
        fs::write(path, contents).expect("write note");
    }

    fn write_settings(root: &Path) {
        write_note(
            root,
            ".obsidian/plugins/obsidian-tasks-plugin/data.json",
            r##"{
              "globalFilter": "#task",
              "statusSettings": {
                "coreStatuses": [
                  {"symbol":" ","name":"Ready","nextStatusSymbol":"x","availableAsCommand":true,"type":"TODO"},
                  {"symbol":"x","name":"Done","nextStatusSymbol":" ","availableAsCommand":true,"type":"DONE"}
                ],
                "customStatuses": [
                  {"symbol":"*","name":"Next","nextStatusSymbol":"x","availableAsCommand":true,"type":"ON_HOLD"},
                  {"symbol":"?","name":"Blocked","nextStatusSymbol":" ","availableAsCommand":true,"type":"ON_HOLD"},
                  {"symbol":"/","name":"In Progress","nextStatusSymbol":"x","availableAsCommand":true,"type":"IN_PROGRESS"},
                  {"symbol":"-","name":"Canceled","nextStatusSymbol":" ","availableAsCommand":true,"type":"CANCELLED"}
                ]
              }
            }"##,
        );
    }

    fn parsed_dep(note: &str, block_id: &str) -> ParsedDependency {
        ParsedDependency {
            raw: format!("&{note}:{block_id}"),
            note: note.to_string(),
            block_id: block_id.to_string(),
            quoted: false,
        }
    }

    fn project_note(body: &str) -> String {
        format!("---\ntype: [[project]]\nstatus: wip\n---\n{body}")
    }

    fn usage_message(error: CaptureError) -> String {
        assert_eq!(
            error.kind,
            CaptureErrorKind::Usage,
            "expected usage, got {error:?}"
        );
        error.message
    }

    fn plan_write(
        root: &Path,
        dependent: &DependentRef,
        deps: &[ParsedDependency],
        options: ExistingDependencyOptions,
    ) -> Result<PlannedDependencyUpdate, CaptureError> {
        let mut ctx = DependencyContext::new(root, today());
        let mut planner = CaptureBatchPlanner::default();
        let mut warnings = Vec::new();
        plan_existing_task_dependencies(
            &mut ctx,
            &mut planner,
            root,
            dependent,
            deps,
            &mut warnings,
            options,
        )
    }

    fn staged_line(
        planner: &mut CaptureBatchPlanner,
        root: &Path,
        relative: &str,
        block_id: &str,
    ) -> String {
        let absolute = root.join(relative);
        let contents =
            current_note_text(planner, &absolute).expect("staged note");
        let settings = note_tasks::read_settings(root);
        let scan = note_tasks::scan(&contents, &settings);
        let task = lookup_staged_task(&scan, relative, block_id).expect("task");
        contents
            .lines()
            .nth(task.line_index)
            .expect("line")
            .to_string()
    }

    #[test]
    fn nested_dependent_path_is_addressed_by_relative_and_block_id() {
        let vault = tempfile::tempdir().expect("vault");
        let root = vault.path();
        write_settings(root);
        write_note(
            root,
            "projects/house/kitchen.md",
            "- [ ] #task Cabinets ^cabinets\n",
        );
        write_note(root, "mac_inbox.md", "- [ ] #task Ask Kelly ^ask-kelly\n");
        let dependent = DependentRef {
            relative: PathBuf::from("projects/house/kitchen.md"),
            block_id: "cabinets".to_string(),
        };
        let mut ctx = DependencyContext::new(root, today());
        let mut planner = CaptureBatchPlanner::default();
        let mut warnings = Vec::new();
        let update = plan_existing_task_dependencies(
            &mut ctx,
            &mut planner,
            root,
            &dependent,
            &[parsed_dep("mac_inbox", "ask-kelly")],
            &mut warnings,
            ExistingDependencyOptions::ampersand(),
        )
        .expect("nested write");
        assert_eq!(update.summary.dependent_note, "projects/house/kitchen.md");
        assert_eq!(
            update.summary.dependent_block_id.as_deref(),
            Some("cabinets")
        );
        assert_eq!(update.summary.role, DependencyUpdateRole::Dependent);
        assert_eq!(update.summary.previous_status, ' ');
        assert_eq!(update.summary.previous_status_name, "Ready");
        assert_eq!(update.summary.blocked_residents, 0);
        assert_eq!(update.dependent_ref.route, "projects/house/kitchen");
        let json = serde_json::to_value(&update.summary).expect("json");
        assert_eq!(json["role"], "dependent");
        assert_eq!(json["previous_status"], " ");
        assert_eq!(json["previous_status_name"], "Ready");
        assert!(json.get("blocked_residents").is_none());
        let staged = planner
            .peek_text(&root.join("projects/house/kitchen.md"))
            .expect("staged kitchen");
        assert!(
            staged.contains("[[mac_inbox#^ask-kelly]]"),
            "Depends-On gained N: {staged}"
        );
        assert!(staged.contains("- [?] "), "T flipped to Blocked: {staged}");
    }

    #[test]
    fn next_cap_holds_in_progress_dependent_and_prj_resident() {
        let vault = tempfile::tempdir().expect("vault");
        let root = vault.path();
        write_settings(root);
        write_note(root, "job.md", "- [/] #task Cleanup ^cleanup\n");
        write_note(root, "mac_inbox.md", "- [ ] #task Ask Kelly ^ask-kelly\n");
        let mut ctx = DependencyContext::new(root, today());
        let mut planner = CaptureBatchPlanner::default();
        let mut warnings = Vec::new();
        plan_existing_task_dependencies(
            &mut ctx,
            &mut planner,
            root,
            &DependentRef::from_route("job", "cleanup"),
            &[parsed_dep("mac_inbox", "ask-kelly")],
            &mut warnings,
            ExistingDependencyOptions::star_new_blocker(),
        )
        .expect("capped write");
        let n = staged_line(&mut planner, root, "mac_inbox.md", "ask-kelly");
        assert!(n.contains("- [*] "), "new blocker capped at Next, got {n}");
        assert!(
            !n.contains("- [/] "),
            "cap must not raise to In Progress: {n}"
        );

        write_note(
            root,
            "proj.md",
            &project_note(concat!(
                "- [ ] #task Ship #hide ^prj\n",
                "- [/] #task Grind ^grind\n",
            )),
        );
        write_note(root, "inbox2.md", "- [ ] #task New blocker ^new-blocker\n");
        let mut ctx = DependencyContext::new(root, today());
        let mut planner = CaptureBatchPlanner::default();
        let mut warnings = Vec::new();
        let update = plan_existing_task_dependencies(
            &mut ctx,
            &mut planner,
            root,
            &DependentRef::from_route("proj", "prj"),
            &[parsed_dep("inbox2", "new-blocker")],
            &mut warnings,
            ExistingDependencyOptions::star_new_blocker(),
        )
        .expect("capped prj write");
        assert!(update.summary.blocked_residents > 0);
        let json = serde_json::to_value(&update.summary).expect("json");
        assert_eq!(json["role"], "blocked_task");
        assert_eq!(json["blocked_residents"], 1);
        let n = staged_line(&mut planner, root, "inbox2.md", "new-blocker");
        assert!(
            n.contains("- [*] "),
            "^prj strongest resident [/] still caps new N at Next: {n}"
        );
    }

    #[test]
    fn existing_owner_is_uncapped_at_in_progress() {
        let vault = tempfile::tempdir().expect("vault");
        let root = vault.path();
        write_settings(root);
        write_note(root, "job.md", "- [/] #task Cleanup ^cleanup\n");
        write_note(root, "mac_inbox.md", "- [ ] #task Ask Kelly ^ask-kelly\n");
        let mut ctx = DependencyContext::new(root, today());
        let mut planner = CaptureBatchPlanner::default();
        let mut warnings = Vec::new();
        plan_existing_task_dependencies(
            &mut ctx,
            &mut planner,
            root,
            &DependentRef::from_route("job", "cleanup"),
            &[parsed_dep("mac_inbox", "ask-kelly")],
            &mut warnings,
            ExistingDependencyOptions::star_existing_owner(),
        )
        .expect("uncapped write");
        let n = staged_line(&mut planner, root, "mac_inbox.md", "ask-kelly");
        assert!(
            n.contains("- [/] "),
            "existing owner takes In Progress: {n}"
        );
    }

    #[test]
    fn always_mints_when_blocks_lines_is_off() {
        let vault = tempfile::tempdir().expect("vault");
        let root = vault.path();
        write_settings(root);
        write_note(root, "mac_inbox.md", "- [ ] #task Ask Kelly\n");
        let config = root.join("config.yml");
        fs::write(&config, "dependencies:\n  blocks_lines: false\n")
            .expect("config");
        let _guard =
            TestEnvGuard::set(&[("BOB_CONFIG_FILE", Some(config.as_os_str()))]);
        let mut ctx = DependencyContext::new(root, today());
        assert!(
            !ctx.blocks_lines_enabled(),
            "kill switch must be off for this test"
        );
        let mut planner = CaptureBatchPlanner::default();
        let absolute = root.join("mac_inbox.md");
        let first = mint_staged_block_id(
            &mut ctx,
            &mut planner,
            &absolute,
            Path::new("mac_inbox.md"),
            "- [ ] #task Ask Kelly",
        )
        .expect("mint");
        assert!(!first.is_empty());
        let second = mint_staged_block_id(
            &mut ctx,
            &mut planner,
            &absolute,
            Path::new("mac_inbox.md"),
            "- [ ] #task Ask Kelly",
        )
        .expect("second mint");
        assert_ne!(first, second, "reservations must not collide");
    }

    #[test]
    fn relocate_by_id_after_id_stamp_and_lane_promotion() {
        let vault = tempfile::tempdir().expect("vault");
        let root = vault.path();
        write_settings(root);
        write_note(root, "job.md", "- [*] #task Cleanup ^cleanup\n");
        write_note(root, "mac_inbox.md", "- [ ] #task Ask Kelly ^ask-kelly\n");
        let original =
            fs::read_to_string(root.join("mac_inbox.md")).expect("original N");
        let mut ctx = DependencyContext::new(root, today());
        let mut planner = CaptureBatchPlanner::default();
        let mut warnings = Vec::new();
        plan_existing_task_dependencies(
            &mut ctx,
            &mut planner,
            root,
            &DependentRef::from_route("job", "cleanup"),
            &[parsed_dep("mac_inbox", "ask-kelly")],
            &mut warnings,
            ExistingDependencyOptions::star_new_blocker(),
        )
        .expect("stamp and promote");
        let found = locate_task_by_block_id(
            &ctx,
            &mut planner,
            root,
            Path::new("mac_inbox.md"),
            "ask-kelly",
        )
        .expect("relocate");
        let staged = planner
            .peek_text(&root.join("mac_inbox.md"))
            .expect("staged N");
        let line = staged.lines().nth(found.line).expect("relocated line");
        assert_ne!(line, original.trim_end(), "line text changed");
        assert!(line.contains("[id::"), "target-ID stamp landed: {line}");
        assert!(line.contains("- [*] "), "lane promotion landed: {line}");
        assert_eq!(found.block_id.as_deref(), Some("ask-kelly"));
        assert_eq!(found.route, "mac_inbox");
    }

    fn resolve_star(
        root: &Path,
        typed: &str,
        block_id: &str,
        owner: Option<&DependentRef>,
    ) -> Result<DependentRef, CaptureError> {
        let mut ctx = DependencyContext::new(root, today());
        let mut planner = CaptureBatchPlanner::default();
        resolve_blocked_task_target(
            &mut ctx,
            &mut planner,
            root,
            typed,
            block_id,
            owner,
        )
    }

    #[test]
    fn a8_refusals_name_star_and_teach() {
        let vault = tempfile::tempdir().expect("vault");
        let root = vault.path();
        write_settings(root);
        write_note(
            root,
            "job.md",
            concat!(
                "- [ ] #task Cleanup ^cleanup\n",
                "- [x] #task Done ^done\n",
                "- [ ] #task Two lines ^two\n",
                "\t- ⛓️ **DEPENDS ON:** [[job#^cleanup]]\n",
                "\t- ⛓️ **DEPENDS ON:** [[job#^done]]\n",
                "- [ ] #task Broken ^broken\n",
                "\t- ⛓️ **DEPENDS ON:** needs review\n",
                "```md\n",
                "- [ ] #task Fenced example ^fenced\n",
                "```\n",
            ),
        );
        write_note(root, "2026/20261014.md", "- [ ] #task Daily ^gtd\n");
        write_note(root, "done/old.md", "- [ ] #task Archived ^old\n");
        write_note(root, "_templates/t.md", "- [ ] #task Template ^tmpl\n");
        write_note(root, "_generated/g.md", "- [ ] #task Generated ^gen\n");
        write_note(root, "_conflicts/c.md", "- [ ] #task Conflict ^con\n");
        write_note(root, ".hidden/x.md", "- [ ] #task Hidden ^hid\n");

        let cases: &[(&str, &str, &str)] = &[
            ("2026/20261014", "gtd", "daily note"),
            ("done/old", "old", "done/"),
            ("_templates/t", "tmpl", "_templates"),
            ("_generated/g", "gen", "_generated"),
            ("_conflicts/c", "con", "_conflicts"),
            (".hidden/x", "hid", "dot-directory"),
            ("job", "fenced", "fenced"),
            ("job", "done", "completed"),
            ("job", "two", "Depends-On"),
            ("job", "broken", "malformed"),
        ];
        for (note, block_id, needle) in cases {
            let message = usage_message(
                resolve_star(root, note, block_id, None).expect_err(needle),
            );
            assert!(
                message.contains('*'),
                "{needle}: message must name *: {message}"
            );
            assert!(
                message
                    .to_ascii_lowercase()
                    .contains(&needle.to_ascii_lowercase())
                    || message.contains(needle),
                "{needle}: {message}"
            );
        }

        write_note(
            root,
            "both.md",
            concat!(
                "```md\n",
                "- [ ] #task Example ^cleanup\n",
                "```\n",
                "- [ ] #task Cleanup ^cleanup\n",
            ),
        );
        resolve_star(root, "both", "cleanup", None).expect(
            "unfenced task wins over a fenced example with the same id",
        );

        let owner = DependentRef::from_route("job", "cleanup");
        let self_message = usage_message(
            resolve_star(root, "job", "cleanup", Some(&owner))
                .expect_err("self"),
        );
        assert!(self_message.contains('*'), "{self_message}");
        assert!(self_message.contains("itself"), "{self_message}");
    }

    #[test]
    fn prj_internal_same_note_and_descendant_refuse() {
        let vault = tempfile::tempdir().expect("vault");
        let root = vault.path();
        write_settings(root);
        write_note(
            root,
            "parent.md",
            &project_note("- [ ] #task Parent #hide ^prj\n"),
        );
        write_note(
            root,
            "child.md",
            "---\ntype: [[project]]\nstatus: wip\nparent: [[parent]]\n---\n- [ ] #task Child #hide ^prj\n- [ ] #task Work ^work\n",
        );

        let mut ctx = DependencyContext::new(root, today());
        let mut planner = CaptureBatchPlanner::default();
        let parent_abs = root.join("parent.md");
        let original =
            fs::read_to_string(&parent_abs).expect("parent original");
        let staged = format!("{original}- [ ] #task Staged resident ^staged\n");
        planner.stage(&parent_abs, staged).expect("stage resident");
        let mut warnings = Vec::new();
        let same = match plan_existing_task_dependencies(
            &mut ctx,
            &mut planner,
            root,
            &DependentRef::from_route("parent", "prj"),
            &[parsed_dep("parent", "staged")],
            &mut warnings,
            ExistingDependencyOptions::star_new_blocker(),
        ) {
            Err(error) => error,
            Ok(_) => panic!("same-note staged N should refuse"),
        };
        assert_eq!(same.kind, CaptureErrorKind::Usage);
        assert!(same.message.contains("same project"), "{}", same.message);

        let descendant = match plan_write(
            root,
            &DependentRef::from_route("parent", "prj"),
            &[parsed_dep("child", "work")],
            ExistingDependencyOptions::star_new_blocker(),
        ) {
            Err(error) => error,
            Ok(_) => panic!("descendant N should refuse"),
        };
        assert_eq!(descendant.kind, CaptureErrorKind::Usage);
        assert!(
            descendant.message.contains("sub-project"),
            "{}",
            descendant.message
        );

        let owner_same = DependentRef::from_route("parent", "staged");
        let resolved_same = usage_message(
            resolve_star(root, "parent", "prj", Some(&owner_same))
                .expect_err("resolver same-note"),
        );
        assert!(resolved_same.contains('*'), "{resolved_same}");
        assert!(
            resolved_same.contains("same project")
                || resolved_same.contains("cannot wait"),
            "{resolved_same}"
        );

        let owner_child = DependentRef::from_route("child", "work");
        let resolved_desc = usage_message(
            resolve_star(root, "parent", "prj", Some(&owner_child))
                .expect_err("resolver descendant"),
        );
        assert!(resolved_desc.contains('*'), "{resolved_desc}");
        assert!(
            resolved_desc.contains("sub-project")
                || resolved_desc.contains("cannot wait"),
            "{resolved_desc}"
        );
    }
}
