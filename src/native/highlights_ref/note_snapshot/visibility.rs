//! Bob-layer managed logs, closed-task appendix, and self-snapshot filter.
//!
//! After chips and relationship rows land, this pass:
//!
//! - recognizes direct-child SCHEDULE / WORK / CANCEL logs from raw source
//!   with the shared marker helpers, counting entries rather than nested
//!   reason bullets
//! - condenses logs in reading mode (`moved N×` chips, newest work
//!   [`LogRow`], cancel logs hidden) and prints full logs when
//!   `include_closed` is set
//! - omits done/canceled subtrees (counting each closed task once) or
//!   restores them into one compact `Closed tasks` appendix
//! - omits genuine `#ref` tasks whose linked note's `source_note` resolves
//!   to this snapshot's root, counting them separately and never also as
//!   closed omissions
//! - leaves the surviving body plus appendix for the assembler to rebuild
//!   visible anchors and shown-task counts
//!
//! Ordinary notes never enter this pass. Source bytes are only read through
//! the shared [`SourceStore`]; this module never writes the vault.

use std::collections::{BTreeSet, HashMap};
use std::ops::Range;
use std::path::{Path, PathBuf};

use chrono::{Datelike, NaiveDate};

use super::bob::{has_ref_tag, remove_consumed_muted};
use super::classify::{SourceLoader, SourceStore};
use super::resolve::{parse_link_target, VaultNotes};
use super::view::{
    AnchorIdentity, AnchorKind, Block, Chip, ChipKind, HeadingClass,
    HeadingLevel, Inline, ListItem, SnapshotAnchor, SnapshotHeading,
    SnapshotSection, SnapshotTask, SnapshotTaskStatus, SnapshotWarning,
    SnapshotWarningKind, SourceIdentity, SourceLocation,
};
use crate::native::capture::{
    list_item_body, parse_managed_task_log_marker, ManagedTaskLogKind,
};
use crate::native::projects::{
    frontmatter_value, parse_frontmatter, trim_yaml_scalar,
    wikilink_spans_in_line,
};
use crate::native::task_dependencies::inline_code_spans;
use crate::native::task_fields::{inline_fields, parse_strict_calendar_date};
use crate::native::task_status_groups::TITLE_CLOSED;
use crate::native::task_status_hooks::is_cancel_log_line;

/// Snapshot-day and appendix policy for the logs/visibility pass.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct BobVisibilityContext {
    pub snapshot_date: NaiveDate,
    pub include_closed: bool,
}

/// Counters, warnings, and the optional closed appendix produced here.
#[derive(Debug, Default)]
pub(crate) struct BobVisibilityOutcome {
    pub closed: Option<SnapshotSection>,
    pub omitted_closed_tasks: u64,
    pub omitted_self_snapshot_tasks: u64,
    pub schedule_logs: u64,
    pub work_logs: u64,
    pub warnings: Vec<SnapshotWarning>,
}

/// Transform assembled Bob sections: logs, self-snapshot filter, closed
/// omission or appendix. The caller rebuilds `view.anchors` and shown
/// task counts from the resulting body plus `outcome.closed`.
pub(crate) fn apply_bob_visibility<L: SourceLoader>(
    sections: &mut [SnapshotSection],
    store: &mut SourceStore<L>,
    root: &SourceIdentity,
    context: &BobVisibilityContext,
) -> BobVisibilityOutcome {
    let notes = store.vault_notes();
    let mut pass = VisibilityPass {
        store,
        notes,
        root,
        context,
        omitted_closed: 0,
        omitted_self: 0,
        schedule_logs: 0,
        work_logs: 0,
        warnings: Vec::new(),
        source_note_of: HashMap::new(),
    };
    for section in sections.iter_mut() {
        pass.convert_logs_in_blocks(&mut section.blocks);
    }
    let mut extracted = Vec::new();
    for section in sections.iter_mut() {
        let (surviving, nested) =
            pass.filter_blocks(std::mem::take(&mut section.blocks));
        section.blocks = surviving;
        extracted.extend(nested);
    }
    let closed = pass.build_appendix(sections, extracted);
    BobVisibilityOutcome {
        closed,
        omitted_closed_tasks: pass.omitted_closed,
        omitted_self_snapshot_tasks: pass.omitted_self,
        schedule_logs: pass.schedule_logs,
        work_logs: pass.work_logs,
        warnings: pass.warnings,
    }
}

struct VisibilityPass<'a, L: SourceLoader> {
    store: &'a mut SourceStore<L>,
    notes: VaultNotes,
    root: &'a SourceIdentity,
    context: &'a BobVisibilityContext,
    omitted_closed: u64,
    omitted_self: u64,
    schedule_logs: u64,
    work_logs: u64,
    warnings: Vec<SnapshotWarning>,
    /// Canonical path of a referenced note → resolved `source_note` path.
    /// `None` means the note is not a confirmed self-snapshot source.
    source_note_of: HashMap<PathBuf, Option<PathBuf>>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum LogKind {
    Schedule,
    Work,
    Cancel,
}

impl<'a, L: SourceLoader> VisibilityPass<'a, L> {
    fn convert_logs_in_blocks(&mut self, blocks: &mut [Block]) {
        for block in blocks {
            match block {
                Block::Task(task) => self.convert_task(task),
                Block::List { items, .. } => {
                    for item in items {
                        self.convert_logs_in_inlines(&mut item.content);
                        self.convert_logs_in_blocks(&mut item.children);
                    }
                }
                Block::Paragraph(inlines) | Block::EmptyState(inlines) => {
                    self.convert_logs_in_inlines(inlines);
                }
                Block::Heading(heading) => {
                    self.convert_logs_in_inlines(&mut heading.content);
                }
                Block::LabeledRow { label, items } => {
                    self.convert_logs_in_inlines(label);
                    for item in items {
                        self.convert_logs_in_inlines(item);
                    }
                }
                Block::LogRow {
                    label,
                    date,
                    content,
                    ..
                } => {
                    self.convert_logs_in_inlines(label);
                    if let Some(date) = date {
                        self.convert_logs_in_inlines(date);
                    }
                    self.convert_logs_in_inlines(content);
                }
                Block::MissingImage { label } => {
                    self.convert_logs_in_inlines(label);
                }
                Block::Embed { blocks, .. }
                | Block::LiveQuery { blocks }
                | Block::Compact { blocks } => {
                    self.convert_logs_in_blocks(blocks);
                }
                Block::Callout { title, blocks, .. } => {
                    self.convert_logs_in_inlines(title);
                    self.convert_logs_in_blocks(blocks);
                }
                _ => {}
            }
        }
    }

    fn convert_logs_in_inlines(&mut self, inlines: &mut [Inline]) {
        for inline in inlines.iter_mut() {
            match inline {
                Inline::EmbeddedBlock(inner) => {
                    let mut nested = vec![std::mem::replace(
                        inner.as_mut(),
                        Block::Paragraph(Vec::new()),
                    )];
                    self.convert_logs_in_blocks(&mut nested);
                    *inner = Box::new(
                        nested.into_iter().next().expect("embedded block"),
                    );
                }
                Inline::Link { label, .. }
                | Inline::Emphasis(label)
                | Inline::Strong(label)
                | Inline::Strike(label)
                | Inline::Muted(label) => {
                    self.convert_logs_in_inlines(label);
                }
                Inline::Chip(chip) => {
                    self.convert_logs_in_inlines(&mut chip.content);
                }
                _ => {}
            }
        }
    }

    fn convert_task(&mut self, task: &mut SnapshotTask) {
        self.convert_logs_in_inlines(&mut task.content);
        for chip in task.chips.iter_mut() {
            self.convert_logs_in_inlines(&mut chip.content);
        }
        self.add_closed_date_chips(task);
        self.convert_task_logs(task);
        self.convert_logs_in_blocks(&mut task.children);
    }

    fn is_root_outcome(&self, task: &SnapshotTask) -> bool {
        task.source.as_ref().is_some_and(|source| {
            source.source == *self.root
                && crate::native::collect_done::trailing_block_id_in_line(
                    source.semantic_line(),
                )
                .as_deref()
                    == Some("prj")
        })
    }

    fn add_closed_date_chips(&self, task: &mut SnapshotTask) {
        if !is_closed(task.status) {
            return;
        }
        let Some(source) = task.source.as_ref() else {
            return;
        };
        let raw = source.semantic_line();
        let code = inline_code_spans(raw);
        let mut consumed = Vec::new();
        if let Some((field, date)) = first_date_field(raw, "completion", &code)
        {
            task.chips.push(date_chip(
                "done",
                date,
                self.context.snapshot_date,
            ));
            consumed.push(format!("completion: {}", field));
        }
        if let Some((field, date)) = first_date_field(raw, "cancelled", &code) {
            task.chips.push(date_chip(
                "canceled",
                date,
                self.context.snapshot_date,
            ));
            consumed.push(format!("cancelled: {}", field));
        }
        if !consumed.is_empty() {
            remove_consumed_muted(&mut task.content, &consumed);
        }
    }

    fn convert_task_logs(&mut self, task: &mut SnapshotTask) {
        let closed = is_closed(task.status);
        let mut output = Vec::with_capacity(task.children.len());
        let mut chips = Vec::new();
        for child in std::mem::take(&mut task.children) {
            match child {
                Block::List { kind, items } => {
                    let mut run = Vec::new();
                    for item in items {
                        if let Some(log_kind) = log_kind_for_item(&item) {
                            if !run.is_empty() {
                                output.push(Block::List {
                                    kind,
                                    items: std::mem::take(&mut run),
                                });
                            }
                            let (more_chips, rows) =
                                self.convert_log(log_kind, item, closed);
                            chips.extend(more_chips);
                            output.extend(rows);
                        } else {
                            run.push(item);
                        }
                    }
                    if !run.is_empty() {
                        output.push(Block::List { kind, items: run });
                    }
                }
                other => output.push(other),
            }
        }
        task.chips.extend(chips);
        task.children = output;
    }

    fn convert_log(
        &mut self,
        kind: LogKind,
        item: ListItem,
        closed: bool,
    ) -> (Vec<Chip>, Vec<Block>) {
        let entries = log_entry_items(&item);
        match kind {
            LogKind::Schedule => {
                let n = entries.len() as u64;
                if n == 0 {
                    return (Vec::new(), Vec::new());
                }
                self.schedule_logs += n;
                if self.context.include_closed {
                    (
                        Vec::new(),
                        full_log_rows(
                            "MOVED",
                            &entries,
                            self.context.snapshot_date,
                        ),
                    )
                } else {
                    (
                        vec![Chip {
                            content: vec![Inline::Text(format!("moved {n}×"))],
                            kind: ChipKind::Normal,
                            target: None,
                        }],
                        Vec::new(),
                    )
                }
            }
            LogKind::Work => {
                if entries.is_empty() {
                    return (Vec::new(), Vec::new());
                }
                self.work_logs += entries.len() as u64;
                if self.context.include_closed {
                    (
                        Vec::new(),
                        full_log_rows(
                            "WORKED",
                            &entries,
                            self.context.snapshot_date,
                        ),
                    )
                } else {
                    (
                        Vec::new(),
                        condensed_work_row(
                            &entries,
                            self.context.snapshot_date,
                        ),
                    )
                }
            }
            LogKind::Cancel => {
                if !closed || entries.is_empty() {
                    return (Vec::new(), Vec::new());
                }
                if self.context.include_closed {
                    (
                        Vec::new(),
                        full_log_rows(
                            "CANCELED",
                            &entries,
                            self.context.snapshot_date,
                        ),
                    )
                } else {
                    (Vec::new(), Vec::new())
                }
            }
        }
    }

    fn filter_blocks(
        &mut self,
        blocks: Vec<Block>,
    ) -> (Vec<Block>, Vec<Block>) {
        let mut surviving = Vec::with_capacity(blocks.len());
        let mut extracted = Vec::new();
        for block in blocks {
            match block {
                Block::Task(task) => {
                    let (keep, nested) = self.filter_task(task);
                    if let Some(task) = keep {
                        surviving.push(Block::Task(task));
                    }
                    extracted.extend(nested);
                }
                Block::Heading(heading) if is_closed_lane_heading(&heading) => {
                }
                Block::Heading(mut heading) => {
                    let (content, nested) =
                        self.filter_inlines(heading.content);
                    heading.content = content;
                    extracted.extend(nested);
                    surviving.push(Block::Heading(heading));
                }
                Block::Paragraph(inlines) => {
                    let (inlines, nested) = self.filter_inlines(inlines);
                    extracted.extend(nested);
                    surviving.push(Block::Paragraph(inlines));
                }
                Block::EmptyState(inlines) => {
                    let (inlines, nested) = self.filter_inlines(inlines);
                    extracted.extend(nested);
                    surviving.push(Block::EmptyState(inlines));
                }
                Block::List { kind, items } => {
                    let mut kept_items = Vec::new();
                    for mut item in items {
                        let (content, mut nested) =
                            self.filter_inlines(item.content);
                        item.content = content;
                        extracted.extend(nested.drain(..));
                        let (children, nested) =
                            self.filter_blocks(item.children);
                        item.children = children;
                        kept_items.push(item);
                        extracted.extend(nested);
                    }
                    if !kept_items.is_empty() {
                        surviving.push(Block::List {
                            kind,
                            items: kept_items,
                        });
                    }
                }
                Block::LabeledRow { mut label, items } => {
                    let (label_filtered, mut nested) =
                        self.filter_inlines(label);
                    label = label_filtered;
                    extracted.extend(nested.drain(..));
                    let mut filtered_items = Vec::with_capacity(items.len());
                    for item in items {
                        let (item_filtered, nested) = self.filter_inlines(item);
                        extracted.extend(nested);
                        filtered_items.push(item_filtered);
                    }
                    surviving.push(Block::LabeledRow {
                        label,
                        items: filtered_items,
                    });
                }
                Block::LogRow {
                    mut label,
                    mut date,
                    mut content,
                    older_count,
                } => {
                    let (filtered, mut nested) = self.filter_inlines(label);
                    label = filtered;
                    extracted.extend(nested.drain(..));
                    if let Some(date_inlines) = date {
                        let (filtered, nested) =
                            self.filter_inlines(date_inlines);
                        extracted.extend(nested);
                        date = Some(filtered);
                    }
                    let (filtered, nested) = self.filter_inlines(content);
                    content = filtered;
                    extracted.extend(nested);
                    surviving.push(Block::LogRow {
                        label,
                        date,
                        content,
                        older_count,
                    });
                }
                Block::MissingImage { label } => {
                    let (label, nested) = self.filter_inlines(label);
                    extracted.extend(nested);
                    surviving.push(Block::MissingImage { label });
                }
                Block::Embed {
                    source,
                    blocks: inner,
                } => {
                    let (blocks, nested) = self.filter_blocks(inner);
                    surviving.push(Block::Embed { source, blocks });
                    extracted.extend(nested);
                }
                Block::LiveQuery { blocks: inner } => {
                    let (blocks, nested) = self.filter_blocks(inner);
                    surviving.push(Block::LiveQuery { blocks });
                    extracted.extend(nested);
                }
                Block::Compact { blocks: inner } => {
                    let (blocks, nested) = self.filter_blocks(inner);
                    surviving.push(Block::Compact { blocks });
                    extracted.extend(nested);
                }
                Block::Callout {
                    kind,
                    title,
                    blocks: inner,
                } => {
                    let (title, nested) = self.filter_inlines(title);
                    extracted.extend(nested);
                    let (blocks, nested) = self.filter_blocks(inner);
                    surviving.push(Block::Callout {
                        kind,
                        title,
                        blocks,
                    });
                    extracted.extend(nested);
                }
                other => surviving.push(other),
            }
        }
        (surviving, extracted)
    }

    fn filter_inlines(
        &mut self,
        inlines: Vec<Inline>,
    ) -> (Vec<Inline>, Vec<Block>) {
        let mut surviving = Vec::with_capacity(inlines.len());
        let mut extracted = Vec::new();
        for mut inline in inlines {
            match &mut inline {
                Inline::EmbeddedBlock(inner) => {
                    let original = std::mem::replace(
                        inner.as_mut(),
                        Block::Paragraph(Vec::new()),
                    );
                    let (kept, nested) = self.filter_single_block(original);
                    if let Some(kept) = kept {
                        **inner = kept;
                    } else {
                        // Keep surrounding prose intact: an omitted inline
                        // embed leaves an empty placeholder, never drops
                        // the host paragraph.
                        **inner = Block::Paragraph(Vec::new());
                    }
                    surviving.push(inline);
                    extracted.extend(nested);
                }
                Inline::Link { label, .. }
                | Inline::Emphasis(label)
                | Inline::Strong(label)
                | Inline::Strike(label)
                | Inline::Muted(label) => {
                    let (filtered, nested) =
                        self.filter_inlines(std::mem::take(label));
                    *label = filtered;
                    surviving.push(inline);
                    extracted.extend(nested);
                }
                Inline::Chip(chip) => {
                    let (filtered, nested) =
                        self.filter_inlines(std::mem::take(&mut chip.content));
                    chip.content = filtered;
                    surviving.push(inline);
                    extracted.extend(nested);
                }
                _ => surviving.push(inline),
            }
        }
        (surviving, extracted)
    }

    fn filter_single_block(
        &mut self,
        block: Block,
    ) -> (Option<Block>, Vec<Block>) {
        match block {
            Block::Task(task) => {
                let (keep, nested) = self.filter_task(task);
                (keep.map(Block::Task), nested)
            }
            Block::Embed { source, blocks } => {
                let (blocks, nested) = self.filter_blocks(blocks);
                (Some(Block::Embed { source, blocks }), nested)
            }
            Block::Paragraph(inlines) => {
                let (inlines, nested) = self.filter_inlines(inlines);
                (Some(Block::Paragraph(inlines)), nested)
            }
            other => {
                let (surviving, nested) = self.filter_blocks(vec![other]);
                let kept = surviving.into_iter().next();
                (kept, nested)
            }
        }
    }

    fn filter_task(
        &mut self,
        task: SnapshotTask,
    ) -> (Option<SnapshotTask>, Vec<Block>) {
        // The promoted root outcome never omits: its real status drives
        // log conversion, and the assembler removes the row after this
        // pass while keeping its history. Counts exclude the row itself.
        if self.is_root_outcome(&task) {
            let mut task = task;
            let (content, nested_content) = self.filter_inlines(task.content);
            task.content = content;
            let mut nested_all = nested_content;
            let mut filtered_chips = Vec::with_capacity(task.chips.len());
            for mut chip in task.chips {
                let (filtered, nested) = self.filter_inlines(chip.content);
                chip.content = filtered;
                filtered_chips.push(chip);
                nested_all.extend(nested);
            }
            task.chips = filtered_chips;
            let (children, nested) = self.filter_blocks(task.children);
            task.children = children;
            nested_all.extend(nested);
            return (Some(task), nested_all);
        }
        if self.is_self_snapshot(&task) {
            self.omitted_self += 1;
            self.account_omitted_descendants(&task.children);
            return (None, Vec::new());
        }
        if is_closed(task.status) {
            if self.context.include_closed {
                let task = self.strip_self_snapshot_tree(task);
                (None, vec![Block::Task(task)])
            } else {
                self.omitted_closed += 1;
                self.account_omitted_descendants(&task.children);
                (None, Vec::new())
            }
        } else {
            let mut task = task;
            let (content, mut nested_all) = self.filter_inlines(task.content);
            task.content = content;
            let mut filtered_chips = Vec::with_capacity(task.chips.len());
            for mut chip in task.chips {
                let (filtered, nested) = self.filter_inlines(chip.content);
                chip.content = filtered;
                filtered_chips.push(chip);
                nested_all.extend(nested);
            }
            task.chips = filtered_chips;
            let (children, nested) = self.filter_blocks(task.children);
            task.children = children;
            nested_all.extend(nested);
            (Some(task), nested_all)
        }
    }

    /// Count every omitted descendant occurrence with self-snapshot
    /// precedence, mirroring `filter_task` rules. Open non-self descendants
    /// are omitted but uncounted; their own children still count.
    fn account_omitted_descendants(&mut self, blocks: &[Block]) {
        for block in blocks {
            match block {
                Block::Task(task) => {
                    if self.is_self_snapshot(task) {
                        self.omitted_self += 1;
                    } else if is_closed(task.status) {
                        self.omitted_closed += 1;
                    }
                    self.account_omitted_descendants(&task.children);
                    self.account_omitted_inlines(&task.content);
                    for chip in &task.chips {
                        self.account_omitted_inlines(&chip.content);
                    }
                }
                Block::List { items, .. } => {
                    for item in items {
                        self.account_omitted_inlines(&item.content);
                        self.account_omitted_descendants(&item.children);
                    }
                }
                Block::Paragraph(inlines) | Block::EmptyState(inlines) => {
                    self.account_omitted_inlines(inlines);
                }
                Block::Heading(heading) => {
                    self.account_omitted_inlines(&heading.content);
                }
                Block::LabeledRow { label, items } => {
                    self.account_omitted_inlines(label);
                    for item in items {
                        self.account_omitted_inlines(item);
                    }
                }
                Block::LogRow {
                    label,
                    date,
                    content,
                    ..
                } => {
                    self.account_omitted_inlines(label);
                    if let Some(date) = date {
                        self.account_omitted_inlines(date);
                    }
                    self.account_omitted_inlines(content);
                }
                Block::MissingImage { label } => {
                    self.account_omitted_inlines(label);
                }
                Block::Embed { blocks, .. }
                | Block::LiveQuery { blocks }
                | Block::Compact { blocks } => {
                    self.account_omitted_descendants(blocks);
                }
                Block::Callout { title, blocks, .. } => {
                    self.account_omitted_inlines(title);
                    self.account_omitted_descendants(blocks);
                }
                _ => {}
            }
        }
    }

    fn account_omitted_inlines(&mut self, inlines: &[Inline]) {
        for inline in inlines {
            match inline {
                Inline::EmbeddedBlock(inner) => {
                    self.account_omitted_descendants(std::slice::from_ref(
                        inner.as_ref(),
                    ));
                }
                Inline::Link { label, .. }
                | Inline::Emphasis(label)
                | Inline::Strong(label)
                | Inline::Strike(label)
                | Inline::Muted(label) => {
                    self.account_omitted_inlines(label);
                }
                Inline::Chip(chip) => {
                    self.account_omitted_inlines(&chip.content);
                }
                _ => {}
            }
        }
    }

    /// Drop self-snapshot descendants inside a restored closed tree so
    /// they never appear in the appendix. Nested closed tasks stay nested.
    fn strip_self_snapshot_tree(
        &mut self,
        mut task: SnapshotTask,
    ) -> SnapshotTask {
        task.children =
            self.strip_self_blocks(std::mem::take(&mut task.children));
        task
    }

    fn strip_self_blocks(&mut self, blocks: Vec<Block>) -> Vec<Block> {
        let mut output = Vec::with_capacity(blocks.len());
        for block in blocks {
            match block {
                Block::Task(mut task) => {
                    if self.is_self_snapshot(&task) {
                        self.omitted_self += 1;
                        self.account_omitted_descendants(&task.children);
                        self.account_omitted_inlines(&task.content);
                        for chip in &task.chips {
                            self.account_omitted_inlines(&chip.content);
                        }
                        continue;
                    }
                    task.content = self.strip_self_inlines(task.content);
                    let mut stripped_chips =
                        Vec::with_capacity(task.chips.len());
                    for mut chip in task.chips {
                        chip.content = self.strip_self_inlines(chip.content);
                        stripped_chips.push(chip);
                    }
                    task.chips = stripped_chips;
                    task.children = self
                        .strip_self_blocks(std::mem::take(&mut task.children));
                    output.push(Block::Task(task));
                }
                Block::Paragraph(inlines) => {
                    output.push(Block::Paragraph(
                        self.strip_self_inlines(inlines),
                    ));
                }
                Block::EmptyState(inlines) => {
                    output.push(Block::EmptyState(
                        self.strip_self_inlines(inlines),
                    ));
                }
                Block::Heading(mut heading) => {
                    heading.content = self.strip_self_inlines(heading.content);
                    output.push(Block::Heading(heading));
                }
                Block::List { kind, items } => {
                    let items = items
                        .into_iter()
                        .map(|mut item| {
                            item.content =
                                self.strip_self_inlines(item.content);
                            item.children =
                                self.strip_self_blocks(item.children);
                            item
                        })
                        .collect();
                    output.push(Block::List { kind, items });
                }
                Block::LabeledRow { label, items } => {
                    let label = self.strip_self_inlines(label);
                    let items = items
                        .into_iter()
                        .map(|item| self.strip_self_inlines(item))
                        .collect();
                    output.push(Block::LabeledRow { label, items });
                }
                Block::LogRow {
                    label,
                    date,
                    content,
                    older_count,
                } => {
                    let label = self.strip_self_inlines(label);
                    let date = date.map(|date| self.strip_self_inlines(date));
                    let content = self.strip_self_inlines(content);
                    output.push(Block::LogRow {
                        label,
                        date,
                        content,
                        older_count,
                    });
                }
                Block::MissingImage { label } => {
                    output.push(Block::MissingImage {
                        label: self.strip_self_inlines(label),
                    });
                }
                Block::Embed {
                    source,
                    blocks: inner,
                } => output.push(Block::Embed {
                    source,
                    blocks: self.strip_self_blocks(inner),
                }),
                Block::LiveQuery { blocks: inner } => {
                    output.push(Block::LiveQuery {
                        blocks: self.strip_self_blocks(inner),
                    });
                }
                Block::Compact { blocks: inner } => {
                    output.push(Block::Compact {
                        blocks: self.strip_self_blocks(inner),
                    });
                }
                Block::Callout {
                    kind,
                    title,
                    blocks: inner,
                } => output.push(Block::Callout {
                    kind,
                    title: self.strip_self_inlines(title),
                    blocks: self.strip_self_blocks(inner),
                }),
                other => output.push(other),
            }
        }
        output
    }

    fn strip_self_inlines(&mut self, inlines: Vec<Inline>) -> Vec<Inline> {
        let mut output = Vec::with_capacity(inlines.len());
        for mut inline in inlines {
            match &mut inline {
                Inline::EmbeddedBlock(inner) => {
                    let original = std::mem::replace(
                        inner.as_mut(),
                        Block::Paragraph(Vec::new()),
                    );
                    let stripped = self.strip_single_block(original);
                    if let Some(stripped) = stripped {
                        **inner = stripped;
                        output.push(inline);
                    } else {
                        **inner = Block::Paragraph(Vec::new());
                        output.push(inline);
                    }
                }
                Inline::Link { label, .. }
                | Inline::Emphasis(label)
                | Inline::Strong(label)
                | Inline::Strike(label)
                | Inline::Muted(label) => {
                    let filtered =
                        self.strip_self_inlines(std::mem::take(label));
                    *label = filtered;
                    output.push(inline);
                }
                Inline::Chip(chip) => {
                    chip.content = self
                        .strip_self_inlines(std::mem::take(&mut chip.content));
                    output.push(inline);
                }
                _ => output.push(inline),
            }
        }
        output
    }

    fn strip_single_block(&mut self, block: Block) -> Option<Block> {
        match block {
            Block::Task(task) => {
                if self.is_self_snapshot(&task) {
                    self.omitted_self += 1;
                    self.account_omitted_descendants(&task.children);
                    return None;
                }
                let mut task = task;
                task.children =
                    self.strip_self_blocks(std::mem::take(&mut task.children));
                Some(Block::Task(task))
            }
            Block::Embed { source, blocks } => Some(Block::Embed {
                source,
                blocks: self.strip_self_blocks(blocks),
            }),
            Block::Paragraph(inlines) => {
                Some(Block::Paragraph(self.strip_self_inlines(inlines)))
            }
            other => {
                let stripped = self.strip_self_blocks(vec![other]);
                stripped.into_iter().next()
            }
        }
    }

    fn is_self_snapshot(&mut self, task: &SnapshotTask) -> bool {
        let Some(source) = task.source.as_ref() else {
            return false;
        };
        let raw = source.semantic_line();
        let code = inline_code_spans(raw);
        if !has_ref_tag(raw, &code) {
            return false;
        }
        let origin = SourceLocation {
            source: source.source.clone(),
            line: Some(source.line),
        };
        let mut confirmed = false;
        for span in wikilink_spans_in_line(raw) {
            if overlaps_code(span.start, span.end, &code) {
                continue;
            }
            if span.end < 4 || span.start + 2 > span.end - 2 {
                continue;
            }
            let inner = &raw[span.start + 2..span.end - 2];
            let parsed = parse_link_target(inner);
            let base = Path::new(&source.source.vault_relative_path);
            match self.notes.resolve_note(base, &parsed) {
                Ok(resolved) => {
                    if let Some(source_note) = self.lookup_source_note(
                        &resolved.canonical_path,
                        &resolved.vault_relative_path,
                        &origin,
                    ) && source_note == self.root.canonical_path
                    {
                        confirmed = true;
                        break;
                    }
                }
                Err(error) => {
                    let kind = match &error {
                        super::resolve::NoteResolveError::Missing {
                            ..
                        } => SnapshotWarningKind::UnresolvedLink,
                        super::resolve::NoteResolveError::Ambiguous {
                            ..
                        } => SnapshotWarningKind::AmbiguousLink,
                    };
                    self.warn(kind, Some(origin.clone()), error.to_string());
                }
            }
        }
        confirmed
    }

    fn lookup_source_note(
        &mut self,
        ref_canonical: &Path,
        ref_relative: &Path,
        origin: &SourceLocation,
    ) -> Option<PathBuf> {
        if let Some(cached) = self.source_note_of.get(ref_canonical) {
            return cached.clone();
        }
        let resolved =
            self.read_source_note(ref_canonical, ref_relative, origin);
        self.source_note_of
            .insert(ref_canonical.to_path_buf(), resolved.clone());
        resolved
    }

    fn read_source_note(
        &mut self,
        ref_canonical: &Path,
        ref_relative: &Path,
        origin: &SourceLocation,
    ) -> Option<PathBuf> {
        let bytes = match self.store.read_canonical(ref_canonical) {
            Ok(bytes) => bytes,
            Err(message) => {
                self.warn(
                    SnapshotWarningKind::ReadFailure,
                    Some(origin.clone()),
                    message,
                );
                return None;
            }
        };
        let text = String::from_utf8_lossy(&bytes);
        let frontmatter = parse_frontmatter(&text)?;
        let raw = frontmatter_value(&frontmatter, "source_note")?;
        let value = trim_yaml_scalar(raw).trim();
        if value.is_empty() {
            return None;
        }
        let parsed = parse_link_target(value);
        if parsed.note.is_empty() {
            self.warn(
                SnapshotWarningKind::InvalidSource,
                Some(origin.clone()),
                format!("malformed source_note {value:?}"),
            );
            return None;
        }
        match self.notes.resolve_note(ref_relative, &parsed) {
            Ok(resolved) => Some(resolved.canonical_path),
            Err(error) => {
                let kind = match &error {
                    super::resolve::NoteResolveError::Missing { .. } => {
                        SnapshotWarningKind::UnresolvedLink
                    }
                    super::resolve::NoteResolveError::Ambiguous { .. } => {
                        SnapshotWarningKind::AmbiguousLink
                    }
                };
                self.warn(kind, Some(origin.clone()), error.to_string());
                None
            }
        }
    }

    fn warn(
        &mut self,
        kind: SnapshotWarningKind,
        origin: Option<SourceLocation>,
        message: String,
    ) {
        self.warnings.push(SnapshotWarning {
            kind,
            origin,
            message,
        });
    }

    fn build_appendix(
        &self,
        sections: &[SnapshotSection],
        extracted: Vec<Block>,
    ) -> Option<SnapshotSection> {
        if !self.context.include_closed || extracted.is_empty() {
            return None;
        }
        let mut used = BTreeSet::new();
        for section in sections {
            used.insert(section.heading.anchor.id.clone());
            collect_block_anchor_ids(&section.blocks, &mut used);
        }
        let id = claim_anchor_id(&mut used, "closed-tasks");
        Some(SnapshotSection {
            source: self.root.clone(),
            heading: SnapshotHeading {
                level: HeadingLevel::H1,
                content: vec![Inline::Text("Closed tasks".to_string())],
                class: HeadingClass::Appendix,
                anchor: SnapshotAnchor {
                    identity: AnchorIdentity {
                        source: self.root.clone(),
                        kind: AnchorKind::Heading,
                        fragment: "Closed tasks".to_string(),
                    },
                    id,
                },
            },
            blocks: vec![Block::Compact { blocks: extracted }],
        })
    }
}

fn is_closed(status: SnapshotTaskStatus) -> bool {
    matches!(
        status,
        SnapshotTaskStatus::Done | SnapshotTaskStatus::Canceled
    )
}

fn is_closed_lane_heading(heading: &SnapshotHeading) -> bool {
    flatten_text(&heading.content).trim() == TITLE_CLOSED
}

fn log_kind_for_item(item: &ListItem) -> Option<LogKind> {
    let raw = item.source.as_ref()?.semantic_line();
    if is_cancel_log_line(raw) {
        return Some(LogKind::Cancel);
    }
    match parse_managed_task_log_marker(raw) {
        Some(ManagedTaskLogKind::Schedule) => Some(LogKind::Schedule),
        Some(ManagedTaskLogKind::Work) => Some(LogKind::Work),
        None => None,
    }
}

/// Direct entry bullets under a log marker, skipping nested reason children.
fn log_entry_items(item: &ListItem) -> Vec<&ListItem> {
    let mut entries = Vec::new();
    for child in &item.children {
        if let Block::List { items, .. } = child {
            entries.extend(items.iter());
        }
    }
    entries
}

fn condensed_work_row(
    entries: &[&ListItem],
    snapshot_date: NaiveDate,
) -> Vec<Block> {
    let Some(newest) = entries.first() else {
        return Vec::new();
    };
    let older = (entries.len() as u64).saturating_sub(1);
    let mut row = entry_log_row("WORKED", newest, snapshot_date);
    row.older_count = (older > 0).then_some(older);
    let mut blocks = vec![Block::LogRow {
        label: row.label,
        date: row.date,
        content: row.content,
        older_count: row.older_count,
    }];
    blocks.extend(newest.children.iter().cloned());
    blocks
}

fn full_log_rows(
    label: &str,
    entries: &[&ListItem],
    snapshot_date: NaiveDate,
) -> Vec<Block> {
    let mut blocks = Vec::new();
    for entry in entries {
        let row = entry_log_row(label, entry, snapshot_date);
        blocks.push(Block::LogRow {
            label: row.label,
            date: row.date,
            content: row.content,
            older_count: None,
        });
        blocks.extend(entry.children.iter().cloned());
    }
    blocks
}

struct ParsedLogRow {
    label: Vec<Inline>,
    date: Option<Vec<Inline>>,
    content: Vec<Inline>,
    older_count: Option<u64>,
}

fn entry_log_row(
    label: &str,
    entry: &ListItem,
    snapshot_date: NaiveDate,
) -> ParsedLogRow {
    let (date, content) =
        split_log_inlines(&entry.content, snapshot_date, entry);
    ParsedLogRow {
        label: vec![Inline::Text(label.to_string())],
        date,
        content,
        older_count: None,
    }
}

fn split_log_inlines(
    content: &[Inline],
    snapshot_date: NaiveDate,
    entry: &ListItem,
) -> (Option<Vec<Inline>>, Vec<Inline>) {
    if let Some(raw) = entry.source.as_ref()
        && let Some(body) = list_item_body(raw.semantic_line())
        && let Some((inner, _)) = parse_emphasized_entry(body)
    {
        let date =
            vec![Inline::Text(format_schedule_inner(inner, snapshot_date))];
        let content = trailing_inlines_after_separator(content);
        return (Some(date), content);
    }
    trailing_date_and_content(content)
}

fn trailing_date_and_content(
    content: &[Inline],
) -> (Option<Vec<Inline>>, Vec<Inline>) {
    let mut index = 0;
    while index < content.len() {
        if let Inline::Text(text) = &content[index]
            && text.trim().is_empty()
        {
            index += 1;
            continue;
        }
        break;
    }
    let date = match content.get(index) {
        Some(Inline::Emphasis(inner) | Inline::Strong(inner)) => {
            index += 1;
            Some(inner.clone())
        }
        _ => None,
    };
    (date, strip_leading_separator(&content[index..]))
}

fn trailing_inlines_after_separator(content: &[Inline]) -> Vec<Inline> {
    let (_, rest) = trailing_date_and_content(content);
    rest
}

fn strip_leading_separator(content: &[Inline]) -> Vec<Inline> {
    if content.is_empty() {
        return Vec::new();
    }
    if let Inline::Text(text) = &content[0] {
        let trimmed = text.trim_start();
        let rest = trimmed
            .strip_prefix('—')
            .or_else(|| trimmed.strip_prefix('-'))
            .unwrap_or(trimmed)
            .trim_start();
        if rest.is_empty() {
            return content[1..].to_vec();
        }
        let mut out = vec![Inline::Text(rest.to_string())];
        out.extend(content[1..].iter().cloned());
        return out;
    }
    content.to_vec()
}

/// `*inner* — reason` or `_inner_ — reason`.
fn parse_emphasized_entry(body: &str) -> Option<(&str, &str)> {
    let body = body.trim();
    let (delim, rest) = if let Some(rest) = body.strip_prefix('*') {
        ('*', rest)
    } else {
        let rest = body.strip_prefix('_')?;
        ('_', rest)
    };
    let close = rest.find(delim)?;
    let inner = rest[..close].trim();
    let after = rest[close + delim.len_utf8()..].trim_start();
    let reason = after
        .strip_prefix('—')
        .or_else(|| after.strip_prefix('-'))
        .unwrap_or(after)
        .trim_start();
    Some((inner, reason))
}

fn format_schedule_inner(inner: &str, snapshot_date: NaiveDate) -> String {
    if let Some((from, to)) = inner.split_once('→') {
        format!(
            "{} → {}",
            format_maybe_date(from.trim(), snapshot_date),
            format_maybe_date(to.trim(), snapshot_date)
        )
    } else {
        format_maybe_date(inner, snapshot_date)
    }
}

fn format_maybe_date(text: &str, snapshot_date: NaiveDate) -> String {
    parse_strict_calendar_date(text)
        .map(|date| format_display_date(date, snapshot_date))
        .unwrap_or_else(|| text.to_string())
}

fn format_display_date(date: NaiveDate, snapshot_date: NaiveDate) -> String {
    let day = format!("{} {}", date.format("%b"), date.day());
    if date.year() == snapshot_date.year() {
        day
    } else {
        format!("{day}, {}", date.year())
    }
}

fn first_date_field(
    raw: &str,
    key: &str,
    code: &[Range<usize>],
) -> Option<(String, NaiveDate)> {
    inline_fields(raw, key)
        .into_iter()
        .filter(|field| !overlaps_code(field.start, field.end, code))
        .find_map(|field| {
            parse_strict_calendar_date(&field.value)
                .map(|date| (field.value, date))
        })
}

fn date_chip(prefix: &str, date: NaiveDate, snapshot_date: NaiveDate) -> Chip {
    Chip {
        content: vec![Inline::Text(format!(
            "{prefix} {}",
            format_display_date(date, snapshot_date)
        ))],
        kind: ChipKind::Normal,
        target: None,
    }
}

fn overlaps_code(start: usize, end: usize, code: &[Range<usize>]) -> bool {
    code.iter().any(|span| start < span.end && end > span.start)
}

fn flatten_text(inlines: &[Inline]) -> String {
    let mut out = String::new();
    for inline in inlines {
        match inline {
            Inline::Text(text) | Inline::Code(text) => out.push_str(text),
            Inline::Link { label, .. }
            | Inline::Emphasis(label)
            | Inline::Strong(label)
            | Inline::Strike(label)
            | Inline::Muted(label) => out.push_str(&flatten_text(label)),
            Inline::Chip(chip) => out.push_str(&flatten_text(&chip.content)),
            Inline::Anchor(_)
            | Inline::Image { .. }
            | Inline::Break
            | Inline::EmbeddedBlock(_) => {}
        }
    }
    out
}

fn collect_block_anchor_ids(blocks: &[Block], used: &mut BTreeSet<String>) {
    for block in blocks {
        match block {
            Block::Heading(heading) => {
                used.insert(heading.anchor.id.clone());
                collect_inline_anchor_ids(&heading.content, used);
            }
            Block::Task(task) => {
                if let Some(anchor) = &task.anchor {
                    used.insert(anchor.id.clone());
                }
                collect_inline_anchor_ids(&task.content, used);
                collect_block_anchor_ids(&task.children, used);
            }
            Block::List { items, .. } => {
                for item in items {
                    collect_inline_anchor_ids(&item.content, used);
                    collect_block_anchor_ids(&item.children, used);
                }
            }
            Block::Paragraph(inlines) | Block::EmptyState(inlines) => {
                collect_inline_anchor_ids(inlines, used);
            }
            Block::LabeledRow { label, items } => {
                collect_inline_anchor_ids(label, used);
                for item in items {
                    collect_inline_anchor_ids(item, used);
                }
            }
            Block::LogRow {
                label,
                date,
                content,
                ..
            } => {
                collect_inline_anchor_ids(label, used);
                if let Some(date) = date {
                    collect_inline_anchor_ids(date, used);
                }
                collect_inline_anchor_ids(content, used);
            }
            Block::Embed { blocks, .. }
            | Block::LiveQuery { blocks }
            | Block::Compact { blocks } => {
                collect_block_anchor_ids(blocks, used);
            }
            Block::Callout { title, blocks, .. } => {
                collect_inline_anchor_ids(title, used);
                collect_block_anchor_ids(blocks, used);
            }
            Block::MissingImage { label } => {
                collect_inline_anchor_ids(label, used);
            }
            Block::Code { .. } | Block::Image { .. } => {}
        }
    }
}

fn collect_inline_anchor_ids(inlines: &[Inline], used: &mut BTreeSet<String>) {
    for inline in inlines {
        match inline {
            Inline::Anchor(anchor) => {
                used.insert(anchor.id.clone());
            }
            Inline::Link { label, .. }
            | Inline::Emphasis(label)
            | Inline::Strong(label)
            | Inline::Strike(label)
            | Inline::Muted(label) => collect_inline_anchor_ids(label, used),
            Inline::Chip(chip) => {
                collect_inline_anchor_ids(&chip.content, used);
            }
            Inline::EmbeddedBlock(block) => {
                collect_block_anchor_ids(
                    std::slice::from_ref(block.as_ref()),
                    used,
                );
            }
            _ => {}
        }
    }
}

fn claim_anchor_id(used: &mut BTreeSet<String>, base: &str) -> String {
    if used.insert(base.to_string()) {
        return base.to_string();
    }
    let mut suffix = 2_u32;
    loop {
        let candidate = format!("{base}-{suffix}");
        if used.insert(candidate.clone()) {
            return candidate;
        }
        suffix += 1;
    }
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, BTreeSet};
    use std::fs;
    use std::path::PathBuf;

    use chrono::NaiveDate;

    use super::super::classify::{
        classify_target, sha256_hex, source_identity, CountingLoader,
        SnapshotTarget, SourceStore,
    };
    use super::super::serialize::serialize_projected_markdown;
    use super::super::transclusion::{
        project_snapshot, SnapshotProjection, SnapshotProjectionOptions,
    };
    use super::super::view::{
        Block, ChipKind, HeadingClass, Inline, SnapshotMasthead, SnapshotTask,
    };
    use super::*;
    use crate::native::task_status_hooks::{TaskStatusType, TasksSettings};

    fn snapshot_date() -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 10, 16).expect("test date")
    }

    fn settings() -> TasksSettings {
        TasksSettings {
            global_filter: "#task".to_string(),
            done_statuses: BTreeSet::from(['x', 'X']),
            status_types: BTreeMap::from([
                (' ', TaskStatusType::Todo),
                ('x', TaskStatusType::Done),
                ('X', TaskStatusType::Done),
                ('/', TaskStatusType::InProgress),
                ('*', TaskStatusType::OnHold),
                ('?', TaskStatusType::OnHold),
                ('-', TaskStatusType::Cancelled),
            ]),
            status_definitions: Vec::new(),
            status_settings_error: None,
        }
    }

    fn masthead() -> SnapshotMasthead {
        SnapshotMasthead {
            title: "Synthetic snapshot".to_string(),
            kicker: vec![Inline::Text("AREA".to_string())],
            display_title: vec![Inline::Text("Synthetic area".to_string())],
            dek: None,
            meta: Vec::new(),
            stats: Vec::new(),
            omitted: None,
            snapshot_line: vec![Inline::Text("Synthetic fixture".to_string())],
            footer: Vec::new(),
            glyph_table: "glyphs.lua".to_string(),
            return_links: false,
        }
    }

    fn vault(files: &[(&str, &str)]) -> tempfile::TempDir {
        let directory = tempfile::tempdir().expect("temp vault");
        for (relative, text) in files {
            let path = directory.path().join(relative);
            fs::create_dir_all(path.parent().expect("file parent"))
                .expect("create parent");
            fs::write(path, text).expect("write synthetic note");
        }
        directory
    }

    fn area(body: &str) -> String {
        format!("---\ntype: \"[[area]]\"\n---\n\n{body}")
    }

    fn project(
        directory: &tempfile::TempDir,
        host: &str,
        include_closed: bool,
    ) -> SnapshotProjection {
        let root = directory.path().canonicalize().expect("canonical vault");
        let mut store = SourceStore::open(&root).expect("source store");
        let display_path = root.join(host).to_string_lossy().into_owned();
        let note = match classify_target(&mut store, &display_path)
            .expect("classify synthetic note")
        {
            SnapshotTarget::VaultNote(note) => note,
            other => panic!("expected vault note, got {other:?}"),
        };
        project_snapshot(
            &mut store,
            &note,
            &settings(),
            SnapshotProjectionOptions {
                masthead: masthead(),
                snapshot_date: snapshot_date(),
                include_closed,
                consumed_title_heading: None,
            },
        )
        .expect("project snapshot")
    }

    fn tasks_in(blocks: &[Block]) -> Vec<&SnapshotTask> {
        let mut found = Vec::new();
        walk_blocks(blocks, &mut |block| {
            if let Block::Task(task) = block {
                found.push(task);
            }
        });
        found
    }

    fn walk_blocks<'a>(blocks: &'a [Block], visit: &mut impl FnMut(&'a Block)) {
        for block in blocks {
            visit(block);
            match block {
                Block::Task(task) => walk_blocks(&task.children, visit),
                Block::List { items, .. } => {
                    for item in items {
                        walk_blocks(&item.children, visit);
                    }
                }
                Block::Embed { blocks, .. }
                | Block::LiveQuery { blocks }
                | Block::Compact { blocks }
                | Block::Callout { blocks, .. } => walk_blocks(blocks, visit),
                _ => {}
            }
        }
    }

    fn body_blocks(projection: &SnapshotProjection) -> Vec<Block> {
        let mut blocks = projection.view.preamble.clone();
        blocks.extend(
            projection
                .view
                .sections
                .iter()
                .flat_map(|section| section.blocks.clone()),
        );
        blocks
    }

    fn all_blocks(projection: &SnapshotProjection) -> Vec<Block> {
        let mut blocks = body_blocks(projection);
        if let Some(closed) = &projection.view.closed {
            blocks.extend(closed.blocks.clone());
        }
        blocks
    }

    fn chip_texts(task: &SnapshotTask) -> Vec<String> {
        task.chips
            .iter()
            .map(|chip| {
                let kind = match chip.kind {
                    ChipKind::Normal => "",
                    ChipKind::Accent => "+",
                };
                format!("{kind}{}", flatten_text(&chip.content))
            })
            .collect()
    }

    fn log_rows(blocks: &[Block]) -> Vec<String> {
        let mut rows = Vec::new();
        walk_blocks(blocks, &mut |block| {
            if let Block::LogRow {
                label,
                date,
                content,
                older_count,
            } = block
            {
                let mut line = flatten_text(label);
                if let Some(date) = date {
                    line.push(' ');
                    line.push_str(&flatten_text(date));
                }
                let body = flatten_text(content);
                if !body.is_empty() {
                    line.push_str(" — ");
                    line.push_str(&body);
                }
                if let Some(count) = older_count {
                    line.push_str(&format!(" +{count} older"));
                }
                rows.push(line);
            }
        });
        rows
    }

    fn task_titles(blocks: &[Block]) -> Vec<String> {
        tasks_in(blocks)
            .into_iter()
            .map(|task| flatten_text(&task.content))
            .collect()
    }

    fn read_tree(root: &std::path::Path) -> BTreeMap<PathBuf, Vec<u8>> {
        let mut result = BTreeMap::new();
        let mut pending = vec![root.to_path_buf()];
        while let Some(directory) = pending.pop() {
            for entry in fs::read_dir(&directory).expect("read tree") {
                let path = entry.expect("entry").path();
                if path.is_dir() {
                    pending.push(path);
                } else {
                    result.insert(
                        path.clone(),
                        fs::read(&path).expect("read file"),
                    );
                }
            }
        }
        result
    }

    #[test]
    fn six_statuses_omit_closed_in_reading_mode() {
        let directory = vault(&[(
            "Host.md",
            &area(
                "- [ ] open #task ^open\n\
                 - [*] next #task ^next\n\
                 - [/] wip #task ^wip\n\
                 - [?] blocked #task ^blocked\n\
                 - [x] done #task ^done\n\
                 - [-] canceled #task ^canceled\n",
            ),
        )]);
        let projection = project(&directory, "Host.md", false);
        let blocks = body_blocks(&projection);
        let titles = task_titles(&blocks);
        assert_eq!(titles.len(), 4, "{titles:?}");
        assert!(titles.iter().any(|t| t.contains("open")), "{titles:?}");
        assert!(titles.iter().any(|t| t.contains("next")), "{titles:?}");
        assert!(titles.iter().any(|t| t.contains("wip")), "{titles:?}");
        assert!(titles.iter().any(|t| t.contains("blocked")), "{titles:?}");
        assert!(!titles.iter().any(|t| t.contains("done")), "{titles:?}");
        assert!(!titles.iter().any(|t| t.contains("canceled")), "{titles:?}");
        assert_eq!(projection.view.summary.omitted_closed_tasks, 2);
        assert!(projection.view.closed.is_none());
        let md =
            serialize_projected_markdown(&projection.view).expect("serialize");
        assert!(!md.contains("#t-done"), "{md}");
        assert!(!md.contains("#t-canceled"), "{md}");
    }

    #[test]
    fn include_closed_restores_appendix_with_date_chips() {
        let directory = vault(&[(
            "Host.md",
            &area(
                "- [ ] open #task ^open\n\
                 - [x] done #task [completion:: 2026-10-15] ^done\n\
                 - [-] canceled #task [cancelled:: 2026-10-14] ^canceled\n",
            ),
        )]);
        let projection = project(&directory, "Host.md", true);
        assert_eq!(projection.view.summary.omitted_closed_tasks, 0);
        let closed = projection.view.closed.as_ref().expect("appendix");
        assert_eq!(closed.heading.class, HeadingClass::Appendix);
        assert_eq!(flatten_text(&closed.heading.content), "Closed tasks");
        assert!(
            matches!(closed.blocks.as_slice(), [Block::Compact { .. }]),
            "{:?}",
            closed.blocks
        );
        let restored = tasks_in(&closed.blocks);
        assert_eq!(restored.len(), 2, "{restored:?}");
        assert_eq!(chip_texts(restored[0]), vec!["done Oct 15"]);
        assert_eq!(chip_texts(restored[1]), vec!["canceled Oct 14"]);
        let md =
            serialize_projected_markdown(&projection.view).expect("serialize");
        assert!(md.contains("#t-done"), "{md}");
        assert!(md.contains(".appendix"), "{md}");
    }

    #[test]
    fn nested_closed_do_not_escape_or_duplicate() {
        let directory = vault(&[(
            "Host.md",
            &area(
                "- [ ] Open parent #task ^open\n\
                 \t- [x] Closed child #task ^child\n\
                 \t\t- [ ] Open grandchild #task ^grand\n\
                 - [x] Closed parent #task ^parent\n\
                 \t- [x] Nested closed #task ^nested\n\
                 \t- [ ] Open under closed #task ^stuck\n",
            ),
        )]);
        let reading = project(&directory, "Host.md", false);
        let body = task_titles(&body_blocks(&reading));
        assert_eq!(body, vec!["Open parent"], "{body:?}");
        assert_eq!(reading.view.summary.omitted_closed_tasks, 3);
        assert!(reading.view.closed.is_none());

        let restored = project(&directory, "Host.md", true);
        let body = task_titles(&body_blocks(&restored));
        assert_eq!(body, vec!["Open parent"], "{body:?}");
        let appendix = restored.view.closed.as_ref().expect("appendix");
        let titles = task_titles(&appendix.blocks);
        assert_eq!(
            titles,
            vec![
                "Closed child",
                "Open grandchild",
                "Closed parent",
                "Nested closed",
                "Open under closed",
            ],
            "{titles:?}"
        );
        let child = tasks_in(&appendix.blocks)
            .into_iter()
            .find(|task| flatten_text(&task.content).contains("Closed child"))
            .expect("closed child");
        assert!(
            task_titles(&child.children)
                .iter()
                .any(|t| t.contains("Open grandchild")),
            "{:?}",
            child.children
        );
    }

    #[test]
    fn done_and_canceled_heading_drops_and_keeps_prose() {
        let directory = vault(&[(
            "Host.md",
            &area(
                "Intro stays.\n\n\
                 ### Done & Canceled\n\n\
                 - [x] Finished #task ^done\n\n\
                 Unrelated prose after the lane.\n",
            ),
        )]);
        let projection = project(&directory, "Host.md", false);
        let md =
            serialize_projected_markdown(&projection.view).expect("serialize");
        assert!(!md.contains("Done & Canceled"), "{md}");
        assert!(md.contains("Intro stays"), "{md}");
        assert!(md.contains("Unrelated prose after the lane"), "{md}");
        assert!(!md.contains("Finished"), "{md}");
    }

    #[test]
    fn schedule_and_work_logs_condense_and_count_entries() {
        let directory = vault(&[(
            "Host.md",
            &area(
                "- [ ] Logged #task ^logged\n\
                 \t- 🗓️ **SCHEDULE LOG**\n\
                 \t\t- *2026-10-01 → 2026-10-10* — first\n\
                 \t\t\t- extra reason detail\n\
                 \t\t- _2026-09-01 → 2026-10-01_ — second\n\
                 \t- 🛠️ **WORK LOG**\n\
                 \t\t- *2026-10-15* — newest **bold**\n\
                 \t\t- _2026-10-14_ — older\n",
            ),
        )]);
        let projection = project(&directory, "Host.md", false);
        let blocks = body_blocks(&projection);
        let tasks = tasks_in(&blocks);
        assert_eq!(tasks.len(), 1, "{blocks:?}");
        assert!(
            chip_texts(tasks[0]).iter().any(|c| c == "moved 2×"),
            "{:?}",
            chip_texts(tasks[0])
        );
        let logs = log_rows(&blocks);
        assert_eq!(logs.len(), 1, "{logs:?}");
        assert!(logs[0].contains("WORKED"), "{logs:?}");
        assert!(logs[0].contains("Oct 15"), "{logs:?}");
        assert!(logs[0].contains("newest"), "{logs:?}");
        assert!(logs[0].contains("+1 older"), "{logs:?}");
        assert_eq!(projection.view.summary.schedule_logs, 2);
        assert_eq!(projection.view.summary.work_logs, 2);
        let md =
            serialize_projected_markdown(&projection.view).expect("serialize");
        assert!(!md.contains("SCHEDULE LOG"), "{md}");
        assert!(!md.contains("WORK LOG"), "{md}");
        assert!(!md.contains("extra reason detail"), "{md}");
    }

    #[test]
    fn include_closed_prints_full_logs_on_open_and_closed_tasks() {
        let directory = vault(&[(
            "Host.md",
            &area(
                "- [ ] Open #task ^open\n\
                 \t- 🗓️ **SCHEDULE LOG**\n\
                 \t\t- *2026-10-01 → 2026-10-10* — moved\n\
                 \t- 🛠️ **WORK LOG**\n\
                 \t\t- *2026-10-15* — newest\n\
                 \t\t- *2026-10-14* — older\n\
                 - [-] Dropped #task [cancelled:: 2026-10-12] ^drop\n\
                 \t- ❌ **CANCEL LOG**\n\
                 \t\t- *2026-10-12* — superseded\n\
                 \t\t- *2026-10-11* — earlier\n",
            ),
        )]);
        let projection = project(&directory, "Host.md", true);
        let body = body_blocks(&projection);
        let body_logs = log_rows(&body);
        assert!(
            body_logs
                .iter()
                .any(|row| row.contains("MOVED") && row.contains("moved")),
            "{body_logs:?}"
        );
        assert!(
            body_logs
                .iter()
                .filter(|row| row.contains("WORKED"))
                .count()
                == 2,
            "{body_logs:?}"
        );
        assert!(
            !body_logs.iter().any(|row| row.contains("+1 older")),
            "{body_logs:?}"
        );
        let appendix = projection.view.closed.as_ref().expect("appendix");
        let cancel_logs = log_rows(&appendix.blocks);
        assert!(
            cancel_logs
                .iter()
                .any(|row| row.contains("CANCELED")
                    && row.contains("superseded")),
            "{cancel_logs:?}"
        );
        assert_eq!(
            cancel_logs
                .iter()
                .filter(|row| row.contains("CANCELED"))
                .count(),
            2,
            "{cancel_logs:?}"
        );
        let md =
            serialize_projected_markdown(&projection.view).expect("serialize");
        assert!(!md.contains("CANCEL LOG"), "{md}");
    }

    #[test]
    fn cancel_log_stays_off_open_tasks() {
        let directory = vault(&[(
            "Host.md",
            &area(
                "- [ ] Reopened #task ^open\n\
                 \t- ❌ **CANCEL LOG**\n\
                 \t\t- *2026-10-12* — was canceled\n",
            ),
        )]);
        for include_closed in [false, true] {
            let projection = project(&directory, "Host.md", include_closed);
            let logs = log_rows(&all_blocks(&projection));
            assert!(
                logs.iter().all(|row| !row.contains("CANCELED")),
                "include_closed={include_closed} {logs:?}"
            );
        }
    }

    #[test]
    fn omitted_and_restored_targets_rewrite_links() {
        let directory = vault(&[(
            "Host.md",
            &area(
                "- [ ] Depends #task ^live\n\
                 \t- ⛓️ **DEPENDS ON:** [[Host#^gone]]\n\
                 - [x] Gone #task ^gone\n",
            ),
        )]);
        let reading = project(&directory, "Host.md", false);
        let reading_md =
            serialize_projected_markdown(&reading.view).expect("serialize");
        assert!(!reading_md.contains("#t-gone"), "{reading_md}");
        assert!(reading_md.contains("Gone") || reading_md.contains("gone"));

        let restored = project(&directory, "Host.md", true);
        let restored_md =
            serialize_projected_markdown(&restored.view).expect("serialize");
        assert!(restored_md.contains("#t-gone"), "{restored_md}");
    }

    #[test]
    fn self_snapshot_ref_is_omitted_in_both_modes() {
        let directory = vault(&[
            (
                "Host.md",
                &area(
                    "- [ ] Read snapshot #task #ref [[Snap]] ^self\n\
                     - [ ] Unrelated #task #ref [[Other]] ^other\n\
                     - [x] Closed self #task #ref [[Snap]] [completion:: 2026-10-01] ^closedself\n",
                ),
            ),
            (
                "Snap.md",
                "---\nsource_note: \"[[Host]]\"\n---\n\n# Snap\n",
            ),
            (
                "Other.md",
                "---\nsource_note: \"[[Elsewhere]]\"\n---\n\n# Other\n",
            ),
        ]);
        for include_closed in [false, true] {
            let projection = project(&directory, "Host.md", include_closed);
            let titles = task_titles(&all_blocks(&projection));
            assert!(
                titles.iter().any(|t| t.contains("Unrelated")),
                "include_closed={include_closed} {titles:?}"
            );
            assert!(
                titles.iter().all(|t| !t.contains("Read snapshot")
                    && !t.contains("Closed self")),
                "include_closed={include_closed} {titles:?}"
            );
            assert_eq!(
                projection.view.summary.omitted_self_snapshot_tasks, 2,
                "include_closed={include_closed}"
            );
            assert!(
                !titles.iter().any(|t| t.contains("Closed self")),
                "self-snapshot must not enter the appendix: {titles:?}"
            );
        }
    }

    #[test]
    fn self_snapshot_false_positives_stay_visible() {
        let directory = vault(&[
            (
                "areas/Host.md",
                &area(
                    "- [ ] Ambiguous #task #ref [[Twin]] ^a\n\
                     - [ ] Code only #task #ref `[[Snap]]` ^c\n\
                     - [ ] Missing #task #ref [[NoSuch]] ^m\n\
                     - [ ] Malformed #task #ref [[BadMeta]] ^b\n\
                     - [ ] Real self #task #ref [[Snap]] ^s\n",
                ),
            ),
            ("other/Host.md", "# Other host\n"),
            ("Twin.md", "---\nsource_note: \"[[Host]]\"\n---\n\n# Twin\n"),
            (
                "Snap.md",
                "---\nsource_note: \"[[areas/Host]]\"\n---\n\n# Snap\n",
            ),
            ("BadMeta.md", "---\nsource_note: \"[\"\n---\n\n# Bad\n"),
        ]);
        let projection = project(&directory, "areas/Host.md", false);
        let titles = task_titles(&all_blocks(&projection));
        assert!(titles.iter().any(|t| t.contains("Ambiguous")), "{titles:?}");
        assert!(titles.iter().any(|t| t.contains("Code only")), "{titles:?}");
        assert!(titles.iter().any(|t| t.contains("Missing")), "{titles:?}");
        assert!(titles.iter().any(|t| t.contains("Malformed")), "{titles:?}");
        assert!(
            titles.iter().all(|t| !t.contains("Real self")),
            "{titles:?}"
        );
        assert_eq!(projection.view.summary.omitted_self_snapshot_tasks, 1);
    }

    #[test]
    fn source_bytes_and_hashes_are_unchanged() {
        let directory = vault(&[(
            "Host.md",
            &area(
                "- [ ] Open #task ^open\n\
                 \t- 🛠️ **WORK LOG**\n\
                 \t\t- *2026-10-15* — did work\n\
                 - [x] Done #task ^done\n",
            ),
        )]);
        let root = directory.path().canonicalize().expect("canonical");
        let before = read_tree(&root);
        let host_bytes = fs::read(root.join("Host.md")).expect("host bytes");
        let projection = project(&directory, "Host.md", false);
        assert_eq!(projection.parsed_source.raw_bytes.as_ref(), host_bytes);
        assert_eq!(projection.parsed_source.sha256, sha256_hex(&host_bytes));
        assert_eq!(before, read_tree(&root));
    }

    #[test]
    fn counting_loader_reads_each_canonical_source_once() {
        let directory = vault(&[
            (
                "Host.md",
                &area(
                    "- [ ] Read it #task #ref [[Snap]] ^self\n\n![[Embed]]\n",
                ),
            ),
            ("Embed.md", "- [ ] Embedded #task ^e\n"),
            ("Snap.md", "---\nsource_note: \"[[Host]]\"\n---\n\n# Snap\n"),
        ]);
        let root = directory.path().canonicalize().expect("canonical root");
        let host_path = root.join("Host.md").canonicalize().expect("host");
        let embed_path = root.join("Embed.md").canonicalize().expect("embed");
        let snap_path = root.join("Snap.md").canonicalize().expect("snap");
        let files = vec![
            (host_path.clone(), fs::read(&host_path).expect("host")),
            (embed_path.clone(), fs::read(&embed_path).expect("embed")),
            (snap_path.clone(), fs::read(&snap_path).expect("snap")),
        ];
        let loader = CountingLoader::with_files(files);
        let mut store =
            SourceStore::with_loader(&root, loader).expect("counting store");
        let identity =
            source_identity(&root, &host_path).expect("host identity");
        let note = super::super::classify::ClassifiedNote {
            display_path: "Host.md".to_string(),
            canonical_path: host_path,
            identity,
            kind: super::super::classify::NoteKind::Area,
        };
        let before = read_tree(&root);
        let projection = project_snapshot(
            &mut store,
            &note,
            &settings(),
            SnapshotProjectionOptions {
                masthead: masthead(),
                snapshot_date: snapshot_date(),
                include_closed: false,
                consumed_title_heading: None,
            },
        )
        .expect("project");
        assert_eq!(store.loader().reads(), 3, "host, embed, snap once each");
        assert_eq!(projection.view.summary.omitted_self_snapshot_tasks, 1);
        assert_eq!(before, read_tree(&root));
    }

    #[test]
    fn ordinary_notes_keep_closed_tasks() {
        let directory = vault(&[(
            "Host.md",
            "- [x] Done ordinary #task ^done\n\
             \t- 🛠️ **WORK LOG**\n\
             \t\t- *2026-10-15* — kept as a list\n",
        )]);
        let projection = project(&directory, "Host.md", false);
        let titles = task_titles(&body_blocks(&projection));
        assert!(
            titles.iter().any(|t| t.contains("Done ordinary")),
            "{titles:?}"
        );
        assert_eq!(projection.view.summary.omitted_closed_tasks, 0);
        assert!(projection.view.closed.is_none());
        let md =
            serialize_projected_markdown(&projection.view).expect("serialize");
        assert!(
            md.contains("WORK LOG")
                || md.contains("did work")
                || md.contains("kept as a list"),
            "{md}"
        );
    }
}
