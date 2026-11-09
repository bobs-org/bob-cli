//! Human output for notebook list and check.

use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
};

use chrono::NaiveDate;

use super::model::{NotebookEntry, NotebookReport};
use crate::native::{
    env as bob_env,
    pomodoro_homes::{DiskHomeSource, HomeResolver},
    style::Styler,
};

pub(crate) fn print_list(report: &NotebookReport, bob_dir: &Path) {
    let styler = Styler::detect();
    if report.notebooks.is_empty() {
        println!("No notebooks.");
        return;
    }
    // Group by root; invalid/unowned last.
    let mut groups: BTreeMap<Option<String>, Vec<&NotebookEntry>> =
        BTreeMap::new();
    for entry in &report.notebooks {
        groups.entry(entry.root.clone()).or_default().push(entry);
    }
    // Sort group keys: Some roots alphabetical case-insensitive, None last.
    let mut ordered_keys: Vec<Option<String>> =
        groups.keys().cloned().collect();
    ordered_keys.sort_by(|left, right| match (left, right) {
        (Some(a), Some(b)) => a
            .to_lowercase()
            .cmp(&b.to_lowercase())
            .then_with(|| a.cmp(b)),
        (Some(_), None) => std::cmp::Ordering::Less,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (None, None) => std::cmp::Ordering::Equal,
    });

    // Resolver for root hues (read-only, agrees with scan ownership).
    let source = DiskHomeSource::new(bob_dir);
    let mut resolver = HomeResolver::new(source);

    for key in ordered_keys {
        let entries = groups.get(&key).cloned().unwrap_or_default();
        let header = match &key {
            Some(root) => {
                // Hue from the root note itself when possible.
                let hue = hue_for_root(&mut resolver, root, bob_dir);
                match hue {
                    Some(code) => styler.xterm256(code, root),
                    None => styler.bold(root),
                }
            }
            None => styler.bold("invalid / unowned"),
        };
        println!("{header}");
        print_forest(&entries, &styler, 0);
        println!();
    }
    let counts = &report.counts;
    let summary = format!(
        "{} notebooks in {} roots — {} errors, {} warnings",
        counts.notebooks, counts.roots, counts.errors, counts.warnings
    );
    println!("{summary}");
}

fn hue_for_root<S>(
    resolver: &mut HomeResolver<S>,
    root: &str,
    _bob_dir: &Path,
) -> Option<u8>
where
    S: crate::native::pomodoro_homes::HomeSource,
{
    // Resolve `<root>.md` to read its own root hue. Fall back to None when
    // the root note cannot be resolved (stays bold).
    let path = PathBuf::from(format!("{root}.md"));
    let home = resolver.resolve_note(&path);
    home.root_hue.map(|hue| hue.xterm())
}

fn print_forest(entries: &[&NotebookEntry], styler: &Styler, _depth: usize) {
    // Map stem -> entry for tree building within this root group.
    let mut by_stem: BTreeMap<String, &NotebookEntry> = BTreeMap::new();
    for entry in entries {
        by_stem.insert(entry.stem.clone(), *entry);
    }
    // Top-level: parent is not a notebook in this group.
    let mut tops: Vec<&NotebookEntry> = Vec::new();
    for entry in entries {
        let nested = match &entry.parent {
            Some(parent)
                if entry.parent_kind.as_deref() == Some("notebook") =>
            {
                by_stem.contains_key(parent)
            }
            _ => false,
        };
        if !nested {
            tops.push(*entry);
        }
    }
    tops.sort_by(|left, right| {
        left.title
            .to_lowercase()
            .cmp(&right.title.to_lowercase())
            .then_with(|| left.path.cmp(&right.path))
    });
    for (index, top) in tops.iter().enumerate() {
        let last = index + 1 == tops.len();
        print_subtree(top, &by_stem, styler, String::new(), last, true);
    }
}

fn print_subtree(
    entry: &NotebookEntry,
    by_stem: &BTreeMap<String, &NotebookEntry>,
    styler: &Styler,
    prefix: String,
    last: bool,
    is_top: bool,
) {
    let glyph = if is_top {
        "- ".to_string()
    } else if last {
        "└─ ".to_string()
    } else {
        "├─ ".to_string()
    };
    let counts = counts_text(entry);
    let issues = issue_marker(entry);
    let summary = entry
        .summary
        .as_deref()
        .map(|text| format!(" — {}", styler.dim(text)))
        .unwrap_or_default();
    let title = styler.bold(&entry.title);
    println!(
        "{prefix}{glyph}[[{}|{title}]]{summary}  {counts}{issues}",
        entry.stem,
    );
    // Children sorted deterministically (scan already sorts, re-sort by title).
    let mut children: Vec<&NotebookEntry> = entry
        .children
        .iter()
        .filter_map(|stem| by_stem.get(stem).copied())
        .collect();
    children.sort_by(|left, right| {
        left.title
            .to_lowercase()
            .cmp(&right.title.to_lowercase())
            .then_with(|| left.path.cmp(&right.path))
    });
    let child_prefix = if is_top {
        format!("{prefix}  ")
    } else {
        format!("{prefix}{}", if last { "   " } else { "│  " })
    };
    for (index, child) in children.iter().enumerate() {
        let child_last = index + 1 == children.len();
        print_subtree(
            child,
            by_stem,
            styler,
            child_prefix.clone(),
            child_last,
            false,
        );
    }
    let _ = by_stem.len();
}

fn counts_text(entry: &NotebookEntry) -> String {
    let age = age_text(entry.last_note.as_deref());
    let mut text =
        format!("{} notes · {} §", entry.notes, entry.sections.len());
    if let Some(age) = age {
        text.push_str(&format!(" · {age}"));
    }
    text
}

fn age_text(last_note: Option<&str>) -> Option<String> {
    let raw = last_note?;
    let date = NaiveDate::parse_from_str(raw, "%Y-%m-%d").ok()?;
    let today = bob_env::current_datetime().date();
    let days = (today - date).num_days();
    if days < 0 {
        return Some(raw.to_string());
    }
    if days == 0 {
        return Some("today".to_string());
    }
    if days == 1 {
        return Some("1d".to_string());
    }
    Some(format!("{days}d"))
}

fn issue_marker(entry: &NotebookEntry) -> String {
    let errors = entry
        .issues
        .iter()
        .filter(|issue| issue.severity == super::model::Severity::Error)
        .count();
    let warnings = entry
        .issues
        .iter()
        .filter(|issue| issue.severity == super::model::Severity::Warning)
        .count();
    if errors > 0 && warnings > 0 {
        format!("  !{errors} ~{warnings}")
    } else if errors > 0 {
        format!("  !{errors}")
    } else if warnings > 0 {
        format!("  ~{warnings}")
    } else {
        String::new()
    }
}

pub(crate) fn print_check(report: &NotebookReport) {
    let styler = Styler::detect();
    let mut any = false;
    // Group actionable findings by notebook, sorted by path.
    let mut entries: Vec<&NotebookEntry> = report.notebooks.iter().collect();
    entries.sort_by(|left, right| left.path.cmp(&right.path));
    for entry in entries {
        if entry.issues.is_empty() {
            continue;
        }
        any = true;
        let header = format!("{} — {}", entry.path, entry.title);
        println!("{}", styler.bold(&header));
        for issue in &entry.issues {
            let severity = match issue.severity {
                super::model::Severity::Error => styler.red("error"),
                super::model::Severity::Warning => styler.yellow("warning"),
            };
            match issue.line {
                Some(line) => println!(
                    "  {line}: [{code}/{severity}] {message}",
                    code = issue.code,
                    message = issue.message
                ),
                None => println!(
                    "  [{code}/{severity}] {message}",
                    code = issue.code,
                    message = issue.message
                ),
            }
        }
        println!();
    }
    let counts = &report.counts;
    let summary = format!(
        "{} notebooks in {} roots — {} errors, {} warnings",
        counts.notebooks, counts.roots, counts.errors, counts.warnings
    );
    if counts.errors > 0 {
        println!("{}", styler.red(&format!("✗ {summary}")));
    } else if any || counts.warnings > 0 {
        println!("{}", styler.yellow(&format!("✓ {summary}")));
    } else {
        println!("{}", styler.green(&format!("✓ {summary}")));
    }
    let _ = BTreeSet::<String>::new();
}
