//! Contract validation wraps the parsers used by Bob's readers.

use std::collections::BTreeSet;

use serde_yaml::Value;

use super::{error, parent, settings};
use crate::native::props::{
    document::PropsDocument, protocol::Op, InspectContext, PropsError,
};
use crate::native::{
    capture_targets::alias_conflict_warnings,
    freshness::state::parse_note_refresh,
    note_ready::{parse_ready_cap, CapSource},
    plan_budget::normalize_component,
    pomodoro_homes::{
        NoteColor, NoteFacts, NoteOffClock, NoteOffClockTypes, NoteTheme,
    },
    projects::{parse_frontmatter, project_name_aliases},
};

/// A canonical single-key document lets existing frontmatter readers validate
/// semantic values even when the input key was quoted or in a flow mapping.
pub(crate) fn single_key(key: &str, value: &Value) -> String {
    let mut mapping = serde_yaml::Mapping::new();
    mapping.insert(Value::String(key.to_owned()), value.clone());
    format!(
        "---\n{}---\n",
        serde_yaml::to_string(&mapping).expect("YAML value")
    )
}

pub(crate) fn bad(key: &str, index: usize, allowed: &str) -> PropsError {
    error(
        "prop_bad_value",
        key,
        index,
        format!("invalid {key} value"),
        allowed,
    )
}

/// Return the canonical value for typed stages, plus nonfatal diagnostics.
pub(crate) fn value(
    context: &InspectContext,
    candidate: &str,
    key: &str,
    value: &Value,
    index: usize,
) -> Result<(Value, Vec<PropsError>), PropsError> {
    let text = single_key(key, value);
    let facts = NoteFacts::parse(&text);
    let mut warnings = Vec::new();
    let canonical = match key {
        "title" if context.roles.note_kind == crate::native::props::roles::NoteKind::Reference => {
            let title = value.as_str().ok_or_else(|| bad(key, index, "Use one nonblank line of text."))?;
            if title.trim().is_empty() || title.contains(['\n', '\r']) {
                return Err(bad(key, index, "Use one nonblank line of text."));
            }
            value.clone()
        }
        "summary" => {
            let summary = value.as_str().ok_or_else(|| bad(key, index, "Use one line of text, at most 140 characters recommended."))?;
            if summary.contains(['\n', '\r']) {
                return Err(error("area_summary_multiline", key, index, "summary must fit on one line", "Use one line of text; keep detail in the note body."));
            }
            if summary.chars().count() > 140 {
                warnings.push(error("area_summary_long", key, index, "summary exceeds 140 characters", "Keep the catalog summary to at most 140 characters."));
            }
            value.clone()
        }
        "author" if context.roles.note_kind == crate::native::props::roles::NoteKind::Reference => {
            let author = value.as_str().ok_or_else(|| bad(key, index, "Use one line of text."))?;
            if author.contains(['\n', '\r']) {
                return Err(bad(key, index, "Use one line of text."));
            }
            value.clone()
        }
        "published" if context.roles.note_kind == crate::native::props::roles::NoteKind::Reference => {
            let date = value.as_str().ok_or_else(|| bad(key, index, "Use YYYY, YYYY-MM, or YYYY-MM-DD."))?;
            if !valid_published(date) {
                return Err(bad(key, index, "Use YYYY, YYYY-MM, or YYYY-MM-DD."));
            }
            value.clone()
        }
        "source_url" | "url" if context.roles.note_kind == crate::native::props::roles::NoteKind::Reference => {
            let source = value.as_str().ok_or_else(|| bad(key, index, "Use one HTTP(S) URL."))?;
            let parsed = url::Url::parse(source)
                .map_err(|_| bad(key, index, "Use one HTTP(S) URL."))?;
            if !matches!(parsed.scheme(), "http" | "https") || parsed.host_str().is_none() {
                return Err(bad(key, index, "Use one HTTP(S) URL."));
            }
            value.clone()
        }
        "aliases" if context.roles.note_kind == crate::native::props::roles::NoteKind::Reference => {
            if !value.as_sequence().is_some_and(|items| {
                items.iter().all(|item| {
                    item.as_str().is_some_and(|text| !text.trim().is_empty())
                })
            }) {
                return Err(bad(key, index, "Use a list of nonblank alias strings."));
            }
            value.clone()
        }
        "parent" => {
            let link = value.as_str().ok_or_else(|| bad(key, index, "Use one quoted wikilink such as \"[[dev]]\"."))?;
            Value::String(parent::validate(context, candidate, link, index)?)
        }
        "color" => match facts.color {
            Some(NoteColor::Hue(hue)) if value.is_string() => Value::String(hue.name().to_owned()),
            Some(NoteColor::Grey) if value.is_string() => Value::String("grey".to_owned()),
            _ => return Err(bad(key, index, "Allowed: blue, orange, aqua, yellow, magenta, green, violet, red, grey (or gray). Clear to inherit.")),
        },
        "task_type" | "theme" => match facts.theme {
            Some(NoteTheme::Off) if value.is_string() => Value::String("off".to_owned()),
            Some(NoteTheme::Name(name)) if value.is_string() => {
                if let Some(contract) = &context.contract {
                    if let Some(message) = &contract.types.warning {
                        warnings.push(error("type_config_unavailable", key, index, message.clone(), "Repair the type configuration to check configured names."));
                    } else if !contract.types.themes.as_ref().is_some_and(|types| types.contains(&normalize_component(&name))) {
                        warnings.push(error("note_theme_unconfigured", key, index, format!("{name} is valid but is not a configured type"), "Choose a configured type or keep this explicit name."));
                    }
                }
                Value::String(name)
            }
            _ => return Err(bad(key, index, "Use one type name or off (Stop parent defaults); clear to inherit.")),
        },
        "offclock" => match facts.offclock {
            Some(NoteOffClock::True) if value.is_bool() => Value::Bool(true),
            Some(NoteOffClock::False) if value.is_bool() => Value::Bool(false),
            _ => return Err(bad(key, index, "Use true or false; clear both clock keys to inherit.")),
        },
        "offclock_types" => match facts.offclock_types {
            Some(NoteOffClockTypes::Keys(names)) if !names.is_empty() && string_items(value) => {
                Value::Sequence(names.into_iter().map(Value::String).collect())
            }
            _ => return Err(bad(key, index, "Use a nonempty type name or list of type names (no clock tags or compound types); clear to inherit.")),
        },
        "ready_cap" => {
            let raw = scalar(value);
            let (cap, _, off, invalid) = parse_ready_cap(raw.as_deref(), 5, CapSource::Config);
            if invalid || raw.is_none() || value.is_bool() || value.is_null() || raw.as_ref().is_some_and(|s| s.trim().is_empty() || s.trim().eq_ignore_ascii_case("false")) {
                return Err(bad(key, index, "Use an integer 1–999 or off; clear to use Bob's setting."));
            }
            if off { Value::String("off".to_owned()) } else { Value::Number(cap.expect("valid cap").into()) }
        }
        "task_refresh" => {
            let raw = scalar(value);
            let (days, _) = parse_note_refresh(raw.as_deref());
            Value::Number(days.ok_or_else(|| bad(key, index, "Use an integer 1–365 days; clear to use Bob's freshness setting."))?.into())
        }
        "root" => {
            if value != &Value::Bool(true) { return Err(bad(key, index, "Use true, or clear the key; root: false is not a setting.")); }
            parent::validate_root(context, candidate, index)?;
            Value::Bool(true)
        }
        "project_name_aliases" => {
            let items = value.as_sequence().ok_or_else(|| bad(key, index, "Use a list of nonblank, distinct capture names."))?;
            let mut seen = BTreeSet::new();
            for item in items {
                let name = item.as_str().filter(|name| !name.trim().is_empty()).ok_or_else(|| bad(key, index, "Capture names must be nonblank strings."))?;
                if !seen.insert(name.trim().to_ascii_lowercase()) {
                    return Err(bad(key, index, "Remove duplicate capture names (case-insensitive)."));
                }
            }
            let frontmatter = parse_frontmatter(&text).expect("single key");
            let (names, invalid) = project_name_aliases(&frontmatter);
            if !items.is_empty() && (!invalid.is_empty() || names.len() != items.len()) {
                return Err(bad(key, index, "Use nonblank, distinct capture names understood by capture-targets."));
            }
            if let Some(contract) = &context.contract {
                let mut targets = contract.targets.clone();
                if let Some(target) = targets.iter_mut().find(|target| target.relative_path == context.normalized_path) {
                    // Retain other notes and test just this proposed alias set.
                    target.project_name_aliases = names.clone();
                    let mut without = targets.clone();
                    without.iter_mut().filter(|target| target.relative_path == context.normalized_path).for_each(|target| target.project_name_aliases.clear());
                    let baseline: BTreeSet<_> = alias_conflict_warnings(&without).iter().map(|warning| warning.display()).collect();
                    for warning in alias_conflict_warnings(&targets) {
                        let message = warning.display();
                        if !baseline.contains(&message) {
                            warnings.push(error("project_name_aliases_conflict", key, index, message, "Capture uses the exact stem first; choose a unique alias to avoid ambiguity."));
                        }
                    }
                }
            }
            Value::Sequence(names.into_iter().map(Value::String).collect())
        }
        _ => value.clone(),
    };
    Ok((canonical, warnings))
}

fn valid_published(value: &str) -> bool {
    let bytes = value.as_bytes();
    match bytes.len() {
        4 if bytes.iter().all(u8::is_ascii_digit) => {
            value.parse::<i32>().ok().is_some_and(|year| {
                year > 0
                    && chrono::NaiveDate::from_ymd_opt(year, 1, 1).is_some()
            })
        }
        7 if bytes[4] == b'-'
            && bytes[..4].iter().all(u8::is_ascii_digit)
            && bytes[5..].iter().all(u8::is_ascii_digit) =>
        {
            let year = value[..4].parse::<i32>().ok();
            let month = value[5..].parse::<u32>().ok();
            year.zip(month).is_some_and(|(year, month)| {
                chrono::NaiveDate::from_ymd_opt(year, month, 1).is_some()
            })
        }
        10 if bytes[4] == b'-' && bytes[7] == b'-' => {
            chrono::NaiveDate::parse_from_str(value, "%Y-%m-%d").is_ok()
        }
        _ => false,
    }
}

fn scalar(value: &Value) -> Option<String> {
    match value {
        Value::String(text) => Some(text.clone()),
        Value::Number(number) => Some(number.to_string()),
        _ => None,
    }
}

fn string_items(value: &Value) -> bool {
    value.as_str().is_some_and(|s| !s.trim().is_empty())
        || value.as_sequence().is_some_and(|items| {
            !items.is_empty()
                && items
                    .iter()
                    .all(|v| v.as_str().is_some_and(|s| !s.trim().is_empty()))
        })
}

/// Reject invalid semantic input before a raw YAML fragment reaches the splicer.
/// Graph checks and diagnostics use the postimage in `operation` below.
pub(crate) fn input(
    context: &InspectContext,
    candidate: &str,
    key: &str,
    proposed: &Value,
    index: usize,
) -> Result<(), PropsError> {
    if settings::for_key(context.roles.note_kind, key).is_none() {
        return Ok(());
    }
    match key {
        "parent" => {
            if proposed
                .as_str()
                .and_then(crate::native::projects::wikilink_path_target)
                .is_none()
            {
                return Err(bad(
                    key,
                    index,
                    "Use one quoted wikilink such as \"[[dev]]\".",
                ));
            }
        }
        "root" => {
            if proposed != &Value::Bool(true) {
                return Err(bad(
                    key,
                    index,
                    "Use true, or clear the key; root: false is not a setting.",
                ));
            }
        }
        _ => {
            value(context, candidate, key, proposed, index)?;
        }
    }
    Ok(())
}

pub(crate) fn removal(
    context: &InspectContext,
    key: &str,
    index: usize,
) -> Result<(), PropsError> {
    if key == "parent"
        && settings::for_key(context.roles.note_kind, key).is_some()
    {
        Err(error(
            "prop_required",
            key,
            index,
            "Lives under is required",
            "Choose another parent instead of clearing it.",
        ))
    } else {
        Ok(())
    }
}

/// Validate the addressed postimage, never unrelated preexisting bad values.
/// Restore deliberately bypasses domain validation so legacy bytes remain undoable.
pub(crate) fn operation(
    context: &InspectContext,
    candidate: &PropsDocument,
    op: &Op,
    index: usize,
) -> Result<Vec<PropsError>, PropsError> {
    let key = match op {
        Op::Set { key, .. } | Op::SetYaml { key, .. } => key,
        Op::Rename { key, to } => {
            if key != to {
                removal(context, key, index)?;
            }
            to
        }
        Op::Unset { key } => {
            removal(context, key, index)?;
            return Ok(Vec::new());
        }
        _ => return Ok(Vec::new()),
    };
    if settings::for_key(context.roles.note_kind, key).is_none() {
        return Ok(Vec::new());
    }
    let entry = candidate
        .entries
        .iter()
        .find(|entry| entry.key == *key)
        .and_then(|entry| entry.raw_value.as_ref())
        .ok_or_else(|| bad(key, index, "Set a valid setting value."))?;
    value(context, &candidate.source, key, entry, index)
        .map(|(_, warnings)| warnings)
}
