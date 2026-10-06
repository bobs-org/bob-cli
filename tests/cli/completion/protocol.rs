//! Protocol 1 goldens: groups, pairing, filtering, directives, skew,
//! malformed requests, and the debug log.

use crate::support::*;
use std::process::Output;

/// Run `bob __complete` with protocol 1 over shell words.
fn complete(words: &[&str]) -> Output {
    complete_with_protocol(words, "1", None)
}

fn complete_with_protocol(
    words: &[&str],
    protocol: &str,
    debug_file: Option<&std::path::Path>,
) -> Output {
    let mut command = bob_command();
    command
        .arg("__complete")
        .arg("zsh")
        .arg("--protocol")
        .arg(protocol)
        .arg("--");
    for word in words {
        command.arg(word);
    }
    if let Some(path) = debug_file {
        command.env("BOB_COMPLETE_DEBUG", path);
    }
    command.output().expect("run bob __complete")
}

/// Split stdout into `(value, description, group, suffix)` rows,
/// skipping directive lines.
fn rows(output: &Output) -> Vec<Vec<String>> {
    stdout(output)
        .lines()
        .filter(|line| !line.starts_with('!'))
        .map(|line| line.split('\t').map(str::to_string).collect())
        .collect()
}

fn directives(output: &Output) -> Vec<String> {
    stdout(output)
        .lines()
        .filter(|line| line.starts_with('!'))
        .map(str::to_string)
        .collect()
}

fn values(output: &Output) -> Vec<String> {
    rows(output).iter().map(|row| row[0].clone()).collect()
}

fn groups(output: &Output) -> Vec<String> {
    rows(output).iter().map(|row| row[2].clone()).collect()
}

#[test]
fn empty_root_offers_commands_then_capture_protocol_without_options() {
    let output = complete(&["bob", ""]);
    assert_success(&output);
    assert!(stderr(&output).is_empty());
    let names = values(&output);
    for command in ["capture", "freshness", "query", "vault-sync"] {
        assert!(
            names.contains(&command.to_string()),
            "missing {command}: {names:?}"
        );
    }
    for endpoint in ["capture-complete", "capture-tasks", "capture-targets"] {
        assert!(
            names.contains(&endpoint.to_string()),
            "missing {endpoint}: {names:?}"
        );
    }
    assert!(
        !names.iter().any(|name| name.starts_with('-')),
        "empty root must not offer options: {names:?}"
    );
    let group_list = groups(&output);
    let mut section_order = Vec::new();
    for group in &group_list {
        if section_order.last() != Some(group) {
            section_order.push(group.clone());
        }
    }
    assert_eq!(
        section_order,
        vec![
            "daily workflow".to_string(),
            "tasks and projects".to_string(),
            "vault".to_string(),
            "integrations".to_string(),
            "setup".to_string(),
            "capture protocol".to_string(),
        ]
    );
    assert!(!group_list.contains(&"options".to_string()));
    assert_stdout_has_no_ansi(&output);
}

#[test]
fn alias_completion_uses_canonical_path_and_freshness_seed_is_hidden() {
    let alias = complete(&["bob", "task-status-setter", "--"]);
    assert_success(&alias);
    let options = values(&alias);
    assert!(options.contains(&"--retry-timeout".to_string()));
    assert!(!stdout(&alias).contains("task-status-setter"));
    assert!(!stdout(&alias).contains("mark-next-tasks"));

    let freshness = complete(&["bob", "freshness", ""]);
    assert_success(&freshness);
    assert!(!values(&freshness).contains(&"seed".to_string()));
}

#[test]
fn partial_command_word_yields_the_full_unfiltered_set() {
    let output = complete(&["bob", "cap"]);
    assert_success(&output);
    let names = values(&output);
    assert!(names.contains(&"capture".to_string()));
    assert!(names.contains(&"vault-sync".to_string()));
    assert!(names.contains(&"capture-complete".to_string()));
}

#[test]
fn lone_dash_offers_paired_forms() {
    let output = complete(&["bob", "capture", "-"]);
    assert_success(&output);
    let table = rows(&output);
    let short = table
        .iter()
        .position(|row| row[0] == "-b")
        .expect("-b present");
    assert_eq!(table[short + 1][0], "--bob-dir");
    assert_eq!(table[short][1], table[short + 1][1]);
    assert!(!table[short][1].is_empty());
    assert!(table.iter().all(|row| row[2] == "options"));
}

#[test]
fn double_dash_offers_long_forms_only() {
    let output = complete(&["bob", "capture", "--"]);
    assert_success(&output);
    let names = values(&output);
    assert!(names.contains(&"--bob-dir".to_string()));
    assert!(
        !names
            .iter()
            .any(|name| name.starts_with('-') && !name.starts_with("--")),
        "short forms must not appear: {names:?}"
    );
}

#[test]
fn present_option_is_dropped() {
    let output = complete(&["bob", "capture", "--route", "cash", "--"]);
    assert_success(&output);
    let names = values(&output);
    assert!(!names.contains(&"--route".to_string()));
    assert!(!names.contains(&"-r".to_string()));
    assert!(names.contains(&"--format".to_string()));
}

#[test]
fn conflicting_option_is_dropped() {
    let output = complete(&["bob", "capture", "--section", "x", "--"]);
    assert_success(&output);
    let names = values(&output);
    assert!(!names.contains(&"--task".to_string()));
    assert!(!names.contains(&"-t".to_string()));
    assert!(!names.contains(&"--task-section".to_string()));
    assert!(names.contains(&"--route".to_string()));
}

#[test]
fn format_values_carry_the_slot_group() {
    let output = complete(&["bob", "capture-sections", "--format", ""]);
    assert_success(&output);
    assert!(directives(&output).is_empty());
    let table = rows(&output);
    let format: Vec<&Vec<String>> =
        table.iter().filter(|row| row[2] == "format").collect();
    assert_eq!(format.len(), 2);
    assert_eq!(format[0][0], "human");
    assert_eq!(format[1][0], "json");
}

#[test]
fn freshness_short_option_value_completes() {
    let output = complete(&["bob", "freshness", "-f", ""]);
    assert_success(&output);
    let names = values(&output);
    assert!(names.contains(&"human".to_string()));
    assert!(names.contains(&"json".to_string()));
}

#[test]
fn file_and_dir_slots_emit_directives() {
    let output = complete(&["bob", "query", "--query-file", ""]);
    assert_success(&output);
    assert_eq!(directives(&output), vec!["!files".to_string()]);
    assert!(rows(&output).is_empty());

    let output = complete(&["bob", "capture", "--bob-dir", ""]);
    assert_success(&output);
    assert_eq!(directives(&output), vec!["!dirs".to_string()]);
}

#[test]
fn attached_option_value_uses_prefix() {
    let output = complete(&["bob", "capture", "--format="]);
    assert_success(&output);
    let lines: Vec<String> =
        stdout(&output).lines().map(str::to_string).collect();
    assert_eq!(lines[0], "!prefix 9");
    let names = values(&output);
    assert!(names.contains(&"human".to_string()));
    assert!(names.contains(&"json".to_string()));
}

#[test]
fn value_hints_beat_generic_kinds_entries() {
    // `install -t` carries a `DirPath` hint: directories, not a message.
    let output = complete(&["bob", "completion", "install", "-t", ""]);
    assert_success(&output);
    assert_eq!(directives(&output), vec!["!dirs".to_string()]);

    // `completion zsh -o` carries a `FilePath` hint: plain files, never
    // the highlights PDF glob.
    let output = complete(&["bob", "completion", "zsh", "-o", ""]);
    assert_success(&output);
    assert_eq!(directives(&output), vec!["!files".to_string()]);

    let output = complete(&["bob", "completion", "bash", "--output", ""]);
    assert_success(&output);
    assert_eq!(directives(&output), vec!["!files".to_string()]);

    // The highlights PDF `--output` keeps its glob through the
    // path-specific entries.
    for command in ["clip", "create"] {
        let output = complete(&["bob", "highlights", command, "--output", ""]);
        assert_success(&output);
        assert_eq!(directives(&output), vec!["!files *.pdf".to_string()]);
    }
}

#[test]
fn empty_cursor_at_positional_slot_offers_values_first() {
    // `highlights create` waits on a Markdown or PDF file first, not options.
    let output = complete(&["bob", "highlights", "create", ""]);
    assert_success(&output);
    assert_eq!(directives(&output), vec!["!files *.{md,pdf}".to_string()]);
    assert!(rows(&output).is_empty());

    // Slots without a value decision keep the options fallback.
    let output = complete(&["bob", "capture-sections", ""]);
    assert_success(&output);
    let names = values(&output);
    assert!(names.contains(&"--route".to_string()), "{names:?}");

    let output = complete(&["bob", "notify", ""]);
    assert_success(&output);
    let names = values(&output);
    assert!(names.contains(&"--verbose".to_string()), "{names:?}");
    assert!(names.contains(&"--help".to_string()), "{names:?}");
}

#[test]
fn text_started_slot_offers_no_options() {
    // Since the capture-text phase, `TEXT` on the capture trio goes through
    // the live marker extraction: a dash word with no marker offers nothing
    // (no options per rule 7, no interim message).
    let output = complete(&["bob", "capture", "fix", "-"]);
    assert_success(&output);
    let lines: Vec<String> =
        stdout(&output).lines().map(str::to_string).collect();
    assert!(lines.is_empty(), "{lines:?}");
}

#[test]
fn nested_subcommands_complete() {
    let output = complete(&["bob", "gkeep", ""]);
    assert_success(&output);
    let names = values(&output);
    for subcommand in ["doctor", "list", "login", "pull"] {
        assert!(
            names.contains(&subcommand.to_string()),
            "missing {subcommand}: {names:?}"
        );
    }
    assert!(!names.iter().any(|name| name.starts_with('-')));
}

#[test]
fn hidden_aliases_stay_hidden() {
    let output = complete(&["bob", "mark-"]);
    assert_success(&output);
    let text = stdout(&output);
    assert!(
        !text.contains("mark-next-tasks"),
        "hidden alias leaked:\n{text}"
    );
    assert!(
        !text.contains("task-status-setter"),
        "hidden alias leaked:\n{text}"
    );
}

#[test]
fn protocol_skew_reports_both_directions() {
    let output = complete_with_protocol(&["bob", ""], "0", None);
    assert_success(&output);
    assert_eq!(
        stdout(&output).trim(),
        "!message bob shell completion is out of date \u{2014} run: bob completion install"
    );

    let output = complete_with_protocol(&["bob", ""], "99", None);
    assert_success(&output);
    assert_eq!(
        stdout(&output).trim(),
        "!message this bob is older than its shell completion \u{2014} reinstall bob (just install)"
    );
}

#[test]
fn malformed_request_exits_two() {
    let output = bob_command()
        .arg("__complete")
        .output()
        .expect("run bare __complete");
    assert_eq!(output.status.code(), Some(2));
    assert!(!stderr(&output).is_empty());
}

#[test]
fn debug_log_writes_the_file_and_nothing_else() {
    let temp = TempDir::new("bob-cli-complete-debug");
    let log = temp.path().join("complete.log");
    let output = complete_with_protocol(&["bob", ""], "1", Some(&log));
    assert_success(&output);
    assert!(stderr(&output).is_empty());
    let logged = std::fs::read_to_string(&log).expect("debug log written");
    assert!(logged.contains("request="), "missing request:\n{logged}");
    assert!(logged.contains("response="), "missing response:\n{logged}");
    assert!(logged.contains("elapsed_ms="), "missing timing:\n{logged}");
    // The log changes nothing about the response itself.
    let plain = complete(&["bob", ""]);
    assert_eq!(stdout(&output), stdout(&plain));
}
