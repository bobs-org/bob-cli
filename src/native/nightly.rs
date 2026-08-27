//! `bob nightly` — the single nightly entry point.
//!
//! Runs a lock-held sequence of vault maintenance steps. Output is laid out in
//! clearly-labeled sections so the cron log always makes it obvious which step
//! is talking.

use std::{
    ffi::{OsStr, OsString},
    io::IsTerminal,
    path::Path,
};

use chrono::Local;

use super::{
    collect_done, env as bob_env,
    ob::{self, ChildEnv},
    vault_sync,
};

/// One wrapped command in the nightly sequence. Adding a future step is a
/// one-line registration here plus its `run` core function.
struct Step {
    name: &'static str,
    blurb: &'static str,
    run: fn(&ChildEnv) -> i32,
}

const STEPS: &[Step] = &[
    Step {
        name: "vault-sync",
        blurb: "Pull and reconcile the vault",
        run: run_vault_sync_step,
    },
    Step {
        name: "move-done-tasks",
        blurb: "Archive done & canceled tasks",
        run: run_collect_done_step,
    },
    Step {
        name: "vault-sync",
        blurb: "Publish maintenance changes",
        run: run_vault_sync_step,
    },
];

const RULE_WIDTH: usize = 68;

pub(crate) fn run(args: Vec<OsString>) -> i32 {
    match parse_args(args) {
        ParseResult::Run => {}
        ParseResult::Help => {
            print_help();
            return 0;
        }
        ParseResult::Error(message) => {
            eprintln!("bob nightly: {message}");
            eprintln!("Try 'bob nightly --help' for more information.");
            return 2;
        }
    }

    let vault = bob_env::bob_dir();
    let styler = Styler::detect();

    print_header(&styler, &vault);

    let _lock = match ob::acquire_lock() {
        Ok(lock) => lock,
        Err(code) => return code,
    };
    let child_env = ob::child_env();

    // Wrapped steps run in order; a failing step does not stop later steps.
    let mut results = Vec::with_capacity(STEPS.len());
    for (index, step) in STEPS.iter().enumerate() {
        print_step_header(&styler, index + 1, STEPS.len(), step);
        let code = (step.run)(&child_env);
        print_step_footer(&styler, step, code);
        results.push((step.name, code));
    }

    print_summary(&styler, &results);

    results
        .iter()
        .map(|(_, code)| *code)
        .find(|code| *code != 0)
        .unwrap_or(0)
}

fn parse_args(args: Vec<OsString>) -> ParseResult {
    if args.iter().any(|arg| {
        let value = arg.as_os_str();
        value == OsStr::new("--help") || value == OsStr::new("-h")
    }) {
        return ParseResult::Help;
    }

    if let Some(arg) = args.first() {
        return ParseResult::Error(format!(
            "unexpected argument: {}",
            arg.to_string_lossy()
        ));
    }

    ParseResult::Run
}

enum ParseResult {
    Run,
    Help,
    Error(String),
}

fn print_help() {
    println!(
        "\
usage: bob nightly

Run the nightly Bob maintenance path. The command acquires the shared lock,
runs `vault-sync`, `move-done-tasks`, and `vault-sync` in order.

workflow:
  1. acquire the shared Bob maintenance lock
  2. run vault-sync against the vault
  3. run move-done-tasks against the vault
  4. run vault-sync against the vault

environment:
  BOB_DIR                        Bob vault root; defaults to ~/bob
  BOB_NOW                        override the date used by wrapped commands
  BOB_VAULT_SYNC_LOCK_FILE       override the shared lock path
  BOB_VAULT_SYNC_STATE_FILE      override the vault-sync status JSON path
  NO_COLOR                       disable color even when stdout is a TTY

Legacy BOB_BULK_GIT_COMMIT_* and BOB_SYNC_* environment variables are no longer recognized; use BOB_VAULT_SYNC_*.

options:
  -h, --help                     show this help message and exit

No other options are accepted."
    );
}

fn run_collect_done_step(child_env: &ChildEnv) -> i32 {
    collect_done::run_collection(collect_done::DEFAULT_THRESHOLD, child_env)
}

fn run_vault_sync_step(child_env: &ChildEnv) -> i32 {
    vault_sync::run_cycle_with_existing_lock(child_env)
}

fn print_header(styler: &Styler, vault: &Path) {
    let rule = styler.cyan(&"\u{2501}".repeat(RULE_WIDTH));
    println!("{rule}");
    println!("  {} \u{b7} {}", styler.bold("bob nightly"), timestamp());
    println!(
        "  Nightly maintenance for the Bob Obsidian vault \u{2014} {}",
        vault.display()
    );
    println!("{rule}");
}

fn print_step_header(
    styler: &Styler,
    number: usize,
    total: usize,
    step: &Step,
) {
    let label = format!(
        "step {number}/{total} \u{b7} {} \u{2014} {}",
        step.name, step.blurb
    );
    println!();
    println!("{}", styler.cyan(&top_rule(&label)));
}

fn print_step_footer(styler: &Styler, step: &Step, code: i32) {
    if code == 0 {
        println!(
            "{} {} {} ok",
            styler.cyan("\u{2570}\u{2500}"),
            styler.green("\u{2713}"),
            step.name
        );
    } else {
        println!(
            "{} {} {} failed (exit {code})",
            styler.cyan("\u{2570}\u{2500}"),
            styler.red("\u{2717}"),
            step.name
        );
    }
}

fn print_summary(styler: &Styler, results: &[(&str, i32)]) {
    println!();
    println!(
        "{}",
        styler.cyan(&top_summary_rule("\u{2501}\u{2501} Summary"))
    );

    let mut failures = 0;
    for (name, code) in results {
        let ok = *code == 0;
        if !ok {
            failures += 1;
        }
        let detail = if ok {
            "ok".to_string()
        } else {
            format!("failed (exit {code})")
        };
        print_summary_row(styler, name, ok, &detail);
    }

    let footer = if failures == 0 {
        format!("All steps passed \u{b7} {}", timestamp())
    } else if failures == 1 {
        format!("1 step failed \u{b7} {}", timestamp())
    } else {
        format!("{failures} steps failed \u{b7} {}", timestamp())
    };
    println!("  {footer}");
    println!("{}", styler.cyan(&"\u{2501}".repeat(RULE_WIDTH)));
}

fn print_summary_row(styler: &Styler, name: &str, ok: bool, detail: &str) {
    let marker = if ok {
        styler.green("\u{2713}")
    } else {
        styler.red("\u{2717}")
    };
    println!("  {marker} {name:<17} {detail}");
}

/// Build a `╭─ <label> ──…──` opening rule, padded to `RULE_WIDTH`.
fn top_rule(label: &str) -> String {
    framed_rule("\u{256d}\u{2500}", label)
}

fn top_summary_rule(label: &str) -> String {
    framed_rule("", label)
}

fn framed_rule(prefix: &str, label: &str) -> String {
    let mut rule = String::new();
    rule.push_str(prefix);
    if !prefix.is_empty() {
        rule.push(' ');
    }
    rule.push_str(label);
    rule.push(' ');

    let used = display_width(prefix)
        + usize::from(!prefix.is_empty())
        + display_width(label)
        + 1;
    if used < RULE_WIDTH {
        rule.push_str(&"\u{2500}".repeat(RULE_WIDTH - used));
    }
    rule
}

/// Count characters (not bytes) so box-drawing glyphs pad correctly.
fn display_width(text: &str) -> usize {
    text.chars().count()
}

fn timestamp() -> String {
    Local::now().format("%Y-%m-%dT%H:%M:%S").to_string()
}

/// Tiny ANSI helper. Color is auto-disabled when stdout is not a TTY (cron log)
/// and when `NO_COLOR` is set, so logs stay clean while live terminals get
/// color. Structure always comes from the box-drawing glyphs and ✓/✗ markers,
/// never color alone.
struct Styler {
    color: bool,
}

impl Styler {
    fn detect() -> Self {
        let color = std::io::stdout().is_terminal()
            && std::env::var_os("NO_COLOR").is_none();
        Self { color }
    }

    fn paint(&self, code: &str, text: &str) -> String {
        if self.color {
            format!("\u{1b}[{code}m{text}\u{1b}[0m")
        } else {
            text.to_string()
        }
    }

    fn bold(&self, text: &str) -> String {
        self.paint("1", text)
    }

    fn green(&self, text: &str) -> String {
        self.paint("32", text)
    }

    fn red(&self, text: &str) -> String {
        self.paint("31", text)
    }

    fn cyan(&self, text: &str) -> String {
        self.paint("36", text)
    }
}
