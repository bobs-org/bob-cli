//! Capture-only integration of deliberate Pomodoro names with task themes.

use super::*;
use crate::native::{
    pomodoro_homes::{HomeResolver, HomeSource, NoteFacts},
    pomodoro_themes::{self, suggest::ThemeSuggestion, Catalog, ThemeRef},
    task_theme::{
        clear_task_theme, parse_task_theme, plan_theme_change, set_task_theme,
        TaskThemeState, ThemeChange,
    },
    vault_links::{target_to_markdown_path, LinkResolution, VaultLinkResolver},
};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
};

pub(super) struct CaptureTaskThemeContext {
    root: PathBuf,
    today: NaiveDate,
    config_path: PathBuf,
    config_loaded: bool,
    cfg: Option<config::PlanConfig>,
    catalog_loaded: bool,
    catalog: Option<Catalog>,
    resolver: HomeResolver<CaptureHomeSource>,
    warned_suggestions: BTreeSet<(String, Option<String>)>,
}

pub(super) struct AppliedTaskTheme {
    pub(super) task_line: String,
    pub(super) summary: CaptureTaskThemeChange,
}

impl CaptureTaskThemeContext {
    pub(super) fn new(
        root: &Path,
        today: NaiveDate,
        config_path: Option<&Path>,
    ) -> Self {
        Self {
            root: root.to_path_buf(),
            today,
            config_path: config_path
                .map(Path::to_path_buf)
                .unwrap_or_else(config::config_path),
            config_loaded: false,
            cfg: None,
            catalog_loaded: false,
            catalog: None,
            resolver: HomeResolver::new(CaptureHomeSource::new(root)),
            warned_suggestions: BTreeSet::new(),
        }
    }

    pub(super) fn apply_new_task(
        &mut self,
        planner: &mut CaptureBatchPlanner,
        target: &Path,
        route: &str,
        block_id: &str,
        destination_name: Option<&str>,
        warnings: &mut Vec<CaptureWarning>,
    ) -> Result<Option<AppliedTaskTheme>, CaptureError> {
        let Some(destination_name) = destination_name else {
            return Ok(None);
        };
        let current = planner.read_existing(target)?;
        let settings = note_tasks::read_settings(&self.root);
        let scan = note_tasks::scan(&current, &settings);
        let task = match scan.by_block_id(block_id) {
            BlockIdLookup::Found(task) => task,
            _ => {
                return Err(CaptureError::io(
                    "task theme capture invariant failed: new task disappeared",
                ));
            }
        };
        let source_line = line_text_at(&current, task.line_index)?.to_string();
        self.apply(
            planner,
            target,
            route,
            block_id,
            &source_line,
            destination_name,
            true,
            warnings,
        )
    }

    pub(super) fn apply_existing_task(
        &mut self,
        planner: &mut CaptureBatchPlanner,
        target: &Path,
        route: &str,
        block_id: &str,
        previous_task_line: &str,
        destination_name: Option<&str>,
        warnings: &mut Vec<CaptureWarning>,
    ) -> Result<Option<AppliedTaskTheme>, CaptureError> {
        let Some(destination_name) = destination_name else {
            return Ok(None);
        };
        self.apply(
            planner,
            target,
            route,
            block_id,
            previous_task_line,
            destination_name,
            false,
            warnings,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn apply(
        &mut self,
        planner: &mut CaptureBatchPlanner,
        target: &Path,
        route: &str,
        block_id: &str,
        source_line: &str,
        destination_name: &str,
        new_task: bool,
        warnings: &mut Vec<CaptureWarning>,
    ) -> Result<Option<AppliedTaskTheme>, CaptureError> {
        self.refresh_residence_snapshot(planner);
        if !self.load_config(warnings) || !self.load_catalog(warnings) {
            return Ok(None);
        }
        let cfg = self.cfg.as_ref().expect("loaded plan config");
        let catalog = self.catalog.as_ref().expect("loaded theme catalog");
        let residence_path = relative_target(Some(route));
        let residence = Path::new(&residence_path);
        let suggested = if new_task {
            pomodoro_themes::suggest::suggest_for_new_task(
                &mut self.resolver,
                catalog,
                cfg,
                residence,
            )
        } else {
            pomodoro_themes::suggest::suggest_for_task(
                &mut self.resolver,
                catalog,
                cfg,
                residence,
                source_line,
            )
        };
        for warning in suggested.warnings {
            let identity = (warning.code.clone(), warning.path.clone());
            if self.warned_suggestions.insert(identity) {
                warnings.push(CaptureWarning::Suggestion(warning));
            }
        }

        let field = parse_task_theme(source_line);
        let inherited = suggestion_inherited(suggested.suggestion.as_ref());
        let shown_key = suggested
            .suggestion
            .as_ref()
            .map(|item| item.theme.key.as_str());
        let inherited_key =
            inherited.as_ref().map(|item| item.theme.key.as_str());
        let exempt_keys = cfg
            .exempt()
            .iter()
            .map(|name| plan_budget::normalize_component(name))
            .collect::<BTreeSet<_>>();
        let change = plan_theme_change(
            &field,
            shown_key,
            inherited_key,
            destination_name,
            &exempt_keys,
            true,
        );
        if change == ThemeChange::None {
            return Ok(None);
        }

        let previous = valid_previous_theme(&field, catalog);
        let (action, theme, task_line) = match &change {
            ThemeChange::Set { key, name } => {
                let display = catalog.display_name(key, name);
                let theme = catalog.theme_ref(key, &display);
                let task_line =
                    self.write_task_field(planner, target, block_id, |line| {
                        set_task_theme(line, &display)
                    })?;
                ("set", Some(theme), task_line)
            }
            ThemeChange::Clear => {
                let task_line = self.write_task_field(
                    planner,
                    target,
                    block_id,
                    clear_task_theme,
                )?;
                ("cleared", None, task_line)
            }
            ThemeChange::None => unreachable!("returned above"),
        };
        let summary = CaptureTaskThemeChange {
            action,
            theme,
            previous,
            inherited,
        };
        Ok(Some(AppliedTaskTheme { task_line, summary }))
    }

    fn write_task_field(
        &self,
        planner: &mut CaptureBatchPlanner,
        target: &Path,
        block_id: &str,
        update: impl FnOnce(&str) -> String,
    ) -> Result<String, CaptureError> {
        let current = planner.read_existing(target)?;
        let settings = note_tasks::read_settings(&self.root);
        let scan = note_tasks::scan(&current, &settings);
        let task = match scan.by_block_id(block_id) {
            BlockIdLookup::Found(task) => task,
            _ => {
                return Err(CaptureError::io(
                    "task theme capture invariant failed: task disappeared before field write",
                ));
            }
        };
        let spans = line_spans(&current);
        let span = spans.get(task.line_index).ok_or_else(|| {
            CaptureError::io(
                "task theme capture invariant failed: task line is out of range",
            )
        })?;
        let start = task
            .line_index
            .checked_sub(1)
            .map_or(0, |previous| spans[previous].end);
        let segment = &current[start..span.end];
        let updated_segment = update(segment);
        let mut updated = String::with_capacity(
            current.len() + updated_segment.len().saturating_sub(segment.len()),
        );
        updated.push_str(&current[..start]);
        updated.push_str(&updated_segment);
        updated.push_str(&current[span.end..]);
        planner.stage(target, updated.clone())?;
        line_text_at(&updated, task.line_index).map(str::to_string)
    }

    fn refresh_residence_snapshot(&mut self, planner: &CaptureBatchPlanner) {
        let snapshot = planner
            .staged_snapshot()
            .into_iter()
            .filter_map(|(absolute, contents)| {
                absolute
                    .strip_prefix(&self.root)
                    .ok()
                    .map(|relative| (relative.to_path_buf(), contents))
            })
            .collect();
        self.resolver.source_mut().staged = snapshot;
        self.resolver.refresh();
    }

    fn load_config(&mut self, warnings: &mut Vec<CaptureWarning>) -> bool {
        if !self.config_loaded {
            self.config_loaded = true;
            match config::load_plan_config(&self.config_path) {
                Ok(cfg) => self.cfg = Some(cfg),
                Err(error) => warnings.push(CaptureWarning::Text(format!(
                    "read plan config: {error}"
                ))),
            }
        }
        self.cfg.is_some()
    }

    fn load_catalog(&mut self, warnings: &mut Vec<CaptureWarning>) -> bool {
        if !self.catalog_loaded {
            self.catalog_loaded = true;
            match Catalog::load(&self.root, self.today, false) {
                Ok(catalog) => {
                    for warning in catalog.warnings() {
                        if !warnings
                            .iter()
                            .any(|existing| existing.message() == *warning)
                        {
                            warnings
                                .push(CaptureWarning::Text(warning.clone()));
                        }
                    }
                    self.catalog = Some(catalog);
                }
                Err(error) => warnings.push(CaptureWarning::Text(format!(
                    "read theme catalog: {error}"
                ))),
            }
        }
        self.catalog.is_some()
    }
}

struct CaptureHomeSource {
    root: PathBuf,
    links: VaultLinkResolver,
    staged: BTreeMap<PathBuf, Option<String>>,
}

impl CaptureHomeSource {
    fn new(root: &Path) -> Self {
        Self {
            root: root.to_path_buf(),
            links: VaultLinkResolver::new(root),
            staged: BTreeMap::new(),
        }
    }
}

impl HomeSource for CaptureHomeSource {
    fn resolve(&self, _current: &Path, target: &str) -> LinkResolution {
        let Some(candidate) = target_to_markdown_path(target) else {
            return LinkResolution::Missing;
        };
        if let Some(contents) = self.staged.get(&candidate) {
            return if contents.is_some() {
                LinkResolution::Found(candidate)
            } else {
                LinkResolution::Missing
            };
        }
        if self.root.join(&candidate).is_file() {
            return LinkResolution::Found(candidate);
        }

        let mut candidates = self
            .links
            .candidates(target)
            .into_iter()
            .filter(|path| !self.staged.get(path).is_some_and(Option::is_none))
            .collect::<BTreeSet<_>>();
        let wanted = candidate
            .file_stem()
            .and_then(|stem| stem.to_str())
            .unwrap_or_default()
            .to_lowercase();
        for (path, contents) in &self.staged {
            if contents.is_some()
                && path
                    .file_stem()
                    .and_then(|stem| stem.to_str())
                    .is_some_and(|stem| stem.eq_ignore_ascii_case(&wanted))
            {
                candidates.insert(path.clone());
            }
        }
        let candidates = candidates.into_iter().collect::<Vec<_>>();
        match candidates.as_slice() {
            [] => LinkResolution::Missing,
            [path] => LinkResolution::Found(path.clone()),
            _ => LinkResolution::Ambiguous,
        }
    }

    fn read_facts(&self, path: &Path) -> Option<NoteFacts> {
        if let Some(contents) = self.staged.get(path) {
            return contents.as_deref().map(NoteFacts::parse);
        }
        let contents = fs::read_to_string(self.root.join(path)).ok()?;
        Some(NoteFacts::parse(&contents))
    }
}

fn suggestion_inherited(
    suggestion: Option<&ThemeSuggestion>,
) -> Option<ThemeSuggestion> {
    let suggestion = suggestion?;
    let inherited = match suggestion.source {
        pomodoro_themes::suggest::SuggestionSource::Task => suggestion
            .inherited
            .as_ref()
            .and_then(|inherited| inherited.as_ref())
            .map(|inherited| (**inherited).clone()),
        _ => Some(suggestion.clone()),
    }?;
    Some(without_nested_inheritance(inherited))
}

fn without_nested_inheritance(
    mut suggestion: ThemeSuggestion,
) -> ThemeSuggestion {
    suggestion.inherited = None;
    suggestion
}

fn valid_previous_theme(
    field: &TaskThemeState,
    catalog: &Catalog,
) -> Option<ThemeRef> {
    match field {
        TaskThemeState::Set { value, key } => {
            let display = catalog.display_name(key, value);
            Some(catalog.theme_ref(key, &display))
        }
        TaskThemeState::None | TaskThemeState::Repair { .. } => None,
    }
}
