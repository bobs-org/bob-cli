//! `bob gcal` CLI contract: help surface, option order, and stubs.

mod gcal_support;

use gcal_support::{stderr, stdout, FakeGog, GcalEnv};

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
    let env = GcalEnv::new("bob-cli-gcal-top-help");
    let output = env
        .command()
        .args(["gcal", "--help"])
        .output()
        .expect("run bob gcal --help");
    assert!(
        output.status.success(),
        "expected success:\n{}",
        stderr(&output)
    );
    let help = stdout(&output);
    assert!(
        help.contains("Google Calendar") && help.contains("daily-note"),
        "expected about:\n{help}"
    );
    for (subcommand, about) in [
        ("calendars", "calendars with id"),
        ("doctor", "Check the Calendar setup"),
        ("list", "List a local day of events"),
        ("login", "read-only Google Calendar consent"),
        ("logout", "Remove Bob's Calendar grant"),
    ] {
        assert!(
            help.contains(subcommand),
            "expected `{subcommand}` in top help:\n{help}"
        );
        assert!(
            help.to_ascii_lowercase()
                .contains(&about.to_ascii_lowercase())
                || help.contains(about),
            "expected `{subcommand}` ({about}) in top help:\n{help}"
        );
    }
    assert!(
        help.contains(
            "Running `bob gcal` with no command runs `bob gcal list`"
        ),
        "expected list-default note:\n{help}"
    );
    for key in ["BOB_DIR", "BOB_CONFIG_FILE", "BOB_GCAL_GOG"] {
        assert!(help.contains(key), "expected Environment {key}:\n{help}");
    }
    for example in [
        "bob gcal login",
        "bob gcal login -c PATH",
        "bob gcal login -m",
        "bob gcal -D tomorrow",
        "bob gcal calendars",
        "bob gcal doctor",
        "bob gcal logout -y",
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
    assert_no_long_only_option_lines("bob gcal --help", &help);
}

#[test]
fn bare_gcal_lists_today_and_exits_0() {
    let env = GcalEnv::new("bob-cli-gcal-bare");
    let fake = FakeGog::new(&env, "fake-gog");
    let mut command = env.command();
    let output = fake
        .install(&mut command)
        .arg("gcal")
        .output()
        .expect("run bob gcal");
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let listing = stdout(&output);
    assert!(
        listing.contains("Google Calendar") && listing.contains("TIME"),
        "bare gcal should list today's events:\n{listing}"
    );
}

#[test]
fn unknown_subcommand_exits_2() {
    let env = GcalEnv::new("bob-cli-gcal-unknown");
    let output = env
        .command()
        .args(["gcal", "not-a-command"])
        .output()
        .expect("run bob gcal list");
    assert_eq!(output.status.code(), Some(2), "{}", stderr(&output));
}

#[test]
fn list_help_has_sorted_short_options_and_day_examples() {
    let env = GcalEnv::new("bob-cli-gcal-list-help");
    let output = env
        .command()
        .args(["gcal", "list", "--help"])
        .output()
        .expect("list help");
    assert!(output.status.success(), "{}", stderr(&output));
    let help = stdout(&output);
    assert_text_order(
        &help,
        &[
            "-a, --all",
            "-b, --bob-dir",
            "-D, --day",
            "-f, --format",
            "-h, --help",
        ],
    );
    for needle in ["bob gcal list -D tomorrow", "YYYY-MM-DD", "BOB_NOW"] {
        assert!(help.contains(needle), "missing `{needle}`:\n{help}");
    }
    assert_no_long_only_option_lines("bob gcal list --help", &help);
}

#[test]
fn login_help_lists_short_aliases_in_order() {
    let env = GcalEnv::new("bob-cli-gcal-login-help");
    let output = env
        .command()
        .args(["gcal", "login", "--help"])
        .output()
        .expect("login help");
    assert!(output.status.success(), "{}", stderr(&output));
    let help = stdout(&output);
    assert_text_order(
        &help,
        &[
            "-a, --account",
            "-c, --credentials",
            "-f, --format",
            "-h, --help",
            "-m, --manual",
        ],
    );
    assert_no_long_only_option_lines("bob gcal login --help", &help);
}

#[test]
fn logout_and_doctor_help_carry_examples() {
    let env = GcalEnv::new("bob-cli-gcal-sub-help");
    for (args, needles) in [
        (
            &["gcal", "logout", "--help"] as &[&str],
            &[
                "Remove Bob's Calendar grant",
                "bob gcal logout -y",
                "https://myaccount.google.com/permissions",
                "BOB_GCAL_GOG",
            ] as &[&str],
        ),
        (
            &["gcal", "doctor", "--help"],
            &["Check the Calendar setup", "bob gcal doctor", "BOB_DIR"],
        ),
        (
            &["gcal", "calendars", "--help"],
            &["bob gcal calendars", "-f json", "BOB_GCAL_GOG"],
        ),
    ] {
        let output =
            env.command().args(args).output().unwrap_or_else(|error| {
                panic!("run {}: {error}", args.join(" "))
            });
        assert!(
            output.status.success(),
            "{} failed: {}",
            args.join(" "),
            stderr(&output)
        );
        let help = stdout(&output);
        for needle in needles {
            assert!(
                help.contains(needle),
                "{} missing `{needle}`:\n{help}",
                args.join(" ")
            );
        }
        assert_no_long_only_option_lines(&args.join(" "), &help);
    }
}
