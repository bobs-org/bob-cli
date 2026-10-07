//! Alphabetical option and subcommand help listings.

use crate::support::*;

#[test]
fn capture_help_lists_options_alphabetically() {
    let output = bob_command()
        .arg("capture")
        .arg("--help")
        .output()
        .expect("run bob capture --help");

    assert_success(&output);
    let help = stdout(&output);
    assert!(
        help.contains(
            "Capture one or more tasks or bullets into the Bob Obsidian vault"
        ),
        "expected capture long help:\n{help}"
    );
    assert!(
        help.contains("Append a trailing lowercase 's:<N>' token")
            && help.contains("bob capture buy milk s:1")
            && help.contains("bob capture buy milk s:2 @groceries"),
        "expected capture schedule help:\n{help}"
    );
    assert!(
        help.contains("@<route>:<block-id>")
            && help.contains("bob capture '@dev:foobar'")
            && help.contains("bob capture '@dev:foobar#bugs'")
            && help.contains("BOB_DAY_FILE"),
        "expected Pomodoro-linked capture help:\n{help}"
    );
    assert!(
        help.contains("BOB_CLIPBOARD_CMD")
            && help.contains("BOB_CLIPBOARD_HISTORY_CMD")
            && help.contains("%<positive integer>")
            && help.contains("%<nonnumeric header>")
            && help.contains("--clip")
            && help.contains("--no-clip")
            && help.contains("'%1' is equivalent to '%'")
            && help.contains("'%0' stays literal")
            && help.contains("bob capture research links %3")
            && help.contains("Bare --clip also captures without a header")
            && help.contains("flat unordered Markdown")
            && help.contains("source list markers removed")
            && !help.contains("bare '%' renders as '**CLIP:**'"),
        "expected clipboard capture help:\n{help}"
    );
    assert_text_order(
        &help,
        &[
            "-b, --bob-dir",
            "-c, --clip",
            "-d, --dry-run",
            "-f, --format",
            "-h, --help",
            "-n, --no-clip",
            "-r, --route",
            "-s, --section",
            "-t, --task",
            "-S, --task-section",
        ],
    );
    assert!(
        help.contains("@<route>+<block-id>")
            && help.contains("@<route>+<block-id>#<section>")
            && help.contains("bob capture '@cash+goog-exit'")
            && help.contains("bob capture 'Postgres 17 minimum @foo+bar#requirements'")
            && help.contains("--task-section REQUIREMENTS")
            && help.contains("first direct-child Schedule Log or Work Log")
            && help.contains("@<route>^<block-id>")
            && help.contains("bob capture '@dev^foobar'")
            && help.contains("@@<route>")
            && help.contains("@@<route>+<block-id>")
            && help.contains("printf '@@foo\\nFirst task\\n\\nSecond task @bar\\n'")
            && !help.contains("--task-ref"),
        "expected public sub-bullet and ID-only help without hidden task ref:\n{help}"
    );
    assert_stdout_has_no_ansi(&output);
}

#[test]
fn task_status_hooks_help_lists_options_alphabetically() {
    let output = bob_command()
        .args(["task", "reconcile"])
        .arg("--help")
        .output()
        .expect("run bob task reconcile --help");

    assert_success(&output);
    let help = stdout(&output);
    assert!(
        help.contains("Make the current Pomodoro ledger the source of truth")
            && help.contains("BOB_DAY_FILE")
            && help.contains("bob task reconcile --dry-run"),
        "expected bob task reconcile long help:\n{help}"
    );
    assert_text_order(
        &help,
        &[
            "-b, --bob-dir",
            "-d, --dry-run",
            "-f, --format",
            "-h, --help",
            "-r, --retry-timeout",
        ],
    );
    assert_stdout_has_no_ansi(&output);
}

#[test]
fn capture_complete_help_lists_options_alphabetically() {
    let output = bob_command()
        .arg("capture-complete")
        .arg("--help")
        .output()
        .expect("run bob capture-complete --help");

    assert_success(&output);
    let help = stdout(&output);
    assert!(
        help.contains("cursor-aware completion candidates")
            && help.contains("task_section")
            && help.contains("capture-task-sections")
            && help.contains("creates_pomodoro")
            && help.contains("creates the named placeholder later"),
        "expected capture-complete long help:\n{help}"
    );
    assert_text_order(
        &help,
        &[
            "-a, --all-tasks",
            "-b, --bob-dir",
            "-c, --cursor",
            "-f, --format",
            "-h, --help",
        ],
    );
    assert_stdout_has_no_ansi(&output);
}

#[test]
fn capture_sections_help_lists_options_alphabetically() {
    let output = bob_command()
        .arg("capture-sections")
        .arg("--help")
        .output()
        .expect("run bob capture-sections --help");

    assert_success(&output);
    let help = stdout(&output);
    assert!(
        help.contains("Missing notes are not errors"),
        "expected capture-sections long help:\n{help}"
    );
    assert_text_order(
        &help,
        &["-b, --bob-dir", "-f, --format", "-h, --help", "-r, --route"],
    );
    assert_stdout_has_no_ansi(&output);
}

#[test]
fn capture_task_id_help_lists_options_alphabetically() {
    let output = bob_command()
        .arg("capture-task-id")
        .arg("--help")
        .output()
        .expect("run bob capture-task-id --help");

    assert_success(&output);
    let help = stdout(&output);
    assert!(
        help.contains("Assign a user-authored Obsidian block ID"),
        "expected capture-task-id long help:\n{help}"
    );
    assert_text_order(
        &help,
        &[
            "-a, --allow-closed",
            "-i, --block-id",
            "-b, --bob-dir",
            "-d, --dry-run",
            "-f, --format",
            "-h, --help",
            "-n, --note-path",
            "-r, --route",
            "-t, --task-ref",
        ],
    );
    assert_stdout_has_no_ansi(&output);
}

#[test]
fn capture_task_sections_help_lists_options_alphabetically() {
    let output = bob_command()
        .arg("capture-task-sections")
        .arg("--help")
        .output()
        .expect("run bob capture-task-sections --help");

    assert_success(&output);
    let help = stdout(&output);
    assert!(
        help.contains("ALL-CAPS direct-child section bullets")
            && help.contains("Exactly one of -i/--block-id or -t/--task-ref")
            && help.contains("successful empty list"),
        "expected capture-task-sections long help:\n{help}"
    );
    assert_text_order(
        &help,
        &[
            "-i, --block-id",
            "-b, --bob-dir",
            "-f, --format",
            "-h, --help",
            "-r, --route",
            "-t, --task-ref",
        ],
    );
    assert_stdout_has_no_ansi(&output);
}

#[test]
fn capture_tasks_help_lists_options_alphabetically() {
    let output = bob_command()
        .arg("capture-tasks")
        .arg("--help")
        .output()
        .expect("run bob capture-tasks --help");

    assert_success(&output);
    let help = stdout(&output);
    assert!(
        help.contains("Missing notes are not errors"),
        "expected capture-tasks long help:\n{help}"
    );
    assert_text_order(
        &help,
        &["-b, --bob-dir", "-f, --format", "-h, --help", "-r, --route"],
    );
    assert_stdout_has_no_ansi(&output);
}

#[test]
fn freshness_help_hides_the_seed_subcommand() {
    let output = bob_command()
        .arg("freshness")
        .arg("--help")
        .output()
        .expect("run bob freshness --help");

    assert_success(&output);
    let help = stdout(&output);
    assert!(
        help.contains("Walk the tiered freshness review queue")
            && help.contains(
                "hidden `seed` subcommand stamped the one-time cutover"
            )
            && !help.lines().any(|line| line.starts_with("  seed ")),
        "expected freshness long help:\n{help}"
    );
    assert!(help.lines().any(|line| line.starts_with("  list ")));
    assert_stdout_has_no_ansi(&output);
}

#[test]
fn default_subcommands_are_labeled_in_help() {
    let cases: &[(&[&str], &str)] = &[
        (&["completion", "--help"], "status"),
        (&["freshness", "--help"], "list"),
        (&["gkeep", "--help"], "list"),
        (&["plugins", "--help"], "list"),
        (&["vault-sync", "--help"], "run"),
    ];

    for (args, default_member) in cases {
        let output = bob_command()
            .args(*args)
            .output()
            .unwrap_or_else(|error| panic!("run bob {args:?}: {error}"));
        assert_success(&output);
        let labeled = stdout(&output).lines().any(|line| {
            line.split_whitespace().next() == Some(default_member)
                && line.contains("(default)")
        });
        assert!(
            labeled,
            "default member `{default_member}` is not labeled for {args:?}:\n{}",
            format_output(&output)
        );
    }
}

#[test]
fn freshness_list_help_lists_options_alphabetically() {
    let output = bob_command()
        .arg("freshness")
        .arg("list")
        .arg("--help")
        .output()
        .expect("run bob freshness list --help");

    assert_success(&output);
    let help = stdout(&output);
    assert!(
        help.contains("List the tiered freshness review queue")
            && help.contains("--limit"),
        "expected freshness list long help:\n{help}"
    );
    assert_text_order(
        &help,
        &["-b, --bob-dir", "-f, --format", "-h, --help", "-l, --limit"],
    );
    assert_stdout_has_no_ansi(&output);
}

#[test]
fn freshness_seed_help_lists_options_alphabetically() {
    let output = bob_command()
        .arg("freshness")
        .arg("seed")
        .arg("--help")
        .output()
        .expect("run bob freshness seed --help");

    assert_success(&output);
    let help = stdout(&output);
    assert!(
        help.contains("Stamp the one-time freshness cutover seed")
            && help.contains("--dry-run")
            && help.contains("--force"),
        "expected freshness seed long help:\n{help}"
    );
    assert_text_order(
        &help,
        &[
            "-b, --bob-dir",
            "-d, --dry-run",
            "-F, --force",
            "-f, --format",
            "-h, --help",
        ],
    );
    assert_stdout_has_no_ansi(&output);
}

#[test]
fn capture_targets_help_lists_options_alphabetically() {
    let output = bob_command()
        .arg("capture-targets")
        .arg("--help")
        .output()
        .expect("run bob capture-targets --help");

    assert_success(&output);
    let help = stdout(&output);
    assert!(
        help.contains("always pins mac_inbox first as the default"),
        "expected capture-targets long help:\n{help}"
    );
    assert_text_order(
        &help,
        &[
            "-b, --bob-dir",
            "-f, --format",
            "-h, --help",
            "-v, --verbose",
        ],
    );
    assert_stdout_has_no_ansi(&output);
}

#[test]
fn capture_parse_help_lists_options_alphabetically() {
    let output = bob_command()
        .arg("capture-parse")
        .arg("--help")
        .output()
        .expect("run bob capture-parse --help");

    assert_success(&output);
    let help = stdout(&output);
    assert!(
        help.contains("purely lexical and completely read-only")
            && help.contains("@route+id#")
            && help.contains("task_section")
            && help.contains("pomodoro_name")
            && help.contains("@foo+bar#req"),
        "expected capture-parse long help:\n{help}"
    );
    assert_text_order(&help, &["-f, --format", "-h, --help"]);
    assert_stdout_has_no_ansi(&output);
}

#[test]
fn capture_pomodoro_name_help_lists_options_alphabetically() {
    let output = bob_command()
        .arg("capture-pomodoro-name")
        .arg("--help")
        .output()
        .expect("run bob capture-pomodoro-name --help");

    assert_success(&output);
    let help = stdout(&output);
    assert!(
        help.contains("canonical ALL-CAPS name")
            && help.contains("named-but-untypeable")
            && help.contains("same-directory temporary file"),
        "expected capture-pomodoro-name long help:\n{help}"
    );
    assert_text_order(
        &help,
        &[
            "-b, --bob-dir",
            "-d, --dry-run",
            "-f, --format",
            "-h, --help",
            "-n, --name",
            "-p, --pomodoro-ref",
        ],
    );
    assert_stdout_has_no_ansi(&output);
}

#[test]
fn capture_pomodoros_help_lists_options_alphabetically() {
    let output = bob_command()
        .arg("capture-pomodoros")
        .arg("--help")
        .output()
        .expect("run bob capture-pomodoros --help");

    assert_success(&output);
    let help = stdout(&output);
    assert!(
        help.contains("Pomodoro entries from today's Bob daily note")
            && help.contains("successful empty list with a warning"),
        "expected capture-pomodoros long help:\n{help}"
    );
    assert_text_order(
        &help,
        &["-a, --all", "-b, --bob-dir", "-f, --format", "-h, --help"],
    );
    assert_stdout_has_no_ansi(&output);
}

#[test]
fn capture_rewrite_help_lists_options_alphabetically() {
    let output = bob_command()
        .arg("capture-rewrite")
        .arg("--help")
        .output()
        .expect("run bob capture-rewrite --help");

    assert_success(&output);
    let help = stdout(&output);
    assert!(
        help.contains("purely lexical and completely read-only")
            && help.contains("absorb")
            && help.contains("no-op"),
        "expected capture-rewrite long help:\n{help}"
    );
    assert_text_order(&help, &["-c, --cursor", "-f, --format", "-h, --help"]);
    assert_stdout_has_no_ansi(&output);
}

#[test]
fn dataview_help_lists_options_alphabetically() {
    let output = bob_command()
        .arg("query")
        .arg("--help")
        .output()
        .expect("run bob query --help");

    assert_success(&output);
    let help = stdout(&output);
    assert_text_order(
        &help,
        &[
            "-b, --bob-dir ",
            "-e, --engine ",
            "-f, --format ",
            "-h, --help",
            "-o, --origin ",
            "-q, --query ",
            "-Q, --query-file ",
            "-s, --source ",
            "-S, --strict-paths",
            "-t, --tasks ",
            "-T, --tasks-file ",
            "-n, --tasks-note ",
            "-v, --vault ",
        ],
    );
    assert!(
        help.contains(
            "Run Dataview source expressions, Dataview DQL, or Obsidian Tasks"
        ) && help.contains("whole-note block execution")
            && help
                .contains("bob query --tasks-note dash.md --format markdown")
            && help.contains("Run every Tasks code block in a vault note"),
        "expected complete Tasks query help:\n{help}"
    );
    assert_stdout_has_no_ansi(&output);
}

#[test]
fn projects_help_lists_subcommands_and_options() {
    let output = bob_command()
        .arg("projects")
        .arg("--help")
        .output()
        .expect("run bob projects --help");

    assert_success(&output);
    let help = stdout(&output);
    assert!(
        help.contains("Manage Bob project notes")
            && help.contains("scheduled: YYYY-MM-DD")
            && help.contains("\n  list ")
            && help.contains("\n  sync "),
        "expected projects help to list subcommands:\n{help}"
    );
    assert_text_order(&help, &["\n  list ", "\n  sync "]);
    assert_stdout_has_no_ansi(&output);

    let output = bob_command()
        .arg("projects")
        .arg("list")
        .arg("--help")
        .output()
        .expect("run bob projects list --help");

    assert_success(&output);
    let help = stdout(&output);
    assert!(
        help.contains("-b, --bob-dir"),
        "expected bob-dir short and long option in list help:\n{help}"
    );
    assert_stdout_has_no_ansi(&output);

    let output = bob_command()
        .arg("projects")
        .arg("sync")
        .arg("--help")
        .output()
        .expect("run bob projects sync --help");

    assert_success(&output);
    let help = stdout(&output);
    assert!(
        help.contains("-b, --bob-dir")
            && help.contains("-d, --dry-run")
            && help.contains("Future dates")
            && help.contains("[scheduled:: YYYY-MM-DD]")
            && help.contains("bob task reconcile")
            && help.contains("Invalid dates"),
        "expected sync short and long options:\n{help}"
    );
    assert_text_order(&help, &["-b, --bob-dir", "-d, --dry-run"]);
    assert_stdout_has_no_ansi(&output);
}

#[test]
fn plugins_help_lists_subcommand_and_options() {
    let output = bob_command()
        .arg("plugins")
        .arg("--help")
        .output()
        .expect("run bob plugins --help");

    assert_success(&output);
    let help = stdout(&output);
    assert!(
        help.contains("Manage Bryan's custom Bob Obsidian plugins")
            && help.contains("\n  list ")
            && help.contains("-n, --no-pull"),
        "expected plugins help to describe the list subcommand:\n{help}"
    );
    assert_text_order(
        &help,
        &[
            "-b, --bob-dir",
            "-f, --format",
            "-n, --no-pull",
            "-r, --repo",
        ],
    );
    assert_stdout_has_no_ansi(&output);

    let output = bob_command()
        .arg("plugins")
        .arg("list")
        .arg("--help")
        .output()
        .expect("run bob plugins list --help");

    assert_success(&output);
    let help = stdout(&output);
    assert!(
        help.contains("-b, --bob-dir")
            && help.contains("-f, --format")
            && help.contains("-n, --no-pull")
            && help.contains("-r, --repo"),
        "expected list short and long options:\n{help}"
    );
    assert_text_order(
        &help,
        &[
            "-b, --bob-dir",
            "-f, --format",
            "-n, --no-pull",
            "-r, --repo",
        ],
    );
    assert_stdout_has_no_ansi(&output);
}

#[test]
fn plugins_sync_help_lists_options_alphabetically() {
    let output = bob_command()
        .arg("plugins")
        .arg("sync")
        .arg("--help")
        .output()
        .expect("run bob plugins sync --help");

    assert_success(&output);
    let help = stdout(&output);
    assert!(
        help.contains("Deploy Bob plugins"),
        "expected a sync description:\n{help}"
    );
    assert!(
        help.contains("-B, --backup-dir")
            && help.contains("-b, --bob-dir")
            && help.contains("-d, --dry-run")
            && help.contains("-F, --force")
            && help.contains("-f, --format")
            && help.contains("-n, --no-pull")
            && help.contains("-p, --plugin")
            && help.contains("-r, --repo"),
        "expected sync short and long options:\n{help}"
    );
    assert_text_order(
        &help,
        &[
            "-B, --backup-dir",
            "-b, --bob-dir",
            "-d, --dry-run",
            "-F, --force",
            "-f, --format",
            "-n, --no-pull",
            "-p, --plugin",
            "-r, --repo",
        ],
    );
    assert_stdout_has_no_ansi(&output);
}

#[test]
fn highlights_ref_help_lists_subcommands_alphabetically() {
    let output = bob_command()
        .arg("highlights")
        .arg("--help")
        .output()
        .expect("run bob highlights --help");

    assert_success(&output);
    let help = stdout(&output);
    assert!(
        !help.contains("\n  clip "),
        "clip must not be listed as a subcommand:\n{help}"
    );
    assert_text_order(
        &help,
        &[
            "\n  create ",
            "\n  doctor ",
            "\n  marker ",
            "\n  scan ",
            "\n  sync ",
        ],
    );
    assert_stdout_has_no_ansi(&output);
}

#[test]
fn highlights_create_help_lists_options_alphabetically() {
    let output = bob_command()
        .arg("highlights")
        .arg("create")
        .arg("--help")
        .output()
        .expect("run bob highlights create --help");

    assert_success(&output);
    let help = stdout(&output);
    assert!(
        help.contains("Arguments:") && help.contains("<TARGET>"),
        "expected TARGET positional argument in Arguments section:\n{help}"
    );
    assert_text_order(
        &help,
        &[
            "-A, --author",
            "-a, --audio",
            "-b, --bob-dir",
            "-d, --dry-run",
            "-f, --force",
            "-H, --html",
            "-i, --include-id",
            "-L, --listen",
            "-l, --lib-dir",
            "-N, --name",
            "-n, --no-audio",
            "-o, --output",
            "-P, --parent",
            "-p, --published",
            "-r, --ref-dir",
            "-s, --status",
            "-T, --title",
            "-t, --ref-type",
            "-x, --xlib-dir",
        ],
    );
    assert!(
        !help.contains("--research-root"),
        "obsolete research-root option must not be advertised:\n{help}"
    );
    assert!(
        help.contains("Complete path for the generated PDF")
            && help.contains("`--output` cannot be combined with `--ref-type`"),
        "expected --output contract in help:\n{help}"
    );
    assert_stdout_has_no_ansi(&output);
}

#[test]
fn highlights_clip_alias_help_matches_create_help() {
    let create = bob_command()
        .arg("highlights")
        .arg("create")
        .arg("--help")
        .output()
        .expect("run bob highlights create --help");
    let clip = bob_command()
        .arg("highlights")
        .arg("clip")
        .arg("--help")
        .output()
        .expect("run bob highlights clip --help");

    assert_success(&create);
    assert_success(&clip);
    assert_eq!(
        stdout(&clip),
        stdout(&create),
        "clip alias --help must be byte-identical to create --help"
    );
    assert!(
        !stdout(&clip).contains("bob ref clip")
            && !stdout(&clip).contains("\n  clip "),
        "alias help must print only the canonical path:\n{}",
        stdout(&clip)
    );
}

#[test]
fn highlights_ref_sync_help_lists_options_alphabetically() {
    let output = bob_command()
        .arg("highlights")
        .arg("sync")
        .arg("--help")
        .output()
        .expect("run bob highlights sync --help");

    assert_success(&output);
    let help = stdout(&output);
    assert!(
        help.contains("Arguments:") && help.contains("<PDF>"),
        "expected PDF positional argument in Arguments section:\n{help}"
    );
    assert_text_order(
        &help,
        &[
            "-b, --bob-dir",
            "-d, --dry-run",
            "-l, --lib-dir",
            "-p, --prefer",
            "-r, --ref-dir",
            "-w, --write-pdf",
            "-x, --xlib-dir",
        ],
    );
    assert_stdout_has_no_ansi(&output);
}

#[test]
fn highlights_ref_scan_help_lists_options_alphabetically() {
    let output = bob_command()
        .arg("highlights")
        .arg("scan")
        .arg("--help")
        .output()
        .expect("run bob ref scan --help");

    assert_success(&output);
    let help = stdout(&output);
    assert!(
        help.contains("-w, --write-pdfs"),
        "expected short and long write-pdfs flag in scan help:\n{help}"
    );
    assert_text_order(
        &help,
        &[
            "-b, --bob-dir",
            "-d, --dry-run",
            "-j, --jobs",
            "-l, --lib-dir",
            "-r, --ref-dir",
            "-v, --verbose",
            "-w, --write-pdfs",
            "-x, --xlib-dir",
        ],
    );
    assert_stdout_has_no_ansi(&output);
}

#[test]
fn completion_help_lists_subcommands_and_options_alphabetically() {
    let output = bob_command()
        .arg("completion")
        .arg("--help")
        .output()
        .expect("run bob completion --help");

    assert_success(&output);
    let help = stdout(&output);
    assert_text_order(
        &help,
        &[
            "install    Install or refresh",
            "status     Show installed adapters",
            "uninstall  Remove completion adapters",
            "zsh        Print the zsh completion adapter",
        ],
    );
    assert_text_order(&help, &["-j, --json", "-v, --verify", "-h, --help"]);

    let install = bob_command()
        .args(["completion", "install", "--help"])
        .output()
        .expect("run bob completion install --help");

    assert_success(&install);
    assert_text_order(
        &stdout(&install),
        &[
            "-d, --dry-run",
            "-f, --force",
            "-n, --no-verify",
            "-q, --quiet",
            "-t, --target",
            "-h, --help",
        ],
    );
    assert_stdout_has_no_ansi(&install);
    assert_stdout_has_no_ansi(&output);
}

#[test]
fn ready_help_lists_options_alphabetically() {
    let output = bob_command()
        .arg("ready")
        .arg("--help")
        .output()
        .expect("run bob ready --help");

    assert_success(&output);
    let help = stdout(&output);
    assert!(
        help.contains("per-note cap") && help.contains("[NOTE]"),
        "expected ready long help:\n{help}"
    );
    assert_text_order(
        &help,
        &[
            "-a, --all",
            "-b, --bob-dir",
            "-n, --cap",
            "-c, --check",
            "-f, --format",
            "-h, --help",
        ],
    );
    assert_stdout_has_no_ansi(&output);
}
