use std::{
    collections::HashSet,
    ffi::OsString,
    fs, io, iter,
    path::{Path, PathBuf},
    process::Command,
};

use clap::{
    builder::OsStringValueParser, Arg, ArgAction, ArgMatches,
    Command as ClapCommand,
};
use serde::{Deserialize, Serialize};
use serde_json::json;

use super::{
    env as bob_env,
    style::{display_width, pad_right, Styler},
};

const COMMAND_NAME: &str = "bob plugins";
const REPO_PLUGINS_SUBDIR: &str = "plugins";
const VAULT_PLUGINS_SUBDIR: &str = ".obsidian/plugins";
const COMMUNITY_PLUGINS_FILE: &str = ".obsidian/community-plugins.json";
/// Files the repo owns and `bob plugins sync` deploys; never `data.json`.
const MANAGED_FILES: &[&str] = &["manifest.json", "main.js", "styles.css"];
/// Width target for the human table when `$COLUMNS` is unavailable.
const DEFAULT_TERM_WIDTH: usize = 100;

pub(crate) fn run(args: Vec<OsString>) -> i32 {
    let mut command = build_cli();
    let matches = match command.try_get_matches_from_mut(
        iter::once(OsString::from(COMMAND_NAME)).chain(args),
    ) {
        Ok(matches) => matches,
        Err(error) => return print_clap_error(error),
    };

    match matches.subcommand() {
        Some(("list", sub_matches)) => run_list(sub_matches),
        Some(("sync", sub_matches)) => run_sync(sub_matches),
        Some((name, _)) => {
            eprintln!("{COMMAND_NAME}: unknown subcommand: {name}");
            2
        }
        // No subcommand defaults to `list`; top-level matches carry the same
        // options so `bob plugins -f json` works without typing `list`.
        None => run_list(&matches),
    }
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
        .about("Manage Bob Obsidian plugins from the bob-plugins repo")
        .long_about(
            "Manage Bryan's custom Bob Obsidian plugins from the \
bob-plugins repo.\n\n\
The list subcommand is read-only: it reads each plugin manifest from the repo, \
byte-compares the managed files against the vault copy to report sync state, \
and reads community-plugins.json to report whether the vault has the plugin \
enabled. Running `bob plugins` with no subcommand runs list.\n\n\
The sync subcommand deploys the repo into the vault: it copies the managed \
files (manifest.json, main.js, and styles.css) from the repo into the vault \
plugin folder, never touching data.json or other runtime files. It refuses to \
overwrite a vault file that has uncommitted changes in the vault Git repo \
unless --force is given.",
        )
        .after_help(
            "Examples:\n  bob plugins\n  bob plugins list\n  bob plugins list -f json\n  bob plugins sync --dry-run\n  bob plugins sync -p bob-project-tasks",
        )
        .arg(bob_dir_arg())
        .arg(format_arg())
        .arg(repo_arg())
        .subcommand(list_command())
        .subcommand(sync_command())
}

fn list_command() -> ClapCommand {
    ClapCommand::new("list")
        .about("List Bob plugins with repo version and vault sync state")
        .after_help(
            "Examples:\n  bob plugins list\n  bob plugins list -f json\n  bob plugins list -b ~/bob -r ~/projects/github/bbugyi200/bob-plugins",
        )
        .arg(bob_dir_arg())
        .arg(format_arg())
        .arg(repo_arg())
}

fn sync_command() -> ClapCommand {
    ClapCommand::new("sync")
        .about("Deploy repo plugin files into the vault")
        .long_about(
            "Deploy Bob plugins from the bob-plugins repo into the vault.\n\n\
For each plugin, the managed files (manifest.json, main.js, and styles.css \
when present) are copied from <repo>/plugins/<id>/ into \
<bob-dir>/.obsidian/plugins/<id>/. Runtime files such as data.json are never \
touched. A vault file that has uncommitted changes in the vault Git repo is \
left alone with a warning unless --force is given, so local edits are never \
clobbered silently. Files that already match the repo are reported as \
unchanged.",
        )
        .after_help(
            "Examples:\n  bob plugins sync --dry-run\n  bob plugins sync -p bob-project-tasks\n  bob plugins sync -F -b ~/bob -r ~/projects/github/bbugyi200/bob-plugins",
        )
        .arg(bob_dir_arg())
        .arg(dry_run_arg())
        .arg(force_arg())
        .arg(plugin_arg())
        .arg(repo_arg())
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
        .value_parser(["table", "json"])
        .default_value("table")
        .help("Output format: table or json")
}

fn repo_arg() -> Arg {
    Arg::new("repo")
        .long("repo")
        .short('r')
        .value_name("DIR")
        .value_parser(OsStringValueParser::new())
        .help(
            "Plugins repo root; defaults to BOB_PLUGINS_DIR or \
~/projects/github/bbugyi200/bob-plugins",
        )
}

fn dry_run_arg() -> Arg {
    Arg::new("dry-run")
        .long("dry-run")
        .short('d')
        .action(ArgAction::SetTrue)
        .help("Preview the copies without writing any files")
}

fn force_arg() -> Arg {
    Arg::new("force")
        .long("force")
        .short('F')
        .action(ArgAction::SetTrue)
        .help("Overwrite vault files with uncommitted Git changes")
}

fn plugin_arg() -> Arg {
    Arg::new("plugin")
        .long("plugin")
        .short('p')
        .value_name("ID")
        .help("Sync only this plugin id; defaults to every plugin")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OutputFormat {
    Table,
    Json,
}

impl OutputFormat {
    fn from_matches(matches: &ArgMatches) -> Self {
        match matches
            .get_one::<String>("format")
            .map(String::as_str)
            .unwrap_or("table")
        {
            "json" => Self::Json,
            _ => Self::Table,
        }
    }
}

fn repo_from_matches(matches: &ArgMatches) -> PathBuf {
    matches
        .get_one::<OsString>("repo")
        .map(PathBuf::from)
        .map(|path| bob_env::expand_tilde(&path))
        .unwrap_or_else(bob_env::plugins_dir)
}

fn bob_dir_from_matches(matches: &ArgMatches) -> PathBuf {
    matches
        .get_one::<OsString>("bob-dir")
        .map(PathBuf::from)
        .map(|path| bob_env::expand_tilde(&path))
        .unwrap_or_else(bob_env::bob_dir)
}

fn run_list(matches: &ArgMatches) -> i32 {
    let repo = repo_from_matches(matches);
    let bob_dir = bob_dir_from_matches(matches);
    let report = scan_plugins(&repo, &bob_dir);

    match OutputFormat::from_matches(matches) {
        OutputFormat::Table => {
            let styler = Styler::detect();
            print_plugins_table(&report, &styler);
            for issue in &report.issues {
                eprintln!("{COMMAND_NAME}: {issue}");
            }
        }
        OutputFormat::Json => {
            if report.issues.is_empty() {
                println!("{}", success_json(&report.result()));
            } else {
                println!(
                    "{}",
                    json!({ "ok": false, "error": report.issue_summary() })
                );
            }
        }
    }

    // Drift and not-installed are reportable states, not errors; only a real
    // failure such as an unreadable repo sets a non-zero exit.
    if report.issues.is_empty() {
        0
    } else {
        1
    }
}

fn run_sync(matches: &ArgMatches) -> i32 {
    let options = SyncOptions {
        repo: repo_from_matches(matches),
        bob_dir: bob_dir_from_matches(matches),
        only: matches.get_one::<String>("plugin").cloned(),
        dry_run: matches.get_flag("dry-run"),
        force: matches.get_flag("force"),
    };

    let report = sync_plugins(&options);
    let styler = Styler::detect();
    print_sync_report(&report, options.dry_run, &styler);
    for issue in &report.issues {
        eprintln!("{COMMAND_NAME}: {issue}");
    }

    // A refused dirty file is a deliberate warning, not a failure; only a real
    // error such as an unreadable repo or a failed copy sets a non-zero exit.
    if report.issues.is_empty() {
        0
    } else {
        1
    }
}

/// Resolved inputs for a single `bob plugins sync` invocation.
struct SyncOptions {
    repo: PathBuf,
    bob_dir: PathBuf,
    only: Option<String>,
    dry_run: bool,
    force: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SyncReport {
    repo: PathBuf,
    bob_dir: PathBuf,
    plugins: Vec<PluginSync>,
    issues: Vec<String>,
}

impl SyncReport {
    fn files(&self) -> impl Iterator<Item = &FileSync> {
        self.plugins.iter().flat_map(|plugin| plugin.files.iter())
    }

    fn copied(&self) -> usize {
        self.files().filter(|file| file.action.is_copy()).count()
    }

    fn skipped(&self) -> usize {
        self.files()
            .filter(|file| file.action == FileAction::SkippedDirty)
            .count()
    }

    fn unchanged(&self) -> usize {
        self.files()
            .filter(|file| file.action == FileAction::Unchanged)
            .count()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PluginSync {
    id: String,
    files: Vec<FileSync>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct FileSync {
    name: String,
    action: FileAction,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FileAction {
    /// The vault had no copy of the file; it was created.
    Created,
    /// The vault copy differed and was clean in Git; it was overwritten.
    Updated,
    /// The vault copy was dirty in Git and was overwritten because of --force.
    Forced,
    /// The vault copy already matched the repo byte-for-byte.
    Unchanged,
    /// The vault copy was dirty in Git and was left alone without --force.
    SkippedDirty,
    /// Reading or writing the file failed; the cause is recorded as an issue.
    Failed,
}

impl FileAction {
    fn is_copy(self) -> bool {
        matches!(self, Self::Created | Self::Updated | Self::Forced)
    }

    fn is_warning(self) -> bool {
        matches!(self, Self::SkippedDirty | Self::Failed)
    }
}

fn sync_plugins(options: &SyncOptions) -> SyncReport {
    let mut report = SyncReport {
        repo: options.repo.clone(),
        bob_dir: options.bob_dir.clone(),
        plugins: Vec::new(),
        issues: Vec::new(),
    };

    let plugins_root = options.repo.join(REPO_PLUGINS_SUBDIR);
    let entries = match read_sorted_directory(&plugins_root) {
        Ok(entries) => entries,
        Err(error) => {
            report.issues.push(format!(
                "failed to read plugins directory {}: {error}",
                plugins_root.display()
            ));
            return report;
        }
    };

    let mut matched = false;
    for entry in entries {
        let path = entry.path();
        let is_dir = entry
            .file_type()
            .map(|file_type| file_type.is_dir())
            .unwrap_or(false);
        if !is_dir {
            continue;
        }
        let Some(folder) = path.file_name().and_then(|name| name.to_str())
        else {
            continue;
        };

        let manifest = match read_manifest(&path) {
            Ok(manifest) => manifest,
            Err(error) => {
                report.issues.push(format!("{folder}: {error}"));
                continue;
            }
        };

        let id = if manifest.id.is_empty() {
            folder.to_string()
        } else {
            manifest.id
        };
        if options.only.as_deref().is_some_and(|only| only != id) {
            continue;
        }
        matched = true;

        let plugin = sync_one_plugin(options, &id, &path, &mut report.issues);
        report.plugins.push(plugin);
    }

    if let Some(only) = &options.only
        && !matched
    {
        report.issues.push(format!("plugin not found in repo: {only}"));
    }

    report.plugins.sort_by(|left, right| left.id.cmp(&right.id));
    report
}

fn sync_one_plugin(
    options: &SyncOptions,
    id: &str,
    repo_plugin_dir: &Path,
    issues: &mut Vec<String>,
) -> PluginSync {
    let vault_plugin_dir =
        options.bob_dir.join(VAULT_PLUGINS_SUBDIR).join(id);
    let mut files = Vec::new();

    for &name in MANAGED_FILES {
        let repo_file = repo_plugin_dir.join(name);
        if !repo_file.is_file() {
            continue;
        }
        let vault_file = vault_plugin_dir.join(name);
        let action = match sync_one_file(options, &repo_file, &vault_file) {
            Ok(action) => action,
            Err(message) => {
                issues.push(format!("{id}/{name}: {message}"));
                FileAction::Failed
            }
        };
        files.push(FileSync {
            name: name.to_string(),
            action,
        });
    }

    PluginSync {
        id: id.to_string(),
        files,
    }
}

fn sync_one_file(
    options: &SyncOptions,
    repo_file: &Path,
    vault_file: &Path,
) -> Result<FileAction, String> {
    let repo_bytes = fs::read(repo_file)
        .map_err(|error| format!("failed to read repo file: {error}"))?;

    let vault_exists = vault_file.is_file();
    let mut dirty = false;
    if vault_exists {
        let vault_bytes = fs::read(vault_file)
            .map_err(|error| format!("failed to read vault file: {error}"))?;
        if vault_bytes == repo_bytes {
            return Ok(FileAction::Unchanged);
        }
        dirty = vault_file_is_dirty(&options.bob_dir, vault_file);
        if dirty && !options.force {
            return Ok(FileAction::SkippedDirty);
        }
    }

    let action = if !vault_exists {
        FileAction::Created
    } else if dirty {
        FileAction::Forced
    } else {
        FileAction::Updated
    };

    if !options.dry_run {
        if let Some(parent) = vault_file.parent() {
            fs::create_dir_all(parent).map_err(|error| {
                format!("failed to create vault directory: {error}")
            })?;
        }
        fs::write(vault_file, &repo_bytes)
            .map_err(|error| format!("failed to write vault file: {error}"))?;
    }

    Ok(action)
}

/// Reports whether `vault_file` has uncommitted changes in the vault Git repo.
///
/// Uses `git status --porcelain` scoped to the single file. A vault that is not
/// a Git repo, or an unavailable `git`, yields `false`: there is no committed
/// state to protect, so the copy proceeds.
fn vault_file_is_dirty(bob_dir: &Path, vault_file: &Path) -> bool {
    let output = Command::new("git")
        .arg("-C")
        .arg(bob_dir)
        .arg("status")
        .arg("--porcelain")
        .arg("--")
        .arg(vault_file)
        .output();
    match output {
        Ok(output) if output.status.success() => !output.stdout.is_empty(),
        _ => false,
    }
}

fn print_sync_report(report: &SyncReport, dry_run: bool, styler: &Styler) {
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
            let detail = file_action_detail(file, dry_run);
            println!("  {prefix} {id}  {detail}");
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
        summary.push_str(&format!(
            " {separator} {} errors",
            report.issues.len()
        ));
    }
    println!("{summary}");
}

fn file_action_detail(file: &FileSync, dry_run: bool) -> String {
    let name = &file.name;
    let copy_verb = if dry_run { "would copy" } else { "copied" };
    match file.action {
        FileAction::Created => format!("{copy_verb} {name} (new)"),
        FileAction::Updated => format!("{copy_verb} {name}"),
        FileAction::Forced => {
            format!("{copy_verb} {name} (overwrote dirty vault file)")
        }
        FileAction::SkippedDirty => {
            format!("skipped {name} (dirty in vault; use -F/--force)")
        }
        FileAction::Failed => format!("failed to copy {name} (see error)"),
        FileAction::Unchanged => format!("{name} unchanged"),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PluginsReport {
    repo: PathBuf,
    bob_dir: PathBuf,
    plugins: Vec<PluginEntry>,
    issues: Vec<String>,
}

impl PluginsReport {
    fn counts(&self) -> StateCounts {
        let mut counts = StateCounts::default();
        for plugin in &self.plugins {
            match plugin.sync {
                SyncState::Synced => counts.synced += 1,
                SyncState::Drift => counts.drift += 1,
                SyncState::Missing => counts.not_installed += 1,
            }
        }
        counts
    }

    fn result(&self) -> PluginsResult {
        let counts = self.counts();
        PluginsResult {
            ok: true,
            repo: self.repo.display().to_string(),
            bob_dir: self.bob_dir.display().to_string(),
            count: self.plugins.len(),
            synced: counts.synced,
            drift: counts.drift,
            not_installed: counts.not_installed,
            plugins: self.plugins.clone(),
        }
    }

    fn issue_summary(&self) -> String {
        self.issues.join("; ")
    }
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
struct StateCounts {
    synced: usize,
    drift: usize,
    not_installed: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct PluginsResult {
    ok: bool,
    repo: String,
    bob_dir: String,
    count: usize,
    synced: usize,
    drift: usize,
    not_installed: usize,
    plugins: Vec<PluginEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct PluginEntry {
    id: String,
    version: String,
    description: String,
    sync: SyncState,
    vault: VaultState,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
enum SyncState {
    Synced,
    Drift,
    Missing,
}

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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
enum VaultState {
    Enabled,
    Disabled,
    NotInstalled,
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

#[derive(Debug, Default, Deserialize)]
struct Manifest {
    #[serde(default)]
    id: String,
    #[serde(default)]
    version: String,
    #[serde(default)]
    description: String,
}

fn scan_plugins(repo: &Path, bob_dir: &Path) -> PluginsReport {
    let mut plugins = Vec::new();
    let mut issues = Vec::new();
    let plugins_root = repo.join(REPO_PLUGINS_SUBDIR);
    let enabled = read_enabled_plugins(bob_dir);

    let entries = match read_sorted_directory(&plugins_root) {
        Ok(entries) => entries,
        Err(error) => {
            issues.push(format!(
                "failed to read plugins directory {}: {error}",
                plugins_root.display()
            ));
            return PluginsReport {
                repo: repo.to_path_buf(),
                bob_dir: bob_dir.to_path_buf(),
                plugins,
                issues,
            };
        }
    };

    for entry in entries {
        let path = entry.path();
        let is_dir = entry
            .file_type()
            .map(|file_type| file_type.is_dir())
            .unwrap_or(false);
        if !is_dir {
            continue;
        }
        let Some(folder) = path.file_name().and_then(|name| name.to_str())
        else {
            continue;
        };

        let manifest = match read_manifest(&path) {
            Ok(manifest) => manifest,
            Err(error) => {
                issues.push(format!("{folder}: {error}"));
                continue;
            }
        };

        let id = if manifest.id.is_empty() {
            folder.to_string()
        } else {
            manifest.id
        };
        let vault_plugin_dir = bob_dir.join(VAULT_PLUGINS_SUBDIR).join(&id);

        plugins.push(PluginEntry {
            sync: sync_state(&path, &vault_plugin_dir),
            vault: vault_state(&id, &enabled, &vault_plugin_dir),
            version: manifest.version,
            description: manifest.description,
            id,
        });
    }

    plugins.sort_by(|left, right| left.id.cmp(&right.id));
    PluginsReport {
        repo: repo.to_path_buf(),
        bob_dir: bob_dir.to_path_buf(),
        plugins,
        issues,
    }
}

fn read_manifest(plugin_dir: &Path) -> Result<Manifest, String> {
    let manifest_path = plugin_dir.join("manifest.json");
    let contents = fs::read_to_string(&manifest_path)
        .map_err(|error| format!("failed to read manifest.json: {error}"))?;
    serde_json::from_str(&contents)
        .map_err(|error| format!("failed to parse manifest.json: {error}"))
}

fn read_enabled_plugins(bob_dir: &Path) -> HashSet<String> {
    let path = bob_dir.join(COMMUNITY_PLUGINS_FILE);
    let Ok(contents) = fs::read_to_string(&path) else {
        return HashSet::new();
    };
    serde_json::from_str::<Vec<String>>(&contents)
        .unwrap_or_default()
        .into_iter()
        .collect()
}

fn sync_state(repo_plugin_dir: &Path, vault_plugin_dir: &Path) -> SyncState {
    if !vault_plugin_dir.is_dir() {
        return SyncState::Missing;
    }

    for file in MANAGED_FILES {
        let repo_file = repo_plugin_dir.join(file);
        if !repo_file.is_file() {
            continue;
        }
        let vault_file = vault_plugin_dir.join(file);
        match (fs::read(&repo_file), fs::read(&vault_file)) {
            (Ok(repo_bytes), Ok(vault_bytes)) if repo_bytes == vault_bytes => {}
            _ => return SyncState::Drift,
        }
    }

    SyncState::Synced
}

fn vault_state(
    id: &str,
    enabled: &HashSet<String>,
    vault_plugin_dir: &Path,
) -> VaultState {
    if enabled.contains(id) {
        VaultState::Enabled
    } else if vault_plugin_dir.is_dir() {
        VaultState::Disabled
    } else {
        VaultState::NotInstalled
    }
}

fn read_sorted_directory(directory: &Path) -> io::Result<Vec<fs::DirEntry>> {
    let mut entries =
        fs::read_dir(directory)?.collect::<Result<Vec<_>, io::Error>>()?;
    entries.sort_by_key(fs::DirEntry::path);
    Ok(entries)
}

fn print_plugins_table(report: &PluginsReport, styler: &Styler) {
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

fn terminal_width() -> usize {
    std::env::var("COLUMNS")
        .ok()
        .and_then(|value| value.trim().parse::<usize>().ok())
        .filter(|width| *width > 0)
        .unwrap_or(DEFAULT_TERM_WIDTH)
}

fn truncate(text: &str, width: usize) -> String {
    if display_width(text) <= width {
        return text.to_string();
    }
    if width == 0 {
        return String::new();
    }
    let kept: String = text.chars().take(width.saturating_sub(1)).collect();
    format!("{kept}\u{2026}")
}

fn success_json(result: &PluginsResult) -> String {
    serde_json::to_string(result).expect("serialize plugins result")
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
    fn sync_state_detects_synced_drift_and_missing() {
        let temp = TempDir::new("bob-cli-plugins-sync");
        let repo = temp.path().join("repo/plugins/alpha");
        let synced_vault = temp.path().join("vault/.obsidian/plugins/alpha");
        write_file(&repo.join("manifest.json"), "{\"id\":\"alpha\"}\n");
        write_file(&repo.join("main.js"), "console.log('alpha');\n");
        write_file(&synced_vault.join("manifest.json"), "{\"id\":\"alpha\"}\n");
        write_file(&synced_vault.join("main.js"), "console.log('alpha');\n");
        assert_eq!(sync_state(&repo, &synced_vault), SyncState::Synced);

        let drift_vault = temp.path().join("vault/.obsidian/plugins/beta");
        write_file(&drift_vault.join("manifest.json"), "{\"id\":\"alpha\"}\n");
        write_file(&drift_vault.join("main.js"), "console.log('old');\n");
        assert_eq!(sync_state(&repo, &drift_vault), SyncState::Drift);

        let missing_vault = temp.path().join("vault/.obsidian/plugins/gone");
        assert_eq!(sync_state(&repo, &missing_vault), SyncState::Missing);
    }

    #[test]
    fn vault_state_reads_enabled_disabled_and_not_installed() {
        let temp = TempDir::new("bob-cli-plugins-vault");
        let installed = temp.path().join(".obsidian/plugins/alpha");
        write_file(&installed.join("manifest.json"), "{}\n");
        let enabled: HashSet<String> = ["alpha".to_string()].into();

        assert_eq!(
            vault_state("alpha", &enabled, &installed),
            VaultState::Enabled
        );
        assert_eq!(
            vault_state("beta", &enabled, &installed),
            VaultState::Disabled
        );
        let missing = temp.path().join(".obsidian/plugins/gone");
        assert_eq!(
            vault_state("gone", &enabled, &missing),
            VaultState::NotInstalled
        );
    }

    #[test]
    fn scan_reports_states_and_counts() {
        let temp = TempDir::new("bob-cli-plugins-scan");
        let repo = temp.path().join("repo");
        let vault = temp.path().join("vault");

        write_plugin(&repo, "alpha", "1.0.0", "Alpha plugin", "alpha-body");
        write_plugin(&repo, "beta", "2.1.0", "Beta plugin", "beta-body");
        write_plugin(&repo, "gamma", "1.0.0", "Gamma plugin", "gamma-body");

        // alpha: identical in vault + enabled.
        write_vault_plugin(
            &vault,
            "alpha",
            "1.0.0",
            "Alpha plugin",
            "alpha-body",
        );
        // beta: installed but body differs + disabled.
        write_vault_plugin(
            &vault,
            "beta",
            "2.1.0",
            "Beta plugin",
            "stale-body",
        );
        // gamma: not installed.
        write_file(
            &vault.join(".obsidian/community-plugins.json"),
            "[\"alpha\"]\n",
        );

        let report = scan_plugins(&repo, &vault);
        assert!(report.issues.is_empty(), "issues: {:?}", report.issues);
        assert_eq!(report.plugins.len(), 3);

        let alpha = &report.plugins[0];
        assert_eq!(alpha.id, "alpha");
        assert_eq!(alpha.version, "1.0.0");
        assert_eq!(alpha.sync, SyncState::Synced);
        assert_eq!(alpha.vault, VaultState::Enabled);

        let beta = &report.plugins[1];
        assert_eq!(beta.sync, SyncState::Drift);
        assert_eq!(beta.vault, VaultState::Disabled);

        let gamma = &report.plugins[2];
        assert_eq!(gamma.sync, SyncState::Missing);
        assert_eq!(gamma.vault, VaultState::NotInstalled);

        let counts = report.counts();
        assert_eq!(counts.synced, 1);
        assert_eq!(counts.drift, 1);
        assert_eq!(counts.not_installed, 1);
    }

    #[test]
    fn unreadable_repo_is_an_error() {
        let temp = TempDir::new("bob-cli-plugins-empty");
        let report = scan_plugins(&temp.path().join("missing"), temp.path());
        assert_eq!(report.issues.len(), 1);
        assert!(report.plugins.is_empty());
    }

    #[test]
    fn json_shape_is_stable() {
        let temp = TempDir::new("bob-cli-plugins-json");
        let repo = temp.path().join("repo");
        let vault = temp.path().join("vault");
        write_plugin(&repo, "alpha", "1.0.0", "Alpha plugin", "body");
        write_vault_plugin(&vault, "alpha", "1.0.0", "Alpha plugin", "body");
        write_file(
            &vault.join(".obsidian/community-plugins.json"),
            "[\"alpha\"]\n",
        );

        let result = scan_plugins(&repo, &vault).result();
        let value: serde_json::Value =
            serde_json::from_str(&success_json(&result)).expect("json");
        assert_eq!(value["ok"], true);
        assert_eq!(value["count"], 1);
        assert_eq!(value["synced"], 1);
        assert_eq!(value["drift"], 0);
        assert_eq!(value["not_installed"], 0);
        assert_eq!(value["plugins"][0]["id"], "alpha");
        assert_eq!(value["plugins"][0]["version"], "1.0.0");
        assert_eq!(value["plugins"][0]["sync"], "synced");
        assert_eq!(value["plugins"][0]["vault"], "enabled");
    }

    #[test]
    fn truncate_adds_ellipsis_only_when_needed() {
        assert_eq!(truncate("short", 10), "short");
        assert_eq!(truncate("toolongdesc", 5), "tool\u{2026}");
        assert_eq!(truncate("anything", 0), "");
    }

    #[test]
    fn sync_creates_updates_and_leaves_unchanged() {
        let temp = TempDir::new("bob-cli-plugins-sync-actions");
        let repo = temp.path().join("repo");
        let vault = temp.path().join("vault");

        write_plugin(&repo, "alpha", "1.0.0", "Alpha", "alpha");
        write_plugin(&repo, "beta", "2.0.0", "Beta", "beta");
        write_plugin(&repo, "gamma", "1.0.0", "Gamma", "gamma");

        // alpha already matches; beta drifts; gamma is not installed.
        write_vault_plugin(&vault, "alpha", "1.0.0", "Alpha", "alpha");
        write_vault_plugin(&vault, "beta", "2.0.0", "Beta", "stale");

        let report = sync_plugins(&options(&repo, &vault));
        assert!(report.issues.is_empty(), "issues: {:?}", report.issues);

        assert_eq!(
            action_for(&report, "alpha", "manifest.json"),
            Some(FileAction::Unchanged)
        );
        assert_eq!(
            action_for(&report, "alpha", "main.js"),
            Some(FileAction::Unchanged)
        );
        // beta's manifest matches but its main.js drifts and is rewritten.
        assert_eq!(
            action_for(&report, "beta", "manifest.json"),
            Some(FileAction::Unchanged)
        );
        assert_eq!(
            action_for(&report, "beta", "main.js"),
            Some(FileAction::Updated)
        );
        assert_eq!(
            action_for(&report, "gamma", "manifest.json"),
            Some(FileAction::Created)
        );

        let beta_main =
            vault.join(".obsidian/plugins/beta/main.js");
        assert_eq!(fs::read_to_string(&beta_main).unwrap(), "// beta\n");
        let gamma_manifest =
            vault.join(".obsidian/plugins/gamma/manifest.json");
        assert!(gamma_manifest.is_file(), "gamma should be created");
        assert_eq!(report.copied(), 3);
        assert_eq!(report.unchanged(), 3);
    }

    #[test]
    fn sync_dry_run_reports_without_writing() {
        let temp = TempDir::new("bob-cli-plugins-sync-dry");
        let repo = temp.path().join("repo");
        let vault = temp.path().join("vault");
        write_plugin(&repo, "beta", "2.0.0", "Beta", "beta");
        write_vault_plugin(&vault, "beta", "2.0.0", "Beta", "stale");

        let mut opts = options(&repo, &vault);
        opts.dry_run = true;
        let report = sync_plugins(&opts);

        assert_eq!(
            action_for(&report, "beta", "main.js"),
            Some(FileAction::Updated)
        );
        let beta_main = vault.join(".obsidian/plugins/beta/main.js");
        assert_eq!(
            fs::read_to_string(&beta_main).unwrap(),
            "// stale\n",
            "dry-run must not write the vault file"
        );
    }

    #[test]
    fn sync_only_filters_to_a_single_plugin() {
        let temp = TempDir::new("bob-cli-plugins-sync-only");
        let repo = temp.path().join("repo");
        let vault = temp.path().join("vault");
        write_plugin(&repo, "alpha", "1.0.0", "Alpha", "alpha");
        write_plugin(&repo, "beta", "2.0.0", "Beta", "beta");

        let mut opts = options(&repo, &vault);
        opts.only = Some("beta".to_string());
        let report = sync_plugins(&opts);

        assert_eq!(report.plugins.len(), 1);
        assert_eq!(report.plugins[0].id, "beta");
        assert!(report.issues.is_empty());
        assert!(
            vault.join(".obsidian/plugins/beta/main.js").is_file(),
            "beta should be synced"
        );
        assert!(
            !vault.join(".obsidian/plugins/alpha").exists(),
            "alpha must be left untouched"
        );
    }

    #[test]
    fn sync_unknown_plugin_is_an_error() {
        let temp = TempDir::new("bob-cli-plugins-sync-unknown");
        let repo = temp.path().join("repo");
        let vault = temp.path().join("vault");
        write_plugin(&repo, "alpha", "1.0.0", "Alpha", "alpha");

        let mut opts = options(&repo, &vault);
        opts.only = Some("missing".to_string());
        let report = sync_plugins(&opts);

        assert!(report.plugins.is_empty());
        assert_eq!(report.issues.len(), 1);
        assert!(report.issues[0].contains("plugin not found in repo: missing"));
    }

    #[test]
    fn sync_preserves_runtime_data_json() {
        let temp = TempDir::new("bob-cli-plugins-sync-data");
        let repo = temp.path().join("repo");
        let vault = temp.path().join("vault");
        write_plugin(&repo, "beta", "2.0.0", "Beta", "beta");
        write_vault_plugin(&vault, "beta", "2.0.0", "Beta", "stale");
        let data_json = vault.join(".obsidian/plugins/beta/data.json");
        write_file(&data_json, "{\"setting\":true}\n");

        let report = sync_plugins(&options(&repo, &vault));
        assert!(report.issues.is_empty(), "issues: {:?}", report.issues);

        assert_eq!(
            fs::read_to_string(&data_json).unwrap(),
            "{\"setting\":true}\n",
            "data.json must never be touched by sync"
        );
        let beta_main = vault.join(".obsidian/plugins/beta/main.js");
        assert_eq!(fs::read_to_string(&beta_main).unwrap(), "// beta\n");
    }

    #[test]
    fn sync_refuses_then_forces_a_dirty_vault_file() {
        let temp = TempDir::new("bob-cli-plugins-sync-dirty");
        let repo = temp.path().join("repo");
        let vault = temp.path().join("vault");
        write_plugin(&repo, "beta", "2.0.0", "Beta", "beta");
        write_vault_plugin(&vault, "beta", "2.0.0", "Beta", "committed");

        // Commit the vault, then dirty beta's main.js so it differs from both
        // the committed version and the repo.
        git_init_commit(&vault);
        let beta_main = vault.join(".obsidian/plugins/beta/main.js");
        write_file(&beta_main, "// local edit\n");

        let report = sync_plugins(&options(&repo, &vault));
        assert!(report.issues.is_empty(), "issues: {:?}", report.issues);
        assert_eq!(
            action_for(&report, "beta", "main.js"),
            Some(FileAction::SkippedDirty)
        );
        assert_eq!(
            fs::read_to_string(&beta_main).unwrap(),
            "// local edit\n",
            "a dirty vault file must not be overwritten without --force"
        );

        let mut opts = options(&repo, &vault);
        opts.force = true;
        let forced = sync_plugins(&opts);
        assert_eq!(
            action_for(&forced, "beta", "main.js"),
            Some(FileAction::Forced)
        );
        assert_eq!(
            fs::read_to_string(&beta_main).unwrap(),
            "// beta\n",
            "--force should overwrite the dirty vault file"
        );
    }

    fn options(repo: &Path, vault: &Path) -> SyncOptions {
        SyncOptions {
            repo: repo.to_path_buf(),
            bob_dir: vault.to_path_buf(),
            only: None,
            dry_run: false,
            force: false,
        }
    }

    fn action_for(
        report: &SyncReport,
        id: &str,
        name: &str,
    ) -> Option<FileAction> {
        report
            .plugins
            .iter()
            .find(|plugin| plugin.id == id)
            .and_then(|plugin| {
                plugin.files.iter().find(|file| file.name == name)
            })
            .map(|file| file.action)
    }

    fn git_init_commit(repo: &Path) {
        run_git(repo, &["init", "-q"]);
        run_git(repo, &["add", "-A"]);
        run_git(
            repo,
            &[
                "-c",
                "user.email=test@example.com",
                "-c",
                "user.name=test",
                "-c",
                "commit.gpgsign=false",
                "commit",
                "-q",
                "-m",
                "init",
            ],
        );
    }

    fn run_git(repo: &Path, args: &[&str]) {
        let status = Command::new("git")
            .arg("-C")
            .arg(repo)
            .args(args)
            .status()
            .unwrap_or_else(|error| panic!("run git {args:?}: {error}"));
        assert!(status.success(), "git {args:?} failed");
    }

    fn write_plugin(
        repo: &Path,
        id: &str,
        version: &str,
        description: &str,
        body: &str,
    ) {
        let dir = repo.join("plugins").join(id);
        write_file(
            &dir.join("manifest.json"),
            &manifest_json(id, version, description),
        );
        write_file(&dir.join("main.js"), &format!("// {body}\n"));
    }

    fn write_vault_plugin(
        vault: &Path,
        id: &str,
        version: &str,
        description: &str,
        body: &str,
    ) {
        let dir = vault.join(".obsidian/plugins").join(id);
        write_file(
            &dir.join("manifest.json"),
            &manifest_json(id, version, description),
        );
        write_file(&dir.join("main.js"), &format!("// {body}\n"));
    }

    fn manifest_json(id: &str, version: &str, description: &str) -> String {
        format!(
            "{{\n  \"id\": \"{id}\",\n  \"version\": \"{version}\",\n  \"description\": \"{description}\"\n}}\n"
        )
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
