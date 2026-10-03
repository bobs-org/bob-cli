use crate::native::style::{
    display_width, pad_right, terminal_width, truncate, Styler,
};

use super::model::{
    DiffKind, FileAction, FileDiff, FileSync, PluginsReport, PluginsResult,
    SyncReport, SyncState, VaultState,
};

const DETAIL_INDENT: &str = "             ";

impl SyncState {
    fn label(self, color: bool) -> String {
        let word = match self {
            Self::Synced => "synced",
            Self::Drift => "drift",
            Self::Missing => "missing",
        };
        match self.glyph().filter(|_| color) {
            Some(glyph) => format!("{glyph} {word}"),
            None => word.to_string(),
        }
    }

    fn glyph(self) -> Option<&'static str> {
        match self {
            Self::Synced => Some("\u{2713}"),
            Self::Drift => Some("\u{26a0}"),
            Self::Missing => Some("\u{2717}"),
        }
    }

    fn paint(self, text: &str, styler: &Styler) -> String {
        match self {
            Self::Synced => styler.green(text),
            Self::Drift => styler.yellow(text),
            Self::Missing => styler.red(text),
        }
    }
}

impl VaultState {
    fn label(self) -> &'static str {
        match self {
            Self::Enabled => "enabled",
            Self::Disabled => "disabled",
            Self::NotInstalled => "not installed",
        }
    }

    fn paint(self, text: &str, styler: &Styler) -> String {
        match self {
            Self::Enabled => styler.green(text),
            Self::Disabled => styler.dim(text),
            Self::NotInstalled => styler.red(text),
        }
    }
}

pub(super) fn print_sync_report(
    report: &SyncReport,
    dry_run: bool,
    styler: &Styler,
) {
    let separator = styler.separator();
    println!(
        "Bob Plugins {separator} sync {separator} {} -> {}",
        report.repo.display(),
        report.bob_dir.display()
    );
    println!();

    let id_width = report
        .plugins
        .iter()
        .map(|plugin| display_width(&plugin.id))
        .max()
        .unwrap_or(0);

    for plugin in &report.plugins {
        let id = styler.cyan(&pad_right(&plugin.id, id_width));
        let changed = plugin
            .files
            .iter()
            .filter(|file| file.action != FileAction::Unchanged)
            .collect::<Vec<_>>();

        if changed.is_empty() {
            let prefix = styler.success_prefix(dry_run);
            println!("  {prefix} {id}  up to date");
            continue;
        }

        for file in changed {
            let prefix = if file.action.is_warning() {
                styler.warning_prefix()
            } else {
                styler.success_prefix(dry_run)
            };
            let detail = file_action_detail(file, dry_run, styler);
            println!("  {prefix} {id}  {detail}");
            print_file_diff(file, styler);
            print_backup_outcome(file, dry_run, styler);
        }
    }

    println!();
    let copied_label = if dry_run { "to copy" } else { "copied" };
    let mut summary = format!(
        "{} {copied_label} {separator} {} skipped {separator} {} unchanged",
        report.copied(),
        report.skipped(),
        report.unchanged()
    );
    if !report.issues.is_empty() {
        summary
            .push_str(&format!(" {separator} {} errors", report.issues.len()));
    }
    let show_backup_footer = (dry_run && report.has_backup_paths())
        || (!dry_run && report.has_written_backups());
    if show_backup_footer {
        let backup_label = if dry_run {
            "backups would go in"
        } else {
            "backups in"
        };
        summary.push_str(&format!(
            " {separator} {backup_label} {}",
            report.backup_run_dir.display()
        ));
    }
    println!("{summary}");
}

fn file_action_detail(
    file: &FileSync,
    dry_run: bool,
    styler: &Styler,
) -> String {
    let name = &file.name;
    let copy_verb = if dry_run { "would copy" } else { "copied" };
    let mut detail = match file.action {
        FileAction::Created => match &file.diff {
            Some(FileDiff::NewFile { lines, .. }) => {
                format!(
                    "{copy_verb} {name} (new file, {})",
                    pluralize(*lines, "line")
                )
            }
            _ => format!("{copy_verb} {name} (new)"),
        },
        FileAction::Updated => format!("{copy_verb} {name}"),
        FileAction::Forced => {
            format!("{copy_verb} {name} (overwrote dirty vault file)")
        }
        FileAction::SkippedDirty => {
            format!("skipped {name} (dirty in vault; use -F/--force)")
        }
        FileAction::Failed => format!("failed to copy {name} (see error)"),
        FileAction::Unchanged => format!("{name} unchanged"),
    };

    if let Some(stat) = diff_stat(&file.diff, styler) {
        detail.push_str("   ");
        detail.push_str(&stat);
    }

    detail
}

fn diff_stat(diff: &Option<FileDiff>, styler: &Styler) -> Option<String> {
    match diff {
        Some(FileDiff::Text { added, removed, .. }) => Some(format!(
            "{} {}",
            styler.green(&format!("+{added}")),
            styler.red(&format!("-{removed}"))
        )),
        _ => None,
    }
}

fn print_file_diff(file: &FileSync, styler: &Styler) {
    let Some(diff) = &file.diff else {
        return;
    };

    match diff {
        FileDiff::Text { lines, hidden, .. } => {
            let line_width =
                terminal_width().saturating_sub(display_width(DETAIL_INDENT));
            for line in lines {
                let text = truncate(&line.text, line_width);
                println!(
                    "{DETAIL_INDENT}{}",
                    paint_diff_line(line.kind, &text, styler)
                );
            }
            if *hidden > 0 {
                let text = format!("... and {} more diff lines", hidden);
                println!("{DETAIL_INDENT}{}", styler.dim(&text));
            }
        }
        FileDiff::Binary { old_len, new_len } => {
            let text = format!(
                "binary or minified file differs ({} -> {})",
                format_bytes(*old_len),
                format_bytes(*new_len)
            );
            println!("{DETAIL_INDENT}{}", styler.dim(&text));
        }
        FileDiff::NewFile { .. } => {}
    }
}

fn paint_diff_line(kind: DiffKind, text: &str, styler: &Styler) -> String {
    match kind {
        DiffKind::Hunk => styler.dim(text),
        DiffKind::Add => styler.green(text),
        DiffKind::Del => styler.red(text),
        DiffKind::Context => text.to_string(),
    }
}

fn print_backup_outcome(file: &FileSync, dry_run: bool, styler: &Styler) {
    let Some(backup) = &file.backup else {
        return;
    };

    let label = if dry_run {
        "would back up to"
    } else if backup.written {
        "backed up to"
    } else {
        "backup failed at"
    };
    let text = format!("\u{21b3} {label} {}", backup.path.display());
    let rendered = if backup.written || dry_run {
        styler.blue(&text)
    } else {
        styler.red(&text)
    };
    println!("{DETAIL_INDENT}{rendered}");
}

fn pluralize(count: usize, singular: &str) -> String {
    if count == 1 {
        format!("1 {singular}")
    } else {
        format!("{count} {singular}s")
    }
}

fn format_bytes(bytes: usize) -> String {
    if bytes < 1024 {
        return format!("{bytes} B");
    }

    let kib = bytes as f64 / 1024.0;
    if kib < 1024.0 {
        return format!("{kib:.1} KB");
    }

    format!("{:.1} MB", kib / 1024.0)
}

pub(super) fn print_plugins_table(report: &PluginsReport, styler: &Styler) {
    let separator = styler.separator();
    println!(
        "Bob Plugins {separator} {} {separator} {}",
        report.plugins.len(),
        report.repo.display()
    );
    println!();

    let widths = ColumnWidths::from_report(report, styler);
    println!(
        "  {:id$}  {:version$}  {:sync$}  {:vault$}  DESCRIPTION",
        "PLUGIN",
        "VERSION",
        "SYNC",
        "VAULT",
        id = widths.id,
        version = widths.version,
        sync = widths.sync,
        vault = widths.vault,
    );

    for plugin in &report.plugins {
        let id = styler.cyan(&pad_right(&plugin.id, widths.id));
        let version = styler.dim(&pad_right(&plugin.version, widths.version));
        let sync_label = plugin.sync.label(styler.is_color());
        let sync = plugin
            .sync
            .paint(&pad_right(&sync_label, widths.sync), styler);
        let vault = plugin
            .vault
            .paint(&pad_right(plugin.vault.label(), widths.vault), styler);
        let description =
            styler.dim(&truncate(&plugin.description, widths.description));
        println!("  {id}  {version}  {sync}  {vault}  {description}");
    }

    println!();
    let counts = report.counts();
    println!(
        "{} synced {separator} {} drift {separator} {} not installed",
        counts.synced, counts.drift, counts.not_installed
    );
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ColumnWidths {
    id: usize,
    version: usize,
    sync: usize,
    vault: usize,
    description: usize,
}

impl ColumnWidths {
    fn from_report(report: &PluginsReport, styler: &Styler) -> Self {
        let color = styler.is_color();
        let id = max_width(report.plugins.iter().map(|p| p.id.as_str()))
            .max("PLUGIN".len());
        let version =
            max_width(report.plugins.iter().map(|p| p.version.as_str()))
                .max("VERSION".len());
        let sync = report
            .plugins
            .iter()
            .map(|p| display_width(&p.sync.label(color)))
            .max()
            .unwrap_or(0)
            .max("SYNC".len());
        let vault = report
            .plugins
            .iter()
            .map(|p| display_width(p.vault.label()))
            .max()
            .unwrap_or(0)
            .max("VAULT".len());

        // Give whatever horizontal room is left to DESCRIPTION. Five column
        // gaps of two spaces plus the two-space left margin precede it.
        let fixed = 2 + id + 2 + version + 2 + sync + 2 + vault + 2;
        let description = terminal_width()
            .saturating_sub(fixed)
            .max("DESCRIPTION".len());

        Self {
            id,
            version,
            sync,
            vault,
            description,
        }
    }
}

fn max_width<'a>(values: impl Iterator<Item = &'a str>) -> usize {
    values.map(display_width).max().unwrap_or(0)
}

pub(super) fn success_json(result: &PluginsResult) -> String {
    serde_json::to_string(result).expect("serialize plugins result")
}
