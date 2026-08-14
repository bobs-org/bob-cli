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
        frontmatter_is_area, frontmatter_is_project, frontmatter_value,
        is_markdown_file, parse_frontmatter, ProjectStatus,
    },
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
    let report = scan_capture_targets(&bob_dir);

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
        eprintln!(
            "{COMMAND_NAME}: failed to print command-line error: {print_error}"
        );
    }
    exit_code
}

fn build_cli() -> ClapCommand {
    ClapCommand::new(COMMAND_NAME)
        .about("List capture routes for inbox, area, and active project notes")
        .long_about(
            "List the routable Bob notes that the task-capture picker can \
offer.\n\n\
The command reads only top-level markdown files in the vault, includes area \
notes and non-terminal project notes, and always pins mac_inbox first as the \
default. It is read-only.",
        )
        .after_help(
            "Examples:\n  bob capture-targets\n  bob capture-targets -f json\n  bob capture-targets -b ~/bob",
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
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum CaptureTargetKind {
    Inbox,
    Area,
    Project,
}

pub(crate) fn scan_capture_targets(bob_dir: &Path) -> CaptureTargetsReport {
    let mut report = CaptureTargetsReport {
        targets: vec![default_inbox_target()],
        warnings: Vec::new(),
        issues: Vec::new(),
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
    report
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
        areas.push(target_from_route(route, CaptureTargetKind::Area, None));
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

    projects.push(target_from_route(
        route,
        CaptureTargetKind::Project,
        Some(status.label().to_string()),
    ));
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
    }
}

fn target_from_route(
    route: String,
    kind: CaptureTargetKind,
    status: Option<String>,
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
    }
}

fn routable_route_for_root_file(path: &Path) -> Option<String> {
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
    if areas.is_empty() && projects.is_empty() {
        println!();
        println!("  No area or active project targets found.");
    }

    println!();
    println!(
        "{} {} {} {} {} {} {} {} {} {}",
        result.count,
        plural(result.count, "target", "targets"),
        styler.separator(),
        inbox.len(),
        plural(inbox.len(), "inbox", "inboxes"),
        styler.separator(),
        areas.len(),
        plural(areas.len(), "area", "areas"),
        styler.separator(),
        active_project_count_label(projects.len())
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
    if target.is_default {
        return styler.yellow("default");
    }

    if target.kind != CaptureTargetKind::Project {
        return String::new();
    }

    let status = target.status.as_deref().unwrap_or("wip");
    if status == "waiting" {
        styler.blue(status)
    } else {
        styler.yellow(status)
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

        let report = scan_capture_targets(temp.path());
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
    fn json_shape_is_stable() {
        let result = CaptureTargetsReport {
            targets: vec![
                default_inbox_target(),
                target_from_route(
                    "cash".to_string(),
                    CaptureTargetKind::Area,
                    None,
                ),
                target_from_route(
                    "bob".to_string(),
                    CaptureTargetKind::Project,
                    Some("wip".to_string()),
                ),
            ],
            warnings: Vec::new(),
            issues: Vec::new(),
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
        assert_eq!(value["targets"][1]["kind"], "area");
        assert_eq!(value["targets"][2]["kind"], "project");
        assert_eq!(value["targets"][2]["status"], "wip");
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
