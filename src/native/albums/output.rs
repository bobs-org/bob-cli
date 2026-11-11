//! Human output for album list and check.

use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

use chrono::NaiveDate;

use super::{
    report::{AlbumIssue, AlbumReport, AlbumReportEntry},
    AlbumIssueSeverity,
};
use crate::native::{
    env as bob_env,
    pomodoro_homes::{DiskHomeSource, HomeResolver},
    style::{pad_right, Styler},
};

pub(crate) fn print_list(report: &AlbumReport, bob_dir: &Path) {
    let styler = Styler::detect();
    if report.albums.is_empty() {
        println!("No albums.");
        println!("Create one with `bob albums create`.");
        return;
    }

    let mut groups: BTreeMap<Option<String>, Vec<&AlbumReportEntry>> =
        BTreeMap::new();
    for entry in &report.albums {
        groups.entry(list_group_key(entry)).or_default().push(entry);
    }
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

    let source = DiskHomeSource::new(bob_dir);
    let mut resolver = HomeResolver::new(source);

    for key in ordered_keys {
        let mut entries = groups.get(&key).cloned().unwrap_or_default();
        entries.sort_by(|left, right| {
            left.title
                .to_lowercase()
                .cmp(&right.title.to_lowercase())
                .then_with(|| left.path.cmp(&right.path))
        });
        let header = match &key {
            Some(root) => match hue_for_root(&mut resolver, root) {
                Some(code) => styler.xterm256(code, root),
                None => styler.bold(root),
            },
            None => styler.bold("invalid / unowned"),
        };
        println!("{header}");
        print_rows(&entries, &styler);
        println!();
    }
}

fn list_group_key(entry: &AlbumReportEntry) -> Option<String> {
    if entry
        .issues
        .iter()
        .any(|issue| issue.severity == AlbumIssueSeverity::Error)
    {
        None
    } else {
        entry.root.clone()
    }
}

fn print_rows(entries: &[&AlbumReportEntry], styler: &Styler) {
    let title_width = entries
        .iter()
        .map(|entry| display_title_width(entry))
        .max()
        .unwrap_or(0);
    let stem_width = entries
        .iter()
        .map(|entry| entry.stem.chars().count() + 1)
        .max()
        .unwrap_or(0);
    for entry in entries {
        let emoji = entry.emoji.as_deref().unwrap_or(" ");
        let title = pad_right(
            &styler.bold(&entry.title),
            title_width + ansi_pad(styler, &entry.title),
        );
        let stem = pad_right(&format!("{}/", entry.stem), stem_width);
        let counts = counts_text(entry);
        let age = age_text(entry.last_record.as_deref())
            .map(|age| format!("  {age}"))
            .unwrap_or_default();
        let issues = issue_marker(entry);
        println!("  {emoji} {title}  {stem}  {counts}{age}{issues}");
    }
}

fn display_title_width(entry: &AlbumReportEntry) -> usize {
    entry.title.chars().count()
}

fn ansi_pad(styler: &Styler, title: &str) -> usize {
    let painted = styler.bold(title);
    painted
        .chars()
        .count()
        .saturating_sub(title.chars().count())
}

fn counts_text(entry: &AlbumReportEntry) -> String {
    let mut parts = vec![entry.records.to_string()];
    for (index, group) in entry.groups.iter().enumerate() {
        parts.push(group_count_text(group, index == 0));
    }
    if entry.needs_status > 0 {
        parts.push(format!("⚠️ {}", entry.needs_status));
    }
    parts.join(" · ")
}

fn group_count_text(
    group: &super::grouping::RecordGroup,
    first: bool,
) -> String {
    let (icon, rest) = split_icon(&group.label);
    match (icon, first) {
        (Some(icon), true) if !rest.is_empty() => {
            format!("{icon} {} {}", group.count, lowercase_rest(rest))
        }
        (Some(icon), _) => format!("{icon} {}", group.count),
        (None, _) => format!("{} {}", group.count, group.label),
    }
}

fn split_icon(label: &str) -> (Option<&str>, &str) {
    let trimmed = label.trim();
    let mut chars = trimmed.char_indices();
    let Some((_, first)) = chars.next() else {
        return (None, trimmed);
    };
    if first.is_ascii_alphanumeric() {
        return (None, trimmed);
    }
    let mut end = first.len_utf8();
    for (index, character) in chars {
        if character.is_whitespace() || character.is_ascii_alphanumeric() {
            break;
        }
        end = index + character.len_utf8();
    }
    let icon = trimmed[..end].trim();
    let rest = trimmed[end..].trim();
    if icon.is_empty() {
        (None, trimmed)
    } else {
        (Some(icon), rest)
    }
}

fn lowercase_rest(rest: &str) -> String {
    if rest
        .as_bytes()
        .get(0..2)
        .is_some_and(|prefix| prefix.iter().all(u8::is_ascii_uppercase))
    {
        rest.to_owned()
    } else {
        let mut chars = rest.chars();
        chars
            .next()
            .map(|first| first.to_lowercase().chain(chars).collect())
            .unwrap_or_default()
    }
}

fn age_text(last_record: Option<&str>) -> Option<String> {
    let raw = last_record?;
    let date = NaiveDate::parse_from_str(raw, "%Y-%m-%d").ok()?;
    let today = bob_env::current_datetime().date();
    let days = (today - date).num_days();
    if days < 0 {
        return Some(raw.to_string());
    }
    if days == 0 {
        return Some("today".to_string());
    }
    if days < 7 {
        return Some(format!("{days}d"));
    }
    Some(format!("{}w", days / 7))
}

fn issue_marker(entry: &AlbumReportEntry) -> String {
    let errors = entry
        .issues
        .iter()
        .filter(|issue| issue.severity == AlbumIssueSeverity::Error)
        .count();
    let warnings = entry
        .issues
        .iter()
        .filter(|issue| issue.severity == AlbumIssueSeverity::Warning)
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

fn hue_for_root<S>(resolver: &mut HomeResolver<S>, root: &str) -> Option<u8>
where
    S: crate::native::pomodoro_homes::HomeSource,
{
    let path = PathBuf::from(format!("{root}.md"));
    let home = resolver.resolve_note(&path);
    home.root_hue.map(|hue| hue.xterm())
}

pub(crate) fn print_check(report: &AlbumReport) {
    let styler = Styler::detect();
    let mut any = false;
    let mut entries: Vec<&AlbumReportEntry> = report.albums.iter().collect();
    entries.sort_by(|left, right| left.path.cmp(&right.path));
    for entry in entries {
        if entry.issues.is_empty() {
            continue;
        }
        any = true;
        let header = format!("{} — {}", entry.path, entry.title);
        println!("{}", styler.bold(&header));
        print_issues(&entry.issues, &styler);
        println!();
    }
    let counts = &report.counts;
    let summary = format!(
        "{} albums, {} records — {} errors, {} warnings",
        counts.albums, counts.records, counts.errors, counts.warnings
    );
    if counts.errors > 0 {
        println!("{}", styler.red(&format!("✗ {summary}")));
    } else if any || counts.warnings > 0 {
        println!("{}", styler.yellow(&format!("✓ {summary}")));
    } else {
        println!("{}", styler.green(&format!("✓ {summary}")));
    }
}

fn print_issues(issues: &[AlbumIssue], styler: &Styler) {
    for issue in issues {
        let severity = match issue.severity {
            AlbumIssueSeverity::Error => styler.red("error"),
            AlbumIssueSeverity::Warning => styler.yellow("warning"),
            AlbumIssueSeverity::Info => styler.dim("info"),
        };
        let location = issue_location(issue);
        println!(
            "  {location}: [{code}/{severity}] {message}",
            code = issue.code,
            message = issue.message
        );
        if !issue.suggestions.is_empty() {
            println!("    suggestions: {}", issue.suggestions.join(", "));
        }
    }
}

fn issue_location(issue: &AlbumIssue) -> String {
    match (issue.line, issue.key_path.as_deref()) {
        (Some(line), Some(key)) => format!("{}:{line} {key}", issue.path),
        (Some(line), None) => format!("{}:{line}", issue.path),
        (None, Some(key)) => format!("{} {key}", issue.path),
        (None, None) => issue.path.clone(),
    }
}
