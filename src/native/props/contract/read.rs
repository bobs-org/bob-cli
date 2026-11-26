//! The `contract` block on `bob props show` for area and project notes.

use std::path::{Path, PathBuf};

use serde::Serialize;
use serde_json::{json, Value as JsonValue};

use super::{issues, settings, settings::Setting, Context};
use crate::native::{
    config::{PlanConfig, ThemesConfig},
    freshness::state::parse_note_refresh,
    markdown,
    note_ready::{parse_ready_cap, CapSource},
    notebooks::snapshot::{display_slash, LiteKind, VaultSnapshot},
    pomodoro_homes::{
        HomeHueSource, HomeResolver, Hue, IndexedHomeSource, NoteColor,
        NoteFacts, NoteOffClock, NoteOffClockTypes, NoteTheme,
    },
    projects::{
        contains_task_tag, has_trailing_prj_anchor, parse_frontmatter,
        wikilink_path_target,
    },
    props::{
        document::PropsDocument, roles::NoteKind, show::CONTRACT_CAPABILITY,
    },
    task_dependencies::{
        blocks::{blocks_children_of, BlocksLine},
        parse::{dependency_child_of, DependencyLine},
    },
    vault_links::NoteIndex,
};

/// Show-report contract block present on area and project notes.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub(crate) struct ContractBlock {
    pub(crate) kind: String,
    pub(crate) settings: Vec<SettingReport>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) prj: Option<PrjFacts>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub(crate) struct SettingReport {
    pub(crate) id: &'static str,
    pub(crate) label: &'static str,
    pub(crate) keys: &'static [&'static str],
    pub(crate) stage: &'static str,
    pub(crate) declared: bool,
    pub(crate) value: JsonValue,
    pub(crate) effective: Effective,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(crate) choices: Vec<Choice>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(crate) issues: Vec<issues::ContractIssue>,
    pub(crate) help: String,
    pub(crate) disabled: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub(crate) struct Effective {
    pub(crate) value: JsonValue,
    pub(crate) source: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) from: Option<String>,
    pub(crate) label: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub(crate) struct Choice {
    pub(crate) id: String,
    pub(crate) label: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) hue: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(crate) used_by: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub(crate) struct PrjFacts {
    pub(crate) line: Option<String>,
    pub(crate) criterion: Option<String>,
    pub(crate) scheduled: Option<String>,
    pub(crate) depends_on: Vec<String>,
    pub(crate) blocks: Vec<String>,
}

pub(super) struct View<'a> {
    pub snapshot: VaultSnapshot,
    pub index: NoteIndex,
    pub buffers: Vec<(String, String)>,
    pub types: &'a crate::native::config::themes::ThemeSources,
    pub plan: &'a PlanConfig,
    pub freshness: &'a crate::native::config::FreshnessConfig,
}

impl<'a> View<'a> {
    pub(super) fn new(
        context: &'a Context,
        path: &str,
        contents: &str,
    ) -> Self {
        let snapshot = context
            .snapshot
            .with_overlay([(PathBuf::from(path), contents.to_owned())]);
        let buffers = buffers_of(&snapshot);
        let index = snapshot.index();
        Self {
            snapshot,
            index,
            buffers,
            types: &context.types,
            plan: &context.plan,
            freshness: &context.freshness,
        }
    }

    pub(super) fn resolver(&self) -> HomeResolver<IndexedHomeSource<'_>> {
        HomeResolver::new(IndexedHomeSource::from_buffers(
            &self.index,
            &self.buffers,
        ))
    }

    pub(super) fn slash(path: &Path) -> String {
        display_slash(path)
    }

    pub(super) fn stem(path: &Path) -> String {
        path.file_stem()
            .and_then(|stem| stem.to_str())
            .unwrap_or("")
            .to_owned()
    }

    pub(super) fn parent_of(&self, path: &Path) -> Option<PathBuf> {
        let file = self.snapshot.files.get(path)?;
        let raw = file.lite.parent_raw.as_deref()?;
        let target = wikilink_path_target(raw)?;
        self.index.resolve(Some(path), &target)
    }

    pub(super) fn facts(&self, path: &Path) -> Option<NoteFacts> {
        self.snapshot
            .files
            .get(path)
            .map(|file| file.contents.as_deref().unwrap_or(&file.header))
            .map(NoteFacts::parse)
    }

    pub(super) fn descendants(&self, start: &Path) -> Vec<PathBuf> {
        let mut found = Vec::new();
        let mut stack = vec![start.to_path_buf()];
        let mut seen = std::collections::BTreeSet::new();
        seen.insert(start.to_path_buf());
        while let Some(current) = stack.pop() {
            for path in self.snapshot.files.keys() {
                if seen.contains(path) {
                    continue;
                }
                if self.parent_of(path).as_deref() == Some(current.as_path()) {
                    seen.insert(path.clone());
                    found.push(path.clone());
                    stack.push(path.clone());
                }
            }
        }
        found
    }
}

pub(super) fn buffers_of(snapshot: &VaultSnapshot) -> Vec<(String, String)> {
    snapshot
        .files
        .iter()
        .map(|(path, file)| {
            (
                display_slash(path),
                file.contents.clone().unwrap_or_else(|| file.header.clone()),
            )
        })
        .collect()
}

/// Build the contract block when this note kind has settings.
pub(crate) fn build(
    context: &Context,
    path: &str,
    contents: &str,
    kind: NoteKind,
    document: &PropsDocument,
) -> Option<ContractBlock> {
    if settings::for_kind(kind).is_empty() {
        return None;
    }
    let view = View::new(context, path, contents);
    Some(build_view(&view, path, contents, kind, document))
}

pub(crate) fn capabilities(kind: NoteKind) -> Vec<&'static str> {
    if settings::for_kind(kind).is_empty() {
        vec![crate::native::props::show::CAPABILITY]
    } else {
        vec![crate::native::props::show::CAPABILITY, CONTRACT_CAPABILITY]
    }
}

pub(super) fn build_view(
    view: &View<'_>,
    path: &str,
    contents: &str,
    kind: NoteKind,
    document: &PropsDocument,
) -> ContractBlock {
    let note = Path::new(path);
    let mut resolver = view.resolver();
    let home = resolver.resolve_note(note);
    let facts = NoteFacts::parse(contents);
    let note_issues = issues::from_contents(contents);
    let used_by = hue_used_by(view, &mut resolver);
    let settings = settings::for_kind(kind)
        .iter()
        .map(|setting| {
            setting_report(
                view,
                setting,
                kind,
                note,
                document,
                contents,
                &facts,
                &home,
                &note_issues,
                &used_by,
            )
        })
        .collect();
    ContractBlock {
        kind: kind.wire_name().to_owned(),
        settings,
        prj: (kind == NoteKind::Project).then(|| prj_facts(contents)),
    }
}

#[allow(clippy::too_many_arguments)]
fn setting_report(
    view: &View<'_>,
    setting: &Setting,
    kind: NoteKind,
    path: &Path,
    document: &PropsDocument,
    contents: &str,
    facts: &NoteFacts,
    home: &crate::native::pomodoro_homes::Home,
    note_issues: &[issues::ContractIssue],
    used_by: &std::collections::BTreeMap<String, Vec<String>>,
) -> SettingReport {
    let declared = setting
        .keys
        .iter()
        .any(|key| document.entries.iter().any(|entry| entry.key == *key));
    let value = declared_value(setting, document);
    let (effective, disabled) = match setting.id {
        "summary" => summary_effective(document, declared),
        "parent" => parent_effective(view, path, declared, document),
        "color" => color_effective(view, path, facts, home, declared),
        "task_type" => type_effective(view, path, facts, declared),
        "clock" => clock_effective(view, path, facts, declared),
        "root" => root_effective(view, path, facts, home, declared),
        "ready_cap" => cap_effective(view, contents, declared),
        "task_refresh" => review_effective(view, contents, declared),
        "project_name_aliases" => aliases_effective(document, declared),
        _ => (
            Effective {
                value: JsonValue::Null,
                source: "none",
                from: None,
                label: String::new(),
            },
            false,
        ),
    };
    let choices = match setting.id {
        "color" => hue_choices(used_by),
        "task_type" => type_choices(view.types.themes.as_ref()),
        _ => Vec::new(),
    };
    let issues = note_issues
        .iter()
        .filter(|issue| issue_belongs(setting.id, &issue.code))
        .cloned()
        .collect();
    SettingReport {
        id: setting.id,
        label: setting.label,
        keys: setting.keys,
        stage: setting.stage,
        declared,
        value,
        effective,
        choices,
        issues,
        help: help_for(setting.id, kind),
        disabled,
    }
}

fn declared_value(setting: &Setting, document: &PropsDocument) -> JsonValue {
    if setting.id == "clock" {
        let mut object = serde_json::Map::new();
        for key in setting.keys {
            if let Some(entry) = document.entries.iter().find(|e| e.key == *key)
            {
                object.insert(key.to_string(), entry.value.clone());
            }
        }
        if object.is_empty() {
            JsonValue::Null
        } else {
            JsonValue::Object(object)
        }
    } else {
        setting
            .keys
            .iter()
            .find_map(|key| {
                document
                    .entries
                    .iter()
                    .find(|entry| entry.key == *key)
                    .map(|entry| entry.value.clone())
            })
            .unwrap_or(JsonValue::Null)
    }
}

fn summary_effective(
    document: &PropsDocument,
    declared: bool,
) -> (Effective, bool) {
    if declared {
        let value = document
            .entries
            .iter()
            .find(|entry| entry.key == "summary")
            .map(|entry| entry.value.clone())
            .unwrap_or(JsonValue::Null);
        (
            Effective {
                value,
                source: "self",
                from: None,
                label: "set on this note".to_owned(),
            },
            false,
        )
    } else {
        (
            Effective {
                value: JsonValue::Null,
                source: "none",
                from: None,
                label: "no summary — catalogs show the link only".to_owned(),
            },
            false,
        )
    }
}

fn parent_effective(
    view: &View<'_>,
    path: &Path,
    declared: bool,
    document: &PropsDocument,
) -> (Effective, bool) {
    if !declared {
        return (
            Effective {
                value: JsonValue::Null,
                source: "none",
                from: None,
                label: "⚠ no parent".to_owned(),
            },
            false,
        );
    }
    let value = document
        .entries
        .iter()
        .find(|entry| entry.key == "parent")
        .map(|entry| entry.value.clone())
        .unwrap_or(JsonValue::Null);
    let parent = view.parent_of(path);
    let root = view.index.resolve(None, "area");
    let label = match parent.as_ref() {
        Some(parent) if root.as_ref() == Some(parent) => {
            format!("[[{}]] · top level", View::stem(parent))
        }
        Some(parent) => format!("[[{}]]", View::stem(parent)),
        None => "⚠ no parent".to_owned(),
    };
    (
        Effective {
            value,
            source: "self",
            from: parent.map(|path| View::slash(&path)),
            label,
        },
        false,
    )
}

fn color_effective(
    view: &View<'_>,
    path: &Path,
    facts: &NoteFacts,
    home: &crate::native::pomodoro_homes::Home,
    declared: bool,
) -> (Effective, bool) {
    if declared {
        let (value, label) = match &facts.color {
            Some(NoteColor::Hue(hue)) => (
                json!(hue.name()),
                format!("● {} · set on this note", hue.name()),
            ),
            Some(NoteColor::Grey) => {
                (json!("grey"), "● grey · set on this note".to_owned())
            }
            Some(NoteColor::Invalid(raw)) => {
                (json!(raw), format!("{raw} · invalid; inherit applies"))
            }
            None => (JsonValue::Null, "set on this note".to_owned()),
        };
        return (
            Effective {
                value,
                source: "self",
                from: Some(View::slash(path)),
                label,
            },
            false,
        );
    }
    let slot = color_slot(view, path);
    match home.hue_source {
        Some(HomeHueSource::Hash) => {
            let name = home.hue.map(Hue::name).unwrap_or("aqua");
            (
                Effective {
                    value: json!(name),
                    source: "hash",
                    from: Some(home.area.clone()),
                    label: format!(
                        "● {name} · from the name (nothing sets a color)"
                    ),
                },
                false,
            )
        }
        Some(HomeHueSource::Muted) => {
            let (from, stem) =
                slot.unwrap_or_else(|| (home.area.clone(), home.area.clone()));
            (
                Effective {
                    value: json!("grey"),
                    source: "inherited",
                    from: Some(from),
                    label: format!("● grey · from {stem}"),
                },
                false,
            )
        }
        Some(HomeHueSource::Color) => {
            let name = home.hue.map(Hue::name).unwrap_or("blue");
            let (from, stem) =
                slot.unwrap_or_else(|| (home.area.clone(), home.area.clone()));
            (
                Effective {
                    value: json!(name),
                    source: "inherited",
                    from: Some(from),
                    label: format!("● {name} · from {stem}"),
                },
                false,
            )
        }
        None => (
            Effective {
                value: JsonValue::Null,
                source: "none",
                from: None,
                label: "no hue".to_owned(),
            },
            false,
        ),
    }
}

fn color_slot(view: &View<'_>, path: &Path) -> Option<(String, String)> {
    let mut current = Some(path.to_path_buf());
    let mut seen = std::collections::BTreeSet::new();
    while let Some(node) = current {
        if !seen.insert(node.clone()) {
            break;
        }
        if let Some(facts) = view.facts(&node)
            && matches!(
                facts.kind,
                crate::native::pomodoro_homes::NoteKind::Area
                    | crate::native::pomodoro_homes::NoteKind::Project
            )
            && facts.color.is_some()
        {
            return Some((View::slash(&node), View::stem(&node)));
        }
        current = view.parent_of(&node);
    }
    None
}

fn type_effective(
    view: &View<'_>,
    path: &Path,
    facts: &NoteFacts,
    declared: bool,
) -> (Effective, bool) {
    if declared {
        let (value, label) = match &facts.theme {
            Some(NoteTheme::Off) => {
                (json!("off"), "off · Stop parent defaults".to_owned())
            }
            Some(NoteTheme::Name(name)) => {
                (json!(name), format!("#{name} · set on this note"))
            }
            Some(NoteTheme::Invalid(raw)) => (json!(raw), raw.clone()),
            None => (JsonValue::Null, "set on this note".to_owned()),
        };
        return (
            Effective {
                value,
                source: "self",
                from: Some(View::slash(path)),
                label,
            },
            false,
        );
    }
    match type_slot(view, path) {
        Some((from, stem, NoteTheme::Name(name))) => (
            Effective {
                value: json!(name),
                source: "inherited",
                from: Some(from),
                label: format!("#{name} · from {stem}"),
            },
            false,
        ),
        Some((from, stem, NoteTheme::Off)) => (
            Effective {
                value: json!("off"),
                source: "inherited",
                from: Some(from),
                label: format!("off · from {stem}"),
            },
            false,
        ),
        _ => {
            if let Some(name) = view.plan.default_type() {
                (
                    Effective {
                        value: json!(name),
                        source: "config",
                        from: None,
                        label: format!("#{name} · Bob setting"),
                    },
                    false,
                )
            } else {
                (
                    Effective {
                        value: JsonValue::Null,
                        source: "none",
                        from: None,
                        label: "none · nothing above sets one".to_owned(),
                    },
                    false,
                )
            }
        }
    }
}

fn type_slot(
    view: &View<'_>,
    path: &Path,
) -> Option<(String, String, NoteTheme)> {
    let mut current = view.parent_of(path);
    let mut seen = std::collections::BTreeSet::new();
    while let Some(node) = current {
        if !seen.insert(node.clone()) {
            break;
        }
        if let Some(facts) = view.facts(&node)
            && matches!(
                facts.kind,
                crate::native::pomodoro_homes::NoteKind::Area
                    | crate::native::pomodoro_homes::NoteKind::Project
            )
            && let Some(theme) = facts.theme.clone()
        {
            return Some((View::slash(&node), View::stem(&node), theme));
        }
        current = view.parent_of(&node);
    }
    None
}

fn clock_effective(
    view: &View<'_>,
    path: &Path,
    facts: &NoteFacts,
    declared: bool,
) -> (Effective, bool) {
    if declared {
        let (value, label) = clock_value(facts);
        return (
            Effective {
                value,
                source: "self",
                from: Some(View::slash(path)),
                label: format!("{label} · set on this note"),
            },
            false,
        );
    }
    match clock_slot(view, path) {
        Some((from, stem, facts)) => {
            let (value, label) = clock_value(&facts);
            (
                Effective {
                    value,
                    source: "inherited",
                    from: Some(from),
                    label: format!("{label} · from {stem}"),
                },
                false,
            )
        }
        None => (
            Effective {
                value: json!({"mode": "on"}),
                source: "none",
                from: None,
                label: "on-clock · nothing above sets a policy".to_owned(),
            },
            false,
        ),
    }
}

fn clock_value(facts: &NoteFacts) -> (JsonValue, String) {
    if let Some(NoteOffClockTypes::Keys(names)) = &facts.offclock_types {
        let joined = names.join(", ");
        return (
            json!({"mode": "types", "types": names}),
            format!("off-clock for {joined}"),
        );
    }
    match &facts.offclock {
        Some(NoteOffClock::True) => {
            (json!({"mode": "off"}), "off-clock".to_owned())
        }
        Some(NoteOffClock::False) => {
            (json!({"mode": "on"}), "on-clock".to_owned())
        }
        _ => (json!({"mode": "on"}), "on-clock".to_owned()),
    }
}

fn clock_slot(
    view: &View<'_>,
    path: &Path,
) -> Option<(String, String, NoteFacts)> {
    let mut current = view.parent_of(path);
    let mut seen = std::collections::BTreeSet::new();
    while let Some(node) = current {
        if !seen.insert(node.clone()) {
            break;
        }
        if let Some(facts) = view.facts(&node)
            && matches!(
                facts.kind,
                crate::native::pomodoro_homes::NoteKind::Area
                    | crate::native::pomodoro_homes::NoteKind::Project
            )
            && (facts.offclock.is_some() || facts.offclock_types.is_some())
        {
            return Some((View::slash(&node), View::stem(&node), facts));
        }
        current = view.parent_of(&node);
    }
    None
}

fn root_effective(
    view: &View<'_>,
    path: &Path,
    facts: &NoteFacts,
    home: &crate::native::pomodoro_homes::Home,
    declared: bool,
) -> (Effective, bool) {
    let parent_is_project = view.parent_of(path).is_some_and(|parent| {
        view.snapshot.files.get(&parent).is_some_and(|file| {
            matches!(file.lite.kind, LiteKind::Project { .. })
        })
    });
    let disabled = !parent_is_project;
    if declared && facts.root {
        return (
            Effective {
                value: json!(true),
                source: "self",
                from: Some(View::slash(path)),
                label: "reports under this project".to_owned(),
            },
            disabled,
        );
    }
    if disabled {
        (
            Effective {
                value: json!(true),
                source: "none",
                from: None,
                label: "already a root (top-level project)".to_owned(),
            },
            true,
        )
    } else {
        (
            Effective {
                value: json!(false),
                source: "inherited",
                from: Some(home.root_parent.clone()),
                label: format!("reports under {}", home.root_parent),
            },
            false,
        )
    }
}

fn cap_effective(
    view: &View<'_>,
    contents: &str,
    declared: bool,
) -> (Effective, bool) {
    let default = view.plan.max_ready_per_note();
    let raw = parse_frontmatter(contents).and_then(|front| {
        crate::native::projects::frontmatter_value(&front, "ready_cap")
            .map(str::to_string)
    });
    let (cap, _, off, _) =
        parse_ready_cap(raw.as_deref(), default, CapSource::Config);
    if declared {
        let (value, label) = if off {
            (json!("off"), "off · no cap".to_owned())
        } else {
            let cap = cap.unwrap_or(default);
            (json!(cap), format!("{cap} · set on this note"))
        };
        (
            Effective {
                value,
                source: "self",
                from: None,
                label,
            },
            false,
        )
    } else {
        (
            Effective {
                value: json!(default),
                source: "config",
                from: None,
                label: format!(
                    "{default} · Bob setting (plan.max_ready_per_note)"
                ),
            },
            false,
        )
    }
}

fn review_effective(
    view: &View<'_>,
    contents: &str,
    declared: bool,
) -> (Effective, bool) {
    let raw = parse_frontmatter(contents).and_then(|front| {
        crate::native::projects::frontmatter_value(&front, "task_refresh")
            .map(str::to_string)
    });
    let (days, _) = parse_note_refresh(raw.as_deref());
    if declared {
        if let Some(days) = days {
            (
                Effective {
                    value: json!(days),
                    source: "self",
                    from: None,
                    label: format!("{days} days · set on this note"),
                },
                false,
            )
        } else {
            (
                Effective {
                    value: JsonValue::Null,
                    source: "self",
                    from: None,
                    label: "invalid · Bob setting applies".to_owned(),
                },
                false,
            )
        }
    } else {
        let days = view.freshness.interval;
        (
            Effective {
                value: json!(days),
                source: "config",
                from: None,
                label: format!(
                    "{days} days · Bob setting (freshness.interval)"
                ),
            },
            false,
        )
    }
}

fn aliases_effective(
    document: &PropsDocument,
    declared: bool,
) -> (Effective, bool) {
    if declared {
        let value = document
            .entries
            .iter()
            .find(|entry| entry.key == "project_name_aliases")
            .map(|entry| entry.value.clone())
            .unwrap_or(JsonValue::Null);
        (
            Effective {
                value,
                source: "self",
                from: None,
                label: "set on this note".to_owned(),
            },
            false,
        )
    } else {
        (
            Effective {
                value: json!([]),
                source: "none",
                from: None,
                label: "none — capture uses the stem".to_owned(),
            },
            false,
        )
    }
}

fn hue_choices(
    used_by: &std::collections::BTreeMap<String, Vec<String>>,
) -> Vec<Choice> {
    let mut choices: Vec<Choice> = Hue::ALL
        .iter()
        .map(|hue| Choice {
            id: hue.name().to_owned(),
            label: hue.name().to_owned(),
            hue: Some(hue.name().to_owned()),
            used_by: used_by.get(hue.name()).cloned().unwrap_or_default(),
        })
        .collect();
    choices.push(Choice {
        id: "grey".to_owned(),
        label: "grey".to_owned(),
        hue: None,
        used_by: used_by.get("grey").cloned().unwrap_or_default(),
    });
    choices
}

fn type_choices(themes: Option<&ThemesConfig>) -> Vec<Choice> {
    let Some(themes) = themes else {
        return Vec::new();
    };
    themes
        .entries
        .iter()
        .map(|entry| Choice {
            id: entry.key.clone(),
            label: entry.name.clone(),
            hue: entry.color.map(Hue::name).map(str::to_owned),
            used_by: Vec::new(),
        })
        .collect()
}

fn hue_used_by(
    view: &View<'_>,
    resolver: &mut HomeResolver<IndexedHomeSource<'_>>,
) -> std::collections::BTreeMap<String, Vec<String>> {
    let root = view.index.resolve(None, "area");
    let mut used = std::collections::BTreeMap::<String, Vec<String>>::new();
    for (path, file) in &view.snapshot.files {
        if file.lite.kind != LiteKind::Area {
            continue;
        }
        let Some(parent) = view.parent_of(path) else {
            continue;
        };
        if root.as_ref() != Some(&parent) {
            continue;
        }
        let home = resolver.resolve_note(path);
        let key = match home.hue_source {
            Some(HomeHueSource::Muted) => "grey".to_owned(),
            _ => home.hue.map(Hue::name).unwrap_or("hash").to_owned(),
        };
        used.entry(key).or_default().push(file.lite.stem.clone());
    }
    for stems in used.values_mut() {
        stems.sort();
        stems.dedup();
    }
    used
}

fn issue_belongs(setting: &str, code: &str) -> bool {
    match setting {
        "summary" => {
            code == "area_summary_multiline" || code == "area_summary_long"
        }
        "parent" => code.starts_with("area_parent") || code == "area_cycle",
        "color" => code == "area_color_invalid",
        "task_type" => {
            code == "type_default_conflict" || code == "note_theme_unconfigured"
        }
        "clock" => {
            code == "note_offclock_shadowed" || code == "note_offclock_invalid"
        }
        "ready_cap" => code == "note_ready_cap_invalid",
        "task_refresh" => code == "task_refresh_invalid",
        _ => false,
    }
}

fn help_for(id: &str, kind: NoteKind) -> String {
    match id {
        "summary" => "One line of text; over 140 characters warns.".to_owned(),
        "parent" if kind == NoteKind::Project => {
            "Required. An area or a non-terminal project.".to_owned()
        }
        "parent" => "Required. The area root or another area.".to_owned(),
        "color" => "Palette hue or grey. Clear to inherit.".to_owned(),
        "task_type" => {
            "A type name or off (Stop parent defaults). Clear to inherit."
                .to_owned()
        }
        "clock" => "Inherit, on-clock, off-clock, or off for types.".to_owned(),
        "root" => "When true, time here reports under this project.".to_owned(),
        "ready_cap" => {
            "1–999 or off. Applies to tasks living in this note only."
                .to_owned()
        }
        "task_refresh" => {
            "1–365 days. Applies to tasks living in this note only.".to_owned()
        }
        "project_name_aliases" => {
            "Extra capture names; the stem always wins.".to_owned()
        }
        _ => String::new(),
    }
}

pub(super) fn prj_facts(contents: &str) -> PrjFacts {
    let Some((line_number, line)) = first_prj_line(contents) else {
        return PrjFacts {
            line: None,
            criterion: None,
            scheduled: None,
            depends_on: Vec::new(),
            blocks: Vec::new(),
        };
    };
    let split: Vec<&str> = contents.split('\n').collect();
    let index = line_number.saturating_sub(1);
    let mut fenced = std::collections::BTreeSet::new();
    let mut fence = None;
    for (offset, row) in split.iter().enumerate() {
        let trimmed = row.strip_suffix('\r').unwrap_or(row);
        if let Some(marker) = markdown::fence_marker(trimmed) {
            if let Some(open) = fence {
                if markdown::closes_fence(trimmed, open) {
                    fence = None;
                    fenced.insert(offset);
                    continue;
                }
            } else {
                fence = Some(marker);
                fenced.insert(offset);
                continue;
            }
        }
        if fence.is_some() {
            fenced.insert(offset);
        }
    }
    let depends_on = match dependency_child_of(&split, &fenced, index)
        .map(|child| child.parsed)
    {
        Some(DependencyLine::Accepted { links, .. }) => links
            .into_iter()
            .map(|link| {
                if link.target.is_empty() {
                    format!("#^{}", link.block_id)
                } else {
                    format!("{}#^{}", link.target, link.block_id)
                }
            })
            .collect(),
        _ => Vec::new(),
    };
    let blocks = blocks_children_of(&split, &fenced, index)
        .into_iter()
        .find_map(|child| match child.parsed {
            BlocksLine::Accepted { links, .. } => Some(
                links
                    .into_iter()
                    .map(|link| {
                        if link.target.is_empty() {
                            format!("#^{}", link.block_id)
                        } else {
                            format!("{}#^{}", link.target, link.block_id)
                        }
                    })
                    .collect(),
            ),
            _ => None,
        })
        .unwrap_or_default();
    PrjFacts {
        line: Some(line.trim_end_matches('\r').to_owned()),
        criterion: criterion_of(&line),
        scheduled: scheduled_of(&line),
        depends_on,
        blocks,
    }
}

fn first_prj_line(contents: &str) -> Option<(usize, String)> {
    let body_start = parse_frontmatter(contents)
        .map(|front| front.body_start_line)
        .unwrap_or(0);
    let mut fence = None;
    for (index, line) in contents.split('\n').enumerate() {
        let number = index + 1;
        if number <= body_start {
            continue;
        }
        let trimmed = line.strip_suffix('\r').unwrap_or(line);
        if let Some(marker) = markdown::fence_marker(trimmed) {
            if let Some(open) = fence {
                if markdown::closes_fence(trimmed, open) {
                    fence = None;
                }
            } else {
                fence = Some(marker);
            }
            continue;
        }
        if fence.is_some() {
            continue;
        }
        if trimmed.starts_with("## ") {
            break;
        }
        if has_trailing_prj_anchor(trimmed) && contains_task_tag(trimmed) {
            return Some((number, trimmed.to_owned()));
        }
    }
    None
}

fn criterion_of(line: &str) -> Option<String> {
    let text = line.trim_start().trim_start_matches(['>', ' ', '\t']);
    let after = text.split_once("] ")?.1;
    let mut out = String::new();
    let mut rest = after;
    while !rest.is_empty() {
        if rest.starts_with('[')
            && let Some(end) = rest.find(']')
        {
            rest = &rest[end + 1..];
            continue;
        }
        if rest.starts_with('#') || rest.starts_with('^') {
            if let Some((token, tail)) = rest.split_once(' ') {
                let _ = token;
                rest = tail;
                continue;
            }
            break;
        }
        let (token, tail) = rest.split_once(' ').unwrap_or((rest, ""));
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(token);
        rest = tail;
    }
    let collapsed = out.split_whitespace().collect::<Vec<_>>().join(" ");
    if collapsed.is_empty() {
        None
    } else {
        Some(collapsed)
    }
}

fn scheduled_of(line: &str) -> Option<String> {
    let start = line.find("[scheduled::")?;
    let rest = &line[start + "[scheduled::".len()..];
    let end = rest.find(']')?;
    let date = rest[..end].trim();
    (date.len() == 10).then(|| date.to_owned())
}
