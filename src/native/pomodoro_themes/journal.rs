use std::{
    collections::BTreeMap,
    fs, io,
    io::Write,
    path::{Path, PathBuf},
};

use chrono::{DateTime, FixedOffset, Local, TimeZone};
use serde_json::{json, Value};

use crate::native::{env as bob_env, pomodoro_homes::Hue, vault_sync};

const JOURNAL_DIR: &str = ".bob/themes";
const MAX_MALFORMED_WARNINGS: usize = 8;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum EventPayload {
    Hide {
        key: String,
        #[allow(dead_code)]
        name: String,
    },
    Unhide {
        key: String,
        #[allow(dead_code)]
        name: String,
    },
    Color {
        key: String,
        #[allow(dead_code)]
        name: String,
        hue: Option<Hue>,
    },
    Rename {
        from: String,
        to: String,
        #[allow(dead_code)]
        from_name: String,
        #[allow(dead_code)]
        to_name: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct JournalEvent {
    pub(crate) ts: DateTime<FixedOffset>,
    pub(crate) host: String,
    pub(crate) path: String,
    pub(crate) line: u32,
    pub(crate) payload: EventPayload,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct KeyPref {
    pub(crate) hidden: bool,
    pub(crate) pin: Option<Hue>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Prefs {
    pub(crate) by_key: BTreeMap<String, KeyPref>,
}

impl Prefs {
    pub(crate) fn hidden(&self, key: &str) -> bool {
        self.by_key.get(key).is_some_and(|pref| pref.hidden)
    }

    pub(crate) fn pin(&self, key: &str) -> Option<Hue> {
        self.by_key.get(key).and_then(|pref| pref.pin)
    }
}

pub(crate) fn load_prefs(vault: &Path) -> io::Result<(Prefs, Vec<String>)> {
    let (events, warnings) = read_journal(vault)?;
    Ok((fold_events(&events), warnings))
}

pub(crate) fn host_label() -> String {
    vault_sync::hostname_slug()
}

pub(crate) fn journal_dir(vault: &Path) -> PathBuf {
    vault.join(JOURNAL_DIR)
}

pub(crate) fn host_journal_path(vault: &Path, host: &str) -> PathBuf {
    journal_dir(vault).join(format!("{host}.jsonl"))
}

pub(crate) fn event_timestamp() -> DateTime<FixedOffset> {
    let naive = bob_env::current_datetime();
    if let Some(timezone) = bob_env::configured_timezone()
        && let Some(datetime) = timezone.from_local_datetime(&naive).single()
    {
        return datetime.fixed_offset();
    }
    Local
        .from_local_datetime(&naive)
        .single()
        .or_else(|| Local.from_local_datetime(&naive).earliest())
        .map(|datetime| datetime.fixed_offset())
        .unwrap_or_else(|| {
            DateTime::from_naive_utc_and_offset(
                naive,
                FixedOffset::east_opt(0).unwrap(),
            )
        })
}

pub(crate) fn serialize_event(
    ts: DateTime<FixedOffset>,
    host: &str,
    payload: &EventPayload,
    via: &str,
) -> String {
    let ts = ts.to_rfc3339_opts(chrono::SecondsFormat::Secs, false);
    let value = match payload {
        EventPayload::Hide { key, name } => json!({
            "v": 1,
            "ts": ts,
            "host": host,
            "kind": "theme.hide",
            "key": key,
            "name": name,
            "via": via,
        }),
        EventPayload::Unhide { key, name } => json!({
            "v": 1,
            "ts": ts,
            "host": host,
            "kind": "theme.unhide",
            "key": key,
            "name": name,
            "via": via,
        }),
        EventPayload::Color { key, name, hue } => json!({
            "v": 1,
            "ts": ts,
            "host": host,
            "kind": "theme.color",
            "key": key,
            "name": name,
            "hue": hue.map(Hue::name),
            "via": via,
        }),
        EventPayload::Rename {
            from,
            to,
            from_name,
            to_name,
        } => json!({
            "v": 1,
            "ts": ts,
            "host": host,
            "kind": "theme.rename",
            "from": from,
            "to": to,
            "from_name": from_name,
            "to_name": to_name,
            "via": via,
        }),
    };
    let mut line =
        serde_json::to_string(&value).unwrap_or_else(|_| "{}".to_string());
    line.push('\n');
    line
}

/// Append one journal event with a single `O_APPEND` write. Creates
/// `.bob/themes/` on first use.
pub(crate) fn append_event_at(
    vault: &Path,
    host: &str,
    payload: &EventPayload,
    via: &str,
    ts: DateTime<FixedOffset>,
) -> io::Result<String> {
    let directory = journal_dir(vault);
    fs::create_dir_all(&directory)?;
    let path = host_journal_path(vault, host);
    let line = serialize_event(ts, host, payload, via);
    let mut options = fs::OpenOptions::new();
    options.create(true).append(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(0o644);
    }
    let mut file = options.open(&path)?;
    file.write_all(line.as_bytes())?;
    Ok(line)
}

pub(crate) fn read_journal(
    vault: &Path,
) -> io::Result<(Vec<JournalEvent>, Vec<String>)> {
    let directory = vault.join(JOURNAL_DIR);
    if !directory.is_dir() {
        return Ok((Vec::new(), Vec::new()));
    }

    let mut paths = match fs::read_dir(&directory) {
        Ok(entries) => entries
            .filter_map(|entry| entry.ok())
            .map(|entry| entry.path())
            .filter(|path| {
                path.extension().is_some_and(|ext| ext == "jsonl")
                    && path.is_file()
            })
            .collect::<Vec<PathBuf>>(),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Ok((Vec::new(), Vec::new()));
        }
        Err(error) => return Err(error),
    };
    paths.sort();

    let mut events = Vec::new();
    let mut malformed = 0usize;
    let mut warnings = Vec::new();
    for path in paths {
        let bytes = match fs::read(&path) {
            Ok(bytes) => bytes,
            Err(_) => continue,
        };
        let relative = path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        for (index, line) in bytes.split(|byte| *byte == b'\n').enumerate() {
            if line.is_empty()
                || line.iter().all(|byte| byte.is_ascii_whitespace())
            {
                continue;
            }
            let Ok(text) = std::str::from_utf8(line) else {
                malformed += 1;
                push_malformed(&mut warnings, &relative, index + 1, malformed);
                continue;
            };
            let Some(event) = parse_event(
                text,
                &relative,
                u32::try_from(index + 1).unwrap_or(u32::MAX),
            ) else {
                malformed += 1;
                push_malformed(&mut warnings, &relative, index + 1, malformed);
                continue;
            };
            events.push(event);
        }
    }
    if malformed > MAX_MALFORMED_WARNINGS {
        warnings
            .push(format!("skipped {malformed} malformed theme journal lines"));
    }
    Ok((events, warnings))
}

fn push_malformed(
    warnings: &mut Vec<String>,
    path: &str,
    line: usize,
    count: usize,
) {
    if count <= MAX_MALFORMED_WARNINGS {
        warnings.push(format!(
            "skipping malformed theme journal line {path}:{line}"
        ));
    }
}

pub(crate) fn parse_event(
    line: &str,
    path: &str,
    line_number: u32,
) -> Option<JournalEvent> {
    let value: Value = serde_json::from_str(line.trim()).ok()?;
    let object = value.as_object()?;
    if object.get("v").and_then(Value::as_u64) != Some(1) {
        return None;
    }
    let ts = object
        .get("ts")
        .and_then(Value::as_str)
        .and_then(|ts| DateTime::parse_from_rfc3339(ts).ok())?;
    let host = object.get("host").and_then(Value::as_str)?.to_string();
    if host.is_empty() {
        return None;
    }
    let kind = object.get("kind").and_then(Value::as_str)?;
    let payload = match kind {
        "theme.hide" => EventPayload::Hide {
            key: required_key(object, "key")?,
            name: optional_name(object, "name"),
        },
        "theme.unhide" => EventPayload::Unhide {
            key: required_key(object, "key")?,
            name: optional_name(object, "name"),
        },
        "theme.color" => {
            let hue = match object.get("hue") {
                None => return None,
                Some(Value::Null) => None,
                Some(value) => Some(Hue::parse(value.as_str()?)?),
            };
            EventPayload::Color {
                key: required_key(object, "key")?,
                name: optional_name(object, "name"),
                hue,
            }
        }
        "theme.rename" => EventPayload::Rename {
            from: required_key(object, "from")?,
            to: required_key(object, "to")?,
            from_name: optional_name(object, "from_name"),
            to_name: optional_name(object, "to_name"),
        },
        _ => return None,
    };
    Some(JournalEvent {
        ts,
        host,
        path: path.to_string(),
        line: line_number,
        payload,
    })
}

fn required_key(
    object: &serde_json::Map<String, Value>,
    field: &str,
) -> Option<String> {
    let key = object.get(field).and_then(Value::as_str)?.trim();
    if key.is_empty() {
        return None;
    }
    Some(crate::native::plan_budget::normalize_component(key))
}

fn optional_name(
    object: &serde_json::Map<String, Value>,
    field: &str,
) -> String {
    object
        .get(field)
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string()
}

pub(crate) fn fold_events(events: &[JournalEvent]) -> Prefs {
    let mut ordered = events.iter().collect::<Vec<_>>();
    ordered.sort_by(|left, right| {
        left.ts
            .cmp(&right.ts)
            .then_with(|| left.host.cmp(&right.host))
            .then_with(|| left.path.cmp(&right.path))
            .then_with(|| left.line.cmp(&right.line))
    });

    let mut prefs = Prefs::default();
    for event in ordered {
        match &event.payload {
            EventPayload::Hide { key, .. } => {
                prefs.by_key.entry(key.clone()).or_default().hidden = true;
            }
            EventPayload::Unhide { key, .. } => {
                prefs.by_key.entry(key.clone()).or_default().hidden = false;
            }
            EventPayload::Color { key, hue, .. } => {
                prefs.by_key.entry(key.clone()).or_default().pin = *hue;
            }
            EventPayload::Rename { from, to, .. } => {
                if from == to {
                    continue;
                }
                let source = prefs.by_key.get(from).cloned();
                let target_has_state = prefs.by_key.contains_key(to);
                if let Some(source) = source
                    && !target_has_state
                {
                    prefs.by_key.insert(to.clone(), source);
                    prefs.by_key.remove(from);
                }
            }
        }
    }
    prefs
}
