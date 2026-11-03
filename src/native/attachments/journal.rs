//! Attachment origin journal: `.bob/attachments/<host>.jsonl`
//! (`docs/attachments.md`).
//!
//! Every file `%c` or `bob attachments import` copies into the vault
//! appends one event recording where the copy came from:
//!
//! ```json
//! {"v":1,"ts":"2026-11-03T14:28:40-05:00","host":"apollo","kind":"attachment.import","saved":"img/20261103_142837.png","source":"/Users/bryan/tmp/screenshots/20261103_142837.png","display":"~/tmp/screenshots/20261103_142837.png","sha256":"<64 hex>","via":"capture"}
//! ```
//!
//! `source` is the absolute path the copy resolved (quotes, `file://`,
//! and `~/` expanded). `display` is `source` with the writer's home
//! directory rewritten as `~` (otherwise identical). `sha256` hashes the
//! copied bytes. `via` is `capture` for `%c`, otherwise the import
//! command's `--via` value. Reused destinations still get an event,
//! because the new source is still this embed's origin; snippet files
//! (`file/clip-*.md`) never do, because they are not attachments.
//!
//! The fold is exactly the shared namespaced rule: read every
//! `*.jsonl`, skip malformed lines with a bounded warning, sort by
//! `(ts instant, host, path, line)`, last writer wins. Bob is the only
//! writer: `%c` and import append under `bob_sync.lock` with a 3 s wait,
//! one `O_APPEND` write per event, and the directory is created on first
//! use.

use std::{io, path::Path};

use chrono::{DateTime, FixedOffset};
use serde_json::{json, Value};

use crate::native::{
    env as bob_env,
    pref_journal::{self, StoredEvent},
};

const NAMESPACE: pref_journal::Namespace = pref_journal::ATTACHMENTS;

pub(crate) const IMPORT_KIND: &str = "attachment.import";

/// One recorded copy: which vault file it saved and where it came from,
/// plus writer-order metadata so resolution can pick the newest.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct OriginEvent {
    pub(crate) ts: DateTime<FixedOffset>,
    pub(crate) host: String,
    pub(crate) path: String,
    pub(crate) line: u32,
    pub(crate) saved: String,
    pub(crate) source: String,
    pub(crate) display: String,
    pub(crate) sha256: String,
    pub(crate) via: String,
}

pub(crate) fn host_label() -> String {
    pref_journal::host_label()
}

pub(crate) fn event_timestamp() -> DateTime<FixedOffset> {
    pref_journal::event_timestamp()
}

/// The `display` form of an absolute source: the writer's home directory
/// prefix rewritten as `~`, otherwise byte-identical to `source`.
pub(crate) fn display_for_source(source: &str) -> String {
    let home = bob_env::home_dir();
    if home.as_os_str().is_empty() {
        return source.to_string();
    }
    let Ok(relative) = Path::new(source).strip_prefix(&home) else {
        return source.to_string();
    };
    if relative.as_os_str().is_empty() {
        return "~".to_string();
    }
    format!("~/{}", relative.display())
}

pub(crate) fn serialize_event(
    ts: DateTime<FixedOffset>,
    host: &str,
    saved: &str,
    source: &str,
    display: &str,
    sha256: &str,
    via: &str,
) -> String {
    let ts = ts.to_rfc3339_opts(chrono::SecondsFormat::Secs, false);
    let value = json!({
        "v": 1,
        "ts": ts,
        "host": host,
        "kind": IMPORT_KIND,
        "saved": saved,
        "source": source,
        "display": display,
        "sha256": sha256,
        "via": via,
    });
    let mut line =
        serde_json::to_string(&value).unwrap_or_else(|_| "{}".to_string());
    line.push('\n');
    line
}

/// Append one origin event with a single `O_APPEND` write. Creates
/// `.bob/attachments/` on first use. The caller holds `bob_sync.lock`.
pub(crate) fn append_event(
    vault: &Path,
    host: &str,
    saved: &str,
    source: &str,
    display: &str,
    sha256: &str,
    via: &str,
    ts: DateTime<FixedOffset>,
) -> io::Result<String> {
    let line = serialize_event(ts, host, saved, source, display, sha256, via);
    pref_journal::append_line(vault, NAMESPACE, host, &line)?;
    Ok(line)
}

/// Append one origin event per attachment after a successful `%c` commit
/// or import save. Reused destinations still get an event, because the
/// new source is still this embed's origin. Failures never fail the
/// caller: each one comes back with the attachment it belongs to.
pub(crate) fn append_origin_events(
    vault: &Path,
    attachments: &[(String, String, String)],
    via: &str,
) -> Vec<OriginFailure> {
    if attachments.is_empty() {
        return Vec::new();
    }
    let host = host_label();
    let ts = event_timestamp();
    let mut failures = Vec::new();
    for (saved, source, sha256) in attachments {
        let display = display_for_source(source);
        if let Err(error) =
            append_event(vault, &host, saved, source, &display, sha256, via, ts)
        {
            failures.push(OriginFailure {
                saved: saved.clone(),
                error: format!(
                    "append attachment journal in {}: {error}",
                    vault.display()
                ),
            });
        }
    }
    failures
}

/// One origin journal append the commit or import could not write.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct OriginFailure {
    pub(crate) saved: String,
    pub(crate) error: String,
}

pub(crate) fn read_journal(
    vault: &Path,
) -> io::Result<(Vec<OriginEvent>, Vec<String>)> {
    let (stored, warnings) = pref_journal::read_raw(vault, NAMESPACE)?;
    let mut events = Vec::new();
    let mut malformed = 0usize;
    let mut warnings = warnings;
    for event in &stored {
        let Some(origin) = parse_event(event) else {
            malformed += 1;
            if malformed <= pref_journal::MAX_MALFORMED_WARNINGS {
                warnings.push(format!(
                    "skipping malformed attachment journal line {}:{}",
                    event.path, event.line
                ));
            }
            continue;
        };
        events.push(origin);
    }
    if malformed > pref_journal::MAX_MALFORMED_WARNINGS {
        warnings.push(format!(
            "skipped {malformed} malformed attachment journal lines"
        ));
    }
    Ok((events, warnings))
}

fn parse_event(event: &StoredEvent) -> Option<OriginEvent> {
    if event.kind != IMPORT_KIND {
        return None;
    }
    let object = event.value.as_object()?;
    let saved = non_empty(object, "saved")?;
    let source = non_empty(object, "source")?;
    let display = non_empty(object, "display")?;
    let sha256 = non_empty(object, "sha256")?;
    if sha256.len() != 64
        || !sha256.bytes().all(|byte| byte.is_ascii_hexdigit())
    {
        return None;
    }
    let via = non_empty(object, "via")?;
    Some(OriginEvent {
        ts: event.ts,
        host: event.host.clone(),
        path: event.path.clone(),
        line: event.line,
        saved,
        source,
        display,
        sha256,
        via,
    })
}

fn non_empty(
    object: &serde_json::Map<String, Value>,
    field: &str,
) -> Option<String> {
    let value = object.get(field)?.as_str()?;
    (!value.is_empty()).then(|| value.to_string())
}
