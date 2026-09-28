//! Vault ledger, journal, and target-note task reader.
//!
//! The ledger scans every vault `.md` file (including `done/`) for
//! `%%gkeep:v1:…%%` markers, so re-runs stay idempotent across hosts with
//! no local state. The journal is the backstop for markers deleted during
//! triage: a `written` record counts as a lower-priority ledger entry.

use std::{
    collections::BTreeMap,
    fs, io,
    path::{Path, PathBuf},
    sync::LazyLock,
};

use chrono::NaiveDate;
use regex::Regex;
use serde::{Deserialize, Serialize};

use crate::native::note_tasks::NoteTaskSettings;
use crate::native::{
    is_always_excluded_note_directory_name, note_tasks, task_fields,
};

static MARKER_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"%%gkeep:v1:([A-Za-z0-9._%-]+):([0-9a-f]{12})%%")
        .expect("valid gkeep marker regex")
});

/// Format a vault marker for `id` with content fingerprint `fp`.
///
/// Id bytes outside `[A-Za-z0-9._-]` are percent-encoded (uppercase hex),
/// so the raw id — dots included — never lands in a block id.
pub(super) fn format_marker(id: &str, fp: &str) -> String {
    format!("%%gkeep:v1:{}:{fp}%%", percent_encode_id(id))
}

/// Every `(id, fp)` marker on `line`, in line order.
///
/// Markers whose id fails percent-decoding are skipped; `format_marker`
/// never emits a bare `%`, so those cannot come from this tool.
pub(super) fn parse_markers(line: &str) -> Vec<(String, String)> {
    MARKER_RE
        .captures_iter(line)
        .filter_map(|captures| {
            let encoded = captures.get(1)?.as_str();
            let fp = captures.get(2)?.as_str().to_string();
            percent_decode_id(encoded).map(|id| (id, fp))
        })
        .collect()
}

fn percent_encode_id(id: &str) -> String {
    let mut encoded = String::with_capacity(id.len());
    for byte in id.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-') {
            encoded.push(byte as char);
        } else {
            encoded.push_str(&format!("%{byte:02X}"));
        }
    }
    encoded
}

fn percent_decode_id(encoded: &str) -> Option<String> {
    let bytes = encoded.as_bytes();
    let mut raw = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            let digits = encoded.get(index + 1..index + 3)?;
            let byte = u8::from_str_radix(digits, 16).ok()?;
            raw.push(byte);
            index += 3;
        } else {
            raw.push(bytes[index]);
            index += 1;
        }
    }
    String::from_utf8(raw).ok()
}

/// One marker hit: vault-relative `path` and 1-based `line`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct LedgerEntry {
    pub(super) id: String,
    pub(super) fp: String,
    pub(super) path: String,
    pub(super) line: usize,
}

/// The vault-wide marker index, in `(path, line)` order.
#[derive(Debug, Clone, Default)]
pub(super) struct Ledger {
    pub(super) entries: Vec<LedgerEntry>,
}

impl Ledger {
    /// Scan every `.md` file under `bob_dir`, including `done/`, while
    /// skipping the always-excluded note directories.
    ///
    /// Files without the `%%gkeep:` pre-filter are never opened. Files
    /// that fail to read are skipped: a missed marker classifies a note
    /// as new and writes a duplicate, which the pull invariant prefers
    /// over data loss.
    pub(super) fn scan(bob_dir: &Path) -> io::Result<Ledger> {
        let mut entries = Vec::new();
        visit_dir(bob_dir, bob_dir, &mut entries)?;
        entries.sort_by(|left, right| {
            (&left.path, left.line).cmp(&(&right.path, right.line))
        });
        Ok(Ledger { entries })
    }

    /// Whether the exact `(id, fp)` pair is already in the vault.
    pub(super) fn has(&self, id: &str, fp: &str) -> bool {
        self.entries
            .iter()
            .any(|entry| entry.id == id && entry.fp == fp)
    }

    /// Whether any fingerprint of `id` is already in the vault.
    pub(super) fn has_id(&self, id: &str) -> bool {
        self.entries.iter().any(|entry| entry.id == id)
    }

    /// Groups with more than one entry for the same `(id, fp)` pair.
    pub(super) fn duplicates(&self) -> Vec<DuplicateGroup> {
        let mut by_pair: BTreeMap<(&str, &str), Vec<String>> = BTreeMap::new();
        for entry in &self.entries {
            by_pair
                .entry((entry.id.as_str(), entry.fp.as_str()))
                .or_default()
                .push(format!("{}:{}", entry.path, entry.line));
        }
        by_pair
            .into_iter()
            .filter(|(_, locations)| locations.len() > 1)
            .map(|((id, fp), locations)| DuplicateGroup {
                id: id.to_string(),
                fp: fp.to_string(),
                locations,
            })
            .collect()
    }
}

/// One `(id, fp)` pair recorded at several `path:line` locations.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct DuplicateGroup {
    pub(super) id: String,
    pub(super) fp: String,
    pub(super) locations: Vec<String>,
}

fn visit_dir(
    dir: &Path,
    bob_dir: &Path,
    entries: &mut Vec<LedgerEntry>,
) -> io::Result<()> {
    let mut names: Vec<PathBuf> = Vec::new();
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let file_type = entry.file_type()?;
        if file_type.is_dir() {
            if is_always_excluded_note_directory_name(&entry.file_name()) {
                continue;
            }
            visit_dir(&entry.path(), bob_dir, entries)?;
        } else if file_type.is_file() {
            names.push(entry.path());
        }
    }
    names.sort();
    for path in names {
        if path.extension().is_none_or(|extension| extension != "md") {
            continue;
        }
        scan_file(bob_dir, &path, entries);
    }
    Ok(())
}

fn scan_file(bob_dir: &Path, path: &Path, entries: &mut Vec<LedgerEntry>) {
    let Ok(contents) = fs::read_to_string(path) else {
        return;
    };
    if !contents.contains("%%gkeep:") {
        return;
    }
    let relative = path.strip_prefix(bob_dir).map_or_else(
        |_| path.to_string_lossy().replace('\\', "/"),
        |relative| relative.to_string_lossy().replace('\\', "/"),
    );
    for (index, line) in contents.lines().enumerate() {
        for (id, fp) in parse_markers(line) {
            entries.push(LedgerEntry {
                id,
                fp,
                path: relative.clone(),
                line: index + 1,
            });
        }
    }
}

/// A journal event. Only `written` feeds the planner; `archived` and
/// `archive_refused` are audit history for `pull`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum JournalEvent {
    Written,
    Archived,
    ArchiveRefused,
}

/// One journal line. It never stores note bodies.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct JournalRecord {
    pub(super) ts: String,
    pub(super) event: JournalEvent,
    pub(super) id: String,
    #[serde(rename = "ref")]
    pub(super) ref_: String,
    pub(super) fp: String,
    pub(super) path: String,
    #[serde(default)]
    pub(super) commit: Option<String>,
    #[serde(default)]
    pub(super) status: Option<String>,
}

/// The append-only journal plus the corrupt-line count from the last read.
#[derive(Debug, Clone, Default)]
pub(super) struct Journal {
    pub(super) records: Vec<JournalRecord>,
    pub(super) skipped: usize,
}

impl Journal {
    /// Read the journal at `path`. A missing file gives an empty journal;
    /// corrupt lines (including non-UTF8) are skipped and counted.
    pub(super) fn read(path: &Path) -> io::Result<Journal> {
        let bytes = match fs::read(path) {
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return Ok(Journal::default());
            }
            Err(error) => return Err(error),
            Ok(bytes) => bytes,
        };
        let mut journal = Journal::default();
        // Split on newlines at the byte level so invalid UTF-8 lines can
        // be skipped without failing the whole read.
        let mut start = 0;
        for (i, b) in bytes.iter().enumerate().chain([(bytes.len(), &b'\n')]) {
            if *b != b'\n' && i != bytes.len() {
                continue;
            }
            let end = if i == bytes.len() { bytes.len() } else { i };
            let slice = &bytes[start..end];
            start = end + 1;
            let Ok(line) = std::str::from_utf8(slice) else {
                journal.skipped += 1;
                continue;
            };
            if line.trim().is_empty() {
                continue;
            }
            match serde_json::from_str::<JournalRecord>(line) {
                Ok(record) => journal.records.push(record),
                Err(_) => journal.skipped += 1,
            }
        }
        Ok(journal)
    }

    /// Append `records` as one fsynced batch, creating the parent
    /// directory (`0700`) and the file (`0600` at open) as needed. When
    /// the existing file is non-empty and lacks a trailing newline, a
    /// leading newline is written first so a torn previous append cannot
    /// corrupt the next record.
    pub(super) fn append(
        path: &Path,
        records: &[JournalRecord],
    ) -> io::Result<()> {
        if let Some(parent) = path.parent()
            && !parent.as_os_str().is_empty()
        {
            fs::create_dir_all(parent)?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt as _;
                fs::set_permissions(parent, fs::Permissions::from_mode(0o700))?;
            }
        }
        let needs_leading_newline = match fs::read(path) {
            Err(error) if error.kind() == io::ErrorKind::NotFound => false,
            Err(_) => false,
            Ok(bytes) => !bytes.is_empty() && !bytes.ends_with(b"\n"),
        };
        let mut options = fs::OpenOptions::new();
        options.create(true).append(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt as _;
            options.mode(0o600);
        }
        let file = options.open(path)?;
        #[cfg(unix)]
        {
            // Mode at open covers creation; tighten existing files too,
            // before writing any bytes.
            use std::os::unix::fs::PermissionsExt as _;
            file.set_permissions(fs::Permissions::from_mode(0o600))?;
        }
        let mut file = file;
        if needs_leading_newline {
            io::Write::write_all(&mut file, b"\n")?;
        }
        for record in records {
            let mut line =
                serde_json::to_string(record).map_err(io::Error::other)?;
            line.push('\n');
            io::Write::write_all(&mut file, line.as_bytes())?;
        }
        file.sync_all()
    }

    /// Whether a `written` record covers the exact `(id, fp)` pair.
    pub(super) fn has(&self, id: &str, fp: &str) -> bool {
        self.records.iter().any(|record| {
            record.event == JournalEvent::Written
                && record.id == id
                && record.fp == fp
        })
    }

    /// Whether a `written` record covers any fingerprint of `id`.
    pub(super) fn has_id(&self, id: &str) -> bool {
        self.records.iter().any(|record| {
            record.event == JournalEvent::Written && record.id == id
        })
    }
}

/// One top-level vault task in the target note.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct VaultTask {
    /// 1-based line of the task.
    pub(super) line: usize,
    pub(super) status_symbol: char,
    pub(super) description: String,
    /// The `[created::…]` date on the task line, if it parses.
    pub(super) created: Option<NaiveDate>,
    /// Unchecked checkbox descendants.
    pub(super) open_items: usize,
    /// Checked checkbox descendants.
    pub(super) checked_items: usize,
    /// The first gkeep marker inside the task's block, if any.
    pub(super) marker: Option<(String, String)>,
}

static CHECKBOX_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^\s*-\s*\[([^\]])\]").expect("valid checkbox regex")
});

/// Read the top-level tasks from target-note `contents`.
///
/// Done and cancelled tasks are included: `list --all` shows them.
/// Checkbox counts cover every descendant line; the marker is the first
/// gkeep marker anywhere in the task's block.
pub(super) fn read_target_tasks(
    contents: &str,
    settings: &NoteTaskSettings,
) -> Vec<VaultTask> {
    let scan = note_tasks::scan(contents, settings);
    let lines: Vec<&str> = contents
        .split_terminator('\n')
        .map(|line| line.strip_suffix('\r').unwrap_or(line))
        .collect();
    scan.tasks()
        .iter()
        .filter(|task| task.indentation.is_empty())
        .map(|task| {
            let block = task_block(&lines, task.line_index);
            let marker = block
                .iter()
                .find_map(|line| parse_markers(line).into_iter().next());
            let (open_items, checked_items) =
                checkbox_counts(&block[1.min(block.len())..]);
            VaultTask {
                line: task.line_index + 1,
                status_symbol: task.status_symbol,
                description: task.description.clone(),
                created: task_created(&block[0]),
                open_items,
                checked_items,
                marker,
            }
        })
        .collect()
}

/// The task line plus its indented descendants, mirroring the block
/// extent in `note_tasks` (blank lines continue only into indentation).
fn task_block(lines: &[&str], start: usize) -> Vec<String> {
    let mut block = vec![lines[start].to_string()];
    let mut index = start + 1;
    while index < lines.len() {
        if lines[index].trim().is_empty() {
            let continues = lines[index + 1..]
                .iter()
                .find(|line| !line.trim().is_empty())
                .is_some_and(|next| indentation_len(next) > 0);
            if continues {
                block.push(lines[index].to_string());
                index += 1;
                continue;
            }
            break;
        }
        if indentation_len(lines[index]) == 0 {
            break;
        }
        block.push(lines[index].to_string());
        index += 1;
    }
    block
}

fn indentation_len(line: &str) -> usize {
    line.find(|character: char| !character.is_whitespace())
        .unwrap_or(line.len())
}

fn checkbox_counts(descendants: &[String]) -> (usize, usize) {
    let mut open = 0;
    let mut checked = 0;
    for line in descendants {
        let Some(captures) = CHECKBOX_RE.captures(line) else {
            continue;
        };
        match captures.get(1).map(|group| group.as_str()) {
            Some(" ") => open += 1,
            Some("x" | "X") => checked += 1,
            _ => {}
        }
    }
    (open, checked)
}

/// The first `[created::…]` value on the task line as a calendar date.
fn task_created(task_line: &str) -> Option<NaiveDate> {
    let field = task_fields::inline_fields(task_line, "created")
        .into_iter()
        .next()?;
    let value = field.value.trim();
    task_fields::parse_strict_calendar_date(value).or_else(|| {
        task_fields::parse_strict_calendar_date(
            value.get(..10.min(value.len()))?,
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::native::note_tasks::read_settings;

    #[test]
    fn marker_round_trips_ids_with_dots_and_exotic_bytes() {
        for id in [
            "simple-id_1.2",
            "with space",
            "a/b?c=d&e",
            "100%",
            "ünïcodé",
            "x%%y",
        ] {
            let marker = format_marker(id, "0123456789ab");
            let parsed = parse_markers(&marker);
            assert_eq!(
                parsed,
                vec![(id.to_string(), "0123456789ab".to_string())]
            );
        }
    }

    #[test]
    fn marker_parsing_finds_all_hits_and_ignores_broken_ones() {
        let line = "a %%gkeep:v1:n1:0123456789ab%% mid %%gkeep:v1:n2:ffffffffffff%% end";
        assert_eq!(
            parse_markers(line),
            vec![
                ("n1".to_string(), "0123456789ab".to_string()),
                ("n2".to_string(), "ffffffffffff".to_string()),
            ],
        );
        assert!(parse_markers("no markers here").is_empty());
        // Uppercase hex is not a marker.
        assert!(parse_markers("%%gkeep:v1:n1:0123456789AB%%").is_empty());
        // A bare % never decodes.
        assert!(parse_markers("%%gkeep:v1:100%:0123456789ab%%").is_empty());
    }

    fn write(path: &Path, contents: &str) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("create parent");
        }
        fs::write(path, contents).expect("write vault file");
    }

    fn temp_vault() -> tempfile::TempDir {
        let dir = tempfile::tempdir().expect("temp dir");
        let root = dir.path();
        write(
            &root.join("gkeep_inbox.md"),
            "- [ ] #task Call dentist [created::2026-09-27]\n\
             \t- Source: [Google Keep](https://keep.google.com/) · 2026-09-27 21:14 %%gkeep:v1:note-1:0123456789ab%%\n",
        );
        write(&root.join("projects/plan.md"), "nothing to see here\n");
        write(
            &root.join("done/triaged.md"),
            "- [x] #task Old task [created::2026-09-20]\n\
             \t- Source: Google Keep · 2026-09-20 08:00 %%gkeep:v1:note-2:ffffffffffff%%\n",
        );
        write(
            &root.join("done/duplicate.md"),
            "- [x] #task Old task copy [created::2026-09-20]\n\
             \t- Source: Google Keep · 2026-09-20 08:00 %%gkeep:v1:note-2:ffffffffffff%%\n",
        );
        write(
            &root.join("_conflicts/clash.md"),
            "%%gkeep:v1:note-9:000000000000%%\n",
        );
        write(
            &root.join(".obsidian/config.md"),
            "%%gkeep:v1:note-9:000000000001%%\n",
        );
        write(&root.join("plain.md"), "plain text, no marker\n");
        dir
    }

    #[test]
    fn scan_includes_done_and_skips_excluded_dirs() {
        let vault = temp_vault();
        let ledger = Ledger::scan(vault.path()).expect("scan");
        let locations: Vec<(&str, usize)> = ledger
            .entries
            .iter()
            .map(|entry| (entry.path.as_str(), entry.line))
            .collect();
        assert_eq!(
            locations,
            vec![
                ("done/duplicate.md", 2),
                ("done/triaged.md", 2),
                ("gkeep_inbox.md", 2),
            ],
        );
        assert!(ledger.has("note-1", "0123456789ab"));
        assert!(!ledger.has("note-1", "ffffffffffff"));
        assert!(ledger.has_id("note-2"));
        assert!(!ledger.has_id("note-9"));
    }

    #[test]
    fn duplicates_name_every_location() {
        let vault = temp_vault();
        let ledger = Ledger::scan(vault.path()).expect("scan");
        assert_eq!(
            ledger.duplicates(),
            vec![DuplicateGroup {
                id: "note-2".to_string(),
                fp: "ffffffffffff".to_string(),
                locations: vec![
                    "done/duplicate.md:2".to_string(),
                    "done/triaged.md:2".to_string(),
                ],
            }],
        );
    }

    #[test]
    fn journal_read_tolerates_missing_files_and_corrupt_lines() {
        let dir = tempfile::tempdir().expect("temp dir");
        let missing = dir.path().join("sub/journal.jsonl");
        let journal = Journal::read(&missing).expect("missing reads empty");
        assert!(journal.records.is_empty());
        assert_eq!(journal.skipped, 0);

        write(
            &missing,
            "{\"ts\":\"2026-09-28T00:00:00Z\",\"event\":\"written\",\"id\":\"n1\",\"ref\":\"abc1234\",\"fp\":\"0123456789ab\",\"path\":\"gkeep_inbox.md\",\"commit\":null,\"status\":null}\n\
             not json\n\
             \n\
             {\"ts\":\"x\",\"event\":\"bogus\",\"id\":\"n2\",\"ref\":\"r\",\"fp\":\"f\",\"path\":\"p\"}\n",
        );
        let journal = Journal::read(&missing).expect("read");
        assert_eq!(journal.records.len(), 1);
        assert_eq!(journal.records[0].id, "n1");
        assert_eq!(journal.skipped, 2);
        assert!(journal.has("n1", "0123456789ab"));
        assert!(!journal.has("n1", "ffffffffffff"));
        assert!(journal.has_id("n1"));
    }

    #[test]
    fn journal_append_round_trips_with_tight_permissions() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("gkeep/journal.jsonl");
        let records = vec![
            JournalRecord {
                ts: "2026-09-28T00:00:01Z".to_string(),
                event: JournalEvent::Written,
                id: "n1".to_string(),
                ref_: "abc1234".to_string(),
                fp: "0123456789ab".to_string(),
                path: "gkeep_inbox.md".to_string(),
                commit: Some("4e1f2a9".to_string()),
                status: None,
            },
            JournalRecord {
                ts: "2026-09-28T00:00:02Z".to_string(),
                event: JournalEvent::Archived,
                id: "n1".to_string(),
                ref_: "abc1234".to_string(),
                fp: "0123456789ab".to_string(),
                path: "gkeep_inbox.md".to_string(),
                commit: None,
                status: Some("archived".to_string()),
            },
        ];
        Journal::append(&path, &records).expect("append");
        // A second batch appends without truncating.
        Journal::append(&path, &records[..1]).expect("append again");

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            assert_eq!(
                fs::metadata(&path).expect("stat file").permissions().mode()
                    & 0o777,
                0o600,
            );
            assert_eq!(
                fs::metadata(path.parent().expect("parent"))
                    .expect("stat dir")
                    .permissions()
                    .mode()
                    & 0o777,
                0o700,
            );
        }

        let journal = Journal::read(&path).expect("read back");
        assert_eq!(journal.skipped, 0);
        assert_eq!(journal.records.len(), 3);
        assert_eq!(journal.records[0], records[0]);
        assert_eq!(journal.records[2], records[0]);
        // Only `written` feeds the planner.
        assert!(journal.has("n1", "0123456789ab"));
        let archived_only = Journal {
            records: vec![records[1].clone()],
            skipped: 0,
        };
        assert!(!archived_only.has("n1", "0123456789ab"));
        assert!(!archived_only.has_id("n1"));
    }

    #[test]
    fn journal_read_skips_non_utf8_lines() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("journal.jsonl");
        let good = b"{\"ts\":\"2026-09-28T00:00:00Z\",\"event\":\"written\",\"id\":\"n1\",\"ref\":\"abc1234\",\"fp\":\"0123456789ab\",\"path\":\"gkeep_inbox.md\",\"commit\":null,\"status\":null}\n";
        let bad: &[u8] = &[0xff, 0xfe, b'\n'];
        let mut bytes = Vec::new();
        bytes.extend_from_slice(good);
        bytes.extend_from_slice(bad);
        bytes.extend_from_slice(good);
        fs::write(&path, &bytes).expect("write journal");
        let journal = Journal::read(&path).expect("read");
        assert_eq!(journal.records.len(), 2);
        assert_eq!(journal.skipped, 1);
    }

    #[test]
    fn journal_append_writes_leading_newline_after_torn_record() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("journal.jsonl");
        // Torn record with no trailing newline.
        fs::write(&path, "{\"torn\": true}").expect("write torn");
        let records = vec![JournalRecord {
            ts: "2026-09-28T00:00:01Z".to_string(),
            event: JournalEvent::Written,
            id: "n1".to_string(),
            ref_: "abc1234".to_string(),
            fp: "0123456789ab".to_string(),
            path: "gkeep_inbox.md".to_string(),
            commit: None,
            status: None,
        }];
        Journal::append(&path, &records).expect("append");
        let journal = Journal::read(&path).expect("read back");
        assert_eq!(journal.records.len(), 1);
        assert_eq!(journal.records[0].id, "n1");
        assert_eq!(journal.skipped, 1);
    }

    #[test]
    fn read_target_tasks_covers_top_level_tasks_only() {
        let settings = read_settings(Path::new("/nonexistent-bob-dir"));
        let contents = "---\ntitle: x\n---\n\n\
             ## Tasks\n\n\
             - [ ] #task Open task [created::2026-09-27]\n\
             \t- [ ] subtask one\n\
             \t- [x] subtask two\n\
             \t- Source: Google Keep · 2026-09-27 21:14 %%gkeep:v1:note-1:0123456789ab%%\n\
             \n\
             - [x] #task Done task [created:: 2026-09-20]\n\
             \t- Source: Google Keep · 2026-09-20 08:00 %%gkeep:v1:note-2:ffffffffffff%%\n\
             \n\
             \t- [ ] indented task is not top level [created::2026-09-21]\n";
        let tasks = read_target_tasks(contents, &settings);
        assert_eq!(tasks.len(), 2);
        assert_eq!(tasks[0].line, 7);
        assert_eq!(tasks[0].status_symbol, ' ');
        assert_eq!(tasks[0].description, "Open task");
        assert_eq!(
            tasks[0].created,
            task_fields::parse_strict_calendar_date("2026-09-27"),
        );
        assert_eq!(tasks[0].open_items, 1);
        assert_eq!(tasks[0].checked_items, 1);
        assert_eq!(
            tasks[0].marker,
            Some(("note-1".to_string(), "0123456789ab".to_string())),
        );
        assert_eq!(tasks[1].line, 12);
        assert_eq!(tasks[1].status_symbol, 'x');
        assert_eq!(
            tasks[1].marker,
            Some(("note-2".to_string(), "ffffffffffff".to_string())),
        );
    }

    #[test]
    fn read_target_tasks_handles_missing_created_and_markers() {
        let settings = read_settings(Path::new("/nonexistent-bob-dir"));
        let contents = "- [ ] #task Plain task\n";
        let tasks = read_target_tasks(contents, &settings);
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0].created, None);
        assert_eq!(tasks[0].open_items, 0);
        assert_eq!(tasks[0].checked_items, 0);
        assert_eq!(tasks[0].marker, None);
    }
}
