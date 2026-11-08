//! `bob gmail [list]`: list saved mail notes without touching the network.
//!
//! Both the bare command and the explicit `list` subcommand run this offline
//! read. It scans only mail-note frontmatter (never bodies), counts distinct
//! source notes that link to each saved thread in one vault pass, and sorts
//! by parsed `updated` instant descending with a stable path tie break.
//!
//! The `backlinks` count means: the number of distinct vault notes (outside
//! fenced/inline code) whose `[[target]]` wikilink resolves to the saved
//! mail note. Aliases (`[[target|label]]`) and heading/block targets
//! (`[[target#heading]]`, `[[target#^block]]`) resolve through their target;
//! self-links never count. Duplicate identities are never selected as
//! canonical: they are reported as diagnostics instead.
//!
//! The footer is owned by the `gmail-link` phase: waiting links that still
//! resolve offline plus remembered-dead links, folded from the failure
//! journal. It stays absent when both are zero rather than claiming zero
//! links.

use std::{
    collections::{BTreeMap, BTreeSet},
    ffi::OsStr,
    fs,
    path::{Path, PathBuf},
};

use chrono::{DateTime, FixedOffset};
use serde_json::json;

use super::{
    cli::ListArgs,
    config::GmailConfig,
    gog::Gog,
    ids,
    note::index::{MailIndex, MailIndexEntry},
    resolve, search_peek, ui, GmailError,
};
use crate::native::{
    is_always_excluded_note_directory_name, style, style::Styler,
};

/// Newest rows shown unless `--all` passes.
const PAGE_SIZE: usize = 20;

struct ListRow {
    entry: MailIndexEntry,
    updated: Option<DateTime<FixedOffset>>,
    backlinks: usize,
    new_messages: Option<usize>,
}

struct ListReport {
    rows: Vec<ListRow>,
    total: usize,
    truncated: bool,
    duplicates: Vec<(String, String, Vec<String>)>,
    skipped_malformed: usize,
    /// Waiting/gone footer folded by the `gmail-link` phase.
    footer: Option<String>,
}

/// Run the offline list. Never invokes gog, creates directories, changes
/// files, or needs the writer host.
pub(crate) fn run(args: &ListArgs) -> i32 {
    match report(args) {
        Ok(report) => {
            print_report(args, &report);
            0
        }
        Err(error) => ui::report_error(
            "list",
            &error,
            if args.format.is_json() {
                "json"
            } else {
                "human"
            },
        ),
    }
}

fn report(args: &ListArgs) -> Result<ListReport, GmailError> {
    let config = GmailConfig::resolve()?;
    let vault = args.bob_dir();
    if !vault.is_dir() {
        return Err(GmailError::setup(
            "vault",
            format!("Bob vault root does not exist: {}", vault.display()),
        ));
    }
    let folder = Path::new(config.folder());
    let index = resolve::scan_index(&vault, folder, config.link_label())?;
    let backlinks = count_backlinks(&vault, &index);
    let rows = sorted_rows(&index, config.account(), &backlinks);
    let total = rows.len();
    let (mut rows, truncated) = if args.all {
        (rows, false)
    } else if rows.len() > PAGE_SIZE {
        (rows.into_iter().take(PAGE_SIZE).collect(), true)
    } else {
        (rows, false)
    };
    if args.remote {
        let gog = Gog::resolve(
            crate::native::google::GMAIL,
            config.gog(),
            config.timeout(),
        )?;
        gog.require_version()?;
        for row in &mut rows {
            let live = search_peek::remote_message_count(
                &gog,
                &row.entry.account,
                &row.entry.thread_id,
            )?;
            row.new_messages =
                Some(live.saturating_sub(row.entry.messages as usize));
        }
    }
    Ok(ListReport {
        rows,
        total,
        truncated,
        duplicates: index.duplicates.clone(),
        skipped_malformed: index.skipped_malformed,
        footer: super::link::waiting_gone_footer(&vault, folder),
    })
}

fn sorted_rows(
    index: &MailIndex,
    account: Option<&str>,
    backlinks: &BTreeMap<String, usize>,
) -> Vec<ListRow> {
    let mut rows: Vec<ListRow> = index
        .by_identity
        .values()
        .filter(|entry| {
            account.is_none_or(|want| {
                entry.account.eq_ignore_ascii_case(want.trim())
            })
        })
        .map(|entry| {
            let updated = parse_instant(&entry.updated);
            let count = backlinks.get(&entry.path).copied().unwrap_or(0);
            ListRow {
                entry: entry.clone(),
                updated,
                backlinks: count,
                new_messages: None,
            }
        })
        .collect();
    rows.sort_by(|left, right| {
        right
            .updated
            .cmp(&left.updated)
            .then_with(|| left.entry.path.cmp(&right.entry.path))
    });
    rows
}

fn parse_instant(raw: &str) -> Option<DateTime<FixedOffset>> {
    if raw.trim().is_empty() {
        return None;
    }
    DateTime::parse_from_rfc3339(raw.trim()).ok()
}

fn display_date(row: &ListRow) -> String {
    match row.updated {
        Some(instant) => instant.format("%Y-%m-%d").to_string(),
        None => "—".to_string(),
    }
}

/// Count distinct source notes linking to each indexed mail path with one
/// vault pass. Aliases and heading/block targets resolve through their
/// link target; self-links and code spans never count.
fn count_backlinks(vault: &Path, index: &MailIndex) -> BTreeMap<String, usize> {
    let mut by_path: BTreeMap<String, String> = BTreeMap::new();
    let mut by_stem: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for entry in index.by_identity.values() {
        let key = strip_markdown_extension(&entry.path).to_ascii_lowercase();
        by_path.insert(key, entry.path.clone());
        let stem = note_stem(&entry.path).to_ascii_lowercase();
        by_stem.entry(stem).or_default().push(entry.path.clone());
    }
    if by_path.is_empty() {
        return BTreeMap::new();
    }
    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    for source in vault_markdown_files(vault) {
        let Ok(relative) = source.strip_prefix(vault) else {
            continue;
        };
        let source_path = path_to_slashes(relative);
        let Ok(contents) = fs::read_to_string(&source) else {
            continue;
        };
        let mut linked = BTreeSet::new();
        for target in wikilink_targets(&contents) {
            if let Some(path) = by_path.get(&target) {
                if *path != source_path {
                    linked.insert(path.clone());
                }
                continue;
            }
            if let Some(paths) = by_stem.get(&target) {
                for path in paths {
                    if *path != source_path {
                        linked.insert(path.clone());
                    }
                }
            }
        }
        for path in linked {
            *counts.entry(path).or_default() += 1;
        }
    }
    counts
}

fn vault_markdown_files(vault: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    collect_markdown(vault, &mut files);
    files.sort();
    files
}

fn collect_markdown(directory: &Path, files: &mut Vec<PathBuf>) {
    let entries = match fs::read_dir(directory) {
        Ok(entries) => entries.collect::<Result<Vec<_>, _>>(),
        Err(_) => return,
    };
    let Ok(entries) = entries else {
        return;
    };
    for entry in entries {
        let path = entry.path();
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if file_type.is_symlink() {
            continue;
        }
        if file_type.is_dir() {
            if is_excluded_directory(&entry.file_name()) {
                continue;
            }
            collect_markdown(&path, files);
        } else if file_type.is_file()
            && path
                .extension()
                .is_some_and(|extension| extension.eq_ignore_ascii_case("md"))
        {
            files.push(path);
        }
    }
}

fn is_excluded_directory(name: &OsStr) -> bool {
    name.to_string_lossy().starts_with('.')
        || is_always_excluded_note_directory_name(name)
}

/// Every `[[target]]` link target in Markdown source, with aliases and
/// heading/block suffixes stripped and normalized for comparison.
/// Skips frontmatter, fenced code blocks, and inline code spans.
pub(crate) fn wikilink_targets(markdown: &str) -> Vec<String> {
    let mut targets = Vec::new();
    let mut in_frontmatter = false;
    let mut frontmatter_done = markdown.is_empty();
    let mut fence: Option<(char, usize)> = None;
    for (position, line) in markdown.split_inclusive('\n').enumerate() {
        let content = line.strip_suffix('\n').unwrap_or(line);
        let content = content.strip_suffix('\r').unwrap_or(content);
        if position == 0 && content.trim() == "---" {
            in_frontmatter = true;
            continue;
        }
        if in_frontmatter {
            if content.trim() == "---" {
                in_frontmatter = false;
                frontmatter_done = true;
            }
            continue;
        }
        if !frontmatter_done && position == 0 {
            frontmatter_done = true;
        }
        if let Some((character, length)) = fence_marker(content) {
            match fence {
                Some((open, open_length))
                    if character == open && length >= open_length =>
                {
                    fence = None;
                }
                None => fence = Some((character, length)),
                _ => {}
            }
            continue;
        }
        if fence.is_some() {
            continue;
        }
        targets.extend(line_link_targets(content));
    }
    targets
}

fn fence_marker(line: &str) -> Option<(char, usize)> {
    let trimmed = line.trim_start();
    if line.len() - trimmed.len() > 3 {
        return None;
    }
    let character = trimmed.chars().next()?;
    if !matches!(character, '`' | '~') {
        return None;
    }
    let length = trimmed.chars().take_while(|c| *c == character).count();
    (length >= 3).then_some((character, length))
}

/// `[[target]]` targets on one physical line, ignoring inline code spans.
fn line_link_targets(line: &str) -> Vec<String> {
    let mut targets = Vec::new();
    let visible = strip_inline_code(line);
    let bytes = visible.as_bytes();
    let mut cursor = 0;
    while cursor + 1 < bytes.len() {
        if bytes[cursor] == b'[' && bytes[cursor + 1] == b'[' {
            let inner_start = cursor + 2;
            if let Some(close) = visible[inner_start..].find("]]") {
                let inner = &visible[inner_start..inner_start + close];
                if let Some(normalized) = normalize_link_target(inner) {
                    targets.push(normalized);
                }
                cursor = inner_start + close + 2;
                continue;
            }
            break;
        }
        cursor += 1;
    }
    targets
}

/// Replace inline code spans with blanks so links inside them never count.
/// A span opens and closes on the same backtick run length.
fn strip_inline_code(line: &str) -> String {
    let bytes = line.as_bytes();
    let mut masked = vec![false; bytes.len()];
    let mut cursor = 0;
    while cursor < bytes.len() {
        if bytes[cursor] != b'`' {
            cursor += 1;
            continue;
        }
        let mut run = 0;
        while bytes.get(cursor + run) == Some(&b'`') {
            run += 1;
        }
        let mut close = cursor + run;
        let mut found = None;
        while close < bytes.len() {
            if bytes[close] == b'`' {
                let mut close_run = 0;
                while bytes.get(close + close_run) == Some(&b'`') {
                    close_run += 1;
                }
                if close_run == run {
                    found = Some(close + close_run);
                    break;
                }
                close += close_run.max(1);
                continue;
            }
            close += 1;
        }
        let end = found.unwrap_or(bytes.len());
        for index in cursor..end {
            masked[index] = true;
        }
        cursor = end;
    }
    line.char_indices()
        .map(
            |(index, character)| {
                if masked[index] {
                    ' '
                } else {
                    character
                }
            },
        )
        .collect()
}

/// Strip the `|alias` and `#heading`/`#^block` suffixes and normalize for
/// comparison: vault-relative paths keep their directory, bare names match
/// note stems. Returns `None` for empty targets.
fn normalize_link_target(inner: &str) -> Option<String> {
    let before_alias = inner.split('|').next().unwrap_or("").trim();
    let before_anchor = before_alias.split('#').next().unwrap_or("").trim();
    let target = before_anchor.trim().trim_start_matches("./");
    let target = target.strip_prefix('/').unwrap_or(target);
    if target.is_empty() {
        return None;
    }
    let lowered = target.to_ascii_lowercase();
    Some(strip_markdown_extension(&lowered).to_string())
}

fn strip_markdown_extension(path: &str) -> &str {
    path.strip_suffix(".md").unwrap_or(path)
}

fn note_stem(path: &str) -> &str {
    strip_markdown_extension(path.rsplit('/').next().unwrap_or(path))
}

fn path_to_slashes(path: &Path) -> String {
    path.components()
        .map(|component| component.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

fn print_report(args: &ListArgs, report: &ListReport) {
    if args.format.is_json() {
        let mut document = json!({
            "schema_version": 1,
            "ok": true,
            "total": report.total,
            "shown": report.rows.len(),
            "truncated": report.truncated,
            "threads": report.rows.iter().map(|row| {
                let mut thread = json!({
                    "account": row.entry.account,
                    "thread_id": row.entry.thread_id,
                    "path": row.entry.path,
                    "subject": row.entry.title,
                    "who": row.entry.who,
                    "label": row.entry.label,
                    "link": format!("[[{}|{}]]", row.entry.path, row.entry.label),
                    "started": row.entry.started,
                    "updated": row.entry.updated,
                    "messages": row.entry.messages,
                    "turn": row.entry.turn,
                    "turn_since": row.entry.turn_since,
                    "backlinks": row.backlinks,
                });
                if args.remote {
                    thread["new_messages"] = json!(row.new_messages.unwrap_or_default());
                }
                thread
            }).collect::<Vec<_>>(),
            "duplicates": report.duplicates.iter().map(|(account, thread, paths)| json!({
                "account": account,
                "thread_id": thread,
                "paths": paths,
            })).collect::<Vec<_>>(),
            "skipped_malformed": report.skipped_malformed,
        });
        if let Some(footer) = &report.footer {
            document["footer"] = json!(footer);
        }
        println!("{document}");
        ui::flush_stdout();
        return;
    }
    let styler = Styler::detect();
    if report.rows.is_empty() {
        println!("No saved threads yet.");
        println!("Save a thread with `bob gmail save`, then list it here.");
    } else {
        let headers = ["updated", "subject", "who", "messages", "backlinks"];
        let rows: Vec<[String; 5]> = report
            .rows
            .iter()
            .map(|row| {
                [
                    display_date(row),
                    row.entry.title.clone(),
                    row.entry.who.clone(),
                    match row.new_messages {
                        Some(new) if new > 0 => {
                            format!("{} (+{new} new)", row.entry.messages)
                        }
                        _ => row.entry.messages.to_string(),
                    },
                    row.backlinks.to_string(),
                ]
            })
            .collect();
        let widths: Vec<usize> = (0..headers.len())
            .map(|column| {
                rows.iter()
                    .map(|row| style::display_width(&row[column]))
                    .chain(std::iter::once(headers[column].len()))
                    .max()
                    .unwrap_or(headers[column].len())
            })
            .collect();
        println!(
            "{}",
            styler.dim(
                &headers
                    .iter()
                    .zip(widths.iter())
                    .map(|(header, width)| style::pad_right(header, *width))
                    .collect::<Vec<_>>()
                    .join("  ")
            )
        );
        for row in &rows {
            println!(
                "{}",
                row.iter()
                    .zip(widths.iter())
                    .map(|(cell, width)| style::pad_right(cell, *width))
                    .collect::<Vec<_>>()
                    .join("  ")
            );
        }
        if report.truncated {
            println!(
                "{}",
                styler.dim(&format!(
                    "showing {} of {}; pass -a to show all",
                    report.rows.len(),
                    report.total
                ))
            );
        }
    }
    for (account, thread, paths) in &report.duplicates {
        println!(
            "{}",
            styler.dim(&format!(
                "duplicate identity {account}/{thread} in {} notes: {}",
                paths.len(),
                paths.join(", ")
            ))
        );
    }
    if report.skipped_malformed > 0 {
        println!(
            "{}",
            styler.dim(&format!(
                "skipped {} malformed mail note{}",
                report.skipped_malformed,
                if report.skipped_malformed == 1 {
                    ""
                } else {
                    "s"
                }
            ))
        );
    }
    if let Some(footer) = &report.footer {
        println!("{}", styler.dim(footer));
    }
    ui::flush_stdout();
}

/// Decoder health for `bob gmail doctor`: a census of Gmail URLs across
/// vault notes with resolvable/refused/unknown counts. Body-free by
/// construction: only counts and reason tokens appear.
pub(crate) fn url_health(vault: &Path) -> (usize, usize, usize, usize) {
    let mut total = 0usize;
    let mut resolvable = 0usize;
    let mut refused = 0usize;
    let mut unknown = 0usize;
    for path in vault_markdown_files(vault) {
        let Ok(contents) = fs::read_to_string(&path) else {
            continue;
        };
        for found in ids::find_gmail_urls(&contents) {
            total += 1;
            match found.verdict {
                ids::GmailVerdict::Resolvable(_) => resolvable += 1,
                ids::GmailVerdict::Refused { .. } => refused += 1,
                ids::GmailVerdict::Unresolvable { .. } => unknown += 1,
            }
        }
    }
    (total, resolvable, refused, unknown)
}

/// Readable stream wrapper so doctor tests can exercise the pure summary.
pub(crate) fn url_summary(
    total: usize,
    resolvable: usize,
    refused: usize,
    unknown: usize,
) -> String {
    if total == 0 {
        return "no Gmail URLs in vault notes".to_string();
    }
    let share = resolvable * 100 / total;
    let mut summary =
        format!("{total} Gmail URLs · {resolvable} resolvable ({share}%)");
    if refused > 0 {
        summary.push_str(&format!(" · {refused} refused"));
    }
    if unknown > 0 {
        summary.push_str(&format!(" · {unknown} unknown token"));
        if unknown > 1 {
            summary.push('s');
        }
    }
    summary
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mail_note(
        account: &str,
        thread: &str,
        title: &str,
        who: &str,
        updated: &str,
        messages: u64,
        path: &str,
    ) -> MailIndexEntry {
        MailIndexEntry {
            account: account.to_string(),
            thread_id: thread.to_string(),
            path: path.to_string(),
            title: title.to_string(),
            who: who.to_string(),
            label: format!("{title} · {who}"),
            started: "2026-09-19T13:46:32+00:00".to_string(),
            updated: updated.to_string(),
            messages,
            turn: None,
            turn_since: None,
        }
    }

    fn note_file(vault: &Path, relative: &str, contents: &str) {
        let path = vault.join(relative);
        fs::create_dir_all(path.parent().expect("parent")).unwrap();
        fs::write(path, contents).unwrap();
    }

    fn frontmatter(
        account: &str,
        thread: &str,
        title: &str,
        who: &str,
        updated: &str,
    ) -> String {
        format!(
            "---\ntype: \"[[mail]]\"\ncontent: untrusted-email\ngmail_account: {account}\ngmail_thread_id: {thread}\ntitle: {title}\nwho: {who}\nstarted: 2026-09-19T13:46:32+00:00\nupdated: {updated}\nmessages: 2\n---\nbody\n"
        )
    }

    #[test]
    fn sorts_by_updated_descending_with_path_tiebreak() {
        let backlinks = BTreeMap::new();
        let mut index = MailIndex::default();
        for (thread, updated, path) in [
            ("aaaa", "2026-09-19T13:46:32+00:00", "mail/b.md"),
            ("bbbb", "2026-09-20T13:46:32+00:00", "mail/a.md"),
            ("cccc", "2026-09-20T13:46:32+00:00", "mail/c.md"),
            ("dddd", "not-a-date", "mail/d.md"),
        ] {
            index.by_identity.insert(
                ("me@example.com".to_string(), thread.to_string()),
                mail_note(
                    "me@example.com",
                    thread,
                    "Subject",
                    "Dana",
                    updated,
                    2,
                    path,
                ),
            );
        }
        let rows = sorted_rows(&index, None, &backlinks);
        let paths: Vec<_> =
            rows.iter().map(|row| row.entry.path.as_str()).collect();
        assert_eq!(
            paths,
            vec!["mail/a.md", "mail/c.md", "mail/b.md", "mail/d.md"]
        );
    }

    #[test]
    fn account_filter_is_case_insensitive() {
        let backlinks = BTreeMap::new();
        let mut index = MailIndex::default();
        for account in ["me@example.com", "YOU@example.com"] {
            index.by_identity.insert(
                (account.to_string(), "aaaa".to_string()),
                mail_note(
                    account,
                    "aaaa",
                    "Subject",
                    "Dana",
                    "2026-09-20T13:46:32+00:00",
                    1,
                    &format!("mail/{account}.md"),
                ),
            );
        }
        let rows = sorted_rows(&index, Some("YOU@EXAMPLE.COM"), &backlinks);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].entry.account, "YOU@example.com");
    }

    #[test]
    fn backlinks_count_distinct_sources_and_ignore_code_self_and_alias_forms() {
        let temp = tempfile::tempdir().unwrap();
        let vault = temp.path();
        note_file(
            vault,
            "mail/2026/lease.md",
            &frontmatter(
                "me@example.com",
                "aabbccdd",
                "Lease",
                "Dana",
                "2026-09-20T13:46:32+00:00",
            ),
        );
        // Alias, heading, block, stem, and path forms all resolve.
        note_file(vault, "tasks.md", "See [[mail/2026/lease|the lease]] and [[lease#Details]] and [[lease#^abc123]].\n");
        // Bare path and .md-suffixed path forms.
        note_file(
            vault,
            "other.md",
            "Ref [[mail/2026/lease]] plus [[mail/2026/lease.md]].\n",
        );
        // A bare stem in its own source note resolves without the directory.
        note_file(vault, "stem-only.md", "A bare [[lease]] link.\n");
        // Code spans and fenced blocks never count.
        note_file(
            vault,
            "code.md",
            "Inline `[[lease]]` is code.\n\n```\n[[lease]]\n```\n",
        );
        // Self-links never count.
        note_file(
            vault,
            "mail/2026/lease.md",
            &format!(
                "{}\n\nSelf [[lease]] stays out.\n",
                frontmatter(
                    "me@example.com",
                    "aabbccdd",
                    "Lease",
                    "Dana",
                    "2026-09-20T13:46:32+00:00",
                )
            ),
        );
        // Frontmatter links never count.
        note_file(
            vault,
            "meta.md",
            "---\nlink: \"[[lease]]\"\n---\n\nNo body links.\n",
        );
        let index = super::super::note::index::scan(
            vault,
            Path::new("mail"),
            "✉️ {subject} · {who}",
        )
        .unwrap();
        let counts = count_backlinks(vault, &index);
        assert_eq!(counts.get("mail/2026/lease.md"), Some(&3));
    }

    #[test]
    fn url_summary_reports_zero_and_mixed_censuses() {
        assert_eq!(url_summary(0, 0, 0, 0), "no Gmail URLs in vault notes");
        assert_eq!(
            url_summary(14, 12, 1, 1),
            "14 Gmail URLs · 12 resolvable (85%) · 1 refused · 1 unknown token"
        );
        assert_eq!(
            url_summary(4, 4, 0, 0),
            "4 Gmail URLs · 4 resolvable (100%)"
        );
    }
}
