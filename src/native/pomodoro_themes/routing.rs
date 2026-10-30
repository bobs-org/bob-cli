//! Pure task-type routing planners: agreement, retype, placement,
//! and relocation (task_types D4–D8, `routing-core`).
//!
//! These are the shared contract behind capture, move, link, ref-today,
//! successor, and Task Card gestures. Every function is pure over plain
//! data so the Ledger Tools JS mirror can consume the same vectors.
//! `plan_theme_change` stays in place until `capture-retype` replaces its
//! caller.
//!
//! No live caller consumes the planners yet; later phases wire them into
//! capture, moves, and links. The shared vectors are the contract until
//! then.
#![allow(dead_code)]

use std::collections::BTreeSet;

use crate::native::{
    config::themes::ThemeSources,
    plan_budget::{normalize_component, split_components},
    task_fields::canonical_theme_name,
    task_theme::TaskThemeState,
};

/// One classified component of a Pomodoro name (D4).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TypeComponent {
    pub(crate) name: String,
    pub(crate) key: String,
    pub(crate) is_type: bool,
}

/// Classify each component of a Pomodoro name (D4).
///
/// A configured vocabulary present means exactly the `types:` block parsed
/// to at least one entry: a component is a type component iff its key is
/// in the vocabulary. Otherwise every component except inventory labels
/// is a type component. Unnamed entries and `(unnamed)` have no
/// components. An entry with no type components is a container.
pub(crate) fn type_components(
    name: Option<&str>,
    sources: &ThemeSources,
) -> Vec<TypeComponent> {
    let Some(raw) = name else {
        return Vec::new();
    };
    let trimmed = raw.trim();
    if trimmed.is_empty() || trimmed == "(unnamed)" {
        return Vec::new();
    }
    let inventory: BTreeSet<String> = sources
        .inventory
        .iter()
        .map(|value| normalize_component(value))
        .collect();
    split_components(trimmed)
        .into_iter()
        .map(|component| {
            let key = normalize_component(&component);
            let is_type = match &sources.themes {
                Some(vocabulary) => vocabulary.contains(&key),
                None => !inventory.contains(&key),
            };
            TypeComponent {
                name: component,
                key,
                is_type,
            }
        })
        .collect()
}

/// Agreement invariant (D5): a live Task Link sits under a Pomodoro whose
/// name includes its task's type, or under a container.
pub(crate) fn agrees(
    task_key: Option<&str>,
    entry_name: Option<&str>,
    sources: &ThemeSources,
) -> bool {
    let Some(key) = task_key else {
        return true;
    };
    if type_components(entry_name, sources)
        .iter()
        .all(|item| !item.is_type)
    {
        return true;
    }
    let Some(name) = entry_name else {
        return true;
    };
    split_components(name)
        .iter()
        .any(|component| normalize_component(component) == key)
}

/// How a link landing in Pomodoro D retypes its task (D6).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum RetypeOutcome {
    Container,
    Consistent,
    Merge { name: String },
    Set { name: String, key: String },
    Clear,
}

pub(crate) struct RetypeInput<'a> {
    pub(crate) field: &'a TaskThemeState,
    pub(crate) effective_key: Option<&'a str>,
    pub(crate) effective_name: Option<&'a str>,
    pub(crate) inherited_key: Option<&'a str>,
    pub(crate) destination: &'a str,
    pub(crate) selector: Option<&'a str>,
    pub(crate) keep_type: bool,
    pub(crate) tab_pick: Option<&'a str>,
}

/// Pure retype rule (D6). `effective_*` is the task's current effective
/// type (field, reading, note, or default); `inherited_*` is the same
/// cascade without the task field. `selector` is the authored `#NAME` /
/// `=name` text; `keep_type` is the Obsidian Alt+Enter merge path;
/// `tab_pick` is the Obsidian Tab-cycled composite choice, if any.
pub(crate) fn retype(
    input: &RetypeInput<'_>,
    sources: &ThemeSources,
) -> RetypeOutcome {
    let _ = input.field;
    let components = type_components(Some(input.destination), sources);
    if components.iter().all(|item| !item.is_type) {
        return RetypeOutcome::Container;
    }
    if let Some(effective) = input.effective_key {
        let entry_keys: BTreeSet<String> = split_components(input.destination)
            .into_iter()
            .map(|component| normalize_component(&component))
            .collect();
        if entry_keys.contains(effective) {
            return RetypeOutcome::Consistent;
        }
    }
    if input.keep_type {
        let current = input
            .effective_name
            .unwrap_or_else(|| input.effective_key.unwrap_or_default());
        let current = canonical_theme_name(current)
            .unwrap_or_else(|| current.to_string());
        return RetypeOutcome::Merge {
            name: format!("{} + {current}", input.destination.trim()),
        };
    }
    let candidates: Vec<&TypeComponent> =
        components.iter().filter(|item| item.is_type).collect();
    if candidates.is_empty() {
        return RetypeOutcome::Container;
    }
    let mut chosen: Option<&TypeComponent> = None;
    if let Some(selector) = input.selector {
        let want = normalize_component(&strip_selector(selector));
        if !want.is_empty() {
            chosen = candidates
                .iter()
                .find(|item| item.key == want)
                .copied()
                .or_else(|| {
                    candidates
                        .iter()
                        .find(|item| item.key.starts_with(&want))
                        .copied()
                });
        }
    }
    if chosen.is_none()
        && let Some(pick) = input.tab_pick
    {
        let want = normalize_component(pick);
        chosen = candidates.iter().find(|item| item.key == want).copied();
    }
    if chosen.is_none() {
        let exempt: BTreeSet<String> = sources
            .exempt
            .iter()
            .map(|value| normalize_component(value))
            .collect();
        chosen = candidates
            .iter()
            .find(|item| !exempt.contains(&item.key))
            .copied()
            .or_else(|| candidates.first().copied());
    }
    let chosen = chosen.expect("nonempty type components have a choice");
    if let Some(inherited) = input.inherited_key
        && inherited == chosen.key.as_str()
    {
        return RetypeOutcome::Clear;
    }
    RetypeOutcome::Set {
        name: chosen.name.clone(),
        key: chosen.key.clone(),
    }
}

fn strip_selector(raw: &str) -> String {
    let mut text = raw.trim().to_string();
    if let Some(stripped) = text.strip_prefix(['#', '=']) {
        text = stripped.to_string();
    }
    let text = text.trim().to_string();
    if text.len() >= 2 && text.starts_with('"') && text.ends_with('"') {
        return text[1..text.len() - 1].to_string();
    }
    text
}

/// One ledger entry as seen by placement and relocation.
#[derive(Debug, Clone)]
pub(crate) struct PlaceEntry<'a> {
    pub(crate) name: Option<&'a str>,
    pub(crate) open: bool,
    pub(crate) running: bool,
}

/// Where Bob puts the Pomodoro for type T (D7).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PlaceOutcome {
    Entry(usize),
    Create { name: String },
    Refused { code: &'static str },
    AlreadyQueued,
    NoType,
}

pub(crate) struct PlaceInput<'a> {
    pub(crate) type_key: Option<&'a str>,
    pub(crate) type_name: Option<&'a str>,
    pub(crate) entries: &'a [PlaceEntry<'a>],
    pub(crate) exempt: &'a [String],
    pub(crate) ledger_keys: &'a [String],
    pub(crate) themes_before: usize,
    pub(crate) cap: u32,
    pub(crate) strict: bool,
    pub(crate) already_queued: bool,
}

/// Pure placement rule (D7). Unlike menus, placement may create entries
/// for unknown or unconfigured names (the `theme_not_configured`
/// warning is the caller's job). With no effective type the caller
/// keeps today's implicit current/next selection.
pub(crate) fn place(input: &PlaceInput<'_>) -> PlaceOutcome {
    let Some(key) = input.type_key else {
        return PlaceOutcome::NoType;
    };
    if input.already_queued {
        return PlaceOutcome::AlreadyQueued;
    }
    if let Some(index) =
        input.entries.iter().enumerate().find_map(|(index, entry)| {
            (entry.open
                && entry.running
                && is_exact(entry.name, key, input.exempt))
            .then_some(index)
        })
    {
        return PlaceOutcome::Entry(index);
    }
    if let Some(index) =
        input.entries.iter().enumerate().find_map(|(index, entry)| {
            (entry.open
                && !entry.running
                && is_exact(entry.name, key, input.exempt))
            .then_some(index)
        })
    {
        // Ledger order decides among duplicates: the first open exact
        // entry wins. Running entries were checked above.
        return PlaceOutcome::Entry(index);
    }
    if let Some(index) =
        input.entries.iter().enumerate().find_map(|(index, entry)| {
            (entry.open && contains_key(entry.name, key)).then_some(index)
        })
    {
        return PlaceOutcome::Entry(index);
    }
    let exempt_suggestion = input
        .exempt
        .iter()
        .any(|value| normalize_component(value) == key);
    let already_in_ledger = input
        .ledger_keys
        .iter()
        .any(|name| normalize_component(name) == key);
    let adds_theme = !exempt_suggestion && !already_in_ledger;
    if input.strict
        && adds_theme
        && input.themes_before.saturating_add(1) > input.cap as usize
    {
        return PlaceOutcome::Refused { code: "strict_cap" };
    }
    PlaceOutcome::Create {
        name: input.type_name.unwrap_or(key).to_string(),
    }
}

fn is_exact(name: Option<&str>, key: &str, exempt: &[String]) -> bool {
    let Some(name) = name else {
        return false;
    };
    let exempt: BTreeSet<String> = exempt
        .iter()
        .map(|value| normalize_component(value))
        .collect();
    let kept: Vec<String> = split_components(name)
        .into_iter()
        .map(|component| normalize_component(&component))
        .filter(|component| component == key || !exempt.contains(component))
        .collect();
    kept.len() == 1 && kept.first().is_some_and(|only| only == key)
}

fn contains_key(name: Option<&str>, key: &str) -> bool {
    let Some(name) = name else {
        return false;
    };
    split_components(name)
        .iter()
        .any(|component| normalize_component(component) == key)
}

/// One ledger entry as seen by relocation, with its live-link count.
#[derive(Debug, Clone)]
pub(crate) struct RelocateEntry {
    pub(crate) name: Option<String>,
    pub(crate) open: bool,
    pub(crate) running: bool,
    pub(crate) links: usize,
}

/// What happens to the live ledger when a task's type becomes X (D8,
/// `running_retype = absorb`: running entries keep the task).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum RelocateOutcome {
    Unchanged,
    RenameEntry { index: usize, new_name: String },
    AppendComponent { index: usize, new_name: String },
    MoveTo { from: usize, to: usize },
    RenamePlaceholder { index: usize, new_name: String },
    CreateBelow { from: usize, name: String },
    Refused { code: &'static str },
}

pub(crate) struct RelocateInput<'a> {
    pub(crate) entries: &'a [RelocateEntry],
    pub(crate) live_link: Option<usize>,
    pub(crate) type_key: &'a str,
    pub(crate) type_name: &'a str,
    pub(crate) sources: &'a ThemeSources,
    pub(crate) ledger_keys: &'a [String],
    pub(crate) themes_before: usize,
    pub(crate) cap: u32,
    pub(crate) strict: bool,
}

/// Pure relocation rule (D8). A backlog task never gains a link, so no
/// live link means ledger unchanged. An emptied placeholder is deleted
/// by the existing cleanup rule; a timed entry never is (the caller's
/// job).
pub(crate) fn relocate(input: &RelocateInput<'_>) -> RelocateOutcome {
    let Some(from) = input.live_link else {
        return RelocateOutcome::Unchanged;
    };
    let Some(entry) = input.entries.get(from) else {
        return RelocateOutcome::Unchanged;
    };
    if !entry.open {
        return RelocateOutcome::Unchanged;
    }
    if agrees(Some(input.type_key), entry.name.as_deref(), input.sources) {
        return RelocateOutcome::Unchanged;
    }
    if entry.running {
        if entry.links <= 1 {
            return RelocateOutcome::RenameEntry {
                index: from,
                new_name: replace_type_components(
                    entry.name.as_deref(),
                    input.type_name,
                    input.sources,
                ),
            };
        }
        let base = entry.name.clone().unwrap_or_default();
        return RelocateOutcome::AppendComponent {
            index: from,
            new_name: format!("{base} + {}", input.type_name),
        };
    }
    // Steps 1-3 of place(X), skipping running entries.
    let borrowed: Vec<PlaceEntry<'_>> = input
        .entries
        .iter()
        .map(|item| PlaceEntry {
            name: item.name.as_deref(),
            open: item.open,
            running: item.running,
        })
        .collect();
    let exempt: Vec<String> = input.sources.exempt.iter().cloned().collect();
    let planned: Vec<PlaceEntry<'_>> = borrowed
        .iter()
        .map(|item| PlaceEntry {
            name: item.name,
            open: item.open && !item.running,
            running: false,
        })
        .collect();
    let placed = place(&PlaceInput {
        type_key: Some(input.type_key),
        type_name: Some(input.type_name),
        entries: &planned,
        exempt: &exempt,
        ledger_keys: input.ledger_keys,
        themes_before: input.themes_before,
        cap: input.cap,
        strict: false,
        already_queued: false,
    });
    if let PlaceOutcome::Entry(to) = placed
        && to != from
    {
        return RelocateOutcome::MoveTo { from, to };
    }
    if entry.links <= 1 {
        return RelocateOutcome::RenamePlaceholder {
            index: from,
            new_name: replace_type_components(
                entry.name.as_deref(),
                input.type_name,
                input.sources,
            ),
        };
    }
    let exempt_set: BTreeSet<String> = input
        .sources
        .exempt
        .iter()
        .map(|value| normalize_component(value))
        .collect();
    let already_in_ledger = input
        .ledger_keys
        .iter()
        .any(|name| normalize_component(name) == input.type_key);
    let adds_theme = !exempt_set.contains(input.type_key) && !already_in_ledger;
    if input.strict
        && adds_theme
        && input.themes_before.saturating_add(1) > input.cap as usize
    {
        return RelocateOutcome::Refused { code: "strict_cap" };
    }
    RelocateOutcome::CreateBelow {
        from,
        name: input.type_name.to_string(),
    }
}

/// Replace an entry's type components with X, keeping labels in order
/// with X in the first type component's position.
pub(crate) fn replace_type_components(
    name: Option<&str>,
    type_name: &str,
    sources: &ThemeSources,
) -> String {
    let Some(name) = name else {
        return type_name.to_string();
    };
    let classified = type_components(Some(name), sources);
    if classified.is_empty() {
        return type_name.to_string();
    }
    let mut parts: Vec<String> = Vec::new();
    let mut placed = false;
    for item in &classified {
        if !item.is_type {
            parts.push(item.name.clone());
        } else if !placed {
            parts.push(type_name.to_string());
            placed = true;
        }
    }
    if !placed {
        parts.push(type_name.to_string());
    }
    parts.join(" + ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::native::config::themes::{ConfiguredTheme, ThemesConfig};
    use serde_json::Value;

    fn sources(
        vocab: &[String],
        exempt: &[String],
        inventory: &[String],
    ) -> ThemeSources {
        ThemeSources {
            exempt: exempt
                .iter()
                .map(|value| normalize_component(value))
                .collect(),
            inventory: inventory
                .iter()
                .map(|value| normalize_component(value))
                .collect(),
            themes: if vocab.is_empty() {
                None
            } else {
                Some(ThemesConfig {
                    entries: vocab
                        .iter()
                        .map(|name| ConfiguredTheme {
                            key: normalize_component(name),
                            name: name.clone(),
                            color: None,
                        })
                        .collect(),
                })
            },
            warning: None,
        }
    }

    fn strings(value: &Value) -> Vec<String> {
        value
            .as_array()
            .map(|items| {
                items
                    .iter()
                    .filter_map(|item| item.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default()
    }

    fn opt_str(value: &Value) -> Option<String> {
        value.as_str().map(str::to_string)
    }

    #[test]
    fn shared_routing_vectors() {
        let vectors: Value = serde_json::from_str(include_str!(
            "../../../tests/fixtures/pomodoro_themes/vectors.json"
        ))
        .expect("shared vectors JSON");
        for case in vectors["agree"].as_array().expect("agree vectors") {
            let id = case["name"].as_str().unwrap_or("?");
            let task = case["task"].as_str().map(str::to_string);
            let entry = case["entry"].as_str().map(str::to_string);
            let made = sources(
                &strings(&case["vocab"]),
                &strings(&case["exempt"]),
                &strings(&case["inventory"]),
            );
            assert_eq!(
                agrees(task.as_deref(), entry.as_deref(), &made),
                case["expected"].as_bool().unwrap(),
                "{id}"
            );
        }
        for case in vectors["retype"].as_array().expect("retype vectors") {
            let id = case["name"].as_str().unwrap_or("?");
            let field = match case["field"].as_str().unwrap() {
                "none" => TaskThemeState::None,
                "set" => TaskThemeState::Set {
                    value: case["field_value"]
                        .as_str()
                        .unwrap_or("FIX")
                        .to_string(),
                    key: normalize_component(
                        case["field_value"].as_str().unwrap_or("FIX"),
                    ),
                },
                "repair" => TaskThemeState::Repair {
                    kind: crate::native::task_theme::RepairKind::Duplicate,
                },
                other => panic!("{id}: unexpected field {other}"),
            };
            let effective = opt_str(&case["effective"]);
            let effective_name = opt_str(&case["effective_name"]);
            let inherited = opt_str(&case["inherited"]);
            let destination = case["destination"].as_str().unwrap();
            let selector = opt_str(&case["selector"]);
            let made = sources(
                &strings(&case["vocab"]),
                &strings(&case["exempt"]),
                &strings(&case["inventory"]),
            );
            let outcome = retype(
                &RetypeInput {
                    field: &field,
                    effective_key: effective.as_deref(),
                    effective_name: effective_name.as_deref(),
                    inherited_key: inherited.as_deref(),
                    destination,
                    selector: selector.as_deref(),
                    keep_type: case["keep_type"].as_bool().unwrap_or(false),
                    tab_pick: case["tab_pick"].as_str(),
                },
                &made,
            );
            let expected = &case["expected"];
            match expected["kind"].as_str().unwrap() {
                "container" => {
                    assert_eq!(outcome, RetypeOutcome::Container, "{id}");
                }
                "consistent" => {
                    assert_eq!(outcome, RetypeOutcome::Consistent, "{id}");
                }
                "merge" => {
                    let RetypeOutcome::Merge { name } = outcome else {
                        panic!("{id}: expected merge, got {outcome:?}");
                    };
                    assert_eq!(
                        name,
                        expected["name"].as_str().unwrap(),
                        "{id}"
                    );
                }
                "set" => {
                    let RetypeOutcome::Set { name, key } = outcome else {
                        panic!("{id}: expected set, got {outcome:?}");
                    };
                    assert_eq!(
                        name,
                        expected["name"].as_str().unwrap(),
                        "{id}"
                    );
                    assert_eq!(key, expected["key"].as_str().unwrap(), "{id}");
                }
                "clear" => {
                    assert_eq!(outcome, RetypeOutcome::Clear, "{id}");
                }
                other => panic!("{id}: unexpected retype kind {other}"),
            }
        }
        for case in vectors["place"].as_array().expect("place vectors") {
            let id = case["name"].as_str().unwrap_or("?");
            let entries = case["entries"]
                .as_array()
                .unwrap()
                .iter()
                .map(|entry| PlaceEntry {
                    name: entry["name"].as_str(),
                    open: entry["open"].as_bool().unwrap(),
                    running: entry["running"].as_bool().unwrap(),
                })
                .collect::<Vec<_>>();
            let exempt = strings(&case["exempt"]);
            let ledger_keys = strings(&case["ledger_keys"]);
            let outcome = place(&PlaceInput {
                type_key: case["type"].as_str(),
                type_name: case["type_name"].as_str(),
                entries: &entries,
                exempt: &exempt,
                ledger_keys: &ledger_keys,
                themes_before: case["themes_before"].as_u64().unwrap() as usize,
                cap: case["cap"].as_u64().unwrap() as u32,
                strict: case["strict"].as_bool().unwrap(),
                already_queued: case["already_queued"].as_bool().unwrap(),
            });
            let expected = &case["expected"];
            match expected["kind"].as_str().unwrap() {
                "entry" => {
                    assert_eq!(
                        outcome,
                        PlaceOutcome::Entry(
                            expected["index"].as_u64().unwrap() as usize
                        ),
                        "{id}"
                    );
                }
                "create" => {
                    let PlaceOutcome::Create { name } = outcome else {
                        panic!("{id}: expected create, got {outcome:?}");
                    };
                    assert_eq!(
                        name,
                        expected["name"].as_str().unwrap(),
                        "{id}"
                    );
                }
                "refused" => {
                    assert_eq!(
                        outcome,
                        PlaceOutcome::Refused { code: "strict_cap" },
                        "{id}"
                    );
                }
                "already_queued" => {
                    assert_eq!(outcome, PlaceOutcome::AlreadyQueued, "{id}");
                }
                "no_type" => {
                    assert_eq!(outcome, PlaceOutcome::NoType, "{id}");
                }
                other => panic!("{id}: unexpected place kind {other}"),
            }
        }
        for case in vectors["relocate"].as_array().expect("relocate vectors") {
            let id = case["name"].as_str().unwrap_or("?");
            let entries = case["entries"]
                .as_array()
                .unwrap()
                .iter()
                .map(|entry| RelocateEntry {
                    name: entry["name"].as_str().map(str::to_string),
                    open: entry["open"].as_bool().unwrap(),
                    running: entry["running"].as_bool().unwrap(),
                    links: entry["links"].as_u64().unwrap() as usize,
                })
                .collect::<Vec<_>>();
            let made = sources(
                &strings(&case["vocab"]),
                &strings(&case["exempt"]),
                &strings(&case["inventory"]),
            );
            let ledger_keys = strings(&case["ledger_keys"]);
            let outcome = relocate(&RelocateInput {
                entries: &entries,
                live_link: case["live_link"]
                    .as_u64()
                    .map(|index| index as usize),
                type_key: case["type"].as_str().unwrap(),
                type_name: case["type_name"].as_str().unwrap(),
                sources: &made,
                ledger_keys: &ledger_keys,
                themes_before: case["themes_before"].as_u64().unwrap() as usize,
                cap: case["cap"].as_u64().unwrap() as u32,
                strict: case["strict"].as_bool().unwrap(),
            });
            let expected = &case["expected"];
            match expected["kind"].as_str().unwrap() {
                "unchanged" => {
                    assert_eq!(outcome, RelocateOutcome::Unchanged, "{id}");
                }
                "rename_entry" => {
                    let RelocateOutcome::RenameEntry { index, new_name } =
                        outcome
                    else {
                        panic!("{id}: expected rename_entry, got {outcome:?}");
                    };
                    assert_eq!(
                        index,
                        expected["index"].as_u64().unwrap() as usize,
                        "{id}"
                    );
                    assert_eq!(
                        new_name,
                        expected["new_name"].as_str().unwrap(),
                        "{id}"
                    );
                }
                "append_component" => {
                    let RelocateOutcome::AppendComponent { index, new_name } =
                        outcome
                    else {
                        panic!(
                            "{id}: expected append_component, got {outcome:?}"
                        );
                    };
                    assert_eq!(
                        index,
                        expected["index"].as_u64().unwrap() as usize,
                        "{id}"
                    );
                    assert_eq!(
                        new_name,
                        expected["new_name"].as_str().unwrap(),
                        "{id}"
                    );
                }
                "move_to" => {
                    assert_eq!(
                        outcome,
                        RelocateOutcome::MoveTo {
                            from: expected["from"].as_u64().unwrap() as usize,
                            to: expected["to"].as_u64().unwrap() as usize,
                        },
                        "{id}"
                    );
                }
                "rename_placeholder" => {
                    let RelocateOutcome::RenamePlaceholder { index, new_name } =
                        outcome
                    else {
                        panic!(
                            "{id}: expected rename_placeholder, got {outcome:?}"
                        );
                    };
                    assert_eq!(
                        index,
                        expected["index"].as_u64().unwrap() as usize,
                        "{id}"
                    );
                    assert_eq!(
                        new_name,
                        expected["new_name"].as_str().unwrap(),
                        "{id}"
                    );
                }
                "create_below" => {
                    let RelocateOutcome::CreateBelow { from, name } = outcome
                    else {
                        panic!("{id}: expected create_below, got {outcome:?}");
                    };
                    assert_eq!(
                        from,
                        expected["from"].as_u64().unwrap() as usize,
                        "{id}"
                    );
                    assert_eq!(
                        name,
                        expected["name"].as_str().unwrap(),
                        "{id}"
                    );
                }
                "refused" => {
                    assert_eq!(
                        outcome,
                        RelocateOutcome::Refused { code: "strict_cap" },
                        "{id}"
                    );
                }
                other => panic!("{id}: unexpected relocate kind {other}"),
            }
        }
    }
}
