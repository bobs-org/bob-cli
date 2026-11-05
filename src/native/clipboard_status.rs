//! Read-only diagnostics for counted clipboard capture.

use std::{ffi::OsString, iter, time::SystemTime};

use clap::{Arg, Command as ClapCommand};
use serde_json::{json, Value};

use super::{
    capture_clip::{
        clipboard_status, ClipboardProviderStatus, ClipboardStatusReport,
    },
    style::Styler,
};

const COMMAND_NAME: &str = "bob clipboard";
const SCHEMA_VERSION: u32 = 1;

pub(crate) fn run(args: Vec<OsString>) -> i32 {
    let mut command = build_cli();
    let matches = match command.try_get_matches_from_mut(
        iter::once(OsString::from(COMMAND_NAME)).chain(args),
    ) {
        Ok(matches) => matches,
        Err(error) => return print_clap_error(error),
    };

    let count = *matches
        .get_one::<usize>("count")
        .expect("count has a default");
    if count < 2 {
        return print_clap_error(command.error(
            clap::error::ErrorKind::ValueValidation,
            "--count must be at least 2",
        ));
    }
    let report = clipboard_status(count);
    let would_succeed = report.result.is_ok();
    match matches.get_one::<String>("format").map(String::as_str) {
        Some("json") => println!("{}", to_json(&report)),
        _ => print_human(&report),
    }
    if would_succeed {
        0
    } else {
        1
    }
}

pub(crate) fn build_cli() -> ClapCommand {
    ClapCommand::new(COMMAND_NAME)
        .about("Diagnose clipboard history for counted capture")
        .long_about(
            "Read the live clipboard and check whether `%c<N>` would succeed. \
            The report includes provider health and selection details, but \
            never prints clipboard or history contents. This command is \
            read-only.",
        )
        .after_help(
            "Examples:\n  bob clipboard\n  bob clipboard -c 5\n  bob clipboard -f json\n\nExit codes:\n  0  the counted read would succeed\n  1  the counted read would refuse\n  2  invalid command-line options",
        )
        .arg(
            Arg::new("count")
                .long("count")
                .short('c')
                .value_name("N")
                .value_parser(clap::value_parser!(usize))
                .default_value("3")
                .help("Counted clipboard values to check; minimum 2"),
        )
        .arg(
            Arg::new("format")
                .long("format")
                .short('f')
                .value_name("FORMAT")
                .value_parser(["human", "json"])
                .default_value("human")
                .help("Output format: human or json"),
        )
}

fn print_clap_error(error: clap::Error) -> i32 {
    let exit_code = error.exit_code();
    if let Err(print_error) = error.print()
        && print_error.kind() != std::io::ErrorKind::BrokenPipe
    {
        eprintln!(
            "{COMMAND_NAME}: failed to print command-line error: {print_error}"
        );
    }
    exit_code
}

fn to_json(report: &ClipboardStatusReport) -> Value {
    let (would_succeed, selected_provider, history, refusal) =
        match &report.result {
            Ok(history) => {
                let entries = history
                .entries
                .iter()
                .map(|entry| {
                    json!({
                        "age_seconds": entry.at.and_then(|at| age_seconds(at)),
                        "app": entry.app.as_deref(),
                    })
                })
                .collect::<Vec<_>>();
                (
                    true,
                    Some(history.provider),
                    Some(json!({
                        "provider": history.provider,
                        "entries": entries,
                    })),
                    Value::Null,
                )
            }
            Err(error) => (
                false,
                None,
                None,
                json!({
                    "code": error.code.unwrap_or("clipboard_live_unavailable"),
                    "message": safe_refusal_message(error.code, report.count),
                }),
            ),
        };
    let providers = report
        .providers
        .iter()
        .map(provider_json)
        .collect::<Vec<_>>();

    json!({
        "ok": true,
        "schema_version": SCHEMA_VERSION,
        "count": report.count,
        "live": {"source": report.live_source},
        "selection": {
            "value": report.selection,
            "source": report.selection_source,
            "config_path": report.config_path,
        },
        "providers": providers,
        "would_succeed": would_succeed,
        "selected_provider": selected_provider,
        "history": history,
        "refusal": refusal,
    })
}

fn provider_json(provider: &ClipboardProviderStatus) -> Value {
    json!({
        "provider": provider.name,
        "selected": provider.selected,
        "store_path": provider.store_path,
        "exists": provider.exists,
        "schema_ok": provider.schema_ok,
        "running": provider.running,
        "newest_entry_age_seconds": provider.newest_entry_age_seconds,
        "head_agrees_with_live": provider.head_agrees_with_live,
        "error": provider.error,
    })
}

fn print_human(report: &ClipboardStatusReport) {
    let style = Styler::detect();
    println!("{}", style.bold("Clipboard history status"));
    println!("  live       {}", report.live_source);
    let source = match report.selection_source.as_str() {
        "environment" => "BOB_CLIPBOARD_HISTORY_CMD".to_string(),
        "config" => report
            .config_path
            .as_deref()
            .unwrap_or("config file")
            .to_string(),
        _ => "default".to_string(),
    };
    println!("  selection  {} · from {source}", report.selection);
    for provider in &report.providers {
        println!("  {}", format_provider(provider, &style));
    }

    match &report.result {
        Ok(history) => {
            let newest = history.entries.first().and_then(|entry| entry.at);
            let oldest = history.entries.last().and_then(|entry| entry.at);
            let ages = match (newest, oldest) {
                (Some(newest), Some(oldest)) => format!(
                    " · newest {} · oldest {}",
                    format_age(age_duration(newest)),
                    format_age(age_duration(oldest))
                ),
                _ => " · ages unavailable".to_string(),
            };
            println!(
                "{}",
                style.green(&format!(
                    "%c{} ok via {}{ages}",
                    report.count, history.provider
                ))
            );
        }
        Err(error) => println!(
            "{}",
            style.red(&format!(
                "%c{} would refuse [{}]: {}",
                report.count,
                error.code.unwrap_or("clipboard_live_unavailable"),
                safe_refusal_message(error.code, report.count)
            ))
        ),
    }
}

fn format_provider(
    provider: &ClipboardProviderStatus,
    style: &Styler,
) -> String {
    let name = if provider.selected {
        style.cyan(provider.name)
    } else {
        provider.name.to_string()
    };
    let path = provider.store_path.as_deref().unwrap_or("not applicable");
    let age = provider
        .newest_entry_age_seconds
        .map(|seconds| format_age(std::time::Duration::from_secs(seconds)))
        .unwrap_or_else(|| "unavailable".to_string());
    let mut line = format!(
        "{name} · store {path} · exists {} · schema {} · running {} · newest {age} · head agrees {}",
        option_bool(provider.exists),
        option_bool(provider.schema_ok),
        option_bool(provider.running),
        if provider.head_agrees_with_live { "yes" } else { "no" },
    );
    if let Some(error) = &provider.error {
        line.push_str(" · ");
        line.push_str(error);
    }
    line
}

fn option_bool(value: Option<bool>) -> &'static str {
    match value {
        Some(true) => "yes",
        Some(false) => "no",
        None => "n/a",
    }
}

fn safe_refusal_message(code: Option<&'static str>, count: usize) -> String {
    match code {
        Some("clipboard_history_mismatch") => {
            "no history provider agreed with the live clipboard within 1.5 seconds".to_string()
        }
        Some("clipboard_history_short") => {
            format!("the selected provider has fewer than {count} entries")
        }
        Some("clipboard_history_not_text") => {
            "a requested history entry is not usable text".to_string()
        }
        Some("clipboard_history_unavailable") => {
            "the selected clipboard history provider is unavailable or disabled".to_string()
        }
        _ => "the live clipboard could not be read".to_string(),
    }
}

fn age_seconds(at: SystemTime) -> Option<u64> {
    SystemTime::now()
        .duration_since(at)
        .ok()
        .map(|age| age.as_secs())
}

fn age_duration(at: SystemTime) -> std::time::Duration {
    SystemTime::now().duration_since(at).unwrap_or_default()
}

fn format_age(age: std::time::Duration) -> String {
    let seconds = age.as_secs();
    if seconds < 60 {
        format!("{seconds} s")
    } else if seconds < 60 * 60 {
        format!("{} min", seconds / 60)
    } else if seconds < 24 * 60 * 60 {
        format!("{} h", seconds / (60 * 60))
    } else {
        format!("{} d", seconds / (24 * 60 * 60))
    }
}
