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
    let mut touched_paths = BTreeSet::new();
    for plan in &plans {
        if note_write_planned(plan) {
            touched_paths.insert(plan.note_path.clone());
        }
        for write in &plan.routed_task_note_writes {
            if write.action != "none" {
                touched_paths.insert(write.path.clone());
            }
        }
        for write in &plan.image_assets {
            touched_paths.insert(write.dest_path.clone());
        }
        if plan.marker_write_needed {
            touched_paths.insert(plan.pdf.clone());
        }
    }

    let touched_paths = touched_paths
        .into_iter()
        .filter(|path| path.exists())
        .filter(|path| path.strip_prefix(&config.bob_dir).is_ok())
        .collect::<Vec<_>>();
    if touched_paths.is_empty() {
        return Ok(());
    }

    match git_status(config, &touched_paths)? {
        GitStatus::Worktree { entries } if !entries.is_empty() => {
            let mut refused = Vec::new();
            let mut refused_paths = Vec::new();
            for entry in &entries {
                if !dirty_entry_allowed_for_plans(config, &plans, entry)? {
                    refused.push(entry.raw.clone());
                    refused_paths.push(config.bob_dir.join(&entry.path));
                }
            }
            if refused.is_empty() {
                Ok(())
            } else {
                Err(CommandError::new(format!(
                    "refusing to modify dirty vault files:\n  {}\ncommit, stash, or clean these files before rerunning",
                    refused.join("\n  ")
                ))
                .with_code("dirty_targets")
                .with_paths(refused_paths))
            }
        }
        _ => Ok(()),
    }
}

pub(super) fn note_write_planned(plan: &PdfSyncPlan) -> bool {
    plan.stable_note_action != "none" || plan.marker_write_needed
}

pub(super) fn dirty_entry_allowed_for_plans(
    config: &Config,
    plans: &[&PdfSyncPlan],
    entry: &GitStatusEntry,
) -> Result<bool> {
    if entry.index_status != ' ' || entry.worktree_status != 'M' {
        return Ok(false);
    }

    let path = config.bob_dir.join(&entry.path);
    let Some(plan) = plans.iter().find(|plan| plan.note_path == path) else {
        return Ok(false);
    };
    if !note_write_planned(plan) {
        return Ok(false);
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
        .arg("status")
        .arg("--short")
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

    Ok(GitStatus::Worktree {
        entries: String::from_utf8_lossy(&output.stdout)
            .lines()
            .filter_map(parse_git_status_entry)
            .collect(),
    })
}

pub(super) fn parse_git_status_entry(line: &str) -> Option<GitStatusEntry> {
    if line.len() < 4 {
        return None;
    }
    let mut chars = line.chars();
    let index_status = chars.next()?;
    let worktree_status = chars.next()?;
    if chars.next()? != ' ' {
        return None;
    }
    Some(GitStatusEntry {
        index_status,
        worktree_status,
        path: PathBuf::from(&line[3..]),
        raw: line.to_string(),
    })
}

pub(super) fn command_output(output: &Output) -> String {
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    format!("stdout:\n{stdout}\nstderr:\n{stderr}")
}
