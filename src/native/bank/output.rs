//! Human output for `bob bank`.

use std::collections::BTreeMap;

use chrono::{Datelike, NaiveDate};

use super::{
    check::{CheckReport, Finding, Severity},
    grammar::{render_amount, render_day_heading},
    model::BankRow,
};
use crate::native::style::Styler;

pub(crate) fn print_check(report: &CheckReport) {
    let styler = Styler::detect();
    if report.teaching_empty {
        print_teaching_empty(report.today);
        return;
    }
    let mut by_path: BTreeMap<&str, Vec<&Finding>> = BTreeMap::new();
    for finding in &report.findings {
        by_path.entry(&finding.path).or_default().push(finding);
    }
    let mut any = false;
    for (path, findings) in by_path {
        any = true;
        println!("{}", styler.bold(path));
        print_path_findings(path, &findings, &styler);
        println!();
    }
    let summary = format!(
        "{} notes — {} errors, {} warnings",
        report.note_count, report.issues.errors, report.issues.warnings
    );
    if report.issues.errors > 0 {
        println!("{}", styler.red(&format!("✗ {summary}")));
    } else if any || report.issues.warnings > 0 {
        println!("{}", styler.yellow(&format!("✓ {summary}")));
    } else {
        println!("{}", styler.green(&format!("✓ {summary}")));
    }
}

pub(crate) fn account_chips(
    accounts: &[super::config::AccountStatus],
    today: NaiveDate,
) -> String {
    if accounts.is_empty() {
        return today.format("%b %-d").to_string();
    }
    let mut parts = Vec::new();
    for account in accounts {
        let chip = match account.status {
            "balanced" => format!("{} ✓ balanced", account.label),
            "unverified" => format!("{} · unverified", account.label),
            "untracked" => format!("{} · untracked", account.label),
            other => format!("{} · {other}", account.label),
        };
        parts.push(chip);
    }
    parts.push(today.format("%b %-d").to_string());
    parts.join(" · ")
}

pub(crate) fn day_label(date: NaiveDate) -> String {
    render_day_heading(date)
        .trim_start_matches('#')
        .trim()
        .to_owned()
}

pub(crate) fn print_list(report: &super::list::ListReport, today: NaiveDate) {
    let styler = Styler::detect();
    let chips = account_chips(&report.accounts, today);
    if report.waiting == 0 {
        println!("🏦 Bank · {}", styler.bold("nothing to acknowledge"));
        println!("  {chips}");
        println!();
        println!(
            "Download a QFX from Wells Fargo, then: bob bank import ~/Downloads/*.qfx"
        );
        return;
    }
    let count = if report.waiting == 1 {
        "1 to acknowledge".to_owned()
    } else {
        format!("{} to acknowledge", report.waiting)
    };
    println!("🏦 Bank · {}  {chips}", styler.bold(&count));
    println!();
    for day in &report.days {
        let (out, inflow) = day_out_in(&day.rows);
        let label = day_label(day.date);
        println!(
            "{label}  out {} · in {}",
            render_amount(-out),
            render_amount(inflow)
        );
        for row in &day.rows {
            print_queue_row(row, &styler, report);
        }
        println!();
    }
    if let Some(first) = report.days.first() {
        let oldest = report
            .days
            .iter()
            .map(|day| day.date)
            .min()
            .unwrap_or(today);
        println!(
            "Tick them in {}, or: bob bank ack -D {}",
            first.path,
            oldest.format("%Y-%m-%d")
        );
    }
}

fn day_out_in(rows: &[BankRow]) -> (i64, i64) {
    let mut out = 0i64;
    let mut inflow = 0i64;
    for row in rows {
        if row.cents < 0 {
            if row.category.as_deref() != Some("transfer") {
                out += -row.cents;
            }
        } else if row.category.as_deref() == Some("income")
            || row.category.as_deref() == Some("refund")
        {
            inflow += row.cents;
        }
    }
    (out, inflow)
}

fn print_queue_row(
    row: &BankRow,
    styler: &Styler,
    report: &super::list::ListReport,
) {
    let glyph = if row.acknowledged { "☑" } else { "☐" };
    let amount = render_amount(row.cents);
    let category = category_chip(row, report);
    let mut line = format!("  {glyph} {amount:>12}  {}", row.payee);
    if !category.is_empty() {
        line.push_str(&format!("  {category}"));
    }
    if row.flagged {
        line.push_str("  ⚑");
    }
    if let Some(note) = &row.note
        && !note.is_empty()
    {
        line.push_str(&format!("  — {note}"));
    }
    if row.cents > 0 {
        println!("{}", styler.green(&line));
    } else {
        println!("{line}");
    }
}

fn category_chip(row: &BankRow, report: &super::list::ListReport) -> String {
    match &row.category {
        Some(key) => match report
            .categories
            .iter()
            .find(|category| &category.key == key)
        {
            Some(category) if !category.emoji.is_empty() => {
                format!("{} {}", category.emoji, category.label)
            }
            Some(category) => category.label.clone(),
            None => key.clone(),
        },
        None => "· Unclassified".to_owned(),
    }
}

pub(crate) fn print_report(report: &super::report::BankReport) {
    let styler = Styler::detect();
    let chips = account_chips(&report.accounts, report.today);
    let title = if report.period.so_far {
        format!("🏦 {} · so far", report.period.label)
    } else {
        format!("🏦 {}", report.period.label)
    };
    println!("{title}  {chips}");
    let totals = &report.totals;
    let waiting = if report.waiting_period == 0 {
        "nothing to acknowledge".to_owned()
    } else if report.waiting_period == 1 {
        "1 to acknowledge".to_owned()
    } else {
        format!("{} to acknowledge", report.waiting_period)
    };
    let mut second = waiting;
    if report.flagged > 0 {
        second.push_str(&format!(" · {} ⚑", report.flagged));
    }
    println!(
        "Out {} · In {} · Net {}  {second}",
        render_amount(-totals.out_cents),
        render_amount(totals.in_cents),
        render_amount(totals.net_cents),
    );
    println!();
    for entry in &report.breakdown {
        let bar = share_bar(entry.share);
        let percent = (entry.share * 100.0).round() as i64;
        let mut line = format!(
            "{} {}  {bar}  {}  {percent}%",
            entry.emoji,
            entry.label,
            render_amount(-entry.out_cents),
        );
        if entry.key == "cash" {
            line.push_str("  destination unknown");
        }
        println!("{line}");
    }
    println!(
        "Transfers ↔ {} · Refunds {} · Cash traced {}",
        render_amount(
            -(report.totals.transfer_out_cents
                + report.totals.transfer_in_cents)
        ),
        render_amount(report.totals.refund_cents),
        render_amount(-report.totals.cash_traced_cents),
    );
    println!();
    print_heat_strip(report, &styler);
}

fn share_bar(share: f64) -> String {
    const WIDTH: usize = 14;
    let filled =
        (share * WIDTH as f64).round().clamp(0.0, WIDTH as f64) as usize;
    format!("{}{}", "█".repeat(filled), "░".repeat(WIDTH - filled))
}

fn print_heat_strip(report: &super::report::BankReport, styler: &Styler) {
    use super::report::PeriodKind;
    if !matches!(report.period.kind, PeriodKind::Month) {
        return;
    }
    println!("Mo Tu We Th Fr Sa Su      ░ under $25 · ▒ under $100 · ▓ $100+");
    let mut waiting_by_date = std::collections::HashSet::new();
    let mut out_by_date = std::collections::HashMap::new();
    for day in &report.days {
        if day.waiting > 0 {
            waiting_by_date.insert(day.date);
        }
        out_by_date.insert(day.date, day.out_cents);
    }
    let mut cursor = report.period.start;
    // Monday-first offset.
    let offset = cursor.weekday().num_days_from_monday() as usize;
    let mut week: Vec<String> = vec!["    ".to_owned(); offset];
    loop {
        let out = out_by_date.get(&cursor).copied().unwrap_or(0);
        let intensity = if out <= 0 {
            ' '
        } else if out < 2500 {
            '░'
        } else if out < 10000 {
            '▒'
        } else {
            '▓'
        };
        let marker = if waiting_by_date.contains(&cursor) {
            '☐'
        } else if cursor == report.today {
            '*'
        } else {
            intensity
        };
        let cell = format!("{:>2}{marker} ", cursor.day());
        if cursor == report.today {
            week.push(styler.bold(&cell));
        } else {
            week.push(cell);
        }
        if cursor.weekday() == chrono::Weekday::Sun
            || cursor == report.period.end
        {
            println!("{}", week.concat().trim_end());
            week.clear();
        }
        if cursor == report.period.end {
            break;
        }
        let Some(next) = cursor.succ_opt() else {
            break;
        };
        cursor = next;
    }
}

pub(crate) fn print_ensure(
    path: &str,
    line: usize,
    day: NaiveDate,
    created_note: bool,
    created_heading: bool,
) {
    if created_note {
        println!("Created {path}");
    }
    if created_heading {
        println!("Created heading {} in {path}", day_label(day));
    }
    println!("{path}:{line}");
}

pub(crate) fn print_ack(
    acknowledged: &[super::ack::AckTarget],
    already: &[super::ack::AckTarget],
    unknown: &[String],
    dry_run: bool,
) {
    let styler = Styler::detect();
    if dry_run {
        println!("{}", styler.dim("Dry run — nothing written"));
    }
    if acknowledged.is_empty() && unknown.is_empty() {
        println!("Nothing to acknowledge.");
    } else {
        println!(
            "{}",
            styler.green(&format!(
                "✓ Acknowledged {} row{}",
                acknowledged.len(),
                if acknowledged.len() == 1 { "" } else { "s" }
            ))
        );
        for target in acknowledged {
            let id = target
                .id
                .as_ref()
                .map(|id| format!(" ^bank-{id}"))
                .unwrap_or_default();
            println!("  {}:{}{id} {}", target.path, target.line, target.payee);
        }
    }
    if !already.is_empty() {
        println!("Already ticked ({}):", already.len());
        for target in already {
            println!("  {}:{} {}", target.path, target.line, target.payee);
        }
    }
    if !unknown.is_empty() {
        println!("Unknown IDs ({}):", unknown.len());
        for id in unknown {
            println!("  {id}");
        }
    }
}

pub(crate) fn print_teaching_empty(today: NaiveDate) {
    let start = today.format("%Y-%m-%d");
    println!("No bank.md yet.");
    println!();
    println!("Create a bank hub at the vault root with a `bank:` mapping.");
    println!("See docs/bank.md.");
    println!();
    println!("Paste-ready start:");
    println!();
    println!("---");
    println!("parent: \"[[cash]]\"");
    println!("type: \"[[area]]\"");
    println!("bank:");
    println!("  accounts:");
    println!("    checking:");
    println!("      label: WF Checking");
    println!("      kind: checking");
    println!("      start: {start}");
    println!("---");
    println!();
    println!("# 🏦 Bank");
}

fn print_path_findings(path: &str, findings: &[&Finding], styler: &Styler) {
    let mut by_day: BTreeMap<Option<NaiveDate>, Vec<&Finding>> =
        BTreeMap::new();
    for finding in findings {
        by_day.entry(finding.day).or_default().push(*finding);
    }
    if let Some(ungrouped) = by_day.remove(&None) {
        for finding in ungrouped {
            print_finding(finding, styler);
        }
    }
    if path != "bank.md" {
        for (date, grouped) in by_day {
            if let Some(date) = date {
                let heading = render_day_heading(date);
                println!("  {}", styler.dim(&heading));
                for finding in grouped {
                    print_finding(finding, styler);
                }
            }
        }
    } else {
        for grouped in by_day.values() {
            for finding in grouped {
                print_finding(finding, styler);
            }
        }
    }
}

fn print_finding(finding: &Finding, styler: &Styler) {
    let severity = match finding.severity {
        Severity::Error => styler.red("error"),
        Severity::Warning => styler.yellow("warning"),
        Severity::Info => styler.dim("info"),
    };
    let location = match finding.line {
        Some(line) => format!("{}:{line}", finding.path),
        None => finding.path.clone(),
    };
    println!(
        "  {location}: [{code}/{severity}] {message}",
        code = finding.code,
        message = finding.message
    );
    if let Some(hint) = &finding.hint {
        println!("    hint: {hint}");
    }
}
