use std::{
    collections::BTreeMap,
    fs, io,
    path::{Path, PathBuf},
};

use chrono::{DateTime, FixedOffset};
use serde_json::Value;

use crate::native::pomodoro_homes::Hue;

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
