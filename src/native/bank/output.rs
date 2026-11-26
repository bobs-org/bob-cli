//! Human output for `bob bank`.

use std::collections::BTreeMap;

use chrono::{Datelike, NaiveDate};

use super::{
    balance::BalanceState,
    charts::{Bucket, Charts},
    check::{CheckReport, Finding, Severity},
    grammar::{render_amount, render_day_heading},
    model::BankRow,
    report::ReportBalance,
};
use crate::native::style::{display_width, pad_right, Styler};

pub(crate) fn print_check(report: &CheckReport) {
    let styler = Styler::detect();
    if report.teaching_empty {
        print_teaching_empty(report.today);
        return;
    }
    let mut by_path: BTreeMap<&str, Vec<&Finding>> = BTreeMap::new();
    let mut subscription_by_path: BTreeMap<&str, Vec<&Finding>> =
        BTreeMap::new();
    for finding in &report.findings {
        if finding.code.starts_with("bank_subscription_") {
            subscription_by_path
                .entry(&finding.path)
                .or_default()
                .push(finding);
        } else {
            by_path.entry(&finding.path).or_default().push(finding);
        }
    }
    let mut any = false;
    for (path, findings) in by_path {
        any = true;
        println!("{}", styler.bold(path));
        print_path_findings(path, &findings, &styler);
        println!();
    }
    if !subscription_by_path.is_empty() {
        any = true;
        println!("{}", styler.bold("🔁 Subscriptions"));
        for (path, findings) in subscription_by_path {
            println!("{}", styler.dim(path));
            print_path_findings(path, &findings, &styler);
        }
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
            "delta" => format!(
                "{} Δ {}",
                account.label,
                render_amount(account.delta_cents.unwrap_or_default())
            ),
            "gap" => format!("{} · coverage gap", account.label),
            "stale" => format!("{} · stale", account.label),
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
    for entry in &report.balances {
        if let Some(line) = balance_line(entry) {
            println!("{line}");
        }
    }
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
    print_coverage(report);
    println!();
    print_heat_strip(report, &styler);
    if let Some(charts) = &report.charts {
        print_charts(charts.as_ref(), &styler);
    }
}

/// The `-c` aggregate as one aligned line per bucket (then per group):
/// label, In, Out, waiting, and state words. In is green, waiting is
/// yellow, and non-tracked state words are dim.
///
/// `print_report` already ends with a blank line, and the month heat strip
/// never prints for a `-c` period, so the table starts right away.
fn print_charts(charts: Option<&Charts>, styler: &Styler) {
    let Some(charts) = charts else {
        println!(
            "{}",
            styler.dim("Charts · history spans more than 64 ISO years")
        );
        return;
    };
    if charts.buckets.is_empty() {
        println!("{}", styler.dim("Charts · no history to chart yet"));
        return;
    }
    print_bucket_table(
        &format!("By {}", charts.granularity.as_str()),
        &charts.buckets,
        styler,
    );
    if let Some(first) = charts.groups.first() {
        println!();
        print_bucket_table(
            &format!("By {}", first.kind.as_str()),
            &charts.groups,
            styler,
        );
    }
}

fn print_bucket_table(title: &str, buckets: &[Bucket], styler: &Styler) {
    let amount = |cents: i64| {
        if cents == 0 {
            "–".to_owned()
        } else {
            render_amount(cents)
        }
    };
    let ins: Vec<String> = buckets
        .iter()
        .map(|bucket| amount(bucket.in_cents))
        .collect();
    let outs: Vec<String> = buckets
        .iter()
        .map(|bucket| amount(-bucket.out_cents))
        .collect();
    let waits: Vec<String> = buckets
        .iter()
        .map(|bucket| match bucket.waiting {
            0 => String::new(),
            count => format!("{count} waiting"),
        })
        .collect();
    let label_width = buckets
        .iter()
        .map(|bucket| display_width(&bucket.label))
        .chain([display_width(title)])
        .max()
        .unwrap_or(0);
    let width = |cells: &[String], header: &str| {
        cells
            .iter()
            .map(|cell| display_width(cell))
            .chain([display_width(header)])
            .max()
            .unwrap_or(0)
    };
    let in_width = width(&ins, "In");
    let out_width = width(&outs, "Out");
    let wait_width = width(&waits, "");
    println!(
        "{}  {}  {}",
        styler.bold(&pad_right(title, label_width)),
        pad_left("In", in_width),
        pad_left("Out", out_width),
    );
    for (index, bucket) in buckets.iter().enumerate() {
        let in_cell = pad_left(&ins[index], in_width);
        let in_cell = if bucket.in_cents == 0 {
            in_cell
        } else {
            styler.green(&in_cell)
        };
        let wait_cell = pad_right(&waits[index], wait_width);
        let wait_cell = if bucket.waiting == 0 {
            wait_cell
        } else {
            styler.yellow(&wait_cell)
        };
        let mut words = Vec::new();
        if bucket.so_far {
            words.push("so far".to_owned());
        }
        if let Some(state) = bucket.state.words() {
            words.push(styler.dim(state));
        }
        let line = format!(
            "{}  {in_cell}  {}  {wait_cell}  {}",
            pad_right(&bucket.label, label_width),
            pad_left(&outs[index], out_width),
            words.join(" · "),
        );
        println!("{}", line.trim_end());
    }
}

fn pad_left(text: &str, width: usize) -> String {
    let padding = width.saturating_sub(display_width(text));
    format!("{}{text}", " ".repeat(padding))
}

fn balance_line(entry: &ReportBalance) -> Option<String> {
    use chrono::Datelike;
    let inner = &entry.balance;
    let prefix = format!("Balance · {}", inner.label);
    // Amounts never carry a `+`: `$1,000.00`, `$0.00`, `−$45.10`.
    let amount = |cents: i64| {
        if cents < 0 {
            format!("−{}", render_amount(cents))
        } else {
            render_amount(-cents)
        }
    };
    // Changes keep the inflow `+`: `+$12.00`, `$0.00`, `−$65.44`.
    let signed = |cents: i64| {
        if cents < 0 {
            format!("−{}", render_amount(cents))
        } else {
            render_amount(cents)
        }
    };
    let day_for = |date: NaiveDate| {
        let mut label = day_label(date);
        if date.year() != inner.wanted.year() {
            label.push_str(&format!(", {}", date.year()));
        }
        label
    };
    match inner.state {
        BalanceState::None => None,
        BalanceState::SoFar => {
            let cents = inner.cents?;
            let as_of = inner.as_of?;
            let mut line = format!(
                "{prefix} {} so far, as of {}",
                amount(cents),
                day_for(as_of)
            );
            if let (Some(open), Some(change)) =
                (&inner.open, inner.change_cents)
            {
                line.push_str(&format!(
                    " · {} since {}",
                    signed(change),
                    day_for(open.date)
                ));
            }
            if inner.stale {
                line.push_str(" · stale");
            }
            Some(line)
        }
        BalanceState::Settled => {
            let cents = inner.cents?;
            let mut line = format!(
                "{prefix} {} at close {}",
                amount(cents),
                day_for(inner.wanted)
            );
            if let (Some(open), Some(change)) =
                (&inner.open, inner.change_cents)
            {
                line.push_str(&format!(
                    " · {} since {}",
                    signed(change),
                    day_for(open.date)
                ));
            }
            Some(line)
        }
        BalanceState::AsOf => {
            let cents = inner.cents?;
            let as_of = inner.as_of?;
            let mut line =
                format!("{prefix} {} as of {}", amount(cents), day_for(as_of));
            if let (Some(open), Some(change)) =
                (&inner.open, inner.change_cents)
            {
                line.push_str(&format!(
                    " · {} since {}",
                    signed(change),
                    day_for(open.date)
                ));
            }
            Some(line)
        }
        BalanceState::AwaitingImport | BalanceState::Gap => {
            Some(format!("{prefix} not imported yet"))
        }
        BalanceState::Unverified => Some(format!(
            "{prefix} unverified (bank balances disagree — see bob bank check)"
        )),
    }
}

fn print_coverage(report: &super::report::BankReport) {
    if report.coverage.complete {
        return;
    }
    let mut parts = Vec::new();
    if let Some(tracked) = report.coverage.tracked_since
        && tracked > report.period.start
    {
        parts.push(format!("Tracked since {}", tracked.format("%b %-d")));
    }
    if let Some(imported) = report.coverage.imported_through
        && imported < report.period.end
    {
        parts.push(format!("imported through {}", imported.format("%b %-d")));
    }
    if !parts.is_empty() {
        println!("{}", parts.join(" · "));
    }
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
