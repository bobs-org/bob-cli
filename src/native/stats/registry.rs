use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::path::Path;

use chrono::NaiveDate;

use super::error::StatsError;
use super::formula::{parse_formula, FormulaExpr};
use super::report::{JournalTiersJson, MetricParameters, StatsWarning};
use super::request::MetricSelection;
use crate::native::config::StatsConfig;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct MetricRegistry {
    pub(crate) defs: Vec<MetricDef>,
    pub(crate) evaluation_order: Vec<String>,
    pub(crate) hidden: BTreeSet<String>,
    pub(crate) overview: Vec<String>,
    pub(crate) warnings: Vec<StatsWarning>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct MetricDef {
    pub(crate) id: String,
    pub(crate) label: String,
    pub(crate) spec: MetricSpec,
    pub(crate) unit: MetricUnit,
    pub(crate) direction: Direction,
    pub(crate) color: PaletteSlot,
    pub(crate) source: MetricSource,
    pub(crate) origin: MetricOrigin,
    pub(crate) since: Option<NaiveDate>,
    pub(crate) definition: String,
    pub(crate) limits: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum MetricSpec {
    Pomodoro {
        measure: PomodoroMeasure,
        themes: Vec<String>,
        exclude_themes: Vec<String>,
    },
    Tasks {
        event: TaskEvent,
        tags_any: Vec<String>,
        tags_all: Vec<String>,
        in_notes: Vec<String>,
        task_kind: Option<TaskKindFilter>,
        recurring: RecurringFilter,
    },
    Checklist {
        tags: Vec<String>,
        match_text: Option<String>,
        measure: ChecklistMeasure,
    },
    Field {
        field: String,
        source: FieldSource,
    },
    Formula {
        expr: String,
        ast: FormulaExpr,
        dependencies: Vec<String>,
    },
    Journal {
        select: JournalSelect,
        tiers: JournalTiers,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PomodoroMeasure {
    Minutes,
    Count,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TaskEvent {
    Created,
    Done,
    Dropped,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TaskKindFilter {
    Task,
    Prj,
    Ref,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RecurringFilter {
    Exclude,
    Only,
    Include,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ChecklistMeasure {
    Ratio,
    Count,
    Bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FieldSource {
    Frontmatter,
    Inline,
    Either,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum JournalSelect {
    Reviewed,
    KeepRate,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum JournalTiers {
    Decision,
    All,
    List(Vec<String>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MetricUnit {
    Count,
    Signed,
    Minutes,
    Bool,
    Ratio,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Direction {
    Up,
    Down,
    Neutral,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PaletteSlot {
    Orange,
    Violet,
    Green,
    Blue,
    Gray,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MetricSource {
    Derived,
    Recorded,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MetricOrigin {
    Builtin,
    Custom,
    Redefined,
}

impl MetricSpec {
    pub(crate) fn kind_name(&self) -> &'static str {
        match self {
            Self::Pomodoro { .. } => "pomodoro",
            Self::Tasks { .. } => "tasks",
            Self::Checklist { .. } => "checklist",
            Self::Field { .. } => "field",
            Self::Formula { .. } => "formula",
            Self::Journal { .. } => "journal",
        }
    }

    fn dependencies(&self) -> &[String] {
        match self {
            Self::Formula { dependencies, .. } => dependencies,
            _ => &[],
        }
    }
}

impl MetricDef {
    fn evaluation_dependencies(&self) -> Vec<&str> {
        match &self.spec {
            MetricSpec::Formula { dependencies, .. } => {
                dependencies.iter().map(String::as_str).collect()
            }
            MetricSpec::Journal { .. } if self.id != "review_closed" => {
                vec!["review_closed"]
            }
            _ => Vec::new(),
        }
    }
}

impl MetricUnit {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Count => "count",
            Self::Signed => "signed",
            Self::Minutes => "minutes",
            Self::Bool => "bool",
            Self::Ratio => "ratio",
        }
    }
}

impl Direction {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Up => "up",
            Self::Down => "down",
            Self::Neutral => "neutral",
        }
    }
}

impl PaletteSlot {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Orange => "orange",
            Self::Violet => "violet",
            Self::Green => "green",
            Self::Blue => "blue",
            Self::Gray => "gray",
        }
    }
}

impl MetricSource {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Derived => "derived",
            Self::Recorded => "recorded",
        }
    }
}

impl MetricOrigin {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Builtin => "builtin",
            Self::Custom => "custom",
            Self::Redefined => "redefined",
        }
    }
}

impl MetricDef {
    pub(crate) fn kind_name(&self) -> &'static str {
        self.spec.kind_name()
    }

    pub(crate) fn parameters(&self) -> MetricParameters {
        let mut parameters = MetricParameters {
            measure: None,
            themes: Vec::new(),
            exclude_themes: Vec::new(),
            event: None,
            tags_any: Vec::new(),
            tags_all: Vec::new(),
            in_notes: Vec::new(),
            task_kind: None,
            recurring: None,
            tags: Vec::new(),
            match_text: None,
            field: None,
            field_source: None,
            expr: None,
            dependencies: Vec::new(),
            select: None,
            tiers: None,
        };
        match &self.spec {
            MetricSpec::Pomodoro {
                measure,
                themes,
                exclude_themes,
            } => {
                parameters.measure = Some(match measure {
                    PomodoroMeasure::Minutes => "minutes".to_string(),
                    PomodoroMeasure::Count => "count".to_string(),
                });
                parameters.themes = themes.clone();
                parameters.exclude_themes = exclude_themes.clone();
            }
            MetricSpec::Tasks {
                event,
                tags_any,
                tags_all,
                in_notes,
                task_kind,
                recurring,
            } => {
                parameters.event = Some(match event {
                    TaskEvent::Created => "created".to_string(),
                    TaskEvent::Done => "done".to_string(),
                    TaskEvent::Dropped => "dropped".to_string(),
                });
                parameters.tags_any = tags_any.clone();
                parameters.tags_all = tags_all.clone();
                parameters.in_notes = in_notes.clone();
                parameters.task_kind = task_kind.map(|kind| match kind {
                    TaskKindFilter::Task => "task".to_string(),
                    TaskKindFilter::Prj => "prj".to_string(),
                    TaskKindFilter::Ref => "ref".to_string(),
                });
                parameters.recurring = Some(match recurring {
                    RecurringFilter::Exclude => "exclude".to_string(),
                    RecurringFilter::Only => "only".to_string(),
                    RecurringFilter::Include => "include".to_string(),
                });
            }
            MetricSpec::Checklist {
                tags,
                match_text,
                measure,
            } => {
                parameters.measure = Some(match measure {
                    ChecklistMeasure::Ratio => "ratio".to_string(),
                    ChecklistMeasure::Count => "count".to_string(),
                    ChecklistMeasure::Bool => "bool".to_string(),
                });
                parameters.tags = tags.clone();
                parameters.match_text = match_text.clone();
            }
            MetricSpec::Field { field, source } => {
                parameters.field = Some(field.clone());
                parameters.field_source = Some(match source {
                    FieldSource::Frontmatter => "frontmatter".to_string(),
                    FieldSource::Inline => "inline".to_string(),
                    FieldSource::Either => "either".to_string(),
                });
            }
            MetricSpec::Formula {
                expr, dependencies, ..
            } => {
                parameters.expr = Some(expr.clone());
                parameters.dependencies = dependencies.clone();
            }
            MetricSpec::Journal { select, tiers } => {
                parameters.select = Some(match select {
                    JournalSelect::Reviewed => "reviewed".to_string(),
                    JournalSelect::KeepRate => "keep_rate".to_string(),
                });
                parameters.tiers = Some(match tiers {
                    JournalTiers::Decision => {
                        JournalTiersJson::Named("decision".to_string())
                    }
                    JournalTiers::All => {
                        JournalTiersJson::Named("all".to_string())
                    }
                    JournalTiers::List(list) => {
                        JournalTiersJson::List(list.clone())
                    }
                });
            }
        }
        parameters
    }
}

impl MetricRegistry {
    pub(crate) fn get(&self, id: &str) -> Option<&MetricDef> {
        self.defs.iter().find(|def| def.id == id)
    }

    pub(crate) fn is_hidden(&self, id: &str) -> bool {
        self.hidden.contains(id)
    }

    pub(crate) fn visible(&self) -> Vec<&MetricDef> {
        self.defs
            .iter()
            .filter(|def| !self.hidden.contains(&def.id))
            .collect()
    }

    pub(crate) fn overview_metrics(&self) -> Vec<&MetricDef> {
        self.overview
            .iter()
            .filter(|id| !self.hidden.contains(id.as_str()))
            .filter_map(|id| self.get(id))
            .collect()
    }

    pub(crate) fn select(
        &self,
        selection: &MetricSelection,
    ) -> Result<Vec<&MetricDef>, StatsError> {
        match selection {
            MetricSelection::Overview => Ok(self.overview_metrics()),
            MetricSelection::AllVisible => Ok(self.visible()),
            MetricSelection::Explicit(ids) => {
                let mut selected = Vec::new();
                for id in ids {
                    let Some(def) = self.get(id) else {
                        if let Some(warning) =
                            self.warnings.iter().find(|warning| {
                                warning.metric.as_deref() == Some(id)
                            })
                        {
                            return Err(StatsError::Request {
                                code: "unknown_metric",
                                message: format!(
                                    "unknown metric {id:?}: {}",
                                    warning.message
                                ),
                            });
                        }
                        return Err(StatsError::unknown_metric(id));
                    };
                    selected.push(def);
                }
                Ok(selected)
            }
        }
    }

    /// Selected metrics plus formula and journal-gate dependencies, in
    /// evaluation order. Journal metrics pull `review_closed` so gap
    /// detection evaluates first, even when that gate is hidden.
    pub(crate) fn with_dependencies(
        &self,
        selected: &[&MetricDef],
    ) -> Vec<&MetricDef> {
        let mut needed = BTreeSet::new();
        let mut stack: Vec<&str> =
            selected.iter().map(|def| def.id.as_str()).collect();
        while let Some(id) = stack.pop() {
            if !needed.insert(id.to_string()) {
                continue;
            }
            if let Some(def) = self.get(id) {
                for dep in def.evaluation_dependencies() {
                    stack.push(dep);
                }
            }
        }
        self.evaluation_order
            .iter()
            .filter(|id| needed.contains(id.as_str()))
            .filter_map(|id| self.get(id))
            .collect()
    }
}

const BUILTINS: &[(&str, &str)] = &[
    (
        "worked",
        r##"
label: Worked
kind: pomodoro
measure: minutes
unit: minutes
direction: up
color: orange
description: "Minutes in closed Pomodoros under ## Pomodoros; equals each daily note's heading."
limits:
  - Open Pomodoros are plans and never count.
  - Attributed from Task Links.
  - Grouped by today's note tree.
  - Sessions with several homes are split evenly by task.
  - Links show what a session was for, not a stopwatch.
"##,
    ),
    (
        "reviewed",
        r##"
label: Reviewed
kind: journal
select: reviewed
tiers: decision
unit: count
direction: up
color: violet
description: "Distinct walk row keys with at least one journaled answer on day D in decision tiers (every tier except pre and post)."
limits:
  - Tracking starts at the first recorded answer.
"##,
    ),
    (
        "done",
        r##"
label: Done
kind: tasks
event: done
recurring: exclude
unit: count
direction: up
color: green
description: "Non-recurring tasks whose status type is DONE and whose completion date is D, excluding the daily-note ^gtd Task Link. Includes #ref / ^ref reading tasks as the ref part."
limits:
  - Reopened tasks leave Done.
  - Deleted tasks disappear from Created.
  - Recurring rows and the daily-note ^gtd row are excluded.
  - Missing dates are never imputed.
"##,
    ),
    (
        "created",
        r##"
label: Created
kind: tasks
event: created
recurring: exclude
unit: count
direction: neutral
color: blue
description: "Non-recurring tasks whose created date is D, any status, same exclusions as Done. Includes #ref / ^ref reading tasks as the ref part."
limits:
  - Reopened tasks leave Done.
  - Deleted tasks disappear from Created.
  - Recurring rows and the daily-note ^gtd row are excluded.
  - Missing dates are never imputed.
"##,
    ),
    (
        "net",
        r##"
label: Net flow
kind: formula
expr: created - done - dropped
unit: signed
direction: down
color: gray
description: "created - done - dropped."
"##,
    ),
    (
        "review_closed",
        r##"
label: Review closed
kind: checklist
tags: [gtd, post]
match: Morning review
measure: bool
unit: bool
direction: up
color: violet
description: "The #gtd #post Morning review checklist chore was completed on D."
limits:
  - Renaming a chore changes its identity.
"##,
    ),
    (
        "dropped",
        r##"
label: Dropped
kind: tasks
event: dropped
recurring: exclude
unit: count
direction: neutral
color: gray
description: "Non-recurring tasks with status type CANCELLED and cancelled date D, same exclusions as Done. Includes #ref / ^ref reading tasks as the ref part."
limits:
  - Reopened tasks leave Done.
  - Deleted tasks disappear from Created.
  - Recurring rows and the daily-note ^gtd row are excluded.
  - Missing dates are never imputed.
"##,
    ),
    (
        "habits",
        r##"
label: Habits
kind: checklist
tags: [gtd, pre]
measure: ratio
unit: ratio
direction: up
color: green
description: "#gtd #pre checklist chores met on D divided by chores due on D."
limits:
  - Renaming a chore changes its identity.
"##,
    ),
    (
        "keep_rate",
        r##"
label: Keep rate
kind: journal
select: keep_rate
tiers: [pending, next]
unit: ratio
direction: neutral
color: violet
description: "Among PENDING and NEXT rows answered on D (last answer per key), the share whose answer was keep."
limits:
  - Tracking starts at the first recorded answer.
"##,
    ),
    (
        "review_time",
        r##"
label: Review time
kind: pomodoro
measure: minutes
themes: [GTD]
unit: minutes
direction: neutral
color: orange
description: "Worked restricted to sessions themed GTD."
limits:
  - Attributed from Task Links.
  - Grouped by today's note tree.
  - Sessions with several homes are split evenly by task.
  - Links show what a session was for, not a stopwatch.
"##,
    ),
    (
        "sessions",
        r##"
label: Sessions
kind: pomodoro
measure: count
unit: count
direction: neutral
color: orange
description: "Closed Pomodoros on D."
limits:
  - Attributed from Task Links.
  - Grouped by today's note tree.
  - Sessions with several homes are split evenly by task.
  - Links show what a session was for, not a stopwatch.
"##,
    ),
];

pub(crate) fn builtin_ids() -> impl Iterator<Item = &'static str> {
    BUILTINS.iter().map(|(id, _)| *id)
}

pub(crate) fn build_registry(config: &StatsConfig) -> MetricRegistry {
    let mut warnings = Vec::new();
    let mut by_id: BTreeMap<String, MetricDef> = BTreeMap::new();
    let mut order: Vec<String> = Vec::new();
    let builtin_ids: HashSet<&str> =
        BUILTINS.iter().map(|(id, _)| *id).collect();

    for (id, yaml) in BUILTINS {
        let value: serde_yaml::Value = serde_yaml::from_str(yaml)
            .unwrap_or_else(|error| {
                panic!("builtin metric {id} yaml: {error}")
            });
        match parse_metric(id, &value, MetricOrigin::Builtin, &config.path) {
            Ok(def) => {
                order.push(id.to_string());
                by_id.insert((*id).to_string(), def);
            }
            Err(errors) => {
                panic!("builtin metric {id} must parse: {}", errors.join("; "))
            }
        }
    }

    for (id, value) in &config.user_metrics {
        if !is_metric_id(id) {
            warnings.push(metric_warning(
                id,
                format!(
                    "stats.metrics.{id} in {} must be a lowercase id matching [a-z][a-z0-9_]*",
                    config.path.display()
                ),
            ));
            if builtin_ids.contains(id.as_str()) {
                by_id.remove(id);
                order.retain(|existing| existing != id);
            }
            continue;
        }
        let origin = if builtin_ids.contains(id.as_str()) {
            MetricOrigin::Redefined
        } else {
            MetricOrigin::Custom
        };
        match parse_metric(id, value, origin, &config.path) {
            Ok(def) => {
                if !by_id.contains_key(id) {
                    order.push(id.clone());
                }
                by_id.insert(id.clone(), def);
            }
            Err(errors) => {
                for message in errors {
                    warnings.push(metric_warning(id, message));
                }
                by_id.remove(id);
                order.retain(|existing| existing != id);
            }
        }
    }

    invalidate_formulas(&mut by_id, &mut order, &mut warnings, &config.path);

    for id in &config.overview {
        if !by_id.contains_key(id) {
            warnings.push(metric_warning(
                id,
                format!(
                    "stats.overview in {} names unknown or invalid metric {id}",
                    config.path.display()
                ),
            ));
        }
    }
    for id in &config.hide {
        if !by_id.contains_key(id) {
            warnings.push(metric_warning(
                id,
                format!(
                    "stats.hide in {} names unknown or invalid metric {id}",
                    config.path.display()
                ),
            ));
        }
    }

    let evaluation_order = evaluation_order(&by_id, &order);
    let defs = order
        .iter()
        .filter_map(|id| by_id.get(id).cloned())
        .collect();
    MetricRegistry {
        defs,
        evaluation_order,
        hidden: config.hide.iter().cloned().collect(),
        overview: config.overview.clone(),
        warnings,
    }
}

fn invalidate_formulas(
    by_id: &mut BTreeMap<String, MetricDef>,
    order: &mut Vec<String>,
    warnings: &mut Vec<StatsWarning>,
    path: &Path,
) {
    loop {
        let available: HashSet<String> = by_id.keys().cloned().collect();
        let mut drop = Vec::new();
        for (id, def) in by_id.iter() {
            let deps = def.spec.dependencies();
            if deps.is_empty() {
                continue;
            }
            if let Some(missing) =
                deps.iter().find(|dep| !available.contains(dep.as_str()))
            {
                drop.push((
                    id.clone(),
                    format!(
                        "stats.metrics.{id}.expr in {} references unknown or invalid metric {missing}",
                        path.display()
                    ),
                ));
                continue;
            }
        }
        let cycles = formula_cycles(by_id);
        for id in cycles {
            drop.push((
                id.clone(),
                format!(
                    "stats.metrics.{id}.expr in {} has a dependency cycle",
                    path.display()
                ),
            ));
        }
        drop.sort_by(|left, right| left.0.cmp(&right.0));
        drop.dedup_by(|left, right| left.0 == right.0);
        if drop.is_empty() {
            break;
        }
        for (id, message) in drop {
            warnings.push(metric_warning(&id, message));
            by_id.remove(&id);
            order.retain(|existing| existing != &id);
        }
    }
}

fn formula_cycles(by_id: &BTreeMap<String, MetricDef>) -> Vec<String> {
    #[derive(Clone, Copy, PartialEq, Eq)]
    enum Color {
        White,
        Gray,
        Black,
    }
    let mut color: BTreeMap<&str, Color> =
        by_id.keys().map(|id| (id.as_str(), Color::White)).collect();
    let mut cyclic = BTreeSet::new();

    fn visit<'a>(
        id: &'a str,
        by_id: &'a BTreeMap<String, MetricDef>,
        color: &mut BTreeMap<&'a str, Color>,
        cyclic: &mut BTreeSet<String>,
    ) {
        color.insert(id, Color::Gray);
        if let Some(def) = by_id.get(id) {
            for dep in def.spec.dependencies() {
                match color.get(dep.as_str()).copied().unwrap_or(Color::Black) {
                    Color::Gray => {
                        cyclic.insert(id.to_string());
                        cyclic.insert(dep.clone());
                    }
                    Color::White => {
                        visit(dep, by_id, color, cyclic);
                        if cyclic.contains(dep) {
                            cyclic.insert(id.to_string());
                        }
                    }
                    Color::Black => {}
                }
            }
        }
        color.insert(id, Color::Black);
    }

    for id in by_id.keys() {
        if color.get(id.as_str()) == Some(&Color::White) {
            visit(id, by_id, &mut color, &mut cyclic);
        }
    }
    cyclic.into_iter().collect()
}

fn evaluation_order(
    by_id: &BTreeMap<String, MetricDef>,
    preferred: &[String],
) -> Vec<String> {
    let mut indegree: BTreeMap<&str, usize> =
        by_id.keys().map(|id| (id.as_str(), 0usize)).collect();
    for def in by_id.values() {
        for dep in def.evaluation_dependencies() {
            if by_id.contains_key(dep) {
                *indegree.entry(def.id.as_str()).or_insert(0) += 1;
            }
        }
    }
    let mut remaining: BTreeSet<&str> =
        by_id.keys().map(String::as_str).collect();
    let mut ordered = Vec::new();
    while !remaining.is_empty() {
        let next = preferred.iter().map(String::as_str).find(|id| {
            remaining.contains(id)
                && indegree.get(id).copied().unwrap_or(0) == 0
        });
        let Some(id) = next else {
            break;
        };
        remaining.remove(id);
        ordered.push(id.to_string());
        for other in by_id.values() {
            if other.evaluation_dependencies().contains(&id)
                && let Some(value) = indegree.get_mut(other.id.as_str())
            {
                *value = value.saturating_sub(1);
            }
        }
    }
    ordered.extend(remaining.into_iter().map(str::to_string));
    ordered
}

fn parse_metric(
    id: &str,
    value: &serde_yaml::Value,
    origin: MetricOrigin,
    path: &Path,
) -> Result<MetricDef, Vec<String>> {
    let mut errors = Vec::new();
    let Some(mapping) = value.as_mapping() else {
        return Err(vec![format!(
            "stats.metrics.{id} in {} must be a mapping",
            path.display()
        )]);
    };

    let label = required_string(mapping, "label", id, path, &mut errors);
    let kind = required_string(mapping, "kind", id, path, &mut errors);
    let spec = kind
        .as_deref()
        .and_then(|kind| parse_spec(kind, mapping, id, path, &mut errors));
    let Some(spec) = spec else {
        return Err(errors);
    };

    let unit = match optional_enum(
        mapping,
        "unit",
        id,
        path,
        &mut errors,
        parse_unit,
        "count, signed, minutes, bool, or ratio",
    ) {
        Some(unit) if !unit_compatible(&spec, unit) => {
            errors.push(format!(
                "stats.metrics.{id}.unit in {} is incompatible with kind {}",
                path.display(),
                spec.kind_name()
            ));
            unit
        }
        Some(unit) => unit,
        None => default_unit(&spec),
    };
    let direction = optional_enum(
        mapping,
        "direction",
        id,
        path,
        &mut errors,
        parse_direction,
        "up, down, or neutral",
    )
    .unwrap_or_else(|| default_direction(&spec));
    let color = optional_enum(
        mapping,
        "color",
        id,
        path,
        &mut errors,
        parse_color,
        "orange, violet, green, blue, or gray",
    )
    .unwrap_or_else(|| default_color(&spec));
    let since = optional_since(mapping, id, path, &mut errors);
    let definition =
        optional_string(mapping, "description", id, path, &mut errors)
            .or_else(|| {
                optional_string(mapping, "definition", id, path, &mut errors)
            })
            .unwrap_or_default();
    let mut limits =
        optional_string_list(mapping, "limits", id, path, &mut errors)
            .unwrap_or_default();
    if matches!(spec, MetricSpec::Pomodoro { .. }) {
        for line in [
            "Attributed from Task Links.",
            "Grouped by today's note tree.",
            "Sessions with several homes are split evenly by task.",
            "Links show what a session was for, not a stopwatch.",
        ] {
            if !limits.iter().any(|existing| existing == line) {
                limits.push(line.to_string());
            }
        }
    }

    if !errors.is_empty() {
        return Err(errors);
    }
    let Some(label) = label else {
        return Err(errors);
    };
    let source = default_source(&spec);

    Ok(MetricDef {
        id: id.to_string(),
        label,
        spec,
        unit,
        direction,
        color,
        source,
        origin,
        since,
        definition,
        limits,
    })
}

fn parse_spec(
    kind: &str,
    mapping: &serde_yaml::Mapping,
    id: &str,
    path: &Path,
    errors: &mut Vec<String>,
) -> Option<MetricSpec> {
    match kind {
        "pomodoro" => Some(parse_pomodoro(mapping, id, path, errors)),
        "tasks" => Some(parse_tasks(mapping, id, path, errors)),
        "checklist" => Some(parse_checklist(mapping, id, path, errors)),
        "field" => parse_field(mapping, id, path, errors),
        "formula" => parse_formula_spec(mapping, id, path, errors),
        "journal" => parse_journal(mapping, id, path, errors),
        other => {
            errors.push(format!(
                "stats.metrics.{id}.kind in {} must be pomodoro, tasks, \
                 checklist, field, formula, or journal; got {other:?}",
                path.display()
            ));
            None
        }
    }
}

fn parse_pomodoro(
    mapping: &serde_yaml::Mapping,
    id: &str,
    path: &Path,
    errors: &mut Vec<String>,
) -> MetricSpec {
    let measure = optional_enum(
        mapping,
        "measure",
        id,
        path,
        errors,
        |text| match text {
            "minutes" => Some(PomodoroMeasure::Minutes),
            "count" => Some(PomodoroMeasure::Count),
            _ => None,
        },
        "minutes or count",
    )
    .unwrap_or(PomodoroMeasure::Minutes);
    MetricSpec::Pomodoro {
        measure,
        themes: optional_string_list(mapping, "themes", id, path, errors)
            .unwrap_or_default(),
        exclude_themes: optional_string_list(
            mapping,
            "exclude_themes",
            id,
            path,
            errors,
        )
        .unwrap_or_default(),
    }
}

fn parse_tasks(
    mapping: &serde_yaml::Mapping,
    id: &str,
    path: &Path,
    errors: &mut Vec<String>,
) -> MetricSpec {
    let event = required_enum(
        mapping,
        "event",
        id,
        path,
        errors,
        |text| match text {
            "created" => Some(TaskEvent::Created),
            "done" => Some(TaskEvent::Done),
            "dropped" => Some(TaskEvent::Dropped),
            _ => None,
        },
        "created, done, or dropped",
    )
    .unwrap_or(TaskEvent::Done);
    let recurring = optional_enum(
        mapping,
        "recurring",
        id,
        path,
        errors,
        |text| match text {
            "exclude" => Some(RecurringFilter::Exclude),
            "only" => Some(RecurringFilter::Only),
            "include" => Some(RecurringFilter::Include),
            _ => None,
        },
        "exclude, only, or include",
    )
    .unwrap_or(RecurringFilter::Exclude);
    let task_kind = optional_enum(
        mapping,
        "task_kind",
        id,
        path,
        errors,
        |text| match text {
            "task" => Some(TaskKindFilter::Task),
            "prj" => Some(TaskKindFilter::Prj),
            "ref" => Some(TaskKindFilter::Ref),
            _ => None,
        },
        "task, prj, or ref",
    );
    MetricSpec::Tasks {
        event,
        tags_any: optional_string_list(mapping, "tags_any", id, path, errors)
            .unwrap_or_default(),
        tags_all: optional_string_list(mapping, "tags_all", id, path, errors)
            .unwrap_or_default(),
        in_notes: optional_string_list(mapping, "in", id, path, errors)
            .unwrap_or_default(),
        task_kind,
        recurring,
    }
}

fn parse_checklist(
    mapping: &serde_yaml::Mapping,
    id: &str,
    path: &Path,
    errors: &mut Vec<String>,
) -> MetricSpec {
    let measure = optional_enum(
        mapping,
        "measure",
        id,
        path,
        errors,
        |text| match text {
            "ratio" => Some(ChecklistMeasure::Ratio),
            "count" => Some(ChecklistMeasure::Count),
            "bool" => Some(ChecklistMeasure::Bool),
            _ => None,
        },
        "ratio, count, or bool",
    )
    .unwrap_or(ChecklistMeasure::Ratio);
    MetricSpec::Checklist {
        tags: optional_string_list(mapping, "tags", id, path, errors)
            .unwrap_or_default(),
        match_text: optional_string(mapping, "match", id, path, errors),
        measure,
    }
}

fn parse_field(
    mapping: &serde_yaml::Mapping,
    id: &str,
    path: &Path,
    errors: &mut Vec<String>,
) -> Option<MetricSpec> {
    let field = required_string(mapping, "field", id, path, errors)?;
    let source = optional_enum(
        mapping,
        "source",
        id,
        path,
        errors,
        |text| match text {
            "frontmatter" => Some(FieldSource::Frontmatter),
            "inline" => Some(FieldSource::Inline),
            "either" => Some(FieldSource::Either),
            _ => None,
        },
        "frontmatter, inline, or either",
    )
    .unwrap_or(FieldSource::Either);
    Some(MetricSpec::Field { field, source })
}

fn parse_formula_spec(
    mapping: &serde_yaml::Mapping,
    id: &str,
    path: &Path,
    errors: &mut Vec<String>,
) -> Option<MetricSpec> {
    let expr = required_string(mapping, "expr", id, path, errors)?;
    match parse_formula(&expr) {
        Ok(ast) => {
            let dependencies = ast.dependencies();
            Some(MetricSpec::Formula {
                expr,
                ast,
                dependencies,
            })
        }
        Err(message) => {
            errors.push(format!(
                "stats.metrics.{id}.expr in {} {message}",
                path.display()
            ));
            None
        }
    }
}

fn parse_journal(
    mapping: &serde_yaml::Mapping,
    id: &str,
    path: &Path,
    errors: &mut Vec<String>,
) -> Option<MetricSpec> {
    let select = required_enum(
        mapping,
        "select",
        id,
        path,
        errors,
        |text| match text {
            "reviewed" => Some(JournalSelect::Reviewed),
            "keep_rate" => Some(JournalSelect::KeepRate),
            _ => None,
        },
        "reviewed or keep_rate",
    )?;
    let tiers = parse_tiers(mapping, id, path, errors)
        .unwrap_or_else(|| default_tiers(select));
    Some(MetricSpec::Journal { select, tiers })
}

fn parse_tiers(
    mapping: &serde_yaml::Mapping,
    id: &str,
    path: &Path,
    errors: &mut Vec<String>,
) -> Option<JournalTiers> {
    let value = mapping_get(mapping, "tiers")?;
    if value.is_null() {
        return None;
    }
    if let Some(text) = value.as_str() {
        return match text.trim() {
            "decision" => Some(JournalTiers::Decision),
            "all" => Some(JournalTiers::All),
            other => {
                errors.push(format!(
                    "stats.metrics.{id}.tiers in {} must be decision, all, \
                     or a list of tier names; got {other:?}",
                    path.display()
                ));
                None
            }
        };
    }
    if let Some(items) =
        optional_string_list(mapping, "tiers", id, path, errors)
    {
        return Some(JournalTiers::List(items));
    }
    errors.push(format!(
        "stats.metrics.{id}.tiers in {} must be decision, all, or a list of \
         tier names",
        path.display()
    ));
    None
}

fn default_tiers(select: JournalSelect) -> JournalTiers {
    match select {
        JournalSelect::Reviewed => JournalTiers::Decision,
        JournalSelect::KeepRate => {
            JournalTiers::List(vec!["pending".to_string(), "next".to_string()])
        }
    }
}

fn default_unit(spec: &MetricSpec) -> MetricUnit {
    match spec {
        MetricSpec::Pomodoro {
            measure: PomodoroMeasure::Minutes,
            ..
        } => MetricUnit::Minutes,
        MetricSpec::Pomodoro {
            measure: PomodoroMeasure::Count,
            ..
        } => MetricUnit::Count,
        MetricSpec::Tasks { .. } => MetricUnit::Count,
        MetricSpec::Checklist {
            measure: ChecklistMeasure::Ratio,
            ..
        } => MetricUnit::Ratio,
        MetricSpec::Checklist {
            measure: ChecklistMeasure::Count,
            ..
        } => MetricUnit::Count,
        MetricSpec::Checklist {
            measure: ChecklistMeasure::Bool,
            ..
        } => MetricUnit::Bool,
        MetricSpec::Field { .. } => MetricUnit::Count,
        MetricSpec::Formula { .. } => MetricUnit::Count,
        MetricSpec::Journal {
            select: JournalSelect::Reviewed,
            ..
        } => MetricUnit::Count,
        MetricSpec::Journal {
            select: JournalSelect::KeepRate,
            ..
        } => MetricUnit::Ratio,
    }
}

fn default_color(spec: &MetricSpec) -> PaletteSlot {
    match spec {
        MetricSpec::Pomodoro { .. } => PaletteSlot::Orange,
        MetricSpec::Journal { .. } => PaletteSlot::Violet,
        MetricSpec::Checklist { .. } => PaletteSlot::Green,
        MetricSpec::Tasks {
            event: TaskEvent::Done,
            ..
        } => PaletteSlot::Green,
        MetricSpec::Tasks {
            event: TaskEvent::Created,
            ..
        } => PaletteSlot::Blue,
        MetricSpec::Tasks {
            event: TaskEvent::Dropped,
            ..
        }
        | MetricSpec::Formula { .. }
        | MetricSpec::Field { .. } => PaletteSlot::Gray,
    }
}

fn default_direction(spec: &MetricSpec) -> Direction {
    match spec {
        MetricSpec::Tasks {
            event: TaskEvent::Created | TaskEvent::Dropped,
            ..
        } => Direction::Neutral,
        _ => Direction::Up,
    }
}

fn default_source(spec: &MetricSpec) -> MetricSource {
    match spec {
        MetricSpec::Journal { .. } => MetricSource::Recorded,
        _ => MetricSource::Derived,
    }
}

fn unit_compatible(spec: &MetricSpec, unit: MetricUnit) -> bool {
    match spec {
        MetricSpec::Pomodoro {
            measure: PomodoroMeasure::Minutes,
            ..
        } => unit == MetricUnit::Minutes,
        MetricSpec::Pomodoro {
            measure: PomodoroMeasure::Count,
            ..
        } => unit == MetricUnit::Count,
        MetricSpec::Tasks { .. } => unit == MetricUnit::Count,
        MetricSpec::Checklist {
            measure: ChecklistMeasure::Ratio,
            ..
        } => unit == MetricUnit::Ratio,
        MetricSpec::Checklist {
            measure: ChecklistMeasure::Count,
            ..
        } => unit == MetricUnit::Count,
        MetricSpec::Checklist {
            measure: ChecklistMeasure::Bool,
            ..
        } => unit == MetricUnit::Bool,
        MetricSpec::Field { .. } => {
            matches!(
                unit,
                MetricUnit::Count | MetricUnit::Minutes | MetricUnit::Bool
            )
        }
        MetricSpec::Formula { .. } => unit != MetricUnit::Ratio,
        MetricSpec::Journal {
            select: JournalSelect::Reviewed,
            ..
        } => unit == MetricUnit::Count,
        MetricSpec::Journal {
            select: JournalSelect::KeepRate,
            ..
        } => unit == MetricUnit::Ratio,
    }
}

fn parse_unit(text: &str) -> Option<MetricUnit> {
    match text {
        "count" => Some(MetricUnit::Count),
        "signed" => Some(MetricUnit::Signed),
        "minutes" => Some(MetricUnit::Minutes),
        "bool" => Some(MetricUnit::Bool),
        "ratio" => Some(MetricUnit::Ratio),
        _ => None,
    }
}

fn parse_direction(text: &str) -> Option<Direction> {
    match text {
        "up" => Some(Direction::Up),
        "down" => Some(Direction::Down),
        "neutral" => Some(Direction::Neutral),
        _ => None,
    }
}

fn parse_color(text: &str) -> Option<PaletteSlot> {
    match text {
        "orange" => Some(PaletteSlot::Orange),
        "violet" => Some(PaletteSlot::Violet),
        "green" => Some(PaletteSlot::Green),
        "blue" => Some(PaletteSlot::Blue),
        "gray" => Some(PaletteSlot::Gray),
        _ => None,
    }
}

fn required_string(
    mapping: &serde_yaml::Mapping,
    key: &str,
    id: &str,
    path: &Path,
    errors: &mut Vec<String>,
) -> Option<String> {
    match mapping_get(mapping, key) {
        None | Some(serde_yaml::Value::Null) => {
            errors.push(format!(
                "stats.metrics.{id}.{key} in {} must be a non-empty string",
                path.display()
            ));
            None
        }
        Some(value) => match scalar_string(value) {
            Some(text) if !text.is_empty() => Some(text),
            _ => {
                errors.push(format!(
                    "stats.metrics.{id}.{key} in {} must be a non-empty string",
                    path.display()
                ));
                None
            }
        },
    }
}

fn optional_string(
    mapping: &serde_yaml::Mapping,
    key: &str,
    id: &str,
    path: &Path,
    errors: &mut Vec<String>,
) -> Option<String> {
    match mapping_get(mapping, key) {
        None | Some(serde_yaml::Value::Null) => None,
        Some(value) => match scalar_string(value) {
            Some(text) if !text.is_empty() => Some(text),
            Some(_) => None,
            None => {
                errors.push(format!(
                    "stats.metrics.{id}.{key} in {} must be a string",
                    path.display()
                ));
                None
            }
        },
    }
}

fn optional_string_list(
    mapping: &serde_yaml::Mapping,
    key: &str,
    id: &str,
    path: &Path,
    errors: &mut Vec<String>,
) -> Option<Vec<String>> {
    let value = mapping_get(mapping, key)?;
    if value.is_null() {
        return None;
    }
    if let Some(text) = scalar_string(value) {
        let text = text.trim();
        if text.is_empty() {
            errors.push(format!(
                "stats.metrics.{id}.{key} in {} must hold non-empty strings",
                path.display()
            ));
            return None;
        }
        return Some(vec![text.to_string()]);
    }
    let Some(items) = value.as_sequence() else {
        errors.push(format!(
            "stats.metrics.{id}.{key} in {} must hold non-empty strings",
            path.display()
        ));
        return None;
    };
    let mut parsed = Vec::new();
    for item in items {
        match scalar_string(item).map(|text| text.trim().to_string()) {
            Some(text) if !text.is_empty() => parsed.push(text),
            _ => {
                errors.push(format!(
                    "stats.metrics.{id}.{key} in {} must hold non-empty strings",
                    path.display()
                ));
                return None;
            }
        }
    }
    Some(parsed)
}

fn optional_since(
    mapping: &serde_yaml::Mapping,
    id: &str,
    path: &Path,
    errors: &mut Vec<String>,
) -> Option<NaiveDate> {
    let value = mapping_get(mapping, "since")?;
    if value.is_null() {
        return None;
    }
    match value.as_str().and_then(super::range::parse_iso_date) {
        Some(date) => Some(date),
        None => {
            errors.push(format!(
                "stats.metrics.{id}.since in {} must be YYYY-MM-DD",
                path.display()
            ));
            None
        }
    }
}

fn required_enum<T>(
    mapping: &serde_yaml::Mapping,
    key: &str,
    id: &str,
    path: &Path,
    errors: &mut Vec<String>,
    parse: impl Fn(&str) -> Option<T>,
    expected: &str,
) -> Option<T> {
    match mapping_get(mapping, key) {
        None | Some(serde_yaml::Value::Null) => {
            errors.push(format!(
                "stats.metrics.{id}.{key} in {} must be {expected}",
                path.display()
            ));
            None
        }
        Some(value) => match scalar_string(value).and_then(|text| parse(&text))
        {
            Some(parsed) => Some(parsed),
            None => {
                errors.push(format!(
                    "stats.metrics.{id}.{key} in {} must be {expected}",
                    path.display()
                ));
                None
            }
        },
    }
}

fn optional_enum<T>(
    mapping: &serde_yaml::Mapping,
    key: &str,
    id: &str,
    path: &Path,
    errors: &mut Vec<String>,
    parse: impl Fn(&str) -> Option<T>,
    expected: &str,
) -> Option<T> {
    let value = mapping_get(mapping, key)?;
    if value.is_null() {
        return None;
    }
    match scalar_string(value).and_then(|text| parse(&text)) {
        Some(parsed) => Some(parsed),
        None => {
            errors.push(format!(
                "stats.metrics.{id}.{key} in {} must be {expected}",
                path.display()
            ));
            None
        }
    }
}

fn scalar_string(value: &serde_yaml::Value) -> Option<String> {
    match value {
        serde_yaml::Value::String(text) => Some(text.trim().to_string()),
        serde_yaml::Value::Number(number) => Some(number.to_string()),
        serde_yaml::Value::Bool(flag) => Some(flag.to_string()),
        _ => None,
    }
}

fn mapping_get<'a>(
    mapping: &'a serde_yaml::Mapping,
    key: &str,
) -> Option<&'a serde_yaml::Value> {
    mapping.iter().find_map(|(candidate, value)| {
        candidate
            .as_str()
            .filter(|name| *name == key)
            .map(|_| value)
    })
}

fn is_metric_id(id: &str) -> bool {
    let mut chars = id.chars();
    matches!(chars.next(), Some('a'..='z'))
        && chars.all(|ch| {
            ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '_'
        })
}

fn metric_warning(id: &str, message: String) -> StatsWarning {
    StatsWarning {
        code: "invalid_metric".to_string(),
        metric: Some(id.to_string()),
        date: None,
        message,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::native::config::parse_stats_config;
    use crate::native::stats::request::MetricSelection;
    use std::path::Path;

    fn registry(text: &str) -> MetricRegistry {
        let config =
            parse_stats_config(text, Path::new("/config.yml")).unwrap();
        build_registry(&config)
    }

    fn default_registry() -> MetricRegistry {
        registry("stats:\n")
    }

    #[test]
    fn eleven_builtins_share_the_user_parser() {
        let registry = default_registry();
        let ids: Vec<_> =
            registry.defs.iter().map(|def| def.id.as_str()).collect();
        assert_eq!(
            ids,
            [
                "worked",
                "reviewed",
                "done",
                "created",
                "net",
                "review_closed",
                "dropped",
                "habits",
                "keep_rate",
                "review_time",
                "sessions",
            ]
        );
        assert!(registry.warnings.is_empty());
        assert_eq!(builtin_ids().count(), 11);
        for def in &registry.defs {
            assert_eq!(def.origin, MetricOrigin::Builtin);
            assert!(!def.unit.as_str().is_empty());
            assert!(!def.color.as_str().is_empty());
            assert!(!def.direction.as_str().is_empty());
            assert!(!def.source.as_str().is_empty());
            assert_eq!(def.origin.as_str(), "builtin");
        }

        let table = [
            (
                "worked",
                MetricUnit::Minutes,
                Direction::Up,
                PaletteSlot::Orange,
            ),
            (
                "reviewed",
                MetricUnit::Count,
                Direction::Up,
                PaletteSlot::Violet,
            ),
            ("done", MetricUnit::Count, Direction::Up, PaletteSlot::Green),
            (
                "created",
                MetricUnit::Count,
                Direction::Neutral,
                PaletteSlot::Blue,
            ),
            (
                "net",
                MetricUnit::Signed,
                Direction::Down,
                PaletteSlot::Gray,
            ),
            (
                "review_closed",
                MetricUnit::Bool,
                Direction::Up,
                PaletteSlot::Violet,
            ),
            (
                "dropped",
                MetricUnit::Count,
                Direction::Neutral,
                PaletteSlot::Gray,
            ),
            (
                "habits",
                MetricUnit::Ratio,
                Direction::Up,
                PaletteSlot::Green,
            ),
            (
                "keep_rate",
                MetricUnit::Ratio,
                Direction::Neutral,
                PaletteSlot::Violet,
            ),
            (
                "review_time",
                MetricUnit::Minutes,
                Direction::Neutral,
                PaletteSlot::Orange,
            ),
            (
                "sessions",
                MetricUnit::Count,
                Direction::Neutral,
                PaletteSlot::Orange,
            ),
        ];
        for (id, unit, direction, color) in table {
            let def = registry.get(id).unwrap();
            assert_eq!(def.unit, unit, "{id} unit");
            assert_eq!(def.direction, direction, "{id} direction");
            assert_eq!(def.color, color, "{id} color");
        }
        assert!(
            registry
                .evaluation_order
                .iter()
                .position(|id| id == "review_closed")
                .unwrap()
                < registry
                    .evaluation_order
                    .iter()
                    .position(|id| id == "reviewed")
                    .unwrap()
        );

        let worked = registry.get("worked").unwrap();
        assert_eq!(worked.unit, MetricUnit::Minutes);
        assert_eq!(worked.color, PaletteSlot::Orange);
        assert_eq!(worked.direction, Direction::Up);
        assert_eq!(worked.source, MetricSource::Derived);
        match &worked.spec {
            MetricSpec::Pomodoro {
                measure: PomodoroMeasure::Minutes,
                themes,
                ..
            } => assert!(themes.is_empty()),
            other => panic!("worked spec {other:?}"),
        }

        let net = registry.get("net").unwrap();
        assert_eq!(net.unit, MetricUnit::Signed);
        assert_eq!(net.direction, Direction::Down);
        match &net.spec {
            MetricSpec::Formula { dependencies, .. } => {
                assert_eq!(dependencies, &["created", "done", "dropped"]);
            }
            other => panic!("net spec {other:?}"),
        }

        let review_closed = registry.get("review_closed").unwrap();
        assert_eq!(review_closed.color, PaletteSlot::Violet);
        match &review_closed.spec {
            MetricSpec::Checklist {
                tags,
                match_text,
                measure: ChecklistMeasure::Bool,
            } => {
                assert_eq!(tags, &["gtd", "post"]);
                assert_eq!(match_text.as_deref(), Some("Morning review"));
            }
            other => panic!("review_closed spec {other:?}"),
        }

        let keep_rate = registry.get("keep_rate").unwrap();
        assert_eq!(keep_rate.source, MetricSource::Recorded);
        match &keep_rate.spec {
            MetricSpec::Journal {
                select: JournalSelect::KeepRate,
                tiers: JournalTiers::List(tiers),
            } => assert_eq!(tiers, &["pending", "next"]),
            other => panic!("keep_rate spec {other:?}"),
        }

        let done = registry.get("done").unwrap();
        match &done.spec {
            MetricSpec::Tasks {
                event: TaskEvent::Done,
                task_kind: None,
                recurring: RecurringFilter::Exclude,
                ..
            } => {}
            other => panic!("done spec {other:?}"),
        }
        assert!(
            registry
                .evaluation_order
                .iter()
                .position(|id| id == "dropped")
                .unwrap()
                < registry
                    .evaluation_order
                    .iter()
                    .position(|id| id == "net")
                    .unwrap()
        );
    }

    #[test]
    fn custom_builtin_id_replaces_the_whole_definition() {
        let registry = registry(
            "
stats:
  metrics:
    review_time:
      label: Ritual time
      kind: pomodoro
      measure: minutes
      themes: [GTD, ADMIN]
      unit: minutes
",
        );
        let def = registry.get("review_time").unwrap();
        assert_eq!(def.origin, MetricOrigin::Redefined);
        assert_eq!(def.label, "Ritual time");
        match &def.spec {
            MetricSpec::Pomodoro { themes, .. } => {
                assert_eq!(themes, &["GTD", "ADMIN"]);
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn invalid_redefinition_does_not_fall_back() {
        let registry = registry(
            "
stats:
  metrics:
    worked:
      label: Broken
      kind: nope
",
        );
        assert!(registry.get("worked").is_none());
        assert!(registry.get("reviewed").is_some());
        assert!(registry.warnings.iter().any(|warning| warning
            .message
            .contains("stats.metrics.worked.kind")));
    }

    #[test]
    fn malformed_metric_leaves_unrelated_entries() {
        let registry = registry(
            "
stats:
  metrics:
    energy:
      label: Energy
      kind: field
      field: energy
      unit: count
    bad: not-a-mapping
    also:
      kind: tasks
",
        );
        assert!(registry.get("energy").is_some());
        assert!(registry.get("bad").is_none());
        assert!(registry.get("also").is_none());
        assert!(registry.get("worked").is_some());
        assert!(registry
            .warnings
            .iter()
            .any(|warning| { warning.message.contains("stats.metrics.bad") }));
        assert!(registry
            .warnings
            .iter()
            .any(|warning| { warning.message.contains("stats.metrics.also") }));
    }

    #[test]
    fn task_kind_filter_is_not_the_metric_kind() {
        let registry = registry(
            "
stats:
  metrics:
    reading:
      label: Reading done
      kind: tasks
      event: done
      task_kind: ref
      in: [job]
",
        );
        let def = registry.get("reading").unwrap();
        assert_eq!(def.origin, MetricOrigin::Custom);
        match &def.spec {
            MetricSpec::Tasks {
                event: TaskEvent::Done,
                task_kind: Some(TaskKindFilter::Ref),
                in_notes,
                ..
            } => assert_eq!(in_notes, &["job"]),
            other => panic!("{other:?}"),
        }
        let json = serde_json::to_value(def.parameters()).unwrap();
        assert_eq!(json["task_kind"], "ref");
        assert_eq!(json["in"], serde_json::json!(["job"]));
        assert!(json.get("kind").is_none());
    }

    #[test]
    fn formula_unknown_refs_and_cycles_drop_dependents() {
        let registry = registry(
            "
stats:
  metrics:
    ghost:
      label: Ghost
      kind: formula
      expr: missing + 1
    downstream:
      label: Downstream
      kind: formula
      expr: ghost + worked
    loop_a:
      label: A
      kind: formula
      expr: loop_b + 1
    loop_b:
      label: B
      kind: formula
      expr: loop_a + 1
    ok:
      label: Ok
      kind: formula
      expr: worked + 1
",
        );
        assert!(registry.get("ghost").is_none());
        assert!(registry.get("downstream").is_none());
        assert!(registry.get("loop_a").is_none());
        assert!(registry.get("loop_b").is_none());
        assert!(registry.get("ok").is_some());
        assert!(registry.get("net").is_some());
        let messages: Vec<_> = registry
            .warnings
            .iter()
            .map(|warning| warning.message.as_str())
            .collect();
        assert!(messages.iter().any(
            |message| message.contains("unknown or invalid metric missing")
        ));
        assert!(messages
            .iter()
            .any(|message| message.contains("dependency cycle")));
    }

    #[test]
    fn hide_keeps_the_definition_for_deps_and_explicit_select() {
        let registry = registry(
            "
stats:
  hide: [dropped, sessions]
",
        );
        assert!(registry.is_hidden("dropped"));
        assert!(registry.get("dropped").is_some());
        assert!(registry.get("net").is_some());
        let overview = registry
            .select(&MetricSelection::Overview)
            .unwrap()
            .into_iter()
            .map(|def| def.id.as_str())
            .collect::<Vec<_>>();
        assert_eq!(
            overview,
            [
                "worked",
                "reviewed",
                "done",
                "created",
                "net",
                "review_closed"
            ]
        );
        let visible: Vec<_> = registry
            .select(&MetricSelection::AllVisible)
            .unwrap()
            .into_iter()
            .map(|def| def.id.as_str())
            .collect();
        assert!(!visible.contains(&"dropped"));
        assert!(!visible.contains(&"sessions"));
        let explicit = registry
            .select(&MetricSelection::Explicit(vec!["dropped".to_string()]))
            .unwrap();
        assert_eq!(explicit[0].id, "dropped");
        let net = registry.get("net").unwrap();
        let eval = registry.with_dependencies(&[net]);
        let eval_ids: Vec<_> = eval.iter().map(|def| def.id.as_str()).collect();
        assert!(eval_ids.contains(&"dropped"));
        assert!(eval_ids.contains(&"net"));
    }

    #[test]
    fn unknown_explicit_metric_is_a_request_error() {
        let registry = default_registry();
        let error = registry
            .select(&MetricSelection::Explicit(vec!["nope".to_string()]))
            .unwrap_err();
        assert_eq!(error.code(), "unknown_metric");
        assert_eq!(error.exit_code(), 2);
    }

    #[test]
    fn invalidating_dropped_invalidates_net() {
        let registry = registry(
            "
stats:
  metrics:
    dropped:
      label: Broken
      kind: tasks
",
        );
        assert!(registry.get("dropped").is_none());
        assert!(registry.get("net").is_none());
        assert!(registry.get("created").is_some());
        assert!(registry.get("done").is_some());
    }

    #[test]
    fn rejects_bad_task_kind_hex_color_and_formula_ratio_unit() {
        let registry = registry(
            "
stats:
  metrics:
    books:
      label: Books
      kind: tasks
      event: done
      task_kind: book
    painted:
      label: Painted
      kind: pomodoro
      color: '#ff00aa'
    ratioed:
      label: Ratioed
      kind: formula
      expr: 1 + 1
      unit: ratio
",
        );
        assert!(registry.get("books").is_none());
        assert!(registry.get("painted").is_none());
        assert!(registry.get("ratioed").is_none());
        let messages: Vec<_> = registry
            .warnings
            .iter()
            .map(|warning| warning.message.as_str())
            .collect();
        assert!(messages.iter().any(|message| {
            message.contains("stats.metrics.books.task_kind")
                && message.contains("task, prj, or ref")
        }));
        assert!(messages.iter().any(|message| {
            message.contains("stats.metrics.painted.color")
                && message.contains("orange, violet, green, blue, or gray")
        }));
        assert!(messages.iter().any(|message| {
            message.contains("stats.metrics.ratioed.unit")
                && message.contains("incompatible with kind formula")
        }));
    }

    #[test]
    fn non_string_metric_key_is_an_invalid_id() {
        let registry = registry(
            "
stats:
  metrics:
    2026:
      label: Year
      kind: formula
      expr: 1
",
        );
        assert!(registry.get("2026").is_none());
        assert!(registry.warnings.iter().any(|warning| {
            warning.metric.as_deref() == Some("2026")
                && warning
                    .message
                    .contains("must be a lowercase id matching [a-z][a-z0-9_]*")
        }));
    }

    #[test]
    fn selecting_an_invalidated_metric_includes_the_registry_warning() {
        let registry = registry(
            "
stats:
  metrics:
    dropped:
      label: Broken
      kind: nope
",
        );
        let error = registry
            .select(&MetricSelection::Explicit(vec!["dropped".to_string()]))
            .unwrap_err();
        assert_eq!(error.code(), "unknown_metric");
        assert!(error.message().contains("stats.metrics.dropped.kind"));
    }
}
