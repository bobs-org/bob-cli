//! Bounded Gmail search and read-only metadata previews.

use std::{
    collections::BTreeMap,
    fs::{self, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};

use chrono::{DateTime, SecondsFormat};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::{
    body::{importable_messages, inert_single_line},
    cli::{PeekArgs, SearchArgs},
    config::GmailConfig,
    gog::Gog,
    model::{GmailThread, HeaderKind},
    note::{label, message_time, subject_without_prefixes, thread_who},
    resolve, ui, GmailError,
};
use crate::native::{env as bob_env, style, style::Styler};

const DEFAULT_QUERY: &str = "in:inbox newer_than:30d";
const MAX_LIMIT: u32 = 50;
const CACHE_TTL_SECS: u64 = 10 * 60;
const CACHE_MAX_ENTRIES: usize = 64;
const CACHE_VERSION: u8 = 1;
static CACHE_TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone, PartialEq, Eq)]
struct ThreadMetadata {
    subject: String,
    who: String,
    messages: usize,
    last_date: Option<String>,
    snippet: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct CachedMetadata {
    subject: String,
    who: String,
    messages: usize,
    last_date: Option<String>,
    fetched_at: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct CacheDocument {
    cache_version: u8,
    entries: BTreeMap<String, CachedMetadata>,
}

#[derive(Debug)]
struct SearchRow {
    thread_id: String,
    metadata: ThreadMetadata,
    saved_note: Option<String>,
}

#[derive(Debug)]
struct PeekResult {
    verdict: &'static str,
    thread_id: Option<String>,
    subject: Option<String>,
    who: Option<String>,
    messages: Option<usize>,
    last_date: Option<String>,
    saved_note: Option<String>,
    label: Option<String>,
    message: String,
}

pub(crate) fn search(args: &SearchArgs) -> i32 {
    match search_inner(args) {
        Ok(rows) => {
            print_search(args, &rows);
            0
        }
        Err(error) => ui::report_error(
            "search",
            &error,
            if args.format.is_json() {
                "json"
            } else {
                "human"
            },
        ),
    }
}

fn search_inner(args: &SearchArgs) -> Result<Vec<SearchRow>, GmailError> {
    let config = GmailConfig::resolve()?;
    let vault = bob_env::bob_dir();
    if !vault.is_dir() {
        return Err(GmailError::setup(
            "vault",
            format!("Bob vault root does not exist: {}", vault.display()),
        ));
    }
    let gog = Gog::resolve(crate::native::google::GMAIL, config.timeout())?;
    gog.require_version()?;
    let account = resolve::resolve_account(&gog, None, config.account())?;
    let index = resolve::scan_index(
        &vault,
        Path::new(config.folder()),
        config.link_label(),
    )?;
    let query = args
        .query
        .as_deref()
        .map(str::trim)
        .filter(|query| !query.is_empty())
        .unwrap_or(DEFAULT_QUERY);
    let limit = args.limit.clamp(1, MAX_LIMIT);
    let result = gog.search(&account, query, limit)?;
    let threads =
        result
            .get("threads")
            .and_then(Value::as_array)
            .ok_or_else(|| {
                bad_search_result("gog returned an invalid Gmail search result")
            })?;

    let mut rows = Vec::with_capacity(threads.len().min(limit as usize));
    for item in threads.iter().take(limit as usize) {
        let thread_id = item
            .get("id")
            .and_then(Value::as_str)
            .filter(|id| is_hex_id(id))
            .ok_or_else(|| {
                bad_search_result(
                    "gog returned a search result without a valid thread ID",
                )
            })?
            .to_ascii_lowercase();
        let thread = gog.thread_metadata(&account, &thread_id)?;
        if thread.id.to_ascii_lowercase() != thread_id {
            return Err(GmailError::runtime(
                "unresolvable",
                "Gmail returned a different thread identity".to_string(),
            ));
        }
        let metadata = summarize_thread(&thread, &account)?;
        let saved_note = index
            .by_identity
            .get(&(account.to_ascii_lowercase(), thread_id.clone()))
            .map(|entry| entry.path.clone());
        rows.push(SearchRow {
            thread_id,
            metadata,
            saved_note,
        });
    }
    Ok(rows)
}

pub(crate) fn peek(args: &PeekArgs) -> i32 {
    match peek_inner(args) {
        Ok(result) => {
            print_peek(args, &result);
            0
        }
        Err(error) => ui::report_error("peek", &error, args.format.as_str()),
    }
}

fn peek_inner(args: &PeekArgs) -> Result<PeekResult, GmailError> {
    let config = GmailConfig::resolve()?;
    let vault = bob_env::bob_dir();
    if !vault.is_dir() {
        return Err(GmailError::setup(
            "vault",
            format!("Bob vault root does not exist: {}", vault.display()),
        ));
    }
    let gog = Gog::resolve(crate::native::google::GMAIL, config.timeout())?;
    gog.require_version()?;
    let account = resolve::resolve_account(&gog, None, config.account())?;
    let folder = Path::new(config.folder());
    let index = resolve::scan_index(&vault, folder, config.link_label())?;
    let identity = match resolve::resolve_identity(
        &gog,
        &account,
        &vault,
        folder,
        &index,
        &args.input,
    ) {
        Ok(identity) => identity,
        Err(error) if matches!(error.kind(), "unresolvable" | "refused") => {
            return Ok(unresolved_peek(if error.kind() == "refused" {
                "refused"
            } else {
                "unresolvable"
            }));
        }
        Err(error) => return Err(error),
    };
    let key = cache_key(&identity.account, &identity.thread_id);
    let cache_path = cache_path();
    let metadata = if let Some(metadata) =
        cache_get(&cache_path, &key, now_unix_seconds())
    {
        metadata
    } else {
        let thread =
            match gog.thread_metadata(&identity.account, &identity.thread_id) {
                Ok(thread) => thread,
                Err(error) if error.kind() == "not_found" => {
                    return Ok(unresolved_peek("unresolvable"));
                }
                Err(error) => return Err(error),
            };
        if thread.id.to_ascii_lowercase() != identity.thread_id {
            return Ok(unresolved_peek("unresolvable"));
        }
        let metadata = summarize_thread(&thread, &identity.account)?;
        cache_put(&cache_path, &key, &metadata, now_unix_seconds());
        metadata
    };

    let saved_note = identity.known_path.or_else(|| {
        index
            .by_identity
            .get(&(
                identity.account.to_ascii_lowercase(),
                identity.thread_id.clone(),
            ))
            .map(|entry| entry.path.clone())
    });
    let label = label::render_saved(
        config.link_label(),
        &metadata.subject,
        &metadata.who,
    );
    let message = preview_line(&metadata);
    Ok(PeekResult {
        verdict: "resolved",
        thread_id: Some(identity.thread_id),
        subject: Some(metadata.subject),
        who: Some(metadata.who),
        messages: Some(metadata.messages),
        last_date: metadata.last_date,
        saved_note,
        label: Some(label),
        message,
    })
}

fn unresolved_peek(verdict: &'static str) -> PeekResult {
    PeekResult {
        verdict,
        thread_id: None,
        subject: None,
        who: None,
        messages: None,
        last_date: None,
        saved_note: None,
        label: None,
        message: "Gmail link could not be resolved".to_string(),
    }
}

/// Read the live count for one already indexed thread. The call is metadata
/// only and is intentionally uncached so `list --remote` always means live.
pub(crate) fn remote_message_count(
    gog: &Gog,
    account: &str,
    thread_id: &str,
) -> Result<usize, GmailError> {
    let thread = gog.thread_metadata(account, thread_id)?;
    if !thread.id.eq_ignore_ascii_case(thread_id)
        || thread
            .messages
            .iter()
            .any(|message| !message.thread_id.eq_ignore_ascii_case(&thread.id))
    {
        return Err(GmailError::runtime(
            "unresolvable",
            "Gmail returned a different thread identity".to_string(),
        ));
    }
    Ok(importable_messages(&thread).count())
}

fn summarize_thread(
    thread: &GmailThread,
    account: &str,
) -> Result<ThreadMetadata, GmailError> {
    if thread
        .messages
        .iter()
        .any(|message| !message.thread_id.eq_ignore_ascii_case(&thread.id))
    {
        return Err(GmailError::runtime(
            "unresolvable",
            "Gmail returned a different thread identity".to_string(),
        ));
    }
    let messages: Vec<_> = importable_messages(thread).collect();
    let Some(first) = messages.first().copied() else {
        return Err(bad_search_result(
            "Gmail returned a thread without messages",
        ));
    };
    let last = messages
        .iter()
        .copied()
        .max_by_key(|message| message_time(message))
        .unwrap_or(first);
    let raw_subject = first
        .first_header(HeaderKind::Subject)
        .map(subject_without_prefixes)
        .unwrap_or_default();
    let raw_subject = if raw_subject.is_empty() {
        "(no subject)".to_string()
    } else {
        raw_subject
    };
    let subject = inert_single_line(&raw_subject);
    let who = inert_single_line(&thread_who(first, account));
    let last_date = message_time(last)
        .map(|date| date.to_rfc3339_opts(SecondsFormat::Secs, true));
    let snippet = last
        .snippet
        .as_deref()
        .map(inert_single_line)
        .map(|value| truncate_chars(&value, 140))
        .unwrap_or_default();
    Ok(ThreadMetadata {
        subject,
        who,
        messages: messages.len(),
        last_date,
        snippet,
    })
}

fn preview_line(metadata: &ThreadMetadata) -> String {
    let date = metadata
        .last_date
        .as_deref()
        .and_then(|value| DateTime::parse_from_rfc3339(value).ok())
        .map(|value| value.format("%b %-d").to_string())
        .unwrap_or_else(|| "date unknown".to_string());
    format!(
        "✉️ {} · {} · {} messages · {date}",
        metadata.subject, metadata.who, metadata.messages
    )
}

fn truncate_chars(value: &str, limit: usize) -> String {
    let mut chars = value.chars();
    let truncated: String = chars.by_ref().take(limit).collect();
    if chars.next().is_some() {
        format!("{truncated}…")
    } else {
        truncated
    }
}

fn is_hex_id(value: &str) -> bool {
    !value.is_empty() && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn bad_search_result(message: &str) -> GmailError {
    GmailError::runtime("bad_json", message.to_string())
}

fn print_search(args: &SearchArgs, rows: &[SearchRow]) {
    if args.format.is_json() {
        println!(
            "{}",
            json!({
                "schema_version": 1,
                "ok": true,
                "query": args.query.as_deref().map(str::trim).filter(|q| !q.is_empty()).unwrap_or(DEFAULT_QUERY),
                "shown": rows.len(),
                "limit": args.limit.min(MAX_LIMIT),
                "threads": rows.iter().map(|row| json!({
                    "thread_id": row.thread_id,
                    "subject": row.metadata.subject,
                    "who": row.metadata.who,
                    "last_date": row.metadata.last_date,
                    "messages": row.metadata.messages,
                    "snippet": row.metadata.snippet,
                    "saved_note": row.saved_note,
                })).collect::<Vec<_>>(),
            })
        );
        ui::flush_stdout();
        return;
    }
    if rows.is_empty() {
        println!("No Gmail threads matched.");
        ui::flush_stdout();
        return;
    }
    let styler = Styler::detect();
    let headers = [
        "thread_id",
        "subject",
        "who",
        "last",
        "messages",
        "snippet",
        "saved",
    ];
    let rendered: Vec<[String; 7]> = rows
        .iter()
        .map(|row| {
            [
                row.thread_id.clone(),
                row.metadata.subject.clone(),
                row.metadata.who.clone(),
                preview_date(row.metadata.last_date.as_deref()),
                row.metadata.messages.to_string(),
                row.metadata.snippet.clone(),
                row.saved_note.clone().unwrap_or_default(),
            ]
        })
        .collect();
    let widths: Vec<usize> = (0..headers.len())
        .map(|column| {
            rendered
                .iter()
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
    for row in rendered {
        println!(
            "{}",
            row.iter()
                .zip(widths.iter())
                .map(|(cell, width)| style::pad_right(cell, *width))
                .collect::<Vec<_>>()
                .join("  ")
        );
    }
    ui::flush_stdout();
}

fn preview_date(raw: Option<&str>) -> String {
    raw.and_then(|value| DateTime::parse_from_rfc3339(value).ok())
        .map(|value| value.format("%b %-d").to_string())
        .unwrap_or_else(|| "—".to_string())
}

fn print_peek(args: &PeekArgs, result: &PeekResult) {
    if args.format.is_json() {
        println!(
            "{}",
            json!({
                "schema_version": 1,
                "ok": true,
                "verdict": result.verdict,
                "thread_id": result.thread_id,
                "subject": result.subject,
                "who": result.who,
                "messages": result.messages,
                "last_date": result.last_date,
                "saved_note": result.saved_note,
                "label": result.label,
                "message": result.message,
            })
        );
    } else {
        println!("{}", result.message);
    }
    ui::flush_stdout();
}

fn cache_path() -> PathBuf {
    bob_env::bob_cli_cache_dir().join("gmail").join("peek.json")
}

fn cache_key(account: &str, thread_id: &str) -> String {
    format!(
        "{}/{}",
        account.trim().to_ascii_lowercase(),
        thread_id.to_ascii_lowercase()
    )
}

fn cache_get(path: &Path, key: &str, now: u64) -> Option<ThreadMetadata> {
    let document = read_cache(path, now);
    let cached = document.entries.get(key)?;
    Some(ThreadMetadata {
        subject: cached.subject.clone(),
        who: cached.who.clone(),
        messages: cached.messages,
        last_date: cached.last_date.clone(),
        snippet: String::new(),
    })
}

fn cache_put(path: &Path, key: &str, metadata: &ThreadMetadata, now: u64) {
    let mut document = read_cache(path, now);
    document.entries.insert(
        key.to_string(),
        CachedMetadata {
            subject: metadata.subject.clone(),
            who: metadata.who.clone(),
            messages: metadata.messages,
            last_date: metadata.last_date.clone(),
            fetched_at: now,
        },
    );
    while document.entries.len() > CACHE_MAX_ENTRIES {
        let oldest = document
            .entries
            .iter()
            .min_by_key(|(_, entry)| entry.fetched_at)
            .map(|(key, _)| key.clone());
        let Some(oldest) = oldest else { break };
        document.entries.remove(&oldest);
    }
    let _ = write_cache(path, &document);
}

fn read_cache(path: &Path, now: u64) -> CacheDocument {
    let Ok(text) = fs::read_to_string(path) else {
        return CacheDocument {
            cache_version: CACHE_VERSION,
            entries: BTreeMap::new(),
        };
    };
    let Ok(mut document) = serde_json::from_str::<CacheDocument>(&text) else {
        return CacheDocument {
            cache_version: CACHE_VERSION,
            entries: BTreeMap::new(),
        };
    };
    if document.cache_version != CACHE_VERSION {
        return CacheDocument {
            cache_version: CACHE_VERSION,
            entries: BTreeMap::new(),
        };
    }
    document.entries.retain(|_, entry| {
        now >= entry.fetched_at
            && now.saturating_sub(entry.fetched_at) <= CACHE_TTL_SECS
    });
    document
}

fn write_cache(path: &Path, document: &CacheDocument) -> std::io::Result<()> {
    let Some(parent) = path.parent() else {
        return Ok(());
    };
    fs::create_dir_all(parent)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
        let mut builder = fs::DirBuilder::new();
        builder.mode(0o700);
        let _ = builder.create(parent);
        fs::set_permissions(parent, fs::Permissions::from_mode(0o700))?;
    }
    let sequence = CACHE_TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let temp = path.with_extension(format!(
        "json.tmp.{}.{}",
        std::process::id(),
        sequence
    ));
    let bytes = serde_json::to_vec(document).map_err(std::io::Error::other)?;
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(&temp)?;
    if let Err(error) = file.write_all(&bytes).and_then(|()| file.sync_all()) {
        let _ = fs::remove_file(&temp);
        return Err(error);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&temp, fs::Permissions::from_mode(0o600))?;
    }
    fs::rename(&temp, path).inspect_err(|_| {
        let _ = fs::remove_file(&temp);
    })
}

fn now_unix_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn search_query_defaults_and_limit_is_bounded() {
        assert_eq!(DEFAULT_QUERY, "in:inbox newer_than:30d");
        assert_eq!(MAX_LIMIT, 50);
        assert_eq!(99u32.clamp(1, MAX_LIMIT), 50);
    }

    #[test]
    fn cache_is_ttl_bounded_and_contains_metadata_fields_only() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("gmail/peek.json");
        let metadata = ThreadMetadata {
            subject: "subject".to_string(),
            who: "sender".to_string(),
            messages: 2,
            last_date: Some("2026-11-05T12:00:00Z".to_string()),
            snippet: "private body excerpt".to_string(),
        };
        cache_put(&path, "you@example.com/0123456789abcdef", &metadata, 1000);
        let cache_text = fs::read_to_string(&path).unwrap();
        assert!(cache_text.contains("subject"));
        assert!(!cache_text.contains("snippet"));
        assert!(!cache_text.contains("private body excerpt"));
        assert!(cache_get(&path, "you@example.com/0123456789abcdef", 1599)
            .is_some());
        assert!(cache_get(&path, "you@example.com/0123456789abcdef", 1601)
            .is_none());
    }

    #[cfg(unix)]
    #[test]
    fn cache_is_private_on_unix() {
        use std::os::unix::fs::PermissionsExt;

        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("gmail/peek.json");
        cache_put(
            &path,
            "you@example.com/0123456789abcdef",
            &ThreadMetadata {
                subject: "subject".to_string(),
                who: "sender".to_string(),
                messages: 2,
                last_date: None,
                snippet: String::new(),
            },
            1000,
        );
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert_eq!(
            fs::metadata(path.parent().unwrap())
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
    }
}
