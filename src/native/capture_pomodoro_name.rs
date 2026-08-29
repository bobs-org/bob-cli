//! Single-purpose write command that names one open Pomodoro.

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
    capture_language,
    capture_pomodoros::{
        self, PomodoroEntry, PomodoroRef, PomodoroRefLookup, PomodoroState,
    },
    collect_done, env as bob_env, pomodoro,
    style::Styler,
};

const COMMAND_NAME: &str = "bob capture-pomodoro-name";

/// Bump only for a breaking change to the JSON object below; new optional
/// fields keep version 1.
const SCHEMA_VERSION: u32 = 1;

pub(crate) fn run(args: Vec<OsString>) -> i32 {
    let mut command = build_cli();
    let matches = match command.try_get_matches_from_mut(
        iter::once(OsString::from(COMMAND_NAME)).chain(args),
    ) {
        Ok(matches) => matches,
        Err(error) => return print_clap_error(error),
    };

    let output_format = OutputFormat::from_matches(&matches);
    let request = match CapturePomodoroNameRequest::from_matches(&matches) {
        Ok(request) => request,
        Err(error) => return print_error(error, output_format),
    };

    match assign_capture_pomodoro_name(request) {
        Ok(result) => {
            print_success(&result, output_format);
            0
        }
        Err(error) => print_error(error, output_format),
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
        .about("Assign a name to an open unnamed Pomodoro")
        .long_about(
            "Assign a canonical ALL-CAPS name to one open Pomodoro in today's \
Bob daily note.\n\n\
The command is the write half of '@route:id#' completion: callers first \
discover an unnamed Pomodoro with `bob capture-pomodoros`, then pass that \
candidate's stale-safe --pomodoro-ref together with the authored --name. \
Names are trimmed, internal whitespace is collapsed, and the result is \
ASCII-uppercased because the vault's named-Pomodoro convention is ALL-CAPS \
and case cannot affect the selector slug. The selected Pomodoro must still \
be open and must not already have a selectable name. A named-but-untypeable \
entry is the exception: naming it is the repair, and the command replaces \
the existing em-dash tail rather than appending a second one.\n\n\
On success the command appends ' — NAME' to the resolved physical line after \
trimming that line's trailing spaces, preserves that line's ending and every \
unrelated byte, and replaces the note with one same-directory temporary file \
rename. The written contents are re-scanned and must parse with the expected \
name and slug before success is reported. Dry-run plans the same mutation \
and returns the same success shape without writing. Stale, ambiguous, \
completed, already-named, missing, or unreadable Pomodoros fail without \
touching the note.",
        )
        .after_help(
            "Examples:\n  bob capture-pomodoro-name -p 38:0b1c2d3e -n 'deep work'\n  bob capture-pomodoro-name --pomodoro-ref 12:abcd1234 --name 'AFTER TUI FIX' -f json\n  bob capture-pomodoro-name -p 38:0b1c2d3e -n 'deep work' --dry-run\n  bob capture-pomodoro-name -b ~/bob -p 8:deadbeef -n MEMORY\n\nEnvironment:\n  BOB_DAY_FILE              Daily note override; otherwise <bob-dir>/YYYY/YYYYMMDD.md\n  BOB_DIR                   Bob vault root when --bob-dir is omitted\n  BOB_NOW                   Local datetime override for default daily-note selection",
        )
        .disable_help_flag(true)
        .arg(bob_dir_arg())
        .arg(dry_run_arg())
        .arg(format_arg())
        .arg(help_arg())
        .arg(name_arg())
        .arg(pomodoro_ref_arg())
}

fn bob_dir_arg() -> Arg {
    Arg::new("bob-dir")
        .long("bob-dir")
        .short('b')
        .value_name("DIR")
        .value_parser(OsStringValueParser::new())
        .help("Bob vault root; defaults to BOB_DIR or ~/bob")
}

fn dry_run_arg() -> Arg {
    Arg::new("dry-run")
        .long("dry-run")
        .short('d')
        .action(ArgAction::SetTrue)
        .help("Plan and report without writing the daily note")
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

fn name_arg() -> Arg {
    Arg::new("name")
        .long("name")
        .short('n')
        .value_name("NAME")
        .required(true)
        .help("Pomodoro name; letters, numbers, spaces, and & ' ( ) , . / -")
}

fn pomodoro_ref_arg() -> Arg {
    Arg::new("pomodoro-ref")
        .long("pomodoro-ref")
        .short('p')
        .value_name("REF")
        .required(true)
        .help("Stale-safe Pomodoro ref from capture-pomodoros")
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

#[derive(Debug, Clone, PartialEq, Eq)]
struct CapturePomodoroNameRequest {
    bob_dir: PathBuf,
    dry_run: bool,
    pomodoro_ref: PomodoroRef,
    name: String,
}

impl CapturePomodoroNameRequest {
    fn from_matches(
        matches: &ArgMatches,
    ) -> Result<Self, CapturePomodoroNameError> {
        Ok(Self {
            bob_dir: bob_dir_from_matches(matches),
            dry_run: matches.get_flag("dry-run"),
            pomodoro_ref: pomodoro_ref_from_matches(matches)?,
            name: name_from_matches(matches)?,
        })
    }
}

fn bob_dir_from_matches(matches: &ArgMatches) -> PathBuf {
    matches
        .get_one::<OsString>("bob-dir")
        .map(PathBuf::from)
        .map(|path| bob_env::expand_tilde(&path))
        .unwrap_or_else(bob_env::bob_dir)
}

fn pomodoro_ref_from_matches(
    matches: &ArgMatches,
) -> Result<PomodoroRef, CapturePomodoroNameError> {
    let value = matches.get_one::<String>("pomodoro-ref").expect("required");
    PomodoroRef::parse(value).ok_or_else(|| {
        CapturePomodoroNameError::usage(
            "--pomodoro-ref must use <line>:<digest>",
        )
    })
}

fn name_from_matches(
    matches: &ArgMatches,
) -> Result<String, CapturePomodoroNameError> {
    let name = matches.get_one::<String>("name").expect("required");
    capture_pomodoros::canonicalize_pomodoro_name(name).ok_or_else(|| {
        CapturePomodoroNameError::usage(capture_pomodoros::POMODORO_NAME_USAGE)
    })
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct CapturePomodoroNameResult {
    ok: bool,
    schema_version: u32,
    dry_run: bool,
    day_file: String,
    relative_day_file: String,
    name: String,
    slug: String,
    line: usize,
    #[serde(rename = "ref")]
    pomodoro_ref: String,
    pomodoro: PomodoroEntry,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct AssignmentPlan {
    target: PathBuf,
    updated: String,
    result: CapturePomodoroNameResult,
}

fn assign_capture_pomodoro_name(
    request: CapturePomodoroNameRequest,
) -> Result<CapturePomodoroNameResult, CapturePomodoroNameError> {
    let day_file = pomodoro::day_file_for(&request.bob_dir);
    assign_capture_pomodoro_name_at(request, &day_file)
}

fn assign_capture_pomodoro_name_at(
    request: CapturePomodoroNameRequest,
    day_file: &Path,
) -> Result<CapturePomodoroNameResult, CapturePomodoroNameError> {
    let plan = plan_assignment(&request, day_file)?;
    if !request.dry_run {
        collect_done::atomic_write(&plan.target, &plan.updated).map_err(
            |error| {
                CapturePomodoroNameError::io(format!(
                    "replace daily note {}: {error}",
                    plan.target.display()
                ))
            },
        )?;
    }
    Ok(plan.result)
}

fn plan_assignment(
    request: &CapturePomodoroNameRequest,
    day_file: &Path,
) -> Result<AssignmentPlan, CapturePomodoroNameError> {
    let relative_day_file =
        capture_pomodoros::relative_day_file(day_file, &request.bob_dir);
    let original = match fs::read_to_string(day_file) {
        Ok(contents) => contents,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Err(CapturePomodoroNameError::io(format!(
                "Bob daily note does not exist: {}",
                day_file.display()
            )));
        }
        Err(error) => {
            return Err(CapturePomodoroNameError::io(format!(
                "read daily note {}: {error}",
                day_file.display()
            )));
        }
    };

    let scan = capture_pomodoros::scan(&original);
    if !scan.has_section {
        return Err(CapturePomodoroNameError::io(format!(
            "Bob daily note has no Pomodoros section: {}",
            day_file.display()
        )));
    }

    let entry = match scan.by_ref(&request.pomodoro_ref) {
        PomodoroRefLookup::Found(entry) => entry,
        PomodoroRefLookup::Stale => {
            return Err(CapturePomodoroNameError::io(format!(
                "the selected Pomodoro is no longer in {relative_day_file}; rerun the Pomodoro picker"
            )));
        }
        PomodoroRefLookup::Ambiguous => {
            return Err(CapturePomodoroNameError::io(format!(
                "the selected Pomodoro matches more than one line in {relative_day_file}; rerun the Pomodoro picker"
            )));
        }
    };

    if entry.state == PomodoroState::Completed {
        return Err(CapturePomodoroNameError::io(format!(
            "Pomodoro on line {} is completed; only an open Pomodoro can be named",
            entry.line
        )));
    }
    if entry.selectable {
        let name = entry.name.as_deref().unwrap_or(&entry.slug);
        return Err(CapturePomodoroNameError::io(format!(
            "Pomodoro on line {} is already named {name}; target it with `#{}`",
            entry.line, entry.slug
        )));
    }

    let slug = capture_language::selector_slug(&request.name);
    let line_index = entry.line - 1;
    let replace_existing = entry.name.is_some();
    let updated = assign_name_to_line(
        &original,
        line_index,
        &request.name,
        replace_existing,
    )?;
    let updated_scan = capture_pomodoros::scan(&updated);
    let updated_entry = updated_scan
        .entries
        .iter()
        .find(|candidate| candidate.line == entry.line)
        .ok_or_else(|| {
            CapturePomodoroNameError::io(format!(
                "updated Pomodoro disappeared from {relative_day_file}"
            ))
        })?;
    if updated_entry.name.as_deref() != Some(request.name.as_str())
        || updated_entry.slug != slug
        || !updated_entry.selectable
    {
        return Err(CapturePomodoroNameError::io(format!(
            "failed to assign Pomodoro name {} on line {} of {relative_day_file}",
            request.name, entry.line
        )));
    }

    Ok(AssignmentPlan {
        target: day_file.to_path_buf(),
        updated,
        result: CapturePomodoroNameResult {
            ok: true,
            schema_version: SCHEMA_VERSION,
            dry_run: request.dry_run,
            day_file: day_file.display().to_string(),
            relative_day_file,
            name: request.name.clone(),
            slug,
            line: updated_entry.line,
            pomodoro_ref: updated_entry.pomodoro_ref.to_string(),
            pomodoro: updated_entry.clone(),
        },
    })
}

fn assign_name_to_line(
    contents: &str,
    line_index: usize,
    name: &str,
    replace_existing: bool,
) -> Result<String, CapturePomodoroNameError> {
    let mut updated = String::with_capacity(contents.len() + name.len() + 8);
    let mut found = false;
    for (index, line) in contents.split_inclusive('\n').enumerate() {
        if index == line_index {
            let (content, ending) = collect_done::split_line_ending(line);
            let trimmed = content.trim_end_matches([' ', '\t']);
            let base = if replace_existing {
                capture_pomodoros::without_name_tail(trimmed)
            } else {
                trimmed
            };
            updated.push_str(base);
            updated.push_str(" — ");
            updated.push_str(name);
            updated.push_str(ending);
            found = true;
        } else {
            updated.push_str(line);
        }
    }
    if !found {
        return Err(CapturePomodoroNameError::io(format!(
            "Pomodoro line {} is outside the daily note",
            line_index + 1
        )));
    }
    Ok(updated)
}

fn print_success(
    result: &CapturePomodoroNameResult,
    output_format: OutputFormat,
) {
    match output_format {
        OutputFormat::Human => print_human_success(result),
        OutputFormat::Json => println!("{}", success_json(result)),
    }
}

fn print_human_success(result: &CapturePomodoroNameResult) {
    let styler = Styler::detect();
    print!("{}", human_success(result, &styler));
}

fn human_success(
    result: &CapturePomodoroNameResult,
    styler: &Styler,
) -> String {
    let mut output = format!(
        "Capture Pomodoro name {} {}\n\n",
        styler.separator(),
        styler.cyan(&result.relative_day_file)
    );
    let verb = if result.dry_run {
        "would name"
    } else {
        "named"
    };
    output.push_str(&format!(
        "  {} {}\n",
        styler.success_prefix(result.dry_run),
        styler.cyan(&format!("{verb} {}", result.name))
    ));
    output.push_str(&format!(
        "    type #{}  line {}  {}\n",
        result.slug,
        result.line,
        styler.dim(&result.pomodoro_ref)
    ));
    output
}

fn success_json(result: &CapturePomodoroNameResult) -> String {
    serde_json::to_string(result)
        .expect("serialize capture pomodoro name result")
}

fn print_error(
    error: CapturePomodoroNameError,
    output_format: OutputFormat,
) -> i32 {
    match output_format {
        OutputFormat::Human => eprintln!("{COMMAND_NAME}: {}", error.message),
        OutputFormat::Json => {
            println!("{}", json!({ "ok": false, "error": error.message }))
        }
    }
    error.kind.exit_code()
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct CapturePomodoroNameError {
    kind: CapturePomodoroNameErrorKind,
    message: String,
}

impl CapturePomodoroNameError {
    fn usage(message: impl Into<String>) -> Self {
        Self {
            kind: CapturePomodoroNameErrorKind::Usage,
            message: message.into(),
        }
    }

    fn io(message: impl Into<String>) -> Self {
        Self {
            kind: CapturePomodoroNameErrorKind::Io,
            message: message.into(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CapturePomodoroNameErrorKind {
    Usage,
    Io,
}

impl CapturePomodoroNameErrorKind {
    fn exit_code(self) -> i32 {
        match self {
            Self::Usage => 2,
            Self::Io => 1,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        path::Path,
        sync::atomic::{AtomicUsize, Ordering},
        time::{SystemTime, UNIX_EPOCH},
    };

    static TEMP_COUNTER: AtomicUsize = AtomicUsize::new(0);

    fn request(
        bob_dir: &Path,
        pomodoro_ref: PomodoroRef,
        name: &str,
        dry_run: bool,
    ) -> CapturePomodoroNameRequest {
        CapturePomodoroNameRequest {
            bob_dir: bob_dir.to_path_buf(),
            dry_run,
            pomodoro_ref,
            name: name.to_string(),
        }
    }

    fn pomodoro_ref_for(contents: &str, line: usize) -> PomodoroRef {
        let scan = capture_pomodoros::scan(contents);
        scan.entries
            .iter()
            .find(|entry| entry.line == line)
            .map(|entry| entry.pomodoro_ref.clone())
            .unwrap_or_else(|| panic!("missing Pomodoro on line {line}"))
    }

    fn assign_at(
        day_file: &Path,
        request: CapturePomodoroNameRequest,
    ) -> Result<CapturePomodoroNameResult, CapturePomodoroNameError> {
        assign_capture_pomodoro_name_at(request, day_file)
    }

    #[test]
    fn build_cli_renders_without_panicking() {
        build_cli().debug_assert();
    }

    #[test]
    fn canonicalizes_names_and_rejects_invalid_ones() {
        assert_eq!(
            capture_pomodoros::canonicalize_pomodoro_name("  deep   work  ")
                .expect("valid"),
            "DEEP WORK"
        );
        assert_eq!(
            capture_pomodoros::canonicalize_pomodoro_name("after-tui-fix")
                .expect("hyphen"),
            "AFTER-TUI-FIX"
        );
        assert_eq!(
            capture_pomodoros::canonicalize_pomodoro_name(
                "q&a (draft), v2.0/ok-go"
            )
            .expect("punctuation"),
            "Q&A (DRAFT), V2.0/OK-GO"
        );
        assert_eq!(
            capture_pomodoros::canonicalize_pomodoro_name("bugs")
                .expect("lower"),
            "BUGS"
        );

        for invalid in ["", "   ", "123", "snake_case", "hello!", "— DASH"] {
            assert!(
                capture_pomodoros::canonicalize_pomodoro_name(invalid)
                    .is_none(),
                "{invalid}"
            );
        }
    }

    #[test]
    fn names_a_placeholder_on_an_lf_note_and_returns_the_updated_ref() {
        let temp = TempDir::new("bob-cli-capture-pomodoro-name-lf");
        let day_file = temp.path().join("2026/20260828.md");
        let original = concat!(
            "# Day\n",
            "## Pomodoros\n",
            "- [ ] ()\n",
            "- [ ] () — MEMORY\n",
        );
        write_file(&day_file, original);
        let pomodoro_ref = pomodoro_ref_for(original, 3);

        let result = assign_at(
            &day_file,
            request(temp.path(), pomodoro_ref.clone(), "DEEP WORK", false),
        )
        .expect("assign");

        assert_eq!(result.schema_version, 1);
        assert!(!result.dry_run);
        assert_eq!(result.day_file, day_file.display().to_string());
        assert_eq!(result.relative_day_file, "2026/20260828.md");
        assert_eq!(result.name, "DEEP WORK");
        assert_eq!(result.slug, "deep-work");
        assert_eq!(result.line, 3);
        assert_eq!(result.pomodoro.name.as_deref(), Some("DEEP WORK"));
        assert!(result.pomodoro.selectable);
        assert_eq!(
            fs::read_to_string(&day_file).expect("read"),
            concat!(
                "# Day\n",
                "## Pomodoros\n",
                "- [ ] () — DEEP WORK\n",
                "- [ ] () — MEMORY\n",
            )
        );
        let updated_ref =
            pomodoro_ref_for(&fs::read_to_string(&day_file).expect("read"), 3);
        assert_eq!(result.pomodoro_ref, updated_ref.to_string());
        assert_ne!(result.pomodoro_ref, pomodoro_ref.to_string());
    }

    #[test]
    fn names_a_crlf_note_without_touching_other_bytes() {
        let temp = TempDir::new("bob-cli-capture-pomodoro-name-crlf");
        let day_file = temp.path().join("2026/20260828.md");
        let original = concat!(
            "# Day  \r\n",
            "## Pomodoros\r\n",
            "- [ ] ()  \r\n",
            "- ordinary\r\n",
        );
        write_file(&day_file, original);
        let pomodoro_ref = pomodoro_ref_for(original, 3);

        assign_at(
            &day_file,
            request(temp.path(), pomodoro_ref, "DEEP WORK", false),
        )
        .expect("assign");

        assert_eq!(
            fs::read_to_string(&day_file).expect("read"),
            concat!(
                "# Day  \r\n",
                "## Pomodoros\r\n",
                "- [ ] () — DEEP WORK\r\n",
                "- ordinary\r\n",
            )
        );
    }

    #[test]
    fn dry_run_returns_the_plan_without_writing() {
        let temp = TempDir::new("bob-cli-capture-pomodoro-name-dry");
        let day_file = temp.path().join("2026/20260828.md");
        let original = "## Pomodoros\n- [ ] ()\n";
        write_file(&day_file, original);
        let pomodoro_ref = pomodoro_ref_for(original, 2);

        let result = assign_at(
            &day_file,
            request(temp.path(), pomodoro_ref, "DEEP WORK", true),
        )
        .expect("dry-run");

        assert!(result.dry_run);
        assert_eq!(result.name, "DEEP WORK");
        assert_eq!(result.slug, "deep-work");
        assert_eq!(fs::read_to_string(&day_file).expect("read"), original);
    }

    #[test]
    fn recovers_a_shifted_line_and_repairs_an_untypeable_name() {
        let temp = TempDir::new("bob-cli-capture-pomodoro-name-repair");
        let day_file = temp.path().join("2026/20260828.md");
        let original = "## Pomodoros\n- [ ] () — SNAKE_CASE\n";
        let original_ref = pomodoro_ref_for(original, 2);
        let shifted = concat!(
            "## Pomodoros\n",
            "Intro\n",
            "- [ ] () — SNAKE_CASE\n",
            "- keep me\n",
        );
        write_file(&day_file, shifted);

        let result = assign_at(
            &day_file,
            request(temp.path(), original_ref, "DEEP WORK", false),
        )
        .expect("repair");

        assert_eq!(result.line, 3);
        assert_eq!(result.name, "DEEP WORK");
        assert_eq!(
            fs::read_to_string(&day_file).expect("read"),
            concat!(
                "## Pomodoros\n",
                "Intro\n",
                "- [ ] () — DEEP WORK\n",
                "- keep me\n",
            )
        );
    }

    #[test]
    fn validation_failures_are_write_free() {
        let temp = TempDir::new("bob-cli-capture-pomodoro-name-errors");
        let day_file = temp.path().join("2026/20260828.md");
        let original = concat!(
            "## Pomodoros\n",
            "- [ ] () — SAME\n",
            "- [ ] () — SAME\n",
            "- [ ] () — MEMORY\n",
            "- [x] () — DONE\n",
            "- [ ] ()\n",
        );
        write_file(&day_file, original);
        let unnamed_ref = pomodoro_ref_for(original, 6);
        let named_ref = pomodoro_ref_for(original, 4);
        let done_ref = pomodoro_ref_for(original, 5);
        let same_digest = pomodoro_ref_for(original, 2).digest;
        let same_ref = PomodoroRef {
            line: 99,
            digest: same_digest,
        };

        let cases = [
            (
                request(temp.path(), named_ref, "DEEP WORK", false),
                "already named MEMORY; target it with `#memory`",
            ),
            (
                request(temp.path(), done_ref, "DEEP WORK", false),
                "is completed; only an open Pomodoro can be named",
            ),
            (
                request(
                    temp.path(),
                    PomodoroRef {
                        line: 99,
                        digest: "deadbeef".to_string(),
                    },
                    "DEEP WORK",
                    false,
                ),
                "no longer in 2026/20260828.md",
            ),
            (
                request(temp.path(), same_ref, "DEEP WORK", false),
                "matches more than one line",
            ),
        ];

        for (request, expected) in cases {
            let error =
                assign_at(&day_file, request).expect_err("expected failure");
            assert_eq!(
                error.kind,
                CapturePomodoroNameErrorKind::Io,
                "{expected}"
            );
            assert!(
                error.message.contains(expected),
                "expected {expected:?} in {}",
                error.message
            );
            assert_eq!(
                fs::read_to_string(&day_file).expect("read"),
                original,
                "{expected} mutated the note"
            );
        }

        let missing_day = temp.path().join("missing.md");
        let missing = assign_at(
            &missing_day,
            request(temp.path(), unnamed_ref.clone(), "DEEP WORK", false),
        )
        .expect_err("missing note");
        assert!(missing.message.contains("does not exist"));
        assert_eq!(fs::read_to_string(&day_file).expect("read"), original);

        let sectionless = temp.path().join("sectionless.md");
        write_file(&sectionless, "# Day\n## Notes\n- no ledger\n");
        let missing_section = assign_at(
            &sectionless,
            request(temp.path(), unnamed_ref.clone(), "DEEP WORK", false),
        )
        .expect_err("missing section");
        assert!(missing_section.message.contains("no Pomodoros section"));
        assert_eq!(
            fs::read_to_string(&sectionless).expect("read sectionless"),
            "# Day\n## Notes\n- no ledger\n"
        );

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let locked = temp.path().join("locked.md");
            write_file(&locked, original);
            fs::set_permissions(&locked, fs::Permissions::from_mode(0o000))
                .expect("lock note");
            let error = assign_at(
                &locked,
                request(temp.path(), unnamed_ref, "DEEP WORK", false),
            )
            .expect_err("unreadable note");
            assert!(
                error.message.contains("read daily note"),
                "{}",
                error.message
            );
            fs::set_permissions(&locked, fs::Permissions::from_mode(0o644))
                .expect("unlock note");
            assert_eq!(
                fs::read_to_string(&locked).expect("read locked"),
                original
            );
        }
    }

    #[test]
    fn json_and_human_success_shapes_are_stable() {
        let result = CapturePomodoroNameResult {
            ok: true,
            schema_version: SCHEMA_VERSION,
            dry_run: false,
            day_file: "/tmp/bob/2026/20260828.md".to_string(),
            relative_day_file: "2026/20260828.md".to_string(),
            name: "DEEP WORK".to_string(),
            slug: "deep-work".to_string(),
            line: 38,
            pomodoro_ref: "38:0b1c2d3e".to_string(),
            pomodoro: PomodoroEntry {
                pomodoro_ref: PomodoroRef {
                    line: 38,
                    digest: "0b1c2d3e".to_string(),
                },
                line: 38,
                state: PomodoroState::Open,
                status_symbol: ' ',
                name: Some("DEEP WORK".to_string()),
                slug: "deep-work".to_string(),
                selectable: true,
                time_range: None,
                placeholder: true,
                is_current: false,
                child_count: 0,
            },
        };

        let value: serde_json::Value =
            serde_json::from_str(&success_json(&result)).expect("json");
        assert_eq!(value["ok"], true);
        assert_eq!(value["schema_version"], 1);
        assert_eq!(value["dry_run"], false);
        assert_eq!(value["day_file"], "/tmp/bob/2026/20260828.md");
        assert_eq!(value["relative_day_file"], "2026/20260828.md");
        assert_eq!(value["name"], "DEEP WORK");
        assert_eq!(value["slug"], "deep-work");
        assert_eq!(value["line"], 38);
        assert_eq!(value["ref"], "38:0b1c2d3e");
        assert_eq!(value["pomodoro"]["ref"], "38:0b1c2d3e");
        assert_eq!(value["pomodoro"]["name"], "DEEP WORK");
        assert_eq!(value["pomodoro"]["slug"], "deep-work");
        assert_eq!(value["pomodoro"]["selectable"], true);
        assert_eq!(value["pomodoro"]["placeholder"], true);

        let human = human_success(&result, &Styler::plain());
        assert!(human.contains("Capture Pomodoro name - 2026/20260828.md"));
        assert!(human.contains("named DEEP WORK"));
        assert!(human.contains("type #deep-work"));
        assert!(human.contains("line 38"));
        assert!(human.contains("38:0b1c2d3e"));
        assert!(!human.contains('\u{1b}'));
    }

    fn write_file(path: &Path, contents: &str) {
        fs::create_dir_all(path.parent().expect("file parent"))
            .expect("create file parent");
        fs::write(path, contents).expect("write file");
    }

    struct TempDir {
        path: PathBuf,
    }

    impl TempDir {
        fn new(prefix: &str) -> Self {
            let nonce = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("system clock before epoch")
                .as_nanos();
            let sequence = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "{prefix}-{}-{nonce}-{sequence}",
                std::process::id()
            ));
            fs::create_dir_all(&path).expect("create temp dir");
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
}
