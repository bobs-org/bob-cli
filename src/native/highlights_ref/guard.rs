//! Output collision and dirty-note guards.
use super::*;

pub(super) fn validate_output_collisions(
    config: &Config,
    pdfs: &[PathBuf],
) -> Result<()> {
    let mut by_note_path: BTreeMap<PathBuf, Vec<PathBuf>> = BTreeMap::new();
    for pdf in pdfs {
        by_note_path
            .entry(ref_note_path(config, pdf)?)
            .or_default()
            .push(pdf.clone());
    }

    let collisions = by_note_path
        .into_iter()
        .filter(|(_, pdfs)| pdfs.len() > 1)
        .collect::<Vec<_>>();
    if collisions.is_empty() {
        return Ok(());
    }

    let mut message =
        String::from("output path collision(s) detected before writes:");
    let mut targets = Vec::new();
    for (note_path, pdfs) in collisions {
        message.push('\n');
        message.push_str("  ");
        message.push_str(&note_path.display().to_string());
        message.push_str(" <= ");
        message.push_str(
            &pdfs
                .iter()
                .map(|path| path.display().to_string())
                .collect::<Vec<_>>()
                .join(", "),
        );
        targets.push(note_path);
    }
    Err(CommandError::new(message)
        .with_code("output_collision")
        .with_paths(targets))
}

pub(super) fn validate_planned_asset_collisions(
    plans: &[&PdfSyncPlan],
) -> Result<()> {
    let mut by_asset_path: BTreeMap<PathBuf, Vec<PathBuf>> = BTreeMap::new();
    for plan in plans {
        for asset in &plan.image_assets {
            by_asset_path
                .entry(asset.dest_path.clone())
                .or_default()
                .push(plan.pdf.clone());
        }
    }

    let collisions = by_asset_path
        .into_iter()
        .filter(|(_, pdfs)| pdfs.len() > 1)
        .collect::<Vec<_>>();
    if collisions.is_empty() {
        return Ok(());
    }

    let mut message =
        String::from("output path collision(s) detected before writes:");
    let mut targets = Vec::new();
    for (asset_path, pdfs) in collisions {
        message.push('\n');
        message.push_str("  ");
        message.push_str(&asset_path.display().to_string());
        message.push_str(" <= ");
        message.push_str(
            &pdfs
                .iter()
                .map(|path| path.display().to_string())
                .collect::<Vec<_>>()
                .join(", "),
        );
        targets.push(asset_path);
    }
    Err(CommandError::new(message)
        .with_code("output_collision")
        .with_paths(targets))
}

pub(super) fn validate_note_target(path: &Path) -> Result<()> {
    if path.is_dir() {
        return Err(CommandError::new(format!(
            "reference note target is a directory: {}",
            path.display()
        )));
    }
    Ok(())
}

pub(super) fn ensure_safe_to_write<'a, I>(
    config: &Config,
    plans: I,
) -> Result<()>
where
    I: IntoIterator<Item = &'a PdfSyncPlan>,
{
    let plans = plans.into_iter().collect::<Vec<_>>();
    let preflight = scan_dirty_preflight(config, plans.iter().copied())?;
    let refused_paths = preflight
        .held_targets_by_pdf
        .values()
        .flatten()
        .cloned()
        .collect::<BTreeSet<_>>();
    if refused_paths.is_empty() {
        return Ok(());
    }
    let display_paths = refused_paths
        .iter()
        .map(|path| display_vault_path(config, path))
        .collect::<Vec<_>>();
    Err(CommandError::new(format!(
        "refusing to modify dirty vault files:\n  {}\ncommit, stash, or clean these files before rerunning",
        display_paths.join("\n  ")
    ))
    .with_code("dirty_targets")
    .with_paths(refused_paths.into_iter().collect()))
}

/// A scan-wide, pre-write snapshot of dirty targets grouped by the PDF plans
/// whose own write policy rejects each target.
pub(super) struct ScanDirtyPreflight {
    pub(super) held_targets_by_pdf: BTreeMap<PathBuf, Vec<PathBuf>>,
}

/// Classify dirty targets once, before any PDF executes. Keeping the snapshot
/// here prevents an earlier plan's writes from changing the status seen by a
/// later plan, and evaluating allowances per owner prevents one plan's note
/// allowance from authorizing another plan's explicit routed write.
pub(super) fn scan_dirty_preflight<'a, I>(
    config: &Config,
    plans: I,
) -> Result<ScanDirtyPreflight>
where
    I: IntoIterator<Item = &'a PdfSyncPlan>,
{
    let mut owners: BTreeMap<PathBuf, Vec<&PdfSyncPlan>> = BTreeMap::new();
    for plan in plans {
        // Preserve replaced-capture precedence: the plan will report that
        // hold in the executor and must not be vetoed by its other targets.
        if super::sync::replaced_capture_hold(config, &plan.pdf)?.is_some() {
            continue;
        }
        let path_set = scoped_write_path_set(config, plan);
        for path in path_set.write_paths.difference(&path_set.veto_exempt_paths)
        {
            if path.exists() && path.strip_prefix(&config.bob_dir).is_ok() {
                owners.entry(path.clone()).or_default().push(plan);
            }
        }
    }

    if owners.is_empty() {
        return Ok(ScanDirtyPreflight {
            held_targets_by_pdf: BTreeMap::new(),
        });
    }

    let touched_paths = owners.keys().cloned().collect::<Vec<_>>();
    let mut held: BTreeMap<PathBuf, BTreeSet<PathBuf>> = BTreeMap::new();
    if let GitStatus::Worktree { entries } = git_status(config, &touched_paths)?
    {
        for entry in &entries {
            let mut entry_paths = vec![entry.path.clone()];
            if let Some(original_path) = &entry.original_path {
                entry_paths.push(original_path.clone());
            }
            for relative_path in entry_paths {
                let target = config.bob_dir.join(&relative_path);
                let Some(target_owners) = owners.get(&target) else {
                    continue;
                };
                for plan in target_owners {
                    if dirty_entry_allowed_for_plan(config, plan, entry)? {
                        continue;
                    }
                    held.entry(plan.pdf.clone())
                        .or_default()
                        .insert(target.clone());
                }
            }
        }
    }

    Ok(ScanDirtyPreflight {
        held_targets_by_pdf: held
            .into_iter()
            .map(|(pdf, paths)| (pdf, paths.into_iter().collect()))
            .collect(),
    })
}

pub(super) fn dirty_targets_hold(
    config: &Config,
    plan: &PdfSyncPlan,
    targets: &[PathBuf],
) -> ScanHold {
    let paths = targets
        .iter()
        .map(|path| display_vault_path(config, path))
        .collect::<Vec<_>>();
    scan_hold_for(
        plan,
        HoldReason::DirtyTargets,
        format!(
            "dirty vault targets: {} · retrying next scan",
            paths.join(" · ")
        ),
    )
}

fn display_vault_path(config: &Config, path: &Path) -> String {
    path.strip_prefix(&config.bob_dir)
        .unwrap_or(path)
        .to_string_lossy()
        .escape_debug()
        .to_string()
}

/// Complete path set for a one-PDF write. The exempt set contains the
/// reading-task and v2-default destinations intentionally excluded from the
/// dirty-target veto; they remain part of a scoped intake commit.
pub(super) struct ScopedWritePathSet {
    pub(super) write_paths: BTreeSet<PathBuf>,
    pub(super) veto_exempt_paths: BTreeSet<PathBuf>,
}

pub(super) fn scoped_write_path_set(
    config: &Config,
    plan: &PdfSyncPlan,
) -> ScopedWritePathSet {
    // V2 default follow-ups share reading-task destinations and must accept
    // unrelated authored changes there. A destination used only by v2
    // default insertions is exempt from the dirty-target veto; any explicit
    // route (or legacy v1 write) to the same path retains the legacy policy.
    let mut explicit_paths: BTreeSet<PathBuf> = BTreeSet::new();
    for write in &plan.routed_task_note_writes {
        if write.action != "none" {
            explicit_paths.insert(write.path.clone());
        }
    }
    for intent in &plan.routed_intents {
        if !intent.is_v2_default {
            explicit_paths.insert(intent.path.clone());
        }
    }
    let mut write_paths = BTreeSet::new();
    let mut veto_exempt_paths = BTreeSet::new();
    if note_write_planned(plan) {
        write_paths.insert(plan.note_path.clone());
    }
    for write in &plan.routed_task_note_writes {
        if write.action != "none" {
            write_paths.insert(write.path.clone());
        }
    }
    for intent in &plan.routed_intents {
        if intent_write_planned(&intent.path, intent) {
            write_paths.insert(intent.path.clone());
        }
    }
    for write in &plan.image_assets {
        write_paths.insert(write.dest_path.clone());
    }
    if plan.marker_write_needed {
        write_paths.insert(plan.pdf.clone());
    }

    let reading_destination =
        plan.reading_task_plan.as_ref().and_then(|reading| {
            match &reading.action {
                ReadingTaskAction::Insert {
                    destination_route, ..
                } => {
                    Some(config.bob_dir.join(format!("{destination_route}.md")))
                }
                ReadingTaskAction::LineEdit { .. } => plan
                    .reading_located
                    .as_ref()
                    .map(|located| config.bob_dir.join(&located.path)),
                ReadingTaskAction::NoWrite => None,
            }
        });
    if let Some(path) = reading_destination {
        write_paths.insert(path.clone());
        if path != plan.note_path && !explicit_paths.contains(&path) {
            veto_exempt_paths.insert(path);
        }
    }

    for intent in &plan.routed_intents {
        if intent.is_v2_default && !explicit_paths.contains(&intent.path) {
            veto_exempt_paths.insert(intent.path.clone());
        }
    }

    ScopedWritePathSet {
        write_paths,
        veto_exempt_paths,
    }
}

pub(super) fn note_write_planned(plan: &PdfSyncPlan) -> bool {
    plan.stable_note_action != "none" || plan.marker_write_needed
}

/// True when an insertion intention may still write: at least one planned
/// line's identity is absent from the destination's current bytes.
pub(super) fn intent_write_planned(
    path: &Path,
    intent: &RoutedInsertionIntent,
) -> bool {
    let Ok(contents) = fs::read_to_string(path) else {
        return true;
    };
    intent.candidates.iter().any(|candidate| {
        !contents.contains(candidate.processed_id.as_str())
            && !contents.contains(candidate.identity.as_str())
    })
}

pub(super) fn dirty_entry_allowed_for_plan(
    config: &Config,
    plan: &PdfSyncPlan,
    entry: &GitStatusEntry,
) -> Result<bool> {
    if entry.index_status != ' '
        || entry.worktree_status != 'M'
        || entry.original_path.is_some()
    {
        return Ok(false);
    }

    let path = config.bob_dir.join(&entry.path);
    if plan.note_path != path {
        return Ok(false);
    }
    if !note_write_planned(plan) {
        return Ok(false);
    }
    // V2 ref notes additionally allow tracked modifications confined to the
    // managed embed (including deletion, insertion, and repointing) plus
    // permitted frontmatter edits; a residence-only frontmatter change is
    // allowed even though `parent` is excluded from the sync contribution.
    // Unrelated body edits still refuse, and the v1 guard below is untouched.
    if plan
        .reading_task_plan
        .as_ref()
        .is_some_and(|reading| reading.branch == NoteBranch::V2)
    {
        if !note_contents_match_plan(&path, plan.note.contents().as_deref())? {
            return Ok(false);
        }
        let Some(head_contents) = git_head_contents(config, &path)? else {
            return Ok(false);
        };
        let current_contents = fs::read_to_string(&path).map_err(|error| {
            CommandError::new(format!("read note {}: {error}", path.display()))
        })?;
        return Ok(v2_dirty_note_allowed(
            &head_contents,
            &current_contents,
            plan.decision.frontmatter_contributed,
        ));
    }
    if path.strip_prefix(&config.ref_dir).is_err() {
        return Ok(false);
    }
    if !note_contents_match_plan(&path, plan.note.contents().as_deref())? {
        return Ok(false);
    }

    let Some(head_contents) = git_head_contents(config, &path)? else {
        return Ok(false);
    };
    let current_contents = fs::read_to_string(&path).map_err(|error| {
        CommandError::new(format!("read note {}: {error}", path.display()))
    })?;
    let Some(change) =
        dirty_note_allowed_change(&head_contents, &current_contents)
    else {
        return Ok(false);
    };
    if change.includes_frontmatter() && !plan.decision.frontmatter_contributed {
        return Ok(false);
    }
    Ok(true)
}

pub(super) fn git_head_contents(
    config: &Config,
    path: &Path,
) -> Result<Option<String>> {
    let child_env = ob::child_env();
    let relative = path.strip_prefix(&config.bob_dir).unwrap_or(path);
    let Some(relative) = relative.to_str() else {
        return Ok(None);
    };
    let output = ob::git_command(&config.bob_dir, &child_env)
        .arg("show")
        .arg(format!("HEAD:{relative}"))
        .output()
        .map_err(|error| {
            CommandError::new(format!(
                "read HEAD version of {}: {error}",
                path.display()
            ))
        })?;
    if output.status.success() {
        return Ok(Some(String::from_utf8_lossy(&output.stdout).into_owned()));
    }
    Ok(None)
}

#[cfg(test)]
pub(super) fn changes_confined_to_frontmatter_or_pdf_task_checkbox(
    base: &str,
    current: &str,
) -> bool {
    dirty_note_allowed_change(base, current).is_some()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum DirtyNoteAllowedChange {
    FrontmatterOnly,
    PdfTaskCheckboxOnly,
    FrontmatterAndPdfTaskCheckbox,
}

impl DirtyNoteAllowedChange {
    pub(super) fn includes_frontmatter(self) -> bool {
        matches!(
            self,
            DirtyNoteAllowedChange::FrontmatterOnly
                | DirtyNoteAllowedChange::FrontmatterAndPdfTaskCheckbox
        )
    }
}

pub(super) fn dirty_note_allowed_change(
    base: &str,
    current: &str,
) -> Option<DirtyNoteAllowedChange> {
    let (base_frontmatter, base_body) = split_frontmatter(base)?;
    let (current_frontmatter, current_body) = split_frontmatter(current)?;
    let frontmatter_changed = base_frontmatter != current_frontmatter;
    let task_checkbox_changed =
        bodies_differ_only_by_pdf_task_checkbox(&base_body, &current_body);

    match (
        frontmatter_changed,
        task_checkbox_changed,
        base_body == current_body,
    ) {
        (true, false, true) => Some(DirtyNoteAllowedChange::FrontmatterOnly),
        (false, true, false) => {
            Some(DirtyNoteAllowedChange::PdfTaskCheckboxOnly)
        }
        (true, true, false) => {
            Some(DirtyNoteAllowedChange::FrontmatterAndPdfTaskCheckbox)
        }
        _ => None,
    }
}

pub(super) fn bodies_differ_only_by_pdf_task_checkbox(
    base_body: &str,
    current_body: &str,
) -> bool {
    let base_task = match parse_pdf_task_line(base_body) {
        Ok(PdfTaskLineState::Present(task_line)) => task_line,
        _ => return false,
    };
    let current_task = match parse_pdf_task_line(current_body) {
        Ok(PdfTaskLineState::Present(task_line)) => task_line,
        _ => return false,
    };
    if base_task.mark == current_task.mark {
        return false;
    }

    replace_pdf_task_checkbox_mark(base_body, current_task.mark).is_ok_and(
        |toggled| {
            // Sync stamps a close date when it flips the mark itself; a
            // user-made close carries no stamp. Both count as a
            // checkbox-only difference, so compare stamp-free.
            toggled == current_body
                || without_close_date_stamp(&toggled)
                    == without_close_date_stamp(current_body)
        },
    )
}

/// Remove one sync-inserted `[completion:: DATE]` / `[cancelled:: DATE]`
/// field sitting immediately before the `^ref` token, so dirty-guard
/// comparisons see the checkbox change without the stamp.
pub(super) fn without_close_date_stamp(body: &str) -> String {
    for field in ["[completion::", "[cancelled::"] {
        let needle = format!(" {field} ");
        let Some(field_start) = body.find(&needle) else {
            continue;
        };
        let value_start = field_start + needle.len();
        let Some(value_end) = body[value_start..].find(']') else {
            continue;
        };
        let after = &body[value_start + value_end + 1..];
        if after == format!(" {PDF_TASK_BLOCK_ID}")
            || after.starts_with(&format!(" {PDF_TASK_BLOCK_ID}\n"))
            || after.starts_with(&format!(" {PDF_TASK_BLOCK_ID}\r"))
        {
            let mut stripped = String::with_capacity(body.len());
            stripped.push_str(&body[..field_start]);
            stripped.push_str(after);
            return stripped;
        }
    }
    body.to_string()
}

pub(super) fn git_status(
    config: &Config,
    paths: &[PathBuf],
) -> Result<GitStatus> {
    let child_env = ob::child_env();
    let rev_parse = ob::git_command(&config.bob_dir, &child_env)
        .arg("rev-parse")
        .arg("--is-inside-work-tree")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();

    let rev_parse = match rev_parse {
        Ok(status) if status.success() => status,
        Ok(_) => return Ok(GitStatus::NotWorktree),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(GitStatus::MissingCommand);
        }
        Err(error) => {
            return Err(CommandError::new(format!(
                "run git rev-parse in {}: {error}",
                config.bob_dir.display()
            )));
        }
    };
    let _ = rev_parse;

    let mut command = ob::git_command(&config.bob_dir, &child_env);
    command
        .arg("-c")
        .arg("color.status=false")
        .arg("--literal-pathspecs")
        .arg("status")
        .arg("--porcelain=v1")
        .arg("-z")
        .arg("--untracked-files=all")
        .arg("--");
    for path in paths {
        let pathspec = path.strip_prefix(&config.bob_dir).unwrap_or(path);
        command.arg(pathspec);
    }
    let output = command.output().map_err(|error| {
        CommandError::new(format!(
            "run git status in {}: {error}",
            config.bob_dir.display()
        ))
    })?;
    if !output.status.success() {
        return Err(CommandError::new(format!(
            "git status failed in {}:\n{}",
            config.bob_dir.display(),
            command_output(&output)
        )));
    }

    let entries =
        parse_git_status_entries(&output.stdout).map_err(|reason| {
            CommandError::new(format!(
                "could not classify Git status in {}: {reason}",
                config.bob_dir.display()
            ))
        })?;
    Ok(GitStatus::Worktree { entries })
}

fn parse_git_status_entries(
    bytes: &[u8],
) -> std::result::Result<Vec<GitStatusEntry>, String> {
    let mut records = bytes.split(|byte| *byte == 0).peekable();
    let mut entries = Vec::new();
    while let Some(record) = records.next() {
        if record.is_empty() {
            if records.peek().is_none() {
                break;
            }
            return Err(
                "unexpected empty NUL-delimited status record".to_string()
            );
        }
        if record.len() < 4 || record[2] != b' ' {
            return Err("status record has no XY/path separator".to_string());
        }
        let index_status = status_char(record[0])?;
        let worktree_status = status_char(record[1])?;
        if !valid_status_char(index_status)
            || !valid_status_char(worktree_status)
        {
            return Err(format!(
                "unknown porcelain status {:?}{:?}",
                index_status, worktree_status
            ));
        }
        let path = path_from_git_bytes(&record[3..])?;
        let has_rename_source = matches!(index_status, 'R' | 'C')
            || matches!(worktree_status, 'R' | 'C');
        let original_path = if has_rename_source {
            let source = records.next().ok_or_else(|| {
                "rename/copy status has no source path".to_string()
            })?;
            if source.is_empty() {
                return Err(
                    "rename/copy status has an empty source path".to_string()
                );
            }
            Some(path_from_git_bytes(source)?)
        } else {
            None
        };
        let raw = match &original_path {
            Some(original_path) => format!(
                "{}{} {} (from {})",
                index_status,
                worktree_status,
                escaped_path(&path),
                escaped_path(original_path)
            ),
            None => format!(
                "{}{} {}",
                index_status,
                worktree_status,
                escaped_path(&path)
            ),
        };
        entries.push(GitStatusEntry {
            index_status,
            worktree_status,
            path,
            original_path,
            raw,
        });
    }
    Ok(entries)
}

fn status_char(byte: u8) -> std::result::Result<char, String> {
    if byte.is_ascii() {
        Ok(char::from(byte))
    } else {
        Err("porcelain status contains a non-ASCII status byte".to_string())
    }
}

fn valid_status_char(status: char) -> bool {
    matches!(
        status,
        ' ' | 'M' | 'A' | 'D' | 'R' | 'C' | 'U' | '?' | '!' | 'T'
    )
}

fn path_from_git_bytes(bytes: &[u8]) -> std::result::Result<PathBuf, String> {
    if bytes.is_empty() {
        return Err("status record has an empty path".to_string());
    }
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStringExt;
        Ok(PathBuf::from(OsString::from_vec(bytes.to_vec())))
    }
    #[cfg(not(unix))]
    {
        let path = std::str::from_utf8(bytes).map_err(|_| {
            "status path is not valid UTF-8 on this platform".to_string()
        })?;
        Ok(PathBuf::from(path))
    }
}

fn escaped_path(path: &Path) -> String {
    path.to_string_lossy().escape_debug().to_string()
}

pub(super) fn command_output(output: &Output) -> String {
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    format!("stdout:\n{stdout}\nstderr:\n{stderr}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn porcelain_status_parser_preserves_quoted_and_non_ascii_paths() {
        let path = "ref/chat/space \"and é\nline.md";
        let bytes = format!(" M {path}\0").into_bytes();
        let entries = parse_git_status_entries(&bytes).expect("parse status");

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].index_status, ' ');
        assert_eq!(entries[0].worktree_status, 'M');
        assert_eq!(entries[0].path, PathBuf::from(path));
        assert!(entries[0].original_path.is_none());
        assert!(!entries[0].raw.contains('\n'));
    }

    #[test]
    fn porcelain_status_parser_keeps_both_rename_paths() {
        let entries = parse_git_status_entries(
            b"R  ref/chat/new name.md\0ref/chat/old name.md\0",
        )
        .expect("parse rename status");

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].index_status, 'R');
        assert_eq!(entries[0].path, PathBuf::from("ref/chat/new name.md"));
        assert_eq!(
            entries[0].original_path,
            Some(PathBuf::from("ref/chat/old name.md"))
        );
    }

    #[test]
    fn porcelain_status_parser_fails_closed_on_unclassifiable_records() {
        assert!(parse_git_status_entries(b" M\0").is_err());
        assert!(parse_git_status_entries(b"XZ path.md\0").is_err());
        assert!(parse_git_status_entries(b"R  new.md\0").is_err());
    }

    #[test]
    fn git_status_keeps_staged_untracked_quoted_and_renamed_targets_exact() {
        let temp = tempfile::tempdir().expect("create vault");
        let bob_dir = temp.path();
        let ref_dir = bob_dir.join("ref/chat");
        fs::create_dir_all(&ref_dir).expect("create ref directory");
        let staged = PathBuf::from("ref/chat/staged \"é.md");
        let old_name = PathBuf::from("ref/chat/old \"é.md");
        let new_name = PathBuf::from("ref/chat/new \"é.md");
        let untracked = PathBuf::from("ref/chat/untracked \"é\n.md");
        fs::write(bob_dir.join(&staged), "base staged\n")
            .expect("write staged");
        fs::write(bob_dir.join(&old_name), "rename me\n")
            .expect("write rename source");

        let run_git = |args: &[&str]| {
            let output = process::Command::new("git")
                .current_dir(bob_dir)
                .args(args)
                .output()
                .expect("run Git fixture command");
            assert!(
                output.status.success(),
                "git {args:?} failed:\n{}",
                String::from_utf8_lossy(&output.stderr)
            );
        };
        run_git(&["init", "-q"]);
        run_git(&["config", "user.name", "Test User"]);
        run_git(&["config", "user.email", "test@example.com"]);
        run_git(&["add", "."]);
        run_git(&["commit", "-q", "-m", "initial"]);

        fs::write(bob_dir.join(&staged), "staged change\n")
            .expect("modify staged");
        run_git(&["add", "--", "ref/chat/staged \"é.md"]);
        fs::rename(bob_dir.join(&old_name), bob_dir.join(&new_name))
            .expect("rename tracked file");
        run_git(&["add", "-A"]);
        fs::write(bob_dir.join(&untracked), "untracked\n")
            .expect("write untracked");

        let config = Config::for_vault(bob_dir);
        let paths = [staged, old_name, new_name, untracked]
            .iter()
            .map(|path| bob_dir.join(path))
            .collect::<Vec<_>>();
        let GitStatus::Worktree { entries } =
            git_status(&config, &paths).expect("structured Git status")
        else {
            panic!("fixture vault must be a Git worktree");
        };

        let staged_entry = entries
            .iter()
            .find(|entry| entry.path == PathBuf::from("ref/chat/staged \"é.md"))
            .expect("staged path preserved");
        assert_eq!(
            (staged_entry.index_status, staged_entry.worktree_status),
            ('M', ' ')
        );
        let untracked_entry = entries
            .iter()
            .find(|entry| {
                entry.path == PathBuf::from("ref/chat/untracked \"é\n.md")
            })
            .expect("untracked path preserved");
        assert_eq!(
            (
                untracked_entry.index_status,
                untracked_entry.worktree_status
            ),
            ('?', '?')
        );
        let rename_entry = entries
            .iter()
            .find(|entry| entry.path == PathBuf::from("ref/chat/new \"é.md"))
            .expect("rename destination preserved");
        assert_eq!(rename_entry.index_status, 'R');
        assert_eq!(
            rename_entry.original_path,
            Some(PathBuf::from("ref/chat/old \"é.md"))
        );
    }

    fn routed_intent(path: PathBuf) -> RoutedInsertionIntent {
        RoutedInsertionIntent {
            path,
            lines: vec!["- [ ] follow up ^ann-1".to_string()],
            candidates: vec![AnnotationTaskCandidate {
                identity: "^ann-1".to_string(),
                task_text: "follow up".to_string(),
                source_block_id: "src".to_string(),
                target: AnnotationTaskTarget::RoutedNote(PathBuf::from(
                    "sase.md",
                )),
                source_ref_note_path: PathBuf::from("ref/papers/src.md"),
                processed_id: "ann-1".to_string(),
            }],
            owner: PathBuf::from("lib/papers/src.pdf"),
            is_v2_default: false,
        }
    }

    #[test]
    fn intent_write_planned_is_true_until_the_destination_contains_identity() {
        let temp = tempfile::tempdir().expect("create temp dir");
        let dest = temp.path().join("sase.md");
        let intent = routed_intent(dest.clone());
        assert!(
            intent_write_planned(&dest, &intent),
            "a missing destination still needs a write"
        );
        fs::write(&dest, "# SASE\n\n## Tasks\n").expect("write dest");
        assert!(
            intent_write_planned(&dest, &intent),
            "a dest without the identity is still planned"
        );
        fs::write(&dest, "# SASE\n\n- [ ] follow up ^ann-1\n")
            .expect("write dest with identity");
        assert!(
            !intent_write_planned(&dest, &intent),
            "once the dest contains the identity the path drops from the commit set"
        );
    }
}
