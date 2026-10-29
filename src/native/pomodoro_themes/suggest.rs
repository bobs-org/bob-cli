//! Pure task-theme suggestion resolver over a [`HomeSource`].

use std::collections::BTreeSet;
use std::path::Path;

use serde::Serialize;

use super::{Catalog, ThemeRef};
use crate::native::{
    collect_done::trailing_block_id_in_line,
    config::PlanConfig,
    plan_budget::normalize_component,
    pomodoro_homes::{HomeResolver, HomeSource, Hue, NoteKind, NoteTheme},
    ref_tasks::{has_ref_token, strip_blockquote_prefix, task_mark},
    task_root::{parse_task_root, TaskRootState},
    task_status_hooks::canonical_daily_date,
    task_theme::{parse_task_theme, RepairKind, TaskThemeState},
    vault_links::LinkResolution,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum SuggestionSource {
    Task,
    Reading,
    Note,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct NoteOrigin {
    pub(crate) path: String,
    pub(crate) stem: String,
    pub(crate) kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) hue: Option<Hue>,
    pub(crate) chain: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct ThemeSuggestion {
    pub(crate) theme: ThemeRef,
    pub(crate) source: SuggestionSource,
    pub(crate) label: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) from: Option<NoteOrigin>,
    #[serde(
        skip_serializing_if = "Option::is_none",
        serialize_with = "serialize_inherited"
    )]
    pub(crate) inherited: Option<Option<Box<ThemeSuggestion>>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) repair: Option<RepairKind>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct SuggestWarning {
    pub(crate) code: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) path: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SuggestResult {
    pub(crate) suggestion: Option<ThemeSuggestion>,
    pub(crate) warnings: Vec<SuggestWarning>,
}

fn serialize_inherited<S>(
    value: &Option<Option<Box<ThemeSuggestion>>>,
    serializer: S,
) -> Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    match value {
        None => serializer.serialize_none(),
        Some(None) => serializer.serialize_none(),
        Some(Some(inner)) => inner.serialize(serializer),
    }
}

pub(crate) fn suggest_for_task<S: HomeSource>(
    source: &mut HomeResolver<S>,
    catalog: &Catalog,
    cfg: &PlanConfig,
    residence_path: &Path,
    line: &str,
) -> SuggestResult {
    if is_closed_task(line) {
        return SuggestResult {
            suggestion: None,
            warnings: vec![SuggestWarning {
                code: "closed_task".to_string(),
                path: None,
            }],
        };
    }
    let field = parse_task_theme(line);
    let inherited =
        resolve_without_task_field(source, catalog, cfg, residence_path, line);
    match field {
        TaskThemeState::Set { value, key } => {
            let name = catalog.display_name(&key, &value);
            SuggestResult {
                suggestion: Some(ThemeSuggestion {
                    theme: catalog.theme_ref(&key, &name),
                    source: SuggestionSource::Task,
                    label: "this task".to_string(),
                    from: None,
                    inherited: Some(inherited.suggestion.map(|suggestion| {
                        Box::new(without_inherited(suggestion))
                    })),
                    repair: None,
                }),
                warnings: inherited.warnings,
            }
        }
        TaskThemeState::Repair { kind } => {
            let mut result = inherited;
            if let Some(suggestion) = result.suggestion.as_mut() {
                suggestion.repair = Some(kind);
            }
            result
        }
        TaskThemeState::None => inherited,
    }
}

pub(crate) fn suggest_for_new_task<S: HomeSource>(
    source: &mut HomeResolver<S>,
    catalog: &Catalog,
    _cfg: &PlanConfig,
    residence_path: &Path,
) -> SuggestResult {
    note_default(source, catalog, residence_path, false)
}

fn resolve_without_task_field<S: HomeSource>(
    source: &mut HomeResolver<S>,
    catalog: &Catalog,
    cfg: &PlanConfig,
    residence_path: &Path,
    line: &str,
) -> SuggestResult {
    if is_reading_task(line)
        && let Some(name) = cfg.reading_theme()
    {
        let key = normalize_component(name);
        let display = catalog.display_name(&key, name);
        return SuggestResult {
            suggestion: Some(ThemeSuggestion {
                theme: catalog.theme_ref(&key, &display),
                source: SuggestionSource::Reading,
                label: "reading task".to_string(),
                from: None,
                inherited: None,
                repair: None,
            }),
            warnings: Vec::new(),
        };
    }
    if let Some(root_path) = set_root_walk_start(source, residence_path, line) {
        return note_default(source, catalog, &root_path, true);
    }
    note_default(source, catalog, residence_path, false)
}

fn set_root_walk_start<S: HomeSource>(
    source: &mut HomeResolver<S>,
    residence_path: &Path,
    line: &str,
) -> Option<std::path::PathBuf> {
    let TaskRootState::Set { raw } = parse_task_root(line) else {
        return None;
    };
    match source.resolve_target(residence_path, &raw) {
        LinkResolution::Found(path) => {
            let home = source.resolve_note(&path);
            (home.key != "@unfiled").then_some(path)
        }
        LinkResolution::Ambiguous | LinkResolution::Missing => None,
    }
}

fn note_default<S: HomeSource>(
    source: &mut HomeResolver<S>,
    catalog: &Catalog,
    residence_path: &Path,
    from_root_field: bool,
) -> SuggestResult {
    if !from_root_field && canonical_daily_date(residence_path).is_some() {
        return SuggestResult {
            suggestion: None,
            warnings: Vec::new(),
        };
    }
    let home = source.resolve_note(residence_path);
    let mut warnings = Vec::new();
    let mut seen_invalid = BTreeSet::new();
    for (index, path_str) in home.residence_chain.iter().enumerate() {
        let path = Path::new(path_str);
        let Some(facts) = source.facts(path) else {
            continue;
        };
        if !matches!(facts.kind, NoteKind::Area | NoteKind::Project) {
            continue;
        }
        match facts.theme {
            None => continue,
            Some(NoteTheme::Off) => {
                return SuggestResult {
                    suggestion: None,
                    warnings,
                };
            }
            Some(NoteTheme::Invalid(_)) => {
                if seen_invalid.insert(path_str.clone()) {
                    warnings.push(SuggestWarning {
                        code: "theme_default_invalid".to_string(),
                        path: Some(path_str.clone()),
                    });
                }
            }
            Some(NoteTheme::Name(canonical)) => {
                let key = normalize_component(&canonical);
                let display = catalog.display_name(&key, &canonical);
                let declaring = source.resolve_note(path);
                let kind = match facts.kind {
                    NoteKind::Area => "area",
                    NoteKind::Project => "project",
                    NoteKind::Other => unreachable!("filtered above"),
                };
                let stem = note_stem(path);
                return SuggestResult {
                    suggestion: Some(ThemeSuggestion {
                        theme: catalog.theme_ref(&key, &display),
                        source: SuggestionSource::Note,
                        label: format!("from {stem}"),
                        from: Some(NoteOrigin {
                            path: path_str.clone(),
                            stem,
                            kind: kind.to_string(),
                            hue: declaring.hue,
                            chain: home.residence_chain[..=index].to_vec(),
                        }),
                        inherited: None,
                        repair: None,
                    }),
                    warnings,
                };
            }
        }
    }
    SuggestResult {
        suggestion: None,
        warnings,
    }
}

fn without_inherited(mut suggestion: ThemeSuggestion) -> ThemeSuggestion {
    suggestion.inherited = None;
    suggestion
}

fn is_closed_task(line: &str) -> bool {
    let stripped = strip_blockquote_prefix(line);
    matches!(task_mark(stripped), Some('x' | 'X' | '-'))
}

fn is_reading_task(line: &str) -> bool {
    if has_ref_token(line) {
        return true;
    }
    trailing_block_id_in_line(line).is_some_and(|id| {
        let lower = id.to_ascii_lowercase();
        lower == "ref" || lower.starts_with("ref-")
    })
}

fn note_stem(path: &Path) -> String {
    path.file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or("")
        .to_lowercase()
}
