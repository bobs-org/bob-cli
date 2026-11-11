use std::{
    ffi::OsString,
    fs, io, iter,
    path::{Path, PathBuf},
};

use clap::{
    builder::OsStringValueParser, Arg, ArgAction, ArgMatches,
    Command as ClapCommand,
};
use serde::Serialize;
use serde_json::json;

use super::{
    capture, env as bob_env,
    projects::{
        frontmatter_is_area, frontmatter_is_notebook, frontmatter_is_project,
        frontmatter_value, is_markdown_file, parse_frontmatter,
        project_name_aliases, ProjectStatus,
    },
    records::{context::RecordContext, RecordCatalog},
    style::{display_width, pad_right, Styler},
};

const COMMAND_NAME: &str = "bob capture-targets";

pub(crate) fn run(args: Vec<OsString>) -> i32 {
    let mut command = build_cli();
    let matches = match command.try_get_matches_from_mut(
        iter::once(OsString::from(COMMAND_NAME)).chain(args),
    ) {
        Ok(matches) => matches,
        Err(error) => return print_clap_error(error),
    };

    let output_format = OutputFormat::from_matches(&matches);
    let verbose = matches.get_flag("verbose");
    let bob_dir = bob_dir_from_matches(&matches);
    let report = scan_capture_targets_with_notebooks(&bob_dir);

    if report.issues.is_empty() {
        print_success(&report.result(&bob_dir), output_format);
        if verbose {
            for warning in &report.warnings {
                eprintln!("{COMMAND_NAME}: {}", warning.display());
            }
        }
        return 0;
    }

    print_scan_error(&report, &bob_dir, output_format, verbose);
    1
}

fn print_clap_error(error: clap::Error) -> i32 {
    let exit_code = error.exit_code();
    if let Err(print_error) = error.print() {
        // A closed pipe renders nothing: stay quiet instead of
        // reporting the failed render, but keep clap's own exit
        // code so a usage error never reports success.
        if print_error.kind() != std::io::ErrorKind::BrokenPipe {
            eprintln!(
                "{COMMAND_NAME}: failed to print command-line error: {print_error}"
            );
        }
    }
    exit_code
}

pub(crate) fn build_cli() -> ClapCommand {
    ClapCommand::new(COMMAND_NAME)
        .about(
            "List capture routes for inbox, area, project, notebook, and record notes",
        )
        .long_about(
            "List the routable Bob notes that capture pickers can offer.\n\n\
The command reads top-level area and non-terminal project notes, opt-in \
notebook notes, plus configured record folders. It always pins mac_inbox \
first as the default task-capture target. It is read-only.",
        )
        .after_help(
            "Examples:\n  bob capture-targets --format json\n  bob capture-targets\n  bob capture-targets -f json\n  bob capture-targets -b ~/bob",
        )
        .disable_help_flag(true)
        .arg(bob_dir_arg())
        .arg(format_arg())
        .arg(help_arg())
        .arg(verbose_arg())
}

fn bob_dir_arg() -> Arg {
    Arg::new("bob-dir")
        .long("bob-dir")
        .short('b')
        .value_name("DIR")
        .value_parser(OsStringValueParser::new())
        .help("Bob vault root; defaults to BOB_DIR or ~/bob")
}

fn format_arg() -> Arg {
    Arg::new("format")
        .long("format")
        .short('f')
        .value_name("FORMAT")
        .value_parser(["human", "json"])
        .default_value("human")
        .help("Output format: human or json")
}

fn help_arg() -> Arg {
    Arg::new("help")
        .long("help")
        .short('h')
        .action(ArgAction::Help)
        .help("Show help")
}

fn verbose_arg() -> Arg {
    Arg::new("verbose")
        .long("verbose")
        .short('v')
        .action(ArgAction::SetTrue)
        .help("Print skipped non-routable note warnings to stderr")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OutputFormat {
    Human,
    Json,
}

impl OutputFormat {
    fn from_matches(matches: &ArgMatches) -> Self {
        match matches
            .get_one::<String>("format")
            .map(String::as_str)
            .unwrap_or("human")
        {
            "json" => Self::Json,
            _ => Self::Human,
        }
    }
}

fn bob_dir_from_matches(matches: &ArgMatches) -> PathBuf {
    matches
        .get_one::<OsString>("bob-dir")
        .map(PathBuf::from)
        .map(|path| bob_env::expand_tilde(&path))
        .unwrap_or_else(bob_env::bob_dir)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CaptureTargetsReport {
    pub(crate) targets: Vec<CaptureTarget>,
    pub(crate) warnings: Vec<ScanNote>,
    pub(crate) issues: Vec<ScanNote>,
    /// Alias problems recorded structurally (also still in `warnings`);
    /// doctor reads this instead of matching on message text.
    pub(crate) alias_warnings: Vec<ScanNote>,
}

impl CaptureTargetsReport {
    fn result(&self, bob_dir: &Path) -> CaptureTargetsResult {
        CaptureTargetsResult {
            ok: true,
            bob_dir: bob_dir.display().to_string(),
            count: self.targets.len(),
            targets: self.targets.clone(),
        }
    }

    pub(crate) fn issue_summary(&self) -> String {
        self.issues
            .iter()
            .map(ScanNote::display)
            .collect::<Vec<_>>()
            .join("; ")
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ScanNote {
    relative_path: PathBuf,
    message: String,
}

impl ScanNote {
    fn path(
        relative_path: impl Into<PathBuf>,
        message: impl Into<String>,
    ) -> Self {
        Self {
            relative_path: relative_path.into(),
            message: message.into(),
        }
    }

    pub(crate) fn display(&self) -> String {
        format!("{}: {}", display_path(&self.relative_path), self.message)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct CaptureTargetsResult {
    ok: bool,
    bob_dir: String,
    count: usize,
    targets: Vec<CaptureTarget>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct CaptureTarget {
    pub(crate) route: String,
    pub(crate) name: String,
    pub(crate) label: String,
    pub(crate) kind: CaptureTargetKind,
    pub(crate) is_default: bool,
    pub(crate) status: Option<String>,
    pub(crate) relative_path: String,
    pub(crate) project_name_aliases: Vec<String>,
    /// Record type id, present only on configured record-folder rows.
    #[serde(rename = "type", skip_serializing_if = "Option::is_none")]
    pub(crate) record_type: Option<String>,
    /// Record type emoji, present only on configured record-folder rows.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) emoji: Option<String>,
    /// Indexed note count, present only on configured record-folder rows.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) count: Option<usize>,
    /// Notebook title, present only on notebook rows.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) title: Option<String>,
    /// Notebook summary, present only on notebook rows with a summary.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) summary: Option<String>,
    /// Notebook owner route, present only on notebook rows with an owner.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) owner: Option<String>,
    /// Notebook breadcrumb (owner through title), present only on rows.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) breadcrumb: Option<Vec<String>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum CaptureTargetKind {
    Inbox,
    Area,
    Project,
    Record,
    Notebook,
}

pub(crate) fn scan_capture_targets(bob_dir: &Path) -> CaptureTargetsReport {
    scan_capture_targets_with_config(bob_dir, &super::config::config_path())
}

/// Opt-in route scan that also offers notebooks as capture destinations.
/// The base [`scan_capture_targets`] stays task-only; only `bob
/// capture-targets` and route completion use this scanner. Notebook rows
/// sort after projects (before record folders) and carry `kind: notebook`
/// plus `title`, `summary`, `owner`, and `breadcrumb`.
pub(crate) fn scan_capture_targets_with_notebooks(
    bob_dir: &Path,
) -> CaptureTargetsReport {
    scan_capture_targets_with_notebooks_config(
        bob_dir,
        &super::config::config_path(),
    )
}

fn scan_capture_targets_with_notebooks_config(
    bob_dir: &Path,
    config_path: &Path,
) -> CaptureTargetsReport {
    let mut report = scan_capture_targets_with_config(bob_dir, config_path);
    if !report.issues.is_empty() {
        return report;
    }
    let notebooks = collect_notebook_targets(bob_dir);
    if notebooks.is_empty() {
        return report;
    }
    let insert_at = report
        .targets
        .iter()
        .position(|target| target.kind == CaptureTargetKind::Record)
        .unwrap_or(report.targets.len());
    report.targets.splice(insert_at..insert_at, notebooks);
    report
}

/// Root notebook notes as capture targets, sorted by route. Each row keeps
/// the vault-relative filename as `label` and reports the notebook catalog
/// title, summary, owner, and breadcrumb. Notes that fail to read or that
/// the notebook policy does not recognize stay absent.
fn collect_notebook_targets(bob_dir: &Path) -> Vec<CaptureTarget> {
    let entries = match read_sorted_root_directory(bob_dir) {
        Ok(entries) => entries,
        Err(_) => return Vec::new(),
    };
    let mut routes = Vec::new();
    for entry in entries {
        let path = entry.path();
        if !entry.file_type().is_ok_and(|kind| kind.is_file())
            || !is_markdown_file(&path)
        {
            continue;
        }
        let Some(route) = routable_route_for_root_file(&path) else {
            continue;
        };
        if route == capture::inbox_route() {
            continue;
        }
        let contents = match fs::read_to_string(&path) {
            Ok(contents) => contents,
            Err(_) => continue,
        };
        // Reuse the frontmatter gate the base scan uses: only notebook
        // notes become notebook rows.
        let Some(frontmatter) = parse_frontmatter(&contents) else {
            continue;
        };
        if !frontmatter_is_notebook(&frontmatter) {
            continue;
        }
        routes.push(route);
    }
    routes.sort();
    routes.dedup();
    let mut targets = Vec::new();
    for route in routes {
        let policy = super::capture_notebooks::notebook_policy_for_forced_route(
            bob_dir, &route,
        );
        let Some(info) = policy.get(&route) else {
            continue;
        };
        let label = capture::route_label(&route);
        targets.push(CaptureTarget {
            route: route.clone(),
            name: route.clone(),
            label: label.clone(),
            kind: CaptureTargetKind::Notebook,
            is_default: false,
            status: None,
            relative_path: label,
            project_name_aliases: Vec::new(),
            record_type: None,
            emoji: None,
            count: None,
            title: Some(info.title.clone()),
            summary: info.summary.clone(),
            owner: info.owner.clone(),
            breadcrumb: Some(info.breadcrumb.clone()),
        });
    }
    targets
}

fn scan_capture_targets_with_config(
    bob_dir: &Path,
    config_path: &Path,
) -> CaptureTargetsReport {
    let mut report = CaptureTargetsReport {
        targets: vec![default_inbox_target()],
        warnings: Vec::new(),
        issues: Vec::new(),
        alias_warnings: Vec::new(),
    };
    let mut areas = Vec::new();
    let mut projects = Vec::new();

    let entries = match read_sorted_root_directory(bob_dir) {
        Ok(entries) => entries,
        Err(error) => {
            report.issues.push(ScanNote::path(
                PathBuf::from("."),
                format!("failed to read directory: {error}"),
            ));
            return report;
        }
    };

    for entry in entries {
        scan_root_entry(bob_dir, entry, &mut report, &mut areas, &mut projects);
    }

    sort_targets(&mut areas);
    sort_targets(&mut projects);
    report.targets.extend(areas);
    report.targets.extend(projects);
    append_record_targets(bob_dir, config_path, &mut report);
    let conflicts = alias_conflict_warnings(&report.targets);
    report.warnings.extend(conflicts.iter().cloned());
    report.alias_warnings.extend(conflicts);
    report
}

/// Cross-note alias problems: two notes claiming one alias, and an alias
/// equal to another candidate's stem (the stem wins at resolve time).
fn alias_conflict_warnings(targets: &[CaptureTarget]) -> Vec<ScanNote> {
    use std::collections::BTreeMap;

    let mut warnings = Vec::new();
    let mut claimants: BTreeMap<String, Vec<&CaptureTarget>> = BTreeMap::new();
    for target in targets {
        for alias in &target.project_name_aliases {
            claimants
                .entry(alias.to_ascii_lowercase())
                .or_default()
                .push(target);
        }
    }
    for (alias, claimants) in &claimants {
        if claimants.len() > 1 {
            let notes = claimants
                .iter()
                .map(|target| target.label.clone())
                .collect::<Vec<_>>()
                .join(", ");
            warnings.push(ScanNote::path(
                PathBuf::from("."),
                format!(
                    "project_name_aliases '{alias}' is claimed by more than one note ({notes}); the alias is ambiguous"
                ),
            ));
        }
    }
    for target in targets {
        for alias in &target.project_name_aliases {
            let lowered = alias.to_ascii_lowercase();
            if let Some(owner) =
                targets.iter().find(|other| other.route == lowered)
                && owner.route != target.route
            {
                warnings.push(ScanNote::path(
                    PathBuf::from(&target.relative_path),
                    format!(
                        "project_name_aliases '{alias}' is shadowed by the {} stem; the stem wins",
                        owner.route
                    ),
                ));
            }
        }
    }
    warnings
}

fn read_sorted_root_directory(
    directory: &Path,
) -> io::Result<Vec<fs::DirEntry>> {
    let mut entries =
        fs::read_dir(directory)?.collect::<Result<Vec<_>, io::Error>>()?;
    entries.sort_by_key(|entry| entry.path());
    Ok(entries)
}

fn scan_root_entry(
    root: &Path,
    entry: fs::DirEntry,
    report: &mut CaptureTargetsReport,
    areas: &mut Vec<CaptureTarget>,
    projects: &mut Vec<CaptureTarget>,
) {
    let path = entry.path();
    let relative_path = relative_or_original(root, &path);
    let file_type = match entry.file_type() {
        Ok(file_type) => file_type,
        Err(error) => {
            report.issues.push(ScanNote::path(
                relative_path,
                format!("failed to inspect path: {error}"),
            ));
            return;
        }
    };

    if !file_type.is_file() || !is_markdown_file(&path) {
        return;
    }

    let Some(route) = routable_route_for_root_file(&path) else {
        report.warnings.push(ScanNote::path(
            relative_path,
            "skipping non-routable note; file stem must be valid UTF-8, lowercase, and contain only letters, digits, '_' or '-'",
        ));
        return;
    };

    if route == capture::inbox_route() {
        return;
    }

    let contents = match fs::read_to_string(&path) {
        Ok(contents) => contents,
        Err(error) => {
            report.issues.push(ScanNote::path(
                relative_path,
                format!("failed to read file: {error}"),
            ));
            return;
        }
    };

    let Some(frontmatter) = parse_frontmatter(&contents) else {
        return;
    };

    if frontmatter_is_area(&frontmatter) {
        let (aliases, alias_warnings) = project_name_aliases(&frontmatter);
        push_alias_warnings(report, &relative_path, alias_warnings);
        areas.push(target_from_route(
            route,
            CaptureTargetKind::Area,
            None,
            aliases,
        ));
        return;
    }

    if !frontmatter_is_project(&frontmatter) {
        return;
    }

    let status =
        ProjectStatus::parse(frontmatter_value(&frontmatter, "status"));
    if status.is_terminal() {
        return;
    }

    let (aliases, alias_warnings) = project_name_aliases(&frontmatter);
    push_alias_warnings(report, &relative_path, alias_warnings);
    projects.push(target_from_route(
        route,
        CaptureTargetKind::Project,
        Some(status.label().to_string()),
        aliases,
    ));
}

fn push_alias_warnings(
    report: &mut CaptureTargetsReport,
    relative_path: &Path,
    warnings: Vec<String>,
) {
    for warning in warnings {
        let note = ScanNote::path(relative_path.to_path_buf(), warning);
        report.warnings.push(note.clone());
        report.alias_warnings.push(note);
    }
}

fn default_inbox_target() -> CaptureTarget {
    let route = capture::inbox_route().to_string();
    CaptureTarget {
        route: route.clone(),
        name: route,
        label: capture::INBOX_FILE.to_string(),
        kind: CaptureTargetKind::Inbox,
        is_default: true,
        status: None,
        relative_path: capture::INBOX_FILE.to_string(),
        project_name_aliases: Vec::new(),
        record_type: None,
        emoji: None,
        count: None,
        title: None,
        summary: None,
        owner: None,
        breadcrumb: None,
    }
}

fn target_from_route(
    route: String,
    kind: CaptureTargetKind,
    status: Option<String>,
    project_name_aliases: Vec<String>,
) -> CaptureTarget {
    let label = capture::route_label(&route);
    CaptureTarget {
        route: route.clone(),
        name: route,
        label: label.clone(),
        kind,
        is_default: false,
        status,
        relative_path: label,
        project_name_aliases,
        record_type: None,
        emoji: None,
        count: None,
        title: None,
        summary: None,
        owner: None,
        breadcrumb: None,
    }
}

fn append_record_targets(
    bob_dir: &Path,
    config_path: &Path,
    report: &mut CaptureTargetsReport,
) {
    let context = RecordContext::discover(bob_dir, config_path);
    if let super::albums::registry::DiscoveryHealth::Partial(errors) =
        &context.registry().health
    {
        report.issues.extend(
            errors
                .iter()
                .map(|error| ScanNote::path(PathBuf::from("."), error.clone())),
        );
        return;
    }

    // A broken album still owns its noun and folder, so report its schema
    // findings while keeping it out of the actionable record routes. Other
    // valid albums and transitional config kinds remain available.
    for album in context.invalid_album_claims() {
        for finding in &album.findings {
            if finding.severity != super::albums::AlbumIssueSeverity::Error {
                continue;
            }
            let mut message = format!("{}: {}", finding.code, finding.message);
            if let Some(line) = finding.line_number {
                message.push_str(&format!(" (line {line})"));
            }
            report
                .warnings
                .push(ScanNote::path(PathBuf::from(&finding.path), message));
        }
    }

    let mut record_types = context.usable_kinds();
    record_types.sort_by(|left, right| {
        left.record_type()
            .dir
            .to_ascii_lowercase()
            .cmp(&right.record_type().dir.to_ascii_lowercase())
            .then_with(|| left.record_type().id.cmp(&right.record_type().id))
    });
    let mut catalog = RecordCatalog::new(bob_dir);
    for kind in record_types {
        let record_type = kind.record_type();
        let count = catalog.folder(&record_type.dir).notes().len();
        let route = format!("{}/", record_type.dir);
        let album = kind.album.as_ref();
        report.targets.push(CaptureTarget {
            route: route.clone(),
            name: route.clone(),
            label: record_type.label.clone(),
            kind: CaptureTargetKind::Record,
            is_default: false,
            status: None,
            relative_path: route,
            project_name_aliases: Vec::new(),
            record_type: Some(record_type.id.clone()),
            emoji: record_type.emoji.clone(),
            count: Some(count),
            title: album.map(|album| album.title.clone()),
            summary: context.album_summary_for(&kind).map(str::to_owned),
            owner: album.and_then(|album| album.owner.clone()),
            breadcrumb: album.map(|album| album.breadcrumb.clone()),
        });
    }
}

pub(crate) fn routable_route_for_root_file(path: &Path) -> Option<String> {
    let stem = path.file_stem()?.to_str()?;
    if stem == stem.to_ascii_lowercase() && capture::is_route_token(stem) {
        Some(stem.to_string())
    } else {
        None
    }
}

fn sort_targets(targets: &mut [CaptureTarget]) {
    targets.sort_by(|left, right| {
        left.name
            .cmp(&right.name)
            .then_with(|| left.relative_path.cmp(&right.relative_path))
    });
}

fn print_success(result: &CaptureTargetsResult, output_format: OutputFormat) {
    match output_format {
        OutputFormat::Human => print_human_success(result),
        OutputFormat::Json => println!("{}", success_json(result)),
    }
}

fn print_human_success(result: &CaptureTargetsResult) {
    let styler = Styler::detect();
    print_human_success_with_styler(result, &styler);
}

fn print_human_success_with_styler(
    result: &CaptureTargetsResult,
    styler: &Styler,
) {
    let widths = ColumnWidths::from_targets(&result.targets);
    let inbox = targets_of_kind(result, CaptureTargetKind::Inbox);
    let areas = targets_of_kind(result, CaptureTargetKind::Area);
    let projects = targets_of_kind(result, CaptureTargetKind::Project);
    let notebooks = targets_of_kind(result, CaptureTargetKind::Notebook);
    let records = targets_of_kind(result, CaptureTargetKind::Record);

    println!("Capture targets {} {}", styler.separator(), result.bob_dir);
    println!();
    print_group("Inbox", &inbox, &widths, styler);
    if !areas.is_empty() {
        println!();
        print_group("Areas", &areas, &widths, styler);
    }
    if !projects.is_empty() {
        println!();
        print_group("Active projects", &projects, &widths, styler);
    }
    if !notebooks.is_empty() {
        println!();
        print_group("Notebooks", &notebooks, &widths, styler);
    }
    if !records.is_empty() {
        println!();
        print_group("Record folders", &records, &widths, styler);
    }
    if areas.is_empty()
        && projects.is_empty()
        && notebooks.is_empty()
        && records.is_empty()
    {
        println!();
        println!("  No area or active project targets found.");
    }

    println!();
    println!(
        "{} {} {} {} {} {} {} {} {} {} {} {} {} {} {} {}",
        result.count,
        plural(result.count, "target", "targets"),
        styler.separator(),
        inbox.len(),
        plural(inbox.len(), "inbox", "inboxes"),
        styler.separator(),
        areas.len(),
        plural(areas.len(), "area", "areas"),
        styler.separator(),
        active_project_count_label(projects.len()),
        styler.separator(),
        notebooks.len(),
        plural(notebooks.len(), "notebook", "notebooks"),
        styler.separator(),
        records.len(),
        plural(records.len(), "record folder", "record folders")
    );
}

fn print_group(
    heading: &str,
    targets: &[&CaptureTarget],
    widths: &ColumnWidths,
    styler: &Styler,
) {
    println!("  {heading}");
    for target in targets {
        print_target_row(target, widths, styler);
    }
}

fn print_target_row(
    target: &CaptureTarget,
    widths: &ColumnWidths,
    styler: &Styler,
) {
    let marker = if target.is_default {
        if styler.is_color() {
            styler.yellow("\u{2605}")
        } else {
            "*".to_string()
        }
    } else {
        " ".to_string()
    };
    let name = styler.cyan(&pad_right(&target.name, widths.name));
    let label = styler.dim(&pad_right(&target.label, widths.label));
    let detail = target_detail(target, styler);

    if detail.is_empty() {
        println!("    {marker} {name}  {label}");
    } else {
        println!("    {marker} {name}  {label}   {detail}");
    }
}

fn target_detail(target: &CaptureTarget, styler: &Styler) -> String {
    let aka = if target.project_name_aliases.is_empty() {
        String::new()
    } else {
        format!("aka {}", target.project_name_aliases.join(", "))
    };

    if target.is_default {
        let default = styler.yellow("default");
        if aka.is_empty() {
            return default;
        }
        return format!("{default} · {aka}");
    }

    if target.kind == CaptureTargetKind::Notebook {
        let title = target.title.as_deref().unwrap_or(&target.route);
        let trail = target
            .breadcrumb
            .as_deref()
            .map(|crumbs| crumbs.join(" › "))
            .filter(|trail| !trail.is_empty());
        let mut detail = match trail {
            Some(trail) => format!("📓 {title} · {trail}"),
            None => format!("📓 {title}"),
        };
        if let Some(summary) = target.summary.as_deref()
            && !summary.is_empty()
        {
            detail.push_str(&format!(" · {summary}"));
        }
        return detail;
    }

    if target.kind != CaptureTargetKind::Project {
        if target.kind != CaptureTargetKind::Record {
            return aka;
        }
        let emoji = target.emoji.as_deref().unwrap_or("");
        let count = target.count.unwrap_or(0);
        return format!("{emoji} {count} {}", plural(count, "note", "notes"));
    }

    let status = target.status.as_deref().unwrap_or("wip");
    let status = if status == "waiting" {
        styler.blue(status)
    } else {
        styler.yellow(status)
    };
    if aka.is_empty() {
        status
    } else {
        format!("{status} · {aka}")
    }
}

fn targets_of_kind(
    result: &CaptureTargetsResult,
    kind: CaptureTargetKind,
) -> Vec<&CaptureTarget> {
    result
        .targets
        .iter()
        .filter(|target| target.kind == kind)
        .collect()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ColumnWidths {
    name: usize,
    label: usize,
}

impl ColumnWidths {
    fn from_targets(targets: &[CaptureTarget]) -> Self {
        Self {
            name: targets
                .iter()
                .map(|target| display_width(&target.name))
                .max()
                .unwrap_or(0),
            label: targets
                .iter()
                .map(|target| display_width(&target.label))
                .max()
                .unwrap_or(0),
        }
    }
}

fn active_project_count_label(count: usize) -> String {
    format!(
        "{} {}",
        count,
        plural(count, "active project", "active projects")
    )
}

fn plural<'a>(count: usize, singular: &'a str, plural: &'a str) -> &'a str {
    if count == 1 {
        singular
    } else {
        plural
    }
}

fn success_json(result: &CaptureTargetsResult) -> String {
    serde_json::to_string(result).expect("serialize capture targets result")
}

fn print_scan_error(
    report: &CaptureTargetsReport,
    bob_dir: &Path,
    output_format: OutputFormat,
    verbose: bool,
) {
    match output_format {
        OutputFormat::Human => {
            print_human_success(&report.result(bob_dir));
            if verbose {
                for warning in &report.warnings {
                    eprintln!("{COMMAND_NAME}: {}", warning.display());
                }
            }
            for issue in &report.issues {
                eprintln!("{COMMAND_NAME}: {}", issue.display());
            }
        }
        OutputFormat::Json => {
            println!(
                "{}",
                json!({ "ok": false, "error": report.issue_summary() })
            );
        }
    }
}

fn relative_or_original(root: &Path, path: &Path) -> PathBuf {
    path.strip_prefix(root).unwrap_or(path).to_path_buf()
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
    use std::{
        fs,
        sync::atomic::{AtomicUsize, Ordering},
        time::{SystemTime, UNIX_EPOCH},
    };

    static TEMP_COUNTER: AtomicUsize = AtomicUsize::new(0);

    #[test]
    fn routable_route_requires_lowercase_valid_token() {
        let base = Path::new("/tmp");
        assert_eq!(
            routable_route_for_root_file(&base.join("cash.md")).as_deref(),
            Some("cash")
        );
        assert_eq!(routable_route_for_root_file(&base.join("Foo.md")), None);
        assert_eq!(
            routable_route_for_root_file(&base.join("bad route.md")),
            None
        );
    }

    #[test]
    fn area_and_project_frontmatter_are_classified() {
        let area = parse_frontmatter("---\ntype: \"[[area]]\"\n---\n")
            .expect("area frontmatter");
        assert!(frontmatter_is_area(&area));
        assert!(!frontmatter_is_project(&area));

        let project =
            parse_frontmatter("---\ntype: [[project]]\nstatus: waiting\n---\n")
                .expect("project frontmatter");
        assert!(frontmatter_is_project(&project));
        let status =
            ProjectStatus::parse(frontmatter_value(&project, "status"));
        assert_eq!(status.label(), "waiting");
        assert!(!status.is_terminal());
    }

    #[test]
    fn scan_orders_inbox_areas_then_active_projects() {
        let temp = TempDir::new("bob-cli-capture-targets-unit");
        write_file(
            &temp.path().join("mac_inbox.md"),
            "---\ntype: [[area]]\n---\n",
        );
        write_file(
            &temp.path().join("z_area.md"),
            "---\ntype: [[area]]\n---\n",
        );
        write_file(
            &temp.path().join("a_area.md"),
            "---\ntype: [[area]]\n---\n",
        );
        write_file(
            &temp.path().join("waiting.md"),
            "---\ntype: [[project]]\nstatus: waiting\n---\n",
        );
        write_file(
            &temp.path().join("done.md"),
            "---\ntype: [[project]]\nstatus: done\n---\n",
        );
        write_file(
            &temp.path().join("nested/child.md"),
            "---\ntype: [[area]]\n---\n",
        );
        write_file(&temp.path().join("Bad.md"), "---\ntype: [[area]]\n---\n");

        let report = scan_capture_targets_with_config(
            temp.path(),
            &temp.path().join("records-config.yml"),
        );
        let routes = report
            .targets
            .iter()
            .map(|target| target.route.as_str())
            .collect::<Vec<_>>();

        assert_eq!(routes, ["mac_inbox", "a_area", "z_area", "waiting"]);
        assert_eq!(report.warnings.len(), 1);
        assert!(report.issues.is_empty());
        assert_eq!(
            report
                .targets
                .iter()
                .filter(|target| target.is_default)
                .count(),
            1
        );
    }

    #[test]
    fn scan_appends_indexed_record_folders_after_note_targets() {
        let temp = TempDir::new("bob-cli-capture-targets-records");
        let config = temp.path().join("config.yml");
        write_file(
            &config,
            "records:\n  restaurant:\n    dir: eat\n    label: Restaurant\n    emoji: \"🍽️\"\n    fields:\n      status:\n        required: true\n        values: {liked: \"💚 Liked\"}\n  podcast:\n    dir: podcasts\n    label: Podcast\n    fields:\n      status:\n        required: true\n        values: {subscribed: \"Subscribed\"}\n",
        );
        write_file(&temp.path().join("cash.md"), "---\ntype: [[area]]\n---\n");
        write_file(
            &temp.path().join("eat/ninja_sushi.md"),
            "---\ntype: [[restaurant]]\ntitle: Ninja Sushi\nstatus: liked\n---\n",
        );

        let report = scan_capture_targets_with_config(temp.path(), &config);
        assert!(report.issues.is_empty());
        let routes = report
            .targets
            .iter()
            .map(|target| target.route.as_str())
            .collect::<Vec<_>>();
        assert_eq!(routes, ["mac_inbox", "cash", "eat/", "podcasts/"]);
        let restaurant = report
            .targets
            .iter()
            .find(|target| target.kind == CaptureTargetKind::Record)
            .expect("restaurant folder target");
        assert_eq!(restaurant.route, "eat/");
        assert_eq!(restaurant.record_type.as_deref(), Some("restaurant"));
        assert_eq!(restaurant.label, "Restaurant");
        assert_eq!(restaurant.emoji.as_deref(), Some("🍽️"));
        assert_eq!(restaurant.count, Some(1));
        assert_eq!(restaurant.relative_path, "eat/");
        let podcast = report
            .targets
            .iter()
            .find(|target| target.route == "podcasts/")
            .unwrap();
        assert_eq!(podcast.count, Some(0));

        let value: serde_json::Value =
            serde_json::from_str(&success_json(&report.result(temp.path())))
                .unwrap();
        let record_row = value["targets"]
            .as_array()
            .unwrap()
            .iter()
            .find(|target| target["route"] == "eat/")
            .unwrap();
        assert_eq!(record_row["kind"], "record");
        assert_eq!(record_row["type"], "restaurant");
        assert_eq!(record_row["label"], "Restaurant");
        assert_eq!(record_row["emoji"], "🍽️");
        assert_eq!(record_row["count"], 1);
    }

    #[test]
    fn scan_uses_album_registry_and_enriches_record_target_metadata() {
        let temp = TempDir::new("bob-cli-capture-targets-album");
        let config = temp.path().join("config.yml");
        write_file(
            &temp.path().join("body.md"),
            "---\ntype: [[area]]\n---\n# Body\n",
        );
        write_file(
            &temp.path().join("eat.md"),
            concat!(
                "---\n",
                "parent: \"[[body]]\"\n",
                "type: [[album]]\n",
                "summary: Restaurants we have tried.\n",
                "emoji: 🍽️\n",
                "record:\n",
                "  noun: restaurant\n",
                "  label: Restaurant\n",
                "---\n",
                "# Restaurants\n",
            ),
        );
        write_file(
            &temp.path().join("eat/nagi.md"),
            "---\nparent: \"[[eat]]\"\ntype: \"[[record]]\"\n---\n# Nagi\n",
        );

        let report = scan_capture_targets_with_config(temp.path(), &config);
        assert!(report.issues.is_empty(), "{:?}", report.issues);
        let target = report
            .targets
            .iter()
            .find(|target| target.route == "eat/")
            .expect("album target");
        assert_eq!(target.kind, CaptureTargetKind::Record);
        assert_eq!(target.record_type.as_deref(), Some("restaurant"));
        assert_eq!(target.label, "Restaurant");
        assert_eq!(target.emoji.as_deref(), Some("🍽️"));
        assert_eq!(target.count, Some(1));
        assert_eq!(target.title.as_deref(), Some("Restaurants"));
        assert_eq!(
            target.summary.as_deref(),
            Some("Restaurants we have tried.")
        );
        assert_eq!(target.owner.as_deref(), Some("body"));
        assert_eq!(
            target.breadcrumb.as_deref(),
            Some(&["body".to_string(), "Restaurants".to_string()][..])
        );

        let value: serde_json::Value =
            serde_json::from_str(&success_json(&report.result(temp.path())))
                .expect("json");
        let row = value["targets"]
            .as_array()
            .unwrap()
            .iter()
            .find(|row| row["route"] == "eat/")
            .expect("album row json");
        assert_eq!(row["title"], "Restaurants");
        assert_eq!(row["summary"], "Restaurants we have tried.");
        assert_eq!(row["owner"], "body");
        assert_eq!(
            row["breadcrumb"],
            serde_json::json!(["body", "Restaurants"])
        );
    }

    #[test]
    fn broken_album_is_reported_without_hiding_healthy_or_config_routes() {
        let temp = TempDir::new("bob-cli-capture-targets-broken-album");
        let config = temp.path().join("config.yml");
        write_file(
            &config,
            "records:\n  restaurant:\n    dir: eat\n    label: Legacy restaurant\n    fields: {}\n  book:\n    dir: books\n    label: Book\n    fields: {}\n",
        );
        write_file(
            &temp.path().join("body.md"),
            "---\ntype: [[area]]\n---\n# Body\n",
        );
        write_file(
            &temp.path().join("eat.md"),
            "---\nparent: \"[[body]]\"\ntype: [[album]]\nrecord:\n  noun: restaurant\n  unknown: value\n---\n# Broken restaurants\n",
        );
        write_file(
            &temp.path().join("podcasts.md"),
            "---\nparent: \"[[body]]\"\ntype: [[album]]\nrecord:\n  noun: podcast\n---\n# Podcasts\n",
        );
        write_file(
            &temp.path().join("podcasts/show.md"),
            "---\nparent: \"[[podcasts]]\"\ntype: \"[[record]]\"\n---\n# Show\n",
        );

        let report = scan_capture_targets_with_config(temp.path(), &config);
        assert!(report.issues.is_empty(), "{:?}", report.issues);
        let record_routes = report
            .targets
            .iter()
            .filter(|target| target.kind == CaptureTargetKind::Record)
            .map(|target| target.route.as_str())
            .collect::<Vec<_>>();
        assert_eq!(record_routes, ["books/", "podcasts/"]);
        let podcast = report
            .targets
            .iter()
            .find(|target| target.route == "podcasts/")
            .expect("healthy album remains available");
        assert_eq!(podcast.record_type.as_deref(), Some("podcast"));
        assert_eq!(podcast.count, Some(1));
        assert!(report.warnings.iter().any(|warning| {
            warning.display().contains("album_schema_invalid")
                && warning.display().contains("eat.md")
        }));
        assert!(
            report.targets.iter().all(|target| target.route != "eat/"),
            "broken album is not actionable"
        );
    }

    #[test]
    fn absent_or_invalid_record_config_keeps_task_targets() {
        let temp = TempDir::new("bob-cli-capture-targets-records-isolation");
        write_file(&temp.path().join("cash.md"), "---\ntype: [[area]]\n---\n");
        let missing = temp.path().join("missing-config.yml");
        let report = scan_capture_targets_with_config(temp.path(), &missing);
        assert!(report.issues.is_empty());
        assert_eq!(
            report
                .targets
                .iter()
                .map(|target| target.route.as_str())
                .collect::<Vec<_>>(),
            ["mac_inbox", "cash"]
        );

        let malformed = temp.path().join("malformed-config.yml");
        write_file(&malformed, "records: [not, a, mapping]\n");
        let report = scan_capture_targets_with_config(temp.path(), &malformed);
        assert!(report.issues.is_empty());
        assert_eq!(report.targets.len(), 2);
        assert!(report
            .targets
            .iter()
            .all(|target| target.kind != CaptureTargetKind::Record));
    }

    #[test]
    fn json_shape_is_stable() {
        let result = CaptureTargetsReport {
            targets: vec![
                default_inbox_target(),
                target_from_route(
                    "cash".to_string(),
                    CaptureTargetKind::Area,
                    None,
                    Vec::new(),
                ),
                target_from_route(
                    "bob".to_string(),
                    CaptureTargetKind::Project,
                    Some("wip".to_string()),
                    vec!["bob-cli".to_string()],
                ),
            ],
            warnings: Vec::new(),
            issues: Vec::new(),
            alias_warnings: Vec::new(),
        }
        .result(Path::new("/tmp/bob"));

        let value: serde_json::Value =
            serde_json::from_str(&success_json(&result)).expect("json");
        assert_eq!(value["ok"], true);
        assert_eq!(value["bob_dir"], "/tmp/bob");
        assert_eq!(value["count"], 3);
        assert_eq!(value["targets"][0]["route"], "mac_inbox");
        assert_eq!(value["targets"][0]["kind"], "inbox");
        assert_eq!(value["targets"][0]["is_default"], true);
        assert!(value["targets"][0]["status"].is_null());
        assert_eq!(
            value["targets"][0]["project_name_aliases"],
            serde_json::json!([])
        );
        assert_eq!(value["targets"][1]["kind"], "area");
        assert_eq!(
            value["targets"][1]["project_name_aliases"],
            serde_json::json!([])
        );
        assert_eq!(value["targets"][2]["kind"], "project");
        assert_eq!(value["targets"][2]["status"], "wip");
        assert_eq!(
            value["targets"][2]["project_name_aliases"],
            serde_json::json!(["bob-cli"])
        );
    }

    #[test]
    fn scan_reads_aliases_and_warns_on_conflicts() {
        let temp = TempDir::new("bob-cli-capture-targets-aliases");
        write_file(
            &temp.path().join("bob.md"),
            "---\ntype: [[project]]\nstatus: wip\nproject_name_aliases: [\"bob-cli\"]\n---\n",
        );
        write_file(
            &temp.path().join("sase.md"),
            "---\ntype: [[area]]\nproject_name_aliases:\n  - sase-alias\n  - \"\"\n---\n",
        );
        write_file(
            &temp.path().join("other.md"),
            "---\ntype: [[area]]\nproject_name_aliases: [\"bob-cli\"]\n---\n",
        );

        let report = scan_capture_targets_with_config(
            temp.path(),
            &temp.path().join("records-config.yml"),
        );
        let bob = report
            .targets
            .iter()
            .find(|target| target.route == "bob")
            .expect("bob target");
        assert_eq!(bob.project_name_aliases, vec!["bob-cli".to_string()]);
        let sase = report
            .targets
            .iter()
            .find(|target| target.route == "sase")
            .expect("sase target");
        assert_eq!(sase.project_name_aliases, vec!["sase-alias".to_string()]);
        let warnings = report
            .warnings
            .iter()
            .map(ScanNote::display)
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            warnings.contains("empty; ignoring"),
            "empty entries warn:\n{warnings}"
        );
        assert!(
            warnings.contains("claimed by more than one note"),
            "duplicate claims warn:\n{warnings}"
        );
    }

    #[test]
    fn scan_warns_on_shadowed_alias() {
        let temp = TempDir::new("bob-cli-capture-targets-shadow");
        write_file(
            &temp.path().join("bob.md"),
            "---\ntype: [[project]]\nstatus: wip\n---\n",
        );
        write_file(
            &temp.path().join("sase.md"),
            "---\ntype: [[area]]\nproject_name_aliases: [\"bob\"]\n---\n",
        );

        let report = scan_capture_targets_with_config(
            temp.path(),
            &temp.path().join("records-config.yml"),
        );
        let warnings = report
            .warnings
            .iter()
            .map(ScanNote::display)
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            warnings.contains("shadowed by the bob stem"),
            "shadowed aliases warn:\n{warnings}"
        );
    }

    #[test]
    fn base_scan_hides_notebooks_while_opt_in_lists_them_after_projects() {
        let temp = TempDir::new("bob-cli-capture-targets-notebooks");
        write_file(
            &temp.path().join("dev.md"),
            "---\ntype: \"[[area]]\"\n---\n",
        );
        write_file(
            &temp.path().join("bob.md"),
            "---\ntype: [[project]]\nstatus: wip\n---\n",
        );
        write_file(
            &temp.path().join("dev_tools.md"),
            "---\nparent: \"[[dev]]\"\ntype: \"[[notebook]]\"\nsummary: Tools.\n---\n\n# **Developer** [[tools]]\n",
        );
        write_file(
            &temp.path().join("notes.md"),
            "---\nparent: \"[[dev]]\"\ntype: \"[[notebook]]\"\n---\n\n# Notes\n",
        );

        let base = scan_capture_targets_with_config(
            temp.path(),
            &temp.path().join("records-config.yml"),
        );
        assert!(
            base.targets
                .iter()
                .all(|target| target.kind != CaptureTargetKind::Notebook),
            "base scan stays task-only: {:?}",
            base.targets
        );

        let report = scan_capture_targets_with_notebooks_config(
            temp.path(),
            &temp.path().join("records-config.yml"),
        );
        let routes = report
            .targets
            .iter()
            .map(|target| (target.route.as_str(), target.kind))
            .collect::<Vec<_>>();
        assert_eq!(
            routes.iter().map(|(route, _)| *route).collect::<Vec<_>>(),
            ["mac_inbox", "dev", "bob", "dev_tools", "notes"]
        );
        let notebook = report
            .targets
            .iter()
            .find(|target| target.route == "dev_tools")
            .expect("notebook row");
        assert_eq!(notebook.kind, CaptureTargetKind::Notebook);
        assert_eq!(notebook.title.as_deref(), Some("Developer tools"));
        assert_eq!(notebook.summary.as_deref(), Some("Tools."));
        assert_eq!(notebook.owner.as_deref(), Some("dev"));
        assert_eq!(
            notebook.breadcrumb.as_deref(),
            Some(&["dev".to_string(), "Developer tools".to_string()][..])
        );
        assert_eq!(notebook.relative_path, "dev_tools.md");
        assert_eq!(notebook.label, "dev_tools.md");

        let value: serde_json::Value =
            serde_json::from_str(&success_json(&report.result(temp.path())))
                .expect("json");
        let row = value["targets"]
            .as_array()
            .unwrap()
            .iter()
            .find(|row| row["route"] == "dev_tools")
            .expect("notebook json row");
        assert_eq!(row["kind"], "notebook");
        assert_eq!(row["title"], "Developer tools");
        assert_eq!(row["summary"], "Tools.");
        assert_eq!(row["owner"], "dev");
        assert_eq!(
            row["breadcrumb"],
            serde_json::json!(["dev", "Developer tools"])
        );
    }

    fn write_file(path: &Path, contents: &str) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap_or_else(|error| {
                panic!("create parent {}: {error}", parent.display())
            });
        }
        fs::write(path, contents).unwrap_or_else(|error| {
            panic!("write {}: {error}", path.display())
        });
    }

    struct TempDir {
        path: PathBuf,
    }

    impl TempDir {
        fn new(prefix: &str) -> Self {
            let mut path = std::env::temp_dir();
            path.push(format!(
                "{}-{}-{}-{}",
                prefix,
                std::process::id(),
                current_time_nanos(),
                TEMP_COUNTER.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir_all(&path).unwrap_or_else(|error| {
                panic!("create temp dir {}: {error}", path.display())
            });
            Self { path }
        }

        fn path(&self) -> &Path {
            &self.path
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            if let Err(error) = fs::remove_dir_all(&self.path) {
                eprintln!("failed to remove {}: {error}", self.path.display());
            }
        }
    }

    fn current_time_nanos() -> u128 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time before epoch")
            .as_nanos()
    }
}
