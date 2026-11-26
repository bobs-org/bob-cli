//! Consequence lines for setting dry-run and apply responses.

use std::path::Path;

use serde::Serialize;
use serde_json::Value as JsonValue;

use super::{
    read::{self, SettingReport, View},
    settings, Context,
};
use crate::native::{
    notebooks::snapshot::LiteKind,
    props::{document::PropsDocument, roles::NoteKind},
};

/// One consequence of a setting write.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub(crate) struct Effect {
    pub(crate) setting: &'static str,
    pub(crate) kind: &'static str,
    pub(crate) message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) from: Option<JsonValue>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) to: Option<JsonValue>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) count: Option<usize>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(crate) syncs: Vec<String>,
}

/// Compare the before and after images and name the setting consequences.
pub(crate) fn collect(
    context: &Context,
    path: &str,
    before: &str,
    after: &str,
    kind: NoteKind,
) -> Vec<Effect> {
    if settings::for_kind(kind).is_empty() || before == after {
        return Vec::new();
    }
    let before_doc = PropsDocument::parse(before);
    let after_doc = PropsDocument::parse(after);
    let before_view = View::new(context, path, before);
    let after_view = View::new(context, path, after);
    let before_block =
        read::build_view(&before_view, path, before, kind, &before_doc);
    let after_block =
        read::build_view(&after_view, path, after, kind, &after_doc);
    if kind == NoteKind::Reference {
        return reference_effects(
            path,
            before,
            after,
            &before_block.settings,
            &after_block.settings,
        );
    }
    let note = Path::new(path);
    let mut effects = Vec::new();
    for (old, new) in before_block.settings.iter().zip(&after_block.settings) {
        if old.id != new.id || !setting_changed(old, new) {
            continue;
        }
        match new.id {
            "parent" => effects.extend(parent_effects(
                &before_view,
                &after_view,
                &before_block.settings,
                &after_block.settings,
                note,
            )),
            "color" | "task_type" | "clock" => {
                let count = inherit_count(&after_view, note, new.id);
                effects.push(Effect {
                    setting: new.id,
                    kind: "inherited",
                    message: format!(
                        "Applies to {count} notes below that don't set their own"
                    ),
                    from: Some(old.effective.value.clone()),
                    to: Some(new.effective.value.clone()),
                    count: Some(count),
                    syncs: Vec::new(),
                });
            }
            "ready_cap" | "task_refresh" => effects.push(Effect {
                setting: new.id,
                kind: "residence",
                message: "Applies to tasks living in this note only".to_owned(),
                from: Some(old.effective.value.clone()),
                to: Some(new.effective.value.clone()),
                count: None,
                syncs: Vec::new(),
            }),
            _ => {}
        }
    }
    effects
}

fn reference_effects(
    _path: &str,
    before: &str,
    after: &str,
    before_settings: &[SettingReport],
    after_settings: &[SettingReport],
) -> Vec<Effect> {
    let before_doc = PropsDocument::parse(before);
    let after_doc = PropsDocument::parse(after);
    let has_marker = ["highlights_marker_base"].iter().any(|key| {
        before_doc.entries.iter().any(|entry| entry.key == *key)
            || after_doc.entries.iter().any(|entry| entry.key == *key)
    });
    let mut effects = Vec::new();
    for (old, new) in before_settings.iter().zip(after_settings) {
        if old.id != new.id || !setting_changed(old, new) {
            continue;
        }
        if has_marker
            && matches!(new.id, "title" | "author" | "published" | "source")
        {
            effects.push(Effect {
                setting: new.id,
                kind: "syncs",
                message: "Syncs to the PDF marker at the next writing scan"
                    .to_owned(),
                from: Some(old.effective.value.clone()),
                to: Some(new.effective.value.clone()),
                count: None,
                syncs: vec!["pdf_marker".to_owned()],
            });
        }
        if new.id == "title"
            && crate::native::ref_tasks::find_managed_embed(
                &before[before_doc.body_range.clone()],
            )
            .is_some()
        {
            effects.push(Effect {
                setting: "title",
                kind: "reading_task_link_text",
                message:
                    "The reading task keeps its link text; open it to retitle."
                        .to_owned(),
                from: Some(old.effective.value.clone()),
                to: Some(new.effective.value.clone()),
                count: None,
                syncs: Vec::new(),
            });
        }
    }
    effects
}

fn setting_changed(old: &SettingReport, new: &SettingReport) -> bool {
    old.declared != new.declared
        || old.value != new.value
        || old.effective.value != new.effective.value
        || old.effective.source != new.effective.source
        || old.effective.from != new.effective.from
}

fn parent_effects(
    before_view: &View<'_>,
    after_view: &View<'_>,
    before_settings: &[SettingReport],
    after_settings: &[SettingReport],
    path: &Path,
) -> Vec<Effect> {
    let mut before_resolver = before_view.resolver();
    let mut after_resolver = after_view.resolver();
    let before_home = before_resolver.resolve_note(path);
    let after_home = after_resolver.resolve_note(path);
    let mut effects = Vec::new();
    push_parent_change(
        &mut effects,
        "hue",
        setting(before_settings, "color"),
        setting(after_settings, "color"),
        |from, to| format!("Hue {from} → {to}"),
    );
    if before_home.root_parent != after_home.root_parent {
        effects.push(Effect {
            setting: "parent",
            kind: "root_parent",
            message: format!(
                "Root parent {} → {} — root parents follow the current tree, so past time regroups too",
                before_home.root_parent, after_home.root_parent
            ),
            from: Some(JsonValue::String(before_home.root_parent.clone())),
            to: Some(JsonValue::String(after_home.root_parent.clone())),
            count: None,
            syncs: Vec::new(),
        });
    }
    push_parent_change(
        &mut effects,
        "type",
        setting(before_settings, "task_type"),
        setting(after_settings, "task_type"),
        |from, to| format!("Inherited type {from} → {to}"),
    );
    push_parent_change(
        &mut effects,
        "clock",
        setting(before_settings, "clock"),
        setting(after_settings, "clock"),
        |from, to| format!("Inherited clock {from} → {to}"),
    );
    let before_block = inherited_block(before_view, path);
    let after_block = inherited_block(after_view, path);
    match (before_block.as_deref(), after_block.as_deref()) {
        (None, Some(stem)) => effects.push(Effect {
            setting: "parent",
            kind: "blocking",
            message: format!("Gains inherited blocking from {stem}"),
            from: None,
            to: Some(JsonValue::String(stem.to_owned())),
            count: None,
            syncs: Vec::new(),
        }),
        (Some(stem), None) => effects.push(Effect {
            setting: "parent",
            kind: "blocking",
            message: format!("Loses inherited blocking from {stem}"),
            from: Some(JsonValue::String(stem.to_owned())),
            to: None,
            count: None,
            syncs: Vec::new(),
        }),
        (Some(from), Some(to)) if from != to => effects.push(Effect {
            setting: "parent",
            kind: "blocking",
            message: format!("Inherited blocking {from} → {to}"),
            from: Some(JsonValue::String(from.to_owned())),
            to: Some(JsonValue::String(to.to_owned())),
            count: None,
            syncs: Vec::new(),
        }),
        _ => {}
    }
    let syncs = catalog_syncs(after_view, path);
    if !syncs.is_empty() {
        effects.push(Effect {
            setting: "parent",
            kind: "syncs",
            message: format!("Syncs {} catalogs", syncs.join(", ")),
            from: None,
            to: None,
            count: None,
            syncs,
        });
    }
    effects
}

fn push_parent_change(
    effects: &mut Vec<Effect>,
    kind: &'static str,
    old: Option<&SettingReport>,
    new: Option<&SettingReport>,
    message: impl Fn(&str, &str) -> String,
) {
    let (Some(old), Some(new)) = (old, new) else {
        return;
    };
    if old.effective.value == new.effective.value
        && old.effective.from == new.effective.from
        && old.effective.label == new.effective.label
    {
        return;
    }
    let from = display_value(&old.effective.value);
    let to = display_value(&new.effective.value);
    effects.push(Effect {
        setting: "parent",
        kind,
        message: message(&from, &to),
        from: Some(old.effective.value.clone()),
        to: Some(new.effective.value.clone()),
        count: None,
        syncs: Vec::new(),
    });
}

fn setting<'a>(
    settings: &'a [SettingReport],
    id: &str,
) -> Option<&'a SettingReport> {
    settings.iter().find(|setting| setting.id == id)
}

fn display_value(value: &JsonValue) -> String {
    match value {
        JsonValue::Null => "none".to_owned(),
        JsonValue::String(text) => text.clone(),
        JsonValue::Bool(flag) => flag.to_string(),
        JsonValue::Number(number) => number.to_string(),
        JsonValue::Object(object) => object
            .get("mode")
            .and_then(JsonValue::as_str)
            .map(|mode| match mode {
                "off" => "off-clock".to_owned(),
                "on" => "on-clock".to_owned(),
                "types" => object
                    .get("types")
                    .and_then(JsonValue::as_array)
                    .map(|types| {
                        let names: Vec<&str> = types
                            .iter()
                            .filter_map(JsonValue::as_str)
                            .collect();
                        format!("off-clock for {}", names.join(", "))
                    })
                    .unwrap_or_else(|| "off for types".to_owned()),
                other => other.to_owned(),
            })
            .unwrap_or_else(|| value.to_string()),
        other => other.to_string(),
    }
}

fn inherit_count(view: &View<'_>, path: &Path, setting: &str) -> usize {
    view.descendants(path)
        .iter()
        .filter(|child| {
            let Some(facts) = view.facts(child) else {
                return false;
            };
            if !matches!(
                facts.kind,
                crate::native::pomodoro_homes::NoteKind::Area
                    | crate::native::pomodoro_homes::NoteKind::Project
            ) {
                return false;
            }
            match setting {
                "color" => facts.color.is_none(),
                "task_type" => facts.theme.is_none(),
                "clock" => {
                    facts.offclock.is_none() && facts.offclock_types.is_none()
                }
                _ => false,
            }
        })
        .count()
}

fn inherited_block(view: &View<'_>, path: &Path) -> Option<String> {
    let mut current = view.parent_of(path);
    let mut seen = std::collections::BTreeSet::new();
    while let Some(node) = current {
        if !seen.insert(node.clone()) {
            break;
        }
        let Some(file) = view.snapshot.files.get(&node) else {
            current = view.parent_of(&node);
            continue;
        };
        if !matches!(file.lite.kind, LiteKind::Project { .. }) {
            current = view.parent_of(&node);
            continue;
        }
        let contents = file.contents.as_deref().unwrap_or(&file.header);
        if project_blocks(contents) {
            return Some(View::stem(&node));
        }
        current = view.parent_of(&node);
    }
    None
}

fn project_blocks(contents: &str) -> bool {
    let facts = read::prj_facts(contents);
    let tagged = facts.line.as_deref().is_some_and(|line| {
        line.contains("#prj_is_blocked") || line.contains("[?]")
    });
    tagged || !facts.depends_on.is_empty()
}

fn catalog_syncs(view: &View<'_>, path: &Path) -> Vec<String> {
    let mut kinds = std::collections::BTreeSet::new();
    push_catalog_kind(&mut kinds, view, path);
    for child in view.descendants(path) {
        push_catalog_kind(&mut kinds, view, &child);
    }
    kinds.into_iter().collect()
}

fn push_catalog_kind(
    kinds: &mut std::collections::BTreeSet<String>,
    view: &View<'_>,
    path: &Path,
) {
    let Some(file) = view.snapshot.files.get(path) else {
        return;
    };
    let name = match file.lite.kind {
        LiteKind::Area => "areas",
        LiteKind::Project { .. } => "projects",
        LiteKind::Album => "albums",
        LiteKind::Notebook => "notebooks",
        _ => return,
    };
    kinds.insert(name.to_owned());
}
