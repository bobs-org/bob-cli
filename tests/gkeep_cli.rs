//! `bob gkeep` CLI contract: help surface, option order, and stubs.

mod gkeep_support;

use gkeep_support::{stderr, stdout, GkeepEnv};

fn assert_no_long_only_option_lines(label: &str, help: &str) {
    for line in help.lines() {
        let trimmed = line.trim_start();
        let starts_with_long_option = trimmed
            .strip_prefix("--")
            .and_then(|tail| tail.chars().next())
            .is_some_and(|first| first.is_ascii_alphabetic());
        if starts_with_long_option {
            panic!(
                "{label} exposes a long-only option line:\n{line}\n\n{help}"
            );
        }
    }
}

fn assert_text_order(text: &str, needles: &[&str]) {
    let mut last = 0;
    for needle in needles {
        let position = text
            .find(needle)
            .unwrap_or_else(|| panic!("expected `{needle}` in text:\n{text}"));
        assert!(position >= last, "`{needle}` is out of order:\n{text}");
        last = position;
    }
}

#[test]
fn top_help_pins_the_command_surface() {
    let env = GkeepEnv::new("bob-cli-gkeep-top-help");
    let output = env
        .command()
        .arg("gkeep")
        .arg("--help")
        .output()
        .expect("run bob gkeep --help");

    assert!(
        output.status.success(),
        "expected success:\nstatus: {}\nstdout:\n{}\nstderr:\n{}",
        output.status,
        stdout(&output),
        stderr(&output)
    );
    let help = stdout(&output);
    assert!(
        help.contains("Drain your Google Keep inbox into Obsidian tasks")
            || help.contains("Google Keep → Obsidian inbox drain"),
        "expected about:\n{help}"
    );
    for (subcommand, about) in [
        ("doctor", "Check the Keep setup"),
        ("list", "side by side"),
        ("login", "stored Keep master token"),
        ("pull", "then archive them"),
    ] {
        assert!(
            help.contains(subcommand) && help.contains(about),
            "expected `{subcommand}` ({about}) in top help:\n{help}"
        );
    }
    assert!(
        help.contains(
            "Running `bob gkeep` with no command runs `bob gkeep list`"
        ),
        "expected default-command note:\n{help}"
    );
    for example in [
        "bob gkeep list -s vault",
        "bob gkeep pull -d",
        "bob gkeep pull -n",
        "bob gkeep pull -i 3f9c2e1",
        "bob gkeep doctor",
        "bob gkeep login",
    ] {
        assert!(
            help.contains(example),
            "expected example `{example}`:\n{help}"
        );
    }
    assert!(
        !output.stdout.contains(&0x1bu8),
        "help must be plain when piped:\n{help}"
    );
    assert_no_long_only_option_lines("bob gkeep --help", &help);
}

#[test]
fn top_level_options_match_list_and_default_to_list() {
    let env = GkeepEnv::new("bob-cli-gkeep-top-options");
    let output = env
        .command()
        .arg("gkeep")
        .arg("--help")
        .output()
        .expect("run bob gkeep --help");

    assert_text_order(
        &stdout(&output),
        &[
            "-a, --all",
            "-b, --bob-dir",
            "-f, --format",
            "-h, --help",
            "-s, --source",
        ],
    );

    // No subcommand defaults to `list`: top-level flags reach it, and
    // `-s vault` needs no Keep setup, so it succeeds on an empty vault.
    let output = env
        .command()
        .arg("gkeep")
        .arg("-s")
        .arg("vault")
        .output()
        .expect("run bob gkeep -s vault");
    assert_eq!(output.status.code(), Some(0));
    assert!(
        stdout(&output).contains("gkeep_inbox.md"),
        "top-level flags must reach list:\nstatus: {}\nstdout:\n{}\nstderr:\n{}",
        output.status,
        stdout(&output),
        stderr(&output)
    );
}

#[test]
fn subcommand_help_blocks_carry_examples_and_environment() {
    let env = GkeepEnv::new("bob-cli-gkeep-sub-help");
    let cases: &[(&[&str], &[&str])] = &[
        (
            &["gkeep", "doctor", "--help"],
            &[
                "Check the Keep setup",
                "bob gkeep doctor",
                "BOB_DIR",
                "BOB_CONFIG_FILE",
                "BOB_GKEEP_ADAPTER",
            ],
        ),
        (
            &["gkeep", "list", "--help"],
            &[
                "side by side",
                "bob gkeep list -s vault",
                "BOB_DIR",
                "BOB_CONFIG_FILE",
                "BOB_GKEEP_ADAPTER",
            ],
        ),
        (
            &["gkeep", "login", "--help"],
            &[
                "stored Keep master token",
                "bob gkeep login",
                "BOB_DIR",
                "BOB_CONFIG_FILE",
                "BOB_GKEEP_ADAPTER",
            ],
        ),
        (
            &["gkeep", "pull", "--help"],
            &[
                "then archive them",
                "Nothing is ever deleted from Keep.",
                "bob gkeep pull -i 3f9c2e1",
                "BOB_DIR",
                "BOB_CONFIG_FILE",
                "BOB_GKEEP_ADAPTER",
            ],
        ),
    ];

    for (args, markers) in cases {
        let output = env
            .command()
            .args(*args)
            .output()
            .unwrap_or_else(|error| panic!("run bob {args:?}: {error}"));

        assert!(
            output.status.success(),
            "expected success for {args:?}:\nstatus: {}\nstdout:\n{}\nstderr:\n{}",
            output.status,
            stdout(&output),
            stderr(&output)
        );
        let help = stdout(&output);
        for marker in *markers {
            assert!(
                help.contains(marker),
                "expected `{marker}` in help for {args:?}:\n{help}"
            );
        }
        assert!(
            !output.stdout.contains(&0x1bu8),
            "help must be plain when piped for {args:?}:\n{help}"
        );
        assert_no_long_only_option_lines(
            &format!("bob {}", args.join(" ")),
            &help,
        );
    }
}

#[test]
fn subcommand_options_are_alphabetical() {
    let env = GkeepEnv::new("bob-cli-gkeep-option-order");
    let cases: &[(&[&str], &[&str])] = &[
        (
            &["gkeep", "doctor", "--help"],
            &["-b, --bob-dir", "-f, --format", "-h, --help"],
        ),
        (
            &["gkeep", "list", "--help"],
            &[
                "-a, --all",
                "-b, --bob-dir",
                "-f, --format",
                "-h, --help",
                "-s, --source",
            ],
        ),
        (
            &["gkeep", "login", "--help"],
            &["-e, --email", "-h, --help"],
        ),
        (
            &["gkeep", "pull", "--help"],
            &[
                "-b, --bob-dir",
                "-d, --dry-run",
                "-f, --format",
                "-h, --help",
                "-i, --id",
                "-p, --include-pinned",
                "-S, --include-shared",
                "-l, --limit",
                "-n, --no-archive",
                "-C, --no-commit",
                "-q, --quiet",
            ],
        ),
    ];

    for (args, order) in cases {
        let output = env
            .command()
            .args(*args)
            .output()
            .unwrap_or_else(|error| panic!("run bob {args:?}: {error}"));
        assert!(
            output.status.success(),
            "expected success for {args:?}:\nstatus: {}\nstdout:\n{}\nstderr:\n{}",
            output.status,
            stdout(&output),
            stderr(&output)
        );
        assert_text_order(&stdout(&output), order);
    }
}

#[test]
fn usage_errors_exit_2() {
    let env = GkeepEnv::new("bob-cli-gkeep-usage");
    let output = env
        .command()
        .arg("gkeep")
        .arg("list")
        .arg("--unknown-option")
        .output()
        .expect("run bob gkeep list --unknown-option");
    assert_eq!(output.status.code(), Some(2));

    let output = env
        .command()
        .arg("gkeep")
        .arg("pull")
        .arg("--format")
        .arg("yaml")
        .output()
        .expect("run bob gkeep pull --format yaml");
    assert_eq!(output.status.code(), Some(2));
}
