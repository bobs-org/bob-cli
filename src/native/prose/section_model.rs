//! Pure selection and eligibility for prose-section extraction.
//!
//! This module deliberately accepts a resolver for prose-child stubs instead
//! of reading a vault. The caller supplies the same-note/vault snapshot fact;
//! every other decision is derived from the note bytes and path here.

use std::{
    collections::BTreeMap,
    ops::Range,
    path::{Component, Path, PathBuf},
};

use super::{
    marker,
    scan::{self, ClassifiedLine, Document, InlineMaskMode, Region},
    sections::{
        heading_identity, heading_plain_identity, normalize_identity,
        strip_section_number,
    },
};
use crate::native::{
    albums::index::records_spec,
    capture_drawers::{drawer_key, is_structural_h2_key},
    host_catalog::{owned_regions, RegionSpec, GENERATED_CATALOGS},
    intentions::subject::NoteSubject,
    is_always_excluded_note_directory_name,
    markdown::{atx_heading, setext_underline},
    projects::{
        contains_task_tag, frontmatter_is_area, frontmatter_is_project,
        frontmatter_is_prose, frontmatter_type_names, is_prose_type_name,
        parse_frontmatter,
    },
    task_block_layout::NOTES_SECTION_TITLE,
    task_status_groups::{is_badge_row, reserved_group_heading_lines},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum HostKind {
    Area,
    Project,
    Prose,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Selector<'a> {
    Section(&'a str),
    /// 1-based physical line number, as accepted by `--line`.
    Line(usize),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SectionRefusal {
    pub(crate) code: String,
    /// 1-based line number when one identifies the refusal.
    pub(crate) line: Option<usize>,
    pub(crate) message: String,
    pub(crate) hint: String,
    pub(crate) next: Option<NextAction>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct NextAction {
    pub(crate) command: &'static str,
    pub(crate) note: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ProseSection<'a> {
    pub(crate) path: PathBuf,
    pub(crate) text: &'a str,
    pub(crate) host_kind: HostKind,
    /// 1-based line of the kept heading.
    pub(crate) heading_line: usize,
    pub(crate) heading_level: usize,
    pub(crate) raw_title: String,
    pub(crate) title: String,
    /// Identity of this heading; use `identity_path` for a nested selector.
    pub(crate) identity: String,
    pub(crate) identity_path: String,
    /// 0-based line range containing everything after this heading, exclusive
    /// of its ATX line or Setext underline, up to the next same/shallower
    /// heading. The range end is exclusive.
    pub(crate) body_range: Range<usize>,
    pub(crate) leading_seam: Range<usize>,
    pub(crate) trailing_seam: Range<usize>,
    pub(crate) moved_headings: Vec<MovedHeading>,
    pub(crate) moved_block_ids: Vec<MovedBlockId>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct MovedHeading {
    pub(crate) line: usize,
    pub(crate) original_level: usize,
    pub(crate) raw_title: String,
    pub(crate) title: String,
    pub(crate) identity: String,
    pub(crate) setext: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct MovedBlockId {
    pub(crate) line: usize,
    pub(crate) id: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct StubTarget {
    pub(crate) line: usize,
    /// The complete wikilink, including its leading `!` when embedded.
    pub(crate) link: String,
    pub(crate) embedded: bool,
    /// Link destination without a fragment or display alias.
    pub(crate) target: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DefinitionIndex {
    /// Case-folded CommonMark/footnote labels and their 1-based definition
    /// lines. A label can have multiple authored definitions.
    pub(crate) definitions: BTreeMap<String, Vec<usize>>,
    /// Case-folded labels and their 1-based reference lines.
    pub(crate) uses: BTreeMap<String, Vec<usize>>,
}

#[derive(Debug, Clone)]
struct Heading {
    line: usize,
    body_start: usize,
    level: usize,
    raw_title: String,
    title: String,
    setext: bool,
    parent: Option<usize>,
    end: usize,
}

/// Validate the CLI selector cardinality before calling [`select_section`].
/// Host classification still runs first so an ineligible note never falls
/// through to a different interpretation.
pub(crate) fn select_section_with_options<'a, F>(
    path: &Path,
    text: &'a str,
    section: Option<&str>,
    line: Option<usize>,
    resolve_child_prose_stub: F,
) -> Result<ProseSection<'a>, SectionRefusal>
where
    F: FnMut(&StubTarget) -> Option<PathBuf>,
{
    let document = scan::classify(text);
    let host_kind = classify_host(path, text)?;
    let selector = match (section, line) {
        (Some(section), None) => Selector::Section(section),
        (None, Some(line)) => Selector::Line(line),
        (None, None) => {
            return Err(refusal(
                "selector-required",
                None,
                "choose a section identity or a line number",
                "Pass exactly one of `--section` or `--line`",
            ));
        }
        (Some(_), Some(_)) => {
            return Err(refusal(
                "selector-conflict",
                None,
                "choose one section selector",
                "Pass `--section` or `--line`, not both",
            ));
        }
    };
    select_classified(
        path,
        text,
        host_kind,
        document,
        selector,
        resolve_child_prose_stub,
    )
}

/// Classify one already-parsed selector. The resolver returns the target note
/// path only when the stub names a prose note whose `parent` is this host.
pub(crate) fn select_section<'a, F>(
    path: &Path,
    text: &'a str,
    selector: Selector<'_>,
    resolve_child_prose_stub: F,
) -> Result<ProseSection<'a>, SectionRefusal>
where
    F: FnMut(&StubTarget) -> Option<PathBuf>,
{
    let document = scan::classify(text);
    let host_kind = classify_host(path, text)?;
    select_classified(
        path,
        text,
        host_kind,
        document,
        selector,
        resolve_child_prose_stub,
    )
}

/// Return every heading section in an eligible prose host. Conversion's
/// inverse uses this to find the one section whose body is a stub for a
/// particular prose note; ordinary extraction still goes through the full
/// selector and eligibility checks above.
pub(crate) fn all_sections<'a>(
    path: &Path,
    text: &'a str,
) -> Result<Vec<ProseSection<'a>>, SectionRefusal> {
    let document = scan::classify(text);
    let host_kind = classify_host(path, text)?;
    let headings = collect_headings(&document);
    Ok((0..headings.len())
        .map(|index| {
            make_section(path, text, host_kind, index, &headings, &document)
        })
        .collect())
}

fn select_classified<'a, F>(
    path: &Path,
    text: &'a str,
    host_kind: HostKind,
    document: Document<'_>,
    selector: Selector<'_>,
    mut resolve_child_prose_stub: F,
) -> Result<ProseSection<'a>, SectionRefusal>
where
    F: FnMut(&StubTarget) -> Option<PathBuf>,
{
    let headings = collect_headings(&document);
    let selected_index = resolve_selector(selector, &headings)?;
    let selected = &headings[selected_index];
    let body_range = selected.body_start..selected.end;
    let section_range = selected.line..selected.end;
    let section = make_section(
        path,
        text,
        host_kind,
        selected_index,
        &headings,
        &document,
    );

    if let Some(line) = structural_line(
        host_kind,
        selected_index,
        section_range.clone(),
        &headings,
        &document.lines,
        text,
    ) {
        return Err(refusal(
            "structural-section",
            Some(line),
            "section contains Bob-owned structure",
            "Choose an authored section outside Tasks, generated catalogs, and Bob-owned blocks",
        ));
    }

    let subtree = &document.lines[section_range.clone()];
    if let Some(body_refusal) = marker::body_refusals(subtree) {
        let line = body_refusal_line(&body_refusal.code, subtree);
        let (message, hint) = match body_refusal.code.as_str() {
            "holds-tasks" => (
                "section contains a #task checkbox",
                "Keep task lines with their task owner before extracting",
            ),
            "holds-anchor" => (
                "section contains a ^prj or ^ref block anchor",
                "Keep the project or reference anchor with its owner",
            ),
            "conflict-markers" => (
                "section contains Git conflict markers",
                "Resolve the conflict markers before extracting",
            ),
            _ => ("section is ineligible", "Choose another section"),
        };
        return Err(refusal(body_refusal.code, line, message, hint));
    }

    if let Some(stub) = stub_target(&section)
        && let Some(note) = resolve_child_prose_stub(&stub)
    {
        let mut result = refusal(
            "stub-section",
            Some(section.heading_line),
            "section already links to its extracted prose note",
            "Inline the linked prose note to restore this section",
        );
        result.next = Some(NextAction {
            command: "inline",
            note,
        });
        return Err(result);
    }

    if let Some(open) = document.unclosed_fence
        && section_range.contains(&open)
    {
        return Err(refusal(
            "unclosed-fence",
            Some(open + 1),
            "a code fence opens in this section and never closes",
            "Close the fence before extracting the section",
        ));
    }

    if let Some((line, _)) =
        multiline_setext_in(&document, section_range.clone())
    {
        return Err(refusal(
            "setext-multiline",
            Some(line + 1),
            "a multi-line Setext heading lies inside this section",
            "Convert the Setext heading to ATX before extracting",
        ));
    }

    if matches!(host_kind, HostKind::Area | HostKind::Project)
        && let Some(heading) = section.moved_headings.iter().find(|heading| {
            strip_section_number(&heading.raw_title).token.is_some()
        })
    {
        return Err(refusal(
            "owned-number-heading",
            Some(heading.line),
            format!(
                "moved heading starts with Bob's section-number form: {:?}",
                heading.raw_title
            ),
            "Escape a literal leading number as `1\\.5`",
        ));
    }

    if let Some(crossing) = definition_boundary(text, &body_range) {
        return Err(crossing);
    }

    if let Some(heading) = section.moved_headings.iter().find(|heading| {
        promoted_level(heading.original_level, section.heading_level)
            .is_none_or(|level| !(2..=6).contains(&level))
    }) {
        return Err(refusal(
            "depth-overflow",
            Some(heading.line),
            "a moved heading would be outside levels 2 through 6",
            "Flatten the heading levels before extracting",
        ));
    }

    Ok(section)
}

pub(crate) fn classify_host(
    path: &Path,
    text: &str,
) -> Result<HostKind, SectionRefusal> {
    if excluded_host_path(path) {
        return Err(not_host_note());
    }
    let Some(frontmatter) = parse_frontmatter(marker::strip_bom(text)) else {
        return Err(not_host_note());
    };
    let types = frontmatter_type_names(&frontmatter);
    if frontmatter_is_area(&frontmatter)
        && types.len() == 1
        && types[0] == "area"
    {
        return Ok(HostKind::Area);
    }
    if frontmatter_is_project(&frontmatter)
        && types.len() == 1
        && types[0] == "project"
    {
        return Ok(HostKind::Project);
    }
    if frontmatter_is_prose(&frontmatter)
        && !types.is_empty()
        && types.iter().all(|name| is_prose_type_name(name))
    {
        return Ok(HostKind::Prose);
    }
    Err(not_host_note())
}

fn excluded_host_path(path: &Path) -> bool {
    if path.components().any(|component| match component {
        Component::Normal(name) => is_always_excluded_note_directory_name(name),
        _ => false,
    }) {
        return true;
    }
    if path.components().any(|component| {
        let Component::Normal(name) = component else {
            return false;
        };
        name.to_string_lossy().eq_ignore_ascii_case("done")
            || name.to_string_lossy().eq_ignore_ascii_case("ref")
            || name.to_string_lossy().eq_ignore_ascii_case("refs")
    }) {
        return true;
    }
    path.to_str().and_then(NoteSubject::canonical).is_some()
}

fn collect_headings(document: &Document<'_>) -> Vec<Heading> {
    let mut headings = Vec::new();
    for (index, line) in document.lines.iter().enumerate() {
        match line.region {
            Region::AtxHeading => {
                let Some((level, title)) = atx_heading(line.content) else {
                    continue;
                };
                headings.push(new_heading(
                    index,
                    index + 1,
                    level,
                    title,
                    false,
                ));
            }
            Region::SetextUnderline => {
                let Some((start, count)) =
                    scan::setext_span(&document.lines, index)
                else {
                    continue;
                };
                if count != 1 {
                    continue;
                }
                let title =
                    document.lines[start].content.trim_end().trim_start();
                let level = setext_underline(line.content).unwrap_or(1);
                headings.push(new_heading(
                    start,
                    index + 1,
                    level,
                    title,
                    true,
                ));
            }
            _ => {}
        }
    }
    headings.sort_by_key(|heading| heading.line);
    for index in 0..headings.len() {
        headings[index].parent = (0..index)
            .rev()
            .find(|parent| headings[*parent].level < headings[index].level);
        headings[index].end = ((index + 1)..headings.len())
            .find(|next| headings[*next].level <= headings[index].level)
            .map_or(document.lines.len(), |next| headings[next].line);
    }
    headings
}

fn new_heading(
    line: usize,
    body_start: usize,
    level: usize,
    raw_title: &str,
    setext: bool,
) -> Heading {
    let title = strip_section_number(raw_title).rest.to_string();
    Heading {
        line,
        body_start,
        level,
        raw_title: raw_title.to_string(),
        title,
        setext,
        parent: None,
        end: usize::MAX,
    }
}

fn resolve_selector(
    selector: Selector<'_>,
    headings: &[Heading],
) -> Result<usize, SectionRefusal> {
    match selector {
        Selector::Line(line) => {
            let Some(index) = line.checked_sub(1) else {
                return Err(no_section(
                    None,
                    "the selected line is not inside a heading section",
                    "Move the cursor into a heading section",
                ));
            };
            let selected = headings
                .iter()
                .enumerate()
                .filter(|(_, heading)| {
                    heading.line <= index && index < heading.end
                })
                .max_by_key(|(_, heading)| (heading.level, heading.line))
                .map(|(heading, _)| heading);
            selected.ok_or_else(|| {
                let before_first_heading =
                    headings.first().is_none_or(|heading| index < heading.line);
                let hint = if before_first_heading {
                    "Type a heading above this text, then extract it"
                } else {
                    "Move the cursor into a heading section"
                };
                no_section(
                    Some(line),
                    "the selected line is not inside a heading section",
                    hint,
                )
            })
        }
        Selector::Section(identity) => {
            let segments: Vec<&str> = identity.split('#').collect();
            if identity.trim().is_empty()
                || segments.iter().any(|segment| segment.trim().is_empty())
            {
                return Err(no_section(
                    None,
                    format!("no section matches {identity:?}"),
                    "Pass a heading title or nested path such as `Parent#Child`",
                ));
            }
            let mut candidates: Vec<usize> = (0..headings.len()).collect();
            for (segment_index, segment) in segments.iter().enumerate() {
                candidates =
                    choose_heading_matches(candidates, segment, headings);
                if candidates.is_empty() {
                    return Err(no_section(
                        None,
                        close_match_message(identity, headings),
                        "Use a heading identity, or a nested path such as `Parent#Child`",
                    ));
                }
                if segment_index + 1 < segments.len() {
                    let parents = candidates;
                    candidates = parents
                        .iter()
                        .flat_map(|parent| {
                            let parent_heading = &headings[*parent];
                            headings
                                .iter()
                                .enumerate()
                                .filter(move |(_, heading)| {
                                    heading.line > parent_heading.line
                                        && heading.line < parent_heading.end
                                        && heading.level > parent_heading.level
                                })
                                .map(|(index, _)| index)
                        })
                        .collect();
                    candidates.sort_unstable();
                    candidates.dedup();
                }
            }
            match candidates.as_slice() {
                [only] => Ok(*only),
                [] => Err(no_section(
                    None,
                    close_match_message(identity, headings),
                    "Use a heading identity, or a nested path such as `Parent#Child`",
                )),
                many => {
                    let mut lines = many
                        .iter()
                        .map(|index| headings[*index].line + 1)
                        .collect::<Vec<_>>();
                    lines.sort_unstable();
                    let listed = lines
                        .iter()
                        .map(usize::to_string)
                        .collect::<Vec<_>>()
                        .join(", ");
                    Err(refusal(
                        "ambiguous-section",
                        lines.first().copied(),
                        format!("section identity {identity:?} matches lines {listed}"),
                        "Add a parent heading, or select the section by line",
                    ))
                }
            }
        }
    }
}

fn choose_heading_matches(
    candidates: Vec<usize>,
    segment: &str,
    headings: &[Heading],
) -> Vec<usize> {
    let wanted = normalize_identity(segment);
    let exact = candidates
        .iter()
        .copied()
        .filter(|index| {
            normalize_identity(&headings[*index].raw_title) == wanted
        })
        .collect::<Vec<_>>();
    if !exact.is_empty() {
        return exact;
    }
    candidates
        .into_iter()
        .filter(|index| {
            heading_identity(&headings[*index].raw_title)
                == heading_identity(segment)
                || heading_plain_identity(&headings[*index].raw_title)
                    == heading_plain_identity(segment)
        })
        .collect()
}

fn close_match_message(identity: &str, headings: &[Heading]) -> String {
    let query = identity.rsplit('#').next().unwrap_or(identity);
    let normalized = heading_identity(query);
    let query_len = normalized.chars().count();
    let threshold = (query_len / 3).max(2);
    let mut matches = headings
        .iter()
        .filter_map(|heading| {
            let distance = edit_distance(
                &normalized,
                &heading_identity(&heading.raw_title),
            );
            (distance <= threshold).then_some((
                distance,
                heading.line + 1,
                heading.raw_title.as_str(),
            ))
        })
        .collect::<Vec<_>>();
    matches.sort_by_key(|(distance, line, _)| (*distance, *line));
    matches.dedup_by_key(|(_, line, _)| *line);
    matches.truncate(5);
    if matches.is_empty() {
        format!("no section matches {identity:?}; no close matches were found")
    } else {
        let listed = matches
            .iter()
            .map(|(_, line, title)| format!("line {line} {title:?}"))
            .collect::<Vec<_>>()
            .join(", ");
        format!("no section matches {identity:?}; close matches: {listed}")
    }
}

fn edit_distance(left: &str, right: &str) -> usize {
    let left: Vec<char> = left.chars().collect();
    let right: Vec<char> = right.chars().collect();
    let mut previous: Vec<usize> = (0..=right.len()).collect();
    for (left_index, left_char) in left.iter().enumerate() {
        let mut current = vec![left_index + 1; right.len() + 1];
        for (right_index, right_char) in right.iter().enumerate() {
            current[right_index + 1] = (previous[right_index + 1] + 1)
                .min(current[right_index] + 1)
                .min(
                    previous[right_index]
                        + usize::from(left_char != right_char),
                );
        }
        previous = current;
    }
    previous[right.len()]
}

fn make_section<'a>(
    path: &Path,
    text: &'a str,
    host_kind: HostKind,
    selected_index: usize,
    headings: &[Heading],
    document: &Document<'_>,
) -> ProseSection<'a> {
    let heading = &headings[selected_index];
    let body_range = heading.body_start..heading.end;
    let mut leading_end = body_range.start;
    while leading_end < body_range.end
        && document.lines[leading_end].content.trim().is_empty()
    {
        leading_end += 1;
    }
    let mut trailing_start = body_range.end;
    while trailing_start > leading_end
        && document.lines[trailing_start - 1].content.trim().is_empty()
    {
        trailing_start -= 1;
    }
    let moved_headings = headings
        .iter()
        .filter(|moved| {
            moved.line >= body_range.start
                && moved.line < body_range.end
                && moved.level > heading.level
        })
        .map(|moved| MovedHeading {
            line: moved.line + 1,
            original_level: moved.level,
            raw_title: moved.raw_title.clone(),
            title: moved.title.clone(),
            identity: heading_identity(&moved.raw_title),
            setext: moved.setext,
        })
        .collect();
    let moved_block_ids = collect_block_ids(
        document.lines.get(body_range.clone()).unwrap_or_default(),
    );
    ProseSection {
        path: path.to_path_buf(),
        text,
        host_kind,
        heading_line: heading.line + 1,
        heading_level: heading.level,
        raw_title: heading.raw_title.clone(),
        title: heading.title.clone(),
        identity: heading_identity(&heading.raw_title),
        identity_path: heading_identity_path(selected_index, headings),
        body_range: body_range.clone(),
        leading_seam: body_range.start..leading_end,
        trailing_seam: trailing_start..body_range.end,
        moved_headings,
        moved_block_ids,
    }
}

fn heading_identity_path(index: usize, headings: &[Heading]) -> String {
    let mut chain = Vec::new();
    let mut current = Some(index);
    while let Some(found) = current {
        chain.push(heading_identity(&headings[found].raw_title));
        current = headings[found].parent;
    }
    chain.reverse();
    chain.join("#")
}

fn structural_line(
    host_kind: HostKind,
    selected_index: usize,
    range: Range<usize>,
    headings: &[Heading],
    lines: &[ClassifiedLine<'_>],
    text: &str,
) -> Option<usize> {
    let reserved_groups = reserved_group_heading_lines(text);
    let is_bob_heading = |heading: &Heading| {
        reserved_groups.contains(&heading.line)
            || (matches!(host_kind, HostKind::Area | HostKind::Project)
                && heading.level == 2
                && (is_structural_h2_key(&drawer_key(&heading.title))
                    || (host_kind == HostKind::Project
                        && heading_identity(&heading.title)
                            == heading_identity(NOTES_SECTION_TITLE))))
    };
    let mut structural = Vec::new();
    let mut ancestor = headings[selected_index].parent;
    while let Some(index) = ancestor {
        if is_bob_heading(&headings[index]) {
            structural.push(headings[index].line + 1);
        }
        ancestor = headings[index].parent;
    }
    for heading in headings.iter().filter(|heading| {
        heading.line >= range.start && heading.line < range.end
    }) {
        if is_bob_heading(heading) {
            structural.push(heading.line + 1);
        }
    }

    for spec in GENERATED_CATALOGS {
        collect_owned_region_lines(spec, &range, text, &mut structural);
    }
    let records = records_spec("");
    collect_owned_region_lines(&records, &range, text, &mut structural);

    for line in lines.get(range.clone()).unwrap_or_default() {
        if is_badge_row(line.content)
            || (line.region == Region::HtmlComment
                && line.content.contains("<!-- bob:"))
        {
            structural.push(line.index + 1);
        }
    }
    structural.into_iter().min()
}

fn collect_owned_region_lines(
    spec: &RegionSpec<'_>,
    selected: &Range<usize>,
    text: &str,
    structural: &mut Vec<usize>,
) {
    match owned_regions(spec, text) {
        Ok(regions) => {
            for region in regions {
                let start = region.start_line.saturating_sub(1);
                let end = region.end_line;
                if start < selected.end && end > selected.start {
                    structural.push(start.max(selected.start) + 1);
                }
            }
        }
        Err(damage) => {
            let line_index = damage.line.saturating_sub(1);
            if selected.contains(&line_index) {
                structural.push(damage.line);
            }
        }
    }
}

fn body_refusal_line(
    code: &str,
    lines: &[ClassifiedLine<'_>],
) -> Option<usize> {
    lines.iter().find_map(|line| {
        if line.region.protects_body() {
            return None;
        }
        let masked = marker::mask_inline_spans(line.content);
        let found = match code {
            "conflict-markers" => {
                let trimmed = line.content.trim_end();
                trimmed.starts_with("<<<<<<<") || trimmed.starts_with(">>>>>>>")
            }
            "holds-anchor" => {
                contains_anchor(&masked, "^prj")
                    || contains_anchor(&masked, "^ref")
            }
            "holds-tasks" => {
                is_checkbox_line(line.content) && contains_task_tag(&masked)
            }
            _ => false,
        };
        found.then_some(line.index + 1)
    })
}

fn contains_anchor(text: &str, token: &str) -> bool {
    let mut offset = 0;
    while let Some(relative) = text[offset..].find(token) {
        let start = offset + relative;
        let end = start + token.len();
        let before = start == 0
            || text[..start]
                .chars()
                .next_back()
                .is_some_and(char::is_whitespace);
        let after = end == text.len()
            || text[end..].chars().next().is_some_and(char::is_whitespace);
        if before && after {
            return true;
        }
        offset = start + 1;
    }
    false
}

fn is_checkbox_line(line: &str) -> bool {
    let line = line.trim_start();
    let bytes = line.as_bytes();
    let marker_end = if bytes
        .first()
        .is_some_and(|byte| matches!(byte, b'-' | b'+' | b'*'))
        && bytes.get(1).is_some_and(u8::is_ascii_whitespace)
    {
        2
    } else {
        let digits = bytes
            .iter()
            .take_while(|byte| byte.is_ascii_digit())
            .count();
        if digits > 0
            && bytes
                .get(digits)
                .is_some_and(|byte| matches!(byte, b'.' | b')'))
            && bytes.get(digits + 1).is_some_and(u8::is_ascii_whitespace)
        {
            digits + 2
        } else {
            return false;
        }
    };
    let body = line[marker_end..].trim_start();
    let bytes = body.as_bytes();
    bytes.len() >= 3
        && bytes[0] == b'['
        && matches!(bytes[1], b' ' | b'x' | b'X' | b'-')
        && bytes[2] == b']'
        && bytes.get(3).is_none_or(u8::is_ascii_whitespace)
}

pub(super) fn collect_block_ids(
    lines: &[ClassifiedLine<'_>],
) -> Vec<MovedBlockId> {
    let mut ids = Vec::new();
    for line in lines {
        if line.region.protects_body() || line.region == Region::HtmlBlock {
            continue;
        }
        let masked = marker::mask_inline_spans(line.content);
        let trimmed_end = masked.trim_end();
        let Some(caret) = trimmed_end.rfind(" ^") else {
            continue;
        };
        let id = &trimmed_end[caret + 2..];
        if !id.is_empty()
            && id.bytes().all(|byte| {
                byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-')
            })
        {
            ids.push(MovedBlockId {
                line: line.index + 1,
                id: id.to_string(),
            });
        }
    }
    ids
}

/// Return a syntactic single-link stub. The caller resolves `target` against
/// its vault snapshot and checks that the target is a child prose note.
pub(crate) fn stub_target(section: &ProseSection<'_>) -> Option<StubTarget> {
    let physical = scan::split_physical_lines(marker::strip_bom(section.text));
    let nonblank = section
        .body_range
        .clone()
        .filter_map(|index| {
            physical.get(index).map(|(content, _)| (index, *content))
        })
        .filter(|(_, content)| !content.trim().is_empty())
        .collect::<Vec<_>>();
    let [(index, content)] = nonblank.as_slice() else {
        return None;
    };
    let indent = content.bytes().take_while(|byte| *byte == b' ').count();
    if indent > 3 {
        return None;
    }
    let bullet = &content[indent..];
    let rest = bullet
        .strip_prefix("- ")
        .or_else(|| bullet.strip_prefix("* "))
        .or_else(|| bullet.strip_prefix("+ "))?;
    let (embedded, link) = if let Some(link) = rest
        .strip_prefix("![[")
        .and_then(|value| value.strip_suffix("]]"))
    {
        (true, link)
    } else if let Some(link) = rest
        .strip_prefix("[[")
        .and_then(|value| value.strip_suffix("]]"))
    {
        (false, link)
    } else {
        return None;
    };
    if link.trim().is_empty() || link.contains("[[") || link.contains("]]") {
        return None;
    }
    let destination = link.split_once('|').map_or(link, |(target, _)| target);
    let target = destination
        .split_once('#')
        .map_or(destination, |(target, _)| target)
        .trim()
        .to_string();
    Some(StubTarget {
        line: *index + 1,
        link: if embedded {
            format!("![[{link}]]")
        } else {
            format!("[[{link}]]")
        },
        embedded,
        target,
    })
}

/// Scan document-global footnote and reference-link labels. Protected
/// Markdown regions and inline code/comments/math are ignored.
pub(crate) fn definition_index(text: &str) -> DefinitionIndex {
    let document = scan::classify(text);
    let mut index = DefinitionIndex {
        definitions: BTreeMap::new(),
        uses: BTreeMap::new(),
    };
    for line in &document.lines {
        if reference_scan_protected(line.region) {
            continue;
        }
        let mask = scan::inline_mask(line.content, InlineMaskMode::Marker);
        let definition = parse_definition(line.content, &mask);
        let mut ignored = vec![false; line.content.len()];
        if let Some((label, span, footnote)) = &definition {
            index
                .definitions
                .entry(normalize_label(label))
                .or_default()
                .push(line.index + 1);
            ignored[span.clone()].fill(true);
            if !footnote {
                ignored.fill(true);
            }
        }
        collect_reference_uses(line, &mask, &ignored, &mut index.uses);
    }
    index
}

fn reference_scan_protected(region: Region) -> bool {
    matches!(
        region,
        Region::Frontmatter
            | Region::Fence
            | Region::UnclosedFence
            | Region::Math
            | Region::PercentComment
            | Region::HtmlComment
            | Region::HtmlBlock
            | Region::IndentedCode
    )
}

fn parse_definition(
    line: &str,
    mask: &[bool],
) -> Option<(String, Range<usize>, bool)> {
    let bytes = line.as_bytes();
    let mut index = 0;
    for _ in 0..8 {
        while bytes.get(index).is_some_and(u8::is_ascii_whitespace) {
            index += 1;
        }
        if bytes.get(index) == Some(&b'>') {
            index += 1;
            continue;
        }
        if bytes
            .get(index)
            .is_some_and(|byte| matches!(byte, b'-' | b'+' | b'*'))
            && bytes.get(index + 1).is_some_and(u8::is_ascii_whitespace)
        {
            index += 2;
            continue;
        }
        let digits = bytes[index..]
            .iter()
            .take_while(|byte| byte.is_ascii_digit())
            .count();
        if digits > 0
            && bytes
                .get(index + digits)
                .is_some_and(|byte| matches!(byte, b'.' | b')'))
            && bytes
                .get(index + digits + 1)
                .is_some_and(u8::is_ascii_whitespace)
        {
            index += digits + 2;
            continue;
        }
        break;
    }
    if bytes.get(index) != Some(&b'[')
        || mask.get(index).copied().unwrap_or(false)
    {
        return None;
    }
    let close = find_bracket_close(bytes, mask, index + 1)?;
    if bytes.get(close + 1) != Some(&b':') {
        return None;
    }
    let label = line.get(index + 1..close)?.trim();
    if label.is_empty() {
        return None;
    }
    Some((label.to_string(), index..close + 1, label.starts_with('^')))
}

fn collect_reference_uses(
    line: &ClassifiedLine<'_>,
    mask: &[bool],
    ignored: &[bool],
    uses: &mut BTreeMap<String, Vec<usize>>,
) {
    let bytes = line.content.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] != b'['
            || mask.get(index).copied().unwrap_or(false)
            || ignored.get(index).copied().unwrap_or(false)
            || is_escaped(bytes, index)
        {
            index += 1;
            continue;
        }
        if bytes.get(index + 1) == Some(&b'[') {
            index = find_double_bracket_end(bytes, index + 2)
                .unwrap_or(bytes.len());
            continue;
        }
        let Some(first_end) = find_bracket_close(bytes, mask, index + 1) else {
            index += 1;
            continue;
        };
        let first = line.content[index + 1..first_end].trim();
        if first.is_empty() {
            index = first_end + 1;
            continue;
        }
        let footnote = first.starts_with('^');
        if bytes.get(first_end + 1) == Some(&b'(') {
            index = find_parenthesis_end(bytes, first_end + 2)
                .map_or(first_end + 2, |end| end + 1);
            continue;
        }
        if bytes.get(first_end + 1) == Some(&b'[') {
            let Some(second_end) =
                find_bracket_close(bytes, mask, first_end + 2)
            else {
                index = first_end + 1;
                continue;
            };
            let second = line.content[first_end + 2..second_end].trim();
            if !footnote {
                push_use(
                    uses,
                    if second.is_empty() { first } else { second },
                    line.index + 1,
                );
            }
            index = second_end + 1;
            continue;
        }
        if bytes.get(first_end + 1) == Some(&b':') {
            index = first_end + 1;
            continue;
        }
        push_use(uses, first, line.index + 1);
        index = first_end + 1;
    }
}

fn find_bracket_close(
    bytes: &[u8],
    mask: &[bool],
    start: usize,
) -> Option<usize> {
    let mut depth = 1_usize;
    let mut index = start;
    while index < bytes.len() {
        if mask.get(index).copied().unwrap_or(false) {
            index += 1;
            continue;
        }
        if bytes[index] == b'\\' {
            index = (index + 2).min(bytes.len());
            continue;
        }
        match bytes[index] {
            b'[' => depth += 1,
            b']' => {
                depth -= 1;
                if depth == 0 {
                    return Some(index);
                }
            }
            _ => {}
        }
        index += 1;
    }
    None
}

fn find_double_bracket_end(bytes: &[u8], start: usize) -> Option<usize> {
    bytes[start..]
        .windows(2)
        .position(|window| window == b"]]")
        .map(|relative| start + relative + 2)
}

fn find_parenthesis_end(bytes: &[u8], start: usize) -> Option<usize> {
    let mut depth = 1_usize;
    let mut index = start;
    while index < bytes.len() {
        if bytes[index] == b'\\' {
            index = (index + 2).min(bytes.len());
            continue;
        }
        match bytes[index] {
            b'(' => depth += 1,
            b')' => {
                depth -= 1;
                if depth == 0 {
                    return Some(index);
                }
            }
            _ => {}
        }
        index += 1;
    }
    None
}

fn is_escaped(bytes: &[u8], index: usize) -> bool {
    let mut start = index;
    while start > 0 && bytes[start - 1] == b'\\' {
        start -= 1;
    }
    (index - start) % 2 == 1
}

fn push_use(uses: &mut BTreeMap<String, Vec<usize>>, label: &str, line: usize) {
    let label = normalize_label(label);
    if !label.is_empty() {
        uses.entry(label).or_default().push(line);
    }
}

fn normalize_label(label: &str) -> String {
    label
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

fn definition_boundary(
    text: &str,
    body_range: &Range<usize>,
) -> Option<SectionRefusal> {
    let index = definition_index(text);
    let mut crossings = Vec::new();
    for (label, definitions) in &index.definitions {
        let Some(uses) = index.uses.get(label) else {
            continue;
        };
        let definitions_inside = definitions
            .iter()
            .copied()
            .filter(|line| body_range.contains(&(line - 1)))
            .collect::<Vec<_>>();
        let definitions_outside = definitions
            .iter()
            .copied()
            .filter(|line| !body_range.contains(&(line - 1)))
            .collect::<Vec<_>>();
        let uses_inside = uses
            .iter()
            .copied()
            .filter(|line| body_range.contains(&(line - 1)))
            .collect::<Vec<_>>();
        let uses_outside = uses
            .iter()
            .copied()
            .filter(|line| !body_range.contains(&(line - 1)))
            .collect::<Vec<_>>();
        if let Some(line) = definitions_inside.first()
            && !uses_outside.is_empty()
        {
            crossings.push((*line, label));
        }
        if let Some(line) = uses_inside.first()
            && !definitions_outside.is_empty()
        {
            crossings.push((*line, label));
        }
    }
    crossings.sort_by_key(|(line, _)| *line);
    crossings.first().map(|(line, label)| {
        refusal(
            "definition-boundary",
            Some(*line),
            format!("reference label {label:?} is used and defined across this section boundary"),
            "Move the label definition and all of its uses together",
        )
    })
}

fn multiline_setext_in(
    document: &Document<'_>,
    range: Range<usize>,
) -> Option<(usize, usize)> {
    document
        .lines
        .iter()
        .enumerate()
        .filter(|(index, _)| range.contains(index))
        .find_map(|(underline, line)| {
            if line.region != Region::SetextUnderline {
                return None;
            }
            let (start, count) = scan::setext_span(&document.lines, underline)?;
            (count > 1 && range.contains(&start)).then_some((start, underline))
        })
}

fn promoted_level(original: usize, selected: usize) -> Option<usize> {
    (original > selected)
        .then_some(original.saturating_sub(selected).saturating_add(1))
}

fn no_section(
    line: Option<usize>,
    message: impl Into<String>,
    hint: impl Into<String>,
) -> SectionRefusal {
    refusal("no-section", line, message, hint)
}

fn not_host_note() -> SectionRefusal {
    refusal(
        "not-host-note",
        None,
        "note is not an eligible area, project, or prose host",
        "Select an area, project, or prose note outside excluded, done, and reference folders",
    )
}

fn refusal(
    code: impl Into<String>,
    line: Option<usize>,
    message: impl Into<String>,
    hint: impl Into<String>,
) -> SectionRefusal {
    SectionRefusal {
        code: code.into(),
        line,
        message: message.into(),
        hint: hint.into(),
        next: None,
    }
}

#[cfg(test)]
#[path = "tests/section_model.rs"]
mod tests;
