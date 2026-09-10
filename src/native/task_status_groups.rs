//! Pure Markdown transform that groups Tasks-section task blocks by status.
//!
//! This module does not read the filesystem, mutate task statuses, resolve
//! links, or write files. Classification is supplied by the caller.
//! Integration in a later phase is the first production caller; unit tests
//! exercise the API until then.
#![allow(dead_code)]

use std::collections::{BTreeMap, BTreeSet};
use std::ops::Range;

use super::markdown;

pub(crate) const TITLE_ACTIVE: &str = "Next & In Progress";
pub(crate) const TITLE_BLOCKED: &str = "Blocked";
pub(crate) const TITLE_CLOSED: &str = "Done & Canceled";

pub(crate) const MARKER_ACTIVE: &str =
    "<!-- bob:task-status-group:v1:active -->";
pub(crate) const MARKER_BLOCKED: &str =
    "<!-- bob:task-status-group:v1:blocked -->";
pub(crate) const MARKER_CLOSED: &str =
    "<!-- bob:task-status-group:v1:closed -->";

const MARKER_PREFIX: &str = "bob:task-status-group:v1:";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum StatusBucket {
    Ready,
    NextAndInProgress,
    Blocked,
    DoneAndCanceled,
    OtherOpen,
    Unknown,
}

impl StatusBucket {
    fn destination(self) -> Destination {
        match self {
            Self::NextAndInProgress => Destination::Active,
            Self::Blocked => Destination::Blocked,
            Self::DoneAndCanceled => Destination::Closed,
            Self::Ready | Self::OtherOpen | Self::Unknown => {
                Destination::Intake
            }
        }
    }

    fn is_groupable(self) -> bool {
        !matches!(self, Self::Ready | Self::OtherOpen | Self::Unknown)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Destination {
    Intake,
    Active,
    Blocked,
    Closed,
}

impl Destination {
    fn group(self) -> Option<GroupKind> {
        match self {
            Self::Active => Some(GroupKind::Active),
            Self::Blocked => Some(GroupKind::Blocked),
            Self::Closed => Some(GroupKind::Closed),
            Self::Intake => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum GroupKind {
    Active,
    Blocked,
    Closed,
}

impl GroupKind {
    const ALL: [Self; 3] = [Self::Active, Self::Blocked, Self::Closed];

    fn title(self) -> &'static str {
        match self {
            Self::Active => TITLE_ACTIVE,
            Self::Blocked => TITLE_BLOCKED,
            Self::Closed => TITLE_CLOSED,
        }
    }

    fn marker(self) -> &'static str {
        match self {
            Self::Active => MARKER_ACTIVE,
            Self::Blocked => MARKER_BLOCKED,
            Self::Closed => MARKER_CLOSED,
        }
    }

    fn from_title(title: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|kind| kind.title() == title)
    }

    fn from_marker_id(id: &str) -> Option<Self> {
        match id {
            "active" => Some(Self::Active),
            "blocked" => Some(Self::Blocked),
            "closed" => Some(Self::Closed),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum GroupingSkipCode {
    H6Container,
    AmbiguousBoundary,
    OwnershipCollision,
    MalformedOwnership,
    DuplicateGroupHeading,
    RenamedMarkedHeading,
    AuthoredHeadingInGroup,
    NestedUnderOrdinaryItem,
    UnsupportedOrderedList,
}

impl GroupingSkipCode {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::H6Container => "h6_container",
            Self::AmbiguousBoundary => "ambiguous_boundary",
            Self::OwnershipCollision => "ownership_collision",
            Self::MalformedOwnership => "malformed_ownership",
            Self::DuplicateGroupHeading => "duplicate_group_heading",
            Self::RenamedMarkedHeading => "renamed_marked_heading",
            Self::AuthoredHeadingInGroup => "authored_heading_in_group",
            Self::NestedUnderOrdinaryItem => "nested_under_ordinary_item",
            Self::UnsupportedOrderedList => "unsupported_ordered_list",
        }
    }

    fn fails_closed(self) -> bool {
        !matches!(
            self,
            Self::NestedUnderOrdinaryItem | Self::UnsupportedOrderedList
        )
    }

    fn message(self, title: &str, line: usize) -> String {
        match self {
            Self::H6Container => format!(
                "H6 heading {title:?} at line {line} cannot have child status groups"
            ),
            Self::AmbiguousBoundary => format!(
                "ambiguous Markdown boundary in {title:?} at line {line}; skipped grouping"
            ),
            Self::OwnershipCollision => format!(
                "unmarked status-group heading in {title:?} at line {line} contains authored context"
            ),
            Self::MalformedOwnership => format!(
                "malformed task-status-group marker in {title:?} at line {line}"
            ),
            Self::DuplicateGroupHeading => format!(
                "duplicate status-group heading in {title:?} at line {line}"
            ),
            Self::RenamedMarkedHeading => format!(
                "renamed marked status-group heading in {title:?} at line {line}"
            ),
            Self::AuthoredHeadingInGroup => format!(
                "authored child heading inside a managed status group in {title:?} at line {line}"
            ),
            Self::NestedUnderOrdinaryItem => format!(
                "task nested under an ordinary list item in {title:?} at line {line} is structurally ineligible"
            ),
            Self::UnsupportedOrderedList => format!(
                "ordered-list task root in {title:?} at line {line} is unsupported in v1"
            ),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct GroupedSection {
    pub original_heading_line: usize,
    pub heading_ancestry: Vec<String>,
    pub next_and_in_progress: usize,
    pub blocked: usize,
    pub done_and_canceled: usize,
    pub moved_block_count: usize,
    pub moved_blocks: Vec<MovedBlock>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct MovedBlock {
    pub original_line: usize,
    pub destination: DestinationLabel,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DestinationLabel {
    Intake,
    NextAndInProgress,
    Blocked,
    DoneAndCanceled,
}

impl Destination {
    fn label(self) -> DestinationLabel {
        match self {
            Self::Intake => DestinationLabel::Intake,
            Self::Active => DestinationLabel::NextAndInProgress,
            Self::Blocked => DestinationLabel::Blocked,
            Self::Closed => DestinationLabel::DoneAndCanceled,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct GroupingWarning {
    pub original_heading_line: usize,
    pub heading_ancestry: Vec<String>,
    pub code: GroupingSkipCode,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TransformOutput {
    pub contents: String,
    pub changed: bool,
    pub grouped_sections: Vec<GroupedSection>,
    pub warnings: Vec<GroupingWarning>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TaskClassification {
    global_filter: String,
    buckets: BTreeMap<char, StatusBucket>,
}

impl TaskClassification {
    pub(crate) fn new(global_filter: impl Into<String>) -> Self {
        Self {
            global_filter: global_filter.into(),
            buckets: BTreeMap::new(),
        }
    }

    pub(crate) fn with_symbol(
        mut self,
        symbol: char,
        bucket: StatusBucket,
    ) -> Self {
        self.buckets.insert(symbol, bucket);
        self
    }

    pub(crate) fn standard(global_filter: impl Into<String>) -> Self {
        Self::new(global_filter)
            .with_symbol(' ', StatusBucket::Ready)
            .with_symbol('*', StatusBucket::NextAndInProgress)
            .with_symbol('/', StatusBucket::NextAndInProgress)
            .with_symbol('?', StatusBucket::Blocked)
            .with_symbol('x', StatusBucket::DoneAndCanceled)
            .with_symbol('X', StatusBucket::DoneAndCanceled)
            .with_symbol('-', StatusBucket::DoneAndCanceled)
    }

    pub(crate) fn from_status_types<'a, I>(
        global_filter: impl Into<String>,
        status_types: I,
        next_symbol: char,
        wip_symbol: char,
        blocked_symbol: char,
        ready_symbol: char,
    ) -> Self
    where
        I: IntoIterator<Item = (char, &'a str)>,
    {
        let mut classification = Self::new(global_filter);
        for (symbol, status_type) in status_types {
            let bucket = if symbol == next_symbol || symbol == wip_symbol {
                StatusBucket::NextAndInProgress
            } else if symbol == blocked_symbol {
                StatusBucket::Blocked
            } else if symbol == ready_symbol {
                StatusBucket::Ready
            } else {
                match status_type {
                    "DONE" | "CANCELLED" => StatusBucket::DoneAndCanceled,
                    "NON_TASK" | "EMPTY" => StatusBucket::Unknown,
                    _ => StatusBucket::OtherOpen,
                }
            };
            classification.buckets.insert(symbol, bucket);
        }
        classification
            .buckets
            .entry('x')
            .or_insert(StatusBucket::DoneAndCanceled);
        classification
            .buckets
            .entry('X')
            .or_insert(StatusBucket::DoneAndCanceled);
        classification
            .buckets
            .entry(next_symbol)
            .or_insert(StatusBucket::NextAndInProgress);
        classification
            .buckets
            .entry(wip_symbol)
            .or_insert(StatusBucket::NextAndInProgress);
        classification
            .buckets
            .entry(blocked_symbol)
            .or_insert(StatusBucket::Blocked);
        classification
            .buckets
            .entry(ready_symbol)
            .or_insert(StatusBucket::Ready);
        classification
    }

    pub(crate) fn matches_filter(&self, body: &str) -> bool {
        self.global_filter.is_empty() || body.contains(&self.global_filter)
    }

    pub(crate) fn bucket(&self, symbol: char) -> StatusBucket {
        self.buckets
            .get(&symbol)
            .copied()
            .unwrap_or(StatusBucket::Unknown)
    }
}

pub(crate) fn transform(
    contents: &str,
    classification: &TaskClassification,
) -> TransformOutput {
    let lines = source_lines(contents);
    if lines.is_empty() {
        return TransformOutput {
            contents: contents.to_string(),
            changed: false,
            grouped_sections: Vec::new(),
            warnings: Vec::new(),
        };
    }

    let heading_mask = heading_mask(&lines);
    let flats = discover_headings(&lines, &heading_mask);
    let tree = build_tree(contents.len(), flats);

    let mut grouped_sections = Vec::new();
    let mut warnings = Vec::new();
    let mut replacements = Vec::new();
    let ctx = TransformCtx {
        contents,
        lines: &lines,
        mask: &heading_mask,
        classification,
    };
    visit_tasks_roots(
        &tree,
        &ctx,
        &mut grouped_sections,
        &mut warnings,
        &mut replacements,
    );

    let mut output = contents.to_string();
    replacements.sort_by_key(|(span, _)| span.start);
    for (span, replacement) in replacements.into_iter().rev() {
        output.replace_range(span, &replacement);
    }
    apply_final_newline(&mut output, contents);

    let changed = output != contents;
    TransformOutput {
        contents: output,
        changed,
        grouped_sections,
        warnings,
    }
}

struct TransformCtx<'a> {
    contents: &'a str,
    lines: &'a [SourceLine<'a>],
    mask: &'a BTreeSet<usize>,
    classification: &'a TaskClassification,
}

fn visit_tasks_roots(
    nodes: &[HeadingNode],
    ctx: &TransformCtx<'_>,
    grouped_sections: &mut Vec<GroupedSection>,
    warnings: &mut Vec<GroupingWarning>,
    replacements: &mut Vec<(Range<usize>, String)>,
) {
    for node in nodes {
        if is_tasks_title(&node.title) {
            let rewritten = rewrite_container(node, ctx);
            grouped_sections.extend(rewritten.grouped_sections);
            warnings.extend(rewritten.warnings);
            if rewritten.section != ctx.contents[node.section_span.clone()] {
                replacements
                    .push((node.section_span.clone(), rewritten.section));
            }
        } else {
            visit_tasks_roots(
                &node.children,
                ctx,
                grouped_sections,
                warnings,
                replacements,
            );
        }
    }
}

struct ContainerRewrite {
    section: String,
    grouped_sections: Vec<GroupedSection>,
    warnings: Vec<GroupingWarning>,
}

fn rewrite_container(
    node: &HeadingNode,
    ctx: &TransformCtx<'_>,
) -> ContainerRewrite {
    let original = ctx.contents[node.section_span.clone()].to_string();
    let heading_raw = &ctx.contents[node.heading_span.clone()];

    if node.level >= 6 {
        return ContainerRewrite {
            section: original,
            grouped_sections: Vec::new(),
            warnings: vec![warning(node, GroupingSkipCode::H6Container)],
        };
    }

    let classified = classify_children(node, ctx);
    let mut warnings = classified.warnings.clone();
    let mut grouped_sections = Vec::new();

    let mut child_rewrites = BTreeMap::new();
    for child in &node.children {
        if classified.is_grouping_child(child) {
            let rewritten = rewrite_container(child, ctx);
            grouped_sections.extend(rewritten.grouped_sections.iter().cloned());
            warnings.extend(rewritten.warnings.iter().cloned());
            child_rewrites.insert(child.heading_span.start, rewritten);
        }
    }

    if let Some(code) = classified.fail {
        let mut section = original;
        apply_child_rewrites(&mut section, node, &child_rewrites);
        warnings.insert(0, warning(node, code));
        return ContainerRewrite {
            section,
            grouped_sections,
            warnings,
        };
    }

    let parsed = match parse_container_regions(node, ctx, &classified) {
        Ok(parsed) => parsed,
        Err(code) => {
            let mut section = original;
            apply_child_rewrites(&mut section, node, &child_rewrites);
            warnings.insert(0, warning(node, code));
            return ContainerRewrite {
                section,
                grouped_sections,
                warnings,
            };
        }
    };
    warnings.extend(parsed.item_warnings.iter().cloned());

    let newline = section_newline(node, ctx.lines);
    let should_group = parsed.has_groupable() || classified.has_groups();
    if !should_group {
        let mut section = original;
        apply_child_rewrites(&mut section, node, &child_rewrites);
        return ContainerRewrite {
            section,
            grouped_sections,
            warnings,
        };
    }

    let plan = grouping_plan(&parsed);
    let body = emit_grouped_body(
        node,
        ctx.contents,
        &classified,
        &plan,
        &child_rewrites,
        newline,
    );
    let section = format!("{heading_raw}{body}");
    let grouping_changed = section != original;
    if grouping_changed {
        grouped_sections.insert(
            0,
            GroupedSection {
                original_heading_line: node.heading_line,
                heading_ancestry: node.ancestry.clone(),
                next_and_in_progress: plan.counts[0],
                blocked: plan.counts[1],
                done_and_canceled: plan.counts[2],
                moved_block_count: plan.moved_blocks.len(),
                moved_blocks: plan.moved_blocks.clone(),
            },
        );
    }

    ContainerRewrite {
        section,
        grouped_sections,
        warnings,
    }
}

fn apply_child_rewrites(
    section: &mut String,
    node: &HeadingNode,
    child_rewrites: &BTreeMap<usize, ContainerRewrite>,
) {
    let base = node.section_span.start;
    let mut children = node
        .children
        .iter()
        .filter_map(|child| {
            child_rewrites
                .get(&child.heading_span.start)
                .map(|rewrite| {
                    (
                        child.section_span.start - base
                            ..child.section_span.end - base,
                        rewrite.section.as_str(),
                    )
                })
        })
        .collect::<Vec<_>>();
    children.sort_by_key(|(span, _)| span.start);
    for (span, replacement) in children.into_iter().rev() {
        section.replace_range(span, replacement);
    }
}

fn warning(node: &HeadingNode, code: GroupingSkipCode) -> GroupingWarning {
    GroupingWarning {
        original_heading_line: node.heading_line,
        heading_ancestry: node.ancestry.clone(),
        code,
        message: code.message(&node.title, node.heading_line),
    }
}

fn warning_at(
    node: &HeadingNode,
    code: GroupingSkipCode,
    line: usize,
) -> GroupingWarning {
    GroupingWarning {
        original_heading_line: node.heading_line,
        heading_ancestry: node.ancestry.clone(),
        code,
        message: code.message(&node.title, line),
    }
}

struct ClassifiedChildren<'a> {
    fail: Option<GroupingSkipCode>,
    warnings: Vec<GroupingWarning>,
    items: Vec<ClassifiedChild<'a>>,
}

impl<'a> ClassifiedChildren<'a> {
    fn is_grouping_child(&self, child: &HeadingNode) -> bool {
        self.items.iter().any(|item| match item {
            ClassifiedChild::Authored(node)
            | ClassifiedChild::NestedTasks(node) => {
                node.heading_span.start == child.heading_span.start
            }
            ClassifiedChild::Group { .. } => false,
        })
    }

    fn has_groups(&self) -> bool {
        self.items
            .iter()
            .any(|item| matches!(item, ClassifiedChild::Group { .. }))
    }

    fn authored(&self) -> impl Iterator<Item = &'a HeadingNode> + '_ {
        self.items.iter().filter_map(|item| match item {
            ClassifiedChild::Authored(node)
            | ClassifiedChild::NestedTasks(node) => Some(*node),
            ClassifiedChild::Group { .. } => None,
        })
    }
}

enum ClassifiedChild<'a> {
    Authored(&'a HeadingNode),
    NestedTasks(&'a HeadingNode),
    Group {
        kind: GroupKind,
        node: &'a HeadingNode,
    },
}

fn classify_children<'a>(
    node: &'a HeadingNode,
    ctx: &TransformCtx<'_>,
) -> ClassifiedChildren<'a> {
    let mut items = Vec::new();
    let mut fail = None;
    let warnings = Vec::new();
    let mut seen_groups = BTreeMap::new();

    if let Some(code) = stray_marker_in_exclusive(node, ctx.lines) {
        fail = Some(code);
    }

    for child in &node.children {
        if is_tasks_title(&child.title) {
            items.push(ClassifiedChild::NestedTasks(child));
            continue;
        }

        if let Some(kind) = GroupKind::from_title(&child.title) {
            if !child.children.is_empty() {
                fail = Some(GroupingSkipCode::AuthoredHeadingInGroup);
                items.push(ClassifiedChild::Authored(child));
                continue;
            }
            match group_marker_state(child, ctx.lines) {
                MarkerState::Valid(found) if found == kind => {
                    if seen_groups.insert(kind, child.heading_line).is_some() {
                        fail = Some(GroupingSkipCode::DuplicateGroupHeading);
                    }
                    items.push(ClassifiedChild::Group { kind, node: child });
                }
                MarkerState::Valid(_) | MarkerState::Mismatch => {
                    fail = Some(GroupingSkipCode::RenamedMarkedHeading);
                }
                MarkerState::Malformed => {
                    fail = Some(GroupingSkipCode::MalformedOwnership);
                }
                MarkerState::Missing => {
                    if adoptable_group_body(child, ctx) {
                        if seen_groups
                            .insert(kind, child.heading_line)
                            .is_some()
                        {
                            fail =
                                Some(GroupingSkipCode::DuplicateGroupHeading);
                        }
                        items
                            .push(ClassifiedChild::Group { kind, node: child });
                    } else {
                        fail = Some(GroupingSkipCode::OwnershipCollision);
                    }
                }
            }
            continue;
        }

        match group_marker_state(child, ctx.lines) {
            MarkerState::Valid(_)
            | MarkerState::Mismatch
            | MarkerState::Malformed => {
                fail = Some(GroupingSkipCode::RenamedMarkedHeading);
            }
            MarkerState::Missing => {
                items.push(ClassifiedChild::Authored(child))
            }
        }
    }

    ClassifiedChildren {
        fail,
        warnings,
        items,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MarkerState {
    Valid(GroupKind),
    Mismatch,
    Malformed,
    Missing,
}

fn group_marker_state(
    node: &HeadingNode,
    lines: &[SourceLine<'_>],
) -> MarkerState {
    let Some(line_index) = first_nonblank_line_index(node, lines) else {
        return MarkerState::Missing;
    };
    match parse_group_marker(lines[line_index].content) {
        Some(Ok(kind)) => {
            if GroupKind::from_title(&node.title) == Some(kind) {
                MarkerState::Valid(kind)
            } else {
                MarkerState::Mismatch
            }
        }
        Some(Err(())) => MarkerState::Malformed,
        None => MarkerState::Missing,
    }
}

fn parse_group_marker(line: &str) -> Option<Result<GroupKind, ()>> {
    let inner = markdown::standalone_html_comment(line)?;
    let rest = inner.strip_prefix(MARKER_PREFIX)?;
    match GroupKind::from_marker_id(rest) {
        Some(kind) => Some(Ok(kind)),
        None => Some(Err(())),
    }
}

fn first_nonblank_line_index(
    node: &HeadingNode,
    lines: &[SourceLine<'_>],
) -> Option<usize> {
    let body_lines = line_range_for_bytes(lines, node.body_span.clone());
    body_lines
        .into_iter()
        .find(|&index| !lines[index].content.trim().is_empty())
}

fn stray_marker_in_exclusive(
    node: &HeadingNode,
    lines: &[SourceLine<'_>],
) -> Option<GroupingSkipCode> {
    for span in exclusive_spans(node) {
        let line_range = line_range_for_bytes(lines, span);
        for index in line_range {
            if parse_group_marker(lines[index].content).is_some() {
                return Some(GroupingSkipCode::MalformedOwnership);
            }
        }
    }
    None
}

fn adoptable_group_body(node: &HeadingNode, ctx: &TransformCtx<'_>) -> bool {
    if !node.children.is_empty() {
        return false;
    }
    let Ok(pieces) = parse_direct_pieces(
        ctx,
        node.body_span.clone(),
        node,
        &mut Vec::new(),
        false,
    ) else {
        return false;
    };
    pieces.iter().all(|piece| match piece {
        DirectPiece::Task(task) => task.movable,
        DirectPiece::Other { raw, .. } => raw.chars().all(char::is_whitespace),
    })
}

fn exclusive_spans(node: &HeadingNode) -> Vec<Range<usize>> {
    let mut spans = Vec::new();
    let mut cursor = node.body_span.start;
    for child in &node.children {
        if child.section_span.start > cursor {
            spans.push(cursor..child.section_span.start);
        }
        cursor = child.section_span.end;
    }
    if cursor < node.body_span.end {
        spans.push(cursor..node.body_span.end);
    }
    spans
}

struct ParsedContainer<'a> {
    intake: Vec<DirectPiece<'a>>,
    groups: BTreeMap<GroupKind, ParsedGroup<'a>>,
    item_warnings: Vec<GroupingWarning>,
}

struct ParsedGroup<'a> {
    leading: &'a str,
    trailing: &'a str,
    pieces: Vec<DirectPiece<'a>>,
}

impl<'a> ParsedContainer<'a> {
    fn has_groupable(&self) -> bool {
        self.iter_tasks()
            .any(|task| task.bucket.is_groupable() && task.movable)
    }

    fn iter_tasks(&self) -> impl Iterator<Item = &TaskBlock<'a>> {
        self.intake.iter().filter_map(DirectPiece::task).chain(
            self.groups.values().flat_map(|group| {
                group.pieces.iter().filter_map(DirectPiece::task)
            }),
        )
    }
}

fn parse_container_regions<'a>(
    node: &'a HeadingNode,
    ctx: &TransformCtx<'a>,
    classified: &ClassifiedChildren<'a>,
) -> Result<ParsedContainer<'a>, GroupingSkipCode> {
    let mut item_warnings = Vec::new();
    let mut intake = Vec::new();
    for span in exclusive_spans(node) {
        let mut pieces =
            parse_direct_pieces(ctx, span, node, &mut item_warnings, true)?;
        intake.append(&mut pieces);
    }

    let mut groups = BTreeMap::new();
    for item in &classified.items {
        let ClassifiedChild::Group {
            kind,
            node: group_node,
        } = item
        else {
            continue;
        };
        let body_start = group_body_start(group_node, ctx.lines);
        let pieces = parse_direct_pieces(
            ctx,
            body_start..group_node.body_span.end,
            node,
            &mut item_warnings,
            true,
        )?;
        let (leading, trailing) = split_group_prose(ctx.contents, &pieces);
        groups.insert(
            *kind,
            ParsedGroup {
                leading,
                trailing,
                pieces,
            },
        );
    }

    Ok(ParsedContainer {
        intake,
        groups,
        item_warnings,
    })
}

fn group_body_start(node: &HeadingNode, lines: &[SourceLine<'_>]) -> usize {
    let Some(index) = first_nonblank_line_index(node, lines) else {
        return node.body_span.start;
    };
    if parse_group_marker(lines[index].content).is_some() {
        lines[index].byte_range.end
    } else {
        node.body_span.start
    }
}

fn split_group_prose<'a>(
    contents: &'a str,
    pieces: &[DirectPiece<'a>],
) -> (&'a str, &'a str) {
    let first_task = pieces.iter().find_map(|piece| match piece {
        DirectPiece::Task(task) => Some(task.byte_range.start),
        DirectPiece::Other { .. } => None,
    });
    let Some(first_task) = first_task else {
        if pieces.is_empty() {
            return ("", "");
        }
        let start = piece_start(&pieces[0]);
        let end = piece_end(pieces.last().expect("non-empty pieces"));
        return (&contents[start..end], "");
    };
    let leading_end = pieces
        .iter()
        .take_while(|piece| matches!(piece, DirectPiece::Other { .. }))
        .last()
        .map(piece_end)
        .unwrap_or(first_task);
    let leading_start = pieces.first().map(piece_start).unwrap_or(first_task);
    let trailing_start = pieces
        .iter()
        .rev()
        .take_while(|piece| matches!(piece, DirectPiece::Other { .. }))
        .last()
        .map(piece_start);
    let trailing_end = pieces.last().map(piece_end);
    let leading = if leading_start < leading_end && leading_end <= first_task {
        &contents[leading_start..leading_end]
    } else {
        ""
    };
    let trailing = match (trailing_start, trailing_end) {
        (Some(start), Some(end)) if start >= first_task && start < end => {
            &contents[start..end]
        }
        _ => "",
    };
    (leading, trailing)
}

fn piece_start(piece: &DirectPiece<'_>) -> usize {
    match piece {
        DirectPiece::Task(task) => task.byte_range.start,
        DirectPiece::Other { byte_range, .. } => byte_range.start,
    }
}

fn piece_end(piece: &DirectPiece<'_>) -> usize {
    match piece {
        DirectPiece::Task(task) => task.byte_range.end,
        DirectPiece::Other { byte_range, .. } => byte_range.end,
    }
}

#[derive(Clone)]
enum DirectPiece<'a> {
    Task(TaskBlock<'a>),
    Other {
        raw: &'a str,
        byte_range: Range<usize>,
    },
}

impl<'a> DirectPiece<'a> {
    fn task(&self) -> Option<&TaskBlock<'a>> {
        match self {
            Self::Task(task) => Some(task),
            Self::Other { .. } => None,
        }
    }
}

#[derive(Clone)]
struct TaskBlock<'a> {
    byte_range: Range<usize>,
    start_line: usize,
    raw: &'a str,
    bucket: StatusBucket,
    movable: bool,
}

fn parse_direct_pieces<'a>(
    ctx: &TransformCtx<'a>,
    span: Range<usize>,
    container: &HeadingNode,
    warnings: &mut Vec<GroupingWarning>,
    fail_on_ambiguous: bool,
) -> Result<Vec<DirectPiece<'a>>, GroupingSkipCode> {
    if span.start >= span.end {
        return Ok(Vec::new());
    }
    let lines = ctx.lines;
    let contents = ctx.contents;
    let mask = ctx.mask;
    let classification = ctx.classification;
    let line_range = line_range_for_bytes(lines, span.clone());
    let mut pieces = Vec::new();
    let mut cursor = span.start;
    let mut index = line_range.start;

    while index < line_range.end {
        let line = &lines[index];
        if line.byte_range.start < span.start {
            index += 1;
            continue;
        }
        if line.byte_range.start >= span.end {
            break;
        }

        if mask.contains(&index)
            || markdown::is_blockquote_line(line.content)
            || markdown::is_indented_code_line(line.content)
            || markdown::html_comment_opens(line.content)
            || markdown::fence_marker(line.content).is_some()
            || markdown::atx_heading(line.content).is_some()
        {
            index += 1;
            continue;
        }

        if let Some(item) = parse_list_item(line.content) {
            let limit = line_range.end;
            let subtree =
                match collect_subtree(lines, index, limit, item.indent) {
                    Ok(subtree) => subtree,
                    Err(code)
                        if code == GroupingSkipCode::AmbiguousBoundary =>
                    {
                        if fail_on_ambiguous {
                            return Err(code);
                        }
                        index += 1;
                        continue;
                    }
                    Err(code) => return Err(code),
                };
            let end_line = subtree.end;
            let byte_range = lines[index].byte_range.start
                ..lines[end_line - 1].byte_range.end.min(span.end);

            if let Some(task) = matching_task(
                &item,
                classification,
                byte_range.clone(),
                contents,
            ) {
                flush_other(
                    contents,
                    &mut pieces,
                    &mut cursor,
                    byte_range.start,
                );
                let movable = !item.ordered;
                if item.ordered {
                    warnings.push(warning_at(
                        container,
                        GroupingSkipCode::UnsupportedOrderedList,
                        line.index + 1,
                    ));
                }
                pieces.push(DirectPiece::Task(TaskBlock {
                    byte_range: byte_range.clone(),
                    start_line: line.index + 1,
                    raw: &contents[byte_range.clone()],
                    bucket: task,
                    movable,
                }));
                cursor = byte_range.end;
                index = end_line;
                continue;
            }

            warn_nested_tasks(
                lines,
                index + 1..end_line,
                classification,
                container,
                warnings,
            );
            index = end_line;
            continue;
        }

        index += 1;
    }

    flush_other(contents, &mut pieces, &mut cursor, span.end);
    Ok(pieces)
}

fn matching_task(
    item: &ListItem<'_>,
    classification: &TaskClassification,
    _byte_range: Range<usize>,
    _contents: &str,
) -> Option<StatusBucket> {
    let symbol = item.checkbox?;
    classification
        .matches_filter(item.body)
        .then(|| classification.bucket(symbol))
}

fn warn_nested_tasks(
    lines: &[SourceLine<'_>],
    range: Range<usize>,
    classification: &TaskClassification,
    container: &HeadingNode,
    warnings: &mut Vec<GroupingWarning>,
) {
    for index in range {
        let Some(item) = parse_list_item(lines[index].content) else {
            continue;
        };
        if item.checkbox.is_some() && classification.matches_filter(item.body) {
            warnings.push(warning_at(
                container,
                GroupingSkipCode::NestedUnderOrdinaryItem,
                lines[index].index + 1,
            ));
        }
    }
}

fn flush_other<'a>(
    contents: &'a str,
    pieces: &mut Vec<DirectPiece<'a>>,
    cursor: &mut usize,
    next: usize,
) {
    if *cursor < next {
        pieces.push(DirectPiece::Other {
            raw: &contents[*cursor..next],
            byte_range: *cursor..next,
        });
        *cursor = next;
    }
}

struct Subtree {
    end: usize,
}

fn collect_subtree(
    lines: &[SourceLine<'_>],
    start: usize,
    limit: usize,
    indent: usize,
) -> Result<Subtree, GroupingSkipCode> {
    let mut index = start + 1;
    let mut last = start;
    let mut in_fence = None;

    while index < limit {
        let content = lines[index].content;
        if let Some(open) = in_fence {
            last = index;
            if markdown::closes_fence(content, open) {
                in_fence = None;
            }
            index += 1;
            continue;
        }

        if content.trim().is_empty() {
            let next_nonblank = (index + 1..limit)
                .find(|&candidate| !lines[candidate].content.trim().is_empty());
            if next_nonblank.is_some_and(|candidate| {
                leading_ws(lines[candidate].content) > indent
            }) {
                last = index;
                index += 1;
                continue;
            }
            break;
        }

        let line_indent = leading_ws(content);
        if line_indent > indent {
            last = index;
            if let Some(marker) = markdown::fence_marker(content) {
                in_fence = Some(marker);
            }
            index += 1;
            continue;
        }

        if markdown::atx_heading(content).is_some()
            || markdown::setext_underline(content).is_some()
            || parse_list_item(content).is_some()
            || markdown::is_blockquote_line(content)
            || markdown::fence_marker(content).is_some()
        {
            break;
        }
        return Err(GroupingSkipCode::AmbiguousBoundary);
    }

    if in_fence.is_some() {
        return Err(GroupingSkipCode::AmbiguousBoundary);
    }
    Ok(Subtree { end: last + 1 })
}

struct ListItem<'a> {
    indent: usize,
    ordered: bool,
    checkbox: Option<char>,
    body: &'a str,
}

fn parse_list_item(content: &str) -> Option<ListItem<'_>> {
    let indent = leading_ws(content);
    let rest = &content[indent..];
    let (after_marker, ordered) = parse_list_marker(rest)?;
    let bytes = after_marker.as_bytes();
    if !bytes.first().is_some_and(u8::is_ascii_whitespace) {
        return None;
    }
    let after_ws = after_marker.trim_start();
    let checkbox = parse_checkbox(after_ws);
    let body = checkbox.map_or(after_ws, |(_, rest)| rest);
    Some(ListItem {
        indent,
        ordered,
        checkbox: checkbox.map(|(symbol, _)| symbol),
        body,
    })
}

fn parse_list_marker(rest: &str) -> Option<(&str, bool)> {
    let bytes = rest.as_bytes();
    if matches!(bytes.first(), Some(b'-' | b'*' | b'+')) {
        return Some((&rest[1..], false));
    }
    let digits = bytes
        .iter()
        .take_while(|byte| byte.is_ascii_digit())
        .count();
    if digits == 0 || !matches!(bytes.get(digits), Some(b'.' | b')')) {
        return None;
    }
    Some((&rest[digits + 1..], true))
}

fn parse_checkbox(rest: &str) -> Option<(char, &str)> {
    let after_open = rest.strip_prefix('[')?;
    let mut characters = after_open.chars();
    let symbol = characters.next()?;
    let after_symbol = &after_open[symbol.len_utf8()..];
    let after_close = after_symbol.strip_prefix(']')?;
    if !after_close.is_empty() && !after_close.starts_with(char::is_whitespace)
    {
        return None;
    }
    Some((symbol, after_close.trim_start()))
}

fn leading_ws(line: &str) -> usize {
    line.as_bytes()
        .iter()
        .take_while(|byte| matches!(byte, b' ' | b'\t'))
        .count()
}

struct GroupingPlan<'a> {
    intake_staying: Vec<DirectPiece<'a>>,
    recovered: Vec<&'a TaskBlock<'a>>,
    groups: BTreeMap<GroupKind, Vec<&'a TaskBlock<'a>>>,
    group_leading: BTreeMap<GroupKind, &'a str>,
    group_trailing: BTreeMap<GroupKind, &'a str>,
    counts: [usize; 3],
    moved_blocks: Vec<MovedBlock>,
}

fn grouping_plan<'a>(parsed: &'a ParsedContainer<'a>) -> GroupingPlan<'a> {
    let mut recovered = Vec::new();
    let mut groups: BTreeMap<GroupKind, Vec<&TaskBlock<'a>>> = BTreeMap::new();
    for kind in GroupKind::ALL {
        groups.insert(kind, Vec::new());
    }
    let mut moved_blocks = Vec::new();

    let mut document_tasks = Vec::new();
    for piece in &parsed.intake {
        if let DirectPiece::Task(task) = piece {
            document_tasks.push((Destination::Intake, task));
        }
    }
    for kind in GroupKind::ALL {
        if let Some(group) = parsed.groups.get(&kind) {
            for piece in &group.pieces {
                if let DirectPiece::Task(task) = piece {
                    document_tasks.push((Destination::from_group(kind), task));
                }
            }
        }
    }

    for (source, task) in document_tasks {
        let dest = if task.movable {
            task.bucket.destination()
        } else {
            source
        };
        if dest != source {
            moved_blocks.push(MovedBlock {
                original_line: task.start_line,
                destination: dest.label(),
            });
        }
        match dest.group() {
            Some(kind) => groups.get_mut(&kind).expect("group slot").push(task),
            None => {
                if source != Destination::Intake && task.movable {
                    recovered.push(task);
                }
            }
        }
    }

    let mut intake_staying = Vec::new();
    for piece in &parsed.intake {
        match piece {
            DirectPiece::Task(task) if task.movable => {
                if task.bucket.destination() == Destination::Intake {
                    intake_staying.push(piece.clone());
                }
            }
            DirectPiece::Task(_) | DirectPiece::Other { .. } => {
                intake_staying.push(piece.clone());
            }
        }
    }

    let mut group_leading = BTreeMap::new();
    let mut group_trailing = BTreeMap::new();
    for kind in GroupKind::ALL {
        if let Some(group) = parsed.groups.get(&kind) {
            group_leading.insert(kind, group.leading);
            group_trailing.insert(kind, group.trailing);
        }
    }

    let counts = [
        groups.get(&GroupKind::Active).map_or(0, Vec::len),
        groups.get(&GroupKind::Blocked).map_or(0, Vec::len),
        groups.get(&GroupKind::Closed).map_or(0, Vec::len),
    ];

    GroupingPlan {
        intake_staying,
        recovered,
        groups,
        group_leading,
        group_trailing,
        counts,
        moved_blocks,
    }
}

impl Destination {
    fn from_group(kind: GroupKind) -> Self {
        match kind {
            GroupKind::Active => Self::Active,
            GroupKind::Blocked => Self::Blocked,
            GroupKind::Closed => Self::Closed,
        }
    }
}

fn emit_grouped_body(
    node: &HeadingNode,
    contents: &str,
    classified: &ClassifiedChildren<'_>,
    plan: &GroupingPlan<'_>,
    child_rewrites: &BTreeMap<usize, ContainerRewrite>,
    newline: &str,
) -> String {
    let mut body = String::new();
    let intake = emit_intake(plan, newline);
    if !intake.trim().is_empty() {
        body.push_str(&intake);
    }

    for child in classified.authored() {
        let part = if let Some(rewrite) =
            child_rewrites.get(&child.heading_span.start)
        {
            rewrite.section.clone()
        } else {
            contents[child.section_span.clone()].to_string()
        };
        push_block(&mut body, &part, newline);
    }

    for kind in GroupKind::ALL {
        let group = emit_group(node.level + 1, kind, plan, newline);
        push_block(&mut body, &group, newline);
    }

    if body.is_empty() {
        return String::new();
    }
    if !body.starts_with('\n') && !body.starts_with("\r\n") {
        let mut prefixed = String::from(newline);
        prefixed.push_str(&body);
        body = prefixed;
    }
    body
}

fn emit_intake(plan: &GroupingPlan<'_>, newline: &str) -> String {
    let mut buf = String::new();
    let mut last_was_task = false;
    let mut inserted_recovered = false;

    for piece in &plan.intake_staying {
        match piece {
            DirectPiece::Other { raw, .. } => {
                if last_was_task
                    && !inserted_recovered
                    && !plan.recovered.is_empty()
                {
                    for task in &plan.recovered {
                        push_task(&mut buf, task.raw, newline);
                    }
                    inserted_recovered = true;
                }
                buf.push_str(raw);
                last_was_task = false;
            }
            DirectPiece::Task(task) => {
                push_task(&mut buf, task.raw, newline);
                last_was_task = true;
            }
        }
    }
    if !inserted_recovered {
        if !last_was_task && !plan.recovered.is_empty() && !buf.is_empty() {
            ensure_single_trailing_newline(&mut buf, newline);
        }
        for task in &plan.recovered {
            push_task(&mut buf, task.raw, newline);
        }
    }
    collapse_internal_blank_runs(&mut buf, newline);
    buf
}

fn emit_group(
    level: usize,
    kind: GroupKind,
    plan: &GroupingPlan<'_>,
    newline: &str,
) -> String {
    let hashes = "#".repeat(level);
    let mut buf = format!(
        "{hashes} {title}{newline}{marker}{newline}",
        title = kind.title(),
        marker = kind.marker()
    );
    let leading = plan.group_leading.get(&kind).copied().unwrap_or("");
    let trailing = plan.group_trailing.get(&kind).copied().unwrap_or("");
    let tasks = plan.groups.get(&kind).map_or(&[][..], Vec::as_slice);
    let leading = trim_blank_edges(leading, newline);
    let trailing = trim_blank_edges(trailing, newline);
    if !leading.is_empty() || !tasks.is_empty() || !trailing.is_empty() {
        buf.push_str(newline);
    }
    if !leading.is_empty() {
        buf.push_str(leading);
        if !leading.ends_with('\n') {
            buf.push_str(newline);
        }
        if !tasks.is_empty() {
            ensure_single_trailing_newline(&mut buf, newline);
        }
    }
    for task in tasks {
        push_task(&mut buf, task.raw, newline);
    }
    if !trailing.is_empty() {
        ensure_trailing_blank(&mut buf, newline);
        buf.push_str(trailing);
        if !trailing.ends_with('\n') {
            buf.push_str(newline);
        }
    }
    buf
}

fn push_task(buf: &mut String, raw: &str, newline: &str) {
    buf.push_str(raw);
    if !raw.ends_with('\n') {
        buf.push_str(newline);
    }
}

fn push_block(buf: &mut String, part: &str, newline: &str) {
    if part.is_empty() {
        return;
    }
    if !buf.is_empty() {
        ensure_trailing_blank(buf, newline);
    }
    buf.push_str(part);
}

fn ensure_trailing_blank(buf: &mut String, newline: &str) {
    let double = format!("{newline}{newline}");
    if buf.ends_with(&double) {
        return;
    }
    if buf.ends_with(newline) {
        buf.push_str(newline);
    } else if !buf.is_empty() {
        buf.push_str(newline);
        buf.push_str(newline);
    }
}

fn ensure_single_trailing_newline(buf: &mut String, newline: &str) {
    if buf.is_empty() {
        return;
    }
    if !buf.ends_with('\n') {
        buf.push_str(newline);
    }
}

fn trim_blank_edges<'a>(raw: &'a str, newline: &str) -> &'a str {
    let mut value = raw;
    while let Some(stripped) = value.strip_prefix(newline) {
        value = stripped;
    }
    while let Some(stripped) = value.strip_suffix(newline) {
        if stripped.ends_with('\n') || stripped.is_empty() {
            value = stripped;
        } else {
            break;
        }
    }
    value
}

fn collapse_internal_blank_runs(buf: &mut String, newline: &str) {
    let triple = format!("{newline}{newline}{newline}");
    let double = format!("{newline}{newline}");
    while buf.contains(&triple) {
        *buf = buf.replace(&triple, &double);
    }
}

fn section_newline(
    node: &HeadingNode,
    lines: &[SourceLine<'_>],
) -> &'static str {
    let heading_lines = line_range_for_bytes(lines, node.heading_span.clone());
    heading_lines
        .into_iter()
        .rev()
        .find_map(|index| match lines[index].ending {
            "\r\n" => Some("\r\n"),
            "\n" => Some("\n"),
            _ => None,
        })
        .or_else(|| {
            line_range_for_bytes(lines, node.body_span.clone())
                .into_iter()
                .find_map(|index| match lines[index].ending {
                    "\r\n" => Some("\r\n"),
                    "\n" => Some("\n"),
                    _ => None,
                })
        })
        .unwrap_or("\n")
}

fn apply_final_newline(output: &mut String, original: &str) {
    let original_nl = original.ends_with('\n');
    let output_nl = output.ends_with('\n');
    if original_nl && !output_nl {
        if original.ends_with("\r\n") {
            output.push_str("\r\n");
        } else {
            output.push('\n');
        }
    } else if !original_nl && output_nl {
        if output.ends_with("\r\n") {
            output.truncate(output.len() - 2);
        } else {
            output.pop();
        }
    }
}

fn is_tasks_title(title: &str) -> bool {
    title.eq_ignore_ascii_case("Tasks")
}

struct SourceLine<'a> {
    index: usize,
    byte_range: Range<usize>,
    content: &'a str,
    ending: &'a str,
}

fn source_lines(contents: &str) -> Vec<SourceLine<'_>> {
    let mut lines = Vec::new();
    let mut start = 0;
    for (offset, byte) in contents.bytes().enumerate() {
        if byte != b'\n' {
            continue;
        }
        let end = offset + 1;
        let raw = &contents[start..end];
        let (content, ending) = markdown::split_line_ending(raw);
        lines.push(SourceLine {
            index: lines.len(),
            byte_range: start..end,
            content,
            ending,
        });
        start = end;
    }
    if start < contents.len() {
        lines.push(SourceLine {
            index: lines.len(),
            byte_range: start..contents.len(),
            content: &contents[start..],
            ending: "",
        });
    }
    lines
}

fn heading_mask(lines: &[SourceLine<'_>]) -> std::collections::BTreeSet<usize> {
    let logical = lines.iter().map(|line| line.content).collect::<Vec<_>>();
    let mut masked = std::collections::BTreeSet::new();
    let frontmatter_end = markdown::strictly_closed_frontmatter_end(&logical);
    let scan_start = frontmatter_end.map_or(0, |end| {
        for index in 0..=end {
            masked.insert(index);
        }
        end + 1
    });
    for index in markdown::fenced_lines(&logical, scan_start..lines.len()) {
        masked.insert(index);
    }

    let mut index = scan_start;
    while index < lines.len() {
        if masked.contains(&index) {
            index += 1;
            continue;
        }
        let content = lines[index].content;
        if markdown::html_comment_opens(content) {
            masked.insert(index);
            if !markdown::html_comment_closes(content) {
                index += 1;
                while index < lines.len() {
                    masked.insert(index);
                    let closes =
                        markdown::html_comment_closes(lines[index].content);
                    index += 1;
                    if closes {
                        break;
                    }
                }
                continue;
            }
        }
        index += 1;
    }
    masked
}

struct FlatHeading {
    level: usize,
    title: String,
    heading_line: usize,
    heading_span: Range<usize>,
}

fn discover_headings(
    lines: &[SourceLine<'_>],
    mask: &std::collections::BTreeSet<usize>,
) -> Vec<FlatHeading> {
    let mut headings = Vec::new();
    let mut previous_plain: Option<usize> = None;
    for (index, line) in lines.iter().enumerate() {
        if mask.contains(&index) {
            previous_plain = None;
            continue;
        }
        if line.content.trim().is_empty() {
            previous_plain = None;
            continue;
        }
        if markdown::is_blockquote_line(line.content)
            || markdown::is_indented_code_line(line.content)
        {
            previous_plain = None;
            continue;
        }
        if let Some((level, title)) = markdown::atx_heading(line.content) {
            headings.push(FlatHeading {
                level,
                title: title.to_string(),
                heading_line: index + 1,
                heading_span: line.byte_range.clone(),
            });
            previous_plain = None;
            continue;
        }
        if let Some(level) = markdown::setext_underline(line.content)
            && let Some(previous) = previous_plain.take()
        {
            headings.push(FlatHeading {
                level,
                title: lines[previous].content.trim().to_string(),
                heading_line: previous + 1,
                heading_span: lines[previous].byte_range.start
                    ..line.byte_range.end,
            });
            continue;
        }
        if parse_list_item(line.content).is_some() {
            previous_plain = None;
            continue;
        }
        previous_plain = Some(index);
    }
    headings
}

struct HeadingNode {
    level: usize,
    title: String,
    heading_line: usize,
    heading_span: Range<usize>,
    body_span: Range<usize>,
    section_span: Range<usize>,
    ancestry: Vec<String>,
    children: Vec<HeadingNode>,
}

fn build_tree(
    contents_len: usize,
    flats: Vec<FlatHeading>,
) -> Vec<HeadingNode> {
    let ends = section_ends(contents_len, &flats);
    nest_headings(&flats, &ends, 0..flats.len(), Vec::new())
}

fn section_ends(contents_len: usize, flats: &[FlatHeading]) -> Vec<usize> {
    let mut ends = vec![contents_len; flats.len()];
    for (index, heading) in flats.iter().enumerate() {
        if let Some(next) = flats[index + 1..]
            .iter()
            .find(|candidate| candidate.level <= heading.level)
        {
            ends[index] = next.heading_span.start;
        }
    }
    ends
}

fn nest_headings(
    flats: &[FlatHeading],
    ends: &[usize],
    range: Range<usize>,
    parent_ancestry: Vec<String>,
) -> Vec<HeadingNode> {
    let mut nodes = Vec::new();
    let mut index = range.start;
    while index < range.end {
        let heading = &flats[index];
        let mut child_end = index + 1;
        while child_end < range.end && flats[child_end].level > heading.level {
            child_end += 1;
        }
        let mut ancestry = parent_ancestry.clone();
        ancestry.push(heading.title.clone());
        let children =
            nest_headings(flats, ends, index + 1..child_end, ancestry.clone());
        nodes.push(HeadingNode {
            level: heading.level,
            title: heading.title.clone(),
            heading_line: heading.heading_line,
            heading_span: heading.heading_span.clone(),
            body_span: heading.heading_span.end..ends[index],
            section_span: heading.heading_span.start..ends[index],
            ancestry,
            children,
        });
        index = child_end;
    }
    nodes
}

fn line_range_for_bytes(
    lines: &[SourceLine<'_>],
    span: Range<usize>,
) -> Range<usize> {
    if span.start >= span.end {
        return 0..0;
    }
    let start = lines
        .iter()
        .position(|line| line.byte_range.end > span.start)
        .unwrap_or(lines.len());
    let end = lines
        .iter()
        .position(|line| line.byte_range.start >= span.end)
        .unwrap_or(lines.len());
    start..end
}

#[cfg(test)]
mod tests {
    use super::*;

    fn grouped(input: &str) -> TransformOutput {
        transform(input, &TaskClassification::standard("#task"))
    }

    fn assert_idempotent(input: &str) {
        let once = grouped(input);
        let twice =
            transform(&once.contents, &TaskClassification::standard("#task"));
        assert_eq!(once.contents, twice.contents, "second pass changed bytes");
        assert!(!twice.changed, "second pass reported a change");
    }

    fn assert_unchanged_outside_tasks(input: &str, output: &str) {
        let input_prefix = input.split("## Tasks").next().unwrap_or(input);
        let output_prefix = output.split("## Tasks").next().unwrap_or(output);
        assert_eq!(input_prefix, output_prefix);
    }

    fn root_task_lines(contents: &str) -> Vec<String> {
        contents
            .lines()
            .filter(|line| {
                let trimmed = line.trim_start();
                trimmed.starts_with("- [") && trimmed.contains("#task")
            })
            .map(str::to_string)
            .collect()
    }

    const GOLDEN_INPUT: &str = "\
## Tasks

Short context for this project.

- [ ] #task An idea to pick up later ^idea
- [/] #task Finish the design ^design
  - Keep the keyboard interaction simple.
- [*] #task Review the implementation ^review
- [?] #task Ship when the dependency is ready ^ship
- [x] #task Agree on the scope ^scope
- [-] #task Superseded experiment ^experiment
";

    const GOLDEN_OUTPUT: &str = "\
## Tasks

Short context for this project.

- [ ] #task An idea to pick up later ^idea

### Next & In Progress
<!-- bob:task-status-group:v1:active -->

- [/] #task Finish the design ^design
  - Keep the keyboard interaction simple.
- [*] #task Review the implementation ^review

### Blocked
<!-- bob:task-status-group:v1:blocked -->

- [?] #task Ship when the dependency is ready ^ship

### Done & Canceled
<!-- bob:task-status-group:v1:closed -->

- [x] #task Agree on the scope ^scope
- [-] #task Superseded experiment ^experiment
";

    #[test]
    fn golden_layout_groups_every_status_bucket_and_keeps_ready_intake() {
        let fixture_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/task_status_groups");
        let fixture_in = std::fs::read_to_string(fixture_dir.join("layout.md"))
            .expect("golden input fixture");
        let fixture_out =
            std::fs::read_to_string(fixture_dir.join("layout.grouped.md"))
                .expect("golden output fixture");
        assert_eq!(fixture_in, GOLDEN_INPUT);
        assert_eq!(fixture_out, GOLDEN_OUTPUT);

        let result = grouped(GOLDEN_INPUT);
        assert_eq!(result.contents, GOLDEN_OUTPUT);
        assert!(result.changed);
        assert_eq!(result.grouped_sections.len(), 1);
        let section = &result.grouped_sections[0];
        assert_eq!(section.original_heading_line, 1);
        assert_eq!(section.heading_ancestry, ["Tasks"]);
        assert_eq!(section.next_and_in_progress, 2);
        assert_eq!(section.blocked, 1);
        assert_eq!(section.done_and_canceled, 2);
        assert_eq!(section.moved_block_count, 5);
        assert_eq!(
            section
                .moved_blocks
                .iter()
                .map(|block| (block.original_line, block.destination))
                .collect::<Vec<_>>(),
            [
                (6, DestinationLabel::NextAndInProgress),
                (8, DestinationLabel::NextAndInProgress),
                (9, DestinationLabel::Blocked),
                (10, DestinationLabel::DoneAndCanceled),
                (11, DestinationLabel::DoneAndCanceled),
            ]
        );
        assert_idempotent(GOLDEN_INPUT);
    }

    #[test]
    fn ready_only_and_empty_sections_are_not_decorated() {
        let ready = "## Tasks\n\nIntro.\n\n- [ ] #task later\n";
        let result = grouped(ready);
        assert!(!result.changed);
        assert_eq!(result.contents, ready);
        assert!(result.grouped_sections.is_empty());

        let empty = "## Tasks\n\nJust context.\n";
        let result = grouped(empty);
        assert!(!result.changed);
        assert_eq!(result.contents, empty);
    }

    #[test]
    fn blockless_and_duplicate_ids_still_group() {
        let input = "\
## Tasks

- [*] #task First
- [*] #task Second ^dup
- [?] #task Third ^dup
";
        let result = grouped(input);
        assert!(result.contents.contains(TITLE_ACTIVE));
        assert!(result
            .contents
            .contains("- [*] #task First\n- [*] #task Second ^dup"));
        assert!(result.contents.contains("- [?] #task Third ^dup"));
        assert_idempotent(input);
    }

    #[test]
    fn custom_terminal_statuses_and_registry_precedence() {
        let classification = TaskClassification::from_status_types(
            "#task",
            [
                (' ', "TODO"),
                ('*', "ON_HOLD"),
                ('/', "IN_PROGRESS"),
                ('?', "ON_HOLD"),
                ('x', "DONE"),
                ('D', "DONE"),
                ('C', "CANCELLED"),
                ('Q', "TODO"),
                ('N', "NON_TASK"),
                ('E', "EMPTY"),
            ],
            '*',
            '/',
            '?',
            ' ',
        );
        let input = "\
## Tasks

- [ ] #task ready
- [*] #task next
- [D] #task custom-done
- [C] #task custom-cancelled
- [Q] #task other-open
- [N] #task non-task
- [E] #task empty
- [Z] #task unknown
";
        let result = transform(input, &classification);
        assert!(result.contents.contains("- [D] #task custom-done"));
        assert!(result.contents.contains("- [C] #task custom-cancelled"));
        let closed = result.contents.split(TITLE_CLOSED).nth(1).unwrap();
        assert!(closed.contains("custom-done"));
        assert!(closed.contains("custom-cancelled"));
        assert!(!closed.contains("non-task"));
        assert!(!closed.contains("unknown"));
        assert!(result.contents.contains("- [Q] #task other-open"));
        assert!(result.contents.contains("- [N] #task non-task"));
        assert!(result.contents.contains("- [Z] #task unknown"));
    }

    #[test]
    fn empty_global_filter_accepts_all_checkbox_tasks() {
        let classification = TaskClassification::standard("");
        let input = "## Tasks\n\n- [*] no tag needed\n- [ ] ready also\n";
        let result = transform(input, &classification);
        assert!(result.contents.contains(TITLE_ACTIVE));
        assert!(result.contents.contains("- [*] no tag needed"));
        assert!(result.contents.contains("- [ ] ready also"));
    }

    #[test]
    fn global_filter_rejects_non_matching_lines() {
        let input = "## Tasks\n\n- [*] not filtered\n- [*] #task real\n";
        let result = grouped(input);
        assert!(result.contents.contains("- [*] not filtered"));
        assert!(result.contents.contains("- [*] #task real"));
        let active = result.contents.split(TITLE_ACTIVE).nth(1).unwrap();
        assert!(active.contains("#task real"));
        assert!(!active.contains("not filtered"));
    }

    #[test]
    fn nested_children_travel_with_parent_status() {
        let input = "\
## Tasks

- [*] #task Parent
  - [x] #task Child done
    - note
  - [?] #task Child blocked

- [ ] #task Ready
";
        let result = grouped(input);
        let active = result.contents.split(TITLE_ACTIVE).nth(1).unwrap();
        let active = active.split("### ").next().unwrap();
        assert!(active.contains("- [*] #task Parent"));
        assert!(active.contains("- [x] #task Child done"));
        assert!(active.contains("- [?] #task Child blocked"));
        assert!(!result
            .contents
            .split(TITLE_CLOSED)
            .nth(1)
            .unwrap()
            .contains("Child done"));
        assert_eq!(result.grouped_sections[0].next_and_in_progress, 1);
        assert_eq!(result.grouped_sections[0].done_and_canceled, 0);
        assert_idempotent(input);
    }

    #[test]
    fn tabs_internal_blanks_and_fences_stay_in_the_task_block() {
        let input = "\
## Tasks

\t- [*] #task Tabbed
\t\t- child

- [/] #task Fenced
  ```
  - [ ] #task inside fence
  ```
  still in item

- [?] #task After
";
        let result = grouped(input);
        let active = result.contents.split(TITLE_ACTIVE).nth(1).unwrap();
        assert!(active.contains("\t- [*] #task Tabbed"));
        assert!(active.contains("\t\t- child"));
        assert!(active.contains("```\n  - [ ] #task inside fence\n  ```"));
        assert!(active.contains("still in item"));
        assert_idempotent(input);
    }

    #[test]
    fn multiple_tasks_headings_and_heading_syntax_variants() {
        let input = "\
# Outer

## Tasks ##

- [*] #task First

## Other

Unrelated.

### tasks

- [?] #task Second

Tasks
-----

- [x] #task Third
";
        let result = grouped(input);
        assert_eq!(result.grouped_sections.len(), 3);
        assert!(result.contents.contains("# Outer"));
        assert!(result.contents.contains("## Other\n\nUnrelated."));
        assert!(result.contents.contains("- [*] #task First"));
        assert!(result.contents.contains("- [?] #task Second"));
        assert!(result.contents.contains("- [x] #task Third"));
        assert_unchanged_outside_tasks(input, &result.contents);
        assert_idempotent(input);
    }

    #[test]
    fn authored_topics_receive_local_groups() {
        let input = "\
## Tasks

### Backend

- [*] #task API
- [ ] #task later

### Frontend

- [?] #task UI
";
        let result = grouped(input);
        assert!(result.contents.contains("### Backend"));
        assert!(result.contents.contains("#### Next & In Progress"));
        assert!(result.contents.contains("### Frontend"));
        let backend = result.contents.split("### Backend").nth(1).unwrap();
        let backend = backend.split("### Frontend").next().unwrap();
        assert!(backend.contains("- [*] #task API"));
        assert!(backend.contains("- [ ] #task later"));
        assert!(!backend.contains("- [?] #task UI"));
        assert_eq!(result.grouped_sections.len(), 2);
        assert_idempotent(input);
    }

    #[test]
    fn nested_tasks_is_processed_once() {
        let input = "\
## Tasks

- [*] #task Outer

### Tasks

- [?] #task Inner
";
        let result = grouped(input);
        assert!(result.contents.contains("- [*] #task Outer"));
        assert!(result.contents.contains("- [?] #task Inner"));
        let outer_record = result
            .grouped_sections
            .iter()
            .find(|section| section.heading_ancestry == ["Tasks"])
            .expect("outer");
        let inner_record = result
            .grouped_sections
            .iter()
            .find(|section| section.heading_ancestry == ["Tasks", "Tasks"])
            .expect("inner");
        assert_eq!(outer_record.next_and_in_progress, 1);
        assert_eq!(inner_record.blocked, 1);
        assert_idempotent(input);
    }

    #[test]
    fn excluded_markdown_contexts_are_not_task_roots_or_headings() {
        let input = "\
---
title: note
---

## Tasks

- [*] #task Real

```md
## Tasks
- [*] #task Fenced
```

<!--
## Tasks
- [*] #task Comment
-->

> ## Tasks
> - [*] #task Quoted

    ## Tasks
    - [*] #task Indented
";
        let result = grouped(input);
        assert_eq!(result.grouped_sections.len(), 1);
        assert!(result.contents.contains("- [*] #task Fenced"));
        assert!(result.contents.contains("- [*] #task Comment"));
        assert!(result.contents.contains("- [*] #task Quoted"));
        assert!(result.contents.contains("- [*] #task Indented"));
        let active = result.contents.split(TITLE_ACTIVE).nth(1).unwrap();
        assert!(active.contains("- [*] #task Real"));
        assert!(!active.contains("Fenced"));
        assert!(!active.contains("Comment"));
        assert!(!active.contains("Quoted"));
        assert!(!active.contains("Indented"));
        assert_idempotent(input);
    }

    #[test]
    fn h6_tasks_is_skipped_with_a_diagnostic() {
        let input = "###### Tasks\n\n- [*] #task Stuck\n";
        let result = grouped(input);
        assert!(!result.changed);
        assert_eq!(result.contents, input);
        assert_eq!(result.warnings.len(), 1);
        assert_eq!(result.warnings[0].code, GroupingSkipCode::H6Container);
    }

    #[test]
    fn crlf_mixed_endings_unicode_and_missing_final_newline() {
        let input = "## Tasks\r\n\r\n- [*] #task café 日本語 ^id\n- [?] #task other\r\n- [x] #task done";
        let result = grouped(input);
        assert!(result.contents.contains("café 日本語 ^id"));
        assert!(result.contents.contains("\r\n"));
        assert!(!result.contents.ends_with('\n') || input.ends_with('\n'));
        assert!(!result.contents.ends_with('\n'));
        assert!(result.contents.contains("- [*] #task café 日本語 ^id\n"));
        assert_idempotent(input);
    }

    #[test]
    fn safe_legacy_adoption_completes_a_partial_set() {
        let input = "\
## Tasks

- [*] #task Next

### Blocked

- [?] #task Already
";
        let result = grouped(input);
        assert!(result.contents.contains(MARKER_ACTIVE));
        assert!(result.contents.contains(MARKER_BLOCKED));
        assert!(result.contents.contains(MARKER_CLOSED));
        assert!(result.contents.contains("- [?] #task Already"));
        assert_idempotent(input);
    }

    #[test]
    fn unmarked_group_title_with_prose_is_a_collision() {
        let input = "\
## Tasks

- [*] #task Next

### Next & In Progress

This is my own notes section.

- some bullet
";
        let result = grouped(input);
        assert!(!result.changed);
        assert_eq!(result.contents, input);
        assert!(result.warnings.iter().any(
            |warning| warning.code == GroupingSkipCode::OwnershipCollision
        ));
    }

    #[test]
    fn prose_inside_managed_groups_is_preserved() {
        let input = "\
## Tasks

- [ ] #task ready

### Next & In Progress
<!-- bob:task-status-group:v1:active -->

Focus on these first.

- [*] #task next
- [ ] #task reopened

### Blocked
<!-- bob:task-status-group:v1:blocked -->

### Done & Canceled
<!-- bob:task-status-group:v1:closed -->
";
        let result = grouped(input);
        assert!(result.contents.contains("Focus on these first."));
        let intake = result.contents.split("### Next").next().unwrap();
        assert!(intake.contains("- [ ] #task ready"));
        assert!(intake.contains("- [ ] #task reopened"));
        assert!(!intake.contains("- [*] #task next"));
        assert_idempotent(&result.contents);
    }

    #[test]
    fn malformed_and_duplicate_markers_fail_closed() {
        let malformed = "\
## Tasks

### Next & In Progress
<!-- bob:task-status-group:v1:ACTIV -->

- [*] #task next
";
        let result = grouped(malformed);
        assert_eq!(result.contents, malformed);
        assert!(result.warnings.iter().any(
            |warning| warning.code == GroupingSkipCode::MalformedOwnership
        ));

        let duplicate = "\
## Tasks

### Next & In Progress
<!-- bob:task-status-group:v1:active -->

- [*] #task a

### Next & In Progress
<!-- bob:task-status-group:v1:active -->

- [/] #task b
";
        let result = grouped(duplicate);
        assert_eq!(result.contents, duplicate);
        assert!(result
            .warnings
            .iter()
            .any(|warning| warning.code
                == GroupingSkipCode::DuplicateGroupHeading));

        let renamed = "\
## Tasks

### My WIP
<!-- bob:task-status-group:v1:active -->

- [*] #task next
";
        let result = grouped(renamed);
        assert_eq!(result.contents, renamed);
        assert!(result
            .warnings
            .iter()
            .any(|warning| warning.code
                == GroupingSkipCode::RenamedMarkedHeading));
    }

    #[test]
    fn authored_heading_inside_a_managed_group_fails_closed() {
        let input = "\
## Tasks

### Next & In Progress
<!-- bob:task-status-group:v1:active -->

- [*] #task next

#### Notes
details
";
        let result = grouped(input);
        assert_eq!(result.contents, input);
        assert!(result
            .warnings
            .iter()
            .any(|warning| warning.code
                == GroupingSkipCode::AuthoredHeadingInGroup));
    }

    #[test]
    fn empty_groups_are_retained_once_created() {
        let input = GOLDEN_OUTPUT.replace(
            "- [?] #task Ship when the dependency is ready ^ship\n",
            "",
        );
        let result = grouped(&input);
        assert!(result.contents.contains(
            "### Blocked\n<!-- bob:task-status-group:v1:blocked -->"
        ));
        assert_idempotent(&input);
    }

    #[test]
    fn reopening_a_task_to_ready_appends_to_intake() {
        let input = "\
## Tasks

Intro.

- [ ] #task existing

### Next & In Progress
<!-- bob:task-status-group:v1:active -->

- [ ] #task reopened
- [*] #task still-next

### Blocked
<!-- bob:task-status-group:v1:blocked -->

### Done & Canceled
<!-- bob:task-status-group:v1:closed -->
";
        let result = grouped(input);
        let intake = result.contents.split("### Next").next().unwrap();
        assert!(intake.contains("Intro."));
        assert!(intake.contains("- [ ] #task existing"));
        assert!(intake.contains("- [ ] #task reopened"));
        let existing_at = intake.find("- [ ] #task existing").unwrap();
        let reopened_at = intake.find("- [ ] #task reopened").unwrap();
        assert!(existing_at < reopened_at);
        assert_idempotent(&result.contents);
    }

    #[test]
    fn ordered_list_roots_and_nested_ordinary_items_are_reported() {
        let ordered =
            "## Tasks\n\n1. [*] #task numbered\n- [?] #task sibling\n";
        let result = grouped(ordered);
        assert!(result.contents.contains("1. [*] #task numbered"));
        assert!(result
            .warnings
            .iter()
            .any(|warning| warning.code
                == GroupingSkipCode::UnsupportedOrderedList));
        let active = result.contents.split(TITLE_ACTIVE).nth(1).unwrap_or("");
        assert!(!active.contains("numbered"));

        let nested =
            "## Tasks\n\n- context\n  - [*] #task nested\n- [?] #task root\n";
        let result = grouped(nested);
        assert!(result.contents.contains("  - [*] #task nested"));
        assert!(result
            .warnings
            .iter()
            .any(|warning| warning.code
                == GroupingSkipCode::NestedUnderOrdinaryItem));
    }

    #[test]
    fn lazy_continuation_skips_the_container() {
        let input = "## Tasks\n\n- [*] #task title\nlazy continuation\n";
        let result = grouped(input);
        assert_eq!(result.contents, input);
        assert!(
            result
                .warnings
                .iter()
                .any(|warning| warning.code
                    == GroupingSkipCode::AmbiguousBoundary)
        );
    }

    #[test]
    fn standalone_prose_is_not_attached_to_a_task() {
        let input = "## Tasks\n\n- [*] #task title\n\nA query follows.\n";
        let result = grouped(input);
        assert!(result.changed);
        let intake = result.contents.split("### Next").next().unwrap();
        assert!(intake.contains("A query follows."));
        assert!(!intake.contains("- [*] #task title"));
        assert_idempotent(input);
    }

    #[test]
    fn skip_codes_are_stable() {
        assert_eq!(GroupingSkipCode::H6Container.as_str(), "h6_container");
        assert_eq!(
            GroupingSkipCode::AmbiguousBoundary.as_str(),
            "ambiguous_boundary"
        );
        assert!(GroupingSkipCode::OwnershipCollision.fails_closed());
        assert!(!GroupingSkipCode::UnsupportedOrderedList.fails_closed());
        assert!(!GroupingSkipCode::NestedUnderOrdinaryItem.fails_closed());
    }

    #[test]
    fn conservation_and_source_records() {
        let result = grouped(GOLDEN_INPUT);
        let input_tasks = root_task_lines(GOLDEN_INPUT);
        let output_tasks = root_task_lines(&result.contents);
        assert_eq!(input_tasks.len(), output_tasks.len());
        let mut input_sorted = input_tasks.clone();
        let mut output_sorted = output_tasks.clone();
        input_sorted.sort();
        output_sorted.sort();
        assert_eq!(input_sorted, output_sorted);
        assert_eq!(result.grouped_sections[0].original_heading_line, 1);
    }

    #[test]
    fn out_of_scope_spans_are_byte_identical() {
        let input = "\
# Project

Keep this.

## Tasks

- [*] #task grouped

## Log

Do not touch.
";
        let result = grouped(input);
        assert!(result.contents.starts_with("# Project\n\nKeep this.\n\n"));
        assert!(result.contents.ends_with("## Log\n\nDo not touch.\n"));
        assert_idempotent(input);
    }

    #[test]
    fn heading_only_first_setup_is_a_change() {
        let input = "\
## Tasks

### Blocked

- [?] #task already
";
        let result = grouped(input);
        assert!(result.changed);
        assert!(result.contents.contains(MARKER_ACTIVE));
        assert!(result.contents.contains(MARKER_CLOSED));
        assert!(
            result.grouped_sections[0].moved_block_count == 0
                || result.contents.contains("- [?] #task already")
        );
    }
}
