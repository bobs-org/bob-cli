//! Human output for `bob bank check`.

use std::collections::BTreeMap;

use chrono::NaiveDate;

use super::{
    check::{CheckReport, Finding, Severity},
    grammar::render_day_heading,
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
