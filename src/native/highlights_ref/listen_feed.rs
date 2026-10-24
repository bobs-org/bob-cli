//! Best-effort notices for episodes queued in sase-listen's publish outbox.
//!
//! bob reads only the small outbox and the referenced episode manifests. It
//! does not inspect sase-listen internals or scan the audio library.

use std::{
    cell::RefCell,
    fs,
    path::{Path, PathBuf},
};

use chrono::NaiveDate;
use serde_json::Value;

use super::{
    audio, bob_config, bob_env, companion,
    listen::{ListenCommand, ListenOutput},
    Styler,
};

const FEED_HINT: &str = "they will not appear in AntennaPod until the feed host is fixed; check `sase listen doctor`, then run `sase listen publish --pending`";

thread_local! {
    static DEFERRED_NOTICES: RefCell<Vec<FeedNotice>> = const { RefCell::new(Vec::new()) };
}

/// One leniently parsed entry from sase-listen's retry outbox.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct QueuedPublish {
    pub(super) episode_id: String,
    pub(super) queued_at: String,
    pub(super) attempts: u64,
    pub(super) last_error: String,
}

/// A queued publish that may match this run, plus every other queued item.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct FeedNotice {
    pub(super) this_episode: Option<QueuedPublish>,
    pub(super) others: Vec<QueuedPublish>,
}

/// The sase-listen retry outbox, under bob's configured state home.
pub(super) fn outbox_dir() -> PathBuf {
    bob_env::state_home().join("sase-listen/outbox")
}

/// Read regular JSON entries from the outbox, skipping malformed data.
pub(super) fn read_outbox(dir: &Path) -> Vec<QueuedPublish> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut queued = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        if name.starts_with('.')
            || path.extension().and_then(|ext| ext.to_str()) != Some("json")
        {
            continue;
        }
        if !entry.file_type().is_ok_and(|kind| kind.is_file()) {
            continue;
        }
        let Ok(text) = fs::read_to_string(&path) else {
            continue;
        };
        let Ok(value) = serde_json::from_str::<Value>(&text) else {
            continue;
        };
        let Some(episode_id) = value
            .get("episode_id")
            .and_then(Value::as_str)
            .filter(|id| audio::is_valid_episode_id(id))
        else {
            continue;
        };
        queued.push(QueuedPublish {
            episode_id: episode_id.to_string(),
            queued_at: value
                .get("queued_at")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            attempts: value
                .get("attempts")
                .and_then(Value::as_u64)
                .unwrap_or_default(),
            last_error: value
                .get("last_error")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
        });
    }
    queued.sort_by(|left, right| {
        left.queued_at
            .cmp(&right.queued_at)
            .then_with(|| left.episode_id.cmp(&right.episode_id))
    });
    queued
}

/// Match a successful listen's audio bytes against entries in the outbox.
pub(super) fn feed_notice(
    audio_path: &Path,
    library: &Path,
    outbox: &Path,
) -> Option<FeedNotice> {
    let entries = read_outbox(outbox);
    if entries.is_empty() {
        return None;
    }
    let audio_size = fs::metadata(audio_path).ok().map(|meta| meta.len());
    let mut this_episode = None;
    let mut others = Vec::new();
    for entry in entries {
        let matching_audio = audio_size.is_some_and(|size| {
            audio::episode_audio_source(library, &entry.episode_id)
                .and_then(|path| {
                    (fs::metadata(&path).ok()?.len() == size).then_some(path)
                })
                .and_then(|path| {
                    companion::files_have_identical_bytes(audio_path, &path)
                        .ok()
                        .filter(|identical| *identical)
                })
                .is_some()
        });
        if matching_audio && this_episode.is_none() {
            this_episode = Some(entry);
        } else {
            others.push(entry);
        }
    }
    Some(FeedNotice {
        this_episode,
        others,
    })
}

/// Render the warning and recovery hint without their output prefixes.
pub(super) fn notice_lines(notice: &FeedNotice) -> (String, String) {
    let warning = if let Some(this_episode) = &notice.this_episode {
        let mut warning = format!(
            "podcast feed: this episode was not published (sase listen queued it: {})",
            compact_error(&this_episode.last_error)
        );
        if !notice.others.is_empty() {
            warning.push_str(&format!(
                "; {} earlier {} {} also queued",
                notice.others.len(),
                episode_word(notice.others.len()),
                if notice.others.len() == 1 {
                    "is"
                } else {
                    "are"
                }
            ));
            if let Some(date) = oldest_date(&notice.others) {
                warning.push_str(&format!(", oldest since {date}"));
            }
        }
        warning
    } else if !notice.others.is_empty() {
        let count = notice.others.len();
        let oldest = oldest_date(&notice.others)
            .map(|date| format!(", oldest since {date}"))
            .unwrap_or_default();
        let newest_error = notice
            .others
            .iter()
            .max_by(|left, right| queued_order(left, right))
            .map(|entry| compact_error(&entry.last_error))
            .unwrap_or_default();
        format!(
            "podcast feed: {count} {} {} still queued for publishing{oldest} (last error: {newest_error})",
            episode_word(count),
            if count == 1 { "is" } else { "are" }
        )
    } else {
        "podcast feed: no queued publishes".to_string()
    };
    (warning, FEED_HINT.to_string())
}

/// Inspect the outbox after a successful listen, never failing the command.
pub(super) fn after_listen_success(audio_path: &Path, output: ListenOutput) {
    let configured =
        bob_config::load_highlights_config(&bob_config::config_path()).ok();
    let library = audio::audio_library_root(
        configured
            .as_ref()
            .and_then(|config| config.audio_library()),
    );
    let Some(notice) = feed_notice(audio_path, &library, &outbox_dir()) else {
        return;
    };
    match output {
        ListenOutput::Print => {
            DEFERRED_NOTICES.with(|notices| notices.borrow_mut().push(notice));
        }
        ListenOutput::Quiet => {
            let (warning, hint) = notice_lines(&notice);
            eprintln!("warning: {warning}\nhint: {hint}");
        }
    }
}

/// Print and clear notices collected during the current interactive create.
pub(super) fn print_deferred_notices() {
    let notices = DEFERRED_NOTICES
        .with(|notices| std::mem::take(&mut *notices.borrow_mut()));
    let styler = Styler::detect();
    for notice in notices {
        let (warning, hint) = notice_lines(&notice);
        eprintln!("{}: {warning}\nhint: {hint}", styler.warning_prefix());
    }
}

/// Append the publish queue's health row when listen is configured.
pub(super) fn append_listen_feed_doctor_row(warnings: &mut Vec<String>) {
    let Some((row, warning)) = listen_feed_doctor_row() else {
        return;
    };
    println!("{row}");
    if let Some(warning) = warning {
        warnings.push(warning);
    }
}

fn listen_feed_doctor_row() -> Option<(String, Option<String>)> {
    if !matches!(ListenCommand::resolve(), Ok(Some(_))) {
        return None;
    }
    let dir = outbox_dir();
    let entries = read_outbox(&dir);
    if entries.is_empty() {
        return Some((
            "listen_feed: ok (no queued feed publishes)".to_string(),
            None,
        ));
    }
    let count = entries.len();
    let oldest = oldest_date(&entries)
        .map(|date| format!(", oldest since {date}"))
        .unwrap_or_default();
    let newest_error = entries
        .iter()
        .max_by(|left, right| queued_order(left, right))
        .map(|entry| compact_error(&entry.last_error))
        .unwrap_or_default();
    let row = format!(
        "listen_feed: warn ({count} {} queued for the podcast feed{oldest}; last error: {newest_error})",
        episode_word(count)
    );
    let warning = format!(
        "sase-listen publish outbox {} is not empty; {FEED_HINT}",
        dir.display()
    );
    Some((row, Some(warning)))
}

fn queued_order(
    left: &QueuedPublish,
    right: &QueuedPublish,
) -> std::cmp::Ordering {
    left.queued_at
        .cmp(&right.queued_at)
        .then_with(|| left.episode_id.cmp(&right.episode_id))
}

fn oldest_date(entries: &[QueuedPublish]) -> Option<&str> {
    let oldest = entries
        .iter()
        .min_by(|left, right| queued_order(left, right))?;
    let date = oldest.queued_at.get(..10)?;
    NaiveDate::parse_from_str(date, "%Y-%m-%d").ok()?;
    Some(date)
}

fn episode_word(count: usize) -> &'static str {
    if count == 1 {
        "episode"
    } else {
        "episodes"
    }
}

fn compact_error(error: &str) -> String {
    let collapsed = error.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut chars = collapsed.chars();
    let shortened: String = chars.by_ref().take(240).collect();
    if chars.next().is_some() {
        let mut shortened: String = shortened.chars().take(239).collect();
        shortened.push('…');
        shortened
    } else {
        shortened
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        ffi::OsStr,
        fs,
        path::{Path, PathBuf},
        sync::atomic::{AtomicUsize, Ordering},
    };

    static TEMP_COUNTER: AtomicUsize = AtomicUsize::new(0);

    struct TempDir(PathBuf);

    impl TempDir {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "bob-listen-feed-{}-{}",
                std::process::id(),
                TEMP_COUNTER.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir_all(&path).expect("create test temp dir");
            Self(path)
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn write_entry(dir: &Path, name: &str, json: &str) {
        fs::create_dir_all(dir).expect("create outbox");
        fs::write(dir.join(name), json).expect("write outbox entry");
    }

    fn entry(
        episode_id: &str,
        queued_at: &str,
        last_error: &str,
    ) -> QueuedPublish {
        QueuedPublish {
            episode_id: episode_id.to_string(),
            queued_at: queued_at.to_string(),
            attempts: 1,
            last_error: last_error.to_string(),
        }
    }

    fn write_library_audio(
        library: &Path,
        id: &str,
        filename: &str,
        bytes: &[u8],
    ) {
        let episode = library.join(id);
        fs::create_dir_all(&episode).expect("create episode directory");
        fs::write(episode.join(filename), bytes).expect("write episode audio");
        fs::write(
            episode.join("manifest.json"),
            format!(r#"{{"audio":{{"file":"{filename}"}}}}"#),
        )
        .expect("write episode manifest");
    }

    #[test]
    fn read_outbox_skips_bad_entries_and_sorts() {
        let temp = TempDir::new();
        let outbox = temp.path().join("outbox");
        write_entry(
            &outbox,
            "z.json",
            r#"{"episode_id":"z","queued_at":"2026-10-02T00:00:00Z","attempts":2,"last_error":"z"}"#,
        );
        write_entry(
            &outbox,
            "a.json",
            r#"{"episode_id":"a","queued_at":"2026-10-02T00:00:00Z","attempts":1,"last_error":"a"}"#,
        );
        write_entry(
            &outbox,
            "older.json",
            r#"{"episode_id":"older","queued_at":"2026-10-01T00:00:00Z","attempts":3,"last_error":"old"}"#,
        );
        write_entry(&outbox, ".tmp-hidden.json", r#"{"episode_id":"hidden"}"#);
        write_entry(&outbox, "not-json.txt", r#"{"episode_id":"text"}"#);
        write_entry(&outbox, "invalid.json", "not json");
        fs::write(outbox.join("unreadable.json"), [0xFF])
            .expect("write invalid UTF-8 entry");
        write_entry(
            &outbox,
            "invalid-id.json",
            r#"{"episode_id":"../escape"}"#,
        );

        let parsed = read_outbox(&outbox);
        assert_eq!(
            parsed
                .iter()
                .map(|item| item.episode_id.as_str())
                .collect::<Vec<_>>(),
            ["older", "a", "z"]
        );
        assert_eq!(parsed[1].attempts, 1);
        assert!(read_outbox(&temp.path().join("missing")).is_empty());
    }

    #[test]
    fn feed_notice_is_none_for_empty_or_missing_outbox() {
        let temp = TempDir::new();
        let audio = temp.path().join("episode.mp3");
        fs::write(&audio, b"ID3 audio").expect("write input audio");
        let library = temp.path().join("library");
        let empty = temp.path().join("empty");
        fs::create_dir_all(&empty).expect("create empty outbox");
        assert!(feed_notice(&audio, &library, &empty).is_none());
        assert!(feed_notice(&audio, &library, &temp.path().join("missing"))
            .is_none());
    }

    #[test]
    fn feed_notice_matches_identical_audio_and_keeps_other_entries() {
        let temp = TempDir::new();
        let audio = temp.path().join("episode.mp3");
        fs::write(&audio, b"ID3 same bytes").expect("write input audio");
        let library = temp.path().join("library");
        write_library_audio(
            &library,
            "current",
            "current.mp3",
            b"ID3 same bytes",
        );
        write_library_audio(&library, "older", "older.mp3", b"ID3 older");
        let outbox = temp.path().join("outbox");
        write_entry(
            &outbox,
            "current.json",
            r#"{"episode_id":"current","queued_at":"2026-10-24T10:00:00Z","last_error":"current failed"}"#,
        );
        write_entry(
            &outbox,
            "older.json",
            r#"{"episode_id":"older","queued_at":"2026-10-23T10:00:00Z","last_error":"older failed"}"#,
        );

        let notice =
            feed_notice(&audio, &library, &outbox).expect("queued notice");
        assert_eq!(
            notice
                .this_episode
                .as_ref()
                .map(|item| item.episode_id.as_str()),
            Some("current")
        );
        assert_eq!(notice.others.len(), 1);
        assert_eq!(notice.others[0].episode_id, "older");
    }

    #[test]
    fn feed_notice_keeps_nonmatching_and_missing_library_entries_as_others() {
        let temp = TempDir::new();
        let audio = temp.path().join("episode.mp3");
        fs::write(&audio, b"ID3 source").expect("write input audio");
        let library = temp.path().join("library");
        write_library_audio(
            &library,
            "different",
            "different.mp3",
            b"ID3 other",
        );
        let outbox = temp.path().join("outbox");
        write_entry(
            &outbox,
            "different.json",
            r#"{"episode_id":"different","queued_at":"2026-10-22T00:00:00Z","last_error":"different"}"#,
        );
        write_entry(
            &outbox,
            "missing.json",
            r#"{"episode_id":"missing","queued_at":"2026-10-23T00:00:00Z","last_error":"missing"}"#,
        );

        let notice =
            feed_notice(&audio, &library, &outbox).expect("queued notice");
        assert!(notice.this_episode.is_none());
        assert_eq!(notice.others.len(), 2);
    }

    #[test]
    fn notice_lines_format_current_and_older_queues() {
        let current = FeedNotice {
            this_episode: Some(entry(
                "current",
                "2026-10-24T10:00:00Z",
                "  connection\n timeout  ",
            )),
            others: vec![
                entry("old-a", "2026-10-22T10:00:00Z", "old"),
                entry("old-b", "2026-10-23T10:00:00Z", "new"),
            ],
        };
        let (warning, hint) = notice_lines(&current);
        assert_eq!(
            warning,
            "podcast feed: this episode was not published (sase listen queued it: connection timeout); 2 earlier episodes are also queued, oldest since 2026-10-22"
        );
        assert_eq!(hint, FEED_HINT);

        let older = FeedNotice {
            this_episode: None,
            others: vec![
                entry("one", "2026-10-22T10:00:00Z", "old error"),
                entry("two", "2026-10-24T10:00:00Z", "new\n error"),
            ],
        };
        let (warning, _) = notice_lines(&older);
        assert_eq!(
            warning,
            "podcast feed: 2 episodes are still queued for publishing, oldest since 2026-10-22 (last error: new error)"
        );
    }

    #[test]
    fn notice_lines_pluralize_omit_invalid_dates_and_truncate_errors() {
        let single = FeedNotice {
            this_episode: None,
            others: vec![entry("one", "not-a-date", "  one\n failure ")],
        };
        let (warning, _) = notice_lines(&single);
        assert_eq!(
            warning,
            "podcast feed: 1 episode is still queued for publishing (last error: one failure)"
        );

        let current = FeedNotice {
            this_episode: Some(entry("current", "bad-date", &"x".repeat(300))),
            others: vec![entry("other", "bad-date", "old")],
        };
        let (warning, _) = notice_lines(&current);
        assert!(warning.contains("; 1 earlier episode is also queued"));
        assert!(!warning.contains("oldest since"));
        let error = warning
            .split("sase listen queued it: ")
            .nth(1)
            .and_then(|tail| tail.split(')').next())
            .expect("current episode error");
        assert_eq!(error.chars().count(), 240);
        assert!(error.ends_with('…'));
    }

    #[test]
    fn doctor_row_uses_configured_state_home_and_reports_queue() {
        let temp = TempDir::new();
        let state = temp.path().join("state");
        let listen =
            OsStr::new("sase listen render {target} -e full -o {audio}");
        let _guard = crate::native::env::TestEnvGuard::set(&[
            ("XDG_STATE_HOME", Some(state.as_os_str())),
            ("BOB_HIGHLIGHTS_LISTEN_COMMAND", Some(listen)),
        ]);
        let (row, warning) =
            listen_feed_doctor_row().expect("configured listen row");
        assert_eq!(row, "listen_feed: ok (no queued feed publishes)");
        assert!(warning.is_none());

        let outbox = state.join("sase-listen/outbox");
        write_entry(
            &outbox,
            "one.json",
            r#"{"episode_id":"one","queued_at":"2026-10-20T10:00:00Z","last_error":"temporary auth failure"}"#,
        );
        let (row, warning) =
            listen_feed_doctor_row().expect("configured listen row");
        assert_eq!(
            row,
            "listen_feed: warn (1 episode queued for the podcast feed, oldest since 2026-10-20; last error: temporary auth failure)"
        );
        let warning = warning.expect("doctor warning");
        assert!(warning.contains(&outbox.display().to_string()));
        assert!(warning.contains(FEED_HINT));
    }
}
