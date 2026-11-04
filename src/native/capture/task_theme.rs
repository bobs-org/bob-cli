//! Capture-only integration of deliberate Pomodoro names with task types.

use super::*;
use crate::native::{
    capture_language::{
        replacement_for, resolve_accepted_type_name, unmatched_type_display,
        TypeMarker,
    },
    config::themes::ThemeSources,
    plan_budget::{normalize_component, split_components},
    pomodoro_homes::{HomeResolver, HomeSource, NoteFacts},
    pomodoro_themes::{
        self, routing,
        suggest::{self, ThemeSuggestion},
        Catalog, ThemeRef,
    },
    task_fields::canonical_theme_name,
    task_theme::{
        clear_task_theme, parse_task_theme, set_task_theme, TaskThemeState,
        ThemeChange,
    },
    vault_links::{target_to_markdown_path, LinkResolution, VaultLinkResolver},
};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
};

pub(crate) struct CaptureTaskThemeContext {
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

/// Pre-close type inputs for R6 close names: the theme vocabulary, each
/// Task Link's effective type, and the running name's refs for JSON.
pub(super) struct CloseTypes {
    pub(super) naming: capture_pomodoro_close::CloseNaming,
    /// Refs for the running entry's name components, so the close JSON can
    /// report the shed types with their hues.
    pub(super) refs: Vec<ThemeRef>,
}

pub(super) struct AppliedTaskTheme {
    pub(super) task_line: String,
    pub(super) summary: CaptureTaskThemeChange,
}

/// One R2 merge staged on the destination entry: the task kept its type
/// and the Pomodoro gained it. `dest_index` is the 0-based headline index
/// of the renamed entry (rename is in-place, so before and after share it).
pub(super) struct AppliedMerge {
    pub(super) before: String,
    pub(super) after: String,
    pub(super) added: ThemeRef,
    pub(super) dest_index: usize,
}

/// Result of landing a task in a named Pomodoro (R2): an optional task
/// field edit plus an optional destination merge. A merge never writes a
/// task field, and a MEET retype never merges.
pub(super) struct AppliedLanding {
    pub(super) task_line: String,
    pub(super) theme: Option<CaptureTaskThemeChange>,
    pub(super) merge: Option<AppliedMerge>,
}

pub(super) struct ExistingTypeMarkerApplication {
    pub(super) summary: CaptureTaskThemeChange,
    pub(super) type_key: String,
    pub(super) type_name: String,
}

/// D8 relocation plus the identity the capture tracker needs to report
/// truthful before/after Pomodoro blocks.
pub(super) struct AppliedRelocation {
    pub(super) json: super::output::TypeRelocationJson,
    pub(super) dest_after: usize,
    pub(super) dest_created: bool,
}

/// Owned D7 typed-placement inputs for one bare link-creating capture:
/// the task's effective type (D3, from the staged line).
pub(crate) struct PlacementType {
    pub(crate) suggestion: ThemeSuggestion,
    pub(super) exempt: Vec<String>,
}

impl PlacementType {
    /// Borrowed inputs for the pure `capture_task_toggle` planners.
    pub(super) fn as_typed(
        &self,
    ) -> crate::native::capture_task_toggle::TypedImplicit<'_> {
        crate::native::capture_task_toggle::TypedImplicit {
            type_key: &self.suggestion.theme.key,
            type_name: &self.suggestion.theme.name,
            exempt: &self.exempt,
        }
    }

    /// Additive `type` record for link-destination JSON.
    pub(super) fn placed_type(&self) -> super::batch::PlacedType {
        super::batch::PlacedType {
            theme: self.suggestion.theme.clone(),
            source: self.suggestion.source,
            label: self.suggestion.label.clone(),
        }
    }
}

impl CaptureTaskThemeContext {
    pub(crate) fn new(
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
        selector: Option<&str>,
        warnings: &mut Vec<CaptureWarning>,
    ) -> Result<Option<AppliedLanding>, CaptureError> {
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
            selector,
            warnings,
        )
    }

    /// Stage an explicit `#<type>` marker on a new-task formatted line before
    /// placement. Resolves X via the shared helper, computes I from the
    /// staged line in its destination, applies the field table, and
    /// enforces named-Pomodoro agreement. Returns the edited line plus
    /// the always-reported summary. Unknown names are refused before staging.
    pub(super) fn apply_type_marker(
        &mut self,
        planner: &mut CaptureBatchPlanner,
        target: &Path,
        task_line: &str,
        marker: &TypeMarker,
        warnings: &mut Vec<CaptureWarning>,
    ) -> Result<AppliedTaskTheme, CaptureError> {
        self.refresh_residence_snapshot(planner);
        if !self.load_catalog(warnings) {
            return Err(CaptureError::io(
                "could not load the task-type catalog to validate `#<type>`",
            ));
        }
        let found = {
            let catalog = self.catalog.as_ref().expect("loaded theme catalog");
            let Some(found) = resolve_accepted_type_name(
                catalog,
                &marker.name,
                marker.literal,
            ) else {
                return Err(self.type_not_configured_error(marker, catalog));
            };
            found
        };
        if !self.load_config(warnings) {
            return Err(CaptureError::io(format!(
                "read plan config {} to apply task type",
                self.config_path.display()
            )));
        }
        let cfg = self.cfg.as_ref().expect("loaded plan config");
        let catalog = self.catalog.as_ref().expect("loaded theme catalog");
        let (x_key, x_name) = (found.key, found.name);
        // Compute I from the staged line in its destination, skipping its
        // own type field (the line has no field yet for a new task).
        let residence = target.strip_prefix(&self.root).unwrap_or(target);
        let suggested = suggest::suggest_for_task(
            &mut self.resolver,
            catalog,
            cfg,
            residence,
            task_line,
        );
        for warning in suggested.warnings {
            let identity = (warning.code.clone(), warning.path.clone());
            if self.warned_suggestions.insert(identity) {
                warnings.push(CaptureWarning::Suggestion(warning));
            }
        }
        let inherited = suggestion_inherited(suggested.suggestion.as_ref());
        let inherited_key =
            inherited.as_ref().map(|item| item.theme.key.as_str());
        // Field table (1.7): X == I → kept (no field) or cleared (remove
        // authored exception); X != I, F == X → kept; else set. A marker
        // always reports.
        let field = parse_task_theme(task_line);
        let x_eq_i = inherited_key == Some(x_key.as_str());
        let (action, theme, edited_line) = if x_eq_i {
            match field {
                TaskThemeState::None => {
                    // No field, inherits X: kept.
                    let display = catalog.display_name(&x_key, &x_name);
                    let theme_ref = catalog.theme_ref(&x_key, &display);
                    ("kept", Some(theme_ref), task_line.to_string())
                }
                _ => {
                    // Authored exception removed: cleared.
                    let cleared = clear_task_theme(task_line);
                    ("cleared", None, cleared)
                }
            }
        } else {
            // X != I: check if field already equals X.
            let field_is_x = match &field {
                TaskThemeState::Set { key, .. } => key == &x_key,
                _ => false,
            };
            if field_is_x {
                let display = catalog.display_name(&x_key, &x_name);
                let theme_ref = catalog.theme_ref(&x_key, &display);
                ("kept", Some(theme_ref), task_line.to_string())
            } else {
                let display = catalog.display_name(&x_key, &x_name);
                let theme_ref = catalog.theme_ref(&x_key, &display);
                let edited = set_task_theme(task_line, &display);
                ("set", Some(theme_ref), edited)
            }
        };
        let previous = valid_previous_theme(&field, catalog);
        let summary = CaptureTaskThemeChange {
            action,
            theme,
            previous,
            inherited,
        };
        Ok(AppliedTaskTheme {
            task_line: edited_line,
            summary,
        })
    }

    /// Check a marker against the destination selected by the normal ledger
    /// resolver. Call after selection has been staged so prefix selectors are
    /// compared with the complete resolved name.
    /// R2 landing retired the `type_marker_conflict` refusal: a `#<type>`
    /// marker sets X first, then the link lands with the new type and
    /// merges a disagreeing named destination. Always succeeds.
    pub(super) fn validate_type_marker_agreement(
        &self,
        _marker_name: &str,
        _marker_literal: bool,
        _destination: Option<&str>,
    ) -> Result<(), CaptureError> {
        Ok(())
    }

    pub(super) fn apply_existing_task(
        &mut self,
        planner: &mut CaptureBatchPlanner,
        target: &Path,
        route: &str,
        block_id: &str,
        previous_task_line: &str,
        destination_name: Option<&str>,
        selector: Option<&str>,
        warnings: &mut Vec<CaptureWarning>,
    ) -> Result<Option<AppliedLanding>, CaptureError> {
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
            selector,
            warnings,
        )
    }

    /// Explicit `#<type>` retype of an existing task. Reads the staged target
    /// line, resolves X, applies the §1.7 field table, and stages the
    /// edit before the gesture runs. Named-Pomodoro agreement is checked
    /// later against the resolved destination. Closed targets are refused
    /// without a write. Always reports, so the caller can relocate the
    /// live link and emit `task_theme` + relocation.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn apply_type_marker_to_existing(
        &mut self,
        planner: &mut CaptureBatchPlanner,
        target: &Path,
        route: &str,
        block_id: &str,
        marker: &TypeMarker,
        warnings: &mut Vec<CaptureWarning>,
    ) -> Result<ExistingTypeMarkerApplication, CaptureError> {
        self.refresh_residence_snapshot(planner);
        let current_contents = planner.read_existing(target)?;
        let settings = note_tasks::read_settings(&self.root);
        let scan = note_tasks::scan(&current_contents, &settings);
        let task = match scan.by_block_id(block_id) {
            BlockIdLookup::Found(task) => task,
            _ => {
                return Err(CaptureError::io(
                    "task theme capture invariant failed: existing task disappeared before retype",
                ));
            }
        };
        if !crate::native::capture_link_tasks::is_linkable_status(
            task.status_symbol,
        ) {
            let line = line_text_at(&current_contents, task.line_index)
                .unwrap_or_default();
            let description = note_tasks::clean_description(
                &line
                    .find("] ")
                    .map(|index| line[index + 2..].to_string())
                    .unwrap_or_default(),
                "",
                None,
            );
            let _ = description;
            return Err(CaptureError::usage(format!(
                "`{}` needs an open task: ^{block_id} is {}; closed tasks keep their fields as history",
                marker.authored,
                task.status_name
            )));
        }
        let source_line =
            line_text_at(&current_contents, task.line_index)?.to_string();
        if !self.load_catalog(warnings) {
            return Err(CaptureError::io(
                "could not load the task-type catalog to validate `#<type>`",
            ));
        }
        let found = {
            let catalog = self.catalog.as_ref().expect("loaded theme catalog");
            let Some(found) = resolve_accepted_type_name(
                catalog,
                &marker.name,
                marker.literal,
            ) else {
                return Err(self.type_not_configured_error(marker, catalog));
            };
            found
        };
        if !self.load_config(warnings) {
            return Err(CaptureError::io(format!(
                "read plan config {} to apply task type",
                self.config_path.display()
            )));
        }
        let cfg = self.cfg.as_ref().expect("loaded plan config");
        let catalog = self.catalog.as_ref().expect("loaded theme catalog");
        let (x_key, x_name) = (found.key, found.name);
        let residence_path = relative_target(Some(route));
        let residence = Path::new(&residence_path);
        let suggested = suggest::suggest_for_task(
            &mut self.resolver,
            catalog,
            cfg,
            residence,
            &source_line,
        );
        for warning in suggested.warnings {
            let identity = (warning.code.clone(), warning.path.clone());
            if self.warned_suggestions.insert(identity) {
                warnings.push(CaptureWarning::Suggestion(warning));
            }
        }
        let inherited = suggestion_inherited(suggested.suggestion.as_ref());
        let inherited_key =
            inherited.as_ref().map(|item| item.theme.key.as_str());
        let field = parse_task_theme(&source_line);
        let x_eq_i = inherited_key == Some(x_key.as_str());
        let (action, theme, edited_line) = if x_eq_i {
            match field {
                TaskThemeState::None => {
                    let display = catalog.display_name(&x_key, &x_name);
                    let theme_ref = catalog.theme_ref(&x_key, &display);
                    ("kept", Some(theme_ref), source_line.clone())
                }
                _ => {
                    let cleared = clear_task_theme(&source_line);
                    ("cleared", None, cleared)
                }
            }
        } else {
            let field_is_x = match &field {
                TaskThemeState::Set { key, .. } => key == &x_key,
                _ => false,
            };
            if field_is_x {
                let display = catalog.display_name(&x_key, &x_name);
                let theme_ref = catalog.theme_ref(&x_key, &display);
                ("kept", Some(theme_ref), source_line.clone())
            } else {
                let display = catalog.display_name(&x_key, &x_name);
                let theme_ref = catalog.theme_ref(&x_key, &display);
                let edited = set_task_theme(&source_line, &display);
                ("set", Some(theme_ref), edited)
            }
        };
        let previous = valid_previous_theme(&field, catalog);
        let summary = CaptureTaskThemeChange {
            action,
            theme,
            previous,
            inherited,
        };
        if edited_line != source_line {
            self.write_task_field(planner, target, block_id, |_| {
                edited_line.clone()
            })?;
        }
        Ok(ExistingTypeMarkerApplication {
            summary,
            type_key: x_key,
            type_name: x_name,
        })
    }

    /// Explicit `#<type>` retype of an existing task selected by a stale-safe
    /// picker ref that carries no block ID. Mirrors
    /// [`Self::apply_type_marker_to_existing`] but addresses the staged task
    /// by its line index, so no ID is minted and no Task Link is touched.
    /// Closed targets refuse without a write.
    pub(super) fn apply_type_marker_to_existing_line(
        &mut self,
        planner: &mut CaptureBatchPlanner,
        target: &Path,
        route: &str,
        line_index: usize,
        marker: &TypeMarker,
        warnings: &mut Vec<CaptureWarning>,
    ) -> Result<ExistingTypeMarkerApplication, CaptureError> {
        self.refresh_residence_snapshot(planner);
        let current_contents = planner.read_existing(target)?;
        let settings = note_tasks::read_settings(&self.root);
        let scan = note_tasks::scan(&current_contents, &settings);
        let task = match scan.task_at(line_index) {
            Some(task) => task,
            None => {
                return Err(CaptureError::io(
                    "task theme capture invariant failed: existing task disappeared before retype",
                ));
            }
        };
        if !crate::native::capture_link_tasks::is_linkable_status(
            task.status_symbol,
        ) {
            return Err(CaptureError::usage(format!(
                "`{}` needs an open task: the selected task in {route}.md is {}; closed tasks keep their fields as history",
                marker.authored,
                task.status_name
            )));
        }
        let source_line =
            line_text_at(&current_contents, task.line_index)?.to_string();
        if !self.load_catalog(warnings) {
            return Err(CaptureError::io(
                "could not load the task-type catalog to validate `#<type>`",
            ));
        }
        let found = {
            let catalog = self.catalog.as_ref().expect("loaded theme catalog");
            let Some(found) = resolve_accepted_type_name(
                catalog,
                &marker.name,
                marker.literal,
            ) else {
                return Err(self.type_not_configured_error(marker, catalog));
            };
            found
        };
        if !self.load_config(warnings) {
            return Err(CaptureError::io(format!(
                "read plan config {} to apply task type",
                self.config_path.display()
            )));
        }
        let cfg = self.cfg.as_ref().expect("loaded plan config");
        let catalog = self.catalog.as_ref().expect("loaded theme catalog");
        let (x_key, x_name) = (found.key, found.name);
        let residence_path = relative_target(Some(route));
        let residence = Path::new(&residence_path);
        let suggested = suggest::suggest_for_task(
            &mut self.resolver,
            catalog,
            cfg,
            residence,
            &source_line,
        );
        for warning in suggested.warnings {
            let identity = (warning.code.clone(), warning.path.clone());
            if self.warned_suggestions.insert(identity) {
                warnings.push(CaptureWarning::Suggestion(warning));
            }
        }
        let inherited = suggestion_inherited(suggested.suggestion.as_ref());
        let inherited_key =
            inherited.as_ref().map(|item| item.theme.key.as_str());
        let field = parse_task_theme(&source_line);
        let x_eq_i = inherited_key == Some(x_key.as_str());
        let (action, theme, edited_line) = if x_eq_i {
            match field {
                TaskThemeState::None => {
                    let display = catalog.display_name(&x_key, &x_name);
                    let theme_ref = catalog.theme_ref(&x_key, &display);
                    ("kept", Some(theme_ref), source_line.clone())
                }
                _ => {
                    let cleared = clear_task_theme(&source_line);
                    ("cleared", None, cleared)
                }
            }
        } else {
            let field_is_x = match &field {
                TaskThemeState::Set { key, .. } => key == &x_key,
                _ => false,
            };
            if field_is_x {
                let display = catalog.display_name(&x_key, &x_name);
                let theme_ref = catalog.theme_ref(&x_key, &display);
                ("kept", Some(theme_ref), source_line.clone())
            } else {
                let display = catalog.display_name(&x_key, &x_name);
                let theme_ref = catalog.theme_ref(&x_key, &display);
                let edited = set_task_theme(&source_line, &display);
                ("set", Some(theme_ref), edited)
            }
        };
        let previous = valid_previous_theme(&field, catalog);
        let summary = CaptureTaskThemeChange {
            action,
            theme,
            previous,
            inherited,
        };
        if edited_line != source_line {
            let line_index = task.line_index;
            self.write_task_field_at_line(planner, target, line_index, |_| {
                edited_line.clone()
            })?;
        }
        Ok(ExistingTypeMarkerApplication {
            summary,
            type_key: x_key,
            type_name: x_name,
        })
    }

    fn write_task_field_at_line(
        &self,
        planner: &mut CaptureBatchPlanner,
        target: &Path,
        line_index: usize,
        update: impl FnOnce(&str) -> String,
    ) -> Result<String, CaptureError> {
        let current = planner.read_existing(target)?;
        let spans = line_spans(&current);
        let span = spans.get(line_index).ok_or_else(|| {
            CaptureError::io(
                "task theme capture invariant failed: task line is out of range",
            )
        })?;
        let start = line_index
            .checked_sub(1)
            .map_or(0, |previous| spans[previous].end);
        let segment = &current[start..span.end];
        let updated_segment = update(segment);
        let (_, line_ending) =
            crate::native::markdown::split_line_ending(segment);
        let updated_segment = if line_ending.is_empty() {
            updated_segment
        } else {
            let (body, _) =
                crate::native::markdown::split_line_ending(&updated_segment);
            format!("{body}{line_ending}")
        };
        let mut updated = String::with_capacity(
            current.len() + updated_segment.len().saturating_sub(segment.len()),
        );
        updated.push_str(&current[..start]);
        updated.push_str(&updated_segment);
        updated.push_str(&current[span.end..]);
        planner.stage(target, updated.clone())?;
        line_text_at(&updated, line_index).map(str::to_string)
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
        selector: Option<&str>,
        warnings: &mut Vec<CaptureWarning>,
    ) -> Result<Option<AppliedLanding>, CaptureError> {
        self.refresh_residence_snapshot(planner);
        if !self.load_config(warnings) || !self.load_catalog(warnings) {
            return Ok(None);
        }
        let cfg = self.cfg.as_ref().expect("loaded plan config");
        let catalog = self.catalog.as_ref().expect("loaded theme catalog");
        let residence_path = relative_target(Some(route));
        let residence = Path::new(&residence_path);
        // The staged line carries authored fields and ref-task markers;
        // resolve it as the task that will be written instead of using
        // a blank-task-only note default.
        let suggested = pomodoro_themes::suggest::suggest_for_task(
            &mut self.resolver,
            catalog,
            cfg,
            residence,
            source_line,
        );
        for warning in suggested.warnings {
            let identity = (warning.code.clone(), warning.path.clone());
            if self.warned_suggestions.insert(identity) {
                warnings.push(CaptureWarning::Suggestion(warning));
            }
        }

        let field = parse_task_theme(source_line);
        let inherited = suggestion_inherited(suggested.suggestion.as_ref());
        let effective_key = suggested
            .suggestion
            .as_ref()
            .map(|item| item.theme.key.as_str());
        let effective_name = suggested
            .suggestion
            .as_ref()
            .map(|item| item.theme.name.as_str());
        let inherited_key =
            inherited.as_ref().map(|item| item.theme.key.as_str());
        let sources = catalog.theme_sources();
        // R2 landing: links merge by default, never retype. Only MEET
        // destinations and explicit Obsidian Alt+Enter retypes take the
        // Set/Clear path; capture always lands with `retype: false`.
        let outcome = routing::land(
            &routing::LandInput {
                field: &field,
                effective_key,
                effective_name,
                inherited_key,
                destination: destination_name,
                selector,
                retype: false,
                tab_pick: None,
            },
            &sources,
        );
        match outcome {
            routing::LandOutcome::Container => {
                if let Some(message) =
                    label_keep_warning(destination_name, &sources)
                {
                    warnings.push(CaptureWarning::Text(message));
                }
                Ok(None)
            }
            routing::LandOutcome::Consistent => Ok(None),
            routing::LandOutcome::Merge { name } => {
                let (Some(key), Some(display_name)) =
                    (effective_key, effective_name)
                else {
                    return Ok(None);
                };
                let display = catalog.display_name(key, display_name);
                let added = catalog.theme_ref(key, &display);
                let before = destination_name.to_string();
                let after = name;
                let mut dest_index = None;
                if before != after {
                    match self
                        .stage_merge_rename(planner, block_id, &before, &after)
                    {
                        Ok(index) => dest_index = index,
                        Err(error) => warnings.push(CaptureWarning::Text(
                            format!("merge Pomodoro rename failed: {error:?}"),
                        )),
                    }
                }
                let Some(dest_index) = dest_index else {
                    return Ok(None);
                };
                Ok(Some(AppliedLanding {
                    task_line: source_line.to_string(),
                    theme: None,
                    merge: Some(AppliedMerge {
                        before,
                        after,
                        added,
                        dest_index,
                    }),
                }))
            }
            routing::LandOutcome::Set { name, key } => {
                let change = ThemeChange::Set { name, key };
                self.apply_theme_change(
                    planner,
                    target,
                    block_id,
                    source_line,
                    change,
                    inherited,
                )
            }
            routing::LandOutcome::Clear => match field {
                TaskThemeState::None => Ok(None),
                _ => self.apply_theme_change(
                    planner,
                    target,
                    block_id,
                    source_line,
                    ThemeChange::Clear,
                    inherited,
                ),
            },
        }
    }

    fn apply_theme_change(
        &self,
        planner: &mut CaptureBatchPlanner,
        target: &Path,
        block_id: &str,
        source_line: &str,
        change: ThemeChange,
        inherited: Option<ThemeSuggestion>,
    ) -> Result<Option<AppliedLanding>, CaptureError> {
        let catalog = self.catalog.as_ref().expect("loaded theme catalog");
        let field = parse_task_theme(source_line);
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
        };
        let summary = CaptureTaskThemeChange {
            action,
            theme,
            previous,
            inherited,
        };
        Ok(Some(AppliedLanding {
            task_line,
            theme: Some(summary),
            merge: None,
        }))
    }

    /// Stage an R2 destination rename for `block_id`'s landing entry.
    /// The link is already staged, so the entry is found by the block ID
    /// it now contains; later batch items then resolve against the merged
    /// name and append each missing type once.
    fn stage_merge_rename(
        &self,
        planner: &mut CaptureBatchPlanner,
        block_id: &str,
        before: &str,
        after: &str,
    ) -> Result<Option<usize>, CaptureError> {
        let day_file = crate::native::pomodoro::day_file_for(&self.root);
        let day_contents = planner.read_existing(&day_file)?;
        let scan = crate::native::capture_pomodoros::scan(&day_contents);
        if !scan.has_section {
            return Ok(None);
        }
        let needle = format!("#^{block_id}]]");
        let link_line = day_contents
            .lines()
            .position(|line| line.contains(needle.as_str()));
        let Some(link_line) = link_line else {
            return Ok(None);
        };
        let link_one_based = link_line + 1;
        // Own the destination: the nearest open headline at or above the
        // staged link (entries scan in document order).
        let mut owner: Option<
            &crate::native::capture_pomodoros::PomodoroEntry,
        > = None;
        for entry in &scan.entries {
            if entry.line <= link_one_based
                && entry.state
                    == crate::native::capture_pomodoros::PomodoroState::Open
            {
                owner = Some(entry);
            }
        }
        let Some(entry) = owner else {
            return Ok(None);
        };
        // Skip when another batch item already merged this entry past
        // `before` (stale destination): the caller resolved against staged
        // state, so a mismatch means the rename already landed.
        if entry.name.as_deref() != Some(before) {
            return Ok(None);
        }
        let entry_index = entry.line.saturating_sub(1);
        let Some(updated) =
            crate::native::pomodoro_themes::relocation_apply::rename_entry(
                &day_contents,
                entry.line,
                after,
            )
        else {
            return Ok(None);
        };
        planner.stage(&day_file, updated)?;
        Ok(Some(entry_index))
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
        let (_, line_ending) =
            crate::native::markdown::split_line_ending(segment);
        let updated_segment = if line_ending.is_empty() {
            updated_segment
        } else {
            let (body, _) =
                crate::native::markdown::split_line_ending(&updated_segment);
            format!("{body}{line_ending}")
        };
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

    /// Resolve one staged task line's effective type (D3) for bare-form
    /// typed placement, with the catalog vocab and exempt list.
    /// Returns `None` when no effective type applies (no default
    /// configured, closed task, unreadable catalog), in which case the
    /// caller keeps the legacy implicit selection. Pushes the same
    /// suggestion warnings as the named-capture path.
    pub(crate) fn placement_for_task(
        &mut self,
        planner: &mut CaptureBatchPlanner,
        residence_route: &str,
        task_line: &str,
        warnings: &mut Vec<CaptureWarning>,
    ) -> Option<PlacementType> {
        self.refresh_residence_snapshot(planner);
        if !self.load_config(warnings) || !self.load_catalog(warnings) {
            return None;
        }
        let cfg = self.cfg.as_ref().expect("loaded plan config");
        let catalog = self.catalog.as_ref().expect("loaded theme catalog");
        let residence_path = relative_target(Some(residence_route));
        let residence = Path::new(&residence_path);
        let suggested = pomodoro_themes::suggest::suggest_for_task(
            &mut self.resolver,
            catalog,
            cfg,
            residence,
            task_line,
        );
        for warning in suggested.warnings {
            let identity = (warning.code.clone(), warning.path.clone());
            if self.warned_suggestions.insert(identity) {
                warnings.push(CaptureWarning::Suggestion(warning));
            }
        }
        let suggestion = suggested.suggestion?;
        let sources = catalog.theme_sources();
        Some(PlacementType {
            suggestion,
            exempt: sources.exempt.into_iter().collect(),
        })
    }

    /// Resolve the pre-close types R6 names a close with. Reads every Task
    /// Link under the running entry through the staged snapshot _before_
    /// the close stages anything, because a closed task stops suggesting.
    ///
    /// Returns `None` when no naming can apply (no running entry, an
    /// unnamed, MEET, or single-type name, or an unreadable config or
    /// catalog), so plain closes pay for no catalog load and never rename.
    pub(super) fn close_types(
        &mut self,
        planner: &mut CaptureBatchPlanner,
        vault: &pomodoro_close::SnapshotCloseVault,
        day_file: &Path,
        day_contents: &str,
        warnings: &mut Vec<CaptureWarning>,
    ) -> Option<CloseTypes> {
        let running =
            capture_pomodoro_close::find_running_pomodoro(day_contents).ok()?;
        self.close_types_for_entry(
            planner,
            vault,
            day_file,
            day_contents,
            &running,
            warnings,
        )
    }

    pub(super) fn close_types_for_entry(
        &mut self,
        planner: &mut CaptureBatchPlanner,
        vault: &pomodoro_close::SnapshotCloseVault,
        day_file: &Path,
        day_contents: &str,
        running: &capture_pomodoro_close::RunningPomodoro,
        warnings: &mut Vec<CaptureWarning>,
    ) -> Option<CloseTypes> {
        use capture_pomodoro_close::CloseVault;
        let name = running.name.as_deref()?;
        if split_components(name).len() < 2 || routing::is_meet_name(Some(name))
        {
            return None;
        }
        if !self.load_config(warnings) || !self.load_catalog(warnings) {
            return None;
        }
        let catalog = self.catalog.as_ref().expect("loaded theme catalog");
        let sources = catalog.theme_sources();
        let types = routing::type_components(Some(name), &sources)
            .iter()
            .filter(|item| item.is_type)
            .count();
        if types < 2 {
            return None;
        }
        let refs = catalog.theme_refs_for_name(name);
        let lines: Vec<&str> = day_contents.lines().collect();
        let range = capture_pomodoro_close::sub_bullet_range(
            &lines,
            running.line.saturating_sub(1),
        );
        let settings = note_tasks::read_settings(&self.root);
        // Suggestion warnings describe task placement, not this close.
        let mut scratch = Vec::new();
        let mut seen = BTreeSet::new();
        let mut link_types = BTreeMap::new();
        for index in range {
            let stripped =
                capture_pomodoro_close::strip_pomodoro_markers(lines[index]);
            for token in capture_pomodoro_close::wikilink_tokens(&stripped) {
                if token.block_id.is_empty() {
                    continue;
                }
                let key = (token.path_part.clone(), token.block_id.clone());
                if !seen.insert(key.clone()) {
                    continue;
                }
                let LinkResolution::Found(path) =
                    vault.resolve_target(day_file, &token.path_part)
                else {
                    continue;
                };
                let Ok(Some(contents)) = vault.read_latest(&path) else {
                    continue;
                };
                let Ok(task) = capture_pomodoro_close::lookup_task(
                    &contents,
                    &settings,
                    &token.block_id,
                ) else {
                    continue;
                };
                let Some(task_line) = contents.lines().nth(task.line_index)
                else {
                    continue;
                };
                let route = path
                    .strip_prefix(&self.root)
                    .unwrap_or(&path)
                    .with_extension("")
                    .to_string_lossy()
                    .replace('\\', "/");
                if let Some(placement) = self.placement_for_task(
                    planner,
                    &route,
                    task_line,
                    &mut scratch,
                ) {
                    link_types.insert(key, placement.suggestion.theme.key);
                }
            }
        }
        Some(CloseTypes {
            naming: capture_pomodoro_close::CloseNaming {
                sources,
                link_types,
            },
            refs,
        })
    }

    /// R2 merge for a `#<type>` marker that lands in a named Pomodoro:
    /// the marker sets X first, then a disagreeing destination gains it.
    /// Stages the destination rename (found by `block_id`) and returns the
    /// merge record. Returns `None` when the destination already contains
    /// X, is a container, or is MEET.
    pub(super) fn merge_named_destination(
        &mut self,
        planner: &mut CaptureBatchPlanner,
        block_id: &str,
        destination_before: &str,
        x_key: &str,
        x_name: &str,
        warnings: &mut Vec<CaptureWarning>,
    ) -> Result<Option<AppliedMerge>, CaptureError> {
        self.refresh_residence_snapshot(planner);
        if !self.load_config(warnings) || !self.load_catalog(warnings) {
            return Ok(None);
        }
        let catalog = self.catalog.as_ref().expect("loaded theme catalog");
        let sources = catalog.theme_sources();
        if routing::is_meet_name(Some(destination_before)) {
            return Ok(None);
        }
        let components =
            routing::type_components(Some(destination_before), &sources);
        if components.is_empty()
            || components.iter().all(|component| !component.is_type)
        {
            return Ok(None);
        }
        if components
            .iter()
            .any(|component| component.is_type && component.key == x_key)
        {
            return Ok(None);
        }
        let display = catalog.display_name(x_key, x_name);
        let added = catalog.theme_ref(x_key, &display);
        let before = destination_before.to_string();
        let after = format!("{} + {display}", destination_before.trim());
        if before == after {
            return Ok(None);
        }
        let dest_index =
            match self.stage_merge_rename(planner, block_id, &before, &after) {
                Ok(Some(index)) => index,
                Ok(None) => return Ok(None),
                Err(error) => {
                    warnings.push(CaptureWarning::Text(format!(
                        "merge Pomodoro rename failed: {error:?}"
                    )));
                    return Ok(None);
                }
            };
        Ok(Some(AppliedMerge {
            before,
            after,
            added,
            dest_index,
        }))
    }

    /// Shared D8 relocation for an explicit `#type` retype: after the gesture,
    /// restore ledger agreement for `block_id` becoming `type_key`/`type_name`.
    /// Stages the day-note edit, pins explicit Pomodoro block refs for the
    /// renamed or created headlines, and rebases existing gesture refs.
    /// Returns `None` when the ledger already agrees; types are never
    /// refused.
    pub(super) fn relocate_for_retype(
        &mut self,
        planner: &mut CaptureBatchPlanner,
        block_id: &str,
        type_key: &str,
        type_name: &str,
        now: chrono::NaiveDateTime,
        pomodoro_refs: &mut Vec<PomodoroBlockRef>,
        warnings: &mut Vec<CaptureWarning>,
    ) -> Result<Option<AppliedRelocation>, CaptureError> {
        self.refresh_residence_snapshot(planner);
        if !self.load_config(warnings) || !self.load_catalog(warnings) {
            return Ok(None);
        }
        let catalog = self.catalog.as_ref().expect("loaded theme catalog");
        let sources = catalog.theme_sources();
        let day_file = crate::native::pomodoro::day_file_for(&self.root);
        let day_contents = match planner.read_existing(&day_file) {
            Ok(contents) => contents,
            Err(_) => return Ok(None),
        };
        let Some((updated, info)) =
            crate::native::pomodoro_themes::relocation_apply::apply_to_day_contents(
                &day_contents,
                block_id,
                type_key,
                type_name,
                &sources,
            )
        else {
            return Ok(None);
        };
        let restamped_refs =
            super::project_deps::restamp_changed_timed_entries(
                planner,
                &self.root,
                &day_file,
                day_file
                    .strip_prefix(&self.root)
                    .unwrap_or(day_file.as_path()),
                &day_contents,
                updated,
                now,
                warnings,
            )?;
        let updated = planner.read_existing(&day_file)?;
        let refs = std::mem::take(pomodoro_refs);
        *pomodoro_refs =
            compose_type_relocation_refs(refs, &info, &day_contents, &updated);
        pomodoro_refs.extend(restamped_refs);
        Ok(Some(AppliedRelocation {
            json: super::output::TypeRelocationJson {
                outcome: info.outcome,
                from: info.from,
                to: info.to,
            },
            dest_after: info.dest_after,
            dest_created: info.dest_created,
        }))
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

    fn type_not_configured_error(
        &self,
        marker: &TypeMarker,
        catalog: &Catalog,
    ) -> CaptureError {
        let authored = &marker.authored;
        let escaped = format!("\\{authored}");
        let canonical = unmatched_type_display(&marker.name);
        let hint = marker
            .heading_route
            .as_ref()
            .map(|route| {
                let name = authored.strip_prefix('#').unwrap_or(authored);
                format!(
                    " (for a bullet under a heading, write `{route}#{name}`)"
                )
            })
            .unwrap_or_default();
        let message = if catalog.configured_themes().is_some() {
            let suggestions = catalog
                .similar_to(&canonical)
                .into_iter()
                .take(2)
                .map(|name| replacement_for(&name, &normalize_component(&name)))
                .collect::<Vec<_>>();
            let add = if suggestions.is_empty() { "add" } else { "Add" };
            let separator = if suggestions.is_empty() { ";" } else { "" };
            let suggestion = if suggestions.is_empty() {
                String::new()
            } else {
                format!(
                    "; did you mean {}?",
                    suggestions
                        .iter()
                        .map(|name| format!("`{name}`"))
                        .collect::<Vec<_>>()
                        .join(" or ")
                )
            };
            format!(
                "`{authored}` is not one of your task types{suggestion}{separator} {add} {canonical} under `types:` in {} to use it, or write `{escaped}` to keep it as a tag{hint}",
                self.config_path.display()
            )
        } else {
            format!(
                "`{authored}` is not a task type yet: no Pomodoro has used {canonical} and no `types:` vocabulary is configured; add `types:` to {}, or write `{escaped}` to keep it as a tag{hint}",
                self.config_path.display()
            )
        };
        CaptureError::usage_code(message, "type_not_configured")
    }
}

/// Rebase the gesture's Pomodoro refs through a D8 rewrite and pin the
/// destination identity so the batch tracker never guesses from name
/// equality. `pre` is the post-gesture day text (relocation input);
/// `post` is the relocated day text. `before` indices on existing refs
/// stay in item-pre coordinates; only `after` is rewritten here.
fn compose_type_relocation_refs(
    refs: Vec<PomodoroBlockRef>,
    info: &crate::native::pomodoro_themes::relocation_apply::TypeRelocation,
    pre: &str,
    post: &str,
) -> Vec<PomodoroBlockRef> {
    let pre_lines: Vec<&str> = pre.lines().collect();
    let post_lines: Vec<&str> = post.lines().collect();
    let (old_to_new, _) = line_maps(&pre_lines, &post_lines);
    let rename_in_place =
        !info.dest_created && info.dest_before == Some(info.source_before);
    let dest_before = info.dest_before;
    let pin_dest = |item: PomodoroBlockRef| -> PomodoroBlockRef {
        let before = match item.before {
            PomodoroBlockBefore::Resolve if info.dest_created => {
                PomodoroBlockBefore::Created
            }
            PomodoroBlockBefore::Resolve => PomodoroBlockBefore::At(
                dest_before.unwrap_or(info.source_before),
            ),
            other => other,
        };
        PomodoroBlockRef {
            before,
            after: Some(info.dest_after),
            ..item
        }
    };
    let mut composed = Vec::with_capacity(refs.len().saturating_add(2));
    for item in refs {
        match item.after {
            Some(after) if dest_before == Some(after) => {
                composed.push(pin_dest(item));
            }
            Some(after) if after == info.source_before && rename_in_place => {
                composed.push(pin_dest(item));
            }
            Some(after) => composed.push(PomodoroBlockRef {
                after: Some(old_to_new.get(&after).copied().unwrap_or(after)),
                ..item
            }),
            None if rename_in_place
                && item.role == PomodoroBlockRole::Unlinked
                && matches!(
                    item.before,
                    PomodoroBlockBefore::At(before)
                        if before == info.source_before
                ) =>
            {
                composed.push(PomodoroBlockRef {
                    after: Some(info.dest_after),
                    ..item
                });
            }
            None => composed.push(item),
        }
    }
    match info.outcome {
        "rename_entry" | "append_component" | "rename_placeholder" => {
            composed.push(PomodoroBlockRef::at(
                PomodoroBlockRole::Changed,
                info.source_before,
                info.dest_after,
            ));
        }
        "move_to" => {
            if let Some(dest) = info.dest_before {
                composed.push(PomodoroBlockRef::at(
                    PomodoroBlockRole::Linked,
                    dest,
                    info.dest_after,
                ));
            }
            composed
                .push(PomodoroBlockRef::unlinked_before(info.source_before));
        }
        "create_below" => {
            composed.push(PomodoroBlockRef::created(
                PomodoroBlockRole::Linked,
                info.dest_after,
            ));
            composed
                .push(PomodoroBlockRef::unlinked_before(info.source_before));
        }
        _ => {}
    }
    composed
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

/// D4/D10 label copy when a named destination is a container.
fn label_keep_warning(
    destination: &str,
    sources: &ThemeSources,
) -> Option<String> {
    let trimmed = destination.trim();
    if trimmed.is_empty() || trimmed == "(unnamed)" {
        return None;
    }
    let components = split_components(trimmed);
    let display = components
        .first()
        .and_then(|component| canonical_theme_name(component))
        .unwrap_or_else(|| trimmed.to_string());
    let inventory =
        components.iter().any(|component| {
            sources.inventory.contains(&normalize_component(component))
        }) || sources.inventory.contains(&normalize_component(trimmed));
    Some(if inventory {
        format!("{display} is a label · tasks keep their own types")
    } else {
        format!(
            "{display} isn't one of your types; tasks here keep their own types"
        )
    })
}
