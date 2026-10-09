//! Sync and scan reporting and summaries.
use super::*;
use crate::native::ref_tasks::LocatedRefTask;

pub(super) fn print_pdf_sync_report(
    operation: &str,
    config: &Config,
    plan: &PdfSyncPlan,
    options: SyncOptions,
) {
    print_config_report(operation, config);
    println!("pdf: {}", plan.pdf.display());
    println!("note: {}", plan.note_path.display());
    println!(
        "sidecar: {}",
        plan.sidecar_path
            .as_ref()
            .map(|path| path.display().to_string())
            .unwrap_or_else(|| "none".to_string())
    );
    println!("dry_run: {}", options.dry_run);
    println!("write_pdf: {}", options.write_pdf);
    if let Some(prefer) = options.prefer {
        println!("prefer: {}", prefer.as_str());
    }
    println!("marker_page: {}", plan.marker.page_number);
    println!("marker_note: {}", plan.marker.note_number);
    println!("sync_source: {}", plan.decision.source.as_str());
    println!("sync_reason: {}", plan.decision.reason);
    println!("pdf_task: {}", plan.pdf_task_signal.status.label());
    if let Some(status) = plan.pdf_task_signal.status_contributed {
        println!("pdf_task_contribution: status={status}");
    }
    if let Some(label) = status_normalization_label(plan.status_normalization) {
        println!("status_normalization: {label}");
    }
    if plan.decision.source == SyncSource::AutoMerge {
        println!(
            "sync_marker_contributed: {}",
            plan.decision.marker_contributed
        );
        println!(
            "sync_frontmatter_contributed: {}",
            plan.decision.frontmatter_contributed
        );
    }
    if let Some(count) = plan.rendered_highlights_count {
        println!("highlights_count: {count}");
    }
    if let Some(rendered) = &plan.rendered_highlights
        && (rendered.image_count > 0 || !plan.image_assets.is_empty())
    {
        println!("images: {}", rendered.image_count);
        println!("image_assets: {}", planned_image_asset_write_count(plan));
    }
    println!("annotation_tasks_create: {}", plan.annotation_tasks_created);
    println!("annotation_tasks_skip: {}", plan.annotation_tasks_skipped);
    println!(
        "routed_task_note_writes: {}",
        planned_routed_note_write_count(plan)
    );
    for line in reading_plan_report_lines(plan) {
        println!("{line}");
    }
}

pub(super) fn print_sync_write_report(report: &SyncWriteReport) {
    println!("note_action: {}", report.note_action);
    println!("pdf_marker_action: {}", report.marker_action);
    if report.image_count > 0 || report.image_assets_written > 0 {
        println!("images: {}", report.image_count);
        println!("image_assets_written: {}", report.image_assets_written);
        println!("image_assets_skipped: {}", report.image_assets_skipped);
    }
    println!(
        "annotation_tasks_created: {}",
        report.annotation_tasks_created
    );
    println!(
        "annotation_tasks_skipped: {}",
        report.annotation_tasks_skipped
    );
    println!("routed_task_note_writes: {}", report.routed_note_actions);
    for line in reading_write_report_lines(report) {
        println!("{line}");
    }
    println!("writes: {}", write_summary(report));
}

pub(super) fn write_summary(report: &SyncWriteReport) -> &'static str {
    let note_writes = report.note_action != "none"
        || report.routed_note_actions > 0
        || report.image_assets_written > 0;
    match (note_writes, report.marker_action != "none") {
        (false, false) => "none",
        (false, true) => "pdf",
        (true, false) => "note",
        (true, true) => "note,pdf",
    }
}

/// One PDF's reading-task plan as a verbose `reading_task:` line naming
/// the destination, preview ID, adoption, or planned line edit.
pub(super) fn reading_plan_report_lines(plan: &PdfSyncPlan) -> Vec<String> {
    let Some(reading) = plan.reading_task_plan.as_ref() else {
        return vec!["reading_task: none (unplanned)".to_string()];
    };
    let located = plan.reading_located.as_ref();
    let located_suffix = |located: Option<&LocatedRefTask>| match located {
        Some(task) => {
            let id = task.block_id.as_deref().unwrap_or("pending");
            format!("{} ^{id}", task.path)
        }
        None => "pending".to_string(),
    };
    match &reading.action {
        ReadingTaskAction::Insert {
            destination_route,
            prefer_block_id,
            ..
        } => {
            let dest = format!("{destination_route}.md");
            let preview = plan.preview_block_id.as_deref().unwrap_or("pending");
            let verb = if reading.kind == ReadingTaskKind::Reopen {
                "reopen"
            } else {
                "insert"
            };
            let mut line = format!("reading_task: {verb} {dest} ^{preview}");
            if let Some(old) = prefer_block_id {
                if old == preview {
                    line.push_str(&format!(" (keeps ^{old})"));
                } else {
                    line.push_str(&format!(" (old ^{old} taken)"));
                }
            }
            vec![line]
        }
        ReadingTaskAction::LineEdit { target_mark } => {
            vec![format!(
                "reading_task: line-edit {} -> [{target_mark}]",
                located_suffix(located)
            )]
        }
        ReadingTaskAction::NoWrite => {
            if reading.kind == ReadingTaskKind::Birth {
                return vec![format!(
                    "reading_task: adopt {}",
                    located_suffix(located)
                )];
            }
            match located {
                Some(_) => vec![format!(
                    "reading_task: unchanged {}",
                    located_suffix(located)
                )],
                None => vec![format!(
                    "reading_task: none ({})",
                    reading_kind_label(&reading.kind)
                )],
            }
        }
    }
}

fn reading_kind_label(kind: &ReadingTaskKind) -> &'static str {
    match kind {
        ReadingTaskKind::V1 => "v1",
        ReadingTaskKind::Birth => "birth",
        ReadingTaskKind::Existing => "existing",
        ReadingTaskKind::ClosedTerminal => "closed",
        ReadingTaskKind::Reopen => "reopen",
        ReadingTaskKind::Missing => "missing",
        ReadingTaskKind::Refused => "refused",
    }
}

/// One PDF's performed reading-task write as `reading_task_created:` /
/// `reading_task_updated:` lines naming the actual destination and final ID.
/// Adoptions, unchanged tasks, and v1 notes print nothing.
pub(super) fn reading_write_report_lines(
    report: &SyncWriteReport,
) -> Vec<String> {
    let mut lines = Vec::new();
    if let Some(created) = report.reading_created.as_ref() {
        lines.push(format!(
            "reading_task_created: {} ^{}",
            created.dest, created.id
        ));
    }
    if let Some(updated) = report.reading_updated.as_ref() {
        lines.push(format!(
            "reading_task_updated: {} ^{}",
            updated.dest, updated.id
        ));
    }
    lines
}

/// Aggregate reading-task counts for one scan's human summary lines.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) struct ReadingRollup {
    pub(super) created: usize,
    /// Per-destination creation counts, sorted by destination.
    pub(super) created_by_dest: Vec<(String, usize)>,
    pub(super) updated: usize,
}

impl ReadingRollup {
    /// Roll up actual successful writes. Adoptions and unchanged tasks
    /// never counted; failures never produce reports.
    pub(super) fn from_reports(reports: &[SyncWriteReport]) -> Self {
        let mut by_dest: BTreeMap<&str, usize> = BTreeMap::new();
        let mut updated = 0usize;
        for report in reports {
            if let Some(created) = report.reading_created.as_ref() {
                *by_dest.entry(created.dest.as_str()).or_default() += 1;
            }
            if report.reading_updated.is_some() {
                updated += 1;
            }
        }
        Self {
            created: by_dest.values().sum(),
            created_by_dest: by_dest
                .into_iter()
                .map(|(dest, count)| (dest.to_string(), count))
                .collect(),
            updated,
        }
    }

    /// Roll up planned writes for dry-run and verbose plan summaries.
    pub(super) fn from_plans(plans: &[&PdfSyncPlan]) -> Self {
        let mut by_dest: BTreeMap<String, usize> = BTreeMap::new();
        let mut updated = 0usize;
        for plan in plans {
            match plan
                .reading_task_plan
                .as_ref()
                .map(|reading| &reading.action)
            {
                Some(ReadingTaskAction::Insert {
                    destination_route, ..
                }) => {
                    *by_dest
                        .entry(format!("{destination_route}.md"))
                        .or_default() += 1;
                }
                Some(ReadingTaskAction::LineEdit { .. }) => {
                    updated += 1;
                }
                _ => {}
            }
        }
        Self {
            created: by_dest.values().sum(),
            created_by_dest: by_dest.into_iter().collect(),
            updated,
        }
    }
}

/// Scan-scoped count of references that still need `bob ref migrate-tasks`:
/// open v1 trackers, whether the note stays on the v1 branch or a v2 task
/// already claims the ref alongside the leftover tracker.
pub(super) fn count_open_v1(plans: &[&PdfSyncPlan]) -> usize {
    plans
        .iter()
        .filter(|plan| {
            plan.reading_task_plan.as_ref().is_some_and(|reading| {
                reading
                    .diagnostics
                    .iter()
                    .any(|diagnostic| diagnostic.code == "open_v1_tracker")
            })
        })
        .count()
}

/// Print the concise reading-task summary lines after a scan summary:
/// creations with their per-destination breakdown, updates, and the
/// migrate-tasks reminder. Each line prints only when its count is nonzero,
/// so no-op scans stay quiet.
pub(super) fn print_reading_summary_lines(
    rollup: &ReadingRollup,
    open_v1: usize,
) {
    if rollup.created > 0 {
        let mut line = format!("📖 {} reading tasks created", rollup.created);
        for (dest, count) in &rollup.created_by_dest {
            line.push_str(&format!(" · {dest} ({count})"));
        }
        println!("{line}");
    }
    if rollup.updated > 0 {
        println!("↻ {} reading tasks updated", rollup.updated);
    }
    if open_v1 > 0 {
        println!("{open_v1} open v1 ref tasks · run bob ref migrate-tasks");
    }
}

pub(super) fn planned_routed_note_write_count(plan: &PdfSyncPlan) -> usize {
    plan.routed_task_note_writes
        .iter()
        .filter(|write| write.action != "none")
        .count()
}

pub(super) fn planned_image_asset_write_count(plan: &PdfSyncPlan) -> usize {
    plan.image_assets
        .iter()
        .filter(|write| write.action == ImageAssetAction::Copy)
        .count()
}

pub(super) fn status_normalization_label(
    normalization: StatusNormalization,
) -> Option<String> {
    let mut labels = Vec::new();
    for kind in [
        DeprecatedStatusNormalization::UnreadToReady,
        DeprecatedStatusNormalization::DoneToRead,
    ] {
        let sources = [
            ("marker", normalization.marker),
            ("frontmatter", normalization.frontmatter),
            ("base", normalization.base),
        ]
        .into_iter()
        .filter_map(|(source, value)| (value == Some(kind)).then_some(source))
        .collect::<Vec<_>>();
        if !sources.is_empty() {
            labels.push(format!("{} ({})", kind.label(), sources.join(",")));
        }
    }
    (!labels.is_empty()).then(|| labels.join("; "))
}

pub(super) fn print_verbose_scan_plan_report(
    config: &Config,
    options: SyncOptions,
    pdf_count: usize,
    intake: &[IntakeMove],
    plan_outcomes: &[ScanPlanOutcome],
) {
    print_config_report("scan", config);
    println!("dry_run: {}", options.dry_run);
    println!("write_pdfs: {}", options.write_pdf);
    println!("ob_sync: not-run");
    println!("pdf_count: {pdf_count}");
    println!("intake_moves: {}", intake.len());
    let audio_moves = intake_audio_file_moves(intake);
    println!("intake_audio_moves: {}", audio_moves.len());
    let action = if options.dry_run {
        "would-move"
    } else {
        "moved"
    };
    for intake_move in intake {
        println!(
            "intake: {action} {} -> {}",
            display_vault_relative_path(config, &intake_move.source),
            display_vault_relative_path(config, &intake_move.destination)
        );
        for (source, destination) in &intake_move.companions {
            if is_audio_companion_path(source) {
                println!(
                    "intake: {action} {} -> {}",
                    display_vault_relative_path(config, source),
                    display_vault_relative_path(config, destination)
                );
            }
        }
    }
    for outcome in plan_outcomes {
        match outcome {
            ScanPlanOutcome::Planned(plan) => print_scan_plan_entry(plan),
            ScanPlanOutcome::Failed(failure) => {
                print_scan_plan_failure_entry(failure);
            }
        }
    }
}

pub(super) fn print_scan_header(
    config: &Config,
    pdf_count: usize,
    dry_run: bool,
    styler: &Styler,
) {
    let separator = styler.separator();
    let mut header = format!(
        "Scanning {} {} in {}",
        pdf_count,
        plural(pdf_count, "PDF", "PDFs"),
        display_scan_lib_dir(config)
    );
    if dry_run {
        header.push_str(&format!(" {separator} dry-run"));
    }
    println!("{}", styler.dim(&header));
    println!();
}

pub(super) fn print_concise_intake_report(
    config: &Config,
    intake: &[IntakeMove],
    dry_run: bool,
    styler: &Styler,
) {
    for intake_move in intake {
        let action = if dry_run { "would move" } else { "moved" };
        println!(
            "  {} {} -> {}",
            styler.green(action),
            display_vault_relative_path(config, &intake_move.source),
            display_vault_relative_path(config, &intake_move.destination)
        );
        for (source, destination) in &intake_move.companions {
            if is_audio_companion_path(source) {
                println!(
                    "  {} {} -> {}",
                    styler.green(action),
                    display_vault_relative_path(config, source),
                    display_vault_relative_path(config, destination)
                );
            }
        }
    }
    if !intake.is_empty() {
        println!();
    }
}

pub(super) fn intake_audio_file_moves(
    intake: &[IntakeMove],
) -> Vec<(PathBuf, PathBuf)> {
    let mut moves = Vec::new();
    for intake_move in intake {
        if is_audio_companion_path(&intake_move.source) {
            moves.push((
                intake_move.source.clone(),
                intake_move.destination.clone(),
            ));
        }
        for (source, destination) in &intake_move.companions {
            if is_audio_companion_path(source) {
                moves.push((source.clone(), destination.clone()));
            }
        }
    }
    moves
}

pub(super) fn display_scan_lib_dir(config: &Config) -> String {
    config
        .lib_dir
        .strip_prefix(&config.bob_dir)
        .ok()
        .filter(|path| !path.as_os_str().is_empty())
        .map(display_path)
        .unwrap_or_else(|| config.lib_dir.display().to_string())
}

pub(super) fn display_vault_relative_path(
    config: &Config,
    path: &Path,
) -> String {
    display_path(Path::new(&vault_relative_path_value(config, path)))
}

pub(super) fn display_path(path: &Path) -> String {
    path.components()
        .map(|component| component.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

pub(super) fn print_concise_scan_plan_report(
    plan_outcomes: &[ScanPlanOutcome],
    plans: &[&PdfSyncPlan],
    pdf_count: usize,
    plan_failure_count: usize,
    styler: &Styler,
) {
    let lines = plan_outcomes
        .iter()
        .filter_map(|outcome| match outcome {
            ScanPlanOutcome::Planned(plan) => ScanLine::from_plan(plan),
            ScanPlanOutcome::Failed(failure) => {
                Some(ScanLine::from_failure(failure, false))
            }
        })
        .collect::<Vec<_>>();
    print_scan_lines(&lines, true, styler);
    println!(
        "{}",
        scan_summary_line(
            pdf_count,
            ScanCounts::from_plans(plans),
            plan_failure_count,
            "none",
            styler,
        )
    );
    print_reading_summary_lines(
        &ReadingRollup::from_plans(plans),
        count_open_v1(plans),
    );
}

pub(super) fn print_concise_scan_write_report(
    plan_outcomes: &[ScanPlanOutcome],
    write_outcomes: &[ScanWriteOutcome],
    reports: &[SyncWriteReport],
    pdf_count: usize,
    plan_failure_count: usize,
    write_failure_count: usize,
    styler: &Styler,
) {
    let mut write_index = 0usize;
    let mut lines = Vec::new();
    for outcome in plan_outcomes {
        match outcome {
            ScanPlanOutcome::Planned(plan) => {
                let write_outcome = write_outcomes.get(write_index);
                write_index += 1;
                match write_outcome {
                    Some(ScanWriteOutcome::Written(report)) => {
                        if let Some(line) = ScanLine::from_write(plan, report) {
                            lines.push(line);
                        }
                    }
                    Some(ScanWriteOutcome::Failed(failure)) => {
                        lines.push(ScanLine::from_failure(failure, true));
                    }
                    None => {}
                }
            }
            ScanPlanOutcome::Failed(failure) => {
                lines.push(ScanLine::from_failure(failure, false));
            }
        }
    }

    print_scan_lines(&lines, false, styler);
    println!(
        "{}",
        scan_summary_line(
            pdf_count,
            ScanCounts::from_reports(reports),
            plan_failure_count + write_failure_count,
            write_summary_from_reports(reports),
            styler,
        )
    );
    let planned: Vec<&PdfSyncPlan> = plan_outcomes
        .iter()
        .filter_map(|outcome| match outcome {
            ScanPlanOutcome::Planned(plan) => Some(plan.as_ref()),
            ScanPlanOutcome::Failed(_) => None,
        })
        .collect();
    print_reading_summary_lines(
        &ReadingRollup::from_reports(reports),
        count_open_v1(&planned),
    );
}

pub(super) fn print_scan_lines(
    lines: &[ScanLine],
    dry_run: bool,
    styler: &Styler,
) {
    let prefix_width = lines
        .iter()
        .map(|line| display_width(line.prefix_label(dry_run)))
        .max()
        .unwrap_or(0);
    let name_width = lines
        .iter()
        .map(|line| display_width(line.name()))
        .max()
        .unwrap_or(0);

    for line in lines {
        println!("{}", line.render(prefix_width, name_width, dry_run, styler));
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum ScanLine {
    Success {
        name: String,
        action: String,
        details: Vec<String>,
    },
    Failed {
        name: String,
        message: String,
    },
}

impl ScanLine {
    pub(super) fn from_plan(plan: &PdfSyncPlan) -> Option<Self> {
        scan_plan_changed(plan).then(|| Self::Success {
            name: scan_display_name(&plan.note_path),
            action: scan_change_action(
                plan.stable_note_action,
                plan.marker_write_needed,
                planned_image_asset_write_count(plan),
                planned_routed_note_write_count(plan),
                true,
            ),
            details: scan_details(plan, plan.annotation_tasks_created),
        })
    }

    pub(super) fn from_write(
        plan: &PdfSyncPlan,
        report: &SyncWriteReport,
    ) -> Option<Self> {
        scan_report_changed(report).then(|| Self::Success {
            name: scan_display_name(&plan.note_path),
            action: scan_change_action(
                report.note_action,
                report.marker_action != "none",
                report.image_assets_written,
                report.routed_note_actions,
                false,
            ),
            details: scan_details(plan, report.annotation_tasks_created),
        })
    }

    pub(super) fn from_failure(
        failure: &ScanFailure,
        write_failure: bool,
    ) -> Self {
        let message = if write_failure {
            format!("write failed: {}", failure.error)
        } else {
            failure.error.to_string()
        };
        Self::Failed {
            name: scan_display_name(&failure.pdf),
            message,
        }
    }

    pub(super) fn name(&self) -> &str {
        match self {
            Self::Success { name, .. } | Self::Failed { name, .. } => name,
        }
    }

    pub(super) fn prefix_label(&self, dry_run: bool) -> &'static str {
        match self {
            Self::Success { .. } => success_prefix_label(dry_run),
            Self::Failed { .. } => "error",
        }
    }

    pub(super) fn render(
        &self,
        prefix_width: usize,
        name_width: usize,
        dry_run: bool,
        styler: &Styler,
    ) -> String {
        let prefix_label = pad_right(self.prefix_label(dry_run), prefix_width);
        let prefix = match self {
            Self::Success { .. } => styler.green(&prefix_label),
            Self::Failed { .. } => styler.red(&prefix_label),
        };
        let name = styler.cyan(&pad_right(self.name(), name_width));

        match self {
            Self::Success {
                action, details, ..
            } => {
                let mut rendered = format!("  {prefix}  {name}  {action}");
                if !details.is_empty() {
                    let separator = format!(" {} ", styler.separator());
                    rendered.push_str("  ");
                    rendered.push_str(&styler.dim(&details.join(&separator)));
                }
                rendered
            }
            Self::Failed { message, .. } => {
                format!("  {prefix}  {name}  {message}")
            }
        }
    }
}

pub(super) fn success_prefix_label(dry_run: bool) -> &'static str {
    if dry_run {
        "[dry-run] ok"
    } else {
        "ok"
    }
}

pub(super) fn scan_plan_changed(plan: &PdfSyncPlan) -> bool {
    plan.stable_note_action != "none"
        || plan.marker_write_needed
        || planned_image_asset_write_count(plan) > 0
        || plan.annotation_tasks_created > 0
        || planned_routed_note_write_count(plan) > 0
}

pub(super) fn scan_report_changed(report: &SyncWriteReport) -> bool {
    report.note_action != "none"
        || report.marker_action != "none"
        || report.image_assets_written > 0
        || report.annotation_tasks_created > 0
        || report.routed_note_actions > 0
}

pub(super) fn scan_change_action(
    note_action: &str,
    marker_changed: bool,
    image_asset_count: usize,
    routed_note_count: usize,
    dry_run: bool,
) -> String {
    let mut targets = Vec::new();
    if note_action != "none" {
        targets.push("note".to_string());
    }
    if image_asset_count > 0 {
        targets.push(count_phrase(
            image_asset_count,
            "image asset",
            "image assets",
        ));
    }
    if routed_note_count > 0 {
        targets.push(
            plural(routed_note_count, "routed note", "routed notes")
                .to_string(),
        );
    }
    if marker_changed {
        targets.push("marker".to_string());
    }

    if targets.is_empty() {
        return "no changes".to_string();
    }

    let verb = match (dry_run, note_action) {
        (true, "create") => "would create",
        (true, _) => "would update",
        (false, "create") => "created",
        (false, _) => "updated",
    };
    format!("{verb} {}", targets.join(" + "))
}

pub(super) fn scan_details(
    plan: &PdfSyncPlan,
    annotation_tasks_created: usize,
) -> Vec<String> {
    let mut details = Vec::new();
    if let Some(count) = plan.rendered_highlights_count {
        details.push(count_phrase(count, "highlight", "highlights"));
    }
    if let Some(rendered) = &plan.rendered_highlights
        && rendered.image_count > 0
    {
        details.push(count_phrase(rendered.image_count, "image", "images"));
    }
    if annotation_tasks_created > 0 {
        details.push(format!(
            "+{}",
            count_phrase(annotation_tasks_created, "task", "tasks")
        ));
    }
    if plan.decision.source == SyncSource::AutoMerge {
        details.push(format!("auto-merge ({})", plan.decision.reason));
    }
    details
}

pub(super) fn scan_display_name(path: &Path) -> String {
    path.file_stem()
        .map(|stem| stem.to_string_lossy().replace(['_', '-'], " "))
        .unwrap_or_else(|| path.display().to_string())
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(super) struct ScanCounts {
    pub(super) creates: usize,
    pub(super) updates: usize,
    pub(super) unchanged: usize,
    pub(super) marker_updates: usize,
    pub(super) images: usize,
    pub(super) image_assets: usize,
    pub(super) annotation_tasks_created: usize,
    pub(super) annotation_tasks_skipped: usize,
    pub(super) routed_task_note_writes: usize,
}

impl ScanCounts {
    pub(super) fn from_plans(plans: &[&PdfSyncPlan]) -> Self {
        Self {
            creates: plans
                .iter()
                .filter(|plan| plan.stable_note_action == "create")
                .count(),
            updates: plans
                .iter()
                .map(|plan| {
                    usize::from(plan.stable_note_action == "update")
                        + planned_routed_note_write_count(plan)
                })
                .sum(),
            unchanged: plans
                .iter()
                .filter(|plan| plan.stable_note_action == "none")
                .count(),
            marker_updates: plans
                .iter()
                .filter(|plan| plan.marker_write_needed)
                .count(),
            images: plans
                .iter()
                .filter_map(|plan| plan.rendered_highlights.as_ref())
                .map(|rendered| rendered.image_count)
                .sum(),
            image_assets: plans
                .iter()
                .map(|plan| planned_image_asset_write_count(plan))
                .sum(),
            annotation_tasks_created: plans
                .iter()
                .map(|plan| plan.annotation_tasks_created)
                .sum(),
            annotation_tasks_skipped: plans
                .iter()
                .map(|plan| plan.annotation_tasks_skipped)
                .sum(),
            routed_task_note_writes: plans
                .iter()
                .map(|plan| planned_routed_note_write_count(plan))
                .sum(),
        }
    }

    pub(super) fn from_reports(reports: &[SyncWriteReport]) -> Self {
        Self {
            creates: reports
                .iter()
                .filter(|report| report.note_action == "create")
                .count(),
            updates: reports
                .iter()
                .map(|report| {
                    usize::from(report.note_action == "update")
                        + report.routed_note_actions
                })
                .sum(),
            unchanged: reports
                .iter()
                .filter(|report| report.note_action == "none")
                .count(),
            marker_updates: reports
                .iter()
                .filter(|report| report.marker_action != "none")
                .count(),
            images: reports.iter().map(|report| report.image_count).sum(),
            image_assets: reports
                .iter()
                .map(|report| report.image_assets_written)
                .sum(),
            annotation_tasks_created: reports
                .iter()
                .map(|report| report.annotation_tasks_created)
                .sum(),
            annotation_tasks_skipped: reports
                .iter()
                .map(|report| report.annotation_tasks_skipped)
                .sum(),
            routed_task_note_writes: reports
                .iter()
                .map(|report| report.routed_note_actions)
                .sum(),
        }
    }
}

pub(super) fn scan_summary_line(
    pdf_count: usize,
    counts: ScanCounts,
    failure_count: usize,
    writes: &str,
    styler: &Styler,
) -> String {
    let separator = styler.separator();
    let mut summary = format!(
        "{} {pdf_noun} {separator} {} created {separator} {} updated {separator} {} unchanged {separator} {} {marker_noun} {separator} {} {task_noun}",
        pdf_count,
        counts.creates,
        counts.updates,
        counts.unchanged,
        counts.marker_updates,
        counts.annotation_tasks_created,
        pdf_noun = plural(pdf_count, "pdf", "pdfs"),
        marker_noun = plural(counts.marker_updates, "marker", "markers"),
        task_noun = plural(counts.annotation_tasks_created, "task", "tasks"),
    );
    if counts.images > 0 {
        summary.push_str(&format!(
            " {separator} {}",
            count_phrase(counts.images, "image", "images")
        ));
    }
    if counts.image_assets > 0 {
        summary.push_str(&format!(
            " {separator} {}",
            count_phrase(counts.image_assets, "image asset", "image assets")
        ));
    }
    if failure_count > 0 {
        summary.push_str(&format!(
            " {separator} {}",
            styler.red(&count_phrase(failure_count, "failure", "failures"))
        ));
    }
    summary.push_str(&format!(" {separator} writes: {writes}"));
    summary
}

pub(super) fn write_summary_from_reports(
    reports: &[SyncWriteReport],
) -> &'static str {
    let note_writes = reports.iter().any(|report| {
        report.note_action != "none"
            || report.routed_note_actions > 0
            || report.image_assets_written > 0
    });
    let marker_writes =
        reports.iter().any(|report| report.marker_action != "none");
    match (note_writes, marker_writes) {
        (false, false) => "none",
        (true, false) => "note",
        (false, true) => "pdf",
        (true, true) => "note,pdf",
    }
}

pub(super) fn count_phrase(
    count: usize,
    singular: &str,
    plural_noun: &str,
) -> String {
    format!("{count} {}", plural(count, singular, plural_noun))
}

pub(super) fn plural<'a>(
    count: usize,
    singular: &'a str,
    plural_noun: &'a str,
) -> &'a str {
    if count == 1 {
        singular
    } else {
        plural_noun
    }
}

pub(super) fn print_scan_plan_entry(plan: &PdfSyncPlan) {
    println!("pdf: {}", plan.pdf.display());
    println!("  note: {}", plan.note_path.display());
    println!(
        "  sidecar: {}",
        plan.sidecar_path
            .as_ref()
            .map(|path| path.display().to_string())
            .unwrap_or_else(|| "none".to_string())
    );
    println!("  sync_source: {}", plan.decision.source.as_str());
    if plan.decision.source == SyncSource::AutoMerge {
        println!("  sync_reason: {}", plan.decision.reason);
    }
    println!("  pdf_task: {}", plan.pdf_task_signal.status.label());
    if let Some(status) = plan.pdf_task_signal.status_contributed {
        println!("  pdf_task_contribution: status={status}");
    }
    if let Some(label) = status_normalization_label(plan.status_normalization) {
        println!("  status_normalization: {label}");
    }
    println!("  note_action: {}", plan.stable_note_action);
    println!(
        "  pdf_marker_action: {}",
        if plan.marker_write_needed {
            "would-update"
        } else {
            "none"
        }
    );
    if let Some(count) = plan.rendered_highlights_count {
        println!("  highlights_count: {count}");
    }
    if let Some(rendered) = &plan.rendered_highlights
        && (rendered.image_count > 0 || !plan.image_assets.is_empty())
    {
        println!("  images: {}", rendered.image_count);
        println!("  image_assets: {}", planned_image_asset_write_count(plan));
    }
    println!(
        "  annotation_tasks_create: {}",
        plan.annotation_tasks_created
    );
    println!("  annotation_tasks_skip: {}", plan.annotation_tasks_skipped);
    println!(
        "  routed_task_note_writes: {}",
        planned_routed_note_write_count(plan)
    );
    for line in reading_plan_report_lines(plan) {
        println!("  {line}");
    }
}

pub(super) fn print_scan_plan_failure_entry(failure: &ScanFailure) {
    println!("pdf: {}", failure.pdf.display());
    println!("  plan_error: {}", failure.error);
}

pub(super) fn print_scan_write_failure_entry(failure: &ScanFailure) {
    println!("write_failure: {}", failure.pdf.display());
    println!("  error: {}", failure.error);
}

pub(super) fn print_scan_plan_summary(
    plans: &[&PdfSyncPlan],
    plan_failure_count: usize,
) {
    let counts = ScanCounts::from_plans(plans);
    println!("summary:");
    println!("  notes_create: {}", counts.creates);
    println!("  notes_update: {}", counts.updates);
    println!("  notes_unchanged: {}", counts.unchanged);
    if counts.images > 0 || counts.image_assets > 0 {
        println!("  images: {}", counts.images);
        println!("  image_assets: {}", counts.image_assets);
    }
    println!(
        "  annotation_tasks_create: {}",
        counts.annotation_tasks_created
    );
    println!(
        "  annotation_tasks_skip: {}",
        counts.annotation_tasks_skipped
    );
    println!(
        "  routed_task_note_writes: {}",
        counts.routed_task_note_writes
    );
    println!("  pdf_markers_would_update: {}", counts.marker_updates);
    let reading = ReadingRollup::from_plans(plans);
    println!("  reading_tasks_create: {}", reading.created);
    println!("  reading_tasks_update: {}", reading.updated);
    println!("  open_v1_ref_tasks: {}", count_open_v1(plans));
    println!("  pdfs_planned: {}", plans.len());
    println!("  plan_failures: {plan_failure_count}");
    println!("  scan_failures: {plan_failure_count}");
    if plan_failure_count > 0 {
        println!("result: partial-failure");
    }
}

pub(super) fn print_scan_write_summary(
    reports: &[SyncWriteReport],
    plans: &[&PdfSyncPlan],
    plan_failure_count: usize,
    write_failure_count: usize,
) {
    let counts = ScanCounts::from_reports(reports);
    println!("summary:");
    println!("  notes_created: {}", counts.creates);
    println!("  notes_updated: {}", counts.updates);
    println!("  notes_unchanged: {}", counts.unchanged);
    if counts.images > 0 || counts.image_assets > 0 {
        println!("  images: {}", counts.images);
        println!("  image_assets_written: {}", counts.image_assets);
    }
    println!(
        "  annotation_tasks_created: {}",
        counts.annotation_tasks_created
    );
    println!(
        "  annotation_tasks_skipped: {}",
        counts.annotation_tasks_skipped
    );
    println!(
        "  routed_task_note_writes: {}",
        counts.routed_task_note_writes
    );
    let reading = ReadingRollup::from_reports(reports);
    println!("  reading_tasks_created: {}", reading.created);
    println!("  reading_tasks_updated: {}", reading.updated);
    println!("  open_v1_ref_tasks: {}", count_open_v1(plans));
    println!("  pdf_markers_updated: {}", counts.marker_updates);
    println!("  write_successes: {}", reports.len());
    println!("  plan_failures: {plan_failure_count}");
    println!("  write_failures: {write_failure_count}");
    println!(
        "  scan_failures: {}",
        plan_failure_count + write_failure_count
    );
    if plan_failure_count + write_failure_count > 0 {
        println!("result: partial-failure");
    }
    println!("writes: {}", write_summary_from_reports(reports));
}

pub(super) fn scan_partial_failure_error(
    plan_failures: &[&ScanFailure],
    write_failures: &[&ScanFailure],
) -> CommandError {
    let total = plan_failures.len() + write_failures.len();
    let mut message = format!("scan completed with {total} per-PDF failure(s)");
    if !plan_failures.is_empty() {
        message.push_str("\nplanning failures:");
        for failure in plan_failures {
            message.push_str("\n  ");
            message.push_str(&failure.pdf.display().to_string());
            message.push_str(": ");
            message.push_str(&failure.error.to_string());
        }
    }
    if !write_failures.is_empty() {
        message.push_str("\nwrite failures:");
        for failure in write_failures {
            message.push_str("\n  ");
            message.push_str(&failure.pdf.display().to_string());
            message.push_str(": ");
            message.push_str(&failure.error.to_string());
        }
    }
    CommandError::new(message)
}
