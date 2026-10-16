//! Locked atomic JSON shelf for saved `@@template:` blocks.
//!
//! The shelf lives at `bob_cli_state_dir()/capture/templates.json`, with
//! `BOB_CAPTURE_TEMPLATES_FILE` overriding the exact path. Entries are
//! keyed by the content hash of their canonical block text, most recently
//! touched first, and every write trims to `capture.max_saved_templates`.
use std::{
    ffi::OsString,
    fs,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use fs2::FileExt;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::super::{capture_language, env as bob_env};

const SCHEMA_VERSION: u64 = 1;
const LOCK_TIMEOUT: Duration = Duration::from_secs(2);
const LOCK_POLL: Duration = Duration::from_millis(10);

/// Where the shelf lives, honoring the exact-file override.
pub(crate) fn templates_file() -> PathBuf {
    if let Some(override_path) = bob_env::var_os("BOB_CAPTURE_TEMPLATES_FILE")
        .filter(|value| !value.is_empty())
    {
        return PathBuf::from(override_path);
    }
    bob_env::bob_cli_state_dir()
        .join("capture")
        .join("templates.json")
}

fn lock_file_for(file: &Path) -> PathBuf {
    let mut name: OsString = file.as_os_str().to_owned();
    name.push(".lock");
    PathBuf::from(name)
}

fn quarantine_file_for(file: &Path) -> PathBuf {
    let parent = file.parent().unwrap_or_else(|| Path::new("."));
    let stem = file
        .file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .filter(|stem| !stem.is_empty())
        .unwrap_or_else(|| "templates".to_string());
    parent.join(format!("{stem}.corrupt.json"))
}

/// Canonical block text: the analyzed template block's lines, header
/// excluded, each stripped of trailing ASCII spaces and tabs and joined
/// with `\n`. Indentation, escapes, quotes, Unicode, and literal text are
/// preserved byte for byte.
pub(crate) fn canonical_template_text(raw: &str) -> Option<String> {
    let analysis = capture_language::template::analyze(raw);
    let block = analysis.block.as_ref()?;
    let mut lines = Vec::with_capacity(block.line_ranges().len());
    for (_, range) in block.line_ranges() {
        let text = raw.get(range.clone())?;
        lines.push(text.trim_end_matches([' ', '\t']));
    }
    Some(lines.join("\n"))
}

/// First 12 lowercase hex digits of SHA-256 over the canonical text.
pub(crate) fn template_id(canonical: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(canonical.as_bytes());
    hex::encode(hasher.finalize())[..12].to_string()
}

/// RFC 3339 with the local offset at seconds precision, from bob's clock
/// so `BOB_NOW` keeps tests deterministic.
pub(crate) fn now_timestamp() -> String {
    format_timestamp(bob_env::current_datetime())
}

fn format_timestamp(now: chrono::NaiveDateTime) -> String {
    use chrono::{Local, LocalResult, TimeZone};
    const FORMAT: &str = "%Y-%m-%dT%H:%M:%S%:z";
    match Local.from_local_datetime(&now) {
        LocalResult::Single(datetime) => datetime.format(FORMAT).to_string(),
        LocalResult::Ambiguous(earliest, _) => {
            earliest.format(FORMAT).to_string()
        }
        LocalResult::None => {
            let later = now
                .checked_add_signed(chrono::Duration::hours(1))
                .unwrap_or(now);
            let offset = match Local.from_local_datetime(&later) {
                LocalResult::Single(datetime) => Some(*datetime.offset()),
                LocalResult::Ambiguous(earliest, _) => Some(*earliest.offset()),
                LocalResult::None => None,
            };
            offset
                .and_then(|offset| offset.from_local_datetime(&now).single())
                .map(|resolved| resolved.format(FORMAT).to_string())
                .unwrap_or_else(|| {
                    format!("{}+00:00", now.format("%Y-%m-%dT%H:%M:%S"))
                })
        }
    }
}

/// Whether a `--template-origin` value is accepted: non-empty ASCII
/// alphanumeric, at most 64 characters.
pub(crate) fn is_valid_origin(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value.bytes().all(|byte| byte.is_ascii_alphanumeric())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TemplateSaveStatus {
    New,
    Updated,
    Replaced,
}

impl TemplateSaveStatus {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::New => "new",
            Self::Updated => "updated",
            Self::Replaced => "replaced",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TemplateSaveOutcome {
    pub(crate) id: String,
    pub(crate) status: TemplateSaveStatus,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum TemplateSaveError {
    Busy(String),
    Refused(String),
    Io(String),
}

impl TemplateSaveError {
    pub(crate) fn reason(&self) -> &str {
        match self {
            Self::Busy(reason) | Self::Refused(reason) | Self::Io(reason) => {
                reason
            }
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ShelfEntry {
    id: Option<String>,
    text: Option<String>,
    #[serde(default)]
    first_saved_at: Option<String>,
    #[serde(default)]
    last_used_at: Option<String>,
    #[serde(default)]
    use_count: Option<u64>,
    #[serde(flatten, default)]
    extra: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ShelfFile {
    schema_version: Option<u64>,
    #[serde(default)]
    templates: Option<Vec<ShelfEntry>>,
    #[serde(flatten, default)]
    extra: serde_json::Map<String, serde_json::Value>,
}

struct LoadedShelf {
    entries: Vec<ShelfEntry>,
    extra: serde_json::Map<String, serde_json::Value>,
    corrupt: bool,
    newer_schema: Option<u64>,
}

/// Save one canonical template block, moving it to the top. `origin` is
/// the checked-out id the app passes back: when it names a different entry
/// on the shelf, that entry is removed in the same locked write.
pub(crate) fn save_template_text(
    canonical: &str,
    origin: Option<&str>,
    cap: u32,
    now: &str,
) -> Result<TemplateSaveOutcome, TemplateSaveError> {
    save_template_text_with_lock_timeout(
        canonical,
        origin,
        cap,
        now,
        LOCK_TIMEOUT,
    )
}

pub(crate) fn save_template_text_with_lock_timeout(
    canonical: &str,
    origin: Option<&str>,
    cap: u32,
    now: &str,
    lock_timeout: Duration,
) -> Result<TemplateSaveOutcome, TemplateSaveError> {
    let file = templates_file();
    let id = template_id(canonical);
    if let Some(parent) = file.parent()
        && !parent.as_os_str().is_empty()
    {
        fs::create_dir_all(parent).map_err(|error| {
            TemplateSaveError::Io(format!("write shelf: {error}"))
        })?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
            let mut builder = fs::DirBuilder::new();
            builder.mode(0o700);
            let _ = builder.create(parent);
            let _ =
                fs::set_permissions(parent, fs::Permissions::from_mode(0o700));
        }
    }
    let lock_path = lock_file_for(&file);
    let lock_dir = lock_path.parent().unwrap_or_else(|| Path::new("."));
    if !lock_dir.as_os_str().is_empty() {
        fs::create_dir_all(lock_dir).map_err(|error| {
            TemplateSaveError::Io(format!("write shelf: {error}"))
        })?;
    }
    let lock = bob_env::create_state_lock_file(
        lock_dir,
        lock_path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("templates.json.lock"),
    )
    .map_err(|error| TemplateSaveError::Io(format!("write shelf: {error}")))?;
    lock_exclusive(&lock, lock_timeout).map_err(|error| {
        TemplateSaveError::Busy(format!("shelf is busy: {error}"))
    })?;

    let mut loaded = read_locked(&file);
    if loaded.corrupt {
        quarantine_locked(&file);
    }
    if let Some(version) = loaded.newer_schema {
        return Err(TemplateSaveError::Refused(format!(
            "shelf uses newer schema_version {version}"
        )));
    }
    if loaded.corrupt {
        loaded.entries.clear();
    }

    let origin = origin.filter(|origin| !origin.is_empty());
    let origin_present = origin.is_some_and(|origin| {
        loaded
            .entries
            .iter()
            .any(|entry| entry.id.as_deref() == Some(origin))
    });
    let existing_index = loaded
        .entries
        .iter()
        .position(|entry| entry.id.as_deref() == Some(id.as_str()));

    let outcome = if origin.is_some_and(|origin| origin != id) && origin_present
    {
        let origin_id = origin.expect("origin is present");
        let origin_entry = loaded
            .entries
            .iter()
            .find(|entry| entry.id.as_deref() == Some(origin_id))
            .cloned();
        loaded
            .entries
            .retain(|entry| entry.id.as_deref() != Some(origin_id));
        match loaded
            .entries
            .iter()
            .position(|entry| entry.id.as_deref() == Some(id.as_str()))
        {
            Some(index) => {
                let mut entry = loaded.entries.remove(index);
                entry.last_used_at = Some(now.to_string());
                entry.use_count =
                    Some(entry.use_count.unwrap_or(0).saturating_add(1));
                loaded.entries.insert(0, entry);
                TemplateSaveOutcome {
                    id,
                    status: TemplateSaveStatus::Replaced,
                }
            }
            None => {
                let (first_saved_at, use_count, extra) = origin_entry
                    .map_or_else(
                        || (now.to_string(), 1u64, Default::default()),
                        |entry| {
                            (
                                entry
                                    .first_saved_at
                                    .unwrap_or_else(|| now.to_string()),
                                entry.use_count.unwrap_or(0).saturating_add(1),
                                entry.extra,
                            )
                        },
                    );
                loaded.entries.insert(
                    0,
                    ShelfEntry {
                        id: Some(id.clone()),
                        text: Some(canonical.to_string()),
                        first_saved_at: Some(first_saved_at),
                        last_used_at: Some(now.to_string()),
                        use_count: Some(use_count),
                        extra,
                    },
                );
                TemplateSaveOutcome {
                    id,
                    status: TemplateSaveStatus::Replaced,
                }
            }
        }
    } else if let Some(index) = existing_index {
        let mut entry = loaded.entries.remove(index);
        entry.last_used_at = Some(now.to_string());
        entry.use_count = Some(entry.use_count.unwrap_or(0).saturating_add(1));
        loaded.entries.insert(0, entry);
        TemplateSaveOutcome {
            id,
            status: TemplateSaveStatus::Updated,
        }
    } else {
        loaded.entries.insert(
            0,
            ShelfEntry {
                id: Some(id.clone()),
                text: Some(canonical.to_string()),
                first_saved_at: Some(now.to_string()),
                last_used_at: Some(now.to_string()),
                use_count: Some(1),
                extra: Default::default(),
            },
        );
        TemplateSaveOutcome {
            id,
            status: TemplateSaveStatus::New,
        }
    };

    let cap_usize = usize::try_from(cap).unwrap_or(usize::MAX);
    loaded.entries.truncate(cap_usize);
    write_locked(&file, &loaded.entries, &loaded.extra)?;
    Ok(outcome)
}

fn lock_exclusive(lock: &fs::File, timeout: Duration) -> Result<(), String> {
    let start = Instant::now();
    loop {
        match lock.try_lock_exclusive() {
            Ok(()) => return Ok(()),
            Err(error) => {
                if start.elapsed() >= timeout {
                    return Err(error.to_string());
                }
                std::thread::sleep(LOCK_POLL);
            }
        }
    }
}

fn read_locked(file: &Path) -> LoadedShelf {
    let bytes = match fs::read(file) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return LoadedShelf {
                entries: Vec::new(),
                extra: Default::default(),
                corrupt: false,
                newer_schema: None,
            };
        }
        Err(_) => {
            return LoadedShelf {
                entries: Vec::new(),
                extra: Default::default(),
                corrupt: true,
                newer_schema: None,
            };
        }
    };
    let parsed: ShelfFile = match serde_json::from_slice(&bytes) {
        Ok(parsed) => parsed,
        Err(_) => {
            return LoadedShelf {
                entries: Vec::new(),
                extra: Default::default(),
                corrupt: true,
                newer_schema: None,
            };
        }
    };
    let Some(version) = parsed.schema_version else {
        return LoadedShelf {
            entries: Vec::new(),
            extra: Default::default(),
            corrupt: true,
            newer_schema: None,
        };
    };
    if version > SCHEMA_VERSION {
        let entries = parsed
            .templates
            .unwrap_or_default()
            .into_iter()
            .filter(|entry| {
                entry.id.as_deref().is_some_and(|id| !id.is_empty())
                    && entry.text.is_some()
            })
            .collect();
        return LoadedShelf {
            entries,
            extra: parsed.extra,
            corrupt: false,
            newer_schema: Some(version),
        };
    }
    let Some(templates) = parsed.templates else {
        return LoadedShelf {
            entries: Vec::new(),
            extra: Default::default(),
            corrupt: true,
            newer_schema: None,
        };
    };
    for entry in &templates {
        if entry.id.as_deref().is_none_or(|id| id.is_empty())
            || entry.text.is_none()
        {
            return LoadedShelf {
                entries: Vec::new(),
                extra: parsed.extra,
                corrupt: true,
                newer_schema: None,
            };
        }
    }
    LoadedShelf {
        entries: templates,
        extra: parsed.extra,
        corrupt: false,
        newer_schema: None,
    }
}

fn quarantine_locked(file: &Path) {
    let quarantine = quarantine_file_for(file);
    let _ = fs::remove_file(&quarantine);
    let _ = fs::rename(file, &quarantine);
}

fn write_locked(
    file: &Path,
    entries: &[ShelfEntry],
    extra: &serde_json::Map<String, serde_json::Value>,
) -> Result<(), TemplateSaveError> {
    if entries.is_empty() {
        match fs::remove_file(file) {
            Ok(()) | Err(_) => {
                if fs::metadata(file).is_err() {
                    return Ok(());
                }
                return fs::remove_file(file).map_err(|error| {
                    TemplateSaveError::Io(format!("write shelf: {error}"))
                });
            }
        }
    }
    let payload = serde_json::json!({
        "schema_version": SCHEMA_VERSION,
        "templates": entries,
    });
    let mut value = serde_json::to_value(&payload).expect("shelf serializes");
    if let serde_json::Value::Object(ref mut object) = value {
        for (key, extra_value) in extra {
            object
                .entry(key.clone())
                .or_insert_with(|| extra_value.clone());
        }
    }
    let mut text =
        serde_json::to_string_pretty(&value).expect("shelf serializes");
    text.push('\n');
    let parent = file.parent().unwrap_or_else(|| Path::new("."));
    let temp_name = format!(
        ".{}.tmp-{}",
        file.file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("templates.json"),
        std::process::id()
    );
    let temp = parent.join(temp_name);
    fs::write(&temp, text.as_bytes()).map_err(|error| {
        TemplateSaveError::Io(format!("write shelf: {error}"))
    })?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(&temp, fs::Permissions::from_mode(0o600));
    }
    let sync_temp = fs::File::open(&temp)
        .and_then(|synced| synced.sync_all().map(|()| synced));
    if let Ok(synced) = sync_temp {
        drop(synced);
    }
    fs::rename(&temp, file).map_err(|error| {
        TemplateSaveError::Io(format!("write shelf: {error}"))
    })?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(file, fs::Permissions::from_mode(0o600));
    }
    if let Ok(dir) = fs::File::open(parent) {
        let _ = dir.sync_all();
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::native::env::TestEnvGuard;
    use std::ffi::OsStr;

    fn shelf_guard(temp: &tempfile::TempDir, name: &str) -> TestEnvGuard {
        let file = temp.path().join(name);
        TestEnvGuard::set(&[(
            "BOB_CAPTURE_TEMPLATES_FILE",
            Some(file.as_os_str()),
        )])
    }

    fn read_entries(file: &Path) -> Vec<ShelfEntry> {
        let bytes = fs::read(file).expect("read shelf");
        let parsed: ShelfFile =
            serde_json::from_slice(&bytes).expect("parse shelf");
        parsed.templates.unwrap_or_default()
    }

    #[test]
    fn canonical_text_strips_trailing_space_and_keeps_layout() {
        let raw = "@@template:\nApply to {0}! @job^apply-{1}  \t\n- **URL**: {2}\t\n\nFoobar foo x\n";
        assert_eq!(
            canonical_template_text(raw).as_deref(),
            Some("Apply to {0}! @job^apply-{1}\n- **URL**: {2}")
        );
    }

    #[test]
    fn canonical_text_handles_crlf_unicode_escapes_and_header_gap() {
        let raw = "@@template:\r\n\r\n  Apply {{{0}}} café {1}  \r\n\t- child {2}\r\n\r\nrow\n";
        let canonical = canonical_template_text(raw).expect("canonical");
        assert_eq!(canonical, "  Apply {{{0}}} café {1}\n\t- child {2}");
    }

    #[test]
    fn template_ids_are_stable_and_content_addressed() {
        let first = template_id("Apply to {0}");
        let second = template_id("Apply to {0}");
        assert_eq!(first, second);
        assert_eq!(first.len(), 12);
        assert!(
            first
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit()
                    && !byte.is_ascii_uppercase())
        );
        assert_ne!(first, template_id("Apply to {0} "));
    }

    #[test]
    fn repeated_captures_dedupe_and_bump() {
        let temp = tempfile::tempdir().expect("tempdir");
        let _guard = shelf_guard(&temp, "templates.json");
        let now = "2026-10-16T09:12:44-04:00";
        let later = "2026-10-16T09:40:02-04:00";
        let first =
            save_template_text("Apply to {0}", None, 36, now).expect("save");
        assert_eq!(first.status, TemplateSaveStatus::New);
        let second =
            save_template_text("Apply to {0}", None, 36, later).expect("save");
        assert_eq!(second.status, TemplateSaveStatus::Updated);
        assert_eq!(second.id, first.id);
        let entries = read_entries(&templates_file());
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].use_count, Some(2));
        assert_eq!(entries[0].first_saved_at.as_deref(), Some(now));
        assert_eq!(entries[0].last_used_at.as_deref(), Some(later));
    }

    #[test]
    fn one_row_and_many_rows_save_the_same_entry() {
        let single =
            canonical_template_text("@@template:\nApply to {0}\n\none\n")
                .expect("canonical");
        let many = canonical_template_text(
            "@@template:\nApply to {0}\n\none\ntwo\nthree\n",
        )
        .expect("canonical");
        assert_eq!(single, many);
        assert_eq!(template_id(&single), template_id(&many));
    }

    #[test]
    fn eviction_drops_least_recently_used_and_lowered_cap_trims() {
        let temp = tempfile::tempdir().expect("tempdir");
        let _guard = shelf_guard(&temp, "templates.json");
        for index in 0..4 {
            save_template_text(
                &format!("Task {index} {{0}}"),
                None,
                3,
                "2026-10-16T09:12:44-04:00",
            )
            .expect("save");
        }
        let entries = read_entries(&templates_file());
        assert_eq!(entries.len(), 3);
        assert_eq!(entries[0].text.as_deref(), Some("Task 3 {0}"));
        save_template_text(
            "Task fresh {0}",
            None,
            1,
            "2026-10-16T09:40:02-04:00",
        )
        .expect("save");
        let entries = read_entries(&templates_file());
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].text.as_deref(), Some("Task fresh {0}"));
    }

    #[test]
    fn origin_replace_swaps_in_one_write_and_keeps_history() {
        let temp = tempfile::tempdir().expect("tempdir");
        let _guard = shelf_guard(&temp, "templates.json");
        let now = "2026-10-16T09:12:44-04:00";
        let later = "2026-10-16T09:40:02-04:00";
        let original =
            save_template_text("Apply to {0}", None, 36, now).expect("save");
        let replaced =
            save_template_text("Apply to {0}!", Some(&original.id), 36, later)
                .expect("save");
        assert_eq!(replaced.status, TemplateSaveStatus::Replaced);
        let entries = read_entries(&templates_file());
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].text.as_deref(), Some("Apply to {0}!"));
        assert_eq!(entries[0].first_saved_at.as_deref(), Some(now));
        assert_eq!(entries[0].use_count, Some(2));
    }

    #[test]
    fn origin_cases_cover_same_missing_and_existing_target() {
        let temp = tempfile::tempdir().expect("tempdir");
        let _guard = shelf_guard(&temp, "templates.json");
        let now = "2026-10-16T09:12:44-04:00";
        let first =
            save_template_text("Task {0}", None, 36, now).expect("save");
        let same = save_template_text("Task {0}", Some(&first.id), 36, now)
            .expect("save");
        assert_eq!(same.status, TemplateSaveStatus::Updated);
        let missing =
            save_template_text("Other {0}", Some("deadbeefcafe"), 36, now)
                .expect("save");
        assert_eq!(missing.status, TemplateSaveStatus::New);
        let second_id = template_id("Second {0}");
        save_template_text("Second {0}", None, 36, now).expect("save");
        let bump = save_template_text("Second {0}", Some(&first.id), 36, now)
            .expect("save");
        assert_eq!(bump.status, TemplateSaveStatus::Replaced);
        assert_eq!(bump.id, second_id);
        let entries = read_entries(&templates_file());
        assert!(entries
            .iter()
            .any(|entry| entry.id.as_deref() == Some(&second_id)));
        assert!(entries
            .iter()
            .all(|entry| entry.id.as_deref() != Some(first.id.as_str())));
    }

    #[test]
    fn corrupt_file_is_quarantined_on_write() {
        let temp = tempfile::tempdir().expect("tempdir");
        let _guard = shelf_guard(&temp, "templates.json");
        let file = templates_file();
        fs::create_dir_all(file.parent().expect("parent")).expect("mkdir");
        fs::write(&file, b"not json").expect("write corrupt");
        let outcome = save_template_text(
            "Task {0}",
            None,
            36,
            "2026-10-16T09:12:44-04:00",
        )
        .expect("save after corrupt");
        assert_eq!(outcome.status, TemplateSaveStatus::New);
        assert!(quarantine_file_for(&file).exists());
        let entries = read_entries(&file);
        assert_eq!(entries.len(), 1);
    }

    #[test]
    fn newer_schema_refuses_writes_and_keeps_bytes() {
        let temp = tempfile::tempdir().expect("tempdir");
        let _guard = shelf_guard(&temp, "templates.json");
        let file = templates_file();
        fs::create_dir_all(file.parent().expect("parent")).expect("mkdir");
        let bytes = br#"{"schema_version": 2, "templates": [{"id": "9c1f0e2a7b3d", "text": "Task {0}"}]}"#;
        fs::write(&file, bytes).expect("write newer");
        let error = save_template_text(
            "Task {0}",
            None,
            36,
            "2026-10-16T09:12:44-04:00",
        )
        .expect_err("newer schema refuses");
        assert!(error.reason().contains("newer"));
        assert_eq!(fs::read(&file).expect("read"), bytes);
    }

    #[test]
    fn unknown_fields_survive_a_rewrite() {
        let temp = tempfile::tempdir().expect("tempdir");
        let _guard = shelf_guard(&temp, "templates.json");
        let file = templates_file();
        fs::create_dir_all(file.parent().expect("parent")).expect("mkdir");
        fs::write(
            &file,
            br#"{"schema_version": 1, "shelf_name": "mine", "templates": [{"id": "x", "text": "Old {0}", "name": "keep me", "first_saved_at": "2026-10-16T09:12:44-04:00", "last_used_at": "2026-10-16T09:12:44-04:00", "use_count": 1}]}"#,
        )
        .expect("write");
        // Saving an unrelated template rewrites the file; unknown top-level
        // and per-entry fields must survive.
        save_template_text("Task {0}", None, 36, "2026-10-16T09:40:02-04:00")
            .expect("save");
        let value: serde_json::Value =
            serde_json::from_slice(&fs::read(&file).expect("read"))
                .expect("parse");
        assert_eq!(value.get("shelf_name"), Some(&serde_json::json!("mine")));
    }

    #[test]
    fn origin_replace_keeps_unknown_fields() {
        let temp = tempfile::tempdir().expect("tempdir");
        let _guard = shelf_guard(&temp, "templates.json");
        let file = templates_file();
        fs::create_dir_all(file.parent().expect("parent")).expect("mkdir");
        fs::write(
            &file,
            br#"{"schema_version": 1, "templates": [{"id": "PLACEHOLDER", "text": "Old {0}", "name": "keep me", "first_saved_at": "2026-10-16T09:12:44-04:00", "last_used_at": "2026-10-16T09:12:44-04:00", "use_count": 4}]}"#,
        )
        .expect("write");
        let old_id = template_id("Old {0}");
        let raw = fs::read_to_string(&file).expect("read");
        fs::write(&file, raw.replace("PLACEHOLDER", &old_id)).expect("fix id");
        let outcome = save_template_text(
            "Old {0}!",
            Some(&old_id),
            36,
            "2026-10-16T09:40:02-04:00",
        )
        .expect("replace");
        assert_eq!(outcome.status, TemplateSaveStatus::Replaced);
        let value: serde_json::Value =
            serde_json::from_slice(&fs::read(&file).expect("read"))
                .expect("parse");
        let entry = value
            .get("templates")
            .and_then(|templates| templates.get(0))
            .expect("entry");
        assert_eq!(entry.get("name"), Some(&serde_json::json!("keep me")));
    }

    #[test]
    fn shelf_files_use_restricted_permissions() {
        let temp = tempfile::tempdir().expect("tempdir");
        let _guard = shelf_guard(&temp, "templates.json");
        save_template_text("Task {0}", None, 36, "2026-10-16T09:12:44-04:00")
            .expect("save");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let file = templates_file();
            let mode = fs::metadata(&file).expect("stat").permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
            let dir_mode = fs::metadata(file.parent().expect("parent"))
                .expect("stat dir")
                .permissions()
                .mode();
            assert_eq!(dir_mode & 0o777, 0o700);
        }
    }

    #[test]
    fn lock_timeout_reports_busy() {
        let temp = tempfile::tempdir().expect("tempdir");
        let _guard = shelf_guard(&temp, "templates.json");
        let file = templates_file();
        let lock_path = lock_file_for(&file);
        let lock_dir = lock_path.parent().expect("parent");
        fs::create_dir_all(lock_dir).expect("mkdir");
        let held = bob_env::create_state_lock_file(
            lock_dir,
            lock_path
                .file_name()
                .and_then(|name| name.to_str())
                .expect("name"),
        )
        .expect("lock file");
        held.lock_exclusive().expect("hold lock");
        let error = save_template_text_with_lock_timeout(
            "Task {0}",
            None,
            36,
            "2026-10-16T09:12:44-04:00",
            Duration::from_millis(50),
        )
        .expect_err("busy");
        assert!(matches!(error, TemplateSaveError::Busy(_)));
    }

    #[test]
    fn origin_validation_accepts_only_short_alphanumerics() {
        assert!(is_valid_origin("9c1f0e2a7b3d"));
        assert!(!is_valid_origin(""));
        assert!(!is_valid_origin("9c1f-0e2a"));
        assert!(!is_valid_origin("9c1f 0e2a"));
        assert!(!is_valid_origin(&"a".repeat(65)));
        assert!(is_valid_origin(&"a".repeat(64)));
        let _ = OsStr::new("keep");
    }
}
