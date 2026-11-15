//! Human output for `bob areas list` and `bob areas check`.

use std::collections::BTreeMap;

use super::{
    catalog::CatalogState,
    report::{AreaEntry, AreaReport, CatalogRecord},
    slash,
};
use crate::native::{notebooks::model::Severity, style::Styler};

pub(crate) fn print_list(report: &AreaReport) {
    let styler = Styler::detect();
    println!(
        "Areas · {} areas · {} catalogs",
        report.counts.areas, report.counts.hosts
    );
    if report.root.state == super::RootState::Found {
        if let Some(path) = &report.root.path {
            let stem = path
                .file_stem()
                .and_then(|stem| stem.to_str())
                .unwrap_or("area");
            println!("{}", styler.bold(stem));
        }
        print_tree(report, &styler);
    }
    print_unplaced(report, &styler);
    print_catalog_footer(report, &styler);
    print_findings_hint(report, &styler);
}

fn print_tree(report: &AreaReport, styler: &Styler) {
    let by_path: BTreeMap<_, _> = report
        .areas
        .iter()
        .map(|entry| (entry.path.clone(), entry))
        .collect();
    let tops: Vec<_> = report
        .areas
        .iter()
        .filter(|entry| entry.depth == Some(1))
        .collect();
    for (index, top) in tops.iter().enumerate() {
        print_node(top, &by_path, styler, "", index + 1 == tops.len());
    }
}

fn print_node(
    entry: &AreaEntry,
    by_path: &BTreeMap<std::path::PathBuf, &AreaEntry>,
    styler: &Styler,
    prefix: &str,
    last: bool,
) {
    let branch = if last { "└─ " } else { "├─ " };
    println!("{prefix}{branch}{}", format_row(entry, styler));
    let child_prefix = format!("{prefix}{}", if last { "   " } else { "│  " });
    let children: Vec<_> = entry
        .children
        .iter()
        .filter_map(|path| by_path.get(path).copied())
        .collect();
    for (index, child) in children.iter().enumerate() {
        print_node(
            child,
            by_path,
            styler,
            &child_prefix,
            index + 1 == children.len(),
        );
    }
}

fn format_row(entry: &AreaEntry, styler: &Styler) -> String {
    let dot = match (entry.hue, entry.hue_source) {
        (Some(hue), _) => styler.xterm256(hue.xterm(), "●"),
        (None, Some(crate::native::pomodoro_homes::HomeHueSource::Muted)) => {
            styler.dim("●")
        }
        _ => styler.dim("●"),
    };
    let mut line = format!("{dot} {}", styler.bold(&entry.stem));
    if entry.titled {
        line.push(' ');
        line.push_str(&styler.dim(&entry.title));
    }
    if let Some(summary) =
        entry.summary.as_deref().filter(|text| !text.is_empty())
    {
        line.push_str(&styler.dim(&format!(" — {summary}")));
    }
    let mut chips = Vec::new();
    if entry.projects.direct > 0 {
        chips.push(format!("{} projects", entry.projects.direct));
    }
    if entry.today.direct > 0 {
        chips.push(format!("● {} today", entry.today.direct));
    }
    if !chips.is_empty() {
        line.push(' ');
        line.push_str(&styler.dim(&chips.join(" ")));
    }
    line
}

fn print_unplaced(report: &AreaReport, styler: &Styler) {
    let unplaced: Vec<_> = report
        .areas
        .iter()
        .filter(|entry| entry.depth.is_none())
        .collect();
    if unplaced.is_empty() {
        return;
    }
    println!("{}", styler.bold("Unplaced"));
    for entry in unplaced {
        let code = entry
            .issues
            .iter()
            .find(|issue| issue.severity == Severity::Error)
            .or_else(|| entry.issues.first())
            .map(|issue| issue.code)
            .unwrap_or("unplaced");
        println!("{}  [{}]", format_row(entry, styler), styler.dim(code));
    }
}

fn print_catalog_footer(report: &AreaReport, styler: &Styler) {
    let damaged: Vec<_> = report
        .catalogs
        .iter()
        .filter(|catalog| catalog.state == CatalogState::Damaged)
        .collect();
    if !damaged.is_empty() {
        for catalog in damaged {
            println!("{}", styler.red(&damaged_line(catalog)));
        }
        return;
    }
    let stale = report.counts.catalogs.missing + report.counts.catalogs.stale;
    if stale > 0 {
        println!(
            "{}",
            styler.yellow(&format!(
                "! {stale} catalogs stale · run bob areas sync"
            ))
        );
        return;
    }
    println!(
        "{}",
        styler.green(&format!(
            "✓ {} catalogs current",
            report.counts.catalogs.current
        ))
    );
}

fn damaged_line(catalog: &CatalogRecord) -> String {
    let issue = catalog.issues.first();
    let line = issue.and_then(|issue| issue.line).unwrap_or(1);
    let code = issue
        .map(|issue| issue.code)
        .unwrap_or("area_catalog_damaged");
    let message = issue
        .map(|issue| issue.message.as_str())
        .unwrap_or("damaged catalog");
    format!("{}:{line} [{code}] {message}", slash(&catalog.path))
}

fn print_findings_hint(report: &AreaReport, styler: &Styler) {
    if report.counts.errors == 0 && report.counts.warnings == 0 {
        return;
    }
    println!(
        "{}",
        styler.dim(&format!(
            "{} errors · {} warnings · run bob areas check",
            report.counts.errors, report.counts.warnings
        ))
    );
}

pub(crate) fn print_check(report: &AreaReport) {
    let styler = Styler::detect();
    if report.findings.is_empty() {
        println!("{}", styler.green("✓ areas match the contract"));
        return;
    }
    let mut grouped: BTreeMap<_, Vec<_>> = BTreeMap::new();
    for finding in &report.findings {
        grouped
            .entry(slash(&finding.path))
            .or_default()
            .push(finding);
    }
    for (path, findings) in grouped {
        println!("{}", styler.bold(&path));
        for finding in findings {
            let glyph = match finding.severity {
                Severity::Error => styler.red("✗"),
                Severity::Warning => styler.yellow("!"),
            };
            let line = finding
                .line
                .map(|line| format!("{line}"))
                .unwrap_or_else(|| "-".to_string());
            println!(
                "{glyph} {path}:{line} [{}] {}",
                finding.code, finding.message
            );
        }
    }
    println!(
        "{} areas · {} errors · {} warnings",
        report.counts.areas, report.counts.errors, report.counts.warnings
    );
}
