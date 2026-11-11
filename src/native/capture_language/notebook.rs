//! Lexical notebook-route classification shared by capture and its editor.

use std::{collections::HashMap, ops::Range, path::PathBuf};

use super::{
    editor_model::{Diagnostic, EditorMode, Need, Severity, Span, SpanKind},
    item::{CaptureItemParseError, ParsedCaptureItemOutcome},
    model::{
        AuthoredSubBullet, CaptureItem, CaptureKind, CaptureParseOptions,
        EditorParseOptions, FanoutContext, ParsedCaptureText,
    },
    section_bullet::{
        empty_section_bullet_error, notebook_section_required_error,
        plan_create_item, plan_from_sub_bullets, plan_list_item, scan_leading,
        section_bullet_position_error, section_operations_from_item,
        NotebookBodyOutcome, SectionBulletHit,
    },
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct NotebookRouteInfo {
    pub(crate) title: String,
    pub(crate) summary: Option<String>,
    pub(crate) owner: Option<String>,
    pub(crate) breadcrumb: Vec<String>,
    pub(crate) sections: Vec<NotebookSection>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct NotebookSection {
    pub(crate) title: String,
    pub(crate) line_index: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AmbiguousSectionMatch {
    pub(crate) title: String,
    /// 1-based physical line of the matching `##` heading.
    pub(crate) line: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum NotebookSectionResolution {
    Existing { title: String, line_index: usize },
    Create { title: String },
    NearMiss { slug: String, quoted_title: String },
    Ambiguous { matches: Vec<AmbiguousSectionMatch> },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum NotebookSectionProvenance {
    Notebook,
    Draft,
}

/// One section in the staged view of a route at a draft offset: disk H2s
/// first, then sections earlier valid operations in this draft create.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct StagedNotebookSection {
    pub(crate) title: String,
    /// Identity used by the resolver. Disk headings use their 0-based
    /// line index; draft headings use a unique synthetic index.
    pub(crate) line_index: usize,
    /// 1-based disk line, or `None` for a draft-only section.
    pub(crate) line: Option<usize>,
    pub(crate) note_count: usize,
    /// The section's last top-level note text on disk, for menu
    /// consequences. `None` for draft sections and empty sections.
    pub(crate) last_note: Option<String>,
    pub(crate) provenance: NotebookSectionProvenance,
    /// Byte offset of the draft operation that created this section.
    pub(crate) draft_offset: Option<usize>,
}

impl StagedNotebookSection {
    pub(crate) fn as_heading(&self) -> NotebookSection {
        NotebookSection {
            title: self.title.clone(),
            line_index: self.line_index,
        }
    }
}

/// Immutable notebook route lookup for a single parse/capture invocation.
/// The native adapter builds it only after finding route-shaped text.
#[derive(Debug, Clone, Default)]
pub(crate) struct NotebookRoutePolicy {
    routes: HashMap<String, NotebookRouteInfo>,
    /// Exact file bytes read while building the policy, keyed by
    /// lower-cased route. The writer compares these preimages against the
    /// planner's staged bytes so a retype between classification and
    /// planning aborts instead of writing the new version silently.
    sources: HashMap<String, String>,
    /// Every metadata preimage consulted while classifying notebook routes,
    /// including owner-chain notes and missing candidate paths.
    metadata_sources: HashMap<PathBuf, Option<String>>,
    /// Lexically discovered `@parent^id@ %n` items, in source order. Disk
    /// routes always win; these activate only after their creation item.
    creates: Vec<ProspectiveCreate>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ProspectiveCreate {
    route: String,
    item_start: usize,
    marker_text: String,
    info: NotebookRouteInfo,
}

/// Ordered notebook-route lookup for one capture item.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum NotebookRouteLookup<'a> {
    Hit(&'a NotebookRouteInfo),
    UsedBeforeCreated { marker_text: &'a str },
    Miss,
}

impl NotebookRoutePolicy {
    pub(crate) fn absent() -> Self {
        Self::default()
    }

    #[cfg(test)]
    pub(crate) fn insert(&mut self, route: String, info: NotebookRouteInfo) {
        self.routes.insert(route.to_ascii_lowercase(), info);
    }

    pub(crate) fn insert_with_source(
        &mut self,
        route: String,
        info: NotebookRouteInfo,
        source: String,
    ) {
        let key = route.to_ascii_lowercase();
        self.routes.insert(key.clone(), info);
        self.sources.insert(key, source);
    }

    pub(crate) fn get(&self, route: &str) -> Option<&NotebookRouteInfo> {
        self.routes.get(&route.to_ascii_lowercase())
    }

    pub(crate) fn source(&self, route: &str) -> Option<&str> {
        self.sources
            .get(&route.to_ascii_lowercase())
            .map(String::as_str)
    }

    pub(crate) fn insert_metadata_source(
        &mut self,
        path: PathBuf,
        source: Option<String>,
    ) {
        self.metadata_sources.entry(path).or_insert(source);
    }

    pub(crate) fn metadata_sources(&self) -> &HashMap<PathBuf, Option<String>> {
        &self.metadata_sources
    }

    /// Record a prospective notebook created at `item_start`. Disk routes
    /// and earlier creates keep their slots; a colliding name is ignored.
    pub(crate) fn insert_create(
        &mut self,
        route: String,
        item_start: usize,
        marker_text: String,
        info: NotebookRouteInfo,
    ) {
        let route = route.to_ascii_lowercase();
        if self.routes.contains_key(&route)
            || self.creates.iter().any(|create| create.route == route)
        {
            return;
        }
        self.creates.push(ProspectiveCreate {
            route,
            item_start,
            marker_text,
            info,
        });
    }

    /// Register a successfully staged notebook so later items in this
    /// batch can write notes. Omits `sources` so the planner's staged
    /// bytes are never mistaken for a disk preimage.
    pub(crate) fn insert_staged(
        &mut self,
        route: String,
        info: NotebookRouteInfo,
    ) {
        self.routes.insert(route.to_ascii_lowercase(), info);
    }

    /// Disk routes always win. A prospective create is visible only to
    /// items that start after it; an earlier reference is
    /// `notebook_used_before_created`.
    pub(crate) fn lookup(
        &self,
        route: &str,
        item_start: usize,
    ) -> NotebookRouteLookup<'_> {
        let key = route.to_ascii_lowercase();
        if let Some(info) = self.routes.get(&key) {
            return NotebookRouteLookup::Hit(info);
        }
        if let Some(create) =
            self.creates.iter().find(|create| create.route == key)
        {
            if item_start > create.item_start {
                return NotebookRouteLookup::Hit(&create.info);
            }
            return NotebookRouteLookup::UsedBeforeCreated {
                marker_text: create.marker_text.as_str(),
            };
        }
        NotebookRouteLookup::Miss
    }

    pub(crate) fn is_visible(&self, route: &str, item_start: usize) -> bool {
        matches!(self.lookup(route, item_start), NotebookRouteLookup::Hit(_))
    }
}

/// H2 section headings from the same note snapshot used to classify its route.
pub(crate) fn notebook_sections(contents: &str) -> Vec<NotebookSection> {
    let lines = crate::native::capture::sections::line_spans(contents);
    crate::native::capture::sections::markdown_headings(&lines)
        .into_iter()
        .filter(|heading| heading.level == 2)
        .map(|heading| NotebookSection {
            title: heading.title.to_string(),
            line_index: heading.line_index,
        })
        .collect()
}

/// Resolve a selector against real H2s. The same pure operation serves the
/// editor snapshot and the writer's later staged snapshot.
///
/// Precedence: quoted exact title (else create verbatim); unquoted exact
/// title then unique whole slug; two or more ties at the winning step are
/// `Ambiguous`; otherwise near-miss (strict slug prefix or edit distance
/// ≤ 2); otherwise `Create`. Matching is never by prefix or fuzz at write
/// time.
pub(crate) fn resolve_notebook_section(
    selector: &str,
    exact: bool,
    headings: &[NotebookSection],
) -> NotebookSectionResolution {
    if exact {
        return match classify_matches(title_matches(selector, headings)) {
            MatchClass::Unique(heading) => {
                NotebookSectionResolution::Existing {
                    title: heading.title.clone(),
                    line_index: heading.line_index,
                }
            }
            MatchClass::None => NotebookSectionResolution::Create {
                title: selector.to_string(),
            },
            MatchClass::Many(matches) => ambiguous_resolution(matches),
        };
    }

    match classify_matches(title_matches(selector, headings)) {
        MatchClass::Unique(heading) => {
            return NotebookSectionResolution::Existing {
                title: heading.title.clone(),
                line_index: heading.line_index,
            };
        }
        MatchClass::Many(matches) => {
            return ambiguous_resolution(matches);
        }
        MatchClass::None => {}
    }

    let query_slug = super::selector_slug(selector);
    match classify_matches(slug_matches(&query_slug, headings)) {
        MatchClass::Unique(heading) => {
            return NotebookSectionResolution::Existing {
                title: heading.title.clone(),
                line_index: heading.line_index,
            };
        }
        MatchClass::Many(matches) => {
            return ambiguous_resolution(matches);
        }
        MatchClass::None => {}
    }

    let mut suggestion = headings.iter().find_map(|heading| {
        let slug = super::selector_slug(&heading.title);
        (!slug.is_empty()
            && !query_slug.is_empty()
            && slug.starts_with(&query_slug))
        .then_some((slug, heading.title.clone()))
    });
    if suggestion.is_none() && !query_slug.is_empty() {
        let mut best: Option<(usize, String)> = None;
        for heading in headings {
            let slug = super::selector_slug(&heading.title);
            if slug.is_empty() {
                continue;
            }
            if let Some(distance) =
                crate::native::note_tasks::bounded_levenshtein(
                    &query_slug,
                    &slug,
                    2,
                )
                && best
                    .as_ref()
                    .is_none_or(|(best_distance, _)| distance < *best_distance)
            {
                best = Some((distance, slug));
            }
        }
        suggestion = best.map(|(_, slug)| {
            let title = headings
                .iter()
                .find(|heading| super::selector_slug(&heading.title) == slug)
                .map(|heading| heading.title.clone())
                .unwrap_or_else(|| slug.clone());
            (slug, title)
        });
    }
    if let Some((slug, _)) = suggestion {
        return NotebookSectionResolution::NearMiss {
            slug,
            quoted_title: format_notebook_section_create_title(selector),
        };
    }

    NotebookSectionResolution::Create {
        title: format_notebook_section_create_title(selector),
    }
}

enum MatchClass<'a> {
    None,
    Unique(&'a NotebookSection),
    Many(Vec<&'a NotebookSection>),
}

fn classify_matches<'a>(matches: Vec<&'a NotebookSection>) -> MatchClass<'a> {
    match matches.len() {
        0 => MatchClass::None,
        1 => MatchClass::Unique(matches[0]),
        _ => MatchClass::Many(matches),
    }
}

fn title_matches<'a>(
    selector: &str,
    headings: &'a [NotebookSection],
) -> Vec<&'a NotebookSection> {
    headings
        .iter()
        .filter(|heading| heading.title.eq_ignore_ascii_case(selector))
        .collect()
}

fn slug_matches<'a>(
    query_slug: &str,
    headings: &'a [NotebookSection],
) -> Vec<&'a NotebookSection> {
    if query_slug.is_empty() {
        return Vec::new();
    }
    headings
        .iter()
        .filter(|heading| {
            let slug = super::selector_slug(&heading.title);
            !slug.is_empty() && slug == query_slug
        })
        .collect()
}

fn ambiguous_resolution(
    matches: Vec<&NotebookSection>,
) -> NotebookSectionResolution {
    NotebookSectionResolution::Ambiguous {
        matches: matches
            .into_iter()
            .map(|heading| AmbiguousSectionMatch {
                title: heading.title.clone(),
                line: heading.line_index + 1,
            })
            .collect(),
    }
}

/// Create-title for an unquoted selector: `-` becomes a space, then each
/// whitespace-separated word that contains an uppercase letter keeps the
/// typed capitals (`#API-keys` → `API Keys`) and every other word takes
/// first-character Title Case (`#git` → `Git`, `#coc.nvim` → `Coc.nvim`).
pub(crate) fn format_notebook_section_create_title(selector: &str) -> String {
    selector
        .replace('-', " ")
        .split_whitespace()
        .map(format_create_word)
        .collect::<Vec<_>>()
        .join(" ")
}

fn format_create_word(word: &str) -> String {
    if word.chars().any(char::is_uppercase) {
        return word.to_string();
    }
    let mut chars = word.chars();
    match chars.next() {
        None => String::new(),
        Some(first) => first.to_uppercase().chain(chars).collect(),
    }
}

pub(crate) fn notebook_section_near_miss_message(
    route: &str,
    selector: &str,
    slug: &str,
    quoted_title: &str,
) -> String {
    format!(
        "no section `{selector}` in {route}.md — did you mean `#{slug}`? Write `#\"{quoted_title}\"` to create it"
    )
}

pub(crate) fn notebook_section_ambiguous_message(
    route: &str,
    matches: &[AmbiguousSectionMatch],
) -> String {
    let count = matches.len();
    let lines = matches
        .iter()
        .map(|entry| entry.line.to_string())
        .collect::<Vec<_>>()
        .join(" and ");
    let titles: Vec<&str> =
        matches.iter().map(|entry| entry.title.as_str()).collect();
    let same_title = titles
        .first()
        .is_some_and(|first| titles.iter().all(|title| title == first));
    if same_title {
        let title = titles.first().copied().unwrap_or_default();
        format!(
            "{route}.md has {count} sections titled “{title}” (lines {lines}); rename one, then capture again"
        )
    } else {
        let named = matches
            .iter()
            .map(|entry| format!("“{}” line {}", entry.title, entry.line))
            .collect::<Vec<_>>()
            .join(", ");
        format!(
            "{route}.md has {count} matching sections ({named}); rename one, then capture again"
        )
    }
}

/// Headings at `at_offset` for `route`: disk H2s, then sections earlier
/// valid operations in this draft create, in source order. Later items,
/// failed claims, and the item at the offset never appear. Draft sections
/// that resolve to an existing heading dedupe into it. Operations carry
/// their own selector offsets so earlier bullets of the item under the
/// caret remain visible.
pub(crate) fn staged_notebook_sections(
    policy: &NotebookRoutePolicy,
    raw_text: &str,
    route: &str,
    at_offset: usize,
) -> Vec<StagedNotebookSection> {
    let route_key = route.to_ascii_lowercase();
    let mut staged = disk_staged_sections(policy, &route_key);
    let draft = super::draft::split_capture_draft(raw_text);
    for item in &draft.items {
        for operation in notebook_section_operations(policy, item) {
            if operation.offset >= at_offset {
                continue;
            }
            if !operation.route.eq_ignore_ascii_case(&route_key) {
                continue;
            }
            absorb_operation(&mut staged, &operation);
        }
    }
    staged
}

fn notebook_section_operations(
    policy: &NotebookRoutePolicy,
    item: &CaptureItem<'_>,
) -> Vec<super::section_bullet::NotebookSectionOperation> {
    if let Some(shape) = complete_notebook_create_shape(item) {
        let created =
            crate::native::capture_project_note::project_note_basename(
                &shape.parent,
                &shape.notebook_id,
            );
        let created = created
            .strip_suffix(".md")
            .unwrap_or(&created)
            .to_ascii_lowercase();
        return section_operations_from_item(
            item,
            &shape.parent,
            Some(&created),
        );
    }
    if let Some(list) = item.list.as_ref() {
        match policy.lookup(&list.route, item.start) {
            NotebookRouteLookup::Hit(_) => {
                return section_operations_from_item(item, &list.route, None);
            }
            NotebookRouteLookup::Miss
            | NotebookRouteLookup::UsedBeforeCreated { .. } => {
                return Vec::new();
            }
        }
    }
    let Ok(Some(claim)) = classify_item(item, Some(policy), None) else {
        return Vec::new();
    };
    let NotebookForm::Note {
        section: Some(selector),
        exact_section,
        section_range: Some(range),
        ..
    } = &claim.marker.form
    else {
        return Vec::new();
    };
    vec![super::section_bullet::NotebookSectionOperation {
        offset: range.0,
        selector: selector.clone(),
        exact: *exact_section,
        route: claim.marker.route,
    }]
}

fn absorb_operation(
    staged: &mut Vec<StagedNotebookSection>,
    operation: &super::section_bullet::NotebookSectionOperation,
) {
    let headings: Vec<NotebookSection> = staged
        .iter()
        .map(StagedNotebookSection::as_heading)
        .collect();
    if let NotebookSectionResolution::Create { title } =
        resolve_notebook_section(
            &operation.selector,
            operation.exact,
            &headings,
        )
    {
        push_draft_section(staged, title, operation.offset);
    }
}

/// The notebook route whose body an item's first-level bullets land in,
/// when they do: a route-only `@nb` list header visible to `policy`, or a
/// `@parent^id@ %n` item's created notebook. `None` under an `@nb#Section`
/// header, under one notebook note, and outside notebook claims, so the
/// section-bullet menu never opens there.
pub(crate) fn section_bullet_notebook_route(
    policy: &NotebookRoutePolicy,
    item: &super::model::CaptureItem<'_>,
) -> Option<String> {
    if let Some(list) = item.list.as_ref() {
        if list.section.is_some() || list.has_section_hash {
            return None;
        }
        if list.ownerless {
            return None;
        }
        if !policy.is_visible(&list.route, item.start) {
            return None;
        }
        return Some(list.route.clone());
    }
    let shape = complete_notebook_create_shape(item)?;
    let created = crate::native::capture_project_note::project_note_basename(
        &shape.parent,
        &shape.notebook_id,
    );
    Some(
        created
            .strip_suffix(".md")
            .unwrap_or(&created)
            .to_ascii_lowercase(),
    )
}

pub(crate) fn staged_headings(
    policy: &NotebookRoutePolicy,
    raw_text: &str,
    route: &str,
    at_offset: usize,
) -> Vec<NotebookSection> {
    staged_notebook_sections(policy, raw_text, route, at_offset)
        .iter()
        .map(StagedNotebookSection::as_heading)
        .collect()
}

fn disk_staged_sections(
    policy: &NotebookRoutePolicy,
    route: &str,
) -> Vec<StagedNotebookSection> {
    let Some(info) = policy.get(route) else {
        return Vec::new();
    };
    let stats = policy
        .source(route)
        .map(section_note_stats)
        .unwrap_or_else(|| vec![(0, None); info.sections.len()]);
    info.sections
        .iter()
        .enumerate()
        .map(|(index, heading)| {
            let (note_count, last_note) =
                stats.get(index).cloned().unwrap_or((0, None));
            StagedNotebookSection {
                title: heading.title.clone(),
                line_index: heading.line_index,
                line: Some(heading.line_index + 1),
                note_count,
                last_note,
                provenance: NotebookSectionProvenance::Notebook,
                draft_offset: None,
            }
        })
        .collect()
}

/// Per-section note stats over note `contents`: the top-level note count
/// plus the last top-level note's text, for menu detail and consequences.
pub(crate) fn section_note_stats(
    contents: &str,
) -> Vec<(usize, Option<String>)> {
    let headings = notebook_sections(contents);
    if headings.is_empty() {
        return Vec::new();
    }
    let lines: Vec<&str> = contents
        .split('\n')
        .map(|line| line.strip_suffix('\r').unwrap_or(line))
        .collect();
    let frontmatter_end =
        crate::native::markdown::strictly_closed_frontmatter_end(&lines);
    let scan_start = frontmatter_end.map_or(0, |end| end + 1);
    let fenced =
        crate::native::markdown::fenced_lines(&lines, scan_start..lines.len());
    let mut stats: Vec<(usize, Option<String>)> =
        vec![(0, None); headings.len()];
    for (index, line) in lines.iter().enumerate() {
        if index < scan_start || fenced.contains(&index) {
            continue;
        }
        if !is_column_zero_note(line) {
            continue;
        }
        let Some(section_index) = headings
            .iter()
            .enumerate()
            .rev()
            .find(|(_, heading)| heading.line_index < index)
            .map(|(section_index, _)| section_index)
        else {
            continue;
        };
        let entry = &mut stats[section_index];
        entry.0 += 1;
        entry.1 = Some(column_zero_note_text(line));
    }
    stats
}

/// The note text behind a column-zero `- `/`* `/`+ ` bullet line,
/// without Bob's `[created::…]` stamp or trailing `^id`.
fn column_zero_note_text(line: &str) -> String {
    let text = line[first_note_text_offset(line)..].trim();
    let (text, _) = strip_trailing_caret_id(text);
    strip_created_stamp(&text)
}

/// Drop one trailing `[created::…]` stamp token, when present.
fn strip_created_stamp(text: &str) -> String {
    let mut tokens: Vec<&str> = text.split_whitespace().collect();
    if tokens.last().is_some_and(|last| {
        last.starts_with("[created::") && last.ends_with(']')
    }) {
        tokens.pop();
    }
    tokens.join(" ")
}

fn first_note_text_offset(line: &str) -> usize {
    let mut offset = 1;
    while line.as_bytes().get(offset) == Some(&b' ')
        || line.as_bytes().get(offset) == Some(&b'\t')
    {
        offset += 1;
    }
    offset
}

fn is_column_zero_note(line: &str) -> bool {
    if line.starts_with([' ', '\t']) {
        return false;
    }
    if crate::native::notebooks::rules::is_checkbox_line(line) {
        return false;
    }
    let Some(first) = line.chars().next() else {
        return false;
    };
    if !matches!(first, '-' | '*' | '+') {
        return false;
    }
    line[first.len_utf8()..]
        .chars()
        .next()
        .is_some_and(char::is_whitespace)
}

fn push_draft_section(
    staged: &mut Vec<StagedNotebookSection>,
    title: String,
    item_start: usize,
) {
    let headings: Vec<NotebookSection> = staged
        .iter()
        .map(StagedNotebookSection::as_heading)
        .collect();
    if !matches!(
        resolve_notebook_section(&title, true, &headings),
        NotebookSectionResolution::Create { .. }
    ) {
        return;
    }
    let line_index = usize::MAX - staged.len();
    staged.push(StagedNotebookSection {
        title,
        line_index,
        line: None,
        note_count: 0,
        last_note: None,
        provenance: NotebookSectionProvenance::Draft,
        draft_offset: Some(item_start),
    });
}

fn sections_from_plan(
    plan: &super::section_bullet::NotebookBodyPlan,
) -> Vec<NotebookSection> {
    plan.sections
        .iter()
        .map(|section| NotebookSection {
            title: section.selector.clone(),
            line_index: 0,
        })
        .collect()
}

/// `%n` seed headings from the shared body plan. Shared by the
/// provisional policy snapshot and the writer's seed builder.
pub(crate) fn notebook_create_seed_sections(
    sub_bullets: &[AuthoredSubBullet],
) -> Vec<NotebookSection> {
    sections_from_plan(&plan_from_sub_bullets(sub_bullets, true))
}

fn notebook_create_seed_sections_from_item(
    item: &super::model::CaptureItem<'_>,
) -> Vec<NotebookSection> {
    match plan_create_item(item, "") {
        NotebookBodyOutcome::Ready(plan)
        | NotebookBodyOutcome::Pending { plan, .. } => {
            sections_from_plan(&plan)
        }
        NotebookBodyOutcome::Failed { .. } => Vec::new(),
    }
}

fn section_resolution_diagnostic(
    route: &str,
    selector: &str,
    exact: bool,
    headings: &[NotebookSection],
    range: Option<(usize, usize)>,
) -> Option<Diagnostic> {
    match resolve_notebook_section(selector, exact, headings) {
        NotebookSectionResolution::NearMiss { slug, quoted_title } => {
            Some(Diagnostic {
                severity: Severity::Error,
                code: "notebook_section_near_miss",
                message: notebook_section_near_miss_message(
                    route,
                    selector,
                    &slug,
                    &quoted_title,
                ),
                range,
                rewrite: None,
            })
        }
        NotebookSectionResolution::Ambiguous { matches } => Some(Diagnostic {
            severity: Severity::Error,
            code: "notebook_section_ambiguous",
            message: notebook_section_ambiguous_message(route, &matches),
            range,
            rewrite: None,
        }),
        NotebookSectionResolution::Existing { .. }
        | NotebookSectionResolution::Create { .. } => None,
    }
}

/// Routes mentioned by capture syntax, in first-seen order. This is a
/// deliberately cheap lexical preflight: native callers use it to avoid
/// opening any note for an unrouted draft, and deduplicate repeated routes
/// before resolving notebook frontmatter.
fn collect_candidate_routes(
    raw_text: &str,
    seen: &mut std::collections::HashSet<String>,
    routes: &mut Vec<String>,
) {
    for token in raw_text.split_whitespace() {
        let Some(rest) = token.strip_prefix('@') else {
            continue;
        };
        let rest = rest.strip_prefix('@').unwrap_or(rest);
        let route: String = rest
            .chars()
            .take_while(|ch| {
                ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-')
            })
            .collect();
        if route.is_empty() || !super::is_route_token(&route) {
            continue;
        }
        let route = route.to_ascii_lowercase();
        if seen.insert(route.clone()) {
            routes.push(route);
        }
    }
}

pub(crate) fn candidate_notebook_routes(raw_text: &str) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    let mut routes = Vec::new();
    collect_candidate_routes(raw_text, &mut seen, &mut routes);
    let analysis = super::template::analyze(raw_text);
    if let Ok(Some(expansion)) =
        super::template::expand_with_analysis(raw_text, &analysis)
    {
        for instance in expansion.instances {
            collect_candidate_routes(&instance.text, &mut seen, &mut routes);
        }
    }
    routes
}

/// One lexically discovered notebook-create item. Filename is the
/// project-note basename (`parent_id.md` with hyphens mapped to
/// underscores).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DiscoveredNotebookCreate {
    pub(crate) parent: String,
    pub(crate) notebook_id: String,
    pub(crate) route: String,
    pub(crate) filename: String,
    pub(crate) item_start: usize,
    pub(crate) marker_text: String,
    pub(crate) summary: String,
    pub(crate) sections: Vec<NotebookSection>,
}

/// Discover `@parent^id@ %n` items with the same item split and seal
/// recognition as the grammar, including expanded template rows.
pub(crate) fn discover_notebook_creates(
    raw_text: &str,
) -> Vec<DiscoveredNotebookCreate> {
    let mut creates = Vec::new();
    let mut seen = std::collections::HashSet::new();
    collect_discovered_creates(
        raw_text,
        |item| item.start,
        &mut creates,
        &mut seen,
    );
    let analysis = super::template::analyze(raw_text);
    if let Ok(Some(expansion)) =
        super::template::expand_with_analysis(raw_text, &analysis)
    {
        for instance in expansion.instances {
            collect_discovered_creates(
                &instance.text,
                |_| instance.row_range.start,
                &mut creates,
                &mut seen,
            );
        }
    }
    creates
}

fn collect_discovered_creates(
    raw_text: &str,
    item_start: impl Fn(&super::model::CaptureItem<'_>) -> usize,
    creates: &mut Vec<DiscoveredNotebookCreate>,
    seen: &mut std::collections::HashSet<String>,
) {
    let draft = super::draft::split_capture_draft(raw_text);
    for item in &draft.items {
        let Some(claim) = complete_notebook_create_shape(item) else {
            continue;
        };
        let filename =
            crate::native::capture_project_note::project_note_basename(
                &claim.parent,
                &claim.notebook_id,
            );
        let route = filename
            .strip_suffix(".md")
            .unwrap_or(&filename)
            .to_ascii_lowercase();
        if !seen.insert(route.clone()) {
            continue;
        }
        creates.push(DiscoveredNotebookCreate {
            parent: claim.parent,
            notebook_id: claim.notebook_id,
            route,
            filename,
            item_start: item_start(item),
            marker_text: claim.marker_text,
            summary: claim.body,
            sections: provisional_create_sections(item),
        });
    }
}

/// Forced `--route` destinations are lexical candidates too, even though
/// they make ordinary `@` tokens literal for this invocation.
pub(crate) fn candidate_notebook_route(route: &str) -> Option<String> {
    let route = route.trim();
    (super::is_route_token(route) && !route.starts_with('@'))
        .then(|| route.to_ascii_lowercase())
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum NotebookForm {
    Note {
        section: Option<String>,
        exact_section: bool,
        block_id: Option<String>,
        section_range: Option<(usize, usize)>,
        block_id_range: Option<(usize, usize)>,
    },
    SubNote {
        parent_block_id: String,
        block_id_range: Option<(usize, usize)>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RouteMarker {
    route: String,
    route_range: (usize, usize),
    token_range: (usize, usize),
    form: NotebookForm,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum RouteScan {
    NotNotebookRoute,
    Supported(RouteMarker),
    Unsupported {
        route: String,
        range: (usize, usize),
        code: &'static str,
        message: String,
    },
}

fn route_prefix(text: &str, start: usize) -> Option<(String, usize)> {
    let tail = text.get(start..)?.strip_prefix('@')?;
    if tail.starts_with('@') {
        return None;
    }
    let length = tail
        .char_indices()
        .take_while(|(_, ch)| {
            ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-')
        })
        .map(|(index, ch)| index + ch.len_utf8())
        .last()?;
    let route = &tail[..length];
    if !super::is_route_token(route) {
        return None;
    }
    Some((route.to_ascii_lowercase(), start + 1 + length))
}

fn scan_route_marker(text: &str, start: usize, end: usize) -> RouteScan {
    let Some((route, route_end)) = route_prefix(text, start) else {
        return RouteScan::NotNotebookRoute;
    };
    let Some(suffix) = text.get(route_end..end) else {
        return RouteScan::NotNotebookRoute;
    };
    let unsupported =
        |code: &'static str, message: &str| RouteScan::Unsupported {
            route: route.clone(),
            range: (start, end),
            code,
            message: message.to_string(),
        };
    if suffix.is_empty() {
        return RouteScan::Supported(RouteMarker {
            route,
            route_range: (start, route_end),
            token_range: (start, end),
            form: NotebookForm::Note {
                section: None,
                exact_section: false,
                block_id: None,
                section_range: None,
                block_id_range: None,
            },
        });
    }
    if suffix.starts_with('/') {
        // Record routes keep their established grammar even when the
        // folder name collides with a notebook.
        return RouteScan::NotNotebookRoute;
    }
    if suffix.starts_with(':') {
        return unsupported(
            "notebook_holds_no_tasks",
            &super::line::notebook_holds_no_tasks_error(&route, None),
        );
    }
    if suffix.starts_with('#') {
        let parsed = super::tokens::parse_hash_section_and_id(&suffix[1..]);
        let consumed = 1
            + parsed.section_len
            + if parsed.has_caret {
                1 + parsed.id_len
            } else {
                0
            };
        if consumed < suffix.len() {
            let rest = &suffix[consumed..];
            if rest.starts_with('#') {
                return unsupported(
                    "notebook_marker_unsupported",
                    &super::line::notebook_no_drawers_error(&route),
                );
            }
            return unsupported(
                "notebook_marker_unsupported",
                &super::line::notebook_marker_unsupported_error(
                    rest, &route, None,
                ),
            );
        }
        let section_end_abs = route_end + 1 + parsed.section_len;
        let block_id_range = parsed.has_caret.then_some((
            section_end_abs + 1,
            section_end_abs + 1 + parsed.id_len,
        ));
        let section_range = (parsed.section.is_some()
            || parsed.incomplete_section
            || parsed.quoted_incomplete)
            .then_some((route_end, section_end_abs));
        return RouteScan::Supported(RouteMarker {
            route,
            route_range: (start, route_end),
            token_range: (start, end),
            form: NotebookForm::Note {
                section: parsed.section,
                exact_section: parsed.exact,
                block_id: parsed.block_id,
                section_range,
                block_id_range,
            },
        });
    }
    if let Some(block_id) = suffix.strip_prefix('^') {
        if let Some(id) = block_id.strip_suffix('@') {
            return unsupported(
                "notebook_project_seal",
                &super::line::notebook_project_seal_error(&route, id),
            );
        }
        if block_id.contains('#') {
            return unsupported(
                "notebook_marker_unsupported",
                &super::line::notebook_no_drawers_error(&route),
            );
        }
        if block_id.contains(['+', ':', '=', '!']) {
            let marker = if block_id.contains('=') {
                "="
            } else if block_id.contains('!') {
                "!"
            } else if block_id.contains(':') {
                ":"
            } else {
                "+"
            };
            return unsupported(
                "notebook_marker_unsupported",
                &super::line::notebook_marker_unsupported_error(
                    marker, &route, None,
                ),
            );
        }
        let id_range = (route_end + 1, end);
        if !block_id.is_empty() && !super::tokens::is_block_id(block_id) {
            return unsupported("notebook_block_id_required", "notebook note block IDs must contain only A-Z, a-z, 0-9, or `-`");
        }
        return RouteScan::Supported(RouteMarker {
            route,
            route_range: (start, route_end),
            token_range: (start, end),
            form: NotebookForm::Note {
                section: None,
                exact_section: false,
                block_id: (!block_id.is_empty()).then(|| block_id.to_string()),
                section_range: None,
                block_id_range: Some(id_range),
            },
        });
    }
    if let Some(parent_block_id) = suffix.strip_prefix('+') {
        if parent_block_id.contains('#') {
            return unsupported(
                "notebook_marker_unsupported",
                &super::line::notebook_no_drawers_error(&route),
            );
        }
        if parent_block_id.ends_with('!')
            || parent_block_id.contains(['^', ':', '=', '!'])
        {
            let marker = if parent_block_id.contains('!') {
                "!"
            } else if parent_block_id.contains('=') {
                "="
            } else {
                ":"
            };
            return unsupported(
                "notebook_marker_unsupported",
                &super::line::notebook_marker_unsupported_error(
                    marker, &route, None,
                ),
            );
        }
        if !parent_block_id.is_empty()
            && !super::tokens::is_block_id(parent_block_id)
        {
            return unsupported("notebook_block_id_required", "notebook parent block IDs must contain only A-Z, a-z, 0-9, or `-`");
        }
        return RouteScan::Supported(RouteMarker {
            route,
            route_range: (start, route_end),
            token_range: (start, end),
            form: NotebookForm::SubNote {
                parent_block_id: parent_block_id.to_string(),
                block_id_range: Some((route_end + 1, end)),
            },
        });
    }
    let marker = suffix
        .chars()
        .next()
        .map(|ch| ch.to_string())
        .unwrap_or_default();
    unsupported(
        "notebook_marker_unsupported",
        &super::line::notebook_marker_unsupported_error(&marker, &route, None),
    )
}

fn quoted_section_marker_len(suffix: &str) -> Option<usize> {
    let after_hash = suffix.strip_prefix('#')?;
    if !super::pomodoro_literal::opens_exact_name(after_hash) {
        return None;
    }
    let lex = super::pomodoro_literal::lex_exact_name_after_hash(
        after_hash,
        after_hash.len(),
    )?;
    if !lex.complete {
        return Some(suffix.len());
    }
    let mut len = 1 + lex.literal_len;
    if let Some(id) = suffix.get(len..).and_then(|rest| rest.strip_prefix('^'))
    {
        let id_len = id.find(char::is_whitespace).unwrap_or(id.len());
        len += 1 + id_len;
    }
    Some(len)
}

fn is_type_marker_token(token: &str) -> bool {
    let Some(rest) = token.strip_prefix('#') else {
        return false;
    };
    if rest.is_empty() || super::pomodoro_literal::opens_exact_name(rest) {
        return true;
    }
    !rest.contains(['@', '^', '+', ':', '/', '=', '#'])
}

fn is_edge_transparent(token: &str) -> bool {
    super::directive::is_directive_token(token)
        || super::markers::parse_schedule_token(token).is_some()
        || super::markers::is_priority_marker_token(token)
        || token == "s:"
        || is_type_marker_token(token)
        || token.starts_with('&')
        || token.starts_with('*')
        || token.starts_with('~')
        || token == "!"
        || (token.starts_with('=') && token != "==")
}

fn skip_leading_transparent(text: &str) -> usize {
    let mut start = text.len() - text.trim_start().len();
    loop {
        let rest = text.get(start..).unwrap_or("");
        let Some(token) = rest.split_whitespace().next() else {
            return start;
        };
        if !is_edge_transparent(token) {
            return start;
        }
        let after = &rest[token.len()..];
        start += token.len() + (after.len() - after.trim_start().len());
    }
}

fn notebook_marker_end(text: &str, start: usize) -> Option<usize> {
    let (_, route_end) = route_prefix(text, start)?;
    let suffix = text.get(route_end..).unwrap_or("");
    if suffix.starts_with('#')
        && super::pomodoro_literal::opens_exact_name(&suffix[1..])
    {
        Some(
            route_end
                + quoted_section_marker_len(suffix).unwrap_or(suffix.len()),
        )
    } else {
        Some(
            text[start..]
                .find(char::is_whitespace)
                .map(|offset| start + offset)
                .unwrap_or(text.len()),
        )
    }
}

fn trailing_rest_is_transparent(rest: &str) -> bool {
    rest.split_whitespace().all(is_edge_transparent)
}

fn edge_marker(item: &CaptureItem<'_>) -> Option<RouteScan> {
    if item.list.is_some() {
        return None;
    }
    let parent = item.lines.first()?.raw;
    let text = parent.text;
    let leading = skip_leading_transparent(text);
    if text
        .get(leading..)
        .is_some_and(|rest| rest.starts_with('@'))
    {
        let marker_end = notebook_marker_end(text, leading)?;
        return Some(shift_scan(
            scan_route_marker(text, leading, marker_end),
            parent.start,
        ));
    }
    let trimmed_end = text.trim_end().len();
    let prefix = &text[..trimmed_end];
    for (relative, _) in prefix.match_indices('@').rev() {
        if relative == 0
            || !prefix[..relative]
                .chars()
                .next_back()
                .is_some_and(char::is_whitespace)
        {
            continue;
        }
        let candidate = &prefix[relative..];
        let Some(marker_end) = notebook_marker_end(candidate, 0) else {
            continue;
        };
        let rest = candidate.get(marker_end..).unwrap_or("");
        if !trailing_rest_is_transparent(rest) {
            continue;
        }
        return Some(shift_scan(
            scan_route_marker(text, relative, relative + marker_end),
            parent.start,
        ));
    }
    None
}

fn shift_scan(scan: RouteScan, offset: usize) -> RouteScan {
    let shift = |range: (usize, usize)| (range.0 + offset, range.1 + offset);
    match scan {
        RouteScan::Supported(mut marker) => {
            marker.route_range = shift(marker.route_range);
            marker.token_range = shift(marker.token_range);
            match &mut marker.form {
                NotebookForm::Note {
                    section_range,
                    block_id_range,
                    ..
                } => {
                    *section_range = section_range.map(shift);
                    *block_id_range = block_id_range.map(shift);
                }
                NotebookForm::SubNote { block_id_range, .. } => {
                    *block_id_range = block_id_range.map(shift);
                }
            }
            RouteScan::Supported(marker)
        }
        RouteScan::Unsupported {
            route,
            range,
            code,
            message,
        } => RouteScan::Unsupported {
            route,
            range: shift(range),
            code,
            message,
        },
        RouteScan::NotNotebookRoute => RouteScan::NotNotebookRoute,
    }
}

fn body_without_marker(item: &CaptureItem<'_>, marker: &RouteMarker) -> String {
    let Some(parent) = item.lines.first() else {
        return String::new();
    };
    let local_start = marker.token_range.0.saturating_sub(parent.raw.start);
    let local_end = marker.token_range.1.saturating_sub(parent.raw.start);
    let before = parent.raw.text.get(..local_start).unwrap_or("").trim();
    let after = parent.raw.text.get(local_end..).unwrap_or("").trim();
    match (before.is_empty(), after.is_empty()) {
        (true, true) => String::new(),
        (false, true) => before.to_string(),
        (true, false) => after.to_string(),
        (false, false) => format!("{before} {after}"),
    }
}

fn notebook_sub_bullets(
    item: &CaptureItem<'_>,
) -> Result<Vec<AuthoredSubBullet>, CaptureItemParseError> {
    let list = item.list.is_some();
    let mut sub_bullets = Vec::new();
    for line in item.lines.iter().skip(1) {
        match super::draft::classify_authored_line(line.raw) {
            super::draft::AuthoredLineClass::EmptyOrPlaceholder => continue,
            super::draft::AuthoredLineClass::Item(authored) => {
                sub_bullets.push(AuthoredSubBullet {
                    body: authored.body.trim().to_string(),
                    depth: if list {
                        super::model::AuthoredDepth::First
                    } else {
                        authored.depth
                    },
                    task_id: None,
                    move_task: None,
                    move_choice: None,
                    drawer: None,
                });
            }
            super::draft::AuthoredLineClass::Invalid => {
                return Err(CaptureItemParseError::coded(
                    "invalid_child_line",
                    super::line::invalid_child_line_error(line.line_number),
                ));
            }
        }
    }
    Ok(sub_bullets)
}

#[derive(Debug, Clone)]
struct NotebookClaim {
    marker: RouteMarker,
    body: String,
    info: NotebookRouteInfo,
    inherited: bool,
}

fn lookup_route(
    policy: &NotebookRoutePolicy,
    route: &str,
    item_start: usize,
    range: (usize, usize),
) -> Result<Option<NotebookRouteInfo>, RouteScan> {
    match policy.lookup(route, item_start) {
        NotebookRouteLookup::Hit(info) => Ok(Some(info.clone())),
        NotebookRouteLookup::Miss => Ok(None),
        NotebookRouteLookup::UsedBeforeCreated { marker_text } => {
            Err(RouteScan::Unsupported {
                route: route.to_ascii_lowercase(),
                range,
                code: "notebook_used_before_created",
                message: super::line::notebook_used_before_created_error(
                    route,
                    marker_text,
                ),
            })
        }
    }
}

fn classify_item(
    item: &CaptureItem<'_>,
    policy: Option<&NotebookRoutePolicy>,
    global_route: Option<&str>,
) -> Result<Option<NotebookClaim>, RouteScan> {
    let Some(policy) = policy else {
        return Ok(None);
    };
    if let Some(scan) = edge_marker(item) {
        let route = match &scan {
            RouteScan::NotNotebookRoute => return Ok(None),
            RouteScan::Supported(marker) => marker.route.as_str(),
            RouteScan::Unsupported { route, .. } => route,
        };
        let range = match &scan {
            RouteScan::Supported(marker) => marker.token_range,
            RouteScan::Unsupported { range, .. } => *range,
            RouteScan::NotNotebookRoute => (item.start, item.end),
        };
        let Some(info) = lookup_route(policy, route, item.start, range)? else {
            return Ok(None);
        };
        return match scan {
            RouteScan::Supported(marker) => Ok(Some(NotebookClaim {
                body: body_without_marker(item, &marker),
                marker,
                info,
                inherited: false,
            })),
            refusal @ RouteScan::Unsupported { .. } => Err(refusal),
            RouteScan::NotNotebookRoute => Ok(None),
        };
    }
    if item_has_explicit_operation(item) {
        return Ok(None);
    }
    let Some(route) = global_route else {
        return Ok(None);
    };
    let range = item
        .lines
        .first()
        .map(|line| (line.raw.start, line.raw.end))
        .unwrap_or((item.start, item.end));
    let Some(info) = lookup_route(policy, route, item.start, range)? else {
        return Ok(None);
    };
    Ok(Some(NotebookClaim {
        body: body_without_globals(item),
        marker: inherited_marker(route.to_string(), item),
        info,
        inherited: true,
    }))
}

/// Global notebook routes apply only to ordinary text and URL notes. Keep
/// lexical operation claims on their established parser path before the
/// notebook can reinterpret an unrouted item.
fn item_has_explicit_operation(item: &CaptureItem<'_>) -> bool {
    if super::tokens::task_link_query_token(item).is_some()
        || super::tokens::task_goto_query_token(item).is_some()
        || super::editor_parse::parse_editor_task_complete_item(item).is_some()
    {
        return true;
    }
    let Some(parent) = item.lines.first() else {
        return false;
    };
    let first = parent.raw.text.split_whitespace().next().unwrap_or("");
    if super::item::is_session_chain_token(first) {
        return true;
    }
    let directives =
        super::directive::resolve(&super::directive::collect_item(item));
    if directives.is_ok_and(|directives| {
        directives.note || directives.listen || directives.template
    }) {
        return true;
    }
    if super::type_marker::collect_item(item)
        .iter()
        .any(super::type_marker::TypeMarkerOccurrence::is_marker)
    {
        return true;
    }
    if super::dependencies::scan_item_dependencies(item).has_entries()
        || super::dependencies::scan_item_dependencies(item)
            .has_partial_tokens()
    {
        return true;
    }
    if super::relations::scan_line_relations(parent.raw.text, parent.raw.start)
        .iter()
        .any(|relation| {
            !matches!(
                relation,
                super::relations::ScannedRelation::Escaped { .. }
            )
        })
    {
        return true;
    }
    super::editor_parse::tokenize_line_with_spans(&parent.raw)
        .iter()
        .any(|token| {
            super::markers::parse_schedule_token(token.text).is_some()
                || super::markers::parse_priority_token(token.text).is_some()
        })
}

fn notebook_authored_block(
    body: &str,
    children: &[AuthoredSubBullet],
) -> String {
    let mut lines = vec![format!("- {body}")];
    lines.extend(children.iter().map(|child| {
        let indentation = "  ".repeat(child.depth.indent_units());
        format!("{indentation}- {}", child.body)
    }));
    lines.join("\n")
}

fn notebook_authored_block_has_checkbox(
    body: &str,
    children: &[AuthoredSubBullet],
) -> bool {
    crate::native::notebooks::rules::contains_checkbox_rows(
        &notebook_authored_block(body, children),
    )
}

fn notebook_task_refusal(route: &str, owner: Option<&str>) -> String {
    super::line::notebook_holds_no_tasks_error(route, owner)
}

fn inherited_marker(route: String, item: &CaptureItem<'_>) -> RouteMarker {
    let range = item
        .lines
        .first()
        .map(|line| (line.raw.start, line.raw.end))
        .unwrap_or((item.start, item.end));
    RouteMarker {
        route,
        route_range: range,
        token_range: range,
        form: NotebookForm::Note {
            section: None,
            exact_section: false,
            block_id: None,
            section_range: None,
            block_id_range: None,
        },
    }
}

fn body_without_globals(item: &CaptureItem<'_>) -> String {
    let Some(parent) = item.lines.first() else {
        return String::new();
    };
    let tokens = super::editor_parse::tokenize_line_with_spans(&parent.raw);
    let body: String = tokens
        .into_iter()
        .filter(|token| !token.text.starts_with("@@"))
        .map(|token| token.text)
        .collect::<Vec<_>>()
        .join(" ");
    body.trim().to_string()
}

fn notebook_kind(claim: &NotebookClaim) -> CaptureKind {
    match &claim.marker.form {
        NotebookForm::Note {
            section,
            exact_section,
            block_id,
            ..
        } => CaptureKind::NotebookNote {
            section: section.clone(),
            exact_section: *exact_section,
            block_id: block_id.clone(),
        },
        NotebookForm::SubNote {
            parent_block_id, ..
        } => CaptureKind::NotebookSubNote {
            parent_block_id: parent_block_id.clone(),
        },
    }
}

fn strip_clip_tokens(body: &str) -> String {
    super::model::tokenize_with_spans(body)
        .into_iter()
        .filter(|token| {
            !token
                .text
                .strip_prefix("%c")
                .is_some_and(|rest| rest.chars().all(|ch| ch.is_ascii_digit()))
        })
        .map(|token| token.text)
        .collect::<Vec<_>>()
        .join(" ")
}

pub(crate) fn strip_trailing_caret_id(body: &str) -> (String, Option<String>) {
    let mut tokens: Vec<&str> = body.split_whitespace().collect();
    let Some(last) = tokens.last() else {
        return (body.trim().to_string(), None);
    };
    if let Some(id) = last.strip_prefix('^')
        && super::tokens::is_block_id(id)
    {
        let id = id.to_string();
        tokens.pop();
        return (tokens.join(" "), Some(id));
    }
    (body.trim().to_string(), None)
}

fn owner_of(info: &NotebookRouteInfo) -> Option<&str> {
    info.owner.as_deref()
}

fn reformat_unsupported(
    code: &'static str,
    message: String,
    route: &str,
    owner: Option<&str>,
) -> (&'static str, String) {
    match code {
        "notebook_holds_no_tasks" => (
            code,
            super::line::notebook_holds_no_tasks_error(route, owner),
        ),
        "notebook_marker_unsupported" => (code, message),
        "notebook_project_seal" => (code, message),
        _ => (code, message),
    }
}

/// `%n` is a note directive even when a type marker, session `!`/`=`, or
/// similar refused token occupies the trailing edge so the shared
/// collector never claims it.
fn item_has_note_token(item: &CaptureItem<'_>) -> bool {
    let Some(parent) = item.lines.first() else {
        return false;
    };
    super::editor_parse::tokenize_line_with_spans(&parent.raw)
        .iter()
        .any(|token| token.text == "%n")
}

/// Incomplete or colon seals plus `%n` keep the pomodoro-note teaching;
/// only a complete caret create seal is lifted to `NotebookCreate`.
fn unsupported_with_optional_note(
    item: &CaptureItem<'_>,
    code: &'static str,
    message: String,
    route: &str,
    owner: Option<&str>,
) -> (&'static str, String) {
    if item_has_note_token(item) {
        return (
            "pomodoro_note_conflict",
            super::line::pomodoro_note_route_conflict_error(),
        );
    }
    reformat_unsupported(code, message, route, owner)
}

/// A notebook route used as a task home (`>`, `<`, `@nb:id`).
pub(super) struct NotebookTaskRouteError {
    pub(super) route: String,
    message: String,
    span: Option<(usize, usize)>,
    code: &'static str,
}

impl NotebookTaskRouteError {
    pub(super) fn into_parse_error(self) -> CaptureItemParseError {
        CaptureItemParseError::coded_at(self.code, self.message, self.span)
    }

    pub(super) fn into_editor_view(self) -> NotebookEditorView {
        notebook_editor_error(self.route, self.code, self.message, self.span)
    }
}

pub(super) fn notebook_task_route_error(
    policy: Option<&NotebookRoutePolicy>,
    route: &str,
    span: Option<(usize, usize)>,
    item_start: usize,
) -> Option<NotebookTaskRouteError> {
    let policy = policy?;
    match policy.lookup(route, item_start) {
        NotebookRouteLookup::Hit(info) => Some(NotebookTaskRouteError {
            route: route.to_ascii_lowercase(),
            message: super::line::notebook_holds_no_tasks_error(
                route,
                info.owner.as_deref(),
            ),
            span,
            code: "notebook_holds_no_tasks",
        }),
        NotebookRouteLookup::UsedBeforeCreated { marker_text } => {
            Some(NotebookTaskRouteError {
                route: route.to_ascii_lowercase(),
                message: super::line::notebook_used_before_created_error(
                    route,
                    marker_text,
                ),
                span,
                code: "notebook_used_before_created",
            })
        }
        NotebookRouteLookup::Miss => None,
    }
}

pub(super) fn first_notebook_task_route<'a>(
    policy: Option<&NotebookRoutePolicy>,
    routes: impl IntoIterator<Item = (&'a str, Option<(usize, usize)>)>,
    item_start: usize,
) -> Option<NotebookTaskRouteError> {
    routes.into_iter().find_map(|(route, span)| {
        notebook_task_route_error(policy, route, span, item_start)
    })
}

fn item_is_claimed_operator(item: &CaptureItem<'_>) -> bool {
    let Some(parent) = item.lines.first() else {
        return false;
    };
    let has_children = item.lines.len() > 1;
    !matches!(
        super::transfer::scan_transfer_line(
            &parent.raw,
            item.source,
            has_children,
        ),
        super::transfer::TransferScan::Unclaimed
            | super::transfer::TransferScan::Escaped { .. }
    ) || !matches!(
        super::merge_in::scan_merge_in_line(
            &parent.raw,
            item.source,
            has_children,
        ),
        super::merge_in::MergeInScan::Unclaimed
            | super::merge_in::MergeInScan::Escaped { .. }
    )
}

fn refuse_notebook_item_markers(
    item: &CaptureItem<'_>,
    route: &str,
    owner: Option<&str>,
    marker_range: (usize, usize),
) -> Result<Option<super::model::ClipRequest>, CaptureItemParseError> {
    let occurrences = super::directive::collect_item(item);
    let directives = match super::directive::resolve(&occurrences) {
        Ok(directives) => directives,
        Err((message, code, span)) => {
            return Err(CaptureItemParseError::coded_at(
                code,
                message,
                Some((span.start, span.end)),
            ));
        }
    };
    if directives.listen {
        return Err(CaptureItemParseError::coded_at(
            "notebook_marker_unsupported",
            super::line::notebook_marker_unsupported_error("%l", route, owner),
            Some(marker_range),
        ));
    }
    if directives.note {
        return Err(CaptureItemParseError::coded_at(
            "pomodoro_note_conflict",
            super::line::pomodoro_note_route_conflict_error(),
            Some(marker_range),
        ));
    }
    let types = super::type_marker::collect_item(item);
    if let Some(occurrence) = types.iter().find(|item| item.is_marker()) {
        let raw = match occurrence {
            super::type_marker::TypeMarkerOccurrence::Complete(marker) => {
                format!("#{}", marker.name)
            }
            super::type_marker::TypeMarkerOccurrence::Incomplete { .. } => {
                "#".to_string()
            }
            super::type_marker::TypeMarkerOccurrence::Invalid {
                raw, ..
            } => raw.clone(),
            super::type_marker::TypeMarkerOccurrence::Escaped { .. } => {
                String::new()
            }
        };
        return Err(CaptureItemParseError::coded_at(
            "notebook_marker_unsupported",
            super::line::notebook_marker_unsupported_error(&raw, route, owner),
            Some(marker_range),
        ));
    }
    let deps = super::dependencies::scan_item_dependencies(item);
    if deps.has_entries() || deps.has_partial_tokens() {
        return Err(CaptureItemParseError::coded_at(
            "notebook_holds_no_tasks",
            super::line::notebook_holds_no_tasks_error(route, owner),
            Some(marker_range),
        ));
    }
    if let Some(parent) = item.lines.first() {
        for token in super::editor_parse::tokenize_line_with_spans(&parent.raw)
        {
            if super::markers::parse_schedule_token(token.text).is_some() {
                return Err(CaptureItemParseError::coded_at(
                    "notebook_marker_unsupported",
                    super::line::notebook_marker_unsupported_error(
                        "s:N", route, owner,
                    ),
                    Some((token.start, token.end)),
                ));
            }
            if super::markers::parse_priority_token(token.text).is_some() {
                return Err(CaptureItemParseError::coded_at(
                    "notebook_marker_unsupported",
                    super::line::notebook_marker_unsupported_error(
                        "$N", route, owner,
                    ),
                    Some((token.start, token.end)),
                ));
            }
            if token.text == "!"
                || (token.text.starts_with('=') && token.text != "==")
            {
                let marker = if token.text == "!" { "!" } else { "=" };
                return Err(CaptureItemParseError::coded_at(
                    "notebook_marker_unsupported",
                    super::line::notebook_marker_unsupported_error(
                        marker, route, owner,
                    ),
                    Some((token.start, token.end)),
                ));
            }
        }
    }
    Ok(directives.clip)
}

fn finish_notebook_claim<'a>(
    item: &CaptureItem<'a>,
    mut claim: NotebookClaim,
    clip: Option<super::model::ClipRequest>,
    local_marker: Option<String>,
) -> Result<ParsedCaptureItemOutcome<'a>, CaptureItemParseError> {
    match &claim.marker.form {
        NotebookForm::Note {
            section: None,
            section_range: Some(_),
            ..
        } => {
            return Err(CaptureItemParseError::coded_at(
                "notebook_section_required",
                "a notebook section marker needs a section name".to_string(),
                Some(claim.marker.token_range),
            ));
        }
        NotebookForm::Note {
            section: Some(section),
            ..
        } if section.is_empty() => {
            return Err(CaptureItemParseError::coded_at(
                "notebook_section_required",
                "a notebook section marker needs a section name".to_string(),
                Some(claim.marker.token_range),
            ));
        }
        NotebookForm::Note {
            block_id_range: Some(_),
            block_id: None,
            ..
        } => {
            return Err(CaptureItemParseError::coded_at(
                "notebook_block_id_required",
                "a notebook note route needs a block ID after `^` or `+`"
                    .to_string(),
                Some(claim.marker.token_range),
            ));
        }
        NotebookForm::SubNote {
            parent_block_id, ..
        } if parent_block_id.is_empty() => {
            return Err(CaptureItemParseError::coded_at(
                "notebook_block_id_required",
                "a notebook note route needs a block ID after `^` or `+`"
                    .to_string(),
                Some(claim.marker.token_range),
            ));
        }
        _ => {}
    }
    if clip.is_some() {
        claim.body = strip_clip_tokens(&claim.body);
    }
    if let NotebookForm::Note { block_id, .. } = &mut claim.marker.form
        && block_id.is_none()
    {
        let (body, trailing) = strip_trailing_caret_id(&claim.body);
        if trailing.is_some() {
            claim.body = body;
            *block_id = trailing;
        }
    }
    let sub_bullets = notebook_sub_bullets(item)?;
    if notebook_authored_block_has_checkbox(&claim.body, &sub_bullets) {
        return Err(CaptureItemParseError::coded_at(
            "notebook_holds_no_tasks",
            notebook_task_refusal(
                &claim.marker.route,
                claim.info.owner.as_deref(),
            ),
            Some(claim.marker.token_range),
        ));
    }
    let empty_ok = !sub_bullets.is_empty()
        || clip.is_some()
        || matches!(claim.marker.form, NotebookForm::SubNote { .. });
    if claim.body.trim().is_empty() && !empty_ok {
        return Err(CaptureItemParseError::coded_at(
            "notebook_note_required",
            "a notebook route needs note text; add the note after the route"
                .to_string(),
            Some(claim.marker.token_range),
        ));
    }
    let route = claim.marker.route.clone();
    let kind = notebook_kind(&claim);
    let local_marker = if claim.inherited {
        None
    } else {
        local_marker.or_else(|| {
            item.source
                .get(claim.marker.token_range.0..claim.marker.token_range.1)
                .map(|text| text.trim().to_string())
        })
    };
    Ok(super::item::parsed_capture_item_outcome(
        item,
        ParsedCaptureText {
            body: claim.body,
            clip,
            route: Some(route),
            kind,
            scheduled_offset: None,
            priority_level: None,
            sub_bullets,
            dependencies: Vec::new(),
            blocked_tasks: Vec::new(),
            relations: Vec::new(),
            dependency_target: None,
            type_marker: None,
            merge_in: None,
        },
        Vec::new(),
        local_marker,
    ))
}

fn claim_forced_notebook<'a>(
    item: &CaptureItem<'a>,
    forced_route: &str,
    forced_section: Option<&str>,
    options: &CaptureParseOptions<'_>,
) -> Result<Option<ParsedCaptureItemOutcome<'a>>, CaptureItemParseError> {
    let Some(policy) = options.notebook_policy else {
        return Ok(None);
    };
    let Some(info) = policy.get(forced_route) else {
        return Ok(None);
    };
    let route = forced_route.to_ascii_lowercase();
    let owner = owner_of(info);
    if options.forced_drawer.is_some() {
        return Err(CaptureItemParseError::coded(
            "notebook_marker_unsupported",
            super::line::notebook_no_drawers_error(&route),
        ));
    }
    let range = item
        .lines
        .first()
        .map(|line| (line.raw.start, line.raw.end))
        .unwrap_or((item.start, item.end));
    let clip = refuse_notebook_item_markers(item, &route, owner, range)?;
    let body = item
        .lines
        .first()
        .map(|line| line.raw.text.trim().to_string())
        .unwrap_or_default();
    let form = if let Some(parent) = options.forced_task {
        if parent.is_empty() || !super::tokens::is_block_id(parent) {
            return Err(CaptureItemParseError::coded(
                "notebook_block_id_required",
                "a notebook note route needs a block ID after `^` or `+`"
                    .to_string(),
            ));
        }
        NotebookForm::SubNote {
            parent_block_id: parent.to_string(),
            block_id_range: None,
        }
    } else {
        NotebookForm::Note {
            section: forced_section.map(str::to_string),
            exact_section: forced_section.is_some(),
            block_id: None,
            section_range: None,
            block_id_range: None,
        }
    };
    let claim = NotebookClaim {
        body,
        marker: RouteMarker {
            route,
            route_range: range,
            token_range: range,
            form,
        },
        info: info.clone(),
        inherited: false,
    };
    finish_notebook_claim(item, claim, clip, None).map(Some)
}

fn claim_list_notebook<'a>(
    item: &CaptureItem<'a>,
    options: &CaptureParseOptions<'_>,
) -> Result<Option<Vec<ParsedCaptureItemOutcome<'a>>>, CaptureItemParseError> {
    let Some(list) = item.list.as_ref() else {
        return Ok(None);
    };
    let Some(policy) = options.notebook_policy else {
        return Ok(None);
    };
    let info = match policy.lookup(&list.route, item.start) {
        NotebookRouteLookup::Hit(info) => info.clone(),
        NotebookRouteLookup::Miss => return Ok(None),
        NotebookRouteLookup::UsedBeforeCreated { marker_text } => {
            return Err(CaptureItemParseError::coded_at(
                "notebook_used_before_created",
                super::line::notebook_used_before_created_error(
                    &list.route,
                    marker_text,
                ),
                Some((list.header_start, list.header_end)),
            ));
        }
    };
    let route = list.route.clone();
    let owner = owner_of(&info);
    let header_range = (list.header_start, list.header_end);
    if list.ownerless {
        return Ok(None);
    }
    if list.count == 0 && list.kind == super::model::ListKind::Tasks {
        return Err(CaptureItemParseError::coded_at(
            "notebook_holds_no_tasks",
            super::line::notebook_holds_no_tasks_error(&route, owner),
            Some(header_range),
        ));
    }
    if list.has_section_hash && list.section.is_none() {
        return Err(CaptureItemParseError::coded_at(
            "notebook_section_required",
            notebook_section_required_error(),
            Some(header_range),
        ));
    }
    if list.count == 0 {
        let header = item
            .source
            .get(list.header_start..list.header_end)
            .unwrap_or("")
            .trim();
        return Err(CaptureItemParseError::coded(
            "empty_list",
            super::project_tasks::empty_list_error(header, true),
        ));
    }
    let outcome = plan_list_item(item, &route);
    let is_section_bullet = match &outcome {
        NotebookBodyOutcome::Ready(plan) => !plan.sections.is_empty(),
        NotebookBodyOutcome::Pending { needs, .. } => {
            needs.contains(&Need::Section)
                || needs.contains(&Need::SectionNotes)
        }
        NotebookBodyOutcome::Failed { diagnostics, .. } => {
            diagnostics.iter().any(|diagnostic| {
                matches!(
                    diagnostic.code,
                    "section_bullet_text"
                        | "section_bullet_depth"
                        | "empty_section_bullet"
                )
            })
        }
    };
    if is_section_bullet {
        if let Some(header) = list.section.as_deref() {
            let headings = info.sections.clone();
            let title =
                match resolve_notebook_section(header, list.exact, &headings) {
                    NotebookSectionResolution::Existing { title, .. }
                    | NotebookSectionResolution::Create { title } => title,
                    _ => header.to_string(),
                };
            let authored = item
                .lines
                .first()
                .and_then(|line| scan_leading(line.raw.text, line.raw.start))
                .and_then(|hit| match hit {
                    SectionBulletHit::Complete { authored, .. } => {
                        Some(authored)
                    }
                    _ => None,
                })
                .unwrap_or_else(|| "#".to_string());
            return Err(CaptureItemParseError::coded_at(
                "section_bullet_position",
                section_bullet_position_error(
                    &title, &authored, &route, header,
                ),
                Some(header_range),
            ));
        }
        return match outcome {
            NotebookBodyOutcome::Failed { diagnostics, .. } => {
                Err(diagnostic_error(&diagnostics[0]))
            }
            NotebookBodyOutcome::Pending {
                needs, placeholder, ..
            } if needs.contains(&Need::Section) => {
                Err(CaptureItemParseError::coded_at(
                    "notebook_section_required",
                    notebook_section_required_error(),
                    placeholder.map(|span| (span.start, span.end)),
                ))
            }
            NotebookBodyOutcome::Pending { .. } => {
                let authored = item
                    .lines
                    .first()
                    .map(|line| line.raw.text.trim())
                    .unwrap_or("#");
                Err(CaptureItemParseError::coded_at(
                    "empty_section_bullet",
                    empty_section_bullet_error(authored),
                    item.lines
                        .first()
                        .map(|line| (line.raw.start, line.raw.end)),
                ))
            }
            NotebookBodyOutcome::Ready(plan) => {
                if let Some(section) = plan.sections.first() {
                    let headings = staged_headings(
                        policy,
                        item.source,
                        &route,
                        section.selector_span.start,
                    );
                    if let Some(diagnostic) = section_resolution_diagnostic(
                        &route,
                        &section.selector,
                        section.exact,
                        &headings,
                        Some((
                            section.selector_span.start,
                            section.selector_span.end,
                        )),
                    ) {
                        return Err(diagnostic_error(&diagnostic));
                    }
                }
                fanout_list_section_notes(
                    item, &route, owner, &info, &plan, list,
                )
                .map(Some)
            }
        };
    }
    let clip = refuse_notebook_item_markers(item, &route, owner, header_range)?;
    let body = item
        .lines
        .first()
        .map(|line| line.raw.text.trim().to_string())
        .unwrap_or_default();
    let body = if matches!(
        scan_leading(
            item.lines.first().map(|line| line.raw.text).unwrap_or(""),
            item.lines.first().map(|line| line.raw.start).unwrap_or(0),
        ),
        Some(SectionBulletHit::Escaped { .. })
    ) {
        super::section_bullet::unescape_escaped_body(&body)
    } else {
        body
    };
    let claim = NotebookClaim {
        body,
        marker: RouteMarker {
            route,
            route_range: header_range,
            token_range: header_range,
            form: NotebookForm::Note {
                section: list.section.clone(),
                exact_section: list.exact,
                block_id: None,
                section_range: None,
                block_id_range: None,
            },
        },
        info,
        inherited: false,
    };
    finish_notebook_claim(item, claim, clip, Some(format!("@{}", list.route)))
        .map(|outcome| Some(vec![outcome]))
}

fn diagnostic_error(diagnostic: &Diagnostic) -> CaptureItemParseError {
    CaptureItemParseError::coded_at(
        diagnostic.code,
        diagnostic.message.clone(),
        diagnostic.range,
    )
}

fn fanout_list_section_notes<'a>(
    item: &CaptureItem<'a>,
    route: &str,
    owner: Option<&str>,
    info: &NotebookRouteInfo,
    plan: &super::section_bullet::NotebookBodyPlan,
    list: &super::model::ListContext,
) -> Result<Vec<ParsedCaptureItemOutcome<'a>>, CaptureItemParseError> {
    let Some(section) = plan.sections.first().cloned() else {
        return Ok(Vec::new());
    };
    let mut outcomes = Vec::new();
    let count = section.notes.len();
    for (index, note) in section.notes.iter().enumerate() {
        let child = CaptureItem {
            source: item.source,
            index: item.index,
            start: note.source_range.start,
            end: note.source_range.end,
            line_start: note.line_number,
            line_end: note.line_number,
            lines: vec![super::model::ItemLine {
                raw: super::model::RawLine {
                    text: item
                        .source
                        .get(note.source_range.start..note.source_range.end)
                        .unwrap_or(""),
                    start: note.source_range.start,
                    end: note.source_range.end,
                },
                line_number: note.line_number,
            }],
            list: item.list.clone(),
        };
        let clip = refuse_notebook_item_markers(
            &child,
            route,
            owner,
            (note.source_range.start, note.source_range.end),
        )?;
        let claim = NotebookClaim {
            body: note.text.clone(),
            marker: RouteMarker {
                route: route.to_string(),
                route_range: (list.header_start, list.header_end),
                token_range: (list.header_start, list.header_end),
                form: NotebookForm::Note {
                    section: Some(section.selector.clone()),
                    exact_section: section.exact,
                    block_id: note.block_id.clone(),
                    section_range: Some((
                        section.selector_span.start,
                        section.selector_span.end,
                    )),
                    block_id_range: None,
                },
            },
            info: info.clone(),
            inherited: false,
        };
        let mut outcome = finish_notebook_claim(
            &child,
            claim,
            clip,
            Some(format!("@{}", list.route)),
        )?;
        outcome.start = note.source_range.start;
        outcome.end = note.source_range.end;
        outcome.line_start = note.line_number;
        outcome.line_end = note.line_number;
        outcome.fanout = Some(FanoutContext {
            line: section.line_number,
            position: index + 1,
            count,
            marker: section.authored_marker.clone(),
        });
        outcomes.push(outcome);
    }
    Ok(outcomes)
}

pub(super) fn claim_strict_notebook<'a>(
    item: &CaptureItem<'a>,
    forced_route: Option<&str>,
    forced_section: Option<&str>,
    options: &CaptureParseOptions<'_>,
) -> Result<Option<ParsedCaptureItemOutcome<'a>>, CaptureItemParseError> {
    if let Some(forced) = forced_route {
        return claim_forced_notebook(item, forced, forced_section, options);
    }
    if forced_section.is_some() || options.explicit_destination {
        return Ok(None);
    }
    if item.list.is_some() {
        return Ok(None);
    }
    if item_is_claimed_operator(item) {
        return Ok(None);
    }
    let claim = match classify_item(
        item,
        options.notebook_policy,
        options.global_route,
    ) {
        Ok(claim) => claim,
        Err(RouteScan::Unsupported {
            message,
            range,
            code,
            route,
        }) => {
            let owner = options
                .notebook_policy
                .and_then(|policy| policy.get(&route))
                .and_then(|info| info.owner.clone());
            let (code, message) = unsupported_with_optional_note(
                item,
                code,
                message,
                &route,
                owner.as_deref(),
            );
            return Err(CaptureItemParseError::coded_at(
                code,
                message,
                Some(range),
            ));
        }
        Err(RouteScan::Supported(_)) | Err(RouteScan::NotNotebookRoute) => None,
    };
    let Some(claim) = claim else {
        return Ok(None);
    };
    let owner = owner_of(&claim.info);
    let clip = refuse_notebook_item_markers(
        item,
        &claim.marker.route,
        owner,
        claim.marker.token_range,
    )?;
    finish_notebook_claim(item, claim, clip, None).map(Some)
}

#[derive(Debug, Clone)]
struct NotebookCreateShape {
    parent: String,
    notebook_id: String,
    token_range: (usize, usize),
    route_range: (usize, usize),
    id_range: (usize, usize),
    marker_text: String,
    body: String,
}

/// A complete `@parent^id@` seal plus `%n`. Independent of whether the
/// parent is already a notebook, so area and project parents claim too.
fn complete_notebook_create_shape(
    item: &CaptureItem<'_>,
) -> Option<NotebookCreateShape> {
    if item.list.is_some() {
        return None;
    }
    if !item_has_note_token(item) {
        return None;
    }
    let scan = edge_marker(item)?;
    let RouteScan::Unsupported {
        code: "notebook_project_seal",
        route,
        range,
        ..
    } = scan
    else {
        return None;
    };
    let token = item.source.get(range.0..range.1)?.trim();
    let rest = token.strip_prefix('@')?;
    let (parent, rest) = rest.split_once('^')?;
    let id = rest.strip_suffix('@')?;
    if parent.to_ascii_lowercase() != route
        || !super::is_route_token(parent)
        || id.is_empty()
        || !super::tokens::is_block_id(id)
    {
        return None;
    }
    let parent_len = parent.len();
    let route_range = (range.0, range.0 + 1 + parent_len);
    let id_range = (route_range.1 + 1, range.1 - 1);
    let marker = RouteMarker {
        route: route.clone(),
        route_range,
        token_range: range,
        form: NotebookForm::Note {
            section: None,
            exact_section: false,
            block_id: Some(id.to_string()),
            section_range: None,
            block_id_range: Some(id_range),
        },
    };
    let body = strip_note_tokens(&body_without_marker(item, &marker));
    Some(NotebookCreateShape {
        parent: parent.to_ascii_lowercase(),
        notebook_id: id.to_string(),
        token_range: range,
        route_range,
        id_range,
        marker_text: token.to_string(),
        body,
    })
}

fn strip_note_tokens(body: &str) -> String {
    super::model::tokenize_with_spans(body)
        .into_iter()
        .filter(|token| token.text != "%n")
        .map(|token| token.text)
        .collect::<Vec<_>>()
        .join(" ")
}

fn provisional_create_title(id: &str) -> String {
    let title = crate::native::highlights_ref::humanize_stem(id);
    let mut chars = title.chars();
    match chars.next() {
        None => id.to_string(),
        Some(first) => first.to_uppercase().chain(chars).collect(),
    }
}

fn provisional_create_sections(item: &CaptureItem<'_>) -> Vec<NotebookSection> {
    notebook_create_seed_sections_from_item(item)
}

fn refuse_notebook_create_markers(
    item: &CaptureItem<'_>,
    token_range: (usize, usize),
) -> Result<(), CaptureItemParseError> {
    if item_is_claimed_operator(item) {
        return Err(CaptureItemParseError::coded_at(
            "notebook_create_unsupported",
            super::line::notebook_create_marker_unsupported_error(">/ <"),
            Some(token_range),
        ));
    }
    let occurrences = super::directive::collect_item(item);
    let directives = match super::directive::resolve(&occurrences) {
        Ok(directives) => directives,
        Err((message, code, span)) => {
            return Err(CaptureItemParseError::coded_at(
                code,
                message,
                Some((span.start, span.end)),
            ));
        }
    };
    if directives.clip.is_some() {
        return Err(CaptureItemParseError::coded_at(
            "notebook_create_unsupported",
            "notebook creation cannot be combined with %c".to_string(),
            Some(token_range),
        ));
    }
    if directives.listen {
        return Err(CaptureItemParseError::coded_at(
            "notebook_create_unsupported",
            super::line::notebook_create_marker_unsupported_error("%l"),
            Some(token_range),
        ));
    }
    if directives.template {
        return Err(CaptureItemParseError::coded_at(
            "notebook_create_unsupported",
            super::line::notebook_create_marker_unsupported_error("%t"),
            Some(token_range),
        ));
    }
    let types = super::type_marker::collect_item(item);
    if let Some(occurrence) = types.iter().find(|item| item.is_marker()) {
        let raw = match occurrence {
            super::type_marker::TypeMarkerOccurrence::Complete(marker) => {
                format!("#{}", marker.name)
            }
            super::type_marker::TypeMarkerOccurrence::Incomplete { .. } => {
                "#".to_string()
            }
            super::type_marker::TypeMarkerOccurrence::Invalid {
                raw, ..
            } => raw.clone(),
            super::type_marker::TypeMarkerOccurrence::Escaped { .. } => {
                String::new()
            }
        };
        return Err(CaptureItemParseError::coded_at(
            "notebook_create_unsupported",
            super::line::notebook_create_marker_unsupported_error(&raw),
            Some(token_range),
        ));
    }
    let deps = super::dependencies::scan_item_dependencies(item);
    if deps.has_entries() || deps.has_partial_tokens() {
        return Err(CaptureItemParseError::coded_at(
            "notebook_create_unsupported",
            super::line::notebook_create_marker_unsupported_error("&/*/~"),
            Some(token_range),
        ));
    }
    if let Some(parent) = item.lines.first() {
        for token in super::editor_parse::tokenize_line_with_spans(&parent.raw)
        {
            if super::markers::parse_schedule_token(token.text).is_some() {
                return Err(CaptureItemParseError::coded_at(
                    "notebook_create_unsupported",
                    super::line::notebook_create_marker_unsupported_error(
                        "s:N",
                    ),
                    Some((token.start, token.end)),
                ));
            }
            if super::markers::parse_priority_token(token.text).is_some() {
                return Err(CaptureItemParseError::coded_at(
                    "notebook_create_unsupported",
                    super::line::notebook_create_marker_unsupported_error("$N"),
                    Some((token.start, token.end)),
                ));
            }
            if token.text == "!"
                || (token.text.starts_with('=') && token.text != "==")
            {
                let marker = if token.text == "!" { "!" } else { "=" };
                return Err(CaptureItemParseError::coded_at(
                    "notebook_create_unsupported",
                    super::line::notebook_create_marker_unsupported_error(
                        marker,
                    ),
                    Some((token.start, token.end)),
                ));
            }
        }
    }
    refuse_notebook_create_children(item, token_range)
}

fn created_notebook_route(item: &CaptureItem<'_>) -> String {
    complete_notebook_create_shape(item)
        .map(|shape| {
            let filename =
                crate::native::capture_project_note::project_note_basename(
                    &shape.parent,
                    &shape.notebook_id,
                );
            filename
                .strip_suffix(".md")
                .unwrap_or(&filename)
                .to_ascii_lowercase()
        })
        .unwrap_or_default()
}

fn first_empty_section_authored(item: &CaptureItem<'_>) -> String {
    for line in item.lines.iter().skip(1) {
        let super::draft::AuthoredLineClass::Item(authored) =
            super::draft::classify_authored_line(line.raw)
        else {
            continue;
        };
        if authored.depth != super::model::AuthoredDepth::First {
            continue;
        }
        if let Some(SectionBulletHit::Complete {
            leftover,
            authored: marker,
            ..
        }) = scan_leading(authored.body, authored.body_start)
            && leftover.is_empty()
        {
            return marker;
        }
    }
    "#".to_string()
}

fn refuse_notebook_create_children(
    item: &CaptureItem<'_>,
    token_range: (usize, usize),
) -> Result<(), CaptureItemParseError> {
    for line in item.lines.iter().skip(1) {
        match super::draft::classify_authored_line(line.raw) {
            super::draft::AuthoredLineClass::EmptyOrPlaceholder => continue,
            super::draft::AuthoredLineClass::Invalid => {
                if super::section_bullet::looks_like_deeper_bullet(
                    line.raw.text,
                ) {
                    continue;
                }
                return Err(CaptureItemParseError::coded(
                    "invalid_child_line",
                    super::line::invalid_child_line_error(line.line_number),
                ));
            }
            super::draft::AuthoredLineClass::Item(authored) => {
                if authored.depth == super::model::AuthoredDepth::First {
                    if let Some(hit) = super::move_bullet::scan_move_bullet(
                        authored.body,
                        authored.body_start,
                        item.source,
                    ) {
                        let _ = hit;
                        return Err(CaptureItemParseError::coded_at(
                            "notebook_create_unsupported",
                            super::line::notebook_create_move_error(),
                            Some(token_range),
                        ));
                    }
                    let mut tokens =
                        super::editor_parse::tokenize_line_with_spans(
                            &super::model::RawLine {
                                text: authored.body,
                                start: authored.body_start,
                                end: line.raw.end,
                            },
                        );
                    if let Some(stripped) =
                        super::project_tasks::strip_trailing_task_id(
                            &mut tokens,
                        )
                        && matches!(
                            stripped.lex,
                            super::project_tasks::TaskIdLex::Valid {
                                sigil: ':',
                                ..
                            } | super::project_tasks::TaskIdLex::Lone {
                                sigil: ':'
                            } | super::project_tasks::TaskIdLex::Invalid {
                                sigil: ':',
                                ..
                            }
                        )
                    {
                        return Err(CaptureItemParseError::coded_at(
                            "notebook_create_unsupported",
                            super::line::notebook_create_task_id_error(),
                            Some(token_range),
                        ));
                    }
                }
            }
        }
    }
    let sub_bullets = match notebook_sub_bullets(item) {
        Ok(bullets) => bullets,
        Err(error)
            if error.code == Some("invalid_child_line")
                && item.lines.iter().skip(1).any(|line| {
                    super::section_bullet::looks_like_deeper_bullet(
                        line.raw.text,
                    )
                }) =>
        {
            Vec::new()
        }
        Err(error) => return Err(error),
    };
    let body = item.lines.first().map(|line| line.raw.text).unwrap_or("");
    if notebook_authored_block_has_checkbox(body, &sub_bullets) {
        return Err(CaptureItemParseError::coded_at(
            "notebook_holds_no_tasks",
            "notebooks hold no tasks; drop checkbox lines outside fenced code"
                .to_string(),
            Some(token_range),
        ));
    }
    Ok(())
}

fn refuse_notebook_create_plan(
    item: &CaptureItem<'_>,
) -> Result<(), CaptureItemParseError> {
    let route = created_notebook_route(item);
    match plan_create_item(item, &route) {
        NotebookBodyOutcome::Failed { diagnostics, .. } => {
            Err(diagnostic_error(&diagnostics[0]))
        }
        NotebookBodyOutcome::Pending {
            needs, placeholder, ..
        } if needs.contains(&Need::Section) => {
            Err(CaptureItemParseError::coded_at(
                "notebook_section_required",
                notebook_section_required_error(),
                placeholder.map(|span| (span.start, span.end)),
            ))
        }
        NotebookBodyOutcome::Pending { needs, .. }
            if needs.contains(&Need::SectionNotes) =>
        {
            let authored = first_empty_section_authored(item);
            Err(CaptureItemParseError::coded_at(
                "empty_section_bullet",
                empty_section_bullet_error(&authored),
                item.lines.get(1).map(|line| (line.raw.start, line.raw.end)),
            ))
        }
        NotebookBodyOutcome::Pending { .. } | NotebookBodyOutcome::Ready(_) => {
            Ok(())
        }
    }
}

fn claim_notebook_create<'a>(
    item: &CaptureItem<'a>,
) -> Result<Option<ParsedCaptureItemOutcome<'a>>, CaptureItemParseError> {
    let Some(claim) = complete_notebook_create_shape(item) else {
        return Ok(None);
    };
    refuse_notebook_create_markers(item, claim.token_range)?;
    refuse_notebook_create_plan(item)?;
    let sub_bullets = notebook_sub_bullets(item)?;
    let local_marker = item
        .source
        .get(claim.token_range.0..claim.token_range.1)
        .map(|text| text.trim().to_string());
    Ok(Some(super::item::parsed_capture_item_outcome(
        item,
        ParsedCaptureText {
            body: claim.body,
            clip: None,
            route: Some(claim.parent.clone()),
            kind: CaptureKind::NotebookCreate {
                parent: claim.parent,
                notebook_id: claim.notebook_id,
            },
            scheduled_offset: None,
            priority_level: None,
            sub_bullets,
            dependencies: Vec::new(),
            blocked_tasks: Vec::new(),
            relations: Vec::new(),
            dependency_target: None,
            type_marker: None,
            merge_in: None,
        },
        Vec::new(),
        local_marker,
    )))
}

fn notebook_create_editor_view(
    item: &CaptureItem<'_>,
    options: &EditorParseOptions<'_>,
) -> Option<NotebookEditorView> {
    let claim = complete_notebook_create_shape(item)?;
    if let Err(error) = refuse_notebook_create_markers(item, claim.token_range)
    {
        return Some(NotebookEditorView {
            mode: EditorMode::NotebookCreate,
            body: claim.body,
            route: claim.parent,
            section: None,
            block_id: Some(claim.notebook_id),
            needs: Vec::new(),
            spans: vec![Span {
                start: claim.token_range.0,
                end: claim.token_range.1,
                kind: SpanKind::ProjectNoteMarker,
                project_note: None,
                title: None,
                key: None,
                section_action: None,
            }],
            diagnostics: vec![Diagnostic {
                severity: Severity::Error,
                code: error.code.unwrap_or("notebook_create_unsupported"),
                message: error.message,
                range: error.span.or(Some(claim.token_range)),
                rewrite: None,
            }],
            sub_bullets: notebook_sub_bullets(item).unwrap_or_default(),
        });
    }
    let mut spans = vec![
        Span {
            start: claim.route_range.0,
            end: claim.route_range.1,
            kind: SpanKind::NotebookRoute,
            project_note: None,
            title: Some(provisional_create_title(&claim.notebook_id)),
            key: Some(claim.parent.clone()),
            section_action: None,
        },
        Span {
            start: claim.id_range.0,
            end: claim.id_range.1,
            kind: SpanKind::NotebookBlockId,
            project_note: None,
            title: None,
            key: None,
            section_action: None,
        },
    ];
    let occurrences = super::directive::collect_item(item);
    for occurrence in occurrences {
        if matches!(
            occurrence,
            super::directive::DirectiveOccurrence::Complete {
                kind: super::directive::DirectiveKind::Note,
                ..
            }
        ) {
            let span = occurrence.span();
            spans.push(Span {
                start: span.start,
                end: span.end,
                kind: SpanKind::Directive,
                project_note: None,
                title: None,
                key: None,
                section_action: None,
            });
        }
    }
    let created = created_notebook_route(item);
    let outcome = plan_create_item(item, &created);
    let mut needs = Vec::new();
    let mut diagnostics = Vec::new();
    let mut mode = EditorMode::NotebookCreate;
    let mut section = None;
    match outcome {
        NotebookBodyOutcome::Failed {
            diagnostics: extra, ..
        } => {
            diagnostics.extend(extra);
        }
        NotebookBodyOutcome::Pending {
            needs: extra,
            placeholder,
            diagnostics: extra_diag,
            plan,
        } => {
            diagnostics.extend(extra_diag);
            needs.extend(extra.iter().copied());
            mode = EditorMode::Incomplete;
            if extra.contains(&Need::Section)
                && let Some(span) = placeholder
            {
                spans.push(Span {
                    start: span.start,
                    end: span.end,
                    kind: SpanKind::InteractivePlaceholder,
                    project_note: None,
                    title: None,
                    key: None,
                    section_action: None,
                });
            }
            section = plan
                .sections
                .first()
                .map(|entry| entry.selector.clone())
                .or_else(|| first_complete_section_selector(item));
            push_create_body_section_spans(&mut spans, item, options, &created);
        }
        NotebookBodyOutcome::Ready(plan) => {
            section = plan.sections.first().map(|entry| entry.selector.clone());
            push_create_body_section_spans(&mut spans, item, options, &created);
        }
    }
    spans.sort_by_key(|span| (span.start, span.end));
    if claim.body.trim().is_empty() {
        diagnostics.push(Diagnostic {
            severity: Severity::Error,
            code: "notebook_summary_required",
            message: "a notebook needs a one-line summary".to_string(),
            range: Some(claim.token_range),
            rewrite: None,
        });
    }
    Some(NotebookEditorView {
        mode,
        body: claim.body,
        route: claim.parent,
        section,
        block_id: Some(claim.notebook_id),
        needs,
        spans,
        diagnostics,
        sub_bullets: notebook_sub_bullets(item).unwrap_or_default(),
    })
}

fn first_complete_section_selector(item: &CaptureItem<'_>) -> Option<String> {
    for line in item.lines.iter().skip(1) {
        let super::draft::AuthoredLineClass::Item(authored) =
            super::draft::classify_authored_line(line.raw)
        else {
            continue;
        };
        if authored.depth != super::model::AuthoredDepth::First {
            continue;
        }
        if let Some(SectionBulletHit::Complete {
            leftover, selector, ..
        }) = scan_leading(authored.body, authored.body_start)
            && leftover.is_empty()
        {
            return Some(selector);
        }
    }
    None
}

fn push_create_body_section_spans(
    spans: &mut Vec<Span>,
    item: &CaptureItem<'_>,
    options: &EditorParseOptions<'_>,
    created: &str,
) {
    let Some(policy) = options.notebook_policy else {
        return;
    };
    for line in item.lines.iter().skip(1) {
        let super::draft::AuthoredLineClass::Item(authored) =
            super::draft::classify_authored_line(line.raw)
        else {
            continue;
        };
        if authored.depth != super::model::AuthoredDepth::First {
            continue;
        }
        if let Some(SectionBulletHit::Complete {
            span,
            selector,
            exact,
            leftover,
            ..
        }) = scan_leading(authored.body, authored.body_start)
            && leftover.is_empty()
        {
            spans.push(section_span(
                policy,
                item.source,
                created,
                &selector,
                exact,
                span,
            ));
        }
    }
}

/// Fan out a marker-only `@nb+id` plus first-level bullets into one
/// sub-note per bullet. Ordinary `@route+id` list headers stay unchanged.
pub(super) fn claim_notebook_members<'a>(
    item: &CaptureItem<'a>,
    forced_route: Option<&str>,
    forced_section: Option<&str>,
    options: &CaptureParseOptions<'_>,
) -> Result<Option<Vec<ParsedCaptureItemOutcome<'a>>>, CaptureItemParseError> {
    if forced_route.is_none()
        && forced_section.is_none()
        && !options.explicit_destination
        && item.list.is_none()
        && let Some(outcome) = claim_notebook_create(item)?
    {
        return Ok(Some(vec![outcome]));
    }
    if forced_route.is_none()
        && forced_section.is_none()
        && !options.explicit_destination
        && item.list.is_some()
    {
        if let Some(outcomes) = claim_list_notebook(item, options)? {
            return Ok(Some(outcomes));
        }
        return Ok(None);
    }
    if forced_route.is_some()
        || forced_section.is_some()
        || options.explicit_destination
        || item.list.is_some()
    {
        if let Some(outcome) =
            claim_strict_notebook(item, forced_route, forced_section, options)?
        {
            return Ok(Some(vec![outcome]));
        }
        return Ok(None);
    }
    if let Some(outcomes) = fanout_notebook_plus(item, options)? {
        return Ok(Some(outcomes));
    }
    if let Some(outcome) =
        claim_strict_notebook(item, forced_route, forced_section, options)?
    {
        return Ok(Some(vec![outcome]));
    }
    Ok(None)
}

fn fanout_notebook_plus<'a>(
    item: &CaptureItem<'a>,
    options: &CaptureParseOptions<'_>,
) -> Result<Option<Vec<ParsedCaptureItemOutcome<'a>>>, CaptureItemParseError> {
    let Some(policy) = options.notebook_policy else {
        return Ok(None);
    };
    let Some(parent) = item.lines.first() else {
        return Ok(None);
    };
    let trimmed = parent.raw.text.trim();
    let Some(marker) = trimmed.strip_prefix('@') else {
        return Ok(None);
    };
    if marker.starts_with('@') {
        return Ok(None);
    }
    let Some((route_part, rest)) = marker.split_once('+') else {
        return Ok(None);
    };
    if rest.contains(['#', '^', ':', '=', '!', '/']) {
        return Ok(None);
    }
    if !super::markers::is_route_token(route_part)
        || !super::tokens::is_block_id(rest)
    {
        return Ok(None);
    }
    match policy.lookup(route_part, item.start) {
        NotebookRouteLookup::Hit(_) => {}
        NotebookRouteLookup::Miss => return Ok(None),
        NotebookRouteLookup::UsedBeforeCreated { marker_text } => {
            return Err(CaptureItemParseError::coded_at(
                "notebook_used_before_created",
                super::line::notebook_used_before_created_error(
                    route_part,
                    marker_text,
                ),
                Some((parent.raw.start, parent.raw.end)),
            ));
        }
    }
    if trimmed.split_whitespace().nth(1).is_some() {
        return Ok(None);
    }
    let first_level = item.lines.iter().skip(1).any(|line| {
        matches!(
            super::draft::classify_authored_line(line.raw),
            super::draft::AuthoredLineClass::Item(authored)
                if authored.depth == super::model::AuthoredDepth::First
        )
    });
    if !first_level {
        return Ok(None);
    }
    let mut outcomes = Vec::new();
    let mut current: Vec<super::model::ItemLine<'a>> = Vec::new();
    let flush = |block: &[super::model::ItemLine<'a>],
                 outcomes: &mut Vec<ParsedCaptureItemOutcome<'a>>|
     -> Result<(), CaptureItemParseError> {
        let Some(first) = block.first() else {
            return Ok(());
        };
        let authored = match super::draft::classify_authored_line(first.raw) {
            super::draft::AuthoredLineClass::Item(authored)
                if authored.depth == super::model::AuthoredDepth::First =>
            {
                authored
            }
            _ => return Ok(()),
        };
        let child = CaptureItem {
            source: item.source,
            index: item.index,
            start: first.raw.start,
            end: block
                .last()
                .map(|line| line.raw.end)
                .unwrap_or(first.raw.end),
            line_start: first.line_number,
            line_end: block
                .last()
                .map(|line| line.line_number)
                .unwrap_or(first.line_number),
            lines: std::iter::once(super::model::ItemLine {
                raw: super::model::RawLine {
                    text: authored.body,
                    start: authored.body_start,
                    end: first.raw.end,
                },
                line_number: first.line_number,
            })
            .chain(block.iter().skip(1).copied())
            .collect(),
            list: None,
        };
        let info = match policy.lookup(route_part, item.start) {
            NotebookRouteLookup::Hit(info) => info.clone(),
            _ => panic!("notebook plus fan-out checks the policy"),
        };
        let claim = NotebookClaim {
            body: authored.body.trim().to_string(),
            marker: RouteMarker {
                route: route_part.to_ascii_lowercase(),
                route_range: (parent.raw.start, parent.raw.end),
                token_range: (parent.raw.start, parent.raw.end),
                form: NotebookForm::SubNote {
                    parent_block_id: rest.to_string(),
                    block_id_range: None,
                },
            },
            info,
            inherited: false,
        };
        let owner = owner_of(&claim.info);
        let clip = refuse_notebook_item_markers(
            &child,
            &claim.marker.route,
            owner,
            claim.marker.token_range,
        )?;
        outcomes.push(finish_notebook_claim(
            &child,
            claim,
            clip,
            Some(trimmed.to_string()),
        )?);
        Ok(())
    };
    for line in item.lines.iter().skip(1).copied() {
        match super::draft::classify_authored_line(line.raw) {
            super::draft::AuthoredLineClass::EmptyOrPlaceholder => continue,
            super::draft::AuthoredLineClass::Item(authored)
                if authored.depth == super::model::AuthoredDepth::First =>
            {
                if !current.is_empty() {
                    flush(&current, &mut outcomes)?;
                    current.clear();
                }
                current.push(line);
            }
            _ => current.push(line),
        }
    }
    if !current.is_empty() {
        flush(&current, &mut outcomes)?;
    }
    let count = outcomes.len();
    let marker = trimmed.to_string();
    let line = parent.line_number;
    for (index, outcome) in outcomes.iter_mut().enumerate() {
        outcome.fanout = Some(FanoutContext {
            line,
            position: index + 1,
            count,
            marker: marker.clone(),
        });
    }
    Ok(Some(outcomes))
}

pub(crate) struct NotebookEditorView {
    pub(crate) mode: EditorMode,
    pub(crate) body: String,
    pub(crate) route: String,
    pub(crate) section: Option<String>,
    pub(crate) block_id: Option<String>,
    pub(crate) needs: Vec<Need>,
    pub(crate) spans: Vec<Span>,
    pub(crate) diagnostics: Vec<Diagnostic>,
    pub(crate) sub_bullets: Vec<AuthoredSubBullet>,
}

fn section_span(
    policy: &NotebookRoutePolicy,
    raw: &str,
    route: &str,
    selector: &str,
    exact: bool,
    span: Range<usize>,
) -> Span {
    let headings = staged_headings(policy, raw, route, span.start);
    let (title, action) =
        match resolve_notebook_section(selector, exact, &headings) {
            NotebookSectionResolution::Existing { title, .. } => {
                (title, Some("existing"))
            }
            NotebookSectionResolution::Create { title } => {
                (title, Some("create"))
            }
            _ => (selector.to_string(), None),
        };
    Span {
        start: span.start,
        end: span.end,
        kind: SpanKind::NotebookSection,
        project_note: None,
        title: Some(title),
        key: Some(if exact { "exact" } else { "slug" }.to_string()),
        section_action: action,
    }
}

fn list_section_bullet_editor_view(
    item: &CaptureItem<'_>,
    list: &super::model::ListContext,
    info: &NotebookRouteInfo,
    policy: &NotebookRoutePolicy,
) -> Option<NotebookEditorView> {
    let route = list.route.as_str();
    let outcome = plan_list_item(item, route);
    let is_section = match &outcome {
        NotebookBodyOutcome::Ready(plan) => !plan.sections.is_empty(),
        NotebookBodyOutcome::Pending { needs, .. } => {
            needs.contains(&Need::Section)
                || needs.contains(&Need::SectionNotes)
        }
        NotebookBodyOutcome::Failed { diagnostics, .. } => {
            diagnostics.iter().any(|diagnostic| {
                matches!(
                    diagnostic.code,
                    "section_bullet_text"
                        | "section_bullet_depth"
                        | "empty_section_bullet"
                )
            })
        }
    };
    if !is_section {
        return None;
    }
    let header_span = Span {
        start: list.header_start,
        end: list.header_end,
        kind: SpanKind::NotebookRoute,
        project_note: None,
        title: Some(info.title.clone()),
        key: info.owner.clone(),
        section_action: None,
    };
    if let Some(header) = list.section.as_deref() {
        let headings = info.sections.clone();
        let title =
            match resolve_notebook_section(header, list.exact, &headings) {
                NotebookSectionResolution::Existing { title, .. }
                | NotebookSectionResolution::Create { title } => title,
                _ => header.to_string(),
            };
        let authored = item
            .lines
            .first()
            .and_then(|line| scan_leading(line.raw.text, line.raw.start))
            .and_then(|hit| match hit {
                SectionBulletHit::Complete { authored, .. } => Some(authored),
                _ => None,
            })
            .unwrap_or_else(|| "#".to_string());
        return Some(notebook_editor_error(
            list.route.clone(),
            "section_bullet_position",
            section_bullet_position_error(&title, &authored, route, header),
            Some((list.header_start, list.header_end)),
        ));
    }
    match outcome {
        NotebookBodyOutcome::Failed { diagnostics, .. } => {
            let diagnostic = diagnostics.into_iter().next()?;
            Some(NotebookEditorView {
                mode: EditorMode::NotebookNote,
                body: String::new(),
                route: list.route.clone(),
                section: None,
                block_id: None,
                needs: Vec::new(),
                spans: vec![header_span],
                diagnostics: vec![diagnostic],
                sub_bullets: notebook_sub_bullets(item).unwrap_or_default(),
            })
        }
        NotebookBodyOutcome::Pending {
            needs,
            placeholder,
            diagnostics,
            plan,
        } => {
            let mut spans = vec![header_span];
            if needs.contains(&Need::Section)
                && let Some(span) = placeholder.clone()
            {
                spans.push(Span {
                    start: span.start,
                    end: span.end,
                    kind: SpanKind::InteractivePlaceholder,
                    project_note: None,
                    title: None,
                    key: None,
                    section_action: None,
                });
            } else if let Some(section) = plan.sections.first() {
                spans.push(section_span(
                    policy,
                    item.source,
                    route,
                    &section.selector,
                    section.exact,
                    section.selector_span.clone(),
                ));
            } else if let Some(first) = item.lines.first()
                && let Some(SectionBulletHit::Complete {
                    span,
                    selector,
                    exact,
                    ..
                }) = scan_leading(first.raw.text, first.raw.start)
            {
                spans.push(section_span(
                    policy,
                    item.source,
                    route,
                    &selector,
                    exact,
                    span,
                ));
            }
            Some(NotebookEditorView {
                mode: EditorMode::Incomplete,
                body: String::new(),
                route: list.route.clone(),
                section: plan
                    .sections
                    .first()
                    .map(|section| section.selector.clone()),
                block_id: None,
                needs,
                spans,
                diagnostics,
                sub_bullets: notebook_sub_bullets(item).unwrap_or_default(),
            })
        }
        NotebookBodyOutcome::Ready(plan) => {
            let section = plan.sections.first()?;
            let mut spans = vec![header_span];
            spans.push(section_span(
                policy,
                item.source,
                route,
                &section.selector,
                section.exact,
                section.selector_span.clone(),
            ));
            let mut diagnostics = Vec::new();
            let headings = staged_headings(
                policy,
                item.source,
                route,
                section.selector_span.start,
            );
            if let Some(diagnostic) = section_resolution_diagnostic(
                route,
                &section.selector,
                section.exact,
                &headings,
                Some((section.selector_span.start, section.selector_span.end)),
            ) {
                diagnostics.push(diagnostic);
            }
            Some(NotebookEditorView {
                mode: EditorMode::NotebookNote,
                body: String::new(),
                route: list.route.clone(),
                section: Some(section.selector.clone()),
                block_id: None,
                needs: Vec::new(),
                spans,
                diagnostics,
                sub_bullets: notebook_sub_bullets(item).unwrap_or_default(),
            })
        }
    }
}

pub(crate) fn editor_view_for_item(
    item: &CaptureItem<'_>,
    options: &EditorParseOptions<'_>,
) -> Option<NotebookEditorView> {
    if item.list.is_none()
        && let Some(view) = notebook_create_editor_view(item, options)
    {
        return Some(view);
    }
    if let Some(list) = item.list.as_ref()
        && let Some(policy) = options.notebook_policy
    {
        let info = match policy.lookup(&list.route, item.start) {
            NotebookRouteLookup::Hit(info) => info.clone(),
            NotebookRouteLookup::Miss => {
                // Fall through to ordinary list/task claiming.
                return None;
            }
            NotebookRouteLookup::UsedBeforeCreated { marker_text } => {
                return Some(notebook_editor_error(
                    list.route.clone(),
                    "notebook_used_before_created",
                    super::line::notebook_used_before_created_error(
                        &list.route,
                        marker_text,
                    ),
                    Some((list.header_start, list.header_end)),
                ));
            }
        };
        let info = &info;
        let owner = info.owner.as_deref();
        let header_range = (list.header_start, list.header_end);
        if list.count == 0 && list.kind == super::model::ListKind::Tasks {
            return Some(notebook_editor_error(
                list.route.clone(),
                "notebook_holds_no_tasks",
                super::line::notebook_holds_no_tasks_error(&list.route, owner),
                Some(header_range),
            ));
        }
        if list.has_section_hash && list.section.is_none() {
            let hash = item
                .source
                .get(list.header_start..list.header_end)
                .and_then(|header| header.find('#'))
                .map(|rel| list.header_start + rel)
                .unwrap_or(list.header_end.saturating_sub(1));
            return Some(NotebookEditorView {
                mode: EditorMode::Incomplete,
                body: String::new(),
                route: list.route.clone(),
                section: None,
                block_id: None,
                needs: vec![Need::Section],
                spans: vec![
                    Span {
                        start: list.header_start,
                        end: list.header_end,
                        kind: SpanKind::NotebookRoute,
                        project_note: None,
                        title: Some(info.title.clone()),
                        key: info.owner.clone(),
                        section_action: None,
                    },
                    Span {
                        start: hash,
                        end: hash + 1,
                        kind: SpanKind::InteractivePlaceholder,
                        project_note: None,
                        title: None,
                        key: None,
                        section_action: None,
                    },
                ],
                diagnostics: Vec::new(),
                sub_bullets: Vec::new(),
            });
        }
        if list.count == 0 {
            let header = item
                .source
                .get(list.header_start..list.header_end)
                .unwrap_or("")
                .trim();
            return Some(notebook_editor_error(
                list.route.clone(),
                "empty_list",
                super::project_tasks::empty_list_error(header, true),
                Some(header_range),
            ));
        }
        if let Some(view) =
            list_section_bullet_editor_view(item, list, info, policy)
        {
            return Some(view);
        }
        let clip = match refuse_notebook_item_markers(
            item,
            &list.route,
            owner,
            header_range,
        ) {
            Ok(clip) => clip,
            Err(error) => {
                return Some(notebook_editor_error(
                    list.route.clone(),
                    error.code.unwrap_or("notebook_marker_unsupported"),
                    error.message,
                    error.span.or(Some(header_range)),
                ));
            }
        };
        let body = item
            .lines
            .first()
            .map(|line| line.raw.text.trim().to_string())
            .unwrap_or_default();
        let body = if matches!(
            scan_leading(
                item.lines.first().map(|line| line.raw.text).unwrap_or(""),
                item.lines.first().map(|line| line.raw.start).unwrap_or(0),
            ),
            Some(SectionBulletHit::Escaped { .. })
        ) {
            super::section_bullet::unescape_escaped_body(&body)
        } else {
            body
        };
        let (mut body, trailing) = strip_trailing_caret_id(&body);
        if clip.is_some() {
            body = strip_clip_tokens(&body);
        }
        let mut needs = Vec::new();
        if body.trim().is_empty() && trailing.is_none() {
            needs.push(Need::NotebookNote);
        }
        let sub_bullets = notebook_sub_bullets(item).unwrap_or_default();
        if notebook_authored_block_has_checkbox(&body, &sub_bullets) {
            return Some(notebook_editor_error(
                list.route.clone(),
                "notebook_holds_no_tasks",
                notebook_task_refusal(&list.route, owner),
                Some(header_range),
            ));
        }
        let mut diagnostics = Vec::new();
        let mut spans = vec![Span {
            start: list.header_start,
            end: list.header_end,
            kind: SpanKind::NotebookRoute,
            project_note: None,
            title: Some(info.title.clone()),
            key: info.owner.clone(),
            section_action: None,
        }];
        if let Some(selector) = list.section.as_deref() {
            let headings =
                staged_headings(policy, item.source, &list.route, item.start);
            if let Some(diagnostic) = section_resolution_diagnostic(
                &list.route,
                selector,
                list.exact,
                &headings,
                Some(header_range),
            ) {
                diagnostics.push(diagnostic);
            }
            if let Some(hash) = item
                .source
                .get(list.header_start..list.header_end)
                .and_then(|header| header.find('#'))
            {
                let start = list.header_start + hash;
                spans.push(section_span(
                    policy,
                    item.source,
                    &list.route,
                    selector,
                    list.exact,
                    start..list.header_end,
                ));
            }
        }
        return Some(NotebookEditorView {
            mode: EditorMode::NotebookNote,
            body,
            route: list.route.clone(),
            section: list.section.clone(),
            block_id: trailing,
            needs,
            spans,
            diagnostics,
            sub_bullets,
        });
    }
    if item_is_claimed_operator(item) {
        return None;
    }
    let claim = match classify_item(
        item,
        options.notebook_policy,
        options.global_route,
    ) {
        Ok(Some(claim)) => claim,
        Ok(None) => return None,
        Err(RouteScan::Unsupported {
            message,
            range,
            code,
            route,
        }) => {
            let owner = options
                .notebook_policy
                .and_then(|policy| policy.get(&route))
                .and_then(|info| info.owner.clone());
            let (code, message) = unsupported_with_optional_note(
                item,
                code,
                message,
                &route,
                owner.as_deref(),
            );
            return Some(notebook_editor_error(
                route,
                code,
                message,
                Some(range),
            ));
        }
        Err(RouteScan::Supported(_)) | Err(RouteScan::NotNotebookRoute) => {
            return None;
        }
    };
    let mut needs = Vec::new();
    let mut diagnostics = Vec::new();
    let mut spans = vec![Span {
        start: claim.marker.route_range.0,
        end: claim.marker.route_range.1,
        kind: SpanKind::NotebookRoute,
        project_note: None,
        title: Some(claim.info.title.clone()),
        key: claim.info.owner.clone(),
        section_action: None,
    }];
    let (mode, section, block_id) = match &claim.marker.form {
        NotebookForm::Note {
            section,
            exact_section,
            block_id,
            section_range,
            block_id_range,
        } => {
            if let Some(range) = section_range {
                if section.as_ref().is_none_or(|value| value.is_empty()) {
                    spans.push(Span {
                        start: range.0,
                        end: range.1,
                        kind: SpanKind::InteractivePlaceholder,
                        project_note: None,
                        title: None,
                        key: None,
                        section_action: None,
                    });
                } else if let (Some(selector), Some(policy)) =
                    (section.as_deref(), options.notebook_policy)
                {
                    spans.push(section_span(
                        policy,
                        item.source,
                        &claim.marker.route,
                        selector,
                        *exact_section,
                        range.0..range.1,
                    ));
                } else {
                    spans.push(Span {
                        start: range.0,
                        end: range.1,
                        kind: SpanKind::NotebookSection,
                        project_note: None,
                        title: section.clone(),
                        key: Some(
                            if *exact_section { "exact" } else { "slug" }
                                .to_string(),
                        ),
                        section_action: None,
                    });
                }
            }
            if let Some(section) = section.as_deref() {
                let offset =
                    section_range.map(|(start, _)| start).unwrap_or(item.start);
                let headings = options.notebook_policy.map_or_else(
                    || claim.info.sections.clone(),
                    |policy| {
                        staged_headings(
                            policy,
                            item.source,
                            &claim.marker.route,
                            offset,
                        )
                    },
                );
                if let Some(diagnostic) = section_resolution_diagnostic(
                    &claim.marker.route,
                    section,
                    *exact_section,
                    &headings,
                    *section_range,
                ) {
                    diagnostics.push(diagnostic);
                }
            }
            if let Some(range) = block_id_range {
                spans.push(Span {
                    start: range.0,
                    end: range.1,
                    kind: SpanKind::NotebookBlockId,
                    project_note: None,
                    title: None,
                    key: None,
                    section_action: None,
                });
            }
            if section_range.is_some()
                && section.as_ref().is_none_or(|section| section.is_empty())
            {
                needs.push(Need::Section);
            }
            if block_id_range.is_some() && block_id.is_none() {
                needs.push(Need::BlockId);
            }
            (EditorMode::NotebookNote, section.clone(), block_id.clone())
        }
        NotebookForm::SubNote {
            parent_block_id,
            block_id_range,
        } => {
            if let Some(range) = block_id_range {
                spans.push(Span {
                    start: range.0,
                    end: range.1,
                    kind: SpanKind::NotebookParentBlockId,
                    project_note: None,
                    title: None,
                    key: None,
                    section_action: None,
                });
            }
            if parent_block_id.is_empty() {
                needs.push(Need::BlockId);
            }
            (
                EditorMode::NotebookSubNote,
                None,
                Some(parent_block_id.clone()),
            )
        }
    };
    let sub_bullets = notebook_sub_bullets(item).unwrap_or_default();
    let clip = match refuse_notebook_item_markers(
        item,
        &claim.marker.route,
        owner_of(&claim.info),
        claim.marker.token_range,
    ) {
        Ok(clip) => clip,
        Err(error) => {
            return Some(notebook_editor_error(
                claim.marker.route,
                error.code.unwrap_or("notebook_marker_unsupported"),
                error.message,
                error.span.or(Some(claim.marker.token_range)),
            ));
        }
    };
    let mut body = claim.body;
    if clip.is_some() {
        body = strip_clip_tokens(&body);
    }
    if notebook_authored_block_has_checkbox(&body, &sub_bullets) {
        return Some(notebook_editor_error(
            claim.marker.route.clone(),
            "notebook_holds_no_tasks",
            notebook_task_refusal(&claim.marker.route, owner_of(&claim.info)),
            Some(claim.marker.token_range),
        ));
    }
    if body.trim().is_empty()
        && needs.is_empty()
        && sub_bullets.is_empty()
        && clip.is_none()
        && !matches!(claim.marker.form, NotebookForm::SubNote { .. })
    {
        needs.push(Need::NotebookNote);
    }
    let mode = if needs.contains(&Need::Section)
        || needs.contains(&Need::SectionNotes)
    {
        EditorMode::Incomplete
    } else {
        mode
    };
    spans.sort_by_key(|span| (span.start, span.end));
    Some(NotebookEditorView {
        mode,
        body,
        route: claim.marker.route,
        section,
        block_id,
        needs,
        spans,
        diagnostics,
        sub_bullets,
    })
}

fn notebook_editor_error(
    route: String,
    code: &'static str,
    message: String,
    range: Option<(usize, usize)>,
) -> NotebookEditorView {
    NotebookEditorView {
        mode: EditorMode::NotebookNote,
        body: String::new(),
        route,
        section: None,
        block_id: None,
        needs: Vec::new(),
        spans: Vec::new(),
        diagnostics: vec![Diagnostic {
            severity: Severity::Error,
            code,
            message,
            range,
            rewrite: None,
        }],
        sub_bullets: Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::native::capture_language::{
        model::{CaptureParseOptions, EditorParseOptions},
        parse_capture_text_with_options,
    };

    fn policy() -> NotebookRoutePolicy {
        let mut policy = NotebookRoutePolicy::default();
        policy.insert(
            "dev".into(),
            NotebookRouteInfo {
                title: "Developer tools".into(),
                summary: Some("Tools".into()),
                owner: Some("dev".into()),
                breadcrumb: vec!["dev".into(), "Developer tools".into()],
                sections: vec![NotebookSection {
                    title: "Git".into(),
                    line_index: 0,
                }],
            },
        );
        policy
    }

    #[test]
    fn candidates_are_deduplicated_and_include_global_routes() {
        assert_eq!(
            candidate_notebook_routes(
                "Try @dev_tools#git\n@dev_tools^x\n@@ideas"
            ),
            vec!["dev_tools", "ideas"]
        );
        assert_eq!(
            candidate_notebook_routes("Note {0} %t\n\n@dev hello\n"),
            vec!["dev"]
        );
        assert!(candidate_notebook_routes("Unrouted note text").is_empty());
    }

    #[test]
    fn classification_keeps_body_details_opaque_and_emits_spans() {
        let mut policy = policy();
        let info = policy.routes.remove("dev").unwrap();
        policy.insert("dev_tools".into(), info);
        let options = EditorParseOptions {
            url_routing: None,
            has_global_destination: false,
            record_policy: None,
            notebook_policy: Some(&policy),
            global_route: None,
        };
        let parsed = super::super::parse_for_editor_with(
            "@dev_tools#git Run `#task`\n  - keep this detail verbatim",
            &options,
        );
        assert_eq!(parsed.items[0].mode, EditorMode::NotebookNote);
        assert_eq!(parsed.items[0].route.as_deref(), Some("dev_tools"));
        assert_eq!(parsed.items[0].section.as_deref(), Some("git"));
        assert_eq!(parsed.items[0].body, "Run `#task`");
        assert_eq!(
            parsed.items[0]
                .sub_bullets
                .iter()
                .map(|bullet| bullet.body.as_str())
                .collect::<Vec<_>>(),
            vec!["keep this detail verbatim"]
        );
        assert!(parsed
            .spans
            .iter()
            .any(|span| span.kind == SpanKind::NotebookRoute));
        assert!(parsed
            .spans
            .iter()
            .any(|span| span.kind == SpanKind::NotebookSection));
    }

    #[test]
    fn unsupported_suffix_is_a_fail_closed_editor_diagnostic() {
        let mut policy = NotebookRoutePolicy::default();
        policy.insert(
            "dev".into(),
            NotebookRouteInfo {
                title: "Dev".into(),
                summary: None,
                owner: Some("dev".into()),
                breadcrumb: vec!["dev".into()],
                sections: Vec::new(),
            },
        );
        let options = EditorParseOptions {
            url_routing: None,
            has_global_destination: false,
            record_policy: None,
            notebook_policy: Some(&policy),
            global_route: None,
        };
        let parsed =
            super::super::parse_for_editor_with("X @dev^x#tail", &options);
        assert_eq!(parsed.items[0].mode, EditorMode::NotebookNote);
        assert_eq!(
            parsed.items[0].diagnostics[0].code,
            "notebook_marker_unsupported"
        );
        let _ = CaptureParseOptions::routing_off();
    }

    #[test]
    fn route_scan_recognizes_the_four_individual_forms() {
        for raw in [
            "X @dev",
            "@dev X",
            "X @dev#git",
            "X @dev^note",
            "X @dev+note",
        ] {
            let parsed =
                edge_marker(&super::super::split_capture_draft(raw).items[0])
                    .unwrap();
            assert!(matches!(parsed, RouteScan::Supported(_)), "{raw}");
        }
    }

    #[test]
    fn strict_parser_claims_individual_notebook_note_forms() {
        let policy = policy();
        let options = CaptureParseOptions {
            notebook_policy: Some(&policy),
            ..CaptureParseOptions::routing_off()
        };
        let examples = [
            (
                "A note @dev",
                CaptureKind::NotebookNote {
                    section: None,
                    exact_section: false,
                    block_id: None,
                },
                "A note",
            ),
            (
                "@dev#Ideas Keep `#task` as prose",
                CaptureKind::NotebookNote {
                    section: Some("Ideas".into()),
                    exact_section: false,
                    block_id: None,
                },
                "Keep `#task` as prose",
            ),
            (
                "An authored note @dev^idea-id",
                CaptureKind::NotebookNote {
                    section: None,
                    exact_section: false,
                    block_id: Some("idea-id".into()),
                },
                "An authored note",
            ),
            (
                "Child note @dev+parent-id",
                CaptureKind::NotebookSubNote {
                    parent_block_id: "parent-id".into(),
                },
                "Child note",
            ),
        ];
        for (raw, expected_kind, expected_body) in examples {
            let parsed =
                parse_capture_text_with_options(raw, None, None, &options)
                    .unwrap();
            assert_eq!(parsed.kind, expected_kind, "{raw}");
            assert_eq!(parsed.body, expected_body, "{raw}");
            assert_eq!(parsed.route.as_deref(), Some("dev"), "{raw}");
        }
    }

    #[test]
    fn editor_spans_use_utf8_byte_offsets_for_trailing_markers() {
        let policy = policy();
        let options = EditorParseOptions {
            notebook_policy: Some(&policy),
            ..EditorParseOptions::routing_off()
        };
        let raw = "é note @dev+parent-id";
        let parsed = super::super::parse_for_editor_with(raw, &options);
        let route = parsed
            .spans
            .iter()
            .find(|span| span.kind == SpanKind::NotebookRoute)
            .unwrap();
        let parent = parsed
            .spans
            .iter()
            .find(|span| span.kind == SpanKind::NotebookParentBlockId)
            .unwrap();
        assert_eq!(&raw[route.start..route.end], "@dev");
        assert_eq!(&raw[parent.start..parent.end], "parent-id");
        assert!(parent.start > route.end);
    }
}
