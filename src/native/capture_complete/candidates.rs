use std::{
    collections::{HashMap, HashSet},
    fs, io,
    path::{Path, PathBuf},
};

use super::model::{
    ActiveTaskCandidate, ActiveTaskPomodoroCandidate, BlockedTaskCandidate,
    CandidateContinuation, Candidates, CompleteError, DependencyCandidate,
    NotebookSectionCreate, PickerRelationEntry, PickerRelationship,
    PickerRelationshipEntry, ProjectTaskMoveCandidate, ProjectTaskMoveContext,
    RecordCreateCandidate, RelationCandidate, RelationLabelSuggestion,
    Replacement, RouteCandidate, SectionCandidate, SectionMenuRow,
    TaskCandidate, TaskCompleteCandidate, TaskGotoCandidate, TaskLinkCandidate,
    TaskParentCandidate,
};
use crate::native::{
    capture, capture_active_tasks, capture_block_ids,
    capture_completable_tasks, capture_dependency_tasks,
    capture_language::{
        DependencyEntry, DependencyTarget, DependencyTargetKind,
        EditorItemParse, ProjectTaskMoveDetail, RelationDirection,
        RelationEntry,
    },
    capture_link_tasks, capture_targets, capture_tasks, config, note_tasks,
    pomodoro, project_dependencies, task_complete, task_fields, task_move,
    task_relations, task_status_hooks,
};

pub(super) fn route_candidates(
    bob_dir: &Path,
    query: &str,
) -> Result<Candidates, CompleteError> {
    let report = capture_targets::scan_capture_targets_with_notebooks(bob_dir);
    if !report.issues.is_empty() {
        return Err(CompleteError::io(report.issue_summary()));
    }

    let ranked = rank_route_targets(report.targets, query);
    Ok(Candidates::Route(
        ranked
            .into_iter()
            .map(|(target, match_kind)| {
                record_aware_route_candidate(target, match_kind)
            })
            .collect(),
    ))
}

fn record_aware_route_candidate(
    target: capture_targets::CaptureTarget,
    match_kind: Option<&'static str>,
) -> RouteCandidate {
    let record = target.kind == capture_targets::CaptureTargetKind::Record;
    RouteCandidate {
        replacement: if record {
            format!("{} ", target.route)
        } else {
            target.route.clone()
        },
        route: target.route,
        label: if record {
            record_new_count_label(
                target.emoji.as_deref(),
                &target.label,
                target.count.unwrap_or(0),
            )
        } else {
            target.label
        },
        kind: target.kind.into(),
        status: target.status,
        match_kind,
        emoji: target.emoji,
        count: target.count,
        record_type: target.record_type,
        title: target.title,
        summary: target.summary,
        owner: target.owner,
        breadcrumb: target.breadcrumb,
        continuation: None,
    }
}

fn record_new_count_label(
    emoji: Option<&str>,
    label: &str,
    count: usize,
) -> String {
    let name = label.to_lowercase();
    match emoji {
        Some(emoji) if !emoji.is_empty() => {
            format!("{emoji} New {name} · {count}")
        }
        _ => format!("New {name} · {count}"),
    }
}

/// Patch record-route continuation cursors once the response replacement
/// start is known.
pub(super) fn annotate_record_route_continuations(
    candidates: &mut Candidates,
    replacement_start: usize,
) {
    let Candidates::Route(rows) = candidates else {
        return;
    };
    for row in rows {
        if row.kind != super::model::RouteCandidateKind::Record {
            continue;
        }
        let cursor = replacement_start + row.replacement.len();
        row.continuation = Some(CandidateContinuation {
            cursor,
            next_missing_field: true,
        });
    }
}

/// Route ranking with `project_name_aliases`: canonical prefix matches
/// first, alias matches (canonical replacement, `match_kind: "alias"`)
/// next, then the remaining substring matches, each group keeping the
/// scan's stable order. One candidate per route; an empty query keeps
/// every target with no `match_kind`.
fn rank_route_targets(
    targets: Vec<capture_targets::CaptureTarget>,
    query: &str,
) -> Vec<(capture_targets::CaptureTarget, Option<&'static str>)> {
    if query.is_empty() {
        return targets.into_iter().map(|target| (target, None)).collect();
    }
    let lowered = query.to_lowercase();
    let mut prefix = Vec::new();
    let mut alias = Vec::new();
    let mut substring = Vec::new();
    for target in targets {
        let route_lower = target.route.to_lowercase();
        if route_lower.starts_with(&lowered) {
            prefix.push((target, None));
            continue;
        }
        let alias_hit = target.project_name_aliases.iter().any(|name| {
            let name_lower = name.to_lowercase();
            name_lower.starts_with(&lowered) || name_lower.contains(&lowered)
        });
        if alias_hit {
            alias.push((target, Some("alias")));
            continue;
        }
        if route_lower.contains(&lowered) {
            substring.push((target, None));
        }
    }
    prefix.extend(alias);
    prefix.extend(substring);
    prefix
}

pub(super) fn section_candidates(
    bob_dir: &Path,
    route: &str,
    query: &str,
) -> Result<Candidates, CompleteError> {
    section_candidates_with_raw(bob_dir, route, query, None, None, None, None)
        .map(|(candidates, _)| candidates)
}

/// Where a notebook section menu files its rows: an existing notebook
/// (disk H2s plus staged draft sections) or a `%n` notebook that exists
/// only in this draft.
pub(super) struct NotebookMenuTarget {
    pub(super) title: String,
    pub(super) owner: Option<String>,
    pub(super) is_new: bool,
}

/// Resolve the menu target for a policy-visible notebook route: the
/// stored title and owner, or the prospective `%n` create's provisional
/// title and owner when the notebook exists only in this draft.
pub(super) fn notebook_menu_target(
    policy: &crate::native::capture_language::NotebookRoutePolicy,
    route: &str,
    at_offset: usize,
) -> Option<NotebookMenuTarget> {
    if let Some(info) = policy.get(route) {
        return Some(NotebookMenuTarget {
            title: info.title.clone(),
            owner: info.owner.clone(),
            is_new: false,
        });
    }
    if let crate::native::capture_language::notebook::NotebookRouteLookup::Hit(
        info,
    ) = policy.lookup(route, at_offset)
    {
        return Some(NotebookMenuTarget {
            title: info.title.clone(),
            owner: info.owner.clone(),
            is_new: true,
        });
    }
    None
}

/// The section-bullet menu for a notebook body: the staged sections at
/// the bullet offset with the target's title and owner for chrome and
/// consequences. The route is policy-visible by construction (the field
/// probe checked), so this never reads the vault and never warns.
pub(super) fn notebook_bullet_candidates(
    policy: &crate::native::capture_language::NotebookRoutePolicy,
    raw_text: &str,
    route: &str,
    query: &str,
    replacement: (usize, usize),
) -> Candidates {
    let staged = crate::native::capture_language::staged_notebook_sections(
        policy,
        raw_text,
        route,
        replacement.0,
    );
    let target = notebook_menu_target(policy, route, replacement.0);
    match target {
        Some(target) => notebook_section_rows(
            &staged,
            Some(&target),
            query,
            Some(raw_text),
            Some(replacement),
            route,
            NotebookSectionSpelling::Bullet,
        )
        .unwrap_or(Candidates::Section(Vec::new())),
        None => Candidates::Section(Vec::new()),
    }
}

pub(super) fn section_candidates_with_raw(
    bob_dir: &Path,
    route: &str,
    query: &str,
    raw_text: Option<&str>,
    replacement: Option<(usize, usize)>,
    notebook_policy: Option<
        &crate::native::capture_language::NotebookRoutePolicy,
    >,
    at_offset: Option<usize>,
) -> Result<(Candidates, Vec<String>), CompleteError> {
    let offset = at_offset.or_else(|| replacement.map(|(start, _)| start));
    if let Some(policy) = notebook_policy
        && policy.is_visible(route, offset.unwrap_or(0))
    {
        let staged = crate::native::capture_language::staged_notebook_sections(
            policy,
            raw_text.unwrap_or(""),
            route,
            offset.unwrap_or(0),
        );
        let target = notebook_menu_target(policy, route, offset.unwrap_or(0));
        return Ok((
            notebook_section_rows(
                &staged,
                target.as_ref(),
                query,
                raw_text,
                replacement,
                route,
                NotebookSectionSpelling::Glued,
            )?,
            Vec::new(),
        ));
    }
    let contents = match read_target(bob_dir, route) {
        Ok(contents) => contents,
        Err(error) => {
            // A notebook section menu never errors on a read failure:
            // it reports a bounded warning with no rows and no create
            // row, so the menu opens calm instead of failing the draft.
            return Ok((
                Candidates::Section(Vec::new()),
                vec![format!(
                    "notebook {route} unavailable: {}",
                    error.message
                )],
            ));
        }
    };
    if is_notebook_route_contents(&contents) {
        return Ok((
            notebook_section_candidates(
                &contents,
                query,
                raw_text,
                replacement,
                route,
                NotebookSectionSpelling::Glued,
            )?,
            Vec::new(),
        ));
    }
    let ranked = capture::section_completion_rows(&contents, query);

    Ok((
        Candidates::Section(
            ranked
                .into_iter()
                .map(|section| {
                    SectionMenuRow::Section(SectionCandidate::plain(
                        section.replacement,
                        section.title,
                        section.level,
                    ))
                })
                .collect(),
        ),
        Vec::new(),
    ))
}

pub(super) fn is_notebook_route_contents(contents: &str) -> bool {
    if let Some(frontmatter) =
        crate::native::projects::parse_frontmatter(contents)
    {
        return crate::native::projects::frontmatter_is_notebook(&frontmatter);
    }
    // Fall back to a byte scan for any accepted `type: [[notebook]]`
    // spelling when the typed parser declines.
    contents.contains("[[notebook]]")
}

/// Notebook `@nb#` sections from disk only: real H2s in document order
/// (never H1/H3 or fenced headings) with their note stats, plus one
/// additive create row for a nonempty query with no exact title or
/// whole-slug match. Without a menu target the rows keep the plain
/// `{replacement, title, level}` shape.
fn notebook_section_candidates(
    contents: &str,
    query: &str,
    raw_text: Option<&str>,
    replacement: Option<(usize, usize)>,
    route: &str,
    spelling: NotebookSectionSpelling,
) -> Result<Candidates, CompleteError> {
    use crate::native::capture_language::notebook::{
        NotebookSectionProvenance, StagedNotebookSection,
    };
    let headings =
        crate::native::capture_language::notebook::notebook_sections(contents);
    let stats =
        crate::native::capture_language::notebook::section_note_stats(contents);
    let staged: Vec<StagedNotebookSection> = headings
        .into_iter()
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
        .collect();
    notebook_section_rows(
        &staged,
        None,
        query,
        raw_text,
        replacement,
        route,
        spelling,
    )
}

#[derive(Clone, Copy)]
enum NotebookSectionSpelling {
    Bullet,
    Glued,
}

fn notebook_section_rows(
    staged: &[crate::native::capture_language::notebook::StagedNotebookSection],
    target: Option<&NotebookMenuTarget>,
    query: &str,
    raw_text: Option<&str>,
    replacement: Option<(usize, usize)>,
    route: &str,
    spelling: NotebookSectionSpelling,
) -> Result<Candidates, CompleteError> {
    use crate::native::capture_language::notebook::{
        resolve_notebook_section, NotebookSectionProvenance,
        NotebookSectionResolution,
    };
    let headings: Vec<_> = staged
        .iter()
        .map(
            crate::native::capture_language::notebook::StagedNotebookSection::as_heading,
        )
        .collect();
    let query_lower = query.to_lowercase();
    let query_slug = crate::native::capture_language::selector_slug(query);
    // Exact title/whole-slug match suppresses the create row, including
    // when that match is Ambiguous across duplicate headings.
    let has_exact = headings.iter().any(|heading| {
        heading.title.eq_ignore_ascii_case(query)
            || (!crate::native::capture_language::selector_slug(&heading.title)
                .is_empty()
                && crate::native::capture_language::selector_slug(
                    &heading.title,
                ) == query_slug)
    });
    // The create row stays hidden while the query is a strict prefix of
    // any staged title or slug: the menu already lists where that
    // prefix leads.
    let prefix_hidden = !query.is_empty()
        && staged.iter().any(|section| {
            let title_lower = section.title.to_lowercase();
            (title_lower.starts_with(&query_lower)
                && title_lower != query_lower)
                || {
                    let slug = crate::native::capture_language::selector_slug(
                        &section.title,
                    );
                    !query_slug.is_empty()
                        && !slug.is_empty()
                        && slug.starts_with(&query_slug)
                        && slug != query_slug
                }
        });
    // Ranked rows keep group order first (`notebook`, then `draft`) and,
    // under a typed query, exact, prefix, then substring within the
    // group; an empty query keeps document order throughout. The sort is
    // stable so document order always breaks ties.
    let mut ranked: Vec<(u8, usize)> = Vec::new();
    for (index, section) in staged.iter().enumerate() {
        let tier = section_rank(&section.title, &query_lower, &query_slug);
        let Some(tier) = tier else {
            continue;
        };
        ranked.push((tier, index));
    }
    if !query.is_empty() {
        ranked.sort_by_key(|(tier, _)| *tier);
    }
    // Notebook rows keep document order across groups (disk H2s precede
    // draft sections in the staged view), so a stable group partition
    // preserves the ranked order inside each group.
    let mut notebook_rows = Vec::new();
    let mut draft_rows = Vec::new();
    for (_, index) in ranked {
        let section = &staged[index];
        let (replacement, refused) = notebook_section_row(
            &section.as_heading(),
            &headings,
            route,
            spelling,
        );
        let mut row =
            SectionCandidate::plain(replacement, section.title.clone(), 2);
        if let Some(target) = target {
            row.kind = Some("notebook_section");
            row.group = Some(match section.provenance {
                NotebookSectionProvenance::Notebook => "notebook",
                NotebookSectionProvenance::Draft => "draft",
            });
            row.line = section.line;
            row.note_count = (section.provenance
                == NotebookSectionProvenance::Notebook)
                .then_some(section.note_count);
            row.detail = Some(section_row_detail(section));
            row.consequence =
                Some(section_row_consequence(section, &section.title, target));
        }
        if refused.is_some() {
            row.refused = refused;
        }
        match section.provenance {
            NotebookSectionProvenance::Notebook => {
                notebook_rows.push(row);
            }
            NotebookSectionProvenance::Draft => draft_rows.push(row),
        }
    }
    let mut rows: Vec<SectionMenuRow> = notebook_rows
        .into_iter()
        .map(SectionMenuRow::Section)
        .chain(draft_rows.into_iter().map(SectionMenuRow::Section))
        .collect();
    // Additive create row for a nonempty query with no exact match.
    if !query.is_empty() && !has_exact && !prefix_hidden {
        let exact = raw_text
            .zip(replacement)
            .and_then(|(text, (start, end))| text.get(start..end))
            .is_some_and(|typed| {
                let trimmed = typed.trim();
                trimmed.starts_with('"')
                    || trimmed.starts_with('\u{201c}')
                    || query.contains(char::is_whitespace)
            });
        // Preserve authored quoted titles: an exact (quoted) query creates
        // verbatim; otherwise resolve the bare query.
        let resolution = resolve_notebook_section(query, exact, &headings);
        let create = match resolution {
            NotebookSectionResolution::Existing { title, .. } => {
                // Should not happen when `has_exact` is false, but keep
                // the row additive rather than dropping the query.
                Some((title.clone(), query.to_string(), None))
            }
            NotebookSectionResolution::Create { title } => {
                if exact {
                    // Preserve the authored quoted spelling.
                    let quoted = raw_text
                        .zip(replacement)
                        .and_then(|(text, (start, end))| text.get(start..end))
                        .map(str::trim)
                        .filter(|typed| {
                            (typed.starts_with('"')
                                && typed.ends_with('"')
                                && typed.len() >= 2)
                                || (typed.starts_with('\u{201c}')
                                    && typed.ends_with('\u{201d}'))
                        })
                        .map(str::to_string)
                        .unwrap_or_else(|| {
                            format!("\"{}\"", quote_escape(&title))
                        });
                    Some((title, quoted, None))
                } else {
                    // Choose a spelling that creates exactly the title
                    // advertised by this row when the resolver sees it.
                    let selector = notebook_section_create_spelling(
                        query, &title, &headings,
                    );
                    Some((title, selector, None))
                }
            }
            NotebookSectionResolution::NearMiss { slug, quoted_title } => {
                // An unquoted slug would be a near miss: offer the quoted
                // selector so the create is explicit, and point at the
                // existing heading the query almost spelled.
                let selector = format!("\"{}\"", quote_escape(&quoted_title));
                let meant = staged
                    .iter()
                    .find(|section| {
                        crate::native::capture_language::selector_slug(
                            &section.title,
                        ) == slug
                    })
                    .map(|section| section.title.clone())
                    .unwrap_or_else(|| quoted_title.clone());
                Some((quoted_title, selector, Some(meant)))
            }
            NotebookSectionResolution::Ambiguous { .. } => None,
        };
        if let Some((title, replacement, meant)) = create {
            let label = format!("New section \u{201c}{title}\u{201d}");
            let mut row = NotebookSectionCreate {
                kind: "notebook_section_create",
                replacement,
                label,
                title: title.clone(),
                group: None,
                detail: None,
                consequence: None,
                creates: false,
                cursor_after: None,
            };
            if let Some(target) = target {
                row.group = Some("new");
                row.creates = true;
                if let Some(meant) = meant {
                    row.detail = Some(format!("did you mean {meant}?"));
                }
                row.consequence = Some(if target.is_new {
                    format!("Seeds ## {title} in the new notebook")
                } else {
                    format!("Creates ## {title} at the end of {}", target.title)
                });
            }
            rows.push(SectionMenuRow::NotebookCreate(row));
        }
    }
    Ok(Candidates::Section(rows))
}

/// Rank one staged title against a typed query: exact title or slug (0),
/// prefix (1), then substring (2). `None` filters the row out; an empty
/// query keeps every row in document order.
fn section_rank(
    title: &str,
    query_lower: &str,
    query_slug: &str,
) -> Option<u8> {
    if query_lower.is_empty() {
        return Some(0);
    }
    let title_lower = title.to_lowercase();
    let slug = crate::native::capture_language::selector_slug(title);
    if title_lower == *query_lower
        || (!query_slug.is_empty() && !slug.is_empty() && slug == *query_slug)
    {
        return Some(0);
    }
    if title_lower.starts_with(query_lower)
        || (!query_slug.is_empty()
            && !slug.is_empty()
            && slug.starts_with(query_slug))
    {
        return Some(1);
    }
    if title_lower.contains(query_lower)
        || (!query_slug.is_empty()
            && !slug.is_empty()
            && slug.contains(query_slug))
    {
        return Some(2);
    }
    None
}

/// The picker detail for one staged row: note counts for notebook rows,
/// `new` for draft rows.
fn section_row_detail(
    section: &crate::native::capture_language::notebook::StagedNotebookSection,
) -> String {
    use crate::native::capture_language::notebook::NotebookSectionProvenance;
    match section.provenance {
        NotebookSectionProvenance::Draft => "new".to_string(),
        NotebookSectionProvenance::Notebook => match section.note_count {
            0 => "empty".to_string(),
            1 => "1 note".to_string(),
            count => format!("{count} notes"),
        },
    }
}

/// Where accepting one staged row files the note, in Bob's words.
fn section_row_consequence(
    section: &crate::native::capture_language::notebook::StagedNotebookSection,
    title: &str,
    target: &NotebookMenuTarget,
) -> String {
    use crate::native::capture_language::notebook::NotebookSectionProvenance;
    match section.provenance {
        NotebookSectionProvenance::Draft => {
            if target.is_new {
                format!("Seeds ## {title} in the new notebook")
            } else {
                format!("Adds to ## {title}, new in this draft")
            }
        }
        NotebookSectionProvenance::Notebook => {
            match section.last_note.as_deref() {
                Some(note) if section.note_count > 0 && !note.is_empty() => {
                    format!(
                        "Adds after \u{201c}{}\u{201d} in ## {title}",
                        ellipsize_note(note)
                    )
                }
                _ => format!("First note in ## {title}"),
            }
        }
    }
}

/// Ellipsize a note snippet for a consequence line: whole when short,
/// otherwise the first 26 characters plus `…`.
fn ellipsize_note(text: &str) -> String {
    const KEEP: usize = 26;
    if text.chars().count() <= KEEP {
        text.to_string()
    } else {
        let kept: String = text.chars().take(KEEP).collect();
        format!("{kept}\u{2026}")
    }
}

/// Open the nested `  - ` bullet right after an accepted section row
/// and record each row's `cursor_after` past its replacement, so the
/// caret lands where the section's first note goes. Only the
/// `notebook_section_bullet` context calls this; glued rows keep no
/// continuation and no `cursor_after`.
pub(super) fn append_section_continuation(
    candidates: &mut Candidates,
    continuation: bool,
    replacement_start: usize,
) {
    let Candidates::Section(rows) = candidates else {
        return;
    };
    for row in rows {
        match row {
            SectionMenuRow::Section(section) => {
                if continuation {
                    section.replacement.push_str("\n  - ");
                }
                section.cursor_after =
                    Some(replacement_start + section.replacement.len());
            }
            SectionMenuRow::NotebookCreate(create) => {
                if continuation {
                    create.replacement.push_str("\n  - ");
                }
                create.cursor_after =
                    Some(replacement_start + create.replacement.len());
            }
            SectionMenuRow::Create(_) => {}
        }
    }
}

fn quote_escape(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}

fn notebook_section_row(
    heading: &crate::native::capture_language::notebook::NotebookSection,
    headings: &[crate::native::capture_language::notebook::NotebookSection],
    route: &str,
    spelling: NotebookSectionSpelling,
) -> (String, Option<String>) {
    if let Some(replacement) = notebook_section_replacement(
        &heading.title,
        heading.line_index,
        headings,
        spelling,
    ) {
        return (replacement, None);
    }
    (
        format!("\"{}\"", quote_escape(&heading.title)),
        notebook_section_refused_reason(heading, headings, route),
    )
}

fn notebook_section_refused_reason(
    heading: &crate::native::capture_language::notebook::NotebookSection,
    headings: &[crate::native::capture_language::notebook::NotebookSection],
    route: &str,
) -> Option<String> {
    use crate::native::capture_language::notebook::{
        notebook_section_ambiguous_message, resolve_notebook_section,
        NotebookSectionResolution,
    };
    for exact in [true, false] {
        if let NotebookSectionResolution::Ambiguous { matches } =
            resolve_notebook_section(&heading.title, exact, headings)
        {
            return Some(notebook_section_ambiguous_message(route, &matches));
        }
    }
    let slug = crate::native::capture_language::selector_slug(&heading.title);
    if !slug.is_empty()
        && let NotebookSectionResolution::Ambiguous { matches } =
            resolve_notebook_section(&slug, false, headings)
    {
        return Some(notebook_section_ambiguous_message(route, &matches));
    }
    None
}

fn notebook_section_replacement(
    title: &str,
    line_index: usize,
    headings: &[crate::native::capture_language::notebook::NotebookSection],
    spelling: NotebookSectionSpelling,
) -> Option<String> {
    use crate::native::capture_language::notebook::{
        resolve_notebook_section, NotebookSectionResolution,
    };
    fn unquoted_spelling(value: &str) -> bool {
        !value.is_empty()
            && !value.chars().any(char::is_whitespace)
            && !value.starts_with('"')
            && !value.starts_with('\u{201c}')
    }
    // Bare title when the relevant scanner accepts it whole and it
    // round-trips to this heading.
    let bare_title = match spelling {
        NotebookSectionSpelling::Bullet => {
            crate::native::capture_language::section_bullet::bare_selector_is_literal(title)
        }
        NotebookSectionSpelling::Glued => unquoted_spelling(title),
    };
    if bare_title
        && let NotebookSectionResolution::Existing {
            line_index: found, ..
        } = resolve_notebook_section(title, false, headings)
        && found == line_index
    {
        return Some(title.to_string());
    }
    if matches!(spelling, NotebookSectionSpelling::Glued) {
        let slug = crate::native::capture_language::selector_slug(title);
        if !slug.is_empty()
            && unquoted_spelling(&slug)
            && let NotebookSectionResolution::Existing {
                line_index: found, ..
            } = resolve_notebook_section(&slug, false, headings)
            && found == line_index
        {
            return Some(slug);
        }
    }
    // Quoted exact title as the explicit spelling.
    if let NotebookSectionResolution::Existing {
        line_index: found, ..
    } = resolve_notebook_section(title, true, headings)
        && found == line_index
    {
        return Some(format!("\"{}\"", quote_escape(title)));
    }
    None
}

fn notebook_section_create_spelling(
    query: &str,
    title: &str,
    headings: &[crate::native::capture_language::notebook::NotebookSection],
) -> String {
    use crate::native::capture_language::notebook::{
        resolve_notebook_section, NotebookSectionResolution,
    };
    let creates_title = |selector: &str| {
        matches!(
            resolve_notebook_section(selector, false, headings),
            NotebookSectionResolution::Create { title: created }
                if created == title
        )
    };
    if creates_title(query) {
        return query.to_string();
    }
    let slug = crate::native::capture_language::selector_slug(title);
    if !slug.is_empty() && creates_title(&slug) {
        return slug;
    }
    format!("\"{}\"", quote_escape(title))
}

/// For a configured record collection's local `@dir#` token, expand the
/// response range to the whole token and make every row a complete
/// replacement. Prepend the create row only on an empty query.
pub(super) fn mix_record_section_create(
    bob_dir: &Path,
    raw_text: &str,
    field: &crate::native::capture_language::CompletionField,
    candidates: &mut Candidates,
) -> Option<Replacement> {
    let Candidates::Section(rows) = candidates else {
        return None;
    };
    let dir = field.route.as_deref()?.trim_end_matches('/');
    let config_path = config::config_path();
    let resolution =
        crate::native::records::context::RecordContext::resolve_route_head(
            bob_dir,
            &config_path,
            dir,
            None,
        )
        .ok()?;
    let crate::native::records::context::RecordKindResolution::Valid {
        kind,
        ..
    } = resolution
    else {
        return None;
    };
    let record_type = kind.record_type();
    let token_start =
        record_section_token_start(raw_text, field.replacement.0)?;
    if token_start > 0
        && raw_text.as_bytes().get(token_start - 1) == Some(&b'@')
    {
        return None;
    }
    let token_prefix = raw_text.get(token_start..field.replacement.0)?;
    if !token_prefix.starts_with('@') || !token_prefix.ends_with('#') {
        return None;
    }
    for row in rows.iter_mut() {
        if let SectionMenuRow::Section(section) = row {
            section.replacement =
                format!("{token_prefix}{}", section.replacement);
        }
    }
    if field.query.is_empty() {
        let mut catalog = crate::native::records::RecordCatalog::new(bob_dir);
        let count = catalog.folder(&record_type.dir).notes().len();
        let route = format!("{}/", record_type.dir);
        let replacement = format!("@{route} ");
        let cursor_after = token_start + replacement.len();
        let label = match record_type.emoji.as_deref() {
            Some(emoji) if !emoji.is_empty() => format!(
                "{} New {} · {route}",
                emoji,
                record_type.label.to_lowercase()
            ),
            _ => format!("New {} · {route}", record_type.label.to_lowercase()),
        };
        rows.insert(
            0,
            SectionMenuRow::Create(RecordCreateCandidate {
                kind: "record_create",
                replacement,
                label,
                route,
                record_type: record_type.id.clone(),
                emoji: record_type.emoji.clone(),
                count: Some(count),
                cursor_after,
                continuation: Some(CandidateContinuation {
                    cursor: cursor_after,
                    next_missing_field: true,
                }),
            }),
        );
    }
    Some(Replacement {
        start: token_start,
        end: field.replacement.1,
    })
}

fn record_section_token_start(
    raw_text: &str,
    replacement_start: usize,
) -> Option<usize> {
    let before = raw_text.get(..replacement_start)?;
    let hash = before.rfind('#')?;
    let at = before[..hash].rfind('@')?;
    if before[at + 1..hash].chars().any(char::is_whitespace) {
        return None;
    }
    Some(at)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum TaskSearch {
    BlockIdOnly,
    MultiField,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum MatchKind {
    Prefix,
    Substring,
}

pub(super) fn task_candidates(
    bob_dir: &Path,
    route: &str,
    query: &str,
    include_missing: bool,
    search: TaskSearch,
) -> Result<Candidates, CompleteError> {
    let contents = read_target(bob_dir, route)?;
    let settings = note_tasks::read_settings(bob_dir);
    let scan = note_tasks::scan(&contents, &settings);
    let used_ids: Vec<String> =
        capture_block_ids::collect_used(&contents, &settings)
            .into_iter()
            .map(|entry| entry.id)
            .collect();
    let ranked =
        rank_open_tasks(scan.open_tasks(), query, include_missing, search);

    Ok(Candidates::Task(
        ranked
            .into_iter()
            .map(|task| {
                let requires_block_id = task.block_id.is_none();
                TaskCandidate {
                    replacement: task.block_id.clone().unwrap_or_default(),
                    task_ref: task.task_ref(),
                    block_id: task.block_id.clone(),
                    route: route.to_string(),
                    requires_block_id,
                    block_id_suggestions: if requires_block_id {
                        let suggestions = capture_block_ids::suggest_ids(
                            &task.description,
                            '^',
                            &used_ids,
                        );
                        (!suggestions.is_empty()).then_some(suggestions)
                    } else {
                        None
                    },
                    status_symbol: task.status_symbol,
                    status_name: task.status_name.clone(),
                    status_type: capture_tasks::status_type_label(
                        task.status_type,
                    ),
                    text: task.description.clone(),
                    disabled_reason: None,
                    consequence: None,
                    detail: None,
                    section: task.section.clone(),
                    depth: capture_tasks::indentation_depth(&task.indentation),
                    child_count: task.child_count,
                    line: task.line_index + 1,
                    pomodoro: None,
                    task_kind: task.task_kind,
                    item_kind: None,
                }
            })
            .collect(),
    ))
}

/// Notebook note-bullet rows for note-scoped `@nb+`: eligible note
/// bullets at any depth, ID'd first then ID-less in stable document
/// order. Duplicate target IDs become disabled refusals, never selectable
/// destinations.
pub(super) fn notebook_note_candidates(
    bob_dir: &Path,
    route: &str,
    query: &str,
) -> Result<Candidates, CompleteError> {
    use crate::native::capture::notebook_bullets;
    let contents = read_target(bob_dir, route)?;
    let bullets = notebook_bullets::scan_notebook_bullets(&contents);
    let duplicates = notebook_bullets::duplicate_ids(&contents);
    let settings = note_tasks::read_settings(bob_dir);
    let used_ids: Vec<String> =
        capture_block_ids::collect_used(&contents, &settings)
            .into_iter()
            .map(|entry| entry.id)
            .collect();
    let query_lower = query.to_lowercase();
    // Filter by the existing text/ID query behavior: substring on text
    // or ID, case-insensitive.
    let mut identified = Vec::new();
    let mut unidentified = Vec::new();
    for bullet in &bullets {
        if !query.is_empty() {
            let text_hit = bullet.text.to_lowercase().contains(&query_lower);
            let id_hit = bullet
                .block_id
                .as_deref()
                .is_some_and(|id| id.to_lowercase().contains(&query_lower));
            if !text_hit && !id_hit {
                continue;
            }
        }
        if bullet.block_id.is_some() {
            identified.push(bullet);
        } else {
            unidentified.push(bullet);
        }
    }
    let mut rows = Vec::new();
    for group in [identified, unidentified] {
        for bullet in group {
            let duplicate = bullet
                .block_id
                .as_deref()
                .is_some_and(|id| duplicates.contains(id));
            let requires_block_id = bullet.block_id.is_none();
            let (replacement, disabled_reason) = if duplicate {
                (
                    String::new(),
                    Some(format!(
                        "block ID ^{} appears more than once in {route}.md",
                        bullet.block_id.as_deref().unwrap_or_default()
                    )),
                )
            } else if requires_block_id {
                (String::new(), None)
            } else {
                (bullet.block_id.clone().unwrap_or_default(), None)
            };
            let block_id_suggestions = if requires_block_id && !duplicate {
                let suggestions = capture_block_ids::suggest_ids(
                    &bullet.text,
                    '^',
                    &used_ids,
                );
                (!suggestions.is_empty()).then_some(suggestions)
            } else {
                None
            };
            rows.push(TaskCandidate {
                replacement,
                task_ref: bullet.task_ref.clone(),
                block_id: bullet.block_id.clone(),
                route: route.to_string(),
                requires_block_id,
                block_id_suggestions,
                status_symbol: '-',
                status_name: "Note".to_string(),
                status_type: "NOTE",
                text: bullet.text.clone(),
                disabled_reason,
                consequence: None,
                detail: None,
                section: bullet.section.clone(),
                depth: bullet.depth,
                child_count: bullet.child_count,
                line: bullet.line,
                pomodoro: None,
                task_kind: None,
                item_kind: Some("note"),
            });
        }
    }
    Ok(Candidates::Task(rows))
}

pub(super) struct ProjectTaskMoveRows {
    pub(super) context: ProjectTaskMoveContext,
    pub(super) rows: Vec<ProjectTaskMoveCandidate>,
    pub(super) warnings: Vec<String>,
}

/// Build note-local move candidates from the shared task-move eligibility
/// API. The only ledger read is today's daily note, for the same Pomodoro
/// chips used by scoped task completion.
pub(super) fn project_task_move_candidates(
    bob_dir: &Path,
    detail: &ProjectTaskMoveDetail,
    query: &str,
) -> ProjectTaskMoveRows {
    let mut warnings = Vec::new();
    let Some(route) = detail.parent_route.as_deref() else {
        warnings.push("choose the parent note first".to_string());
        return ProjectTaskMoveRows {
            context: project_task_move_context(detail, 0),
            rows: Vec::new(),
            warnings,
        };
    };
    let parent = detail
        .parent
        .clone()
        .unwrap_or_else(|| capture::route_label(route));
    let target = bob_dir.join(&parent);
    let contents = match fs::read_to_string(&target) {
        Ok(contents) => contents,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            warnings.push(format!(
                "{parent} does not exist (run 'bob capture-targets' to list routable notes)"
            ));
            return ProjectTaskMoveRows {
                context: project_task_move_context(detail, 0),
                rows: Vec::new(),
                warnings,
            };
        }
        Err(error) => {
            warnings.push(capture_active_tasks::bounded_warning(format!(
                "failed to read {parent}: {error}"
            )));
            return ProjectTaskMoveRows {
                context: project_task_move_context(detail, 0),
                rows: Vec::new(),
                warnings,
            };
        }
    };

    let settings = note_tasks::read_settings(bob_dir);
    let scan = note_tasks::scan(&contents, &settings);
    let roots = task_move::movable_roots(Path::new(&parent), &contents);
    let used_ids = capture_block_ids::collect_used(&contents, &settings);
    let used: HashSet<&str> =
        used_ids.iter().map(|entry| entry.id.as_str()).collect();
    let ledger = capture_active_tasks::read_ledger(
        &pomodoro::day_file_for(bob_dir),
        &mut warnings,
    );
    let lines: Vec<&str> = contents.lines().collect();
    let mut id_counts = HashMap::<String, usize>::new();
    for task in scan.tasks() {
        if let Some(id) = &task.block_id {
            *id_counts.entry(id.clone()).or_default() += 1;
        }
    }

    let mut ranked = Vec::new();
    let mut open_count = 0;
    for (document_order, root) in roots.iter().enumerate() {
        if root
            .block_id
            .as_deref()
            .is_some_and(|id| id.eq_ignore_ascii_case("prj"))
        {
            continue;
        }
        let Some(task) = scan.task_at(root.line.saturating_sub(1)) else {
            continue;
        };
        if !task.status_type.is_open() {
            continue;
        }
        open_count += 1;
        let block_id = root.block_id.clone();
        let raw_line = lines
            .get(root.line.saturating_sub(1))
            .copied()
            .unwrap_or_default();
        let scheduled = task_fields::inline_fields(raw_line, "scheduled")
            .into_iter()
            .find_map(|field| {
                task_fields::parse_strict_calendar_date(&field.value)
                    .map(|_| field.value)
            });
        let hidden =
            crate::native::capture_dependency_tasks::has_hide_tag(raw_line);
        let suggestions = if block_id.is_none() {
            capture_block_ids::suggest_ids_with_used(
                &task.description,
                '^',
                &used,
            )
        } else {
            Vec::new()
        };
        let duplicate_count = block_id
            .as_ref()
            .and_then(|id| id_counts.get(id))
            .copied()
            .unwrap_or_default();
        let moved = block_id.as_deref().and_then(|id| {
            detail.existing_moves.iter().find(|occurrence| {
                occurrence.block_id == id
                    && !(occurrence.item_index == detail.item_index
                        && occurrence.line == detail.current_line)
            })
        });
        let source_reason = root.disabled_reason.clone();
        let duplicate_reason = (duplicate_count > 1).then(|| {
            format!(
                "{duplicate_count} tasks end ^{} in {parent}",
                block_id.as_deref().unwrap_or_default()
            )
        });
        let moved_reason = moved.map(|occurrence| {
            if occurrence.item_index == detail.item_index {
                format!("already moving on line {}", occurrence.line)
            } else if occurrence.item_index < detail.item_index {
                format!("already moved in item {}", occurrence.item_index + 1)
            } else {
                format!("already moving in item {}", occurrence.item_index + 1)
            }
        });
        let project_reason = detail.project_start.as_ref().and_then(|date| {
            (root.status != '?').then(|| {
                format!(
                    "project starts {date} — only Blocked tasks can move in"
                )
            })
        });
        let disabled_reason = duplicate_reason
            .or(moved_reason)
            .or(project_reason)
            .or(source_reason);
        let replacement = if disabled_reason.is_none() {
            block_id.clone().unwrap_or_default()
        } else {
            String::new()
        };
        let pomodoro = block_id.as_ref().and_then(|id| {
            ledger
                .owners
                .get(&(route.to_string(), id.clone()))
                .map(|entry| ActiveTaskPomodoroCandidate {
                    line: entry.line,
                    name: entry.name.clone(),
                    time_range: entry.time_range.clone(),
                    is_current: entry.is_current,
                })
        });
        let consequence = project_move_consequence(
            detail,
            root.status,
            block_id.is_none(),
            suggestions.first(),
            pomodoro.as_ref(),
        );
        let row = ProjectTaskMoveCandidate {
            replacement,
            sigil: detail.sigil,
            task_ref: task.task_ref(),
            block_id,
            route: route.to_string(),
            requires_block_id: root.block_id.is_none(),
            block_id_suggestions: suggestions,
            status_symbol: root.status,
            status_name: task.status_name.clone(),
            status_type: capture_tasks::status_type_label(task.status_type),
            text: task.description.clone(),
            disabled_reason,
            section: task.section.clone(),
            depth: capture_tasks::indentation_depth(&task.indentation),
            child_count: task.child_count,
            line: root.line,
            pomodoro,
            scheduled,
            hidden,
            consequence,
        };
        let rank = project_move_match_rank(&row, query);
        if let Some(rank) = rank {
            ranked.push((rank, row.requires_block_id, document_order, row));
        }
    }
    if query.is_empty() {
        ranked.sort_by_key(|(_, _, document_order, _)| *document_order);
    } else {
        ranked.sort_by_key(|(rank, requires_id, document_order, _)| {
            (*rank, *requires_id, *document_order)
        });
    }
    let rows = ranked.into_iter().map(|(_, _, _, row)| row).collect();
    let context = project_task_move_context(detail, open_count);
    ProjectTaskMoveRows {
        context,
        rows,
        warnings,
    }
}

fn project_task_move_context(
    detail: &ProjectTaskMoveDetail,
    open_count: usize,
) -> ProjectTaskMoveContext {
    ProjectTaskMoveContext {
        parent: detail.parent.clone(),
        parent_route: detail.parent_route.clone(),
        destination: detail.destination.clone(),
        sigil: detail.sigil,
        open_count,
        pomodoro_name: detail.pomodoro_name.clone(),
    }
}

fn project_move_consequence(
    detail: &ProjectTaskMoveDetail,
    status: char,
    requires_block_id: bool,
    suggestion: Option<&String>,
    pomodoro: Option<&ActiveTaskPomodoroCandidate>,
) -> String {
    use crate::native::capture::move_consequence::{
        self, LinkGuard, PredictionInput,
    };

    let prediction = move_consequence::predict(&PredictionInput {
        sigil: detail.sigil,
        status,
        destination: detail
            .destination
            .clone()
            .unwrap_or_else(|| "the new note".to_string()),
        pomodoro_name: detail.pomodoro_name.clone(),
        today_link: pomodoro.and_then(|item| item.name.clone()),
        own_scheduled: None,
        project_starts: detail.project_start.clone(),
        guard: if status == '?' {
            LinkGuard::StatusOnly
        } else {
            LinkGuard::Link
        },
        id_to_add: None,
    });
    if !requires_block_id {
        return prediction.consequence;
    }
    let mut text = match suggestion {
        Some(suggestion) => format!("Needs an ID first · ⊕ ^{suggestion}"),
        None => "Needs an ID first".to_string(),
    };
    text.push_str(" · ");
    text.push_str(&prediction.consequence);
    text
}

fn project_move_match_rank(
    row: &ProjectTaskMoveCandidate,
    query: &str,
) -> Option<u8> {
    if query.is_empty() {
        return Some(0);
    }
    let query = query.to_lowercase();
    if row
        .block_id
        .as_deref()
        .is_some_and(|id| id.to_lowercase().starts_with(&query))
    {
        return Some(0);
    }
    let fields = [
        row.block_id.as_deref().unwrap_or_default(),
        row.text.as_str(),
        row.section.as_deref().unwrap_or_default(),
        row.status_name.as_str(),
    ];
    if fields.iter().any(|field| {
        field
            .split(|character: char| !character.is_alphanumeric())
            .any(|word| word.to_lowercase().starts_with(&query))
    }) {
        return Some(1);
    }
    if fields
        .iter()
        .any(|field| field.to_lowercase().contains(&query))
    {
        return Some(2);
    }
    if fields.iter().any(|field| {
        let lowered = field.to_lowercase();
        let mut chars = lowered.chars();
        query
            .chars()
            .all(|needle| chars.any(|candidate| candidate == needle))
    }) {
        return Some(3);
    }
    None
}

/// Link-only candidates for `pomodoro_block_id` with `link` intent:
/// identified open tasks the link path accepts (Ready, Blocked, Next, In
/// Progress via the same predicate the link resolver uses), annotated with
/// the queued Pomodoro from today's ledger.
pub(super) fn link_candidates(
    bob_dir: &Path,
    route: &str,
    query: &str,
) -> Result<(Candidates, Vec<String>), CompleteError> {
    let contents = read_target(bob_dir, route)?;
    let settings = note_tasks::read_settings(bob_dir);
    let scan = note_tasks::scan(&contents, &settings);
    let linkable = scan.open_tasks().filter(|task| {
        task.block_id.is_some()
            && capture_link_tasks::is_linkable_status(task.status_symbol)
    });
    let ranked =
        rank_task_group(linkable.collect(), query, TaskSearch::BlockIdOnly);
    let day_file = pomodoro::day_file_for(bob_dir);
    let mut warnings = Vec::new();
    let ledger = capture_active_tasks::read_ledger(&day_file, &mut warnings);
    let candidates = ranked
        .into_iter()
        .map(|task| {
            let block_id =
                task.block_id.clone().expect("filtered to identified tasks");
            let pomodoro = ledger
                .owners
                .get(&(route.to_string(), block_id.clone()))
                .map(|entry| ActiveTaskPomodoroCandidate {
                    line: entry.line,
                    name: entry.name.clone(),
                    time_range: entry.time_range.clone(),
                    is_current: entry.is_current,
                });
            TaskCandidate {
                replacement: block_id.clone(),
                task_ref: task.task_ref(),
                block_id: Some(block_id),
                route: route.to_string(),
                requires_block_id: false,
                block_id_suggestions: None,
                status_symbol: task.status_symbol,
                status_name: task.status_name.clone(),
                status_type: capture_tasks::status_type_label(task.status_type),
                text: task.description.clone(),
                disabled_reason: None,
                consequence: None,
                detail: None,
                section: task.section.clone(),
                depth: capture_tasks::indentation_depth(&task.indentation),
                child_count: task.child_count,
                line: task.line_index + 1,
                pomodoro,
                task_kind: task.task_kind,
                item_kind: None,
            }
        })
        .collect();
    Ok((Candidates::Task(candidates), warnings))
}

pub(super) fn rank_open_tasks<'a>(
    tasks: impl Iterator<Item = &'a note_tasks::NoteTask>,
    query: &str,
    include_missing: bool,
    search: TaskSearch,
) -> Vec<&'a note_tasks::NoteTask> {
    let mut identified = Vec::new();
    let mut unidentified = Vec::new();
    for task in tasks {
        if task.block_id.is_some() {
            identified.push(task);
        } else if include_missing {
            unidentified.push(task);
        }
    }

    let mut ranked = rank_task_group(identified, query, search);
    ranked.extend(rank_task_group(unidentified, query, search));
    ranked
}

pub(super) fn rank_task_group<'a>(
    tasks: Vec<&'a note_tasks::NoteTask>,
    query: &str,
    search: TaskSearch,
) -> Vec<&'a note_tasks::NoteTask> {
    if query.is_empty() {
        return tasks;
    }

    let query = query.to_lowercase();
    let mut prefix_matches = Vec::new();
    let mut substring_matches = Vec::new();
    for task in tasks {
        match task_match_kind(task, &query, search) {
            Some(MatchKind::Prefix) => prefix_matches.push(task),
            Some(MatchKind::Substring) => substring_matches.push(task),
            None => {}
        }
    }
    prefix_matches.extend(substring_matches);
    prefix_matches
}

pub(super) fn task_match_kind(
    task: &note_tasks::NoteTask,
    query: &str,
    search: TaskSearch,
) -> Option<MatchKind> {
    let mut prefix = false;
    let mut substring = false;
    for field in task_search_fields(task, search) {
        let value = field.to_lowercase();
        if value.starts_with(query) {
            prefix = true;
        } else if value.contains(query) {
            substring = true;
        }
    }
    if prefix {
        Some(MatchKind::Prefix)
    } else if substring {
        Some(MatchKind::Substring)
    } else {
        None
    }
}

/// Active-task candidates for a solo leading `^` token: In Progress and
/// Next tasks with block IDs, ordered by today's open-Pomodoro Task Links
/// and ranked by the query. The `replacement` is the `route:block-id` an
/// accept inserts; `pomodoro` is the queued entry, or `null` when the task
/// is not queued.
pub(super) fn active_task_candidates(
    bob_dir: &Path,
    query: &str,
) -> (Candidates, Vec<String>) {
    let discovered = capture_active_tasks::discover(bob_dir);
    let candidates = capture_active_tasks::rank(&discovered.tasks, query)
        .into_iter()
        .map(|task| ActiveTaskCandidate {
            replacement: task.replacement(),
            task_ref: task.task_ref.clone(),
            route: task.route.clone(),
            block_id: task.block_id.clone(),
            status_symbol: task.status_symbol,
            status_name: task.status_name.clone(),
            status_type: task.status_type,
            text: task.text.clone(),
            section: task.section.clone(),
            pomodoro: task.pomodoro.as_ref().map(|pomodoro| {
                ActiveTaskPomodoroCandidate {
                    line: pomodoro.line,
                    name: pomodoro.name.clone(),
                    time_range: pomodoro.time_range.clone(),
                    is_current: pomodoro.is_current,
                }
            }),
            task_kind: task.task_kind,
        })
        .collect();
    (Candidates::ActiveTask(candidates), discovered.warnings)
}

/// `task_link` candidates for a solo leading `:` token: every linkable
/// open task (Ready, Blocked, Next, In Progress) in the routable inbox,
/// area, and non-terminal project notes, in canonical order and ranked by
/// the query. ID-less tasks are always included; `--all-tasks` does not
/// affect this context. The `replacement` covers the whole `:` token,
/// sigil included, because accepting rewrites the query into the canonical
/// `@route:block-id` marker.
pub(super) fn task_link_candidates(
    bob_dir: &Path,
    query: &str,
) -> (Candidates, Vec<String>) {
    let discovered = capture_link_tasks::discover(bob_dir);
    let candidates = capture_link_tasks::rank(&discovered.tasks, query)
        .into_iter()
        .map(|task| TaskLinkCandidate {
            replacement: task.replacement(),
            task_ref: task.task_ref.clone(),
            route: task.route.clone(),
            note_kind: task.note_kind,
            block_id: task.block_id.clone(),
            requires_block_id: task.block_id.is_none(),
            block_id_suggestions: task.block_id_suggestions.clone(),
            status_symbol: task.status_symbol,
            status_name: task.status_name.clone(),
            status_type: task.status_type,
            text: task.text.clone(),
            section: task.section.clone(),
            depth: task.depth,
            line: task.line,
            group: task.group,
            scheduled: task.scheduled.clone(),
            pulls_forward: task.pulls_forward,
            pomodoro: task.pomodoro.as_ref().map(|pomodoro| {
                ActiveTaskPomodoroCandidate {
                    line: pomodoro.line,
                    name: pomodoro.name.clone(),
                    time_range: pomodoro.time_range.clone(),
                    is_current: pomodoro.is_current,
                }
            }),
            task_kind: task.task_kind,
        })
        .collect();
    (Candidates::TaskLink(candidates), discovered.warnings)
}

/// `task_goto` candidates for a whole-draft `/` token: every linkable
/// open task (Ready, Blocked, Next, In Progress) in the routable inbox,
/// area, and non-terminal project notes, in canonical order and ranked by
/// the query -- exactly the `task_link` rows and order for the same query,
/// including ID-less tasks. `--all-tasks` does not affect this context.
/// The `replacement` is always empty (clients never insert it) and carries
/// no Add block ID metadata; `note_path` is the exact vault-relative path
/// and `target` the absolute path for opening the task.
///
/// Behind `closed_tasks`, the context lists closed rows from the
/// vault-wide catalog instead of the open rows, with the same filtering,
/// ordering, and row shape described today: filtered by the query with the
/// same `match_score` matcher, ordered newest close date first (undated
/// last, ties by `note_path` then `line`; for a non-empty query score
/// comes first). Closed rows carry `group: "completed"`, a `locator`
/// route, no `note_kind`, no `scheduled`/`pomodoro`, and an optional
/// `closed` date.
pub(super) fn task_goto_candidates(
    bob_dir: &Path,
    query: &str,
    closed_tasks: bool,
) -> (Candidates, Vec<String>) {
    if closed_tasks {
        return closed_task_goto_candidates(bob_dir, query);
    }
    let discovered = capture_link_tasks::discover(bob_dir);
    let candidates: Vec<TaskGotoCandidate> =
        capture_link_tasks::rank(&discovered.tasks, query)
            .into_iter()
            .map(|task| TaskGotoCandidate {
                replacement: String::new(),
                task_ref: task.task_ref.clone(),
                route: task.route.clone(),
                note_kind: Some(task.note_kind),
                note_path: task.note_path.clone(),
                target: bob_dir.join(&task.note_path).display().to_string(),
                block_id: task.block_id.clone(),
                status_symbol: task.status_symbol,
                status_name: task.status_name.clone(),
                status_type: task.status_type,
                text: task.text.clone(),
                section: task.section.clone(),
                depth: task.depth,
                line: task.line,
                group: task.group,
                scheduled: task.scheduled.clone(),
                pomodoro: task.pomodoro.as_ref().map(|pomodoro| {
                    ActiveTaskPomodoroCandidate {
                        line: pomodoro.line,
                        name: pomodoro.name.clone(),
                        time_range: pomodoro.time_range.clone(),
                        is_current: pomodoro.is_current,
                    }
                }),
                closed: None,
                task_kind: task.task_kind,
            })
            .collect();
    let warnings = discovered.warnings;
    (Candidates::TaskGoto(candidates), warnings)
}

/// Closed-only `task_goto` rows from the vault-wide catalog, newest close
/// date first (undated last). Warnings come from the vault catalog.
fn closed_task_goto_candidates(
    bob_dir: &Path,
    query: &str,
) -> (Candidates, Vec<String>) {
    let vault = capture_dependency_tasks::discover(bob_dir);
    let mut scored: Vec<(&capture_dependency_tasks::DependencyTask, u32)> =
        vault
            .tasks
            .iter()
            .filter(|task| !task.open)
            .filter_map(|task| {
                capture_link_tasks::match_score(
                    &capture_dependency_tasks::dependency_search_fields(task),
                    query,
                )
                .map(|score| (task, score))
            })
            .collect();
    scored.sort_by(|left, right| {
        let (left_task, left_score) = left;
        let (right_task, right_score) = right;
        right_score.cmp(left_score).then_with(|| {
            closed_goto_order_key(left_task)
                .cmp(&closed_goto_order_key(right_task))
        })
    });
    let candidates: Vec<TaskGotoCandidate> = scored
        .into_iter()
        .map(|(task, _)| TaskGotoCandidate {
            replacement: String::new(),
            task_ref: task.task_ref.clone(),
            route: task.locator.clone(),
            note_kind: None,
            note_path: task.note_path.clone(),
            target: bob_dir.join(&task.note_path).display().to_string(),
            block_id: task.block_id.clone(),
            status_symbol: task.status_symbol,
            status_name: task.status_name.clone(),
            status_type: capture_tasks::status_type_label(task.status_type),
            text: task.text.clone(),
            section: task.section.clone(),
            depth: task.depth,
            line: task.line,
            group: capture_link_tasks::LinkTaskGroup::Completed,
            scheduled: None,
            pomodoro: None,
            closed: task.closed_on.clone(),
            task_kind: task.task_kind,
        })
        .collect();
    (Candidates::TaskGoto(candidates), vault.warnings)
}

/// Close-date order for a closed Go To row: dated rows first, newest
/// first, undated last; ties break by `note_path`, then `line`.
fn closed_goto_order_key(
    task: &capture_dependency_tasks::DependencyTask,
) -> (u8, std::cmp::Reverse<String>, String, usize) {
    match &task.closed_on {
        Some(date) => (
            0,
            std::cmp::Reverse(date.clone()),
            task.note_path.clone(),
            task.line,
        ),
        None => (
            1,
            std::cmp::Reverse(String::new()),
            task.note_path.clone(),
            task.line,
        ),
    }
}

/// `task_parent` candidates for a bare-plus selector. Reuses the colon
/// picker discovery and stable ranking, but emits the parent-task `+`
/// spelling and omits link-only pull-forward metadata.
pub(super) fn task_parent_candidates(
    bob_dir: &Path,
    query: &str,
) -> (Candidates, Vec<String>) {
    let discovered = capture_link_tasks::discover(bob_dir);
    let candidates = capture_link_tasks::rank(&discovered.tasks, query)
        .into_iter()
        .map(|task| {
            let replacement = task
                .block_id
                .as_ref()
                .map(|block_id| format!("@{}+{block_id}", task.route))
                .unwrap_or_default();
            TaskParentCandidate {
                replacement,
                task_ref: task.task_ref.clone(),
                route: task.route.clone(),
                note_path: task.note_path.clone(),
                note_kind: task.note_kind,
                block_id: task.block_id.clone(),
                requires_block_id: task.block_id.is_none(),
                block_id_suggestions: task.block_id_suggestions.clone(),
                status_symbol: task.status_symbol,
                status_name: task.status_name.clone(),
                status_type: task.status_type,
                text: task.text.clone(),
                disabled_reason: None,
                consequence: None,
                detail: None,
                section: task.section.clone(),
                depth: task.depth,
                line: task.line,
                group: task.group,
                pomodoro: task.pomodoro.as_ref().map(|pomodoro| {
                    ActiveTaskPomodoroCandidate {
                        line: pomodoro.line,
                        name: pomodoro.name.clone(),
                        time_range: pomodoro.time_range.clone(),
                        is_current: pomodoro.is_current,
                    }
                }),
                task_kind: task.task_kind,
            }
        })
        .collect();
    (Candidates::TaskParent(candidates), discovered.warnings)
}

/// Adds relationship-aware guards to either parent-task picker and returns
/// the matching picker descriptor data. The same vault-wide task catalog and
/// dependency graph power both the `+` and note-scoped `@route+` pickers.
pub(super) fn apply_relationship_picker(
    bob_dir: &Path,
    candidates: &mut Candidates,
    item: &EditorItemParse,
) -> Option<PickerRelationship> {
    if item.dependencies.is_empty()
        && item.blocked_tasks.is_empty()
        && item.relations.is_empty()
    {
        return None;
    }

    let discovered = capture_dependency_tasks::discover(bob_dir);
    let graph =
        capture_dependency_tasks::dependency_graph(bob_dir, &discovered);
    let resolved_dependencies: Vec<_> = item
        .dependencies
        .iter()
        .map(|entry| resolve_relationship_entry(bob_dir, &discovered, entry))
        .collect();
    let resolved_blocked: Vec<_> = item
        .blocked_tasks
        .iter()
        .map(|entry| resolve_relationship_entry(bob_dir, &discovered, entry))
        .collect();
    let relations = item
        .relations
        .iter()
        .map(|entry| {
            let text = capture_dependency_tasks::resolve_dependency_note(
                bob_dir,
                &discovered,
                &entry.note,
            )
            .ok()
            .map(|path| capture_dependency_tasks::display_path(&path))
            .and_then(|note_path| {
                unique_task(&discovered, &note_path, &entry.block_id)
                    .map(|task| task.text.clone())
            })
            .unwrap_or_else(|| entry.raw.clone());
            PickerRelationEntry {
                direction: entry.direction,
                text,
            }
        })
        .collect();
    let recognized_projects = recognized_project_paths(bob_dir, &discovered);
    let settings = note_tasks::read_settings(bob_dir);
    let mut note_cache = HashMap::new();

    let mut annotate =
        |route: &str,
         block_id: Option<&str>,
         replacement: &mut String,
         disabled_reason: &mut Option<String>| {
            let Some(block_id) = block_id else {
                return;
            };
            let Some(identity) =
                task_identity_for_route(bob_dir, &discovered, route, block_id)
            else {
                return;
            };
            let reason = relationship_disabled_reason(
                bob_dir,
                &discovered,
                &graph,
                &recognized_projects,
                &settings,
                &mut note_cache,
                &identity,
                &resolved_dependencies,
                &resolved_blocked,
            );
            if let Some(reason) = reason {
                *replacement = String::new();
                *disabled_reason = Some(reason);
            }
        };

    match candidates {
        Candidates::TaskParent(rows) => {
            for row in rows {
                annotate(
                    &row.route,
                    row.block_id.as_deref(),
                    &mut row.replacement,
                    &mut row.disabled_reason,
                );
            }
        }
        Candidates::Task(rows) => {
            for row in rows {
                annotate(
                    &row.route,
                    row.block_id.as_deref(),
                    &mut row.replacement,
                    &mut row.disabled_reason,
                );
            }
        }
        _ => return None,
    }

    Some(PickerRelationship {
        dependencies: resolved_dependencies
            .into_iter()
            .map(|entry| entry.wire)
            .collect(),
        blocked_tasks: resolved_blocked
            .into_iter()
            .map(|entry| entry.wire)
            .collect(),
        relations,
    })
}

struct ResolvedRelationshipEntry {
    wire: PickerRelationshipEntry,
    identity: Option<capture_dependency_tasks::TaskIdentity>,
}

fn resolve_relationship_entry(
    bob_dir: &Path,
    discovered: &capture_dependency_tasks::DependencyTaskResult,
    entry: &DependencyEntry,
) -> ResolvedRelationshipEntry {
    let resolved = capture_dependency_tasks::resolve_dependency_note(
        bob_dir,
        discovered,
        &entry.note,
    )
    .ok()
    .map(|path| capture_dependency_tasks::display_path(&path));
    let task = resolved.as_ref().and_then(|note_path| {
        unique_task(discovered, note_path, &entry.block_id)
    });
    let note_path = task
        .map(|task| task.note_path.clone())
        .or_else(|| resolved.clone())
        .unwrap_or_else(|| entry.note.clone());
    ResolvedRelationshipEntry {
        wire: PickerRelationshipEntry {
            raw: entry.raw.clone(),
            note_path,
            block_id: entry.block_id.clone(),
            text: task.map(|task| task.text.clone()),
        },
        identity: task
            .map(|task| (task.note_path.clone(), entry.block_id.clone())),
    }
}

fn unique_task<'a>(
    discovered: &'a capture_dependency_tasks::DependencyTaskResult,
    note_path: &str,
    block_id: &str,
) -> Option<&'a capture_dependency_tasks::DependencyTask> {
    let mut matches = discovered.tasks.iter().filter(|task| {
        task.note_path == note_path
            && task.block_id.as_deref() == Some(block_id)
    });
    let first = matches.next()?;
    matches.next().is_none().then_some(first)
}

fn task_identity_for_route(
    bob_dir: &Path,
    discovered: &capture_dependency_tasks::DependencyTaskResult,
    route: &str,
    block_id: &str,
) -> Option<capture_dependency_tasks::TaskIdentity> {
    let typed = capture::route_label(route);
    let path = capture_dependency_tasks::resolve_dependency_note(
        bob_dir, discovered, &typed,
    )
    .ok()?;
    let note_path = capture_dependency_tasks::display_path(&path);
    unique_task(discovered, &note_path, block_id)?;
    Some((note_path, block_id.to_string()))
}

fn relationship_disabled_reason(
    bob_dir: &Path,
    discovered: &capture_dependency_tasks::DependencyTaskResult,
    graph: &HashMap<
        capture_dependency_tasks::TaskIdentity,
        HashSet<capture_dependency_tasks::TaskIdentity>,
    >,
    recognized_projects: &HashSet<String>,
    settings: &note_tasks::NoteTaskSettings,
    note_cache: &mut HashMap<PathBuf, String>,
    candidate: &capture_dependency_tasks::TaskIdentity,
    dependencies: &[ResolvedRelationshipEntry],
    blocked_tasks: &[ResolvedRelationshipEntry],
) -> Option<String> {
    if dependencies
        .iter()
        .any(|entry| entry.identity.as_ref() == Some(candidate))
    {
        return Some("the prerequisite itself".to_string());
    }
    if blocked_tasks
        .iter()
        .any(|entry| entry.identity.as_ref() == Some(candidate))
    {
        return Some("the blocked task itself".to_string());
    }

    let dependency_cycle = dependencies.iter().any(|entry| {
        entry.identity.as_ref().is_some_and(|identity| {
            dependency_reachable(graph, std::slice::from_ref(identity))
                .contains(candidate)
        })
    });
    let blocked_cycle = blocked_tasks.iter().any(|entry| {
        entry.identity.as_ref().is_some_and(|identity| {
            dependency_reachable(graph, std::slice::from_ref(candidate))
                .contains(identity)
        })
    });
    if dependency_cycle || blocked_cycle {
        return Some("would create a dependency cycle".to_string());
    }

    let candidate_note = candidate.0.as_str();
    // `&` candidate is the dependent: refuse only when it is `^prj`
    // of a recognised project and a typed prerequisite lives in that
    // note or a descendant. `*` typed blocked task is the dependent:
    // refuse only when that task is such a `^prj` and the candidate
    // lives in its note or a descendant. Matches the writer
    // (`refuse_internal_prj_targets`).
    let project_resident = dependencies.iter().any(|entry| {
        entry.identity.as_ref().is_some_and(|identity| {
            project_cannot_wait_on_resident(
                bob_dir,
                discovered,
                recognized_projects,
                settings,
                note_cache,
                candidate,
                &identity.0,
            )
        })
    }) || blocked_tasks.iter().any(|entry| {
        entry.identity.as_ref().is_some_and(|identity| {
            project_cannot_wait_on_resident(
                bob_dir,
                discovered,
                recognized_projects,
                settings,
                note_cache,
                identity,
                candidate_note,
            )
        })
    });
    project_resident
        .then(|| "a project cannot depend on its own resident".to_string())
}

fn recognized_project_paths(
    bob_dir: &Path,
    discovered: &capture_dependency_tasks::DependencyTaskResult,
) -> HashSet<String> {
    let settings = note_tasks::read_settings(bob_dir);
    let mut note_cache = HashMap::new();
    discovered
        .tasks
        .iter()
        .filter(|task| task.block_id.as_deref() == Some("prj") && task.open)
        .filter_map(|task| {
            let path = PathBuf::from(&task.note_path);
            read_note_cached(bob_dir, &path, &mut note_cache)
                .filter(|contents| {
                    is_recognized_project(&path, contents, bob_dir, &settings)
                })
                .map(|_| task.note_path.clone())
        })
        .collect()
}

fn project_cannot_wait_on_resident(
    bob_dir: &Path,
    discovered: &capture_dependency_tasks::DependencyTaskResult,
    recognized_projects: &HashSet<String>,
    settings: &note_tasks::NoteTaskSettings,
    note_cache: &mut HashMap<PathBuf, String>,
    dependent: &capture_dependency_tasks::TaskIdentity,
    other_note: &str,
) -> bool {
    if dependent.1 != "prj" {
        return false;
    }
    project_resident_relation(
        bob_dir,
        discovered,
        recognized_projects,
        settings,
        note_cache,
        &dependent.0,
        other_note,
    )
}

fn project_resident_relation(
    bob_dir: &Path,
    discovered: &capture_dependency_tasks::DependencyTaskResult,
    recognized_projects: &HashSet<String>,
    settings: &note_tasks::NoteTaskSettings,
    note_cache: &mut HashMap<PathBuf, String>,
    project_note: &str,
    resident_note: &str,
) -> bool {
    if !recognized_projects.contains(project_note) {
        return false;
    }
    if project_note == resident_note {
        return true;
    }
    let resident_path = PathBuf::from(resident_note);
    let Some(contents) = read_note_cached(bob_dir, &resident_path, note_cache)
    else {
        return false;
    };
    project_dependencies::project_ancestors(
        &resident_path,
        &contents,
        &discovered.index,
        |path| read_note_cached(bob_dir, path, note_cache),
        |path, contents| {
            is_recognized_project(path, contents, bob_dir, settings)
        },
    )
    .into_iter()
    .any(|path| capture_dependency_tasks::display_path(&path) == project_note)
}

/// `task_dependency` candidates for an `&` token: every task in the
/// vault-wide catalog in picker order (same-note open tasks, In
/// Progress, Next, other open tasks grouped by note, then
/// completed/cancelled history; ranked matches first for a query),
/// annotated with the exact note identity, display locator, stable
/// group, and the guards the picker badges.
///
/// Known self/already-selected targets and unusable identities stay
/// visible with an explanatory reason and no insertable replacement;
/// ID-less rows carry `requires_block_id` and suggestions for the
/// explicit Add block ID flow instead. Read-only: selecting or
/// highlighting a row writes nothing. No ledger file is required.
pub(super) fn dependency_candidates(
    bob_dir: &Path,
    query: &str,
    owner: Option<&DependencyTarget>,
    typed: &[(String, String)],
) -> (Candidates, Vec<String>) {
    let discovered = capture_dependency_tasks::discover(bob_dir);
    let owner_note = owner
        .as_ref()
        .and_then(|target| target.route.as_deref())
        .map(capture::route_label);
    let owner_block_id = owner.as_ref().and_then(|target| {
        if target.kind == DependencyTargetKind::ExistingTask {
            target.block_id.as_deref()
        } else {
            None
        }
    });
    let dependent_note = owner_note.as_deref().map(Path::new);
    let present = capture_dependency_tasks::already_present_prerequisites(
        bob_dir,
        &discovered,
        dependent_note,
        owner_block_id,
        typed,
    );
    let ordered = capture_dependency_tasks::order_for_picker(
        &discovered.tasks,
        query,
        owner_note.as_deref(),
    );
    let candidates = ordered
        .into_iter()
        .map(|task| {
            let is_self = owner_block_id.is_some_and(|id| {
                Some(task.note_path.as_str()) == owner_note.as_deref()
                    && task.block_id.as_deref() == Some(id)
            });
            let already = present.contains(&(
                task.note_path.clone(),
                task.block_id.clone().unwrap_or_default(),
            )) && task.block_id.is_some();
            let disabled_reason = if is_self {
                Some("the dependent task itself".to_string())
            } else if task.duplicate_id {
                Some(format!(
                    "duplicate block ID ^{} in {}",
                    task.block_id.as_deref().unwrap_or_default(),
                    task.note_path
                ))
            } else {
                None
            };
            let requires_block_id = task.block_id.is_none();
            let replacement = if requires_block_id || disabled_reason.is_some()
            {
                String::new()
            } else {
                capture_dependency_tasks::replacement_for(
                    &discovered.index,
                    Path::new(&task.note_path),
                    task.block_id.as_deref().expect("identified task"),
                )
            };
            DependencyCandidate {
                replacement,
                task_ref: task.task_ref.clone(),
                note_path: task.note_path.clone(),
                locator: task.locator.clone(),
                group: capture_dependency_tasks::group_for(
                    task,
                    owner_note.as_deref(),
                )
                .to_string(),
                hidden: task.hidden,
                block_id: task.block_id.clone(),
                requires_block_id,
                block_id_suggestions: task.block_id_suggestions.clone(),
                already_dependency: already,
                disabled_reason,
                status_symbol: task.status_symbol,
                status_name: task.status_name.clone(),
                status_type: capture_tasks::status_type_label(task.status_type),
                text: task.text.clone(),
                section: task.section.clone(),
                depth: task.depth,
                line: task.line,
                task_kind: task.task_kind,
            }
        })
        .collect();
    (Candidates::Dependency(candidates), discovered.warnings)
}

/// `blocked_task` candidates for a `*` modifier. Only open, editable tasks
/// are offered. Today links and the task lanes establish the stable order;
/// the dependency graph adds early self, cycle, project-resident, duplicate,
/// and already-blocked metadata while the capture writer remains final.
pub(super) fn blocked_task_candidates(
    bob_dir: &Path,
    query: &str,
    owner: Option<&DependencyTarget>,
    owner_note: &str,
    typed_dependencies: &[(String, String)],
    typed_blocked: &[(String, String)],
) -> (Candidates, Vec<String>) {
    let discovered = capture_dependency_tasks::discover(bob_dir);
    let graph =
        capture_dependency_tasks::dependency_graph(bob_dir, &discovered);
    let day_file = pomodoro::day_file_for(bob_dir);
    let mut ledger_warnings = Vec::new();
    let ledger =
        capture_active_tasks::read_ledger(&day_file, &mut ledger_warnings);
    let _ = ledger_warnings;
    let warnings = discovered.warnings.clone();

    let owner_path = PathBuf::from(capture::route_label(owner_note));
    let owner_path_display = owner_path.to_string_lossy().replace('\\', "/");
    let owner_identity = owner.and_then(|target| {
        if target.kind != DependencyTargetKind::ExistingTask {
            return None;
        }
        let block_id = target.block_id.as_ref()?;
        let path = target
            .route
            .as_deref()
            .map(capture::route_label)
            .unwrap_or_else(|| owner_path_display.clone());
        Some((path, block_id.clone()))
    });

    let mut typed_blocked_identities = HashSet::new();
    for (note, block_id) in typed_blocked {
        if let Ok(path) = capture_dependency_tasks::resolve_dependency_note(
            bob_dir,
            &discovered,
            note,
        ) {
            typed_blocked_identities.insert((
                capture_dependency_tasks::display_path(&path),
                block_id.clone(),
            ));
        }
    }

    let mut cycle_roots = Vec::new();
    if let Some(identity) = &owner_identity {
        cycle_roots.push(identity.clone());
    }
    for (note, block_id) in typed_dependencies {
        if let Ok(path) = capture_dependency_tasks::resolve_dependency_note(
            bob_dir,
            &discovered,
            note,
        ) {
            cycle_roots.push((
                capture_dependency_tasks::display_path(&path),
                block_id.clone(),
            ));
        }
    }
    cycle_roots.sort();
    cycle_roots.dedup();
    let reachable = dependency_reachable(&graph, &cycle_roots);

    let settings = note_tasks::read_settings(bob_dir);
    let mut note_cache: HashMap<PathBuf, String> = HashMap::new();
    let recognized_projects = recognized_project_paths(bob_dir, &discovered);

    let mut ordered: Vec<(&capture_dependency_tasks::DependencyTask, u32)> =
        discovered
            .tasks
            .iter()
            .filter(|task| is_editable_open_task(task))
            .filter_map(|task| {
                capture_link_tasks::match_score(
                    &capture_dependency_tasks::dependency_search_fields(task),
                    query,
                )
                .map(|score| (task, score))
            })
            .collect();
    ordered.sort_by(|(left, left_score), (right, right_score)| {
        right_score.cmp(left_score).then_with(|| {
            blocked_task_order_key(left, &ledger, &owner_path_display).cmp(
                &blocked_task_order_key(right, &ledger, &owner_path_display),
            )
        })
    });

    let candidates = ordered
        .into_iter()
        .map(|(task, _)| {
            let key = task
                .block_id
                .as_ref()
                .map(|id| (task.note_path.clone(), id.clone()));
            let is_self = owner_identity
                .as_ref()
                .is_some_and(|owner| key.as_ref() == Some(owner));
            let already = key.as_ref().is_some_and(|key| {
                typed_blocked_identities.contains(key)
                    || owner_identity.as_ref().is_some_and(|owner| {
                        graph.get(key).is_some_and(|prerequisites| {
                            prerequisites.contains(owner)
                        })
                    })
            });
            let internal_project = key.as_ref().is_some_and(|identity| {
                project_cannot_wait_on_resident(
                    bob_dir,
                    &discovered,
                    &recognized_projects,
                    &settings,
                    &mut note_cache,
                    identity,
                    &owner_path_display,
                )
            });
            let disabled_reason = if is_self {
                Some("the owner task itself".to_string())
            } else if task.duplicate_id {
                Some(format!(
                    "duplicate block ID ^{} in {}",
                    task.block_id.as_deref().unwrap_or_default(),
                    task.note_path
                ))
            } else if !already
                && key.as_ref().is_some_and(|key| reachable.contains(key))
            {
                Some("would create a dependency cycle".to_string())
            } else if internal_project {
                Some("a project cannot depend on its own resident".to_string())
            } else {
                None
            };
            let requires_block_id = task.block_id.is_none();
            let replacement = if requires_block_id || disabled_reason.is_some()
            {
                String::new()
            } else {
                capture_dependency_tasks::replacement_for_sigil(
                    &discovered.index,
                    Path::new(&task.note_path),
                    task.block_id.as_deref().expect("identified task"),
                    b'*',
                )
            };
            BlockedTaskCandidate {
                replacement,
                task_ref: task.task_ref.clone(),
                note_path: task.note_path.clone(),
                locator: task.locator.clone(),
                group: blocked_task_group(task, &ledger, &owner_path_display)
                    .to_string(),
                hidden: task.hidden,
                block_id: task.block_id.clone(),
                requires_block_id,
                block_id_suggestions: task.block_id_suggestions.clone(),
                already_blocked: already,
                disabled_reason,
                status_symbol: task.status_symbol,
                status_name: task.status_name.clone(),
                status_type: capture_tasks::status_type_label(task.status_type),
                text: task.text.clone(),
                section: task.section.clone(),
                depth: task.depth,
                line: task.line,
                task_kind: task.task_kind,
            }
        })
        .collect();
    (Candidates::BlockedTask(candidates), warnings)
}

/// Relation targets use the same complete task catalog as dependencies.
/// `to` reads the owner's LINKS line, while `from` reads each offered
/// source's LINKS line because that is the task the capture would edit.
pub(super) fn relation_candidates(
    bob_dir: &Path,
    query: &str,
    direction: RelationDirection,
    owner: Option<&DependencyTarget>,
    owner_note: &str,
    typed_relations: &[RelationEntry],
    typed_dependencies: &[DependencyEntry],
) -> (Candidates, Vec<String>) {
    let discovered = capture_dependency_tasks::discover(bob_dir);
    let (links, mut warnings, _) = task_links_catalog(bob_dir, &discovered);
    let graph =
        capture_dependency_tasks::dependency_graph(bob_dir, &discovered);
    let mut ledger_warnings = Vec::new();
    let ledger = capture_active_tasks::read_ledger(
        &pomodoro::day_file_for(bob_dir),
        &mut ledger_warnings,
    );
    warnings.extend(ledger_warnings);
    let owner_identity = owner.and_then(|target| {
        if target.kind != DependencyTargetKind::ExistingTask {
            return None;
        }
        let route = target.route.as_deref()?;
        let block_id = target.block_id.as_deref()?;
        task_identity_for_route(bob_dir, &discovered, route, block_id)
    });

    let mut prerequisites = HashSet::new();
    if let Some(identity) = &owner_identity
        && let Some(targets) = graph.get(identity)
    {
        prerequisites.extend(targets.iter().cloned());
    }
    for entry in typed_dependencies {
        if let Ok(path) = capture_dependency_tasks::resolve_dependency_note(
            bob_dir,
            &discovered,
            &entry.note,
        ) {
            prerequisites.insert((
                capture_dependency_tasks::display_path(&path),
                entry.block_id.clone(),
            ));
        }
    }

    let mut typed = HashMap::new();
    for entry in typed_relations
        .iter()
        .filter(|entry| entry.direction == direction)
    {
        if let Ok(path) = capture_dependency_tasks::resolve_dependency_note(
            bob_dir,
            &discovered,
            &entry.note,
        ) {
            typed
                .entry((
                    capture_dependency_tasks::display_path(&path),
                    entry.block_id.clone(),
                ))
                .or_insert_with(|| entry.label.clone());
        }
    }

    let owner_links = owner_identity
        .as_ref()
        .and_then(|identity| links.get(identity));
    let ordered = relation_picker_order(
        bob_dir,
        &discovered.tasks,
        query,
        Some(owner_note),
        direction,
        &ledger,
    );
    let candidates = ordered
        .into_iter()
        .map(|task| {
            let key = task
                .block_id
                .as_ref()
                .map(|id| (task.note_path.clone(), id.clone()));
            let is_self = key.as_ref().is_some_and(|identity| {
                owner_identity.as_ref() == Some(identity)
            });
            let candidate_links =
                key.as_ref().and_then(|identity| links.get(identity));
            let stored_link = match direction {
                RelationDirection::To => key.as_ref().and_then(|identity| {
                    owner_links.and_then(|source| source.outgoing.get(identity))
                }),
                RelationDirection::From => {
                    owner_identity.as_ref().and_then(|owner| {
                        candidate_links
                            .and_then(|source| source.outgoing.get(owner))
                    })
                }
            };
            let stored_label = stored_link.cloned().flatten();
            let typed_label = key
                .as_ref()
                .and_then(|identity| typed.get(identity).cloned())
                .flatten();
            let already_linked = stored_link.is_some()
                || key
                    .as_ref()
                    .is_some_and(|identity| typed.contains_key(identity));
            let archived = crate::native::done_archive::is_archive_path(
                Path::new(&task.note_path),
            );
            let read_only = direction == RelationDirection::From
                && is_relation_source_read_only(bob_dir, task);
            let malformed = direction == RelationDirection::From
                && candidate_links.is_some_and(|source| source.malformed);
            let disabled_reason = if is_self {
                Some("the owner task itself".to_string())
            } else if task.duplicate_id {
                Some(format!(
                    "duplicate block ID ^{} in {}",
                    task.block_id.as_deref().unwrap_or_default(),
                    task.note_path
                ))
            } else if read_only {
                Some("read-only note".to_string())
            } else if malformed {
                Some("its LINKS line is malformed".to_string())
            } else if archived && task.block_id.is_none() {
                Some("archived tasks can't get an id".to_string())
            } else {
                None
            };
            let requires_block_id = task.block_id.is_none();
            let replacement = if requires_block_id || disabled_reason.is_some()
            {
                String::new()
            } else {
                capture_dependency_tasks::replacement_for_prefix(
                    &discovered.index,
                    Path::new(&task.note_path),
                    task.block_id.as_deref().expect("identified relation task"),
                    match direction {
                        RelationDirection::To => "~",
                        RelationDirection::From => "~~",
                    },
                )
            };
            RelationCandidate {
                replacement,
                task_ref: task.task_ref.clone(),
                note_path: task.note_path.clone(),
                locator: task.locator.clone(),
                group: capture_dependency_tasks::group_for(
                    task,
                    Some(owner_note),
                )
                .to_string(),
                hidden: task.hidden,
                block_id: task.block_id.clone(),
                requires_block_id,
                block_id_suggestions: task.block_id_suggestions.clone(),
                already_linked,
                existing_label: typed_label.or(stored_label),
                also_prerequisite: key
                    .as_ref()
                    .is_some_and(|identity| prerequisites.contains(identity)),
                disabled_reason,
                status_symbol: task.status_symbol,
                status_name: task.status_name.clone(),
                status_type: capture_tasks::status_type_label(task.status_type),
                text: task.text.clone(),
                section: task.section.clone(),
                depth: task.depth,
                line: task.line,
                task_kind: task.task_kind,
            }
        })
        .collect::<Vec<_>>();

    (Candidates::Relation(candidates), warnings)
}

/// Build one source LINKS view per identified task and count normalized
/// labels across every accepted LINKS line, including `done/` history.
fn task_links_catalog(
    bob_dir: &Path,
    discovered: &capture_dependency_tasks::DependencyTaskResult,
) -> (
    HashMap<capture_dependency_tasks::TaskIdentity, TaskLinks>,
    Vec<String>,
    Vec<(String, usize)>,
) {
    let mut tasks_by_note: HashMap<
        &str,
        Vec<&capture_dependency_tasks::DependencyTask>,
    > = HashMap::new();
    for task in &discovered.tasks {
        tasks_by_note
            .entry(task.note_path.as_str())
            .or_default()
            .push(task);
    }
    let mut links = HashMap::new();
    let mut warnings = Vec::new();
    let mut labels: HashMap<String, (String, usize)> = HashMap::new();
    for (note_path, tasks) in tasks_by_note {
        let Ok(contents) = fs::read_to_string(bob_dir.join(note_path)) else {
            continue;
        };
        let lines = contents.lines().collect::<Vec<_>>();
        let fenced =
            crate::native::markdown::fenced_lines(&lines, 0..lines.len());
        for task in tasks {
            let block_id = task.block_id.as_deref();
            let task_line = task.line.saturating_sub(1);
            let children = task_relations::parse::links_children_of(
                &lines, &fenced, task_line,
            );
            let mut source = TaskLinks::default();
            for child in children {
                match child.parsed {
                    task_relations::LinksLine::Accepted { entries, .. } => {
                        for entry in entries {
                            if let Some(label) = entry.label.as_deref() {
                                let folded = label.to_lowercase();
                                let value = labels
                                    .entry(folded)
                                    .or_insert_with(|| (label.to_string(), 0));
                                value.1 += 1;
                            }
                            let target_path = if entry.target.is_empty() {
                                Some(PathBuf::from(&task.note_path))
                            } else {
                                discovered.index.resolve(
                                    Some(Path::new(&task.note_path)),
                                    &entry.target,
                                )
                            };
                            if let Some(target_path) = target_path {
                                let identity = (
                                    capture_dependency_tasks::display_path(
                                        &target_path,
                                    ),
                                    entry.block_id,
                                );
                                source
                                    .outgoing
                                    .entry(identity)
                                    .or_insert(entry.label);
                            }
                        }
                    }
                    task_relations::LinksLine::Malformed => {
                        source.malformed = true;
                    }
                    task_relations::LinksLine::Empty
                    | task_relations::LinksLine::NotALine => {}
                }
            }
            if let Some(block_id) = block_id {
                links.insert(
                    (task.note_path.clone(), block_id.to_string()),
                    source,
                );
            }
        }
    }
    let mut labels = labels.into_values().collect::<Vec<_>>();
    labels.sort_by(|left, right| {
        right
            .1
            .cmp(&left.1)
            .then_with(|| left.0.to_lowercase().cmp(&right.0.to_lowercase()))
            .then_with(|| left.0.cmp(&right.0))
    });
    warnings.extend(discovered.warnings.iter().cloned());
    (links, warnings, labels)
}

fn relation_picker_order<'a>(
    bob_dir: &Path,
    tasks: &'a [capture_dependency_tasks::DependencyTask],
    query: &str,
    owner_note: Option<&str>,
    direction: RelationDirection,
    ledger: &capture_active_tasks::Ledger,
) -> Vec<&'a capture_dependency_tasks::DependencyTask> {
    let mut scored = tasks
        .iter()
        .filter(|task| {
            direction == RelationDirection::To
                || is_editable_open_task(task)
                || is_relation_source_read_only(bob_dir, task)
        })
        .filter_map(|task| {
            capture_link_tasks::match_score(
                &capture_dependency_tasks::dependency_search_fields(task),
                query,
            )
            .map(|score| (task, score))
        })
        .collect::<Vec<_>>();
    scored.sort_by(|(left, left_score), (right, right_score)| {
        right_score.cmp(left_score).then_with(|| {
            relation_order_key(left, owner_note, direction, ledger)
                .cmp(&relation_order_key(right, owner_note, direction, ledger))
        })
    });
    scored.into_iter().map(|(task, _)| task).collect()
}

fn relation_order_key(
    task: &capture_dependency_tasks::DependencyTask,
    owner_note: Option<&str>,
    direction: RelationDirection,
    ledger: &capture_active_tasks::Ledger,
) -> (u8, u8, usize, usize, u8, String, usize) {
    let hidden = u8::from(task.hidden);
    if direction == RelationDirection::To {
        let group = if owner_note == Some(task.note_path.as_str()) {
            0
        } else if crate::native::done_archive::is_archive_path(Path::new(
            &task.note_path,
        )) {
            5
        } else if task.open && task.status_symbol == '/' {
            1
        } else if task.open && task.status_symbol == '*' {
            2
        } else if task.open {
            3
        } else {
            4
        };
        return (group, hidden, 0, 0, 0, task.note_path.clone(), task.line);
    }

    if let Some((entry, link)) = task_ledger_position(task, ledger) {
        let running = task
            .block_id
            .as_ref()
            .and_then(|id| {
                ledger
                    .owners
                    .get(&(task_route(&task.note_path), id.clone()))
            })
            .is_some_and(|pomodoro| pomodoro.is_current);
        return (
            0,
            u8::from(!running),
            entry,
            link,
            hidden,
            task.note_path.clone(),
            task.line,
        );
    }
    let group = if task.status_symbol == '/' {
        1
    } else if task.status_symbol == '*' {
        2
    } else if owner_note == Some(task.note_path.as_str()) {
        3
    } else {
        4
    };
    (group, 0, 0, 0, hidden, task.note_path.clone(), task.line)
}

#[derive(Default)]
struct TaskLinks {
    outgoing: HashMap<capture_dependency_tasks::TaskIdentity, Option<String>>,
    malformed: bool,
}

/// Relation label rows for one `relation_label` probe. `one_off` is true
/// when the probe sits in a `||` label: `replacement` then keeps the
/// `||` spelling while `hide_replacement` is always the `||` form.
/// Configured effect labels mark their rows with their effect flag, and
/// zero-use configured labels appear as a trailing `configured` overlay
/// that respects hiding.
pub(super) fn relation_label_suggestions(
    bob_dir: &Path,
    query: &str,
    one_off: bool,
    policy: &config::RelationsConfig,
) -> (Candidates, Vec<String>, usize) {
    let catalog = match crate::native::relation_labels::Catalog::load(bob_dir) {
        Ok(catalog) => catalog,
        Err(error) => {
            return (
                Candidates::RelationLabel(Vec::new()),
                vec![format!("relation label catalog: {error}")],
                0,
            );
        }
    };
    let validation =
        crate::native::task_relations::label::normalize_relation_label(query);
    let normalized_query = validation.as_ref().cloned().unwrap_or_else(|_| {
        crate::native::task_relations::label::collapse_label_text(query)
    });
    let mut suggestions = catalog
        .rank_for_query(
            &normalized_query,
            crate::native::relation_labels::HiddenFilter::Exclude,
        )
        .into_iter()
        .map(|record| RelationLabelSuggestion {
            replacement: if one_off {
                record.hide_replacement()
            } else {
                record.replacement()
            },
            hide_replacement: record.hide_replacement(),
            label: record.name.clone(),
            key: record.key.clone(),
            inverse: record.inverse.clone(),
            count: Some(record.count as usize),
            typed: false,
            disabled_reason: None,
            existing: None,
            hidden: None,
            cancels_source: policy.is_cancel_label(&record.key),
            completes_source: policy.is_complete_label(&record.key),
            merges_source: policy.is_merge_label(&record.key),
            configured: false,
            offers_source: policy.is_successor_label(&record.key),
        })
        .collect::<Vec<_>>();
    append_cancel_overlay(
        &mut suggestions,
        &catalog,
        &normalized_query,
        one_off,
        policy,
    );
    append_complete_overlay(
        &mut suggestions,
        &catalog,
        &normalized_query,
        one_off,
        policy,
    );
    append_merge_overlay(
        &mut suggestions,
        &catalog,
        &normalized_query,
        one_off,
        policy,
    );
    append_successor_overlay(
        &mut suggestions,
        &catalog,
        &normalized_query,
        one_off,
        policy,
    );
    if !normalized_query.is_empty() {
        let (label, disabled_reason) = match validation {
            Ok(label) => (label, None),
            Err(error) => (
                normalized_query.clone(),
                Some(match error {
                    crate::native::task_relations::label::LabelError::ReservedMention => {
                        "💬 is reserved for text mentions"
                    }
                    crate::native::task_relations::label::LabelError::Empty => {
                        "label can't be empty"
                    }
                    crate::native::task_relations::label::LabelError::TooLong => {
                        "label can't exceed 40 characters"
                    }
                    crate::native::task_relations::label::LabelError::ForbiddenCharacter => {
                        "label contains a forbidden character"
                    }
                    crate::native::task_relations::label::LabelError::InvalidStart => {
                        "label can't start with a middle dot or comma"
                    }
                }
                .to_string()),
            ),
        };
        let key = crate::native::relation_labels::relation_label_key(&label);
        let existing = catalog.get(&key);
        let canonical_label =
            existing.map_or(label, |record| record.name.clone());
        let hidden =
            existing.is_some_and(|record| catalog.is_hidden(&record.key));
        let replacement = if disabled_reason.is_some() {
            String::new()
        } else if one_off {
            crate::native::task_relations::label::relation_label_hide_form(
                &canonical_label,
            )
        } else {
            crate::native::task_relations::label::relation_label_quoted_form(
                &canonical_label,
            )
        };
        let hide_replacement =
            crate::native::task_relations::label::relation_label_hide_form(
                &canonical_label,
            );
        let inverse = crate::native::task_relations::label::inverse_phrase(
            &canonical_label,
        );
        suggestions.retain(|item| item.key != key);
        suggestions.insert(
            0,
            RelationLabelSuggestion {
                replacement,
                hide_replacement,
                label: canonical_label,
                key: key.clone(),
                inverse,
                count: existing.map(|record| record.count as usize),
                typed: true,
                disabled_reason,
                existing: Some(existing.is_some()),
                hidden: Some(hidden),
                cancels_source: policy.is_cancel_label(&key),
                completes_source: policy.is_complete_label(&key),
                merges_source: policy.is_merge_label(&key),
                configured: false,
                offers_source: policy.is_successor_label(&key),
            },
        );
    }
    let mut warnings = catalog.warnings().to_vec();
    warnings.extend(policy.warnings.iter().cloned());
    (
        Candidates::RelationLabel(suggestions),
        warnings,
        catalog.hidden_count(),
    )
}

/// Zero-use configured cancel labels, in config order, ranked last.
/// Hidden labels stay out; the typed query still matches them exactly
/// through the typed row above.
fn append_cancel_overlay(
    suggestions: &mut Vec<RelationLabelSuggestion>,
    catalog: &crate::native::relation_labels::Catalog,
    normalized_query: &str,
    one_off: bool,
    policy: &config::RelationsConfig,
) {
    if policy.cancel_labels.is_empty() {
        return;
    }
    let query_key = crate::native::task_relations::label::collapse_label_text(
        normalized_query,
    )
    .to_lowercase();
    for cancel in &policy.cancel_labels {
        if suggestions.iter().any(|item| item.key == cancel.key) {
            continue;
        }
        if catalog.get(&cancel.key).is_some() {
            continue;
        }
        if catalog.is_hidden(&cancel.key) {
            continue;
        }
        if !query_key.is_empty()
            && !crate::native::task_relations::label::collapse_label_text(
                &cancel.name,
            )
            .to_lowercase()
            .contains(&query_key)
        {
            continue;
        }
        suggestions.push(RelationLabelSuggestion {
            replacement: if one_off {
                crate::native::task_relations::label::relation_label_hide_form(
                    &cancel.name,
                )
            } else {
                crate::native::task_relations::label::relation_label_quoted_form(
                    &cancel.name,
                )
            },
            hide_replacement:
                crate::native::task_relations::label::relation_label_hide_form(
                    &cancel.name,
                ),
            label: cancel.name.clone(),
            key: cancel.key.clone(),
            inverse:
                crate::native::task_relations::label::inverse_phrase(
                    &cancel.name,
                ),
            count: None,
            typed: false,
            disabled_reason: None,
            existing: None,
            hidden: None,
            cancels_source: true,
            completes_source: false,
            merges_source: false,
            configured: true,
            offers_source: false,
        });
    }
}

/// Zero-use configured complete labels, in config order, ranked last.
/// Hidden labels stay out; a typed query still matches them through the
/// typed row above.
fn append_complete_overlay(
    suggestions: &mut Vec<RelationLabelSuggestion>,
    catalog: &crate::native::relation_labels::Catalog,
    normalized_query: &str,
    one_off: bool,
    policy: &config::RelationsConfig,
) {
    if policy.complete_labels.is_empty() {
        return;
    }
    let query_key = crate::native::task_relations::label::collapse_label_text(
        normalized_query,
    )
    .to_lowercase();
    for complete in &policy.complete_labels {
        if suggestions.iter().any(|item| item.key == complete.key)
            || catalog.get(&complete.key).is_some()
            || catalog.is_hidden(&complete.key)
        {
            continue;
        }
        if !query_key.is_empty()
            && !crate::native::task_relations::label::collapse_label_text(
                &complete.name,
            )
            .to_lowercase()
            .contains(&query_key)
        {
            continue;
        }
        suggestions.push(RelationLabelSuggestion {
            replacement: if one_off {
                crate::native::task_relations::label::relation_label_hide_form(
                    &complete.name,
                )
            } else {
                crate::native::task_relations::label::relation_label_quoted_form(
                    &complete.name,
                )
            },
            hide_replacement:
                crate::native::task_relations::label::relation_label_hide_form(
                    &complete.name,
                ),
            label: complete.name.clone(),
            key: complete.key.clone(),
            inverse:
                crate::native::task_relations::label::inverse_phrase(
                    &complete.name,
                ),
            count: None,
            typed: false,
            disabled_reason: None,
            existing: None,
            hidden: None,
            cancels_source: false,
            completes_source: true,
            merges_source: false,
            configured: true,
            offers_source: false,
        });
    }
}

/// Zero-use configured merge labels, in config order, ranked last.
/// Hidden labels stay out; a typed query still matches through the typed row.
fn append_merge_overlay(
    suggestions: &mut Vec<RelationLabelSuggestion>,
    catalog: &crate::native::relation_labels::Catalog,
    normalized_query: &str,
    one_off: bool,
    policy: &config::RelationsConfig,
) {
    if policy.merge_labels.is_empty() {
        return;
    }
    let query_key = crate::native::task_relations::label::collapse_label_text(
        normalized_query,
    )
    .to_lowercase();
    for merge in &policy.merge_labels {
        if suggestions.iter().any(|item| item.key == merge.key)
            || catalog.get(&merge.key).is_some()
            || catalog.is_hidden(&merge.key)
        {
            continue;
        }
        if !query_key.is_empty()
            && !crate::native::task_relations::label::collapse_label_text(
                &merge.name,
            )
            .to_lowercase()
            .contains(&query_key)
        {
            continue;
        }
        suggestions.push(RelationLabelSuggestion {
            replacement: if one_off {
                crate::native::task_relations::label::relation_label_hide_form(
                    &merge.name,
                )
            } else {
                crate::native::task_relations::label::relation_label_quoted_form(
                    &merge.name,
                )
            },
            hide_replacement:
                crate::native::task_relations::label::relation_label_hide_form(
                    &merge.name,
                ),
            label: merge.name.clone(),
            key: merge.key.clone(),
            inverse:
                crate::native::task_relations::label::inverse_phrase(
                    &merge.name,
                ),
            count: None,
            typed: false,
            disabled_reason: None,
            existing: None,
            hidden: None,
            cancels_source: false,
            completes_source: false,
            merges_source: true,
            configured: true,
            offers_source: false,
        });
    }
}

/// Zero-use configured successor labels, in config order, ranked last.
/// Hidden labels stay out; a typed query still matches through the typed row.
fn append_successor_overlay(
    suggestions: &mut Vec<RelationLabelSuggestion>,
    catalog: &crate::native::relation_labels::Catalog,
    normalized_query: &str,
    one_off: bool,
    policy: &config::RelationsConfig,
) {
    if policy.successor_labels.is_empty() {
        return;
    }
    let query_key = crate::native::task_relations::label::collapse_label_text(
        normalized_query,
    )
    .to_lowercase();
    for successor in &policy.successor_labels {
        if suggestions.iter().any(|item| item.key == successor.key)
            || catalog.get(&successor.key).is_some()
            || catalog.is_hidden(&successor.key)
        {
            continue;
        }
        if !query_key.is_empty()
            && !crate::native::task_relations::label::collapse_label_text(
                &successor.name,
            )
            .to_lowercase()
            .contains(&query_key)
        {
            continue;
        }
        suggestions.push(RelationLabelSuggestion {
            replacement: if one_off {
                crate::native::task_relations::label::relation_label_hide_form(
                    &successor.name,
                )
            } else {
                crate::native::task_relations::label::relation_label_quoted_form(
                    &successor.name,
                )
            },
            hide_replacement:
                crate::native::task_relations::label::relation_label_hide_form(
                    &successor.name,
                ),
            label: successor.name.clone(),
            key: successor.key.clone(),
            inverse:
                crate::native::task_relations::label::inverse_phrase(
                    &successor.name,
                ),
            count: None,
            typed: false,
            disabled_reason: None,
            existing: None,
            hidden: None,
            cancels_source: false,
            completes_source: false,
            merges_source: false,
            configured: true,
            offers_source: true,
        });
    }
}

fn is_relation_source_read_only(
    bob_dir: &Path,
    task: &capture_dependency_tasks::DependencyTask,
) -> bool {
    let path = Path::new(&task.note_path);
    crate::native::done_archive::is_archive_path(path)
        || task_status_hooks::canonical_daily_date(path).is_some()
        || fs::metadata(bob_dir.join(path))
            .is_ok_and(|metadata| metadata.permissions().readonly())
}

fn is_editable_open_task(
    task: &capture_dependency_tasks::DependencyTask,
) -> bool {
    if !task.open || !task_complete::is_completable_status(task.status_symbol) {
        return false;
    }
    let path = Path::new(&task.note_path);
    if task_status_hooks::canonical_daily_date(path).is_some() {
        return false;
    }
    !crate::native::done_archive::is_archive_path(path)
}

fn blocked_task_group(
    task: &capture_dependency_tasks::DependencyTask,
    ledger: &capture_active_tasks::Ledger,
    owner_note: &str,
) -> &'static str {
    if task_has_today_link(task, ledger) {
        "today"
    } else if task.status_symbol == '/' {
        "in_progress"
    } else if task.status_symbol == '*' {
        "next"
    } else if task.note_path == owner_note {
        "in_this_note"
    } else {
        "open"
    }
}

fn blocked_task_order_key(
    task: &capture_dependency_tasks::DependencyTask,
    ledger: &capture_active_tasks::Ledger,
    owner_note: &str,
) -> (u8, usize, usize) {
    if let Some((entry, link)) = task_ledger_position(task, ledger) {
        let running = task
            .block_id
            .as_ref()
            .and_then(|id| {
                ledger
                    .owners
                    .get(&(task_route(&task.note_path), id.clone()))
            })
            .is_some_and(|pomodoro| pomodoro.is_current);
        (if running { 0 } else { 1 }, entry, link)
    } else if task.status_symbol == '/' {
        (2, 0, 0)
    } else if task.status_symbol == '*' {
        (3, 0, 0)
    } else if task.note_path == owner_note {
        (4, 0, 0)
    } else {
        (5, 0, 0)
    }
}

fn task_has_today_link(
    task: &capture_dependency_tasks::DependencyTask,
    ledger: &capture_active_tasks::Ledger,
) -> bool {
    task_ledger_position(task, ledger).is_some()
}

fn task_ledger_position(
    task: &capture_dependency_tasks::DependencyTask,
    ledger: &capture_active_tasks::Ledger,
) -> Option<(usize, usize)> {
    let id = task.block_id.as_ref()?;
    ledger.position(&(task_route(&task.note_path), id.clone()))
}

fn task_route(note_path: &str) -> String {
    let mut path = PathBuf::from(note_path);
    path.set_extension("");
    path.to_string_lossy().replace('\\', "/")
}

fn dependency_reachable(
    graph: &HashMap<
        capture_dependency_tasks::TaskIdentity,
        HashSet<capture_dependency_tasks::TaskIdentity>,
    >,
    roots: &[capture_dependency_tasks::TaskIdentity],
) -> HashSet<capture_dependency_tasks::TaskIdentity> {
    let mut reachable = HashSet::new();
    let mut pending = roots.to_vec();
    while let Some(task) = pending.pop() {
        if !reachable.insert(task.clone()) {
            continue;
        }
        if let Some(prerequisites) = graph.get(&task) {
            pending.extend(prerequisites.iter().cloned());
        }
    }
    reachable
}

fn read_note_cached(
    bob_dir: &Path,
    path: &Path,
    cache: &mut HashMap<PathBuf, String>,
) -> Option<String> {
    if !cache.contains_key(path) {
        let contents = fs::read_to_string(bob_dir.join(path)).ok()?;
        cache.insert(path.to_path_buf(), contents);
    }
    cache.get(path).cloned()
}

fn is_recognized_project(
    relative: &Path,
    contents: &str,
    bob_dir: &Path,
    settings: &note_tasks::NoteTaskSettings,
) -> bool {
    let tasks = task_status_hooks::parse_tasks(contents, settings);
    project_dependencies::ProjectContext::for_note(
        &task_status_hooks::FileScan {
            path: bob_dir.join(relative),
            relative_path: relative.to_path_buf(),
            contents: contents.to_string(),
            tasks,
        },
    )
    .recognised
}

/// `task_complete` candidates for a whole-item `!` token: every open
/// task (Ready, Blocked, Next, In Progress) in the vault-wide
/// completable catalog, in today-first picker order (today rows by
/// Pomodoro role, then In Progress, Next, and other open tasks) and
/// ranked by the query with today matches on top. ID-less and guarded
/// (recurring, already-selected) rows carry an empty replacement the
/// client must never insert: ID-less rows resolve through the explicit
/// Add block ID flow via `capture-task-id`'s `complete_replacement`.
/// Read-only: selecting or highlighting a row writes nothing. A
/// missing day file means no today rows and no warning.
pub(super) fn task_complete_candidates(
    bob_dir: &Path,
    query: &str,
    raw_text: &str,
    cursor: usize,
) -> (Candidates, Vec<String>) {
    use std::path::Path as StdPath;
    let discovered = capture_dependency_tasks::discover(bob_dir);
    let day_file = pomodoro::day_file_for(bob_dir);
    let catalog =
        capture_completable_tasks::discover(bob_dir, &discovered, &day_file);
    let selected = capture_completable_tasks::draft_selected(
        &discovered,
        bob_dir,
        raw_text,
        cursor,
    );
    let ordered =
        capture_completable_tasks::order_for_picker(&catalog.tasks, query);
    let candidates = ordered
        .into_iter()
        .map(|task| {
            let requires_block_id = task.block_id.is_none();
            let already = task.block_id.as_ref().is_some_and(|_| {
                selected.contains(&(
                    task.note_path.clone(),
                    task.block_id.clone().unwrap_or_default(),
                ))
            });
            let disabled_reason = if task.recurring {
                Some(
                    capture_completable_tasks::RECURRING_DISABLED_REASON
                        .to_string(),
                )
            } else if already {
                Some(
                    capture_completable_tasks::ALREADY_SELECTED_DISABLED_REASON
                        .to_string(),
                )
            } else {
                None
            };
            let replacement = if requires_block_id || disabled_reason.is_some()
            {
                String::new()
            } else {
                capture_dependency_tasks::replacement_for_sigil(
                    &catalog.index,
                    StdPath::new(&task.note_path),
                    task.block_id.as_deref().expect("identified task"),
                    b'!',
                )
            };
            TaskCompleteCandidate {
                replacement,
                task_ref: task.task_ref.clone(),
                note_path: task.note_path.clone(),
                locator: task.locator.clone(),
                group: task.group,
                hidden: task.hidden,
                block_id: task.block_id.clone(),
                requires_block_id,
                block_id_suggestions: task.block_id_suggestions.clone(),
                recurring: task.recurring,
                already_selected: already,
                disabled_reason,
                status_symbol: task.status_symbol,
                status_name: task.status_name.clone(),
                status_type: capture_tasks::status_type_label(task.status_type),
                text: task.text.clone(),
                section: task.section.clone(),
                depth: task.depth,
                line: task.line,
                scheduled: task.scheduled.clone(),
                today: task.today.clone(),
                task_kind: task.task_kind,
            }
        })
        .collect();
    (Candidates::TaskComplete(candidates), catalog.warnings)
}

pub(super) fn task_search_fields(
    task: &note_tasks::NoteTask,
    search: TaskSearch,
) -> Vec<String> {
    match search {
        TaskSearch::BlockIdOnly => task.block_id.iter().cloned().collect(),
        TaskSearch::MultiField => {
            let mut fields = Vec::new();
            if let Some(block_id) = &task.block_id {
                fields.push(block_id.clone());
            }
            fields.push(task.description.clone());
            if let Some(section) = &task.section {
                fields.push(section.clone());
            }
            fields.push(task.status_name.clone());
            fields.push(task.status_symbol.to_string());
            fields
        }
    }
}

/// Read one routed note's contents; a missing note is not an error, exactly
/// like `capture-sections` and `capture-tasks`.
pub(super) fn read_target(
    bob_dir: &Path,
    route: &str,
) -> Result<String, CompleteError> {
    let target = bob_dir.join(capture::route_label(route));
    match fs::read_to_string(&target) {
        Ok(contents) => Ok(contents),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            Ok(String::new())
        }
        Err(error) => Err(CompleteError::io(format!(
            "read target {}: {error}",
            target.display()
        ))),
    }
}

#[cfg(test)]
mod closed_goto_tests {
    use super::*;
    use crate::native::note_tasks::TaskStatusType;

    fn closed_task(
        note_path: &str,
        line: usize,
        closed_on: Option<&str>,
    ) -> capture_dependency_tasks::DependencyTask {
        capture_dependency_tasks::DependencyTask {
            note_path: note_path.to_string(),
            locator: note_path
                .strip_suffix(".md")
                .unwrap_or(note_path)
                .to_string(),
            block_id: None,
            block_id_suggestions: Vec::new(),
            status_symbol: 'x',
            status_name: "Done".to_string(),
            status_type: TaskStatusType::Done,
            text: "closed".to_string(),
            section: None,
            depth: 0,
            line,
            task_ref: format!("{line}:deadbeef"),
            hidden: false,
            open: false,
            duplicate_id: false,
            task_kind: None,
            closed_on: closed_on.map(str::to_string),
        }
    }

    #[test]
    fn dated_rows_come_before_undated_newest_first() {
        let newest = closed_task("b.md", 1, Some("2026-10-03"));
        let older = closed_task("a.md", 1, Some("2026-09-20"));
        let undated = closed_task("a.md", 2, None);
        assert!(
            closed_goto_order_key(&newest) < closed_goto_order_key(&older),
            "newest date should sort first"
        );
        assert!(
            closed_goto_order_key(&older) < closed_goto_order_key(&undated),
            "dated rows should sort before undated rows"
        );
    }

    #[test]
    fn ties_break_by_note_path_then_line() {
        let first = closed_task("a.md", 1, Some("2026-09-20"));
        let second = closed_task("b.md", 1, Some("2026-09-20"));
        let later_line = closed_task("a.md", 2, Some("2026-09-20"));
        assert!(closed_goto_order_key(&first) < closed_goto_order_key(&second));
        assert!(
            closed_goto_order_key(&first) < closed_goto_order_key(&later_line)
        );
    }
}
