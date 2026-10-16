//! Bob-layer task rows: chips and relationship metadata.
//!
//! This module owns the source-local Bob transform for area and project
//! snapshots. Every task and list row projected by the Obsidian layer
//! carries a [`TaskSource`] with its full original source line, so this
//! pass reads fields and managed rows with the shared parsers —
//! [`inline_fields`], [`parse_dependency_line`], [`parse_blocks_line`],
//! and the Sub-projects marker recognizer — instead of matching rendered
//! text or reopening vault files. Rows without block IDs, with identical
//! text, or inside repeated transclusions keep their actual source because
//! each occurrence carries its own attribution.
//!
//! Three responsibilities live here for the task-rows phase:
//!
//! - [`BobTaskContext`] carries the snapshot date every chip formats
//!   against. Dates come from the caller; this module never reads the
//!   clock.
//! - Supported task fields become [`Chip`] values without duplicating
//!   their old muted representation: `highest`/`high` accent,
//!   `low`/`lowest` muted, `medium` consumed silently, `scheduled` today
//!   or later as `sched Oct 17` (with `, YYYY` when the year differs from
//!   the snapshot year), `repeat` as `repeats <value>`, and `#ref` as
//!   `ref`. Past scheduled dates, malformed values, and custom fields keep
//!   their readable muted text.
//! - Recognized direct children become typed [`Block::LabeledRow`] values:
//!   `Waits on` (from a `DEPENDS ON` source line), `Blocks` (from a
//!   `BLOCKS` source line), and `Sub-projects` (from the `🧩
//!   **Sub-projects:**` marker line). Links keep the labels and targets
//!   the Obsidian layer already resolved (`Note › task text`); closed
//!   sub-projects get `done`/`canceled` chips with no strike around the
//!   link. Code, prose mentioning a marker, malformed lines, and nested
//!   children's rows are never mistaken for a parent's metadata.
//!
//! Wire-up note: [`apply_bob_task_rows`] is the per-view entry point. The
//! transclusion assembler calls it on the final section blocks while each
//! occurrence still carries its own [`TaskSource`], so embed anchor
//! namespacing and link rewriting keep working on the rows produced here.
//! Later Bob phases (managed logs, closed appendix, masthead) build on the
//! same seam.
//!
//! [`inline_fields`]: crate::native::task_fields::inline_fields
//! [`parse_dependency_line`]:
//!   crate::native::task_dependencies::parse::parse_dependency_line
//! [`parse_blocks_line`]:
//!   crate::native::task_dependencies::blocks::parse_blocks_line

use std::collections::HashMap;
use std::ops::Range;

use chrono::NaiveDate;

use super::view::{
    Block, Chip, ChipKind, Inline, ListItem, SnapshotTask, TaskSource,
};
use crate::native::projects::{
    display_for_wikilink, list_item_content, wikilink_spans_in_line,
    SubprojectState, SUBPROJECTS_MARKER_PREFIX,
    SUBPROJECT_FUTURE_SCHEDULE_MARKER,
};
use crate::native::task_dependencies::blocks::{parse_blocks_line, BlocksLine};
use crate::native::task_dependencies::inline_code_spans;
use crate::native::task_dependencies::parse::{
    parse_dependency_line, DependencyLine,
};
use crate::native::task_fields::{inline_fields, parse_strict_calendar_date};

/// Reader context for the Bob task-row transform.
///
/// `snapshot_date` is the caller-selected snapshot day: `scheduled` chips
/// appear only for dates on or after it, and the year suffix appears only
/// when a chip's year differs from this date's year. Callers compute it
/// with the existing `env::current_datetime()`/`BOB_NOW` path and pass it
/// in so tests can pin deterministic dates.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct BobTaskContext {
    pub snapshot_date: NaiveDate,
}

/// Apply Bob task rows to already-assembled section blocks.
///
/// The transform walks the tree in place: every [`Block::Task`] gains
/// chips (with consumed muted fields removed) and typed relationship
/// rows, including tasks nested under other tasks, list items, embeds,
/// callouts, and compact containers. Status values are never touched —
/// they stay exactly what the checkbox and Tasks settings produced.
/// Tasks without source attribution (synthetic test fixtures) keep their
/// content and children unchanged.
pub(crate) fn apply_bob_task_rows(
    blocks: &mut [Block],
    context: &BobTaskContext,
) {
    for block in blocks.iter_mut() {
        apply_to_block(block, context);
    }
}

fn apply_to_block(block: &mut Block, context: &BobTaskContext) {
    match block {
        Block::Task(task) => apply_to_task(task, context),
        Block::List { items, .. } => {
            for item in items {
                apply_to_blocks(&mut item.children, context);
            }
        }
        Block::Embed { blocks, .. }
        | Block::LiveQuery { blocks }
        | Block::Compact { blocks } => apply_to_blocks(blocks, context),
        Block::Callout { blocks, .. } => apply_to_blocks(blocks, context),
        _ => {}
    }
}

fn apply_to_blocks(blocks: &mut [Block], context: &BobTaskContext) {
    for block in blocks.iter_mut() {
        apply_to_block(block, context);
    }
}

/// Transform one task: chips first, then direct-child relationship rows,
/// then recursion into surviving children (nested tasks own their rows).
fn apply_to_task(task: &mut SnapshotTask, context: &BobTaskContext) {
    if let Some(source) = task.source.as_ref() {
        let (chips, consumed) = chips_for_task(source, context.snapshot_date);
        if !consumed.is_empty() {
            remove_consumed_muted(&mut task.content, &consumed);
        }
        task.chips.extend(chips);
    }
    task.children =
        convert_task_children(std::mem::take(&mut task.children), context);
    apply_to_blocks(&mut task.children, context);
}

/// Split a task's direct children into relationship rows and leftovers.
///
/// Every [`Block::List`] directly under the task is partitioned item by
/// item: recognized managed rows become [`Block::LabeledRow`] blocks (each
/// followed by that item's surviving children), while unrecognized items
/// stay in ordinary lists in their original order. Non-list children pass
/// through untouched for the recursion step.
fn convert_task_children(
    children: Vec<Block>,
    context: &BobTaskContext,
) -> Vec<Block> {
    let mut output = Vec::with_capacity(children.len());
    for child in children {
        match child {
            Block::List { kind, items } => {
                let mut run = Vec::new();
                for mut item in items {
                    if let Some(mut row) = managed_row_for_item(&item) {
                        if !run.is_empty() {
                            output.push(Block::List {
                                kind,
                                items: std::mem::take(&mut run),
                            });
                        }
                        output.append(&mut row);
                        let mut leftovers = std::mem::take(&mut item.children);
                        apply_to_blocks(&mut leftovers, context);
                        output.append(&mut leftovers);
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
    output
}

/// Compute a task's chips from its raw source line.
///
/// Fields inside inline code spans are ignored, so literal examples never
/// mint chips. The returned muted texts name exactly the projected
/// `key: value` nodes the chips replace, so callers can remove them
/// without touching custom or malformed values.
fn chips_for_task(
    source: &TaskSource,
    snapshot_date: NaiveDate,
) -> (Vec<Chip>, Vec<String>) {
    let raw = source.raw_line.as_str();
    let code = inline_code_spans(raw);
    let outside_code = |start: usize, end: usize| {
        !code.iter().any(|span| start < span.end && end > span.start)
    };
    let mut chips = Vec::new();
    let mut consumed = Vec::new();

    let first_scheduled = inline_fields(raw, "scheduled")
        .into_iter()
        .filter(|field| outside_code(field.start, field.end))
        .filter_map(|field| {
            parse_strict_calendar_date(&field.value).map(|date| (field, date))
        })
        .next();
    if let Some((field, date)) = first_scheduled
        && date >= snapshot_date
    {
        chips.push(Chip {
            content: vec![Inline::Text(format_scheduled(date, snapshot_date))],
            kind: ChipKind::Normal,
            target: None,
        });
        consumed.push(format!("scheduled: {}", field.value));
    }

    for field in inline_fields(raw, "priority") {
        if !outside_code(field.start, field.end) {
            continue;
        }
        let normalized = field.value.to_ascii_lowercase();
        let kind = match normalized.as_str() {
            "highest" | "high" => Some(ChipKind::Accent),
            "low" | "lowest" => Some(ChipKind::Normal),
            "medium" => None,
            _ => continue,
        };
        if let Some(kind) = kind {
            chips.push(Chip {
                content: vec![Inline::Text(normalized)],
                kind,
                target: None,
            });
        }
        consumed.push(format!("priority: {}", field.value));
        break;
    }

    for field in inline_fields(raw, "repeat") {
        if !outside_code(field.start, field.end) || field.value.is_empty() {
            continue;
        }
        chips.push(Chip {
            content: vec![Inline::Text(format!("repeats {}", field.value))],
            kind: ChipKind::Normal,
            target: None,
        });
        consumed.push(format!("repeat: {}", field.value));
        break;
    }

    if has_ref_tag(raw, &code) {
        chips.push(Chip {
            content: vec![Inline::Text("ref".to_string())],
            kind: ChipKind::Normal,
            target: None,
        });
    }

    (chips, consumed)
}

/// `sched Oct 17`, with `, YYYY` when the year differs from the snapshot
/// year so December/January boundaries stay unambiguous.
fn format_scheduled(date: NaiveDate, snapshot_date: NaiveDate) -> String {
    use chrono::Datelike;
    let day = format!("{} {}", date.format("%b"), date.day());
    if date.year() == snapshot_date.year() {
        format!("sched {day}")
    } else {
        format!("sched {day}, {}", date.year())
    }
}

/// True when `raw` carries an exact lowercase `#ref` tag outside code.
/// The boundary rules mirror the Obsidian projector: the `#` must not
/// follow an alphanumeric, `_`, or `/`, and `#references` never counts.
fn has_ref_tag(raw: &str, code: &[Range<usize>]) -> bool {
    let bytes = raw.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        let Some(offset) = raw[index..].find("#ref") else {
            return false;
        };
        let start = index + offset;
        let end = start + "#ref".len();
        let prev_ok = start == 0
            || !raw[..start].chars().next_back().is_some_and(|prev| {
                prev.is_alphanumeric() || prev == '_' || prev == '/'
            });
        let next_ok = raw[end..].chars().next().is_none_or(|next| {
            !(next.is_alphanumeric()
                || next == '_'
                || next == '-'
                || next == '/')
        });
        if prev_ok
            && next_ok
            && !code.iter().any(|span| start < span.end && end > span.start)
        {
            return true;
        }
        index = end;
    }
    false
}

/// Convert one direct-child list item into relationship rows when its raw
/// source line is a recognized managed row. Recognition runs on the raw
/// line with the shared parsers, so prose mentioning a marker, fenced or
/// inline code, and malformed lines never convert. The returned blocks are
/// the [`Block::LabeledRow`] values for that line; the item's children are
/// left for the caller to reattach. Unrecognized items return `None` and
/// stay ordinary list content.
fn managed_row_for_item(item: &ListItem) -> Option<Vec<Block>> {
    let raw = item.source.as_ref()?.raw_line.as_str();
    if matches!(parse_dependency_line(raw), DependencyLine::Accepted { .. }) {
        return Some(vec![Block::LabeledRow {
            label: vec![Inline::Text("Waits on".to_string())],
            items: extract_link_items(&item.content),
        }]);
    }
    if matches!(parse_blocks_line(raw), BlocksLine::Accepted { .. }) {
        return Some(vec![Block::LabeledRow {
            label: vec![Inline::Text("Blocks".to_string())],
            items: extract_link_items(&item.content),
        }]);
    }
    if is_subprojects_marker_line(raw) {
        return Some(vec![Block::LabeledRow {
            label: vec![Inline::Text("Sub-projects".to_string())],
            items: subproject_items(raw, &item.content),
        }]);
    }
    None
}

/// True for a direct-child `🧩 **Sub-projects:**` marker line, using the
/// same list-item content rule as project sync.
fn is_subprojects_marker_line(raw: &str) -> bool {
    list_item_content(raw)
        .is_some_and(|content| content.starts_with(SUBPROJECTS_MARKER_PREFIX))
}

/// Collect the already-resolved link inlines of a managed row as one item
/// per link. Labels and targets come straight from the Obsidian layer
/// (`Note › task text` with internal or Obsidian-URI targets, including
/// its unresolved-target warnings), so no vault read happens here. Link
/// decorations the writer never emits (strike, alias) survive untouched;
/// rows with no extractable link keep their whole content as one item.
fn extract_link_items(content: &[Inline]) -> Vec<Vec<Inline>> {
    let mut items = Vec::new();
    for inline in content {
        match inline {
            Inline::Link { .. } | Inline::Chip(_) => {
                items.push(vec![inline.clone()]);
            }
            Inline::Strike(inner) if contains_link(inner) => {
                items.push(vec![inline.clone()]);
            }
            _ => {}
        }
    }
    if items.is_empty() {
        items.push(content.to_vec());
    }
    items
}

fn contains_link(inlines: &[Inline]) -> bool {
    inlines.iter().any(|inline| match inline {
        Inline::Link { .. } | Inline::Chip(_) => true,
        Inline::Emphasis(inner)
        | Inline::Strong(inner)
        | Inline::Strike(inner)
        | Inline::Muted(inner) => contains_link(inner),
        _ => false,
    })
}

/// Build one Sub-projects item per wikilink span on the marker line.
///
/// Each item reuses the Obsidian layer's resolved link inline in span
/// order (unwrapping the source strike: closed sub-projects get a
/// `done`/`canceled` chip instead). A future-schedule `🗓️` keeps its
/// marker text ahead of the link. Spans inside inline code are skipped,
/// and when spans and resolved links disagree in count the links are
/// used as-is with open display.
fn subproject_items(raw: &str, content: &[Inline]) -> Vec<Vec<Inline>> {
    let code = inline_code_spans(raw);
    let spans: Vec<_> = wikilink_spans_in_line(raw)
        .into_iter()
        .filter(|span| {
            !code
                .iter()
                .any(|code| span.start < code.end && span.end > code.start)
        })
        .collect();
    let mut links = Vec::new();
    for inline in content {
        match inline {
            Inline::Link { .. } => links.push(inline.clone()),
            Inline::Strike(inner) => {
                for nested in inner {
                    if matches!(nested, Inline::Link { .. }) {
                        links.push(nested.clone());
                    }
                }
            }
            _ => {}
        }
    }
    if spans.len() != links.len() {
        return extract_link_items(content);
    }
    spans
        .into_iter()
        .zip(links)
        .map(|(span, link)| {
            let display = display_for_wikilink(raw, span.start, span.end);
            let mut item = Vec::new();
            if display.future_scheduled {
                item.push(Inline::Text(format!(
                    "{SUBPROJECT_FUTURE_SCHEDULE_MARKER} "
                )));
            }
            item.push(link);
            match display.state {
                SubprojectState::Open => {}
                SubprojectState::Done => item.push(Inline::Chip(Chip {
                    content: vec![Inline::Text("done".to_string())],
                    kind: ChipKind::Normal,
                    target: None,
                })),
                SubprojectState::Canceled => item.push(Inline::Chip(Chip {
                    content: vec![Inline::Text("canceled".to_string())],
                    kind: ChipKind::Normal,
                    target: None,
                })),
            }
            item
        })
        .collect()
}

/// Drop the muted `key: value` nodes a chip replaces, one per consumed
/// occurrence. Anything else — custom fields, malformed values, past
/// scheduled dates — stays readable.
fn remove_consumed_muted(content: &mut Vec<Inline>, consumed: &[String]) {
    let mut remaining: HashMap<&str, usize> = HashMap::new();
    for text in consumed {
        *remaining.entry(text.as_str()).or_default() += 1;
    }
    content.retain(|inline| {
        if let Inline::Muted(inner) = inline
            && let [Inline::Text(text)] = inner.as_slice()
            && let Some(count) = remaining.get_mut(text.as_str())
            && *count > 0
        {
            *count -= 1;
            return false;
        }
        true
    });
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, BTreeSet};
    use std::fs;

    use chrono::NaiveDate;

    use super::super::classify::{
        classify_target, source_identity, FilesystemSourceLoader,
        SnapshotTarget, SourceStore,
    };
    use super::super::obsidian::project_obsidian_syntax;
    use super::super::resolve::AttachmentIndex;
    use super::super::transclusion::{
        project_snapshot, SnapshotProjectionOptions,
    };
    use super::super::view::{
        Block, ChipKind, Inline, SnapshotMasthead, SnapshotTask,
        SnapshotTaskStatus,
    };
    use super::*;
    use crate::native::task_status_hooks::{TaskStatusType, TasksSettings};

    fn snapshot_date() -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 10, 16).expect("test date")
    }

    fn context() -> BobTaskContext {
        BobTaskContext {
            snapshot_date: snapshot_date(),
        }
    }

    /// Vault Tasks settings mirroring the corpus: `*` Next, `?` Blocked,
    /// `/` In Progress, `-` Canceled.
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

    /// Project one host's syntax blocks and run the Bob task-row pass.
    fn bob_blocks(files: &[(&str, &str)], host: &str) -> Vec<Block> {
        let directory = vault(files);
        let root = directory.path().canonicalize().expect("canonical root");
        let mut store = SourceStore::with_loader(&root, FilesystemSourceLoader)
            .expect("open store");
        let vault_root = store.vault_root().to_path_buf();
        let canonical = root.join(host).canonicalize().expect("host exists");
        let identity =
            source_identity(&vault_root, &canonical).expect("identity");
        let mut attachments = AttachmentIndex::new(vault_root);
        let projection = project_obsidian_syntax(
            &mut store,
            &mut attachments,
            &identity,
            &settings(),
        )
        .expect("project host");
        let mut blocks = projection.blocks;
        apply_bob_task_rows(&mut blocks, &context());
        blocks
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
                | Block::Compact { blocks } => walk_blocks(blocks, visit),
                Block::Callout { blocks, .. } => walk_blocks(blocks, visit),
                _ => {}
            }
        }
    }

    fn labeled_rows(
        blocks: &[Block],
    ) -> Vec<(&Vec<Inline>, &Vec<Vec<Inline>>)> {
        let mut rows = Vec::new();
        walk_blocks(blocks, &mut |block| {
            if let Block::LabeledRow { label, items } = block {
                rows.push((label, items));
            }
        });
        rows
    }

    fn chip_texts(task: &SnapshotTask) -> Vec<String> {
        task.chips
            .iter()
            .map(|chip| {
                let kind = match chip.kind {
                    ChipKind::Normal => "",
                    ChipKind::Accent => "+",
                };
                format!("{kind}{}", flat(&chip.content))
            })
            .collect()
    }

    fn flat(inlines: &[Inline]) -> String {
        let mut out = String::new();
        for inline in inlines {
            match inline {
                Inline::Text(text) | Inline::Code(text) => out.push_str(text),
                Inline::Link { label, .. }
                | Inline::Emphasis(label)
                | Inline::Strong(label)
                | Inline::Strike(label)
                | Inline::Muted(label) => out.push_str(&flat(label)),
                Inline::Chip(chip) => out.push_str(&flat(&chip.content)),
                Inline::Anchor(_) | Inline::Image { .. } | Inline::Break => {}
                Inline::EmbeddedBlock(_) => {}
            }
        }
        out
    }

    fn row_label(label: &[Inline]) -> String {
        flat(label)
    }

    #[test]
    fn chips_cover_priority_scheduled_repeat_and_ref() {
        let blocks = bob_blocks(
            &[(
                "Host.md",
                "- [ ] Ship it [priority:: high] [scheduled:: 2026-10-17] \
                 [repeat:: every week] #task #ref\n",
            )],
            "Host.md",
        );
        let tasks = tasks_in(&blocks);
        assert_eq!(tasks.len(), 1, "{blocks:?}");
        assert_eq!(
            chip_texts(tasks[0]),
            vec!["sched Oct 17", "+high", "repeats every week", "ref"]
        );
        let content = flat(&tasks[0].content);
        assert!(!content.contains("priority:"), "{content}");
        assert!(!content.contains("scheduled:"), "{content}");
        assert!(!content.contains("repeat:"), "{content}");
        assert!(content.contains("Ship it"), "{content}");
    }

    #[test]
    fn untagged_checkboxes_get_chips() {
        let blocks = bob_blocks(
            &[("Host.md", "- [ ] Plain row [priority:: low]\n")],
            "Host.md",
        );
        let tasks = tasks_in(&blocks);
        assert_eq!(tasks.len(), 1, "{blocks:?}");
        assert_eq!(chip_texts(tasks[0]), vec!["low"]);
    }

    #[test]
    fn custom_settings_without_task_filter_still_chip() {
        let directory =
            vault(&[("Host.md", "- [ ] Bare row [priority:: highest]\n")]);
        let root = directory.path().canonicalize().expect("canonical root");
        let mut store = SourceStore::with_loader(&root, FilesystemSourceLoader)
            .expect("open store");
        let vault_root = store.vault_root().to_path_buf();
        let canonical =
            root.join("Host.md").canonicalize().expect("host exists");
        let identity =
            source_identity(&vault_root, &canonical).expect("identity");
        let mut attachments = AttachmentIndex::new(vault_root);
        let empty_settings = TasksSettings {
            global_filter: String::new(),
            done_statuses: BTreeSet::from(['x', 'X']),
            status_types: BTreeMap::from([
                (' ', TaskStatusType::Todo),
                ('x', TaskStatusType::Done),
            ]),
            status_definitions: Vec::new(),
            status_settings_error: None,
        };
        let projection = project_obsidian_syntax(
            &mut store,
            &mut attachments,
            &identity,
            &empty_settings,
        )
        .expect("project host");
        let mut blocks = projection.blocks;
        apply_bob_task_rows(&mut blocks, &context());
        let tasks = tasks_in(&blocks);
        assert_eq!(tasks.len(), 1, "{blocks:?}");
        assert_eq!(chip_texts(tasks[0]), vec!["+highest"]);
    }

    #[test]
    fn duplicate_text_with_different_fields_chips_per_row() {
        let blocks = bob_blocks(
            &[(
                "Host.md",
                "- [ ] Same words [priority:: high] #task ^one\n\
                 - [ ] Same words [priority:: low] #task ^two\n",
            )],
            "Host.md",
        );
        let tasks = tasks_in(&blocks);
        assert_eq!(tasks.len(), 2, "{blocks:?}");
        assert_eq!(chip_texts(tasks[0]), vec!["+high"]);
        assert_eq!(chip_texts(tasks[1]), vec!["low"]);
    }

    #[test]
    fn past_and_malformed_fields_stay_readable() {
        let blocks = bob_blocks(
            &[(
                "Host.md",
                "- [ ] Old plan [scheduled:: 2026-10-01] [priority:: ASAP] \
                 [custom:: kept] #task\n",
            )],
            "Host.md",
        );
        let tasks = tasks_in(&blocks);
        assert_eq!(tasks.len(), 1, "{blocks:?}");
        assert!(tasks[0].chips.is_empty(), "{tasks:?}");
        let content = flat(&tasks[0].content);
        assert!(content.contains("scheduled: 2026-10-01"), "{content}");
        assert!(content.contains("priority: ASAP"), "{content}");
        assert!(content.contains("custom: kept"), "{content}");
    }

    #[test]
    fn medium_priority_is_consumed_without_a_chip() {
        let blocks = bob_blocks(
            &[("Host.md", "- [ ] Steady work [priority:: medium] #task\n")],
            "Host.md",
        );
        let tasks = tasks_in(&blocks);
        assert_eq!(tasks.len(), 1, "{blocks:?}");
        assert!(tasks[0].chips.is_empty(), "{tasks:?}");
        let content = flat(&tasks[0].content);
        assert!(!content.contains("priority:"), "{content}");
    }

    #[test]
    fn scheduled_today_and_next_year_format() {
        let blocks = bob_blocks(
            &[(
                "Host.md",
                "- [ ] Due now [scheduled:: 2026-10-16] #task\n\
                 - [ ] Far out [scheduled:: 2027-01-05] #task\n",
            )],
            "Host.md",
        );
        let tasks = tasks_in(&blocks);
        assert_eq!(tasks.len(), 2, "{blocks:?}");
        assert_eq!(chip_texts(tasks[0]), vec!["sched Oct 16"]);
        assert_eq!(chip_texts(tasks[1]), vec!["sched Jan 5, 2027"]);
    }

    #[test]
    fn literal_code_never_mints_chips() {
        let blocks = bob_blocks(
            &[(
                "Host.md",
                "- [ ] Fix `[priority:: high]` and `[scheduled:: 2026-10-20]` \
                 #task\n",
            )],
            "Host.md",
        );
        let tasks = tasks_in(&blocks);
        assert_eq!(tasks.len(), 1, "{blocks:?}");
        assert!(tasks[0].chips.is_empty(), "{tasks:?}");
        let content = flat(&tasks[0].content);
        assert!(content.contains("[priority:: high]"), "{content}");
    }

    #[test]
    fn six_statuses_survive_the_bob_pass() {
        let blocks = bob_blocks(
            &[(
                "Host.md",
                "- [ ] open [priority:: high] #task\n\
                 - [*] next #task\n\
                 - [/] wip #task\n\
                 - [?] blocked #task\n\
                 - [x] done #task\n\
                 - [-] canceled #task\n",
            )],
            "Host.md",
        );
        let tasks = tasks_in(&blocks);
        assert_eq!(tasks.len(), 6, "{blocks:?}");
        let statuses: Vec<_> = tasks.iter().map(|task| task.status).collect();
        assert_eq!(
            statuses,
            vec![
                SnapshotTaskStatus::Open,
                SnapshotTaskStatus::Next,
                SnapshotTaskStatus::Wip,
                SnapshotTaskStatus::Blocked,
                SnapshotTaskStatus::Done,
                SnapshotTaskStatus::Canceled,
            ]
        );
        assert_eq!(chip_texts(tasks[0]), vec!["+high"]);
    }

    #[test]
    fn waits_on_and_blocks_rows_keep_resolved_labels() {
        let blocks = bob_blocks(
            &[
                (
                    "Host.md",
                    "- [ ] Launch #task ^launch\n\
                     \n\
                     - [ ] blocked [priority:: high] #task\n\
                     \t- ⛓️ **DEPENDS ON:** [[Host#^launch]]\n\
                     - [ ] unblocker #task\n\
                     \t- 🧱 **BLOCKS:** [[Host#^launch]]\n",
                ),
                ("Other.md", "- [ ] Other task #task ^o1\n"),
            ],
            "Host.md",
        );
        let rows = labeled_rows(&blocks);
        assert_eq!(rows.len(), 2, "{blocks:?}");
        assert_eq!(row_label(rows[0].0), "Waits on");
        assert_eq!(row_label(rows[1].0), "Blocks");
        for (_, items) in &rows {
            assert_eq!(items.len(), 1, "{items:?}");
            let label = flat(&items[0]);
            assert!(
                label.starts_with("Host › Launch"),
                "expected resolved label, got {label:?}"
            );
        }
        let tasks = tasks_in(&blocks);
        assert_eq!(chip_texts(tasks[1]), vec!["+high"]);
    }

    #[test]
    fn unresolved_targets_stay_readable() {
        let blocks = bob_blocks(
            &[(
                "Host.md",
                "- [ ] blocked #task\n\
                 \t- ⛓️ **DEPENDS ON:** [[Missing#^nope]]\n",
            )],
            "Host.md",
        );
        let rows = labeled_rows(&blocks);
        assert_eq!(rows.len(), 1, "{blocks:?}");
        assert_eq!(row_label(rows[0].0), "Waits on");
        let label = flat(&rows[0].1[0]);
        assert!(label.contains("Missing"), "{label}");
    }

    #[test]
    fn nested_child_rows_belong_to_the_child() {
        let blocks = bob_blocks(
            &[
                (
                    "Host.md",
                    "- [ ] Parent #task\n\
                     \t- ⛓️ **DEPENDS ON:** [[Host#^launch]]\n\
                     \t- [ ] Child #task\n\
                     \t\t- 🧱 **BLOCKS:** [[Other#^o1]]\n",
                ),
                ("Other.md", "- [ ] Other #task ^o1\n"),
            ],
            "Host.md",
        );
        let tasks = tasks_in(&blocks);
        assert_eq!(tasks.len(), 2, "{blocks:?}");
        let parent_rows: Vec<_> = tasks[0]
            .children
            .iter()
            .filter_map(|child| match child {
                Block::LabeledRow { label, .. } => Some(row_label(label)),
                _ => None,
            })
            .collect();
        assert_eq!(parent_rows, vec!["Waits on"], "{blocks:?}");
        let child_rows: Vec<_> = tasks[1]
            .children
            .iter()
            .filter_map(|child| match child {
                Block::LabeledRow { label, .. } => Some(row_label(label)),
                _ => None,
            })
            .collect();
        assert_eq!(child_rows, vec!["Blocks"], "{blocks:?}");
    }

    #[test]
    fn prose_and_code_mentioning_markers_stay_lists() {
        let blocks = bob_blocks(
            &[(
                "Host.md",
                "- [ ] Parent #task\n\
                 \t- remember that BLOCKS: is a label, not prose\n\
                 \t- `🧱 **BLOCKS:** [[Host#^x]]`\n\
                 \t- ⛓️ **DEPENDS ON: extra words here**\n",
            )],
            "Host.md",
        );
        let rows = labeled_rows(&blocks);
        assert!(rows.is_empty(), "{blocks:?}");
        let tasks = tasks_in(&blocks);
        assert_eq!(tasks.len(), 1, "{blocks:?}");
        let lists = tasks[0]
            .children
            .iter()
            .filter(|child| matches!(child, Block::List { .. }))
            .count();
        assert_eq!(lists, 1, "{blocks:?}");
    }

    #[test]
    fn rich_inline_content_survives_chips_and_rows() {
        let blocks = bob_blocks(
            &[
                (
                    "Host.md",
                    "- [ ] Read **bold** *em* `code` [[Other]] [priority:: high] \
                     [custom:: kept] #task\n\
                     \t- ⛓️ **DEPENDS ON:** [[Other#^o1]]\n",
                ),
                ("Other.md", "- [ ] Other task #task ^o1\n"),
            ],
            "Host.md",
        );
        let tasks = tasks_in(&blocks);
        assert_eq!(tasks.len(), 1, "{blocks:?}");
        let content = flat(&tasks[0].content);
        for needle in ["bold", "em", "code", "Other", "custom: kept"] {
            assert!(content.contains(needle), "{needle} missing in {content}");
        }
        assert_eq!(chip_texts(tasks[0]), vec!["+high"]);
        let rows = labeled_rows(&blocks);
        assert_eq!(rows.len(), 1, "{blocks:?}");
    }

    #[test]
    fn subprojects_marker_converts_with_closed_chips() {
        let blocks = bob_blocks(
            &[
                (
                    "Host.md",
                    "- [ ] Outcome #task #prj ^prj\n\
                     \t- 🧩 **Sub-projects:** [[Alpha]] • ~~[[Beta]]~~ ✅ • \
                     ~~[[Gamma]]~~ ❌\n",
                ),
                ("Alpha.md", "# Alpha\n"),
                ("Beta.md", "# Beta\n"),
                ("Gamma.md", "# Gamma\n"),
            ],
            "Host.md",
        );
        let rows = labeled_rows(&blocks);
        assert_eq!(rows.len(), 1, "{blocks:?}");
        assert_eq!(row_label(rows[0].0), "Sub-projects");
        assert_eq!(rows[0].1.len(), 3, "{rows:?}");
        assert_eq!(flat(&rows[0].1[0]), "Alpha");
        assert_eq!(
            rows[0].1[1]
                .iter()
                .map(|inline| flat(std::slice::from_ref(inline)))
                .collect::<Vec<_>>(),
            vec!["Beta", "done"]
        );
        assert_eq!(
            rows[0].1[2]
                .iter()
                .map(|inline| flat(std::slice::from_ref(inline)))
                .collect::<Vec<_>>(),
            vec!["Gamma", "canceled"]
        );
        for items in rows[0].1 {
            for inline in items {
                assert!(
                    !matches!(inline, Inline::Strike(_)),
                    "strike must not wrap sub-project links: {items:?}"
                );
            }
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

    /// Full transclusion projection, so kind gating and embed occurrences
    /// are covered exactly as the pipeline runs them.
    fn snapshot(
        directory: &tempfile::TempDir,
        host: &str,
    ) -> super::super::transclusion::SnapshotProjection {
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
            },
        )
        .expect("project snapshot")
    }

    fn section_blocks(
        projection: &super::super::transclusion::SnapshotProjection,
    ) -> Vec<Block> {
        projection
            .view
            .sections
            .iter()
            .flat_map(|section| section.blocks.clone())
            .collect()
    }

    #[test]
    fn ordinary_host_keeps_plain_projection() {
        let directory = vault(&[(
            "Host.md",
            "- [ ] Plain [priority:: high] #task\n\
             \t- 🧱 **BLOCKS:** [[Host#^x]]\n",
        )]);
        let projection = snapshot(&directory, "Host.md");
        assert_eq!(
            projection.view.source.route, "Host",
            "{:?}",
            projection.view.source
        );
        let blocks = section_blocks(&projection);
        let tasks = tasks_in(&blocks);
        assert_eq!(tasks.len(), 1, "{blocks:?}");
        assert!(tasks[0].chips.is_empty(), "{tasks:?}");
        assert!(labeled_rows(&blocks).is_empty(), "{blocks:?}");
    }

    #[test]
    fn area_host_applies_rows_to_repeated_embeds() {
        let directory = vault(&[
            (
                "Host.md",
                "---\ntype: \"[[area]]\"\n---\n\n# Area\n\n![[Tasks]]\n\n![[Tasks]]\n",
            ),
            (
                "Tasks.md",
                "- [ ] Shipped [priority:: high] #task ^t1\n\
                 \t- ⛓️ **DEPENDS ON:** [[Tasks#^t1]]\n",
            ),
        ]);
        let projection = snapshot(&directory, "Host.md");
        let blocks = section_blocks(&projection);
        let tasks = tasks_in(&blocks);
        assert_eq!(tasks.len(), 2, "{blocks:?}");
        for task in &tasks {
            assert_eq!(chip_texts(task), vec!["+high"], "{task:?}");
        }
        let anchors: Vec<_> = tasks
            .iter()
            .map(|task| task.anchor.as_ref().map(|anchor| anchor.id.as_str()))
            .collect();
        assert_ne!(anchors[0], anchors[1], "occurrences share an anchor");
        let rows = labeled_rows(&blocks);
        assert_eq!(rows.len(), 2, "{blocks:?}");
        for (label, items) in &rows {
            assert_eq!(row_label(label), "Waits on");
            assert_eq!(items.len(), 1, "{items:?}");
            assert!(
                flat(&items[0]).starts_with("Tasks › Shipped"),
                "unexpected label {}",
                flat(&items[0])
            );
        }
    }

    #[test]
    fn selected_section_embed_keeps_source_rows() {
        let directory = vault(&[
            (
                "Host.md",
                "---\ntype: \"[[project]]\"\n---\n\n# Project\n\n![[Tasks#Todo]]\n",
            ),
            (
                "Tasks.md",
                "# Todo\n\n- [ ] Picked [scheduled:: 2026-10-20] #task\n\
                 \n\
                 # Later\n\n- [ ] Skipped #task\n",
            ),
        ]);
        let projection = snapshot(&directory, "Host.md");
        let blocks = section_blocks(&projection);
        let tasks = tasks_in(&blocks);
        assert_eq!(tasks.len(), 1, "{blocks:?}");
        assert_eq!(chip_texts(tasks[0]), vec!["sched Oct 20"]);
        assert!(flat(&tasks[0].content).contains("Picked"), "{tasks:?}");
    }

    #[test]
    fn bob_kinds_all_apply_rows() {
        for frontmatter in [
            "type: \"[[area]]\"",
            "type: \"[[project]]\"",
            "type: \"[[project]]\"\nstatus: done",
        ] {
            let directory = vault(&[(
                "Host.md",
                &format!(
                    "---\n{frontmatter}\n---\n\n- [ ] Row [priority:: low] #task\n"
                ),
            )]);
            let projection = snapshot(&directory, "Host.md");
            let blocks = section_blocks(&projection);
            let tasks = tasks_in(&blocks);
            assert_eq!(tasks.len(), 1, "{frontmatter} {blocks:?}");
            assert_eq!(
                chip_texts(tasks[0]),
                vec!["low"],
                "{frontmatter} {tasks:?}"
            );
        }
    }

    #[test]
    fn task_source_attribution_matches_raw_bytes() {
        let directory = vault(&[(
            "Host.md",
            "- [ ] Attributed [priority:: high] #task ^abc\n",
        )]);
        let root = directory.path().canonicalize().expect("canonical root");
        let mut store = SourceStore::with_loader(&root, FilesystemSourceLoader)
            .expect("open store");
        let vault_root = store.vault_root().to_path_buf();
        let canonical =
            root.join("Host.md").canonicalize().expect("host exists");
        let identity =
            source_identity(&vault_root, &canonical).expect("identity");
        let mut attachments = AttachmentIndex::new(vault_root);
        let projection = project_obsidian_syntax(
            &mut store,
            &mut attachments,
            &identity,
            &settings(),
        )
        .expect("project host");
        let parsed = &projection.parsed;
        let mut blocks = projection.blocks;
        apply_bob_task_rows(&mut blocks, &context());
        let tasks = tasks_in(&blocks);
        assert_eq!(tasks.len(), 1, "{blocks:?}");
        let source = tasks[0].source.as_ref().expect("task has a source");
        assert_eq!(source.line, 1);
        assert_eq!(source.source, identity);
        let raw = String::from_utf8_lossy(&parsed.raw_bytes);
        let raw_line = raw.lines().nth(source.line - 1).expect("raw line");
        assert_eq!(source.raw_line, raw_line);
        assert_eq!(parsed.sha256.len(), 64);
    }
}
