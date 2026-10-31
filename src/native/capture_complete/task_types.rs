//! Read-only task-type candidates for capture's `#<type>` marker.

use std::{collections::BTreeSet, path::Path};

use super::{
    model::{CompleteError, TaskTypeCandidate, TaskTypeEffect},
    pomodoros::resolve_task_suggestion,
    support::bounded_warning,
};
use crate::native::{
    capture,
    capture_language::{
        replacement_for, resolve_accepted_type_name, selector_slug,
    },
    capture_language::{EditorItemParse, SpanKind},
    env,
    plan_budget::normalize_component,
    pomodoro_themes::{self, preselect::MenuSuggestion, Catalog, ThemeRef},
    task_fields::canonical_theme_name,
};

pub(super) struct TaskTypeContext {
    pub(super) candidates: Vec<TaskTypeCandidate>,
    pub(super) subject: &'static str,
    pub(super) suggestion: Option<MenuSuggestion>,
    pub(super) warnings: Vec<String>,
}

#[derive(Debug, Clone)]
struct TypeRow {
    name: String,
    key: String,
    origin: &'static str,
    last_used: Option<String>,
    days_used: Option<u32>,
    hidden: bool,
}

pub(super) fn build(
    bob_dir: &Path,
    raw_text: &str,
    field: &crate::native::capture_language::CompletionField,
    editor_item: Option<&EditorItemParse>,
) -> Result<TaskTypeContext, CompleteError> {
    build_inner(bob_dir, raw_text, field, editor_item, false)
}

pub(super) fn build_for_shell(
    bob_dir: &Path,
    raw_text: &str,
    field: &crate::native::capture_language::CompletionField,
    editor_item: Option<&EditorItemParse>,
) -> Result<TaskTypeContext, CompleteError> {
    build_inner(bob_dir, raw_text, field, editor_item, true)
}

fn build_inner(
    bob_dir: &Path,
    raw_text: &str,
    field: &crate::native::capture_language::CompletionField,
    editor_item: Option<&EditorItemParse>,
    include_all_visible: bool,
) -> Result<TaskTypeContext, CompleteError> {
    let today = env::current_datetime().date();
    let mut extra_keys = BTreeSet::new();
    if !field.query.is_empty() {
        let key = normalize_component(&field.query);
        if !key.is_empty() {
            extra_keys.insert(key);
        }
    }
    let catalog =
        Catalog::load_for_completion(bob_dir, today, extra_keys, None)
            .map_err(|error| CompleteError::io(error.to_string()))?;
    let mut warnings = catalog
        .warnings()
        .iter()
        .cloned()
        .map(bounded_warning)
        .collect::<Vec<_>>();

    let existing = editor_item
        .and_then(|item| item.dependency_target.as_ref())
        .filter(|target| {
            target.kind
                == crate::native::capture_language::DependencyTargetKind::ExistingTask
        });
    // A solo whole-item link is itself the task target. The editor parser
    // reserves `dependency_target` for relationship owners, so also read the
    // route and block id from the existing-link form here. An empty `#` on
    // that link makes the editor mode incomplete while it reports the need.
    let has_new_task_id = editor_item.is_some_and(|item| {
        item.spans
            .iter()
            .any(|span| span.kind == SpanKind::TaskBlockId)
    });
    let whole_item_link = editor_item.filter(|item| {
        matches!(
            item.mode,
            crate::native::capture_language::EditorMode::PomodoroLink
                | crate::native::capture_language::EditorMode::Incomplete
        ) && item
            .spans
            .iter()
            .any(|span| span.kind == SpanKind::PomodoroRoute)
            && item
                .spans
                .iter()
                .any(|span| span.kind == SpanKind::PomodoroBlockId)
    });
    let whole_item_toggle = editor_item.filter(|item| {
        item.spans
            .iter()
            .any(|span| span.kind == SpanKind::TaskToggleBlockId)
    });
    let subject = if !has_new_task_id
        && (existing.is_some()
            || whole_item_link.is_some()
            || whole_item_toggle.is_some())
    {
        "existing"
    } else {
        "new"
    };
    let route: &str = existing
        .and_then(|target| target.route.as_deref())
        .or_else(|| whole_item_link.and_then(|item| item.route.as_deref()))
        .or_else(|| whole_item_toggle.and_then(|item| item.route.as_deref()))
        .or_else(|| editor_item.and_then(|item| item.route.as_deref()))
        .unwrap_or(capture::inbox_route());
    let block_id = existing
        .and_then(|target| target.block_id.as_deref())
        .or_else(|| whole_item_link.and_then(|item| item.block_id.as_deref()))
        .or_else(|| {
            whole_item_toggle.and_then(|item| item.block_id.as_deref())
        });
    let suggestion = resolve_task_suggestion(
        bob_dir,
        route,
        block_id,
        &catalog,
        &mut warnings,
    );
    let inherited = suggestion.as_ref().and_then(|suggestion| {
        if subject == "existing"
            && suggestion.source
                == pomodoro_themes::suggest::SuggestionSource::Task
        {
            suggestion
                .inherited
                .as_ref()
                .and_then(|inherited| inherited.as_deref())
        } else {
            Some(suggestion)
        }
    });
    let current_key = suggestion
        .as_ref()
        .map(|suggestion| suggestion.theme.key.as_str());
    let inherited_key =
        inherited.map(|suggestion| suggestion.theme.key.as_str());
    let inherited_label = inherited.map(|suggestion| suggestion.label.clone());
    let has_current_field = subject == "existing"
        && suggestion.as_ref().is_some_and(|suggestion| {
            suggestion.source
                == pomodoro_themes::suggest::SuggestionSource::Task
        });
    let leading = crate::native::capture_language::task_type_completion_at(
        raw_text,
        field.replacement.1,
    )
    .is_some_and(|marker| marker.leading);

    let rows = rows_in_empty_order(&catalog, &field.query);
    let mut matched = Vec::<(u8, usize, TypeRow)>::new();
    for (order, row) in rows.into_iter().enumerate() {
        let rank = match_rank(&row.name, &row.key, &field.query);
        if field.query.is_empty() {
            matched.push((0, order, row));
        } else if let Some(rank) = rank
            && (!row.hidden || rank == 0)
        {
            matched.push((rank, order, row));
        } else if include_all_visible && !row.hidden {
            matched.push((3, order, row));
        }
    }
    matched.sort_by_key(|(rank, order, _)| (*rank, *order));
    if field.query.is_empty() && catalog.configured_themes().is_none() {
        matched.truncate(12);
    }
    let exact_match = matched.iter().any(|(rank, _, _)| *rank == 0);

    let mut candidates = matched
        .into_iter()
        .map(|(_, _, row)| {
            candidate(
                row.name,
                row.key,
                row.origin,
                row.last_used,
                row.days_used,
                row.hidden,
                Vec::new(),
                None,
                &catalog,
                field,
                raw_text,
                leading,
                subject,
                current_key,
                inherited_key,
                inherited_label.as_deref(),
                has_current_field,
            )
        })
        .collect::<Vec<_>>();

    if !field.query.is_empty()
        && !exact_match
        && let Some(name) = canonical_theme_name(&field.query)
    {
        let key = normalize_component(&name);
        let mut theme = catalog.theme_ref(&key, &name);
        if catalog.configured_themes().is_some()
            && catalog.configured(&key) != Some(true)
        {
            theme.configured = Some(false);
            theme.known = false;
        }
        let similar_to =
            catalog.similar_to(&name).into_iter().take(2).collect();
        candidates.push(candidate(
            name,
            key,
            "typed",
            None,
            None,
            false,
            similar_to,
            Some(theme),
            &catalog,
            field,
            raw_text,
            leading,
            subject,
            current_key,
            inherited_key,
            inherited_label.as_deref(),
            has_current_field,
        ));
    }

    let suggestion = suggestion.map(|suggestion| MenuSuggestion {
        suggestion,
        fallback: None,
        create: false,
    });
    Ok(TaskTypeContext {
        candidates,
        subject,
        suggestion,
        warnings,
    })
}

fn rows_in_empty_order(catalog: &Catalog, query: &str) -> Vec<TypeRow> {
    let mut rows = if let Some(configured) = catalog.configured_themes() {
        configured
            .iter()
            .map(|entry| TypeRow {
                name: entry.name.clone(),
                key: entry.key.clone(),
                origin: "configured",
                last_used: catalog
                    .get(&entry.key)
                    .map(|record| record.last_used.to_string()),
                days_used: catalog
                    .get(&entry.key)
                    .map(|record| record.days_used),
                hidden: catalog.is_hidden(&entry.key),
            })
            .collect::<Vec<_>>()
    } else {
        catalog
            .rank_for_menu("", &BTreeSet::new())
            .into_iter()
            .map(|record| {
                let key = normalize_component(&record.name);
                let hidden = catalog.is_hidden(&key);
                TypeRow {
                    key,
                    name: record.name,
                    origin: "history",
                    last_used: record.last_used.map(|date| date.to_string()),
                    days_used: record.days_used,
                    hidden,
                }
            })
            .collect()
    };
    if catalog.configured_themes().is_none() {
        rows.retain(|row| !catalog.is_inventory(&row.key));
    }
    if query.is_empty() {
        rows.retain(|row| !row.hidden);
    } else {
        // Hidden rows remain eligible for exact lookup, as hiding affects
        // suggestions only and never typed use.
        rows.retain(|row| {
            !row.hidden || match_rank(&row.name, &row.key, query) == Some(0)
        });
    }
    rows
}

fn match_rank(name: &str, key: &str, query: &str) -> Option<u8> {
    if query.is_empty() {
        return Some(0);
    }
    let query_slug = selector_slug(query).to_ascii_lowercase();
    let query_key = normalize_component(query).to_ascii_lowercase();
    if query_slug.is_empty() && query_key.is_empty() {
        return None;
    }
    let name_slug = selector_slug(name).to_ascii_lowercase();
    let key = key.to_ascii_lowercase();
    if name_slug == query_slug || key == query_key {
        Some(0)
    } else if name_slug.starts_with(&query_slug) || key.starts_with(&query_key)
    {
        Some(1)
    } else if name_slug.contains(&query_slug) || key.contains(&query_key) {
        Some(2)
    } else {
        None
    }
}

#[allow(clippy::too_many_arguments)]
fn candidate(
    name: String,
    key: String,
    origin: &'static str,
    last_used: Option<String>,
    days_used: Option<u32>,
    hidden: bool,
    similar_to: Vec<String>,
    typed_theme: Option<ThemeRef>,
    catalog: &Catalog,
    field: &crate::native::capture_language::CompletionField,
    raw_text: &str,
    leading: bool,
    subject: &'static str,
    current_key: Option<&str>,
    inherited_key: Option<&str>,
    inherited_label: Option<&str>,
    has_current_field: bool,
) -> TaskTypeCandidate {
    let key = normalize_component(&key);
    let current = current_key == Some(key.as_str());
    let inherited = inherited_key == Some(key.as_str());
    let effect = if inherited && has_current_field {
        // The selected type matches inheritance, so the task-field
        // exception should be absent (or be cleared on an existing task).
        TaskTypeEffect::Clear
    } else if inherited {
        // A new task, or an existing task without an explicit exception,
        // already gets this type through inheritance.
        TaskTypeEffect::Keep
    } else if subject == "existing" && has_current_field && current {
        TaskTypeEffect::Keep
    } else {
        TaskTypeEffect::Set
    };
    let mut replacement = replacement_for(&name, &key);
    let next_is_whitespace = raw_text
        .get(field.replacement.1..)
        .and_then(|rest| rest.chars().next())
        .is_some_and(char::is_whitespace);
    if leading && !next_is_whitespace {
        replacement.push(' ');
    }
    let themes =
        vec![typed_theme.unwrap_or_else(|| catalog.theme_ref(&key, &name))];
    let refused = resolve_accepted_type_name(catalog, &name, false)
        .is_none()
        .then_some("type_not_configured");
    TaskTypeCandidate {
        cursor_after: field.replacement.0 + replacement.len(),
        replacement,
        name,
        key,
        themes,
        origin,
        last_used,
        days_used,
        current,
        inherited,
        suggestion_label: inherited
            .then(|| inherited_label.map(str::to_string))
            .flatten(),
        effect,
        hidden,
        similar_to,
        refused,
    }
}
