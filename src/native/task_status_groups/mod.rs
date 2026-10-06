//! Pure Markdown transform that groups Tasks-section task blocks by status.
//!
//! This module does not read the filesystem, mutate task statuses, resolve
//! links, or write files. Classification is supplied by the caller.
//! Integration in a later phase is the first production caller; unit tests
//! exercise the API until then.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
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

/// Shared badge-row grammar: the four fixed-order `(emoji, label)` chips.
/// Both the emitter (`render_badges`) and the recognizer (`is_badge_row`)
/// iterate this table so the two cannot drift apart.
const BADGE_CHIPS: [(&str, &str); 4] = [
    ("⚪", "open"),
    ("🔵", "next/wip"),
    ("🔴", "blocked"),
    ("🟢", "done/canceled"),
];

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
    MisplacedBadgeRow,
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
            Self::MisplacedBadgeRow => "misplaced_badge_row",
            Self::NestedUnderOrdinaryItem => "nested_under_ordinary_item",
            Self::UnsupportedOrderedList => "unsupported_ordered_list",
        }
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
            Self::MisplacedBadgeRow => format!(
                "status badge row inside a managed status group in {title:?} at line {line}"
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
    pub open: usize,
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

impl DestinationLabel {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Intake => "intake",
            Self::NextAndInProgress => "next_and_in_progress",
            Self::Blocked => "blocked",
            Self::DoneAndCanceled => "done_and_canceled",
        }
    }
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

    #[cfg(test)]
    fn with_symbol(mut self, symbol: char, bucket: StatusBucket) -> Self {
        self.buckets.insert(symbol, bucket);
        self
    }

    #[cfg(test)]
    fn standard(global_filter: impl Into<String>) -> Self {
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
    let should_decorate = parsed.has_groupable() || classified.has_groups();
    let should_rewrite = should_decorate || parsed.has_badges;
    if !should_rewrite {
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
        should_decorate,
        heading_raw.ends_with('\n'),
    );
    let section = format!("{heading_raw}{body}");
    let grouping_changed = section != original;
    if grouping_changed && should_decorate {
        grouped_sections.insert(
            0,
            GroupedSection {
                original_heading_line: node.heading_line,
                heading_ancestry: node.ancestry.clone(),
                open: plan.open_count,
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
            if badge_row_in_span(ctx.lines, child.body_span.clone()) {
                fail = Some(GroupingSkipCode::MisplacedBadgeRow);
            }
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

mod emit;
mod parse;
#[cfg(test)]
mod tests;

use emit::*;
use parse::*;
pub(crate) use parse::{is_badge_row, is_legacy_badge_marker};
