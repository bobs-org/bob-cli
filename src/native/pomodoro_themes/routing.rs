//! Pure task-type routing planners: agreement, landing, placement,
//! and relocation (task_types D4–D8 plus merged-Pomodoro R1–R7).
//!
//! These are the shared contract behind capture, move, link, ref-today,
//! successor, and Task Card gestures. Every function is pure over plain
//! data so the Ledger Tools JS mirror can consume the same vectors.
//! Capture links land by merging (R2); only Pomodoro rename (D6) passes
//! `retype: true`. Merge, split, narrow, shed, and type-select live in
//! [`super::merged`].

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

/// How a link landing in Pomodoro D agrees with its task (R2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum LandOutcome {
    Container,
    Consistent,
    Merge { name: String },
    Set { name: String, key: String },
    Clear,
}

pub(crate) struct LandInput<'a> {
    pub(crate) field: &'a TaskThemeState,
    pub(crate) effective_key: Option<&'a str>,
    pub(crate) effective_name: Option<&'a str>,
    pub(crate) inherited_key: Option<&'a str>,
    pub(crate) destination: &'a str,
    pub(crate) selector: Option<&'a str>,
    /// When true, take the explicit Set/Clear retype path (Obsidian
    /// Alt+Enter and Pomodoro rename). False is the default merge.
    pub(crate) retype: bool,
    pub(crate) tab_pick: Option<&'a str>,
}

/// Pure landing rule (R2). `effective_*` is the task's current effective
/// type (field, reading, note, or default); `inherited_*` is the same
/// cascade without the task field. `selector` is the authored `#NAME` /
/// `=name` text; `retype` is the explicit Set/Clear path; `tab_pick` is
/// the Obsidian Tab-cycled composite choice, if any.
pub(crate) fn land(
    input: &LandInput<'_>,
    sources: &ThemeSources,
) -> LandOutcome {
    let _ = input.field;
    let components = type_components(Some(input.destination), sources);
    if components.iter().all(|item| !item.is_type) {
        return LandOutcome::Container;
    }
    let Some(effective) = input.effective_key else {
        return LandOutcome::Consistent;
    };
    let entry_keys: BTreeSet<String> = split_components(input.destination)
        .into_iter()
        .map(|component| normalize_component(&component))
        .collect();
    if entry_keys.contains(effective) {
        return LandOutcome::Consistent;
    }
    if !input.retype && !is_meet_name(Some(input.destination)) {
        let current = input.effective_name.unwrap_or(effective);
        let current = canonical_theme_name(current)
            .unwrap_or_else(|| current.to_string());
        return LandOutcome::Merge {
            name: format!("{} + {current}", input.destination.trim()),
        };
    }
    retype_destination(input, sources, &components)
}

fn retype_destination(
    input: &LandInput<'_>,
    sources: &ThemeSources,
    components: &[TypeComponent],
) -> LandOutcome {
    let candidates: Vec<&TypeComponent> =
        components.iter().filter(|item| item.is_type).collect();
    if candidates.is_empty() {
        return LandOutcome::Container;
    }
    let mut chosen: Option<&TypeComponent> = None;
    if let Some(selector) = input.selector {
        let parsed = parse_type_selector(selector);
        if !parsed.key.is_empty() {
            chosen = candidates
                .iter()
                .find(|item| item.key == parsed.key)
                .copied();
            if chosen.is_none() && !parsed.literal {
                chosen = candidates
                    .iter()
                    .find(|item| item.key.starts_with(&parsed.key))
                    .copied();
            }
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
        return LandOutcome::Clear;
    }
    LandOutcome::Set {
        name: chosen.name.clone(),
        key: chosen.key.clone(),
    }
}

/// A type selector: bare `build`, `#build`, or `#"SASE V18"`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ParsedSelector {
    pub(crate) raw: String,
    pub(crate) key: String,
    pub(crate) literal: bool,
}

/// Strip `#` / `=` / wrapping quotes from a type selector.
pub(crate) fn parse_type_selector(raw: &str) -> ParsedSelector {
    let mut text = raw.trim().to_string();
    if let Some(stripped) = text.strip_prefix(['#', '=']) {
        text = stripped.to_string();
    }
    let text = text.trim().to_string();
    let literal =
        text.len() >= 2 && text.starts_with('"') && text.ends_with('"');
    let raw = if literal {
        text[1..text.len() - 1].to_string()
    } else {
        text
    };
    ParsedSelector {
        key: normalize_component(&raw),
        raw,
        literal,
    }
}

/// True when the whole name normalizes to `meet`. Calendar sync owns
/// that exact name; merged-Pomodoro rules never rename, merge, or split
/// it.
pub(crate) fn is_meet_name(name: Option<&str>) -> bool {
    name.is_some_and(|name| normalize_component(name) == "meet")
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
    AlreadyQueued,
    NoType,
}

pub(crate) struct PlaceInput<'a> {
    pub(crate) type_key: Option<&'a str>,
    pub(crate) type_name: Option<&'a str>,
    pub(crate) entries: &'a [PlaceEntry<'a>],
    pub(crate) exempt: &'a [String],
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
    PlaceOutcome::Create {
        name: input.type_name.unwrap_or(key).to_string(),
    }
}

/// Whether D7 `place` step 1 would take this entry: it is open, running,
/// and an exact (modulo exempt keys other than the type) match for the
/// type. Lets bare-capture callers keep the multiple-open-timed guard for
/// every other case.
pub(crate) fn is_exact(
    name: Option<&str>,
    key: &str,
    exempt: &[String],
) -> bool {
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

pub(crate) fn contains_key(name: Option<&str>, key: &str) -> bool {
    let Some(name) = name else {
        return false;
    };
    split_components(name)
        .iter()
        .any(|component| normalize_component(component) == key)
}

/// `is_exact` with key-prefix matching: after dropping exempt keys other
/// than the prefix match, exactly one component remains and its key
/// starts with `prefix`.
pub(crate) fn is_prefix_exact(
    name: Option<&str>,
    prefix: &str,
    exempt: &[String],
) -> bool {
    if prefix.is_empty() {
        return false;
    }
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
        .filter(|component| {
            component.starts_with(prefix) || !exempt.contains(component)
        })
        .collect();
    kept.len() == 1 && kept.first().is_some_and(|only| only.starts_with(prefix))
}

pub(crate) fn contains_key_prefix(name: Option<&str>, prefix: &str) -> bool {
    if prefix.is_empty() {
        return false;
    }
    let Some(name) = name else {
        return false;
    };
    split_components(name)
        .iter()
        .any(|component| normalize_component(component).starts_with(prefix))
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
}

pub(crate) struct RelocateInput<'a> {
    pub(crate) entries: &'a [RelocateEntry],
    pub(crate) live_link: Option<usize>,
    pub(crate) type_key: &'a str,
    pub(crate) type_name: &'a str,
    pub(crate) sources: &'a ThemeSources,
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
    if is_meet_name(entry.name.as_deref()) {
        if entry.running {
            return RelocateOutcome::Unchanged;
        }
        let borrowed: Vec<PlaceEntry<'_>> = input
            .entries
            .iter()
            .map(|item| PlaceEntry {
                name: item.name.as_deref(),
                open: item.open,
                running: item.running,
            })
            .collect();
        let exempt: Vec<String> =
            input.sources.exempt.iter().cloned().collect();
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
            already_queued: false,
        });
        if let PlaceOutcome::Entry(to) = placed
            && to != from
        {
            return RelocateOutcome::MoveTo { from, to };
        }
        if entry.links > 1 {
            return RelocateOutcome::CreateBelow {
                from,
                name: input.type_name.to_string(),
            };
        }
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
        for case in vectors["land"].as_array().expect("land vectors") {
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
            let outcome = land(
                &LandInput {
                    field: &field,
                    effective_key: effective.as_deref(),
                    effective_name: effective_name.as_deref(),
                    inherited_key: inherited.as_deref(),
                    destination,
                    selector: selector.as_deref(),
                    retype: case["retype"].as_bool().unwrap_or(false),
                    tab_pick: case["tab_pick"].as_str(),
                },
                &made,
            );
            let expected = &case["expected"];
            match expected["kind"].as_str().unwrap() {
                "container" => {
                    assert_eq!(outcome, LandOutcome::Container, "{id}");
                }
                "consistent" => {
                    assert_eq!(outcome, LandOutcome::Consistent, "{id}");
                }
                "merge" => {
                    let LandOutcome::Merge { name } = outcome else {
                        panic!("{id}: expected merge, got {outcome:?}");
                    };
                    assert_eq!(
                        name,
                        expected["name"].as_str().unwrap(),
                        "{id}"
                    );
                }
                "set" => {
                    let LandOutcome::Set { name, key } = outcome else {
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
                    assert_eq!(outcome, LandOutcome::Clear, "{id}");
                }
                other => panic!("{id}: unexpected land kind {other}"),
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
            let outcome = place(&PlaceInput {
                type_key: case["type"].as_str(),
                type_name: case["type_name"].as_str(),
                entries: &entries,
                exempt: &exempt,
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
            let outcome = relocate(&RelocateInput {
                entries: &entries,
                live_link: case["live_link"]
                    .as_u64()
                    .map(|index| index as usize),
                type_key: case["type"].as_str().unwrap(),
                type_name: case["type_name"].as_str().unwrap(),
                sources: &made,
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
                other => panic!("{id}: unexpected relocate kind {other}"),
            }
        }
        for case in vectors["merge"].as_array().expect("merge vectors") {
            let id = case["name"].as_str().unwrap_or("?");
            let base = case["base"].as_str().unwrap();
            let others: Vec<&str> = case["others"]
                .as_array()
                .unwrap()
                .iter()
                .map(|item| item.as_str().unwrap())
                .collect();
            assert_eq!(
                super::super::merged::merge_names(base, &others),
                case["expected"].as_str().unwrap(),
                "{id}"
            );
        }
        for case in vectors["split"].as_array().expect("split vectors") {
            let id = case["name"].as_str().unwrap_or("?");
            let made = sources(
                &strings(&case["vocab"]),
                &strings(&case["exempt"]),
                &strings(&case["inventory"]),
            );
            let link_owned: Vec<Option<String>> = case["link_keys"]
                .as_array()
                .unwrap()
                .iter()
                .map(opt_str)
                .collect();
            let link_keys: Vec<Option<&str>> =
                link_owned.iter().map(|item| item.as_deref()).collect();
            let split_owned = case["split"].as_array().map(|items| {
                items
                    .iter()
                    .filter_map(|item| item.as_str().map(str::to_string))
                    .collect::<Vec<_>>()
            });
            let split_refs: Option<Vec<&str>> = split_owned
                .as_ref()
                .map(|items| items.iter().map(String::as_str).collect());
            let outcome = super::super::merged::split_plan(
                &super::super::merged::SplitPlanInput {
                    name: case["entry"].as_str().unwrap(),
                    open: case["open"].as_bool().unwrap_or(true),
                    link_keys: &link_keys,
                    split_selectors: split_refs.as_deref(),
                    sources: &made,
                },
            );
            let expected = &case["expected"];
            match expected["kind"].as_str().unwrap() {
                "ok" => {
                    let plan = outcome.unwrap_or_else(|error| {
                        panic!("{id}: expected ok, got {error:?}")
                    });
                    assert_eq!(
                        plan.remaining_name,
                        expected["remaining"].as_str().unwrap(),
                        "{id}"
                    );
                    let created = expected["created"].as_array().unwrap();
                    assert_eq!(plan.created.len(), created.len(), "{id}");
                    for (got, want) in plan.created.iter().zip(created) {
                        assert_eq!(
                            got.name,
                            want["name"].as_str().unwrap(),
                            "{id}"
                        );
                        let indices: Vec<usize> = want["link_indices"]
                            .as_array()
                            .unwrap()
                            .iter()
                            .map(|item| item.as_u64().unwrap() as usize)
                            .collect();
                        assert_eq!(got.link_indices, indices, "{id}");
                    }
                }
                "error" => {
                    let error = outcome
                        .expect_err(&format!("{id}: expected error, got ok"));
                    assert_eq!(
                        error.code(),
                        expected["code"].as_str().unwrap(),
                        "{id}"
                    );
                }
                other => panic!("{id}: unexpected split kind {other}"),
            }
        }
        // The `narrow` vectors stay for the Ledger Tools JS mirror,
        // which still runs `narrowClosed` until the task-status-cycler
        // close switches to `settleClosed`. Rust reads them no longer:
        // the close settle (`agree::settle_closed`) replaced
        // `merged::narrow_closed`.
        for case in vectors["shed"].as_array().expect("shed vectors") {
            let id = case["name"].as_str().unwrap_or("?");
            let made = sources(
                &strings(&case["vocab"]),
                &strings(&case["exempt"]),
                &strings(&case["inventory"]),
            );
            let before: Vec<&str> = case["before"]
                .as_array()
                .unwrap()
                .iter()
                .map(|item| item.as_str().unwrap())
                .collect();
            let after: Vec<&str> = case["after"]
                .as_array()
                .unwrap()
                .iter()
                .map(|item| item.as_str().unwrap())
                .collect();
            assert_eq!(
                super::super::merged::shed(
                    case["entry"].as_str().unwrap(),
                    &before,
                    &after,
                    &made,
                ),
                case["expected"].as_str().unwrap(),
                "{id}"
            );
        }
        for case in vectors["select"].as_array().expect("select vectors") {
            let id = case["name"].as_str().unwrap_or("?");
            let made = sources(
                &strings(&case["vocab"]),
                &strings(&case["exempt"]),
                &strings(&case["inventory"]),
            );
            let entries = case["entries"]
                .as_array()
                .unwrap()
                .iter()
                .map(|entry| super::super::merged::SelectEntry {
                    name: entry["name"].as_str(),
                    open: entry["open"].as_bool().unwrap(),
                    running: entry["running"].as_bool().unwrap(),
                    timed: entry["timed"].as_bool().unwrap_or(false),
                })
                .collect::<Vec<_>>();
            let filter = match case["filter"].as_str().unwrap() {
                "open" => super::super::merged::SelectFilter::Open,
                "untimed_planned" => {
                    super::super::merged::SelectFilter::UntimedPlanned {
                        except: case["except"]
                            .as_u64()
                            .map(|index| index as usize),
                    }
                }
                "merged_base" => super::super::merged::SelectFilter::MergedBase,
                other => panic!("{id}: unexpected filter {other}"),
            };
            let outcome = super::super::merged::select_by_type(
                &super::super::merged::SelectInput {
                    entries: &entries,
                    selector: case["selector"].as_str().unwrap(),
                    filter,
                    sources: &made,
                },
            );
            let expected = &case["expected"];
            match expected["kind"].as_str().unwrap() {
                "entry" => {
                    assert_eq!(
                        outcome,
                        super::super::merged::SelectOutcome::Entry(
                            expected["index"].as_u64().unwrap() as usize
                        ),
                        "{id}"
                    );
                }
                "missing" => {
                    assert_eq!(
                        outcome,
                        super::super::merged::SelectOutcome::Missing,
                        "{id}"
                    );
                }
                other => panic!("{id}: unexpected select kind {other}"),
            }
        }
    }
}
