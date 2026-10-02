use std::{
    ffi::OsString,
    io::{self, Write},
    process::{Command, Stdio},
    thread,
    time::Duration,
};

use clap::{Arg, ArgAction, Command as ClapCommand};

use super::{env as bob_env, pomodoro_status};

const SCRIPT_NAME: &str = "bob_notify";

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
            eprintln!("{SCRIPT_NAME}: error: {message}");
            return 2;
        }
    };

    if args.verbose > 1 {
        eprintln!("{SCRIPT_NAME}: debug: verbose tracing requested");
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
                eprintln!("{SCRIPT_NAME}: error: sleep failed: {error}");
                return 1;
            }

            match pomodoro_status() {
                Ok(Some(output)) if output.contains("OVERDUE") => break,
                Ok(_) => loop_count = 1,
                Err(_) => loop_count = 1,
            }
        }

        info(format_args!(
            "Sending notifications since pomodoro is done!"
        ));
        notify_me(loop_count);

        info(format_args!(
            "Sleeping for {} seconds before restarting check loop.",
            args.post_notify_sleep
        ));
        if let Err(error) = sleep_arg(&args.post_notify_sleep) {
            eprintln!("{SCRIPT_NAME}: error: sleep failed: {error}");
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
    ClapCommand::new("bob notify")
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
                    "The number of seconds to wait between calls to bob_pomodoro.",
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
    the number of seconds to wait between calls to bob_pomodoro.

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
        "usage: {SCRIPT_NAME} [-v] PRE_CHECK_SLEEP POST_NOTIFY_SLEEP\n       {SCRIPT_NAME} -h"
    )
}

fn notify_me(count: i32) {
    if command_available("notify-send") {
        let message = format!(" Pomodoro is done! (#{count})");
        let _ = Command::new("notify-send")
            .arg("-u")
            .arg("critical")
            .arg("bob")
            .arg(message)
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
    let Some(path) = std::env::var_os("PATH") else {
        return false;
    };

    std::env::split_paths(&path).any(|dir| {
        let path = dir.join(command);
        path.is_file()
    })
}

fn info(args: std::fmt::Arguments<'_>) {
    eprintln!("{SCRIPT_NAME}: info: {args}");
}
