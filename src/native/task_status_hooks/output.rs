//! Human and JSON printing for task-status sync.
use super::*;

pub(super) fn print_result(result: &SyncResult, format: OutputFormat) {
    match format {
        OutputFormat::Json => println!(
            "{}",
            serde_json::to_string(result).unwrap_or_else(|error| {
                panic!("serialize {COMMAND_NAME} result: {error}")
            })
        ),
        OutputFormat::Human => print_human_result(result),
    }
    print_warnings(result);
}

pub(super) fn print_human_result(result: &SyncResult) {
    let styler = Styler::detect();
    let change_count = result.marked_next.len()
        + result.marked_in_progress.len()
        + result.cleared.len()
        + result.cleared_in_progress.len()
        + result.marked_blocked.len()
        + result.unblocked.len()
        + result.struck_completed_references.len()
        + result.moved_completed_references.len()
        + result.marker_added_references.len()
        + result.marker_removed_references.len()
        + result.removed_canceled_references.len()
        + result.removed_duplicate_lines.len()
        + result.removed_empty_pomodoros.len()
        + result.grouped_task_sections.len()
        + result.dependency_projection_updates.len()
        + result.adopted_dependency_lines.len()
        + result.healed_dependency_links.len()
        + result.canonicalized_dependency_lines.len();
    let prefix = if result.dry_run {
        styler.success_prefix(true)
    } else {
        styler.green("\u{2713}")
    };
    let previous_context = result.previous_daily_file.as_ref().map_or_else(
        || "no previous daily".to_string(),
        |path| {
            format!(
                "previous {} ({} references)",
                path, result.previous_daily_references
            )
        },
    );
    if change_count == 0 && result.grouping_warnings.is_empty() {
        println!(
            "{prefix} {COMMAND_NAME}  {} \u{2014} already in sync, no changes \u{b7} {previous_context}",
            styler.cyan(&result.daily_file),
        );
        print_plan_budget_line(result);
        return;
    }

    println!(
        "{prefix} {COMMAND_NAME}  {}",
        styler.cyan(&result.daily_file)
    );
    println!(
        "  {} open pomodoros \u{b7} {} direct references \u{b7} {} recent activity references \u{b7} {} dependency references \u{b7} {} files scanned \u{b7} {previous_context}",
        result.open_pomodoros,
        result.references,
        result.recent_activity_references,
        result.dependency_references,
        result.scanned_files
    );
    print_plan_budget_line(result);
    print_change_section(
        &styler,
        if result.dry_run {
            "would mark next"
        } else {
            "marked next"
        },
        "[ ] \u{2192} [*]",
        &result.marked_next,
        true,
    );
    print_change_section(
        &styler,
        if result.dry_run {
            "would mark in progress"
        } else {
            "marked in progress"
        },
        "[ ] or [*] -> [/]",
        &result.marked_in_progress,
        true,
    );
    print_change_section(
        &styler,
        if result.dry_run {
            "would clear"
        } else {
            "cleared"
        },
        "[*] \u{2192} [ ]",
        &result.cleared,
        false,
    );
    print_change_section(
        &styler,
        if result.dry_run {
            "would clear in progress"
        } else {
            "cleared in progress"
        },
        "[/] \u{2192} [ ]",
        &result.cleared_in_progress,
        false,
    );
    print_dependency_status_section(
        &styler,
        true,
        if result.dry_run {
            "would mark blocked"
        } else {
            "marked blocked"
        },
        &result.marked_blocked,
    );
    print_dependency_status_section(
        &styler,
        false,
        if result.dry_run {
            "would unblock"
        } else {
            "unblocked"
        },
        &result.unblocked,
    );
    print_completed_reference_sections(result);
    print_marker_reference_sections(result);
    print_canceled_reference_section(result);
    print_duplicate_line_section(result);
    print_empty_pomodoro_section(result);
    print_dependency_projection_sections(&styler, result);
    print_grouped_task_sections(&styler, result);
    if result.kept_next > 0 || result.kept_in_progress > 0 {
        println!();
        println!(
            "  kept {} already next \u{b7} {} in progress",
            result.kept_next, result.kept_in_progress
        );
    }
    if !result.dry_run
        && let Some(recovery) = &result.recovery_directory
    {
        println!();
        println!("  recovery copies: {recovery}");
    }
    println!(
        "Dependencies: {} projected, {} adopted, {} healed, {} canonicalized, {} legacy children, {} warnings",
        result.dependency_projection_updates.len(),
        result.adopted_dependency_lines.len(),
        result.healed_dependency_links.len(),
        result.canonicalized_dependency_lines.len(),
        result.legacy_dependency_children,
        result.dependency_warnings.len()
    );
    println!(
        "Summary: {} marked next, {} marked in progress, {} cleared, {} cleared in progress, {} blocked, {} unblocked, {} struck, {} moved, {} marked, {} unmarked, {} canceled-reference triggers, {} duplicate-line removals, {} empty Pomodoros removed, {} grouped sections, {} dependency projected, {} adopted, {} healed, {} canonicalized, {} legacy children, {} warnings",
        result.marked_next.len(),
        result.marked_in_progress.len(),
        result.cleared.len(),
        result.cleared_in_progress.len(),
        result.marked_blocked.len(),
        result.unblocked.len(),
        result.struck_completed_references.len(),
        result.moved_completed_references.len(),
        result.marker_added_references.len(),
        result.marker_removed_references.len(),
        result.removed_canceled_references.len(),
        result.removed_duplicate_lines.len(),
        result.removed_empty_pomodoros.len(),
        result.grouped_task_sections.len(),
        result.dependency_projection_updates.len(),
        result.adopted_dependency_lines.len(),
        result.healed_dependency_links.len(),
        result.canonicalized_dependency_lines.len(),
        result.legacy_dependency_children,
        result.dependency_warnings.len()
    );
}

pub(super) fn print_dependency_projection_sections(
    styler: &Styler,
    result: &SyncResult,
) {
    for (entries, dry_heading, heading) in [
        (
            &result.adopted_dependency_lines,
            "would adopt dependency lines",
            "adopted dependency lines",
        ),
        (
            &result.healed_dependency_links,
            "would heal dependency links",
            "healed dependency links",
        ),
        (
            &result.canonicalized_dependency_lines,
            "would canonicalize dependency lines",
            "canonicalized dependency lines",
        ),
    ] {
        if entries.is_empty() {
            continue;
        }
        println!();
        println!("  {}", if result.dry_run { dry_heading } else { heading });
        for entry in entries {
            println!(
                "    {}  line {}  {}",
                styler.cyan(&entry.path),
                entry.line,
                entry.detail
            );
        }
    }
    if !result.dependency_projection_updates.is_empty() {
        println!();
        println!(
            "  {} dependency fields",
            if result.dry_run {
                "would project"
            } else {
                "projected"
            }
        );
        for entry in &result.dependency_projection_updates {
            println!(
                "    {} {}  line {}  {}",
                styler.cyan(&entry.path),
                entry.kind,
                entry.line,
                entry.detail
            );
        }
    }
    if result.legacy_dependency_children > 0 {
        println!();
        println!(
            "  {} legacy dependency {} (counted for the legacy-window retirement decision)",
            result.legacy_dependency_children,
            if result.legacy_dependency_children == 1 {
                "child"
            } else {
                "children"
            }
        );
    }
}

pub(super) fn print_grouped_task_sections(
    styler: &Styler,
    result: &SyncResult,
) {
    if result.grouped_task_sections.is_empty() {
        return;
    }
    println!();
    println!(
        "  {} task sections",
        if result.dry_run {
            "would group"
        } else {
            "grouped"
        }
    );
    for section in &result.grouped_task_sections {
        let heading = section.heading_ancestry.join(" > ");
        println!(
            "    {}  {}  open {} \u{b7} next/in progress {} \u{b7} blocked {} \u{b7} done/canceled {} \u{b7} moved {}",
            styler.cyan(&section.path),
            heading,
            section.open,
            section.next_and_in_progress,
            section.blocked,
            section.done_and_canceled,
            section.moved_block_count
        );
    }
}

pub(super) fn print_dependency_status_section(
    styler: &Styler,
    blocking: bool,
    heading: &str,
    changes: &[DependencyStatusChange],
) {
    if changes.is_empty() {
        return;
    }
    println!();
    println!("  {heading}");
    let description_width = changes
        .iter()
        .map(|change| display_width(&change.description))
        .max()
        .unwrap_or(0);
    for change in changes {
        let transition = format!("[{}] \u{2192} [{}]", change.from, change.to);
        let transition = if blocking {
            styler.yellow(&transition)
        } else {
            styler.green(&transition)
        };
        let description = pad_right(&change.description, description_width);
        let reasons = if blocking {
            let mut reasons = Vec::new();
            if let Some(scheduled) = &change.future_scheduled_date {
                reasons.push(format!("scheduled: {scheduled}"));
            }
            if !change.open_dependency_ids.is_empty() {
                reasons.push(format!(
                    "open: {}",
                    change.open_dependency_ids.join(", ")
                ));
            }
            if reasons.is_empty() {
                String::new()
            } else {
                format!(" ({})", reasons.join("; "))
            }
        } else if change.unresolved_dependency_ids.is_empty() {
            String::new()
        } else {
            format!(
                " (unresolved: {})",
                change.unresolved_dependency_ids.join(", ")
            )
        };
        println!(
            "    {transition}  {description}  {}{}{}",
            styler.cyan(&change.path),
            if change.block_id.is_empty() {
                String::new()
            } else {
                format!(" ^{}", change.block_id)
            },
            reasons
        );
    }
}

pub(super) fn print_duplicate_line_section(result: &SyncResult) {
    if result.removed_duplicate_lines.is_empty() {
        return;
    }
    println!();
    println!(
        "  {} duplicate task-link lines",
        if result.dry_run {
            "would remove"
        } else {
            "removed"
        }
    );
    for item in &result.removed_duplicate_lines {
        let identities = item
            .duplicate_tasks
            .iter()
            .map(|task| format!("{}#^{}", task.path, task.block_id))
            .collect::<Vec<_>>()
            .join(", ");
        println!(
            "    line {}  {}  {}  {}",
            item.line_number,
            item.line.trim(),
            item.pomodoro,
            identities
        );
    }
}

pub(super) fn print_empty_pomodoro_section(result: &SyncResult) {
    if result.removed_empty_pomodoros.is_empty() {
        return;
    }
    println!();
    println!(
        "  {} empty Pomodoros",
        if result.dry_run {
            "would remove"
        } else {
            "removed"
        }
    );
    for item in &result.removed_empty_pomodoros {
        println!("    line {}  {}", item.line_number, item.line);
    }
}

pub(super) fn print_canceled_reference_section(result: &SyncResult) {
    if result.removed_canceled_references.is_empty() {
        return;
    }
    println!();
    println!(
        "  {} list items containing canceled task references",
        if result.dry_run {
            "would remove"
        } else {
            "removed"
        }
    );
    for item in &result.removed_canceled_references {
        println!(
            "    [[{}#^{}]]  line {}  {}",
            item.target, item.block_id, item.line_number, item.pomodoro
        );
    }
}

pub(super) fn print_marker_reference_sections(result: &SyncResult) {
    for (items, dry_heading, heading, marker) in [
        (
            &result.marker_added_references,
            "would mark",
            "marked",
            "🍅",
        ),
        (
            &result.marker_removed_references,
            "would unmark",
            "unmarked",
            "",
        ),
    ] {
        if items.is_empty() {
            continue;
        }
        println!();
        println!(
            "  {} Pomodoro references",
            if result.dry_run { dry_heading } else { heading }
        );
        for item in items {
            println!(
                "    {}[[{}#^{}]]  {}",
                if marker.is_empty() { "" } else { "🍅 " },
                item.target,
                item.block_id,
                item.pomodoro
            );
        }
    }
}

pub(super) fn print_completed_reference_sections(result: &SyncResult) {
    if !result.struck_completed_references.is_empty() {
        println!();
        println!(
            "  {} completed references",
            if result.dry_run {
                "would retire"
            } else {
                "retired"
            }
        );
        for item in &result.struck_completed_references {
            println!(
                "    ~~[[{}#^{}]]~~  {}{}",
                item.target,
                item.block_id,
                item.pomodoro,
                if item.removed_embed {
                    " (removed embed)"
                } else {
                    ""
                }
            );
        }
    }
    if !result.moved_completed_references.is_empty() {
        println!();
        println!(
            "  {} completed references",
            if result.dry_run {
                "would move"
            } else {
                "moved"
            }
        );
        for item in &result.moved_completed_references {
            println!(
                "    [[{}#^{}]]  {} -> {}",
                item.target,
                item.block_id,
                item.source_pomodoro,
                item.destination_pomodoro
            );
        }
    }
}

pub(super) fn print_change_section(
    styler: &Styler,
    heading: &str,
    transition: &str,
    changes: &[ChangeItem],
    promotion: bool,
) {
    if changes.is_empty() {
        return;
    }
    println!();
    println!("  {heading}");
    let description_width = changes
        .iter()
        .map(|change| display_width(&change.description))
        .max()
        .unwrap_or(0);
    for change in changes {
        let transition = if promotion {
            styler.green(transition)
        } else {
            styler.yellow(transition)
        };
        let description = pad_right(&change.description, description_width);
        let block = if change.block_id.is_empty() {
            String::new()
        } else {
            format!(" ^{}", change.block_id)
        };
        println!(
            "    {transition}  {description}  {}{block}{}",
            styler.cyan(&change.path),
            if change.dependency {
                " (dependency)"
            } else {
                ""
            }
        );
    }
}

/// One plan-budget stats line after the sync stats. Over-cap meters
/// are red. Individual plan lints are never printed here: this command
/// runs every 15 minutes, so it points at `bob plan` instead.
pub(super) fn print_plan_budget_line(result: &SyncResult) {
    let Some(budget) = &result.plan_budget else {
        return;
    };
    let styler = Styler::detect();
    let themes =
        format!("{}/{} themes", budget.themes.count, budget.themes.cap);
    let links = format!("{}/{} links", budget.links.count, budget.links.cap);
    let today = format!("TODAY {}", budget.today.count);
    let pending = format!("{}/{}", budget.pending.count, budget.pending.cap);
    let next = format!("{}/{}", budget.next.count, budget.next.cap);
    let themes = if budget.themes.over {
        styler.red(&themes)
    } else {
        themes
    };
    let links = if budget.links.over {
        styler.red(&links)
    } else {
        links
    };
    let pending = if budget.pending.over {
        styler.red(&pending)
    } else {
        pending
    };
    let next = if budget.next.over {
        styler.red(&next)
    } else {
        next
    };
    let mut line = format!(
        "  plan {themes} \u{b7} {links} \u{b7} {today} \u{b7} PENDING {pending} \u{b7} NEXT {next}"
    );
    if !budget.warnings.is_empty() {
        let count = budget.warnings.len();
        line.push_str(&format!(
            " \u{b7} {count} plan {} (run bob plan)",
            if count == 1 { "warning" } else { "warnings" }
        ));
    }
    println!("{line}");
}

pub(super) fn print_warnings(result: &SyncResult) {
    let styler = Styler::detect();
    for warning in &result.unresolved_references {
        eprintln!(
            "{}: [[{}#^{}]] \u{2014} {}",
            styler.warning_prefix(),
            warning.target,
            warning.block_id,
            warning.reason
        );
    }
    for warning in &result.dependency_warnings {
        eprintln!(
            "{}: {} {}:{} \u{2014} {}",
            styler.warning_prefix(),
            warning.kind,
            warning.path,
            warning.line,
            warning.detail
        );
    }
    for warning in &result.grouping_warnings {
        eprintln!(
            "{}: {}:{} {} \u{2014} {}",
            styler.warning_prefix(),
            warning.path,
            warning.original_heading_line,
            warning.heading_ancestry.join(" > "),
            warning.message
        );
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SyncError {
    pub(super) message: String,
    pub(super) reason: Option<String>,
    pub(super) applied_files: Vec<String>,
    pub(super) deferred_files: Vec<String>,
    pub(super) recovery_directory: Option<String>,
}

impl SyncError {
    // Read by `bob task reroll` when the Blocked registry check fails.
    pub(crate) fn message(&self) -> &str {
        &self.message
    }

    pub(super) fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            reason: None,
            applied_files: Vec::new(),
            deferred_files: Vec::new(),
            recovery_directory: None,
        }
    }

    pub(super) fn io(action: &str, path: &Path, error: io::Error) -> Self {
        Self::new(format!("failed to {action} {}: {error}", path.display()))
    }

    pub(super) fn from_apply(error: ApplyError) -> Self {
        Self::from_apply_with_root(error, None)
    }

    pub(super) fn from_apply_with_vault(
        error: ApplyError,
        vault: &Path,
    ) -> Self {
        Self::from_apply_with_root(error, Some(vault))
    }

    pub(super) fn from_apply_with_root(
        error: ApplyError,
        root: Option<&Path>,
    ) -> Self {
        let display = |path: &Path| {
            root.map(|root| display_report_path(root, path))
                .unwrap_or_else(|| path.display().to_string())
        };
        Self {
            message: error.message,
            reason: Some(error.reason.as_str().to_string()),
            applied_files: error
                .applied_files
                .iter()
                .map(|path| display(path))
                .collect(),
            deferred_files: error
                .deferred_files
                .iter()
                .map(|path| display(path))
                .collect(),
            recovery_directory: error
                .recovery_directory
                .map(|path| path.display().to_string()),
        }
    }
}

pub(super) fn print_error(error: SyncError, format: OutputFormat) -> i32 {
    match format {
        OutputFormat::Human => {
            eprintln!("{COMMAND_NAME}: {}", error.message);
            if let Some(recovery) = &error.recovery_directory {
                eprintln!("{COMMAND_NAME}: recovery copies: {recovery}");
            }
        }
        OutputFormat::Json => {
            println!(
                "{}",
                json!({
                    "ok": false,
                    "error": error.message,
                    "reason": error.reason,
                    "applied_files": error.applied_files,
                    "deferred_files": error.deferred_files,
                    "recovery_directory": error.recovery_directory,
                    "plan_budget": serde_json::Value::Null,
                })
            )
        }
    }
    1
}
