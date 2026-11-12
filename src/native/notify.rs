use std::{
    ffi::OsString,
    io::{self, Write},
    process::{Command, Stdio},
    thread,
    time::Duration,
};

use clap::{Arg, ArgAction, Command as ClapCommand};

use super::{env as bob_env, pomodoro_status};

const COMMAND_NAME: &str = "bob pomodoro notify";
const MAX_SLEEP_SECONDS: f64 = 31_536_000.0;

#[derive(Debug)]
struct Args {
    verbose: u8,
    pre_check_sleep: String,
    post_notify_sleep: String,
}

pub(crate) fn run(args: Vec<OsString>) -> i32 {
    let args = match parse_args(args) {
        ParseResult::Run(args) => args,
        ParseResult::Help => {
            print_help();
            return 0;
        }
        ParseResult::Error(message) => {
            eprintln!("{COMMAND_NAME}: error: {message}");
            return 2;
        }
    };

    if args.verbose > 1 {
        eprintln!("{COMMAND_NAME}: debug: verbose tracing requested");
    }

    run_loop(&args)
}

fn run_loop(args: &Args) -> i32 {
    let mut loop_count = 0;
    loop {
        loop_count += 1;
        info(format_args!(
            "Starting Loop: Every {} seconds, I will check if the current pomodoro is done.",
            args.pre_check_sleep
        ));

        loop {
            if let Err(error) = sleep_arg(&args.pre_check_sleep) {
                eprintln!("{COMMAND_NAME}: error: sleep failed: {error}");
                return 1;
            }

            match pomodoro_status() {
                Ok(Some(output)) if pomodoro_is_done(&output) => break,
                Ok(_) => loop_count = 1,
                Err(_) => loop_count = 1,
            }
        }

        info(format_args!(
            "Sending notifications since pomodoro is done!"
        ));
        notify_me(loop_count, persist_notification(loop_count));

        info(format_args!(
            "Sleeping for {} seconds before restarting check loop.",
            args.post_notify_sleep
        ));
        if let Err(error) = sleep_arg(&args.post_notify_sleep) {
            eprintln!("{COMMAND_NAME}: error: sleep failed: {error}");
            return 1;
        }
    }
}

fn parse_args(args: Vec<OsString>) -> ParseResult {
    let mut verbose = 0;
    let mut positionals = Vec::new();
    let mut positional_only = false;

    for arg in args {
        if !positional_only {
            let text = bob_env::os_to_string(&arg);
            match text.as_str() {
                "-h" | "--help" => return ParseResult::Help,
                "-v" | "--verbose" => {
                    verbose += 1;
                    continue;
                }
                "--" => {
                    positional_only = true;
                    continue;
                }
                _ if text.starts_with("-vv")
                    && text.chars().all(|char| char == '-' || char == 'v') =>
                {
                    verbose +=
                        text.chars().filter(|char| *char == 'v').count() as u8;
                    continue;
                }
                _ => {}
            }
        }

        positionals.push(bob_env::os_to_string(&arg));
    }

    if positionals.len() != 2 {
        return ParseResult::Error(usage());
    }

    if positionals.iter().any(|value| {
        value.parse::<f64>().is_ok_and(|seconds| {
            seconds.is_finite() && seconds >= 0.0 && seconds > MAX_SLEEP_SECONDS
        })
    }) {
        return ParseResult::Error(format!(
            "sleep values must be at most {MAX_SLEEP_SECONDS:.0} seconds"
        ));
    }

    ParseResult::Run(Args {
        verbose,
        pre_check_sleep: positionals.remove(0),
        post_notify_sleep: positionals.remove(0),
    })
}

enum ParseResult {
    Run(Args),
    Help,
    Error(String),
}

pub(crate) fn completion_descriptor() -> ClapCommand {
    ClapCommand::new(COMMAND_NAME)
        .about("Notify when the current Pomodoro is complete")
        .disable_help_flag(true)
        .arg(
            Arg::new("verbose")
                .long("verbose")
                .short('v')
                .action(ArgAction::Count)
                .help(
                    "Enable verbose output. This option can be specified multiple times (e.g. -v, -vv, ...).",
                ),
        )
        .arg(
            Arg::new("PRE_CHECK_SLEEP")
                .value_name("PRE_CHECK_SLEEP")
                .required(true)
                .help(
                    "The number of seconds to wait between Pomodoro status checks.",
                ),
        )
        .arg(
            Arg::new("POST_NOTIFY_SLEEP")
                .value_name("POST_NOTIFY_SLEEP")
                .required(true)
                .help(
                    "The number of seconds to wait after a notification.",
                ),
        )
        .arg(
            Arg::new("help")
                .long("help")
                .short('h')
                .action(ArgAction::Help)
                .help("View this help message."),
        )
}

pub(crate) fn help_text() -> String {
    format!(
        "\
{usage}

Notify me when the current Bob pomodoro is complete.

Positional Arguments:
---------------------
PRE_CHECK_SLEEP
    the number of seconds to wait between Pomodoro status checks.

POST_NOTIFY_SLEEP
    the number of seconds to wait after a notification.

Optional Arguments
------------------
-h | --help
    View this help message.

-v | --verbose
    Enable verbose output. This option can be specified multiple times (e.g. -v, -vv, ...).",
        usage = usage()
    )
}

fn print_help() {
    println!("{}", help_text());
}

fn usage() -> String {
    format!(
        "usage: {COMMAND_NAME} [-v] PRE_CHECK_SLEEP POST_NOTIFY_SLEEP\n       {COMMAND_NAME} -h"
    )
}

fn pomodoro_is_done(output: &str) -> bool {
    output.starts_with("[OVERDUE")
}

fn persist_notification(count: i32) -> bool {
    count >= 10
}

fn notify_send_args(count: i32, no_timeout: bool) -> Vec<String> {
    let mut args = Vec::new();
    if no_timeout {
        args.push("-t".to_string());
        args.push("0".to_string());
    }
    args.push("-u".to_string());
    args.push("critical".to_string());
    args.push("bob".to_string());
    args.push(format!(" Pomodoro is done! (#{count})"));
    args
}

fn notify_me(count: i32, no_timeout: bool) {
    if command_available("notify-send") {
        let _ = Command::new("notify-send")
            .args(notify_send_args(count, no_timeout))
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }

    for _ in 0..3 {
        println!("\x07");
        let _ = io::stdout().flush();
        thread::sleep(Duration::from_secs(1));
    }
}

fn sleep_arg(value: &str) -> io::Result<()> {
    if let Ok(seconds) = value.parse::<f64>()
        && seconds.is_finite()
        && seconds >= 0.0
        && seconds <= MAX_SLEEP_SECONDS
    {
        thread::sleep(Duration::from_secs_f64(seconds));
        return Ok(());
    }

    let status = Command::new("sleep").arg(value).status()?;
    if status.success() {
        Ok(())
    } else {
        Err(io::Error::other(format!(
            "sleep exited with status {}",
            bob_env::exit_code(status)
        )))
    }
}

fn command_available(command: &str) -> bool {
    let Some(path) = bob_env::var_os("PATH") else {
        return false;
    };

    std::env::split_paths(&path).any(|dir| {
        let path = dir.join(command);
        path.is_file()
    })
}

fn info(args: std::fmt::Arguments<'_>) {
    eprintln!("{COMMAND_NAME}: info: {args}");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_sleep_values_above_one_year() {
        match parse_args(vec!["1e20".into(), "300".into()]) {
            ParseResult::Error(message) => {
                assert!(message.contains("at most 31536000 seconds"));
            }
            _ => panic!("oversized sleep value should be a usage error"),
        }
    }

    #[test]
    fn overdue_status_matches_the_line_prefix() {
        assert!(pomodoro_is_done(
            "[OVERDUE by 10m] 0945-1015 Review crate skeleton\n"
        ));
        assert!(!pomodoro_is_done("0945-1015 Chase OVERDUE invoices\n"));
        assert!(!pomodoro_is_done("[<5m] Chase OVERDUE invoices\n"));
    }

    #[test]
    fn tenth_and_later_overdue_alerts_stay_on_screen() {
        assert!(!persist_notification(9));
        assert!(persist_notification(10));
        assert!(persist_notification(11));
    }

    #[test]
    fn persistent_notify_send_uses_no_timeout() {
        assert_eq!(
            notify_send_args(10, true),
            [
                "-t",
                "0",
                "-u",
                "critical",
                "bob",
                " Pomodoro is done! (#10)"
            ]
        );
        assert_eq!(
            notify_send_args(3, false),
            ["-u", "critical", "bob", " Pomodoro is done! (#3)"]
        );
    }
}
