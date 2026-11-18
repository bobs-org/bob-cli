//! Pure title-ordered record walks and first-body-line edits.
//!
//! A walk stores only the record links. Position and album size are derived
//! from the same ordered folder slice as the records index.

use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
};

use super::{
    grouping::{title_order, GroupingRecord},
    index::ALIAS_DROP,
    write::{folder_complete, overlay_folder_members},
    AlbumDeclaration,
};
use crate::native::{
    highlights_ref::humanize_stem,
    host_catalog::FileEdit,
    markdown::{closes_fence, fence_marker, split_line_ending, MarkdownFence},
    notebooks::catalog::inert_text,
    records::FolderIndex,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum WalkLineState {
    Current,
    Missing,
    Stale,
    Damaged,
}

impl WalkLineState {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Current => "current",
            Self::Missing => "missing",
            Self::Stale => "stale",
            Self::Damaged => "damaged",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum WalkDamageReason {
    ExtraText,
    DuplicateLines,
    NoFrontmatter,
}

impl WalkDamageReason {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::ExtraText => "extra_text",
            Self::DuplicateLines => "duplicate_lines",
            Self::NoFrontmatter => "no_frontmatter",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct WalkClassification {
    pub(crate) state: WalkLineState,
    pub(crate) reason: Option<WalkDamageReason>,
    line_index: Option<usize>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AlbumWalkState {
    Held,
    Empty,
    Current,
    Stale,
    Damaged,
}

impl AlbumWalkState {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Held => "held",
            Self::Empty => "empty",
            Self::Current => "current",
            Self::Stale => "stale",
            Self::Damaged => "damaged",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct WalkMember {
    pub(crate) path: String,
    pub(crate) stem: String,
    pub(crate) title: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct WalkSlot {
    pub(crate) member: WalkMember,
    pub(crate) position: usize,
    pub(crate) total: usize,
    pub(crate) prev: Option<WalkMember>,
    pub(crate) next: Option<WalkMember>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct WalkRecord {
    pub(crate) slot: WalkSlot,
    pub(crate) state: WalkLineState,
    pub(crate) reason: Option<WalkDamageReason>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct WalkWarning {
    pub(crate) path: String,
    pub(crate) reason: WalkDamageReason,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AlbumWalkPlan {
    pub(crate) state: AlbumWalkState,
    pub(crate) total: usize,
    /// Record stems in the same title order used by the records index.
    pub(crate) order: Vec<String>,
    pub(crate) records: Vec<WalkRecord>,
    pub(crate) edits: Vec<FileEdit>,
    pub(crate) warnings: Vec<WalkWarning>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum WalkScope {
    All,
    Radius { created: HashSet<String> },
}

/// Build every ring slot from a title-sorted member slice.
pub(crate) fn ring_slots(records: &[GroupingRecord]) -> Vec<WalkSlot> {
    let mut ordered = records.to_vec();
    ordered.sort_by(title_order);
    let members: Vec<WalkMember> =
        ordered.iter().map(member_from_group).collect();
    let total = members.len();
    members
        .iter()
        .enumerate()
        .map(|(index, member)| WalkSlot {
            member: member.clone(),
            position: index + 1,
            total,
            prev: (total > 1)
                .then(|| members[(index + total - 1) % total].clone()),
            next: (total > 1).then(|| members[(index + 1) % total].clone()),
        })
        .collect()
}

/// Sanitize a rendered title for use as a wikilink alias.
pub(crate) fn sanitize_walk_label(title: &str, stem: &str) -> String {
    let clean = sanitize_candidate(title);
    if !clean.is_empty() {
        return inert_text(&clean);
    }
    let humanized = sanitize_candidate(&humanize_stem(stem));
    if !humanized.is_empty() {
        return inert_text(&humanized);
    }
    inert_text(&sanitize_candidate(stem))
}

/// Render the canonical line for one slot. A singleton points only to its
/// album; two-member rings point both arms at the other member.
pub(crate) fn render_walk_line(
    declaration: &AlbumDeclaration,
    slot: &WalkSlot,
) -> String {
    let album_label =
        sanitize_walk_label(&declaration.title, &declaration.stem);
    let album = format!("[[{}|↑ {}]]", declaration.stem, album_label);
    match (&slot.prev, &slot.next) {
        (Some(prev), Some(next)) => format!(
            "[[{}/{}|‹ {}]] · {} · [[{}/{}|{} ›]]",
            declaration.stem,
            prev.stem,
            sanitize_walk_label(&prev.title, &prev.stem),
            album,
            declaration.stem,
            next.stem,
            sanitize_walk_label(&next.title, &next.stem),
        ),
        _ => album,
    }
}

/// Recognize the walk state in one record and retain the physical line index
/// for an in-place stale-line repair.
pub(crate) fn classify(
    markdown: &str,
    album_stem: &str,
    canonical: &str,
) -> WalkClassification {
    let lines = physical_lines(markdown);
    let frontmatter_end = frontmatter_close(&lines);
    let body_start = frontmatter_end.map_or(0, |index| index + 1);
    let mut fence: Option<MarkdownFence> = None;
    let mut walk_lines = Vec::new();
    let mut nav_like = false;

    for (index, line) in lines.iter().enumerate().skip(body_start) {
        if let Some(open) = fence {
            if closes_fence(line.content, open) {
                fence = None;
            }
            continue;
        }
        if let Some(open) = fence_marker(line.content) {
            fence = Some(open);
            continue;
        }
        if is_walk_line(line.content.trim(), album_stem) {
            walk_lines.push(index);
        } else if has_nav_like_link(line.content, album_stem) {
            nav_like = true;
        }
    }

    let damage = |reason| WalkClassification {
        state: WalkLineState::Damaged,
        reason: Some(reason),
        line_index: None,
    };
    if walk_lines.len() > 1 {
        return damage(WalkDamageReason::DuplicateLines);
    }
    if nav_like {
        return damage(WalkDamageReason::ExtraText);
    }
    let Some(_frontmatter_end) = frontmatter_end else {
        return damage(WalkDamageReason::NoFrontmatter);
    };
    match walk_lines.first().copied() {
        None => WalkClassification {
            state: WalkLineState::Missing,
            reason: None,
            line_index: None,
        },
        Some(index) => {
            let state = if lines[index].content == canonical {
                WalkLineState::Current
            } else {
                WalkLineState::Stale
            };
            WalkClassification {
                state,
                reason: None,
                line_index: Some(index),
            }
        }
    }
}

/// Plan only the walk-line byte edit. Damage is always left for a person to
/// resolve; a recognized moved line is rewritten where it currently sits.
pub(crate) fn plan_line_edit(
    markdown: &str,
    album_stem: &str,
    canonical: &str,
) -> Option<String> {
    let classification = classify(markdown, album_stem, canonical);
    match classification.state {
        WalkLineState::Current | WalkLineState::Damaged => None,
        WalkLineState::Missing => insert_walk_line(markdown, canonical),
        WalkLineState::Stale => {
            replace_walk_line(markdown, classification.line_index?, canonical)
        }
    }
}

/// Plan one album from a cached folder slice plus optional staged postimages.
/// `text_of` returns snapshot or disk bytes for members without an overlay.
pub(crate) fn plan_album_walk<F>(
    declaration: &AlbumDeclaration,
    folder: &FolderIndex,
    overlays: &[(PathBuf, String)],
    mut text_of: F,
    scope: WalkScope,
) -> AlbumWalkPlan
where
    F: FnMut(&str) -> Option<String>,
{
    let members = overlay_folder_members(folder, overlays);
    let total = members.len();
    if !folder_complete(folder) {
        return held_plan(total);
    }

    let overlay_text: HashMap<String, String> = overlays
        .iter()
        .filter_map(|(path, contents)| {
            normalized_path(path).map(|path| (path, contents.clone()))
        })
        .collect();
    let mut contents = HashMap::new();
    for member in &members {
        let path = normalized_path(&member.relative_path);
        let Some(path) = path else {
            return held_plan(total);
        };
        let text = overlay_text.get(&path).cloned().or_else(|| text_of(&path));
        let Some(text) = text else {
            return held_plan(total);
        };
        contents.insert(path, text);
    }

    let grouping_records: Vec<GroupingRecord> =
        members.iter().map(GroupingRecord::from_indexed).collect();
    let slots = ring_slots(&grouping_records);
    let order: Vec<String> =
        slots.iter().map(|slot| slot.member.stem.clone()).collect();
    if slots.is_empty() {
        return AlbumWalkPlan {
            state: AlbumWalkState::Empty,
            total: 0,
            order,
            records: Vec::new(),
            edits: Vec::new(),
            warnings: Vec::new(),
        };
    }

    let old_lines = match &scope {
        WalkScope::All => HashMap::new(),
        WalkScope::Radius { .. } => {
            let old_records: Vec<GroupingRecord> = folder
                .notes()
                .iter()
                .map(GroupingRecord::from_indexed)
                .collect();
            ring_slots(&old_records)
                .into_iter()
                .map(|slot| {
                    let line = render_walk_line(declaration, &slot);
                    (slot.member.path, line)
                })
                .collect()
        }
    };
    let created = match &scope {
        WalkScope::All => HashSet::new(),
        WalkScope::Radius { created } => {
            created.iter().map(|path| normalize_slashes(path)).collect()
        }
    };

    let mut records = Vec::with_capacity(slots.len());
    let mut edits = Vec::new();
    let mut warnings = Vec::new();
    for slot in slots {
        let canonical = render_walk_line(declaration, &slot);
        let path = slot.member.path.clone();
        let text = contents
            .get(&path)
            .expect("all member text is loaded before planning");
        let classification = classify(text, &declaration.stem, &canonical);
        let should_edit = match &scope {
            WalkScope::All => matches!(
                classification.state,
                WalkLineState::Missing | WalkLineState::Stale
            ),
            WalkScope::Radius { .. } => {
                created.contains(&path)
                    || old_lines.get(&path).is_some_and(|old| old != &canonical)
            }
        };
        if classification.state == WalkLineState::Damaged
            && matches!(scope, WalkScope::Radius { .. })
            && (created.contains(&path)
                || old_lines.get(&path).is_some_and(|old| old != &canonical))
        {
            warnings.push(WalkWarning {
                path: path.clone(),
                reason: classification
                    .reason
                    .expect("damaged classification has a reason"),
            });
        } else if should_edit
            && let Some(after) =
                plan_line_edit(text, &declaration.stem, &canonical)
            && after != *text
        {
            edits.push(FileEdit {
                path: PathBuf::from(&path),
                before: text.clone(),
                after,
            });
        }
        records.push(WalkRecord {
            slot,
            state: classification.state,
            reason: classification.reason,
        });
    }
    edits.sort_by(|left, right| left.path.cmp(&right.path));
    warnings.sort_by(|left, right| left.path.cmp(&right.path));
    let state = album_state(&records);
    AlbumWalkPlan {
        state,
        total,
        order,
        records,
        edits,
        warnings,
    }
}

fn held_plan(total: usize) -> AlbumWalkPlan {
    AlbumWalkPlan {
        state: AlbumWalkState::Held,
        total,
        order: Vec::new(),
        records: Vec::new(),
        edits: Vec::new(),
        warnings: Vec::new(),
    }
}

fn album_state(records: &[WalkRecord]) -> AlbumWalkState {
    if records.is_empty() {
        return AlbumWalkState::Empty;
    }
    if records
        .iter()
        .any(|record| record.state == WalkLineState::Damaged)
    {
        AlbumWalkState::Damaged
    } else if records.iter().any(|record| {
        matches!(record.state, WalkLineState::Missing | WalkLineState::Stale)
    }) {
        AlbumWalkState::Stale
    } else {
        AlbumWalkState::Current
    }
}

fn member_from_group(record: &GroupingRecord) -> WalkMember {
    WalkMember {
        path: record.path.clone(),
        stem: record.stem.clone(),
        title: record.title.clone(),
    }
}

fn sanitize_candidate(candidate: &str) -> String {
    let filtered: String = candidate
        .chars()
        .filter(|character| !ALIAS_DROP.contains(character))
        .collect();
    let mut trimmed = filtered.trim().to_owned();
    loop {
        let next = trimmed
            .trim_start_matches(['‹', '<'])
            .trim_end_matches(['›', '>'])
            .trim()
            .to_owned();
        if next == trimmed {
            return trimmed;
        }
        trimmed = next;
    }
}

#[derive(Debug, Clone, Copy)]
struct PhysicalLine<'a> {
    content: &'a str,
    ending: &'a str,
    start: usize,
    content_end: usize,
    end: usize,
}

fn physical_lines(markdown: &str) -> Vec<PhysicalLine<'_>> {
    let mut lines = Vec::new();
    let mut start = 0;
    for raw in markdown.split_inclusive('\n') {
        let (content, ending) = split_line_ending(raw);
        let content_end = start + content.len();
        let end = start + raw.len();
        lines.push(PhysicalLine {
            content,
            ending,
            start,
            content_end,
            end,
        });
        start = end;
    }
    if start < markdown.len() {
        let raw = &markdown[start..];
        let (content, ending) = split_line_ending(raw);
        let content_end = start + content.len();
        lines.push(PhysicalLine {
            content,
            ending,
            start,
            content_end,
            end: markdown.len(),
        });
    }
    lines
}

fn frontmatter_close(lines: &[PhysicalLine<'_>]) -> Option<usize> {
    let first_line = lines.first()?.content;
    let first = first_line.strip_prefix('\u{feff}').unwrap_or(first_line);
    if first.trim_end() != "---" {
        return None;
    }
    lines.iter().enumerate().skip(1).find_map(|(index, line)| {
        (line.content.trim_end() == "---").then_some(index)
    })
}

fn replace_walk_line(
    markdown: &str,
    line_index: usize,
    canonical: &str,
) -> Option<String> {
    let line = *physical_lines(markdown).get(line_index)?;
    let mut after = String::with_capacity(
        markdown.len() - line.content.len() + canonical.len(),
    );
    after.push_str(&markdown[..line.start]);
    after.push_str(canonical);
    after.push_str(line.ending);
    after.push_str(&markdown[line.end..]);
    Some(after)
}

fn insert_walk_line(markdown: &str, canonical: &str) -> Option<String> {
    let lines = physical_lines(markdown);
    let close_index = frontmatter_close(&lines)?;
    let close = lines[close_index];
    let newline = if close.ending.is_empty() {
        "\n"
    } else {
        close.ending
    };
    let first_content = lines
        .iter()
        .enumerate()
        .skip(close_index + 1)
        .find(|(_, line)| !line.content.trim().is_empty())
        .map(|(_, line)| line.start)
        .unwrap_or(markdown.len());
    let has_body = first_content < markdown.len();

    let mut after = String::with_capacity(markdown.len() + canonical.len() + 8);
    after.push_str(&markdown[..close.content_end]);
    after.push_str(newline);
    after.push_str(newline);
    after.push_str(canonical);
    after.push_str(newline);
    if has_body {
        after.push_str(newline);
        after.push_str(&markdown[first_content..]);
    }
    Some(after)
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ParsedLink<'a> {
    target: &'a str,
    label: &'a str,
}

fn parse_link_inner(inner: &str) -> Option<ParsedLink<'_>> {
    let mut pieces = inner.split('|');
    let target = pieces.next()?;
    let alias = pieces.next();
    if pieces.next().is_some() || target.is_empty() {
        return None;
    }
    let label =
        alias.unwrap_or_else(|| target.rsplit('/').next().unwrap_or(target));
    Some(ParsedLink { target, label })
}

fn parse_link_run(line: &str) -> Option<Vec<ParsedLink<'_>>> {
    let mut remaining = line.trim();
    let mut links = Vec::new();
    loop {
        let rest = remaining.strip_prefix("[[")?;
        let end = rest.find("]]")?;
        let inner = &rest[..end];
        if inner.contains("[[") {
            return None;
        }
        links.push(parse_link_inner(inner)?);
        if links.len() > 3 {
            return None;
        }
        remaining = rest[end + 2..].trim_start();
        if remaining.is_empty() {
            return Some(links);
        }
        remaining = remaining.strip_prefix('·')?.trim_start();
        if remaining.is_empty() {
            return None;
        }
    }
}

fn is_walk_line(line: &str, album_stem: &str) -> bool {
    let Some(links) = parse_link_run(line) else {
        return false;
    };
    let mut roots = 0;
    for link in &links {
        let Some(root) = allowed_target(link.target, album_stem) else {
            return false;
        };
        if root {
            roots += 1;
            if !link.label.starts_with('↑') {
                return false;
            }
        }
    }
    roots == 1
}

fn has_nav_like_link(line: &str, album_stem: &str) -> bool {
    let mut remaining = line;
    while let Some(open) = remaining.find("[[") {
        let after_open = &remaining[open + 2..];
        let Some(close) = after_open.find("]]") else {
            return false;
        };
        if let Some(link) = parse_link_inner(&after_open[..close])
            && allowed_target(link.target, album_stem) == Some(true)
            && link.label.starts_with('↑')
        {
            return true;
        }
        remaining = &after_open[close + 2..];
    }
    false
}

/// True when a body line is the album's walk line or carries a nav-like
/// `↑` link. Tidy's probes skip such lines when they look for the first
/// body content, so an H1 directly below the walk line is still redundant.
pub(crate) fn is_walk_or_nav_line(line: &str, album_stem: &str) -> bool {
    let trimmed = line.trim();
    is_walk_line(trimmed, album_stem) || has_nav_like_link(line, album_stem)
}

/// `Some(true)` means the album note; `Some(false)` means a direct child.
fn allowed_target(target: &str, album_stem: &str) -> Option<bool> {
    let target = target.strip_suffix(".md").unwrap_or(target);
    if target == album_stem {
        return Some(true);
    }
    let child = target.strip_prefix(album_stem)?.strip_prefix('/')?;
    (!child.is_empty() && !child.contains('/')).then_some(false)
}

fn insert_walk_line_for_test(
    markdown: &str,
    canonical: &str,
) -> Option<String> {
    insert_walk_line(markdown, canonical)
}

fn normalized_path(path: &Path) -> Option<String> {
    Some(normalize_slashes(path.to_str()?))
}

fn normalize_slashes(path: &str) -> String {
    path.replace('\\', "/")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::native::albums::{
        grouping::{group_records, GroupingRecord},
        parse_album_declaration,
    };
    use crate::native::records::RecordCatalog;
    use proptest::prelude::*;
    use serde_json::Value;
    use std::path::Path;

    const VECTORS: &str =
        include_str!("../../../tests/fixtures/record_walk/vectors.json");

    fn vectors() -> Value {
        serde_json::from_str(VECTORS).expect("record walk vectors")
    }

    fn records_from(value: &Value) -> Vec<GroupingRecord> {
        value
            .as_array()
            .expect("records array")
            .iter()
            .map(|record| {
                let stem = record["stem"].as_str().expect("stem").to_owned();
                let title = record["title"].as_str().expect("title");
                GroupingRecord {
                    path: record["path"].as_str().expect("path").to_owned(),
                    stem: stem.clone(),
                    title: if title.trim().is_empty() {
                        humanize_stem(&stem)
                    } else {
                        title.to_owned()
                    },
                    created: None,
                    fields: Vec::new(),
                }
            })
            .collect()
    }

    fn declaration(stem: &str, title: &str) -> AlbumDeclaration {
        let markdown = format!(
            "---\ntype: \"[[album]]\"\nparent: \"[[body]]\"\n---\n\n# {title}\n"
        );
        parse_album_declaration(Path::new(&format!("{stem}.md")), &markdown)
            .expect("album declaration")
    }

    #[test]
    fn parser_accepts_canonical_and_rejects_extra_text() {
        let canonical =
            "[[eat/pho|‹ Pho]] · [[eat|↑ Eat]] · [[eat/ramen|Ramen ›]]";
        assert!(is_walk_line(canonical, "eat"));
        assert!(!is_walk_line(&format!("{canonical} extra"), "eat"));
        assert!(has_nav_like_link(&format!("{canonical} extra"), "eat"));
    }

    #[test]
    fn trim_arrow_edges_before_markdown_escaping() {
        assert_eq!(
            sanitize_walk_label("‹  Ramen Ya ›", "ramen_ya"),
            "Ramen Ya"
        );
        assert_eq!(sanitize_walk_label("|[]#^", "the_taste"), "the taste");
        assert_eq!(sanitize_walk_label("  ‹›  ", "!!!"), "!!!");
    }

    #[test]
    fn insert_uses_frontmatter_newline_and_preserves_bom() {
        let source = "\u{feff}---\r\ntype: record\r\n---\r\n\r\n# Sheet\r\n";
        let expected =
            "\u{feff}---\r\ntype: record\r\n---\r\n\r\nWALK\r\n\r\n# Sheet\r\n";
        assert_eq!(
            insert_walk_line_for_test(source, "WALK").as_deref(),
            Some(expected)
        );
    }

    #[test]
    fn singleton_blank_body_gets_one_trailing_newline() {
        let source = "---\n\ntype: record\n---\n\n  \n";
        assert_eq!(
            insert_walk_line_for_test(source, "WALK").as_deref(),
            Some("---\n\ntype: record\n---\n\nWALK\n")
        );
    }

    #[test]
    fn shared_order_label_line_and_recognition_vectors() {
        let vectors = vectors();
        for case in vectors["order"].as_array().expect("order cases") {
            let records = records_from(&case["records"]);
            let actual: Vec<String> = ring_slots(&records)
                .into_iter()
                .map(|slot| slot.member.stem)
                .collect();
            let expected: Vec<String> = case["expected"]
                .as_array()
                .expect("expected order")
                .iter()
                .map(|stem| stem.as_str().unwrap().to_owned())
                .collect();
            assert_eq!(actual, expected, "{}", case["name"]);
        }

        for case in vectors["labels"].as_array().expect("label cases") {
            assert_eq!(
                sanitize_walk_label(
                    case["title"].as_str().unwrap(),
                    case["stem"].as_str().unwrap(),
                ),
                case["expected"].as_str().unwrap(),
                "{}",
                case["name"]
            );
        }

        for case in vectors["lines"].as_array().expect("line cases") {
            let album = declaration(
                case["album"]["stem"].as_str().unwrap(),
                case["album"]["title"].as_str().unwrap(),
            );
            let slots = ring_slots(&records_from(&case["members"]));
            let actual: Vec<String> = slots
                .iter()
                .map(|slot| render_walk_line(&album, slot))
                .collect();
            let expected: Vec<String> = case["expected"]
                .as_array()
                .unwrap()
                .iter()
                .map(|line| line.as_str().unwrap().to_owned())
                .collect();
            assert_eq!(actual, expected, "{}", case["name"]);
        }

        for case in vectors["recognition"]
            .as_array()
            .expect("recognition cases")
        {
            let actual = classify(
                case["markdown"].as_str().unwrap(),
                case["album"].as_str().unwrap(),
                case["canonical"].as_str().unwrap(),
            );
            assert_eq!(actual.state.as_str(), case["state"].as_str().unwrap());
            assert_eq!(
                actual.reason.map(WalkDamageReason::as_str),
                case["reason"].as_str()
            );
        }
    }

    #[test]
    fn moved_stale_walk_line_is_replaced_in_place_and_is_idempotent() {
        let stale = "[[eat|↑ Old Eat]]";
        let canonical =
            "[[eat/pho|‹ Pho]] · [[eat|↑ Eat]] · [[eat/ramen|Ramen ›]]";
        let markdown = format!(
            "---\ntype: record\n---\n\n# Authored heading\n\n{stale}\n\n## Notes\n"
        );
        let after = plan_line_edit(&markdown, "eat", canonical)
            .expect("stale line edit");
        assert!(after.contains(&format!(
            "# Authored heading\n\n{canonical}\n\n## Notes"
        )));
        assert_eq!(
            classify(&after, "eat", canonical).state,
            WalkLineState::Current
        );
        assert!(plan_line_edit(&after, "eat", canonical).is_none());
    }

    #[test]
    fn insertion_without_a_frontmatter_newline_uses_lf_and_preserves_prose() {
        assert_eq!(
            insert_walk_line_for_test("---\ntype: record\n---", "WALK")
                .as_deref(),
            Some("---\ntype: record\n---\n\nWALK\n")
        );
        assert_eq!(
            insert_walk_line_for_test(
                "---\ntype: record\n---\n\n prose\n",
                "WALK"
            )
            .as_deref(),
            Some("---\ntype: record\n---\n\nWALK\n\n prose\n")
        );
    }

    #[test]
    fn walk_matcher_mirror_keeps_the_hub_out_of_both_directions() {
        let line = "[[eat/pho|‹ Pho]] · [[eat|↑ Eat]] · [[eat/ramen|Ramen ›]]";
        let links = parse_link_run(line).expect("canonical links");
        let is_prev =
            |label: &str| label == "prev" || label.starts_with(['‹', '<']);
        let is_next = |label: &str| {
            label == "next"
                || label.starts_with(['›', '>'])
                || label.ends_with(['›', '>'])
        };
        assert!(is_prev(links[0].label));
        assert!(!is_next(links[0].label));
        assert!(!is_prev(links[1].label));
        assert!(!is_next(links[1].label));
        assert!(!is_prev(links[2].label));
        assert!(is_next(links[2].label));
    }

    #[test]
    fn incomplete_frontmatter_is_damaged_and_never_written() {
        let classification =
            classify("# No frontmatter\n", "eat", "[[eat|↑ Eat]]");
        assert_eq!(classification.state, WalkLineState::Damaged);
        assert_eq!(
            classification.reason,
            Some(WalkDamageReason::NoFrontmatter)
        );
        assert!(plan_line_edit("# No frontmatter\n", "eat", "[[eat|↑ Eat]]")
            .is_none());
    }

    proptest! {
        #[test]
        fn ring_is_a_cycle_and_matches_the_index_tally(
            titles in prop::collection::vec("[A-Za-z0-9 ]{1,16}", 1..64)
        ) {
            let records: Vec<GroupingRecord> = titles
                .iter()
                .enumerate()
                .map(|(index, title)| GroupingRecord {
                    path: format!("eat/{index:03}.md"),
                    stem: format!("{index:03}"),
                    title: title.clone(),
                    created: None,
                    fields: Vec::new(),
                })
                .collect();
            let slots = ring_slots(&records);
            let index_tally = group_records(&[], None, &[], &records).ungrouped.len();
            prop_assert_eq!(slots.len(), index_tally);
            let by_path: HashMap<String, &WalkSlot> = slots
                .iter()
                .map(|slot| (slot.member.path.clone(), slot))
                .collect();
            let mut positions: Vec<usize> = slots.iter().map(|slot| slot.position).collect();
            positions.sort_unstable();
            prop_assert_eq!(positions, (1..=slots.len()).collect::<Vec<_>>());
            for slot in &slots {
                if slots.len() == 1 {
                    prop_assert!(slot.prev.is_none() && slot.next.is_none());
                    continue;
                }
                let next = slot.next.as_ref().unwrap();
                let prev = slot.prev.as_ref().unwrap();
                prop_assert_eq!(
                    by_path[next.path.as_str()]
                        .prev
                        .as_ref()
                        .unwrap()
                        .path
                        .as_str(),
                    slot.member.path.as_str()
                );
                prop_assert_eq!(
                    by_path[prev.path.as_str()]
                        .next
                        .as_ref()
                        .unwrap()
                        .path
                        .as_str(),
                    slot.member.path.as_str()
                );
                let mut cursor = slot.member.path.clone();
                for _ in 0..slots.len() {
                    cursor = by_path[&cursor].next.as_ref().unwrap().path.clone();
                }
                prop_assert_eq!(cursor.as_str(), slot.member.path.as_str());
            }
        }
    }

    #[test]
    fn radius_in_an_unbackfilled_album_edits_only_new_record_and_changed_neighbours(
    ) {
        const ALBUM: &str = concat!(
            "---\n",
            "type: \"[[album]]\"\n",
            "parent: \"[[body]]\"\n",
            "record:\n",
            "  noun: item\n",
            "  label: Item\n",
            "---\n\n",
            "# Eat\n",
        );
        let temp = tempfile::tempdir().expect("tempdir");
        std::fs::create_dir_all(temp.path().join("eat")).expect("folder");
        std::fs::write(temp.path().join("eat.md"), ALBUM).expect("album");
        for index in 0..104 {
            let path = temp.path().join(format!("eat/record_{index:03}.md"));
            std::fs::write(
                path,
                format!(
                    "---\nparent: \"[[eat]]\"\ntype: \"[[item]]\"\ntitle: \"Record {index:03}\"\n---\n\n"
                ),
            )
            .expect("record");
        }
        let album = declaration("eat", "Eat");
        let mut catalog = RecordCatalog::new(temp.path());
        let folder = catalog.folder("eat");
        let path = PathBuf::from("eat/record_0505.md");
        let created =
            "---\nparent: \"[[eat]]\"\ntype: \"[[item]]\"\ntitle: \"Record 050.5\"\n---\n\n"
                .to_string();
        let overlay = vec![(path.clone(), created)];
        let created_paths = HashSet::from(["eat/record_0505.md".to_owned()]);
        let plan = plan_album_walk(
            &album,
            folder,
            &overlay,
            |relative| std::fs::read_to_string(temp.path().join(relative)).ok(),
            WalkScope::Radius {
                created: created_paths,
            },
        );
        assert_eq!(plan.total, 105);
        assert_eq!(plan.edits.len(), 3);
        assert!(plan.edits.iter().any(|edit| edit.path == path));
    }
}
