use super::receipt::{Outcome, Receipt, StepReceipt};
use crate::native::{env as bob_env, style::Styler};
use chrono::{DateTime, Local, Utc};
use std::{collections::BTreeMap, time::Duration};

pub(super) fn clock(at: DateTime<Utc>) -> String {
    match bob_env::configured_timezone() {
        Some(tz) => at.with_timezone(&tz).format("%H:%M").to_string(),
        None => at.with_timezone(&Local).format("%H:%M").to_string(),
    }
}
fn stamp(at: DateTime<Utc>) -> String {
    match bob_env::configured_timezone() {
        Some(tz) => at.with_timezone(&tz).format("%a %b %d %H:%M").to_string(),
        None => at
            .with_timezone(&Local)
            .format("%a %b %d %H:%M")
            .to_string(),
    }
}
/// Treat subprocess diagnostics as text, never terminal control sequences.
pub(super) fn safe_text(text: &str) -> String {
    text.chars().filter(|c| !c.is_control()).take(240).collect()
}
fn glyph(row: &StepReceipt, styler: Styler) -> String {
    if row.would_run == Some(true) {
        return styler.cyan("▸");
    }
    match row.outcome {
        Outcome::Ok => styler.green("✓"),
        Outcome::Failed => styler.red("✗"),
        Outcome::Waiting => styler.yellow("◌"),
        Outcome::Skipped if row.cadence == "off" || row.cadence == "manual" => {
            styler.dim("–")
        }
        Outcome::Skipped => styler.dim("·"),
    }
}
pub(super) fn header(receipt: &Receipt) {
    println!(
        "bob tick · {} · {}",
        stamp(receipt.started_at),
        receipt.host
    );
}
pub(super) fn run_row(row: &StepReceipt) {
    let styler = Styler::detect();
    let duration = row
        .duration_ms
        .map(|ms| format!("{:.1}s", ms as f64 / 1000.0))
        .unwrap_or_default();
    println!(
        "  {} {:<9} {:<40} {}",
        glyph(row, styler),
        row.name,
        safe_text(&row.summary),
        duration
    );
    if row.outcome == Outcome::Failed {
        for line in &row.stderr_tail {
            println!("      {} {}", styler.dim("│"), line);
        }
    }
}
fn next(row: &StepReceipt, now: DateTime<Utc>) -> String {
    if row.cadence == "off" || row.cadence == "manual" {
        return "—".into();
    }
    match row.next_due_at {
        Some(at) if at > now => clock(at),
        _ => "due".into(),
    }
}
fn next_summary(receipt: &Receipt) -> String {
    let mut groups = BTreeMap::<Option<DateTime<Utc>>, Vec<&str>>::new();
    for row in &receipt.steps {
        if row.cadence != "off" && row.cadence != "manual" {
            groups.entry(row.next_due_at).or_default().push(&row.name);
        }
    }
    groups
        .into_iter()
        .map(|(at, names)| {
            format!(
                "{} {}",
                names.join(", "),
                at.filter(|t| *t > receipt.finished_at)
                    .map(clock)
                    .unwrap_or_else(|| "due".into())
            )
        })
        .collect::<Vec<_>>()
        .join(" · ")
}
pub(super) fn summary(
    receipt: &Receipt,
    duration: Duration,
    printed_header: bool,
) {
    let ran = receipt
        .steps
        .iter()
        .filter(|r| r.started_at.is_some())
        .count();
    let next = next_summary(receipt);
    let next = if next.is_empty() {
        String::new()
    } else {
        format!(" · next {next}")
    };
    if !printed_header
        && receipt.exit_code == 0
        && ran == 0
        && receipt.steps.iter().all(|r| r.outcome == Outcome::Skipped)
    {
        println!(
            "{}",
            Styler::detect().dim(&format!(
                "bob tick · {} · nothing due{next}",
                clock(receipt.finished_at)
            ))
        );
    } else {
        let result = if receipt.exit_code == 0 {
            "ok"
        } else {
            "failed"
        };
        println!(
            "  {result} · {ran} ran in {:.1}s{next}",
            duration.as_secs_f64()
        );
    }
}
pub(super) fn view(receipt: &Receipt, previous: Option<&Receipt>, quiet: bool) {
    if quiet {
        return;
    }
    let mode = receipt.mode.as_deref().unwrap_or("run");
    println!(
        "bob tick · {mode} · {} · {}",
        stamp(receipt.started_at),
        receipt.host
    );
    if let Some(banner) = &receipt.banner {
        println!("  {}", Styler::detect().yellow(banner));
    }
    if mode == "status" {
        if let Some(last) = previous {
            println!(
                "  last tick {} ({})",
                clock(last.finished_at),
                if last.exit_code == 0 { "ok" } else { "failed" }
            );
        }
        println!("  STEP      EVERY   LAST      RESULT                         LAST OK  NEXT");
        for row in &receipt.steps {
            println!(
                "  {:<9} {:<7} {:<5} {}   {:<30} {:<8} {}",
                row.name,
                row.cadence,
                row.last_finished_at
                    .map(clock)
                    .unwrap_or_else(|| "—".into()),
                glyph(row, Styler::detect()),
                crate::native::style::truncate(&safe_text(&row.summary), 30),
                row.last_success_at.map(clock).unwrap_or_else(|| "—".into()),
                next(row, receipt.finished_at)
            );
        }
    } else {
        for row in &receipt.steps {
            let action = if row.would_run == Some(true) {
                "would run".into()
            } else {
                next(row, receipt.finished_at)
            };
            let last = row
                .last_finished_at
                .map(|at| {
                    let mark = match row.last_outcome {
                        Some(Outcome::Ok) => "✓",
                        Some(Outcome::Failed) => "✗",
                        Some(Outcome::Waiting) => "◌",
                        _ => "·",
                    };
                    format!(
                        " · last {mark} {} · {}",
                        clock(at),
                        row.last_summary.as_deref().unwrap_or("—")
                    )
                })
                .unwrap_or_default();
            println!(
                "  {} {:<9} {:<7} {:<10} {}{}",
                glyph(row, Styler::detect()),
                row.name,
                row.cadence,
                action,
                safe_text(&row.summary),
                safe_text(&last)
            );
        }
        println!(
            "  {} steps would run · dry run, nothing written",
            receipt
                .steps
                .iter()
                .filter(|r| r.would_run == Some(true))
                .count()
        );
    }
}
