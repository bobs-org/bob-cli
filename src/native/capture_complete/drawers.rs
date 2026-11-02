//! Drawer completion for a glued `@route+id#…` selector or a first-level
//! bullet's leading `#`: the target's own drawers (`task`), catalog drawers
//! it lacks (`recent`), and one `new` row for a typed name nothing else
//! claims.
//!
//! Every row's `replacement` is the exact text a commit would receive (a
//! `#slug` / `#"TITLE"` selector for the glued form, the bare ALL-CAPS
//! title for a bullet), and its `refused` reason is the commit's own
//! resolver decision (`resolve_drawer_for_write` / `resolve_h2_for_write`),
//! never a parallel rule. A menu choice is therefore never authority to
//! skip commit-time resolution.

use std::{
    collections::{BTreeMap, HashSet},
    fs, io,
    path::Path,
};

use super::model::{Candidates, CompleteError, DrawerCandidate, DrawerTarget};
use crate::native::{
    capture,
    capture_drawers::{
        self, Drawer, DrawerCatalogRecord, DrawerMatch, DrawerResolveError,
        DrawerResolveOutcome, DrawerResolved, DrawerScope,
    },
    capture_language::{self, DrawerBulletTarget},
    capture_project_note,
    note_tasks::{self, BlockIdLookup},
};

pub(super) const GROUP_TASK: &str = "task";
pub(super) const GROUP_RECENT: &str = "recent";
pub(super) const GROUP_NEW: &str = "new";

/// How a menu row spells its `replacement`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RowStyle {
    /// A glued `@route+id#…` selector: `#slug` or `#"TITLE"`.
    Glued,
    /// A first-level bullet's body: the bare ALL-CAPS title.
    Bullet,
}

/// The full drawer menu for `@route+block-id#query`. `quoted` is true when
/// the typed selector opened a `"` literal.
pub(super) fn drawer_candidates(
    bob_dir: &Path,
    route: &str,
    block_id: Option<&str>,
    query: &str,
    quoted: bool,
) -> Result<(Candidates, Vec<String>), CompleteError> {
    match lookup_target(bob_dir, route, block_id)? {
        Lookup::Found(target) => Ok(menu(bob_dir, &target, query, quoted)),
        Lookup::Diagnostic(warnings) => {
            Ok((Candidates::Drawer(Vec::new()), warnings))
        }
    }
}

/// The drawer menu behind a first-level bullet's leading `#`. Existing
/// tasks offer their own drawers too; a new task or project note has none,
/// so it offers catalog (`recent`) and `new` rows only.
pub(super) fn bullet_drawer_candidates(
    bob_dir: &Path,
    bullet: &DrawerBulletTarget,
    query: &str,
    quoted: bool,
) -> Result<(Candidates, Vec<String>), CompleteError> {
    let target = match bullet {
        DrawerBulletTarget::ExistingTask { route, block_id } => {
            match lookup_target(bob_dir, route, Some(block_id))? {
                Lookup::Found(mut target) => {
                    target.style = RowStyle::Bullet;
                    target
                }
                Lookup::Diagnostic(warnings) => {
                    return Ok((Candidates::Drawer(Vec::new()), warnings));
                }
            }
        }
        fresh => Box::new(
            Target::fresh(fresh).expect("fresh bullet target has no lookup"),
        ),
    };
    Ok(menu(bob_dir, &target, query, quoted))
}

/// The `target` object a bullet drawer response carries for a task or
/// project note that does not exist yet. An existing task reads its
/// target from the vault like the glued menu does.
pub(super) fn fresh_bullet_target(
    bullet: &DrawerBulletTarget,
) -> Option<DrawerTarget> {
    let target = Target::fresh(bullet)?;
    Some(DrawerTarget {
        route: capture::route_label(&target.route),
        block_id: target.block_id,
        text: target.text,
    })
}

/// One menu over a resolved target: its own drawers, the catalog drawers it
/// lacks, and the single `new` row.
fn menu(
    bob_dir: &Path,
    target: &Target,
    query: &str,
    quoted: bool,
) -> (Candidates, Vec<String>) {
    let mut warnings = Vec::new();
    // One cached catalog load per request; the target's own rows read only
    // the target note.
    let catalog = match capture_drawers::load_drawer_catalog(bob_dir) {
        Ok(catalog) => catalog,
        Err(error) => {
            warnings.push(format!("drawer catalog unavailable: {error}"));
            BTreeMap::new()
        }
    };

    let mut rows = target.task_rows(query, &catalog);
    let on_task: HashSet<String> = target
        .drawers
        .iter()
        .map(|drawer| drawer.key.clone())
        .collect();
    let records = if query.trim().is_empty() {
        capture_drawers::drawer_catalog_recent(&catalog, &on_task)
    } else {
        capture_drawers::drawer_catalog_search(&catalog, query, &on_task)
    };
    let now = capture_drawers::drawer_now_unix();
    for record in records {
        rows.push(target.recent_row(record, &catalog, now));
    }
    if let Some(row) = target.new_row(query, quoted, &rows, &catalog) {
        rows.push(row);
    }
    (Candidates::Drawer(rows), warnings)
}

/// Only the target's own drawers, for shell completion: no catalog scan,
/// no query, and no refused rows. Shell completion serves one-line drafts
/// only, so it never reaches a bullet start and only spells glued
/// selectors.
pub(super) fn target_drawer_candidates(
    bob_dir: &Path,
    route: &str,
    block_id: Option<&str>,
) -> Result<(Candidates, Vec<String>), CompleteError> {
    let target = match lookup_target(bob_dir, route, block_id)? {
        Lookup::Found(target) => target,
        Lookup::Diagnostic(warnings) => {
            return Ok((Candidates::Drawer(Vec::new()), warnings));
        }
    };
    let rows = target
        .task_rows("", &BTreeMap::new())
        .into_iter()
        .filter(|row| row.refused.is_none())
        .collect();
    Ok((Candidates::Drawer(rows), Vec::new()))
}

/// Open the nested `  - ` bullet right after an accepted row's title, so
/// the caret lands where the drawer's first note goes.
pub(super) fn append_continuation(candidates: &mut Candidates) {
    if let Candidates::Drawer(rows) = candidates {
        for row in rows {
            row.replacement.push_str("\n  - ");
        }
    }
}

/// Each row's `cursor_after`: the draft offset just past its replacement,
/// which starts at `replacement_start` (the `#`).
pub(super) fn place_cursor_after(
    candidates: &mut Candidates,
    replacement_start: usize,
) {
    if let Candidates::Drawer(rows) = candidates {
        for row in rows {
            row.cursor_after = Some(replacement_start + row.replacement.len());
        }
    }
}

enum Lookup {
    Found(Box<Target>),
    /// No usable target: the diagnostic (if any) and no rows, never a
    /// plausible create row.
    Diagnostic(Vec<String>),
}

/// A resolved drawer target: the task (or the project note's own `^prj`)
/// and every drawer a selector can name on it.
struct Target {
    route: String,
    block_id: Option<String>,
    text: String,
    contents: String,
    scope: DrawerScope,
    /// Every eligible drawer on the target, unfiltered: ambiguity and
    /// round trips are checked against the whole list.
    drawers: Vec<Drawer>,
    style: RowStyle,
    /// A project note this capture is about to create. Its writer turns
    /// every ALL-CAPS bullet into a `## Title Case` section, structural
    /// and managed names included, so no resolver refusal applies.
    new_note: bool,
}

impl Target {
    /// A bullet target that does not exist in the vault yet: a new task
    /// (no drawers, no block ID) or a new project note (no sections yet).
    /// `None` for an existing task, which needs a vault lookup.
    fn fresh(bullet: &DrawerBulletTarget) -> Option<Self> {
        let (route, block_id, text, scope, new_note) = match bullet {
            DrawerBulletTarget::ExistingTask { .. } => return None,
            DrawerBulletTarget::NewTask { route, text } => (
                route
                    .clone()
                    .unwrap_or_else(|| capture::inbox_route().to_string()),
                None,
                text.clone(),
                DrawerScope::Task,
                false,
            ),
            DrawerBulletTarget::NewProjectNote {
                route,
                block_id,
                text,
            } => (
                capture_project_note::project_note_basename(route, block_id)
                    .trim_end_matches(".md")
                    .to_string(),
                Some("prj".to_string()),
                text.clone(),
                DrawerScope::ProjectSection,
                true,
            ),
        };
        Some(Self {
            route,
            block_id,
            text,
            contents: String::new(),
            scope,
            drawers: Vec::new(),
            style: RowStyle::Bullet,
            new_note,
        })
    }
}

fn lookup_target(
    bob_dir: &Path,
    route: &str,
    block_id: Option<&str>,
) -> Result<Lookup, CompleteError> {
    let Some(block_id) = block_id.filter(|id| !id.is_empty()) else {
        return Ok(Lookup::Diagnostic(Vec::new()));
    };
    let path = bob_dir.join(capture::route_label(route));
    let contents = match fs::read_to_string(&path) {
        Ok(contents) => contents,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Ok(Lookup::Diagnostic(vec![unresolvable_parent_warning(
                route,
                block_id,
                DrawerLookupFailure::MissingNote,
            )]));
        }
        Err(error) => {
            return Err(CompleteError::io(format!(
                "read target {}: {error}",
                path.display()
            )));
        }
    };

    let settings = note_tasks::read_settings(bob_dir);
    let scan = note_tasks::scan(&contents, &settings);
    let failure = match scan.by_block_id(block_id) {
        BlockIdLookup::Found(parent) => {
            // On the project note's own `^prj` task, the drawers are the
            // note's eligible `##` sections.
            let prj = capture_drawers::is_prj_parent(&contents, parent);
            let (scope, drawers) = if prj {
                (
                    DrawerScope::ProjectSection,
                    capture_drawers::project_h2_drawers(&contents),
                )
            } else {
                (
                    DrawerScope::Task,
                    capture_drawers::drawers(&contents, parent),
                )
            };
            let target = Target {
                route: route.to_string(),
                block_id: parent.block_id.clone(),
                text: parent.description.clone(),
                scope,
                drawers,
                contents,
                style: RowStyle::Glued,
                new_note: false,
            };
            return Ok(Lookup::Found(Box::new(target)));
        }
        BlockIdLookup::Missing => DrawerLookupFailure::Missing {
            suggestion: scan.suggest_block_id(block_id).map(str::to_string),
        },
        BlockIdLookup::Duplicate(count) => {
            DrawerLookupFailure::Duplicate(count)
        }
        BlockIdLookup::NotATask { .. } => DrawerLookupFailure::NotATask,
    };
    Ok(Lookup::Diagnostic(vec![unresolvable_parent_warning(
        route, block_id, failure,
    )]))
}

/// How a row's `replacement` was chosen and what the commit would do with
/// it.
struct Choice {
    /// `#slug` or `#"TITLE"`, covering the `#` and the name.
    replacement: String,
    resolved: Result<DrawerResolved, DrawerResolveError>,
}

/// What the row intends: an existing drawer (by line) or a new one.
#[derive(Clone, Copy)]
enum Want {
    Existing(usize),
    Create,
}

impl Target {
    fn resolve(
        &self,
        selector: &str,
        exact: bool,
        catalog: &BTreeMap<String, DrawerCatalogRecord>,
    ) -> Result<DrawerResolved, DrawerResolveError> {
        match self.scope {
            DrawerScope::Task => capture_drawers::resolve_drawer_for_write(
                &self.drawers,
                selector,
                exact,
                catalog,
                &self.route,
                self.block_id.as_deref(),
                DrawerScope::Task,
            ),
            DrawerScope::ProjectSection => {
                capture_drawers::resolve_h2_for_write(
                    &self.drawers,
                    selector,
                    exact,
                    catalog,
                    &self.route,
                    self.block_id.as_deref(),
                )
            }
        }
    }

    /// Pick the selector a commit should receive for `title`: `#slug` only
    /// when the slug resolves back to exactly the wanted drawer against the
    /// full target list, else `#"TITLE"`. A slug the commit refuses for a
    /// reason the quoted form would not share (reserved, near-duplicate)
    /// keeps the slug and carries the refusal.
    fn choose(
        &self,
        title: &str,
        want: Want,
        catalog: &BTreeMap<String, DrawerCatalogRecord>,
    ) -> Choice {
        if self.style == RowStyle::Bullet {
            return self.choose_bullet(title, want, catalog);
        }
        let slug = capture_language::selector_slug(title);
        if capture_drawers::is_drawer_slug(&slug) {
            match self.resolve(&slug, false, catalog) {
                Ok(resolved) if matches_want(&resolved, want, title) => {
                    return Choice {
                        replacement: format!("#{slug}"),
                        resolved: Ok(resolved),
                    };
                }
                Err(error)
                    if matches!(want, Want::Create)
                        && error.code != "ambiguous_drawer" =>
                {
                    return Choice {
                        replacement: format!("#{slug}"),
                        resolved: Err(error),
                    };
                }
                _ => {}
            }
        }
        Choice {
            replacement: format!("#\"{}\"", quote_escape(title)),
            resolved: self.resolve(title, true, catalog),
        }
    }

    /// A bullet row's replacement is the bare ALL-CAPS title, valid only
    /// when committing that literal title (an exact resolution, like any
    /// ALL-CAPS drawer bullet) reaches the row's intent: the same existing
    /// drawer, or a creation of the same key.
    fn choose_bullet(
        &self,
        title: &str,
        want: Want,
        catalog: &BTreeMap<String, DrawerCatalogRecord>,
    ) -> Choice {
        let Some(canonical) = capture_drawers::normalize_drawer_literal(title)
        else {
            return Choice {
                replacement: capture_drawers::drawer_key(title),
                resolved: Err(DrawerResolveError {
                    code: "unnameable_drawer",
                    message: unnameable_bullet_copy(self.scope),
                }),
            };
        };
        if self.new_note {
            // The writer owns a new note's sections: any ALL-CAPS title
            // becomes `## Title Case`, merging repeats.
            let key = capture_drawers::drawer_key(&canonical);
            return Choice {
                resolved: Ok(DrawerResolved {
                    outcome: DrawerResolveOutcome::Create {
                        title:
                            capture_project_note::format_project_section_title(
                                &canonical,
                            ),
                        slug: capture_language::selector_slug(&canonical),
                        key,
                    },
                    created: true,
                    warning: None,
                }),
                replacement: canonical,
            };
        }
        let resolved = match self.resolve(&canonical, true, catalog) {
            Ok(resolved) if !matches_want(&resolved, want, &canonical) => {
                Err(DrawerResolveError {
                    code: "drawer_mismatch",
                    message: format!(
                        "`{canonical}` would not reach this {} on commit",
                        scope_noun(self.scope)
                    ),
                })
            }
            other => other,
        };
        Choice {
            replacement: canonical,
            resolved,
        }
    }

    fn base_row(&self, title: &str, key: &str, group: &str) -> DrawerCandidate {
        DrawerCandidate {
            replacement: String::new(),
            title: title.to_string(),
            key: key.to_string(),
            slug: capture_language::selector_slug(title),
            route: self.route.clone(),
            block_id: self.block_id.clone(),
            text: self.text.clone(),
            line: 0,
            entry_count: 0,
            group: group.to_string(),
            note_count: None,
            last_seen: None,
            detail: String::new(),
            creates: group != GROUP_TASK,
            refused: None,
            consequence: None,
            cursor_after: None,
        }
    }

    /// Rows for the target's own drawers matching `query`, in document
    /// order within prefix then substring tiers.
    fn task_rows(
        &self,
        query: &str,
        catalog: &BTreeMap<String, DrawerCatalogRecord>,
    ) -> Vec<DrawerCandidate> {
        let mut tiers: [Vec<&Drawer>; 2] = [Vec::new(), Vec::new()];
        for drawer in &self.drawers {
            if query.trim().is_empty() {
                tiers[0].push(drawer);
                continue;
            }
            match capture_drawers::drawer_query_match(
                &drawer.key,
                &drawer.slug,
                query,
            ) {
                Some(DrawerMatch::Prefix) => tiers[0].push(drawer),
                Some(DrawerMatch::Substring) => tiers[1].push(drawer),
                None => {}
            }
        }
        tiers
            .into_iter()
            .flatten()
            .map(|drawer| self.task_row(drawer, catalog))
            .collect()
    }

    fn task_row(
        &self,
        drawer: &Drawer,
        catalog: &BTreeMap<String, DrawerCatalogRecord>,
    ) -> DrawerCandidate {
        let mut row = self.base_row(&drawer.title, &drawer.key, GROUP_TASK);
        row.line = drawer.line;
        row.entry_count = drawer.entry_count;
        row.detail = entry_count_detail(drawer.entry_count);
        let choice =
            self.choose(&drawer.title, Want::Existing(drawer.line), catalog);
        self.apply_choice(&mut row, choice, &drawer.title);
        if row.refused.is_none() {
            row.consequence = Some(match self.scope {
                DrawerScope::ProjectSection => {
                    capture_drawers::h2_consequence(&drawer.title)
                }
                DrawerScope::Task => {
                    match capture_drawers::drawer_last_entry_text(
                        &self.contents,
                        drawer,
                    ) {
                        Some(entry) => format!(
                            "Adds after \"{}\" in {}",
                            ellipsize(&entry, ENTRY_PREVIEW_CHARS),
                            drawer.title
                        ),
                        None => format!("Adds to {}", drawer.title),
                    }
                }
            });
        }
        row
    }

    fn recent_row(
        &self,
        record: &DrawerCatalogRecord,
        catalog: &BTreeMap<String, DrawerCatalogRecord>,
        now: u64,
    ) -> DrawerCandidate {
        let title = wanted_title(&record.title, &record.key);
        let mut row = self.base_row(&title, &record.key, GROUP_RECENT);
        row.note_count = Some(record.note_count);
        row.last_seen = Some(record.last_seen);
        row.detail = recent_detail(record.note_count, record.last_seen, now);
        let choice = self.choose(&title, Want::Create, catalog);
        self.apply_choice(&mut row, choice, &title);
        row
    }

    /// The one `new` row: only for a non-empty typed query whose name no
    /// other row (nor an existing key on the target) already claims.
    fn new_row(
        &self,
        query: &str,
        quoted: bool,
        rows: &[DrawerCandidate],
        catalog: &BTreeMap<String, DrawerCatalogRecord>,
    ) -> Option<DrawerCandidate> {
        if query.trim().is_empty() {
            return None;
        }
        let title = capture_drawers::normalize_drawer_literal(query)?;
        // A slug names either spelling; a quoted literal only its own key.
        let mut claimed = vec![capture_drawers::drawer_key(&title)];
        if !quoted {
            claimed.push(capture_drawers::drawer_key(&title.replace('-', " ")));
        }
        if rows.iter().any(|row| claimed.contains(&row.key)) {
            return None;
        }
        // A bullet commits the literal title, so an unquoted slug spells
        // the title the `#slug` shortcut would create: hyphens read as
        // spaces.
        let wanted = match self.style {
            RowStyle::Bullet if !quoted => {
                capture_drawers::normalize_drawer_literal(
                    &title.replace('-', " "),
                )
                .unwrap_or_else(|| title.clone())
            }
            _ => title,
        };
        let mut row = self.base_row(&wanted, &claimed[0], GROUP_NEW);
        let choice = self.choose(&wanted, Want::Create, catalog);
        self.apply_choice(&mut row, choice, &wanted);
        if row.refused.is_none()
            && let Some(near) =
                capture_drawers::near_catalog_title(&row.key, catalog)
        {
            row.detail = format!("did you mean {near}?");
        }
        Some(row)
    }

    /// Fill the commit-derived fields: replacement, refusal, and, for a
    /// create, the created title and consequence.
    fn apply_choice(
        &self,
        row: &mut DrawerCandidate,
        choice: Choice,
        wanted_title: &str,
    ) {
        row.replacement = choice.replacement;
        match choice.resolved {
            Err(error) => row.refused = Some(error.message),
            Ok(resolved) => {
                if !quoted_title_is_nameable(&row.replacement, wanted_title) {
                    row.refused = Some(unnameable_copy(self.scope));
                    return;
                }
                if let DrawerResolveOutcome::Create { title, key, slug } =
                    resolved.outcome
                {
                    // A new note's template already carries `## Tasks`,
                    // so a `TASKS` bullet adds to it instead of creating.
                    let template_tasks = self.new_note
                        && key == capture_drawers::drawer_key("Tasks");
                    row.consequence = Some(if template_tasks {
                        capture_drawers::h2_consequence(&title)
                    } else {
                        self.create_consequence(&title)
                    });
                    if template_tasks {
                        row.creates = false;
                    }
                    row.title = title;
                    row.key = key;
                    row.slug = slug;
                }
            }
        }
    }

    fn create_consequence(&self, title: &str) -> String {
        match (self.scope, self.block_id.as_deref()) {
            (DrawerScope::Task, Some(block_id)) => {
                format!("Creates {title} under ^{block_id}")
            }
            (DrawerScope::Task, None) => {
                format!("Starts {title} on the new task")
            }
            (DrawerScope::ProjectSection, _) => {
                format!("Creates ## {title}")
            }
        }
    }
}

/// Whether a slug resolution is the drawer the row wants: the same existing
/// drawer, or a creation of the same key.
fn matches_want(resolved: &DrawerResolved, want: Want, title: &str) -> bool {
    match (&resolved.outcome, want) {
        (DrawerResolveOutcome::Existing { drawer }, Want::Existing(line)) => {
            drawer.line == line
        }
        (DrawerResolveOutcome::Create { key, .. }, Want::Create) => {
            *key == capture_drawers::drawer_key(title)
        }
        _ => false,
    }
}

/// A catalog title that is not a valid ALL-CAPS drawer title (a project
/// `##` heading's raw spelling) falls back to its uppercase key.
fn wanted_title(title: &str, key: &str) -> String {
    capture_drawers::normalize_drawer_literal(title)
        .unwrap_or_else(|| key.to_string())
}

/// A quoted selector only names drawers whose uppercased title is valid
/// drawer syntax; a `##` heading outside that grammar cannot be addressed.
fn quoted_title_is_nameable(replacement: &str, title: &str) -> bool {
    !replacement.starts_with("#\"")
        || capture_drawers::normalize_drawer_literal(title).is_some()
}

fn scope_noun(scope: DrawerScope) -> &'static str {
    match scope {
        DrawerScope::Task => "drawer",
        DrawerScope::ProjectSection => "section",
    }
}

fn unnameable_copy(scope: DrawerScope) -> String {
    format!(
        "this {} name uses characters `#\"NAME\"` cannot spell; capture into it with `@route#Heading`",
        scope_noun(scope)
    )
}

fn unnameable_bullet_copy(scope: DrawerScope) -> String {
    format!(
        "this {} name uses characters an ALL-CAPS bullet cannot spell; capture into it with `@route#Heading`",
        scope_noun(scope)
    )
}

fn quote_escape(title: &str) -> String {
    title.replace('\\', "\\\\").replace('"', "\\\"")
}

const ENTRY_PREVIEW_CHARS: usize = 40;

fn ellipsize(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let kept: String = text.chars().take(max.saturating_sub(1)).collect();
    format!("{}…", kept.trim_end())
}

fn plural(count: u64, noun: &str) -> String {
    if count == 1 {
        format!("{count} {noun}")
    } else {
        format!("{count} {noun}s")
    }
}

/// `empty`, `1 note`, `3 notes`: the entries already in a task drawer.
fn entry_count_detail(entry_count: usize) -> String {
    if entry_count == 0 {
        "empty".to_string()
    } else {
        plural(entry_count as u64, "note")
    }
}

/// `used in 12 notes · 3 days ago`; the age is dropped when the catalog has
/// no timestamp.
pub(super) fn recent_detail(
    note_count: u64,
    last_seen: u64,
    now: u64,
) -> String {
    let used = format!("used in {}", plural(note_count, "note"));
    if last_seen == 0 {
        used
    } else {
        format!("{used} · {}", age_phrase(last_seen, now))
    }
}

/// Coarse relative age of a unix timestamp: `today`, `yesterday`,
/// `3 days ago`, `last week`, `3 weeks ago`, `last month`, `5 months ago`,
/// `last year`, `2 years ago`.
pub(super) fn age_phrase(last_seen: u64, now: u64) -> String {
    const DAY: u64 = 24 * 60 * 60;
    let days = now.saturating_sub(last_seen) / DAY;
    match days {
        0 => "today".to_string(),
        1 => "yesterday".to_string(),
        2..=6 => format!("{days} days ago"),
        7..=13 => "last week".to_string(),
        14..=29 => format!("{} weeks ago", days / 7),
        30..=59 => "last month".to_string(),
        60..=364 => format!("{} months ago", days / 30),
        365..=729 => "last year".to_string(),
        _ => format!("{} years ago", days / 365),
    }
}

pub(super) enum DrawerLookupFailure {
    MissingNote,
    Missing { suggestion: Option<String> },
    Duplicate(usize),
    NotATask,
}

/// One warning, no draft text, and no task description.
pub(super) fn unresolvable_parent_warning(
    route: &str,
    block_id: &str,
    failure: DrawerLookupFailure,
) -> String {
    match failure {
        DrawerLookupFailure::MissingNote => {
            format!("note does not exist: {route}.md")
        }
        DrawerLookupFailure::Missing { suggestion } => match suggestion {
            Some(suggestion) => format!(
                "no task with block ID ^{block_id} in {route}.md; did you mean ^{suggestion}?"
            ),
            None => {
                format!("no task with block ID ^{block_id} in {route}.md")
            }
        },
        DrawerLookupFailure::Duplicate(count) => {
            format!("block ID ^{block_id} appears {count} times in {route}.md")
        }
        DrawerLookupFailure::NotATask => {
            format!("^{block_id} in {route}.md is not a task")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plural_copy_is_singular_only_for_one() {
        assert_eq!(entry_count_detail(0), "empty");
        assert_eq!(entry_count_detail(1), "1 note");
        assert_eq!(entry_count_detail(3), "3 notes");
        assert_eq!(recent_detail(1, 0, 0), "used in 1 note");
        assert_eq!(recent_detail(12, 0, 0), "used in 12 notes");
    }

    #[test]
    fn age_phrase_buckets_follow_whole_days() {
        const DAY: u64 = 86_400;
        let now = 1_000 * DAY;
        let cases = [
            (0, "today"),
            (1, "yesterday"),
            (2, "2 days ago"),
            (6, "6 days ago"),
            (7, "last week"),
            (13, "last week"),
            (14, "2 weeks ago"),
            (29, "4 weeks ago"),
            (30, "last month"),
            (59, "last month"),
            (60, "2 months ago"),
            (364, "12 months ago"),
            (365, "last year"),
            (729, "last year"),
            (730, "2 years ago"),
        ];
        for (days, expected) in cases {
            assert_eq!(age_phrase(now - days * DAY, now), expected, "{days}d");
        }
        // A timestamp in the future never underflows.
        assert_eq!(age_phrase(now + DAY, now), "today");
        assert_eq!(
            recent_detail(12, now - 3 * DAY, now),
            "used in 12 notes · 3 days ago"
        );
    }

    #[test]
    fn ellipsize_keeps_short_text_and_trims_long_text() {
        assert_eq!(ellipsize("2 GB RAM", 40), "2 GB RAM");
        let long = "x".repeat(60);
        let cut = ellipsize(&long, 10);
        assert_eq!(cut.chars().count(), 10);
        assert!(cut.ends_with('…'));
    }

    #[test]
    fn quote_escape_protects_quotes_and_backslashes() {
        assert_eq!(quote_escape("Q&A"), "Q&A");
        assert_eq!(quote_escape("a\"b\\c"), "a\\\"b\\\\c");
    }
}
