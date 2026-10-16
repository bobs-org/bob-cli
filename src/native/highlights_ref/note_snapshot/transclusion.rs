//! One-level, source-aware note transclusion assembly.
//!
//! The syntax projector owns Obsidian parsing. This module selects cached source
//! extents, projects each included slice with the same rules, namespaces its anchors,
//! and leaves every nested request as a readable Obsidian link.

use std::{
    collections::{BTreeMap, BTreeSet, HashMap, HashSet},
    path::{Path, PathBuf},
};

use super::{
    bob::{
        apply_bob_task_rows, apply_scheduled_lanes, is_snapshot_lane_title,
        take_project_outcome, BobTaskContext, ProjectOutcome,
    },
    classify::{
        source_identity, ClassifiedNote, NoteKind, SourceLoader, SourceStore,
    },
    obsidian::{
        project_obsidian_syntax, project_obsidian_syntax_selection,
        PendingEmbed, SyntaxProjection, TitleCandidate,
    },
    resolve::{
        build_fragment_index, obsidian_uri_with_fragment, AttachmentIndex,
        FragmentIndex, FragmentLookup, LinkFragment,
    },
    view::{
        AnchorIdentity, AnchorKind, Block, Chip, ChipKind, HeadingClass,
        HeadingLevel, Inline, LinkTarget, ListItem, ParsedSource,
        SnapshotAnchor, SnapshotAsset, SnapshotHeading, SnapshotMasthead,
        SnapshotSection, SnapshotSummary, SnapshotTaskStatus, SnapshotView,
        SnapshotWarning, SnapshotWarningKind, SourceFrontmatter,
        SourceIdentity, SourceLocation, StatusCount,
    },
    visibility::{apply_bob_visibility, BobVisibilityContext},
};
use crate::native::{
    capture::{parse_managed_task_log_marker, ManagedTaskLogKind},
    markdown::strictly_closed_frontmatter_end,
    task_status_hooks::TasksSettings,
};
use crate::native::{
    projects::ProjectStatus,
    task_status_groups::{is_badge_row, TITLE_CLOSED},
};
use serde_yaml::Value as YamlValue;

const MAX_TRANSCLUSION_BYTES: usize = 64 * 1024;
const MAX_TRANSCLUSION_LINES: usize = 1_500;

/// Caller-owned, deterministic metadata for one snapshot projection.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct SnapshotProjectionOptions {
    /// Caller-selected PDF identity and visible title, plus render metadata
    /// such as the glyph table and return-link policy. Bob assembly keeps the
    /// identity title (`masthead.title`) distinct from `display_title`.
    pub masthead: SnapshotMasthead,
    /// Caller-selected snapshot day for Bob date chips. Callers compute it
    /// with the existing `env::current_datetime()`/`BOB_NOW` path; tests
    /// pass explicit dates.
    pub snapshot_date: chrono::NaiveDate,
    /// When true, restore done/canceled tasks in a compact appendix with
    /// full managed logs. Reading mode (`false`) omits those subtrees and
    /// condenses logs on the tasks that remain.
    pub include_closed: bool,
    /// Anchor ID of the exact source H1 consumed by the caller's title
    /// selection. Only this heading is removed from a Bob document.
    pub consumed_title_heading: Option<String>,
}

/// The structured view plus lossless root-source data needed by the later Bob layer.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct SnapshotProjection {
    pub view: SnapshotView,
    pub parsed_source: ParsedSource,
    pub title_candidate: Option<TitleCandidate>,
}

/// Project a classified note with one shared source cache and attachment index.
///
/// The returned source record is the exact cached root byte slice used for its digest;
/// consumers can parse Bob fields and finalize `view.anchors` without reopening it.
pub(crate) fn project_snapshot<L: SourceLoader>(
    store: &mut SourceStore<L>,
    note: &ClassifiedNote,
    settings: &TasksSettings,
    options: SnapshotProjectionOptions,
) -> Result<SnapshotProjection, String> {
    let mut attachments =
        AttachmentIndex::new(store.vault_root().to_path_buf());
    let syntax = project_obsidian_syntax(
        store,
        &mut attachments,
        &note.identity,
        settings,
    )?;
    project_snapshot_from_syntax(
        store,
        note,
        settings,
        options,
        attachments,
        syntax,
    )
}

/// Assemble one already-parsed syntax projection. Create-wiring uses this
/// seam to select the title from the exact `TitleCandidate` and parsed
/// frontmatter before Bob finalizes the view, without rereading or parsing
/// the source a second time.
pub(crate) fn project_snapshot_from_syntax<L: SourceLoader>(
    store: &mut SourceStore<L>,
    note: &ClassifiedNote,
    settings: &TasksSettings,
    options: SnapshotProjectionOptions,
    mut attachments: AttachmentIndex,
    syntax: SyntaxProjection,
) -> Result<SnapshotProjection, String> {
    let SyntaxProjection {
        parsed,
        mut blocks,
        pending_embeds,
        warnings,
        anchors,
        title_candidate,
        assets,
        links_resolved,
        links_unresolved,
    } = syntax;

    let mut expander = Expander::new(
        store,
        &mut attachments,
        settings,
        note.identity.clone(),
        anchors,
    );
    expander.warnings = warnings;
    expander.asset_paths.extend(
        assets
            .iter()
            .map(|asset| asset.canonical_source_path.clone()),
    );
    expander.assets = assets;
    expander.summary.links.resolved = links_resolved;
    expander.summary.links.unresolved = links_unresolved;
    let mut request_index = 0;
    expander.expand_blocks(
        &mut blocks,
        &pending_embeds,
        &note.identity,
        &mut request_index,
        true,
    );

    let is_bob = matches!(
        note.kind,
        NoteKind::Area | NoteKind::Project | NoteKind::TerminalProject
    );
    let (mut preamble, mut sections, synthetic_anchor) = if is_bob {
        let (preamble, sections) = sectionize_bob_root(
            &note.identity,
            blocks,
            options.consumed_title_heading.as_deref(),
        );
        (preamble, sections, None)
    } else {
        let blocks = if let Some(selected) =
            options.consumed_title_heading.as_deref()
        {
            blocks
                .into_iter()
                .filter(|block| {
                    !matches!(
                        block,
                        Block::Heading(heading)
                            if heading.level == HeadingLevel::H1
                                && heading.anchor.identity.source == note.identity
                                && heading.anchor.id == selected
                    )
                })
                .collect()
        } else {
            blocks
        };
        let (sections, synthetic_anchor) =
            sectionize_root(&note.identity, blocks, &expander.used_anchor_ids);
        (Vec::new(), sections, synthetic_anchor)
    };
    // Bob content policy follows the root's area/project kind, including
    // terminal projects; ordinary notes keep the plain Obsidian projection.
    // The host policy covers transcluded tasks while each occurrence keeps
    // its actual source identity. Logs, closed-task omission, and
    // self-snapshot filtering rebuild the visible anchor set from the
    // surviving body plus any appendix.
    let mut closed = None;
    let mut bob_omitted_closed = 0;
    let mut bob_omitted_self = 0;
    let mut bob_omitted_sections = 0;
    let mut bob_masthead = options.masthead.clone();
    let mut bob_outcome = None;
    if is_bob {
        let context = BobTaskContext {
            snapshot_date: options.snapshot_date,
        };
        apply_bob_task_rows(&mut preamble, &context);
        for section in &mut sections {
            apply_bob_task_rows(&mut section.blocks, &context);
        }
        // Visibility treats preamble tasks by running the same pass over a
        // temporary section in document order, then the serializer places
        // surviving blocks directly below the masthead without a synthetic
        // Content heading. The preamble section is inserted first so
        // closed-task extraction preserves source order.
        let preamble_index = if preamble.is_empty() {
            None
        } else {
            sections.insert(
                0,
                preamble_section(
                    &note.identity,
                    std::mem::take(&mut preamble),
                    &expander.used_anchor_ids,
                ),
            );
            Some(0)
        };
        let outcome = apply_bob_visibility(
            &mut sections,
            expander.store,
            &note.identity,
            &BobVisibilityContext {
                snapshot_date: options.snapshot_date,
                include_closed: options.include_closed,
            },
        );
        if let Some(index) = preamble_index {
            preamble = std::mem::take(&mut sections[index].blocks);
            sections.remove(index);
        }
        if matches!(note.kind, NoteKind::Project | NoteKind::TerminalProject) {
            let mut project_outcome = take_project_outcome(
                &mut preamble,
                &mut sections,
                &note.identity,
            );
            if let Some(outcome) = &mut project_outcome {
                if !outcome.history.is_empty() {
                    sections.push(outcome_history_section(
                        &note.identity,
                        std::mem::take(&mut outcome.history),
                        &expander.used_anchor_ids,
                    ));
                }
            }
            bob_outcome = project_outcome;
        }
        apply_scheduled_lanes(
            &mut preamble,
            &mut sections,
            options.snapshot_date,
        );
        closed = outcome.closed;
        bob_omitted_closed = outcome.omitted_closed_tasks;
        bob_omitted_self = outcome.omitted_self_snapshot_tasks;
        expander.warnings.extend(outcome.warnings);

        bob_omitted_sections = finalize_bob_sections(
            &mut preamble,
            &mut sections,
            bob_omitted_closed,
            options.include_closed,
        );
    }
    if let Some(anchor) = synthetic_anchor {
        expander.used_anchor_ids.insert(anchor.id);
    }
    let mut anchors = Vec::new();
    collect_block_anchors(&preamble, &mut anchors);
    for section in &sections {
        collect_section_anchors(section, &mut anchors);
    }
    if let Some(closed_section) = &closed {
        collect_section_anchors(closed_section, &mut anchors);
    }
    let (bob_schedule_logs, bob_work_logs) =
        count_visible_log_entries(&preamble, &sections, closed.as_ref());
    let mut summary = expander.summary;
    summary.sections = sections.len() as u64;
    summary.assets = unique_asset_count(&expander.assets) as u64;
    summary.missing_images = expander
        .warnings
        .iter()
        .filter(|warning| {
            matches!(
                warning.kind,
                SnapshotWarningKind::MissingImage
                    | SnapshotWarningKind::AmbiguousImage
            )
        })
        .count() as u64;
    count_block_statuses(&preamble, &mut summary.task_counts);
    count_task_statuses(&sections, &mut summary.task_counts);
    if let Some(closed_section) = &closed {
        count_task_statuses(
            std::slice::from_ref(closed_section),
            &mut summary.task_counts,
        );
    }
    summary.omitted_closed_tasks = bob_omitted_closed;
    summary.omitted_self_snapshot_tasks = bob_omitted_self;
    summary.omitted_sections = summary
        .omitted_sections
        .saturating_add(bob_omitted_sections);
    summary.schedule_logs = bob_schedule_logs;
    summary.work_logs = bob_work_logs;
    if is_bob {
        compose_bob_masthead(
            &mut bob_masthead,
            &note.identity,
            &note.kind,
            parsed.frontmatter.as_ref(),
            bob_outcome.as_ref(),
            &summary,
            options.snapshot_date,
            options.include_closed,
            expander.store,
            settings,
        );
    }

    let view = SnapshotView {
        source: note.identity.clone(),
        masthead: bob_masthead,
        preamble,
        sections,
        closed,
        assets: expander.assets,
        summary,
        warnings: expander.warnings,
        anchors,
    };
    Ok(SnapshotProjection {
        view,
        parsed_source: parsed,
        title_candidate,
    })
}

/// Bob-layer integration seam for create-wiring. The caller supplies the
/// selected identity/display titles, render metadata, explicit snapshot
/// date, closed-task mode, and (when selected) the exact H1 anchor to consume.
/// Area/project behavior is chosen from `note.kind`; ordinary notes retain
/// their existing Obsidian-only body projection. `view.summary` reports the
/// shown task counts, closed/self-snapshot omissions, surviving sections,
/// and schedule/work-log entry counts.
pub(crate) fn project_bob_snapshot<L: SourceLoader>(
    store: &mut SourceStore<L>,
    note: &ClassifiedNote,
    settings: &TasksSettings,
    options: SnapshotProjectionOptions,
) -> Result<SnapshotProjection, String> {
    project_snapshot(store, note, settings, options)
}

struct Expander<'a, L: SourceLoader> {
    store: &'a mut SourceStore<L>,
    attachments: &'a mut AttachmentIndex,
    settings: &'a TasksSettings,
    root: SourceIdentity,
    vault_root: PathBuf,
    vault_name: String,
    fragment_indexes: HashMap<PathBuf, FragmentIndex>,
    used_anchor_ids: BTreeSet<String>,
    warnings: Vec<SnapshotWarning>,
    assets: Vec<SnapshotAsset>,
    asset_paths: HashSet<PathBuf>,
    summary: SnapshotSummary,
    next_occurrence: u64,
}

impl<'a, L: SourceLoader> Expander<'a, L> {
    fn new(
        store: &'a mut SourceStore<L>,
        attachments: &'a mut AttachmentIndex,
        settings: &'a TasksSettings,
        root: SourceIdentity,
        root_anchors: Vec<SnapshotAnchor>,
    ) -> Self {
        let vault_root = store.vault_root().to_path_buf();
        let vault_name = store.vault_name().to_string();
        let used_anchor_ids =
            root_anchors.into_iter().map(|anchor| anchor.id).collect();
        Self {
            store,
            attachments,
            settings,
            root,
            vault_root,
            vault_name,
            fragment_indexes: HashMap::new(),
            used_anchor_ids,
            warnings: Vec::new(),
            assets: Vec::new(),
            asset_paths: HashSet::new(),
            summary: SnapshotSummary::default(),
            next_occurrence: 0,
        }
    }

    fn expand_blocks(
        &mut self,
        blocks: &mut Vec<Block>,
        requests: &[PendingEmbed],
        current_source: &SourceIdentity,
        request_index: &mut usize,
        may_expand: bool,
    ) {
        for block in blocks.iter_mut() {
            match block {
                Block::Embed { source, .. } => {
                    let request = requests.get(*request_index).cloned();
                    *request_index += 1;
                    match request {
                        Some(request) if may_expand => {
                            *block = self.expand_one(request, current_source);
                        }
                        Some(request) => {
                            *block =
                                self.nested_fallback(request, current_source);
                        }
                        None => {
                            self.warn(
                                SnapshotWarningKind::InvalidSource,
                                current_source,
                                None,
                                format!(
                                    "embed placeholder '{source}' has no resolved source record"
                                ),
                            );
                            *block = Block::Paragraph(vec![Inline::Text(
                                source.clone(),
                            )]);
                        }
                    }
                }
                Block::List { items, .. } => {
                    for item in items {
                        self.expand_inline_embeds(
                            &mut item.content,
                            requests,
                            current_source,
                            request_index,
                            may_expand,
                        );
                        self.expand_blocks(
                            &mut item.children,
                            requests,
                            current_source,
                            request_index,
                            may_expand,
                        );
                    }
                }
                Block::Task(task) => {
                    self.expand_inline_embeds(
                        &mut task.content,
                        requests,
                        current_source,
                        request_index,
                        may_expand,
                    );
                    for chip in &mut task.chips {
                        self.expand_inline_embeds(
                            &mut chip.content,
                            requests,
                            current_source,
                            request_index,
                            may_expand,
                        );
                    }
                    self.expand_blocks(
                        &mut task.children,
                        requests,
                        current_source,
                        request_index,
                        may_expand,
                    );
                }
                Block::Paragraph(inlines) | Block::EmptyState(inlines) => {
                    self.expand_inline_embeds(
                        inlines,
                        requests,
                        current_source,
                        request_index,
                        may_expand,
                    );
                }
                Block::Heading(heading) => self.expand_inline_embeds(
                    &mut heading.content,
                    requests,
                    current_source,
                    request_index,
                    may_expand,
                ),
                Block::LabeledRow { label, items } => {
                    self.expand_inline_embeds(
                        label,
                        requests,
                        current_source,
                        request_index,
                        may_expand,
                    );
                    for item in items {
                        self.expand_inline_embeds(
                            item,
                            requests,
                            current_source,
                            request_index,
                            may_expand,
                        );
                    }
                }
                Block::LogRow {
                    label,
                    date,
                    content,
                    ..
                } => {
                    self.expand_inline_embeds(
                        label,
                        requests,
                        current_source,
                        request_index,
                        may_expand,
                    );
                    if let Some(date) = date {
                        self.expand_inline_embeds(
                            date,
                            requests,
                            current_source,
                            request_index,
                            may_expand,
                        );
                    }
                    self.expand_inline_embeds(
                        content,
                        requests,
                        current_source,
                        request_index,
                        may_expand,
                    );
                }
                Block::Callout { blocks, .. }
                | Block::Compact { blocks }
                | Block::LiveQuery { blocks } => self.expand_blocks(
                    blocks,
                    requests,
                    current_source,
                    request_index,
                    may_expand,
                ),
                _ => {}
            }
        }
    }

    fn expand_inline_embeds(
        &mut self,
        inlines: &mut [Inline],
        requests: &[PendingEmbed],
        current_source: &SourceIdentity,
        request_index: &mut usize,
        may_expand: bool,
    ) {
        for inline in inlines {
            match inline {
                Inline::EmbeddedBlock(block) => {
                    let source = match block.as_ref() {
                        Block::Embed { source, .. } => source.clone(),
                        _ => continue,
                    };
                    let request = requests.get(*request_index).cloned();
                    *request_index += 1;
                    let expanded = match request {
                        Some(request) if may_expand => {
                            self.expand_one(request, current_source)
                        }
                        Some(request) => {
                            self.nested_fallback(request, current_source)
                        }
                        None => {
                            self.warn(
                                SnapshotWarningKind::InvalidSource,
                                current_source,
                                None,
                                format!(
                                    "inline embed placeholder '{source}' has no resolved source record"
                                ),
                            );
                            Block::Paragraph(vec![Inline::Text(source)])
                        }
                    };
                    *block = Box::new(expanded);
                }
                Inline::Link { label, .. }
                | Inline::Emphasis(label)
                | Inline::Strong(label)
                | Inline::Strike(label)
                | Inline::Muted(label) => self.expand_inline_embeds(
                    label,
                    requests,
                    current_source,
                    request_index,
                    may_expand,
                ),
                Inline::Chip(chip) => self.expand_inline_embeds(
                    &mut chip.content,
                    requests,
                    current_source,
                    request_index,
                    may_expand,
                ),
                Inline::Text(_)
                | Inline::Anchor(_)
                | Inline::Code(_)
                | Inline::Image { .. }
                | Inline::Break => {}
            }
        }
    }

    fn expand_one(
        &mut self,
        request: PendingEmbed,
        requesting_source: &SourceIdentity,
    ) -> Block {
        if request.same_note
            || request.canonical_path == self.root.canonical_path
        {
            self.summary.guarded_omissions += 1;
            self.warn(
                SnapshotWarningKind::CyclicTransclusion,
                requesting_source,
                request.origin_line,
                format!("not including cyclic embed '{}'", request.label),
            );
            return self.fallback_link(&request);
        }

        let bytes = match self.store.read_canonical(&request.canonical_path) {
            Ok(bytes) => bytes,
            Err(error) => {
                self.warn(
                    SnapshotWarningKind::ReadFailure,
                    requesting_source,
                    request.origin_line,
                    format!("cannot include '{}': {error}", request.label),
                );
                return self.fallback_link(&request);
            }
        };
        let text = String::from_utf8_lossy(&bytes).into_owned();
        let source = match source_identity(
            &self.vault_root,
            &request.canonical_path,
        ) {
            Some(source) => source,
            None => {
                self.warn(
                    SnapshotWarningKind::InvalidSource,
                    requesting_source,
                    request.origin_line,
                    format!(
                        "cannot include '{}': resolved source is outside the vault",
                        request.label
                    ),
                );
                return self.fallback_link(&request);
            }
        };
        let index = self.fragment_index(&request.canonical_path, &text);
        let (selected, first_line) = match select_extent(
            &text,
            request.fragment.as_ref(),
            &index,
        ) {
            Ok(selection) => selection,
            Err(FragmentSelectionError::Missing) => {
                self.warn(
                    SnapshotWarningKind::MissingFragment,
                    requesting_source,
                    request.origin_line,
                    format!(
                        "cannot include '{}': fragment was not found",
                        request.label
                    ),
                );
                return self.fallback_link(&request);
            }
            Err(FragmentSelectionError::Ambiguous { count, heading }) => {
                let fragment = request
                    .fragment
                    .as_ref()
                    .map(fragment_display)
                    .unwrap_or_default();
                self.warn(
                    SnapshotWarningKind::AmbiguousFragment,
                    requesting_source,
                    request.origin_line,
                    format!(
                        "cannot include '{}': {count} {} fragments match '{fragment}'",
                        request.label,
                        if heading { "heading" } else { "block" }
                    ),
                );
                return self.fallback_link(&request);
            }
            Err(FragmentSelectionError::InvalidExtent) => {
                self.warn(
                    SnapshotWarningKind::InvalidSource,
                    requesting_source,
                    request.origin_line,
                    format!(
                        "cannot include '{}': source extent is invalid",
                        request.label
                    ),
                );
                return self.fallback_link(&request);
            }
        };
        let selected_lines = line_count(selected);
        if selected.len() > MAX_TRANSCLUSION_BYTES
            || selected_lines > MAX_TRANSCLUSION_LINES
        {
            self.summary.guarded_omissions += 1;
            self.warn(
                SnapshotWarningKind::OversizedTransclusion,
                requesting_source,
                request.origin_line,
                format!(
                    "not including '{}': selected content is {} bytes and {} lines (limits: {} bytes and {} lines)",
                    request.label,
                    selected.len(),
                    selected_lines,
                    MAX_TRANSCLUSION_BYTES,
                    MAX_TRANSCLUSION_LINES
                ),
            );
            return self.fallback_link(&request);
        }

        let syntax = match project_obsidian_syntax_selection(
            self.store,
            self.attachments,
            &source,
            &bytes,
            selected,
            first_line,
            self.settings,
        ) {
            Ok(syntax) => syntax,
            Err(error) => {
                self.warn(
                    SnapshotWarningKind::ReadFailure,
                    requesting_source,
                    request.origin_line,
                    format!("cannot project '{}': {error}", request.label),
                );
                return self.fallback_link(&request);
            }
        };
        self.summary.links.resolved += syntax.links_resolved;
        self.summary.links.unresolved += syntax.links_unresolved;
        self.warnings.extend(syntax.warnings.iter().cloned());
        self.add_assets(&syntax.assets);

        let SyntaxProjection {
            mut blocks,
            pending_embeds,
            anchors,
            ..
        } = syntax;
        let mut nested_index = 0;
        self.expand_blocks(
            &mut blocks,
            &pending_embeds,
            &source,
            &mut nested_index,
            false,
        );

        self.next_occurrence += 1;
        let prefix = self.next_occurrence;
        let (anchor_map, unique_anchor_ids, emitted_anchors) =
            self.namespace_anchors(&anchors, prefix);
        remap_block_anchors(&mut blocks, &anchor_map);
        rewrite_local_links(
            &mut blocks,
            &source,
            &unique_anchor_ids,
            &index,
            &self.vault_name,
        );
        self.summary.transclusions += 1;
        // The emitted anchor nodes are already rewritten into `blocks`; this assertion
        // keeps their source identity and presentation IDs paired at assembly time.
        debug_assert!(emitted_anchors.iter().all(|anchor| {
            anchor.identity.source == source
                && anchor.id.starts_with(&format!("embed-{prefix}-"))
        }));
        Block::Embed {
            source: request.label,
            blocks,
        }
    }

    fn nested_fallback(
        &mut self,
        request: PendingEmbed,
        requesting_source: &SourceIdentity,
    ) -> Block {
        let cyclic = request.same_note
            || request.canonical_path == requesting_source.canonical_path
            || request.canonical_path == self.root.canonical_path;
        let kind = if cyclic {
            SnapshotWarningKind::CyclicTransclusion
        } else {
            SnapshotWarningKind::NestedTransclusion
        };
        self.summary.guarded_omissions += 1;
        self.warn(
            kind,
            requesting_source,
            request.origin_line,
            if cyclic {
                format!("not following cyclic nested embed '{}'", request.label)
            } else {
                format!("not following nested embed '{}'", request.label)
            },
        );
        self.fallback_link(&request)
    }

    fn fallback_link(&self, request: &PendingEmbed) -> Block {
        let path = request.vault_relative.to_string_lossy().replace('\\', "/");
        let uri = obsidian_uri_with_fragment(
            &self.vault_name,
            &path,
            request.fragment.as_ref(),
        );
        Block::Paragraph(vec![Inline::Link {
            label: vec![Inline::Text(request.label.clone())],
            target: LinkTarget::ObsidianUri(uri),
        }])
    }

    fn fragment_index(&mut self, path: &Path, text: &str) -> FragmentIndex {
        self.fragment_indexes
            .entry(path.to_path_buf())
            .or_insert_with(|| build_fragment_index(text))
            .clone()
    }

    fn namespace_anchors(
        &mut self,
        anchors: &[SnapshotAnchor],
        occurrence: u64,
    ) -> (
        BTreeMap<(AnchorIdentity, String), SnapshotAnchor>,
        BTreeMap<AnchorIdentity, String>,
        Vec<SnapshotAnchor>,
    ) {
        let mut by_identity: BTreeMap<AnchorIdentity, Vec<String>> =
            BTreeMap::new();
        let mut by_original = BTreeMap::new();
        let mut emitted = Vec::new();
        for anchor in anchors {
            let base = format!("embed-{occurrence}-{}", anchor.id);
            let id = self.claim_anchor_id(&base);
            let namespaced = SnapshotAnchor {
                identity: anchor.identity.clone(),
                id,
            };
            by_identity
                .entry(anchor.identity.clone())
                .or_default()
                .push(namespaced.id.clone());
            by_original.insert(
                (anchor.identity.clone(), anchor.id.clone()),
                namespaced.clone(),
            );
            emitted.push(namespaced);
        }
        let unique = by_identity
            .into_iter()
            .filter_map(|(identity, ids)| {
                (ids.len() == 1).then(|| (identity, ids[0].clone()))
            })
            .collect();
        (by_original, unique, emitted)
    }

    fn claim_anchor_id(&mut self, base: &str) -> String {
        if self.used_anchor_ids.insert(base.to_string()) {
            return base.to_string();
        }
        let mut suffix = 2_u64;
        loop {
            let candidate = format!("{base}-{suffix}");
            if self.used_anchor_ids.insert(candidate.clone()) {
                return candidate;
            }
            suffix += 1;
        }
    }

    fn add_assets(&mut self, assets: &[SnapshotAsset]) {
        for asset in assets {
            if self.asset_paths.insert(asset.canonical_source_path.clone()) {
                self.assets.push(asset.clone());
            }
        }
    }

    fn warn(
        &mut self,
        kind: SnapshotWarningKind,
        source: &SourceIdentity,
        line: Option<usize>,
        message: String,
    ) {
        self.warnings.push(SnapshotWarning {
            kind,
            origin: Some(SourceLocation {
                source: source.clone(),
                line,
            }),
            message,
        });
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum FragmentSelectionError {
    Missing,
    Ambiguous { count: usize, heading: bool },
    InvalidExtent,
}

fn select_extent<'a>(
    text: &'a str,
    fragment: Option<&LinkFragment>,
    index: &FragmentIndex,
) -> Result<(&'a str, usize), FragmentSelectionError> {
    let Some(fragment) = fragment else {
        let lines = text.lines().collect::<Vec<_>>();
        let Some(frontmatter_end) = strictly_closed_frontmatter_end(&lines)
        else {
            return Ok((text, 1));
        };
        let start = text
            .split_inclusive('\n')
            .take(frontmatter_end + 1)
            .map(str::len)
            .sum::<usize>();
        let first_line = frontmatter_end + 2;
        return text
            .get(start..)
            .map(|body| (body, first_line))
            .ok_or(FragmentSelectionError::InvalidExtent);
    };
    match index.lookup(fragment) {
        FragmentLookup::Missing => Err(FragmentSelectionError::Missing),
        FragmentLookup::Ambiguous { count } => {
            Err(FragmentSelectionError::Ambiguous {
                count,
                heading: matches!(fragment, LinkFragment::Heading(_)),
            })
        }
        FragmentLookup::Found(span) => text
            .get(span.byte_start..span.byte_end)
            .map(|selected| (selected, span.first_line))
            .ok_or(FragmentSelectionError::InvalidExtent),
    }
}

fn line_count(text: &str) -> usize {
    text.lines().count()
}

fn fragment_display(fragment: &LinkFragment) -> String {
    match fragment {
        LinkFragment::Heading(heading) => heading.clone(),
        LinkFragment::Block(id) => format!("^{id}"),
    }
}

fn sectionize_root(
    source: &SourceIdentity,
    blocks: Vec<Block>,
    reserved_anchor_ids: &BTreeSet<String>,
) -> (Vec<SnapshotSection>, Option<SnapshotAnchor>) {
    let mut sections = Vec::new();
    let mut current: Option<SnapshotSection> = None;
    let mut prelude = Vec::new();
    for block in blocks {
        let begins_section = matches!(
            &block,
            Block::Heading(SnapshotHeading {
                level: HeadingLevel::H1,
                ..
            })
        );
        if begins_section {
            if current
                .as_ref()
                .is_some_and(|section| !section.blocks.is_empty())
            {
                sections.push(current.take().expect("checked section"));
            }
            if !prelude.is_empty() {
                let heading = content_heading(source, reserved_anchor_ids);
                sections.push(SnapshotSection {
                    source: source.clone(),
                    heading: heading.clone(),
                    blocks: std::mem::take(&mut prelude),
                });
            }
            let Block::Heading(heading) = block else {
                unreachable!("section start is a heading")
            };
            current = Some(SnapshotSection {
                source: source.clone(),
                heading,
                blocks: Vec::new(),
            });
        } else if let Some(section) = &mut current {
            section.blocks.push(block);
        } else {
            prelude.push(block);
        }
    }
    if current
        .as_ref()
        .is_some_and(|section| !section.blocks.is_empty())
    {
        sections.push(current.expect("checked section"));
    }
    let synthetic = if prelude.is_empty() && sections.is_empty() {
        None
    } else if !prelude.is_empty() {
        let heading = content_heading(source, reserved_anchor_ids);
        sections.push(SnapshotSection {
            source: source.clone(),
            heading: heading.clone(),
            blocks: prelude,
        });
        Some(heading.anchor)
    } else {
        None
    };
    (sections, synthetic)
}

/// Assemble Bob headings after consuming only the title heading selected by
/// the caller. Intro blocks remain a preamble, and when a title H1 is
/// consumed its source H2 headings become the document's section headings.
fn sectionize_bob_root(
    source: &SourceIdentity,
    blocks: Vec<Block>,
    consumed_title_heading: Option<&str>,
) -> (Vec<Block>, Vec<SnapshotSection>) {
    let mut preamble = Vec::new();
    let mut sections = Vec::new();
    let mut current: Option<SnapshotSection> = None;
    let mut consumed_title = false;

    for block in blocks {
        if let Block::Heading(mut heading) = block {
            let is_selected_title = heading.level == HeadingLevel::H1
                && heading.anchor.identity.source == *source
                && consumed_title_heading
                    .is_some_and(|selected| heading.anchor.id == selected);
            if is_selected_title {
                if let Some(section) = current.take() {
                    sections.push(section);
                }
                consumed_title = true;
                continue;
            }

            let starts_section = heading.level == HeadingLevel::H1
                || (consumed_title && heading.level == HeadingLevel::H2);
            if starts_section {
                if let Some(section) = current.take() {
                    sections.push(section);
                }
                heading.level = HeadingLevel::H1;
                current = Some(SnapshotSection {
                    source: source.clone(),
                    heading,
                    blocks: Vec::new(),
                });
            } else if let Some(section) = &mut current {
                if consumed_title && heading.level == HeadingLevel::H3 {
                    heading.level = HeadingLevel::H2;
                }
                section.blocks.push(Block::Heading(heading));
            } else {
                if consumed_title && heading.level == HeadingLevel::H3 {
                    heading.level = HeadingLevel::H2;
                }
                preamble.push(Block::Heading(heading));
            }
        } else if let Some(section) = &mut current {
            section.blocks.push(block);
        } else {
            preamble.push(block);
        }
    }
    if let Some(section) = current {
        sections.push(section);
    }
    (preamble, sections)
}

fn outcome_history_section(
    source: &SourceIdentity,
    blocks: Vec<Block>,
    reserved: &BTreeSet<String>,
) -> SnapshotSection {
    SnapshotSection {
        source: source.clone(),
        heading: synthetic_heading(
            source,
            "Outcome history",
            "__snapshot_outcome_history",
            "snapshot-outcome-history",
            reserved,
        ),
        blocks,
    }
}

fn preamble_section(
    source: &SourceIdentity,
    blocks: Vec<Block>,
    reserved: &BTreeSet<String>,
) -> SnapshotSection {
    SnapshotSection {
        source: source.clone(),
        heading: synthetic_heading(
            source,
            "Preamble",
            "__snapshot_preamble",
            "snapshot-preamble",
            reserved,
        ),
        blocks,
    }
}

fn synthetic_heading(
    source: &SourceIdentity,
    label: &str,
    fragment: &str,
    base_id: &str,
    reserved: &BTreeSet<String>,
) -> SnapshotHeading {
    let mut id = base_id.to_string();
    let mut suffix = 2_u32;
    while reserved.contains(&id) {
        id = format!("{base_id}-{suffix}");
        suffix += 1;
    }
    SnapshotHeading {
        level: HeadingLevel::H1,
        content: vec![Inline::Text(label.to_string())],
        class: HeadingClass::Normal,
        anchor: SnapshotAnchor {
            identity: AnchorIdentity {
                source: source.clone(),
                kind: AnchorKind::Heading,
                fragment: fragment.to_string(),
            },
            id,
        },
    }
}

fn compose_bob_masthead<L: SourceLoader>(
    masthead: &mut SnapshotMasthead,
    source: &SourceIdentity,
    kind: &NoteKind,
    frontmatter: Option<&SourceFrontmatter>,
    outcome: Option<&ProjectOutcome>,
    summary: &SnapshotSummary,
    snapshot_date: chrono::NaiveDate,
    include_closed: bool,
    store: &mut SourceStore<L>,
    settings: &TasksSettings,
) {
    let metadata = frontmatter.map(|front| &front.metadata);
    let category = if *kind == NoteKind::Area {
        "AREA"
    } else {
        "PROJECT"
    };
    let parent = metadata
        .and_then(|metadata| parent_label(metadata, source, store, settings));
    masthead.kicker = vec![Inline::Text(match parent {
        Some(parent) => format!("{category} · {parent}"),
        None => category.to_string(),
    })];

    if let Some(outcome) = outcome {
        if !outcome.content.is_empty() {
            masthead.dek = Some(outcome.content.clone());
        }
        for chip in &outcome.chips {
            masthead.meta.push(vec![Inline::Chip(chip.clone())]);
        }
        if outcome.status != SnapshotTaskStatus::Open {
            masthead.meta.push(vec![Inline::Chip(Chip {
                content: vec![Inline::Text(
                    outcome.status.as_str().to_string(),
                )],
                kind: ChipKind::Normal,
                target: None,
            })]);
        }
        masthead.meta.extend(outcome.relationships.iter().cloned());
    } else if let Some(description) = metadata
        .and_then(|metadata| metadata_value(metadata, "description"))
        .and_then(yaml_scalar_text)
        .filter(|description| !description.trim().is_empty())
    {
        masthead.dek = Some(vec![Inline::Text(description)]);
    }

    let created = metadata
        .and_then(|metadata| metadata_value(metadata, "created"))
        .and_then(yaml_scalar_text)
        .and_then(|value| {
            crate::native::task_fields::parse_strict_calendar_date(&value)
        })
        .or_else(|| outcome.and_then(|outcome| outcome.created));
    if let Some(created) = created {
        masthead.meta.push(vec![Inline::Text(format!(
            "Started {}",
            date_label(created)
        ))]);
    }

    if outcome.is_none() && *kind == NoteKind::TerminalProject {
        if let Some(status) = metadata
            .and_then(|metadata| metadata_value(metadata, "status"))
            .and_then(yaml_scalar_text)
        {
            let status = ProjectStatus::parse(Some(&status));
            if matches!(status, ProjectStatus::Done | ProjectStatus::Canceled) {
                masthead.meta.push(vec![Inline::Chip(Chip {
                    content: vec![Inline::Text(status.label().to_string())],
                    kind: ChipKind::Normal,
                    target: None,
                })]);
            }
        }
    }

    const STATUSES: [SnapshotTaskStatus; 6] = [
        SnapshotTaskStatus::Open,
        SnapshotTaskStatus::Next,
        SnapshotTaskStatus::Wip,
        SnapshotTaskStatus::Blocked,
        SnapshotTaskStatus::Done,
        SnapshotTaskStatus::Canceled,
    ];
    let mut stats = Vec::with_capacity(7);
    for status in STATUSES {
        let count = summary.task_counts.get(&status).copied().unwrap_or(0);
        if status == SnapshotTaskStatus::Blocked {
            stats.push(StatusCount { status, count });
            let scheduled = summary
                .task_counts
                .get(&SnapshotTaskStatus::Scheduled)
                .copied()
                .unwrap_or(0);
            if scheduled > 0 {
                stats.push(StatusCount {
                    status: SnapshotTaskStatus::Scheduled,
                    count: scheduled,
                });
            }
        } else {
            stats.push(StatusCount { status, count });
        }
    }
    masthead.stats = stats;
    masthead.omitted = Some(vec![Inline::Text(format!(
        "Omitted: {} closed · {} self-snapshot",
        summary.omitted_closed_tasks, summary.omitted_self_snapshot_tasks
    ))]);

    let date = date_label(snapshot_date);
    let mut snapshot_line = vec![Inline::Text("Snapshot of ".to_string())];
    snapshot_line.extend(masthead.display_title.iter().cloned());
    snapshot_line.push(Inline::Text(format!(" · {} · {date}", source.route)));
    masthead.snapshot_line = snapshot_line;
    masthead.footer = vec![Inline::Text(format!(
        "Source: {} · {date}{}",
        source.route,
        if include_closed {
            " · closed tasks included"
        } else {
            ""
        }
    ))];
}

fn date_label(date: chrono::NaiveDate) -> String {
    date.format("%b %-d, %Y").to_string()
}

fn metadata_value<'a>(
    metadata: &'a YamlValue,
    key: &str,
) -> Option<&'a YamlValue> {
    metadata
        .as_mapping()?
        .get(YamlValue::String(key.to_string()))
}

fn yaml_scalar_text(value: &YamlValue) -> Option<String> {
    match value {
        YamlValue::String(value) => Some(value.clone()),
        YamlValue::Sequence(items) => {
            let item = items.first()?;
            let inner = yaml_scalar_text(item)?;
            if matches!(item, YamlValue::Sequence(_)) {
                Some(format!("[[{inner}]]"))
            } else {
                Some(inner)
            }
        }
        YamlValue::Tagged(tagged) => yaml_scalar_text(&tagged.value),
        _ => None,
    }
}

fn parent_label<L: SourceLoader>(
    metadata: &YamlValue,
    source: &SourceIdentity,
    store: &mut SourceStore<L>,
    settings: &TasksSettings,
) -> Option<String> {
    let raw = yaml_scalar_text(metadata_value(metadata, "parent")?)?;
    let parsed = super::resolve::parse_link_target(&raw);
    if let Some(alias) = parsed
        .alias
        .as_ref()
        .filter(|alias| !alias.trim().is_empty())
    {
        return Some(alias.clone());
    }
    if parsed.note.is_empty() {
        return None;
    }

    // Reuse the syntax projector's title-candidate rules with the same
    // SourceStore so linked, emphasized, commented, and fenced titles all
    // project to readable text. The projection shares the cached source
    // bytes (one-read invariant), never expands embeds, and never recurses
    // into parent rendering.
    let readable = store
        .vault_notes()
        .resolve_note(Path::new(&source.vault_relative_path), &parsed)
        .ok()
        .and_then(|resolved| {
            let vault_root = store.vault_root().to_path_buf();
            let identity =
                source_identity(&vault_root, &resolved.canonical_path)?;
            let mut attachments =
                AttachmentIndex::new(store.vault_root().to_path_buf());
            project_obsidian_syntax(
                store,
                &mut attachments,
                &identity,
                settings,
            )
            .ok()
            .and_then(|projection| projection.title_candidate)
            .map(|candidate| candidate.display_text)
            .filter(|text| !text.trim().is_empty())
        });
    readable.or_else(|| {
        parsed
            .note
            .rsplit('/')
            .next()
            .map(|name| name.trim_end_matches(".md").replace(['_', '-'], " "))
            .filter(|name| !name.trim().is_empty())
    })
}

fn finalize_bob_sections(
    preamble: &mut Vec<Block>,
    sections: &mut Vec<SnapshotSection>,
    omitted_closed: u64,
    include_closed: bool,
) -> u64 {
    let mut omitted_sections = prune_bob_blocks(preamble);
    let mut retained: Vec<SnapshotSection> = Vec::with_capacity(sections.len());
    for mut section in std::mem::take(sections) {
        omitted_sections += prune_bob_blocks(&mut section.blocks);
        let title = plain_inline_text(&section.heading.content);
        if title == TITLE_CLOSED {
            omitted_sections += 1;
            if let Some(previous) = retained.last_mut() {
                previous.blocks.append(&mut section.blocks);
            } else {
                preamble.append(&mut section.blocks);
            }
            continue;
        }
        if is_snapshot_lane_title(&title) {
            section.heading.class = HeadingClass::Lane;
        }
        if title == "Tasks" && !has_task(&section.blocks) {
            let mut empty = "No open tasks".to_string();
            if !include_closed && omitted_closed > 0 {
                empty
                    .push_str(&format!(" · {omitted_closed} closed not shown"));
            }
            section
                .blocks
                .push(Block::EmptyState(vec![Inline::Text(empty)]));
        }
        if section.blocks.is_empty() && title != "Tasks" {
            omitted_sections += 1;
        } else {
            retained.push(section);
        }
    }
    *sections = retained;
    omitted_sections
}

fn prune_bob_blocks(blocks: &mut Vec<Block>) -> u64 {
    for block in blocks.iter_mut() {
        match block {
            Block::Task(task) => {
                prune_bob_inlines(&mut task.content);
                for chip in task.chips.iter_mut() {
                    prune_bob_inlines(&mut chip.content);
                }
                let mut children = std::mem::take(&mut task.children);
                prune_bob_blocks(&mut children);
                task.children = children;
            }
            Block::Paragraph(inlines) | Block::EmptyState(inlines) => {
                prune_bob_inlines(inlines);
            }
            Block::Heading(heading) => {
                prune_bob_inlines(&mut heading.content);
            }
            Block::List { items, .. } => {
                for item in items {
                    prune_bob_inlines(&mut item.content);
                    prune_bob_blocks(&mut item.children);
                }
            }
            Block::LabeledRow { label, items } => {
                prune_bob_inlines(label);
                for item in items {
                    prune_bob_inlines(item);
                }
            }
            Block::LogRow {
                label,
                date,
                content,
                ..
            } => {
                prune_bob_inlines(label);
                if let Some(date) = date {
                    prune_bob_inlines(date);
                }
                prune_bob_inlines(content);
            }
            Block::MissingImage { label } => {
                prune_bob_inlines(label);
            }
            Block::Embed { blocks, .. }
            | Block::LiveQuery { blocks }
            | Block::Compact { blocks } => {
                prune_bob_blocks(blocks);
            }
            Block::Callout { title, blocks, .. } => {
                prune_bob_inlines(title);
                prune_bob_blocks(blocks);
            }
            _ => {}
        }
    }

    for block in blocks.iter_mut() {
        if let Block::Heading(heading) = block {
            let title = plain_inline_text(&heading.content);
            if is_snapshot_lane_title(&title) {
                heading.class = HeadingClass::Lane;
            }
        }
    }

    let mut remove = vec![false; blocks.len()];
    let mut omitted_heading = vec![false; blocks.len()];
    for index in 0..blocks.len() {
        match &blocks[index] {
            Block::Paragraph(inlines) if badge_row(inlines) => {
                remove[index] = true;
            }
            Block::Heading(heading) => {
                let title = plain_inline_text(&heading.content);
                if title == TITLE_CLOSED {
                    remove[index] = true;
                    omitted_heading[index] = true;
                    continue;
                }
                let next_heading = (index + 1..blocks.len()).find(|next| {
                    matches!(
                        &blocks[*next],
                        Block::Heading(next_heading)
                            if heading_rank(next_heading.level) <= heading_rank(heading.level)
                    )
                });
                let end = next_heading.unwrap_or(blocks.len());
                if !blocks[index + 1..end].iter().any(block_has_visible_content)
                {
                    remove[index] = true;
                    omitted_heading[index] = true;
                }
            }
            _ => {}
        }
    }
    let removed =
        omitted_heading.iter().filter(|removed| **removed).count() as u64;
    *blocks = std::mem::take(blocks)
        .into_iter()
        .enumerate()
        .filter_map(|(index, block)| (!remove[index]).then_some(block))
        .collect();
    removed
}

fn prune_bob_inlines(inlines: &mut [Inline]) {
    for inline in inlines.iter_mut() {
        match inline {
            Inline::EmbeddedBlock(inner) => {
                let mut nested = vec![std::mem::replace(
                    inner.as_mut(),
                    Block::Paragraph(Vec::new()),
                )];
                prune_bob_blocks(&mut nested);
                **inner = nested.into_iter().next().expect("embedded block");
            }
            Inline::Link { label, .. }
            | Inline::Emphasis(label)
            | Inline::Strong(label)
            | Inline::Strike(label)
            | Inline::Muted(label) => prune_bob_inlines(label),
            Inline::Chip(chip) => prune_bob_inlines(&mut chip.content),
            _ => {}
        }
    }
}

fn badge_row(inlines: &[Inline]) -> bool {
    let mut source = String::new();
    for inline in inlines {
        match inline {
            Inline::Text(text) => source.push_str(text),
            Inline::Code(text) => {
                source.push('`');
                source.push_str(text);
                source.push('`');
            }
            Inline::Link { label, target }
                if matches!(
                    target,
                    LinkTarget::Internal(_) | LinkTarget::ScopedInternal { .. }
                ) && matches!(label.as_slice(), [Inline::Code(_)]) =>
            {
                let Inline::Code(text) = &label[0] else {
                    unreachable!()
                };
                source.push_str(&format!("[`{text}`](#Tasks)"));
            }
            _ => return false,
        }
    }
    is_badge_row(&source)
}

fn plain_inline_text(inlines: &[Inline]) -> String {
    let mut output = String::new();
    for inline in inlines {
        match inline {
            Inline::Text(text) | Inline::Code(text) => output.push_str(text),
            Inline::Link { label, .. }
            | Inline::Emphasis(label)
            | Inline::Strong(label)
            | Inline::Strike(label)
            | Inline::Muted(label) => {
                output.push_str(&plain_inline_text(label))
            }
            Inline::Chip(chip) => {
                output.push_str(&plain_inline_text(&chip.content))
            }
            Inline::Break => output.push(' '),
            Inline::Anchor(_)
            | Inline::Image { .. }
            | Inline::EmbeddedBlock(_) => {}
        }
    }
    output.trim().to_string()
}

fn heading_rank(level: HeadingLevel) -> usize {
    match level {
        HeadingLevel::H1 => 1,
        HeadingLevel::H2 => 2,
        HeadingLevel::H3 => 3,
    }
}

fn block_has_visible_content(block: &Block) -> bool {
    match block {
        Block::Heading(_) => false,
        Block::Paragraph(content) | Block::EmptyState(content) => {
            !plain_inline_text(content).is_empty()
                || inline_has_visible_content(content)
        }
        Block::List { items, .. } => items.iter().any(|item| {
            !plain_inline_text(&item.content).is_empty()
                || inline_has_visible_content(&item.content)
                || item.children.iter().any(block_has_visible_content)
        }),
        Block::Task(_) | Block::Code { .. } | Block::Image { .. } => true,
        Block::LabeledRow { label, items } => {
            !items.is_empty()
                || inline_has_visible_content(label)
                || items.iter().any(|item| inline_has_visible_content(item))
        }
        Block::LogRow { .. } | Block::MissingImage { .. } => true,
        Block::Embed { blocks, .. }
        | Block::LiveQuery { blocks }
        | Block::Compact { blocks }
        | Block::Callout { blocks, .. } => {
            blocks.iter().any(block_has_visible_content)
        }
    }
}

fn inline_has_visible_content(inlines: &[Inline]) -> bool {
    inlines.iter().any(|inline| match inline {
        Inline::EmbeddedBlock(inner) => {
            block_has_visible_content(inner.as_ref())
        }
        Inline::Link { label, .. }
        | Inline::Emphasis(label)
        | Inline::Strong(label)
        | Inline::Strike(label)
        | Inline::Muted(label) => inline_has_visible_content(label),
        Inline::Chip(chip) => inline_has_visible_content(&chip.content),
        _ => false,
    })
}

fn has_task(blocks: &[Block]) -> bool {
    blocks.iter().any(|block| match block {
        Block::Task(_) => true,
        Block::Paragraph(inlines) | Block::EmptyState(inlines) => {
            has_task_in_inlines(inlines)
        }
        Block::Heading(heading) => has_task_in_inlines(&heading.content),
        Block::List { items, .. } => items.iter().any(|item| {
            has_task_in_inlines(&item.content) || has_task(&item.children)
        }),
        Block::LabeledRow { label, items } => {
            has_task_in_inlines(label)
                || items.iter().any(|item| has_task_in_inlines(item))
        }
        Block::LogRow {
            label,
            date,
            content,
            ..
        } => {
            has_task_in_inlines(label)
                || date.as_ref().is_some_and(|date| has_task_in_inlines(date))
                || has_task_in_inlines(content)
        }
        Block::MissingImage { label } => has_task_in_inlines(label),
        Block::Embed { blocks, .. }
        | Block::LiveQuery { blocks }
        | Block::Compact { blocks } => has_task(blocks),
        Block::Callout { title, blocks, .. } => {
            has_task_in_inlines(title) || has_task(blocks)
        }
        _ => false,
    })
}

fn has_task_in_inlines(inlines: &[Inline]) -> bool {
    inlines.iter().any(|inline| match inline {
        Inline::EmbeddedBlock(inner) => {
            has_task(std::slice::from_ref(inner.as_ref()))
        }
        Inline::Link { label, .. }
        | Inline::Emphasis(label)
        | Inline::Strong(label)
        | Inline::Strike(label)
        | Inline::Muted(label) => has_task_in_inlines(label),
        Inline::Chip(chip) => has_task_in_inlines(&chip.content),
        _ => false,
    })
}

fn count_block_statuses(
    blocks: &[Block],
    counts: &mut BTreeMap<SnapshotTaskStatus, u64>,
) {
    count_blocks(blocks, counts);
}

fn count_visible_log_entries(
    preamble: &[Block],
    sections: &[SnapshotSection],
    closed: Option<&SnapshotSection>,
) -> (u64, u64) {
    let mut counts = (0, 0);
    count_log_blocks(preamble, &mut counts);
    for section in sections {
        count_log_blocks(&section.blocks, &mut counts);
    }
    if let Some(section) = closed {
        count_log_blocks(&section.blocks, &mut counts);
    }
    counts
}

fn count_log_blocks(blocks: &[Block], counts: &mut (u64, u64)) {
    for block in blocks {
        match block {
            Block::Task(task) => {
                for chip in &task.chips {
                    if let Some(text) = chip.content.iter().find_map(|inline| {
                        if let Inline::Text(text) = inline {
                            Some(text.as_str())
                        } else {
                            None
                        }
                    }) && let Some(value) = text
                        .strip_prefix("moved ")
                        .and_then(|value| value.strip_suffix('×'))
                        .and_then(|value| value.parse::<u64>().ok())
                    {
                        counts.0 += value;
                    }
                }
                count_log_inlines(&task.content, counts);
                for chip in &task.chips {
                    count_log_inlines(&chip.content, counts);
                }
                count_log_blocks(&task.children, counts);
            }
            Block::Paragraph(inlines) | Block::EmptyState(inlines) => {
                count_log_inlines(inlines, counts);
            }
            Block::Heading(heading) => {
                count_log_inlines(&heading.content, counts);
            }
            Block::List { items, .. } => {
                for item in items {
                    count_log_inlines(&item.content, counts);
                    if let Some(source) = item.source.as_ref()
                        && let Some(kind) = parse_managed_task_log_marker(
                            source.semantic_line(),
                        )
                    {
                        let entries = item
                            .children
                            .iter()
                            .map(|child| match child {
                                Block::List { items, .. } => items.len() as u64,
                                _ => 0,
                            })
                            .sum::<u64>();
                        match kind {
                            ManagedTaskLogKind::Schedule => counts.0 += entries,
                            ManagedTaskLogKind::Work => counts.1 += entries,
                        }
                    }
                    count_log_blocks(&item.children, counts);
                }
            }
            Block::LogRow {
                label,
                date,
                content,
                older_count,
            } => {
                match plain_inline_text(label).as_str() {
                    "MOVED" => counts.0 += 1,
                    "WORKED" => counts.1 += 1 + older_count.unwrap_or(0),
                    _ => {}
                }
                count_log_inlines(label, counts);
                if let Some(date) = date {
                    count_log_inlines(date, counts);
                }
                count_log_inlines(content, counts);
            }
            Block::LabeledRow { label, items } => {
                count_log_inlines(label, counts);
                for item in items {
                    count_log_inlines(item, counts);
                }
            }
            Block::MissingImage { label } => {
                count_log_inlines(label, counts);
            }
            Block::Embed { blocks, .. }
            | Block::LiveQuery { blocks }
            | Block::Compact { blocks } => count_log_blocks(blocks, counts),
            Block::Callout { title, blocks, .. } => {
                count_log_inlines(title, counts);
                count_log_blocks(blocks, counts);
            }
            _ => {}
        }
    }
}

fn count_log_inlines(inlines: &[Inline], counts: &mut (u64, u64)) {
    for inline in inlines {
        match inline {
            Inline::EmbeddedBlock(inner) => {
                count_log_blocks(std::slice::from_ref(inner.as_ref()), counts);
            }
            Inline::Link { label, .. }
            | Inline::Emphasis(label)
            | Inline::Strong(label)
            | Inline::Strike(label)
            | Inline::Muted(label) => count_log_inlines(label, counts),
            Inline::Chip(chip) => count_log_inlines(&chip.content, counts),
            _ => {}
        }
    }
}

fn content_heading(
    source: &SourceIdentity,
    reserved_anchor_ids: &BTreeSet<String>,
) -> SnapshotHeading {
    let base = "snapshot-content";
    let mut id = base.to_string();
    let mut suffix = 2_u32;
    while reserved_anchor_ids.contains(&id) {
        id = format!("{base}-{suffix}");
        suffix += 1;
    }
    SnapshotHeading {
        level: HeadingLevel::H1,
        content: vec![Inline::Text("Content".to_string())],
        class: HeadingClass::Normal,
        anchor: SnapshotAnchor {
            identity: AnchorIdentity {
                source: source.clone(),
                kind: AnchorKind::Heading,
                fragment: "__snapshot_content".to_string(),
            },
            id,
        },
    }
}

fn collect_section_anchors(
    section: &SnapshotSection,
    anchors: &mut Vec<SnapshotAnchor>,
) {
    anchors.push(section.heading.anchor.clone());
    collect_inline_anchors(&section.heading.content, anchors);
    collect_block_anchors(&section.blocks, anchors);
}

fn collect_block_anchors(blocks: &[Block], anchors: &mut Vec<SnapshotAnchor>) {
    for block in blocks {
        match block {
            Block::Paragraph(inlines) | Block::EmptyState(inlines) => {
                collect_inline_anchors(inlines, anchors)
            }
            Block::Heading(heading) => {
                anchors.push(heading.anchor.clone());
                collect_inline_anchors(&heading.content, anchors);
            }
            Block::Task(task) => {
                if let Some(anchor) = &task.anchor {
                    anchors.push(anchor.clone());
                }
                collect_inline_anchors(&task.content, anchors);
                for chip in &task.chips {
                    collect_inline_anchors(&chip.content, anchors);
                }
                collect_block_anchors(&task.children, anchors);
            }
            Block::List { items, .. } => {
                for item in items {
                    collect_inline_anchors(&item.content, anchors);
                    collect_block_anchors(&item.children, anchors);
                }
            }
            Block::LabeledRow { label, items } => {
                collect_inline_anchors(label, anchors);
                for item in items {
                    collect_inline_anchors(item, anchors);
                }
            }
            Block::LogRow {
                label,
                date,
                content,
                ..
            } => {
                collect_inline_anchors(label, anchors);
                if let Some(date) = date {
                    collect_inline_anchors(date, anchors);
                }
                collect_inline_anchors(content, anchors);
            }
            Block::MissingImage { label } => {
                collect_inline_anchors(label, anchors)
            }
            Block::Embed { blocks, .. }
            | Block::LiveQuery { blocks }
            | Block::Compact { blocks } => {
                collect_block_anchors(blocks, anchors)
            }
            Block::Callout { title, blocks, .. } => {
                collect_inline_anchors(title, anchors);
                collect_block_anchors(blocks, anchors);
            }
            Block::Code { .. } | Block::Image { .. } => {}
        }
    }
}

fn collect_inline_anchors(
    inlines: &[Inline],
    anchors: &mut Vec<SnapshotAnchor>,
) {
    for inline in inlines {
        match inline {
            Inline::Anchor(anchor) => anchors.push(anchor.clone()),
            Inline::Link { label, .. }
            | Inline::Emphasis(label)
            | Inline::Strong(label)
            | Inline::Strike(label)
            | Inline::Muted(label) => collect_inline_anchors(label, anchors),
            Inline::Chip(chip) => {
                collect_inline_anchors(&chip.content, anchors)
            }
            Inline::EmbeddedBlock(block) => collect_block_anchors(
                std::slice::from_ref(block.as_ref()),
                anchors,
            ),
            Inline::Text(_)
            | Inline::Code(_)
            | Inline::Image { .. }
            | Inline::Break => {}
        }
    }
}

fn count_task_statuses(
    sections: &[SnapshotSection],
    counts: &mut BTreeMap<SnapshotTaskStatus, u64>,
) {
    for section in sections {
        count_blocks(&section.blocks, counts);
    }
}

fn count_blocks(
    blocks: &[Block],
    counts: &mut BTreeMap<SnapshotTaskStatus, u64>,
) {
    for block in blocks {
        match block {
            Block::Task(task) => {
                *counts.entry(task.status).or_default() += 1;
                count_inlines(&task.content, counts);
                for chip in &task.chips {
                    count_inlines(&chip.content, counts);
                }
                count_blocks(&task.children, counts);
            }
            Block::Paragraph(inlines) | Block::EmptyState(inlines) => {
                count_inlines(inlines, counts);
            }
            Block::Heading(heading) => {
                count_inlines(&heading.content, counts);
            }
            Block::List { items, .. } => {
                for item in items {
                    count_inlines(&item.content, counts);
                    count_blocks(&item.children, counts);
                }
            }
            Block::LabeledRow { label, items } => {
                count_inlines(label, counts);
                for item in items {
                    count_inlines(item, counts);
                }
            }
            Block::LogRow {
                label,
                date,
                content,
                ..
            } => {
                count_inlines(label, counts);
                if let Some(date) = date {
                    count_inlines(date, counts);
                }
                count_inlines(content, counts);
            }
            Block::MissingImage { label } => {
                count_inlines(label, counts);
            }
            Block::Embed { blocks, .. }
            | Block::LiveQuery { blocks }
            | Block::Callout { blocks, .. }
            | Block::Compact { blocks } => count_blocks(blocks, counts),
            _ => {}
        }
    }
}

fn count_inlines(
    inlines: &[Inline],
    counts: &mut BTreeMap<SnapshotTaskStatus, u64>,
) {
    for inline in inlines {
        match inline {
            Inline::EmbeddedBlock(inner) => {
                count_blocks(std::slice::from_ref(inner.as_ref()), counts);
            }
            Inline::Link { label, .. }
            | Inline::Emphasis(label)
            | Inline::Strong(label)
            | Inline::Strike(label)
            | Inline::Muted(label) => count_inlines(label, counts),
            Inline::Chip(chip) => count_inlines(&chip.content, counts),
            _ => {}
        }
    }
}

fn unique_asset_count(assets: &[SnapshotAsset]) -> usize {
    assets
        .iter()
        .map(|asset| &asset.canonical_source_path)
        .collect::<HashSet<_>>()
        .len()
}

fn remap_block_anchors(
    blocks: &mut [Block],
    mapping: &BTreeMap<(AnchorIdentity, String), SnapshotAnchor>,
) {
    for block in blocks {
        match block {
            Block::Heading(heading) => {
                if let Some(anchor) = mapping.get(&(
                    heading.anchor.identity.clone(),
                    heading.anchor.id.clone(),
                )) {
                    heading.anchor = anchor.clone();
                }
                remap_inline_anchors(&mut heading.content, mapping);
            }
            Block::Task(task) => {
                if let Some(original) = &task.anchor {
                    if let Some(anchor) = mapping
                        .get(&(original.identity.clone(), original.id.clone()))
                    {
                        task.anchor = Some(anchor.clone());
                    }
                }
                remap_inline_anchors(&mut task.content, mapping);
                for chip in &mut task.chips {
                    remap_inline_anchors(&mut chip.content, mapping);
                }
                remap_block_anchors(&mut task.children, mapping);
            }
            Block::List { items, .. } => {
                for ListItem {
                    content, children, ..
                } in items
                {
                    remap_inline_anchors(content, mapping);
                    remap_block_anchors(children, mapping);
                }
            }
            Block::Paragraph(inlines) | Block::EmptyState(inlines) => {
                remap_inline_anchors(inlines, mapping)
            }
            Block::LabeledRow { label, items } => {
                remap_inline_anchors(label, mapping);
                for item in items {
                    remap_inline_anchors(item, mapping);
                }
            }
            Block::LogRow {
                label,
                date,
                content,
                ..
            } => {
                remap_inline_anchors(label, mapping);
                if let Some(date) = date {
                    remap_inline_anchors(date, mapping);
                }
                remap_inline_anchors(content, mapping);
            }
            Block::MissingImage { label } => {
                remap_inline_anchors(label, mapping)
            }
            Block::Embed { blocks, .. }
            | Block::LiveQuery { blocks }
            | Block::Compact { blocks } => remap_block_anchors(blocks, mapping),
            Block::Callout { title, blocks, .. } => {
                remap_inline_anchors(title, mapping);
                remap_block_anchors(blocks, mapping);
            }
            Block::Code { .. } | Block::Image { .. } => {}
        }
    }
}

fn remap_inline_anchors(
    inlines: &mut [Inline],
    mapping: &BTreeMap<(AnchorIdentity, String), SnapshotAnchor>,
) {
    for inline in inlines {
        match inline {
            Inline::Anchor(anchor) => {
                if let Some(replacement) =
                    mapping.get(&(anchor.identity.clone(), anchor.id.clone()))
                {
                    *anchor = replacement.clone();
                }
            }
            Inline::Link { label, .. }
            | Inline::Emphasis(label)
            | Inline::Strong(label)
            | Inline::Strike(label)
            | Inline::Muted(label) => remap_inline_anchors(label, mapping),
            Inline::Chip(chip) => {
                remap_inline_anchors(&mut chip.content, mapping)
            }
            Inline::EmbeddedBlock(block) => remap_block_anchors(
                std::slice::from_mut(block.as_mut()),
                mapping,
            ),
            Inline::Text(_)
            | Inline::Code(_)
            | Inline::Image { .. }
            | Inline::Break => {}
        }
    }
}

fn rewrite_local_links(
    blocks: &mut [Block],
    source: &SourceIdentity,
    local_anchors: &BTreeMap<AnchorIdentity, String>,
    full_index: &FragmentIndex,
    vault_name: &str,
) {
    for block in blocks {
        match block {
            Block::Paragraph(inlines) | Block::EmptyState(inlines) => {
                rewrite_inlines(
                    inlines,
                    source,
                    local_anchors,
                    full_index,
                    vault_name,
                )
            }
            Block::Heading(heading) => rewrite_inlines(
                &mut heading.content,
                source,
                local_anchors,
                full_index,
                vault_name,
            ),
            Block::List { items, .. } => {
                for item in items {
                    rewrite_inlines(
                        &mut item.content,
                        source,
                        local_anchors,
                        full_index,
                        vault_name,
                    );
                    rewrite_local_links(
                        &mut item.children,
                        source,
                        local_anchors,
                        full_index,
                        vault_name,
                    );
                }
            }
            Block::Task(task) => {
                rewrite_inlines(
                    &mut task.content,
                    source,
                    local_anchors,
                    full_index,
                    vault_name,
                );
                for chip in &mut task.chips {
                    rewrite_inlines(
                        &mut chip.content,
                        source,
                        local_anchors,
                        full_index,
                        vault_name,
                    );
                    if let Some(target) = &mut chip.target {
                        rewrite_target(
                            target,
                            source,
                            local_anchors,
                            full_index,
                            vault_name,
                        );
                    }
                }
                rewrite_local_links(
                    &mut task.children,
                    source,
                    local_anchors,
                    full_index,
                    vault_name,
                );
            }
            Block::LabeledRow { label, items } => {
                rewrite_inlines(
                    label,
                    source,
                    local_anchors,
                    full_index,
                    vault_name,
                );
                for item in items {
                    rewrite_inlines(
                        item,
                        source,
                        local_anchors,
                        full_index,
                        vault_name,
                    );
                }
            }
            Block::LogRow {
                label,
                date,
                content,
                ..
            } => {
                rewrite_inlines(
                    label,
                    source,
                    local_anchors,
                    full_index,
                    vault_name,
                );
                if let Some(date) = date {
                    rewrite_inlines(
                        date,
                        source,
                        local_anchors,
                        full_index,
                        vault_name,
                    );
                }
                rewrite_inlines(
                    content,
                    source,
                    local_anchors,
                    full_index,
                    vault_name,
                );
            }
            Block::MissingImage { label } => rewrite_inlines(
                label,
                source,
                local_anchors,
                full_index,
                vault_name,
            ),
            Block::Embed { source: _, blocks }
            | Block::LiveQuery { blocks }
            | Block::Callout {
                title: _, blocks, ..
            }
            | Block::Compact { blocks } => rewrite_local_links(
                blocks,
                source,
                local_anchors,
                full_index,
                vault_name,
            ),
            Block::Code { .. } | Block::Image { .. } => {}
        }
    }
}

fn rewrite_inlines(
    inlines: &mut [Inline],
    source: &SourceIdentity,
    local_anchors: &BTreeMap<AnchorIdentity, String>,
    full_index: &FragmentIndex,
    vault_name: &str,
) {
    for inline in inlines {
        match inline {
            Inline::Link { label, target } => {
                rewrite_target(
                    target,
                    source,
                    local_anchors,
                    full_index,
                    vault_name,
                );
                rewrite_inlines(
                    label,
                    source,
                    local_anchors,
                    full_index,
                    vault_name,
                );
            }
            Inline::Emphasis(content)
            | Inline::Strong(content)
            | Inline::Strike(content)
            | Inline::Muted(content) => rewrite_inlines(
                content,
                source,
                local_anchors,
                full_index,
                vault_name,
            ),
            Inline::Chip(chip) => {
                rewrite_inlines(
                    &mut chip.content,
                    source,
                    local_anchors,
                    full_index,
                    vault_name,
                );
                if let Some(target) = &mut chip.target {
                    rewrite_target(
                        target,
                        source,
                        local_anchors,
                        full_index,
                        vault_name,
                    );
                }
            }
            Inline::EmbeddedBlock(block) => rewrite_local_links(
                std::slice::from_mut(block.as_mut()),
                source,
                local_anchors,
                full_index,
                vault_name,
            ),
            Inline::Text(_)
            | Inline::Anchor(_)
            | Inline::Code(_)
            | Inline::Image { .. }
            | Inline::Break => {}
        }
    }
}

fn rewrite_target(
    target: &mut LinkTarget,
    source: &SourceIdentity,
    local_anchors: &BTreeMap<AnchorIdentity, String>,
    full_index: &FragmentIndex,
    vault_name: &str,
) {
    let LinkTarget::Internal(identity) = target else {
        return;
    };
    if identity.source.canonical_path != source.canonical_path {
        return;
    }
    if let Some(presentation_id) = local_anchors.get(identity) {
        *target = LinkTarget::ScopedInternal {
            identity: identity.clone(),
            presentation_id: presentation_id.clone(),
        };
        return;
    }
    let fragment = match identity.kind {
        AnchorKind::Heading => LinkFragment::Heading(identity.fragment.clone()),
        AnchorKind::Block => LinkFragment::Block(identity.fragment.clone()),
    };
    // A link to content outside this selected slice opens the source note in
    // Obsidian. The syntax projector only creates Internal targets for known
    // fragments, so this cannot fabricate an anchor for a missing target.
    if !matches!(full_index.lookup(&fragment), FragmentLookup::Missing) {
        let path = identity.source.vault_relative_path.replace('\\', "/");
        *target = LinkTarget::ObsidianUri(obsidian_uri_with_fragment(
            vault_name,
            &path,
            Some(&fragment),
        ));
    }
}

#[cfg(test)]
mod tests {
    use std::{
        collections::{BTreeMap, BTreeSet},
        fs,
        io::Write,
        path::{Path, PathBuf},
        process::{Command, Stdio},
    };

    use super::super::serialize_projected_markdown;
    use super::*;
    use crate::native::{
        highlights_ref::note_snapshot::{
            classify::{ClassifiedNote, NoteKind},
            classify_target, sha256_hex, SourceStore,
        },
        task_status_hooks::{TaskStatusType, TasksSettings},
    };

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

    fn test_snapshot_date() -> chrono::NaiveDate {
        chrono::NaiveDate::from_ymd_opt(2026, 10, 16).expect("test date")
    }

    fn masthead() -> SnapshotMasthead {
        SnapshotMasthead {
            title: "Synthetic snapshot".to_string(),
            kicker: vec![Inline::Text("NOTE".to_string())],
            display_title: vec![Inline::Text("Synthetic snapshot".to_string())],
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

    fn project(
        directory: &tempfile::TempDir,
        host: &str,
    ) -> SnapshotProjection {
        let root = directory.path().canonicalize().expect("canonical vault");
        let mut store = SourceStore::open(&root).expect("source store");
        let display_path = root.join(host).to_string_lossy().into_owned();
        let note = match classify_target(&mut store, &display_path)
            .expect("classify synthetic note")
        {
            super::super::classify::SnapshotTarget::VaultNote(note) => note,
            other => panic!("expected vault note, got {other:?}"),
        };
        project_snapshot(
            &mut store,
            &note,
            &settings(),
            SnapshotProjectionOptions {
                masthead: masthead(),
                snapshot_date: test_snapshot_date(),
                include_closed: false,
                consumed_title_heading: None,
            },
        )
        .expect("project snapshot")
    }

    fn project_bob(
        directory: &tempfile::TempDir,
        host: &str,
        include_closed: bool,
        consume_selected_title: bool,
    ) -> SnapshotProjection {
        let root = directory.path().canonicalize().expect("canonical vault");
        let mut store = SourceStore::open(&root).expect("source store");
        let display_path = root.join(host).to_string_lossy().into_owned();
        let note = match classify_target(&mut store, &display_path)
            .expect("classify synthetic note")
        {
            super::super::classify::SnapshotTarget::VaultNote(note) => note,
            other => panic!("expected vault note, got {other:?}"),
        };
        let tasks_settings = settings();
        let consumed_title_heading = if consume_selected_title {
            let mut attachments = AttachmentIndex::new(root.clone());
            project_obsidian_syntax(
                &mut store,
                &mut attachments,
                &note.identity,
                &tasks_settings,
            )
            .expect("project title candidate")
            .title_candidate
            .map(|candidate| candidate.anchor_id)
        } else {
            None
        };
        let mut masthead = masthead();
        masthead.title = "Selected PDF identity".to_string();
        masthead.display_title =
            vec![Inline::Text("Selected display title".to_string())];
        project_bob_snapshot(
            &mut store,
            &note,
            &tasks_settings,
            SnapshotProjectionOptions {
                masthead,
                snapshot_date: test_snapshot_date(),
                include_closed,
                consumed_title_heading,
            },
        )
        .expect("project Bob snapshot")
    }

    fn markdown(projection: &SnapshotProjection) -> String {
        let mut markdown = serialize_projected_markdown(&projection.view)
            .expect("serialize view");
        let mut search_from = 0;
        while let Some(start) = markdown[search_from..].find("vault=") {
            let value_start = search_from + start + "vault=".len();
            let Some(value_end) = markdown[value_start..].find('&') else {
                break;
            };
            let value_end = value_start + value_end;
            markdown.replace_range(value_start..value_end, "synthetic-vault");
            search_from = value_start + "synthetic-vault".len();
        }
        markdown
    }

    #[test]
    fn bob_project_masthead_and_sections_are_composed_from_synthetic_source() {
        let project_note = "---\n\
type: [[project]]\n\
parent: \"[[Portfolio|Portfolio Alpha]]\"\n\
created: 2026-10-01\n\
status: wip\n\
---\n\
# [[Portfolio]]\n\
This intro keeps **rich prose** and a link to [[Portfolio]].\n\
\n\
## Tasks\n\
`⚪ 0 open` · `🔵 1 next/wip` · `🔴 1 blocked` · `🟢 1 done/canceled`\n\
### Next & In Progress\n\
- [*] Continue **important** work #task ^next\n\
### Blocked\n\
- [?] Waiting on a decision #task ^blocked\n\
### Done & Canceled\n\
- [x] Finished work #task ^closed\n\
Unrelated prose after the closed lane stays visible.\n\
\n\
## Outcome\n\
- [/] Deliver **a result** with [[Portfolio]] [created:: 2026-10-02] #task ^prj\n\
\t- ⛓️ **DEPENDS ON:** [[Project#^blocked]]\n\
\t- 🧱 **BLOCKS:** [[Project#^next]]\n\
\t- 🧩 **Sub-projects:** [[OpenChild]] • ~~[[DoneChild]]~~ ✅ • ~~[[CanceledChild]]~~ ❌\n\
\t- 🛠️ **WORK LOG**\n\
\t\t- *2026-10-15* — latest **progress**\n\
\t- Ordinary outcome child prose remains here.\n";
        let directory = vault(&[
            ("Portfolio.md", "# Portfolio\n"),
            ("OpenChild.md", "# Open child\n"),
            ("DoneChild.md", "# Done child\n"),
            ("CanceledChild.md", "# Canceled child\n"),
            ("Project.md", project_note),
        ]);
        let projection = project_bob(&directory, "Project.md", false, true);
        let output = markdown(&projection);

        assert_eq!(projection.view.masthead.title, "Selected PDF identity");
        assert_eq!(
            projection.view.masthead.display_title,
            vec![Inline::Text("Selected display title".to_string())]
        );
        assert_eq!(
            projection.view.masthead.kicker[0],
            Inline::Text("PROJECT · Portfolio Alpha".to_string())
        );
        assert_eq!(
            plain_inline_text(projection.view.masthead.dek.as_deref().unwrap()),
            "Deliver a result with Portfolio"
        );
        assert!(
            output.contains("This intro keeps **rich prose**"),
            "{output}"
        );
        assert!(output.contains("Started Oct 1\\, 2026"), "{output}");
        assert!(output.contains("[wip]{.chip}"), "{output}");
        assert!(output.contains("WAITS ON\\:"), "{output}");
        assert!(output.contains("BLOCKS\\:"), "{output}");
        assert!(output.contains("Sub\\-projects\\:"), "{output}");
        assert!(output.contains("Outcome history"), "{output}");
        assert!(
            output.contains("Ordinary outcome child prose remains here"),
            "{output}"
        );
        assert!(output.contains("[WORKED]{.label} *Oct 15*"), "{output}");
        assert!(!output.contains("WORK LOG"), "{output}");
        assert!(
            output.contains(
                "Unrelated prose after the closed lane stays visible"
            ),
            "{output}"
        );
        assert!(!output.contains("Done & Canceled"), "{output}");
        assert!(!output.contains("⚪ 0 open"), "{output}");
        assert!(!output.contains("#t-prj"), "{output}");
        assert_eq!(projection.view.summary.omitted_closed_tasks, 1);
        assert_eq!(
            projection
                .view
                .summary
                .task_counts
                .get(&SnapshotTaskStatus::Next),
            Some(&1)
        );
        assert_eq!(
            projection
                .view
                .summary
                .task_counts
                .get(&SnapshotTaskStatus::Blocked),
            Some(&1)
        );
        assert!(!projection
            .view
            .summary
            .task_counts
            .contains_key(&SnapshotTaskStatus::Wip));
        assert_eq!(projection.view.summary.work_logs, 1);
        assert_transclusion_golden(&output, "bob-prose-project");
    }

    #[test]
    fn area_projection_keeps_blocked_ticklers_and_condenses_long_histories() {
        let mut area_note = String::from(
            "---\ntype: [[area]]\nparent: \"[[Portfolio|Portfolio Alpha]]\"\ncreated: 2026-09-30\n---\n# Area title\n\nIntro paragraph stays above sections.\n\n## Tasks\n### Blocked\n",
        );
        for index in 0..23 {
            area_note.push_str(&format!(
                "- [?] Tickler {index:02} [scheduled:: 2026-10-17] [key:: value-{index}] $1,250.00 #task ^tickler-{index:02}\n"
            ));
            area_note.push_str("\t- 🗓️ **SCHEDULE LOG**\n");
            for entry in 0..4 {
                area_note.push_str(&format!(
                    "\t\t- *2026-10-0{} → 2026-10-1{}* — moved entry {entry}\n\t\t\t- reason detail that is not an extra log entry\n",
                    entry + 1,
                    entry + 2
                ));
            }
            area_note.push_str(
                "\t- 🛠️ **WORK LOG**\n\
\t\t- *2026-10-15* — tickler review\n\
\t\t- *2026-10-14* — earlier review\n",
            );
        }
        area_note.push_str(
            "\n## Legacy Notes\n\n- Legacy value [custom:: retained] with literal $4,200.75.\n\n`key:: literal` stays code.\n",
        );
        let directory = vault(&[
            ("Portfolio.md", "# Portfolio\n"),
            ("Area.md", &area_note),
        ]);
        let before = read_tree(directory.path());
        let projection = project_bob(&directory, "Area.md", false, true);
        let output = markdown(&projection);

        assert!(!projection
            .view
            .summary
            .task_counts
            .contains_key(&SnapshotTaskStatus::Blocked));
        assert_eq!(
            projection
                .view
                .summary
                .task_counts
                .get(&SnapshotTaskStatus::Scheduled),
            Some(&23)
        );
        assert_eq!(projection.view.summary.schedule_logs, 92);
        assert_eq!(projection.view.summary.work_logs, 0);
        assert_eq!(output.matches("moved 4×").count(), 23);
        assert!(output.contains("Scheduled"), "{output}");
        assert!(output.contains("This week"), "{output}");
        assert!(output.contains("{.compact}"), "{output}");
        assert!(!output.contains("## Blocked"), "{output}");
        assert!(!output.contains("WORKED"), "{output}");
        assert!(output.contains("Legacy Notes"), "{output}");
        assert!(output.contains("custom\\: retained"), "{output}");
        assert!(output.contains("\\$4\\,200\\.75"), "{output}");
        assert!(output.contains("`key:: literal`"), "{output}");
        assert!(!output.contains("SCHEDULE LOG"), "{output}");
        assert!(!output.contains("reason detail"), "{output}");
        assert_eq!(before, read_tree(directory.path()));
        assert_transclusion_golden(&output, "bob-area-ticklers");
    }

    #[test]
    fn generated_three_hundred_task_projection_has_stable_counts_and_order() {
        let symbols = [' ', '*', '/', '?', 'x', '-'];
        let mut generated = String::from(
            "---\ntype: [[area]]\ncreated: 2026-10-01\n---\n# Generated area\n\n## Tasks\n",
        );
        for index in 0..300 {
            let symbol = symbols[index % symbols.len()];
            generated.push_str(&format!(
                "- [{symbol}] Generated task {index:03} [scheduled:: 2026-10-17] #task ^generated-{index:03}\n"
            ));
            if index % 25 == 0 {
                generated.push_str(
                    "\t- 🗓️ **SCHEDULE LOG**\n\
\t\t- *2026-10-01 → 2026-10-02* — moved\n\
\t- 🛠️ **WORK LOG**\n\
\t\t- *2026-10-15* — generated review\n",
                );
            }
        }
        let directory = vault(&[("Generated.md", &generated)]);
        let projection = project_bob(&directory, "Generated.md", false, true);
        let output = markdown(&projection);

        for status in [
            SnapshotTaskStatus::Open,
            SnapshotTaskStatus::Next,
            SnapshotTaskStatus::Wip,
        ] {
            assert_eq!(
                projection.view.summary.task_counts.get(&status),
                Some(&50)
            );
        }
        assert!(!projection
            .view
            .summary
            .task_counts
            .contains_key(&SnapshotTaskStatus::Blocked));
        assert_eq!(
            projection
                .view
                .summary
                .task_counts
                .get(&SnapshotTaskStatus::Scheduled),
            Some(&50)
        );
        assert_eq!(projection.view.summary.omitted_closed_tasks, 100);
        assert_eq!(projection.view.summary.schedule_logs, 8);
        assert_eq!(projection.view.summary.work_logs, 6);
        let first = output.find("Generated task 000").expect("first task");
        let last = output.find("Generated task 273").expect("last shown task");
        assert!(first < last);
        assert!(!output.contains("Generated task 299"));
        assert!(output.contains("Scheduled"), "{output}");
        assert_transclusion_golden(&output, "bob-generated-300");
    }

    #[test]
    fn prj_is_blocked_and_open_deps_stay_waiting_on() {
        let directory = vault(&[(
            "Project.md",
            "---\ntype: [[project]]\n---\n# Project\n\n## Tasks\n### Blocked\n\
- [?] Resident tickler [scheduled:: 2026-10-20] #task #prj_is_blocked ^resident\n\
- [?] Date only [scheduled:: 2026-10-17] #task ^tickler\n\
- [?] Waiting on a gate [scheduled:: 2026-10-20] #task ^waiter\n\
\t- ⛓️ **DEPENDS ON:** [[Project#^tickler]]\n\
- [?] Due today [scheduled:: 2026-10-16] #task ^today\n\
- [?] No date #task ^mystery\n",
        )]);
        let projection = project_bob(&directory, "Project.md", false, true);
        let output = markdown(&projection);
        assert_eq!(
            projection
                .view
                .summary
                .task_counts
                .get(&SnapshotTaskStatus::Blocked),
            Some(&4)
        );
        assert_eq!(
            projection
                .view
                .summary
                .task_counts
                .get(&SnapshotTaskStatus::Scheduled),
            Some(&1)
        );
        assert!(output.contains("Waiting on"), "{output}");
        assert!(output.contains("Resident tickler"), "{output}");
        assert!(output.contains("Waiting on a gate"), "{output}");
        assert!(output.contains("No date"), "{output}");
        assert!(output.contains("Due today"), "{output}");
        let scheduled_at = output.find("Scheduled").expect("scheduled lane");
        let compact = &output[scheduled_at..];
        assert!(compact.contains("Date only"), "{compact}");
        assert!(!compact.contains("Resident tickler"), "{compact}");
        assert!(!compact.contains("Waiting on a gate"), "{compact}");
        assert!(!compact.contains("No date"), "{compact}");
        assert!(!compact.contains("Due today"), "{compact}");
        assert!(!output.contains("## Blocked"), "{output}");
    }

    #[test]
    fn scheduled_rows_group_this_week_this_month_and_later() {
        let directory = vault(&[(
            "Area.md",
            "---\ntype: [[area]]\n---\n# Area\n\n## Tasks\n### Blocked\n\
- [?] Later year [scheduled:: 2027-01-05] [priority:: high] #task ^later-year\n\
- [?] This week [scheduled:: 2026-10-17] #task ^week\n\
- [?] This month [scheduled:: 2026-10-25] #task ^month\n\
- [?] Later month [scheduled:: 2026-11-02] #task ^later-month\n\
- [?] Week end [scheduled:: 2026-10-18] #task ^week-end\n",
        )]);
        let projection = project_bob(&directory, "Area.md", false, true);
        let output = markdown(&projection);
        assert_eq!(
            projection
                .view
                .summary
                .task_counts
                .get(&SnapshotTaskStatus::Scheduled),
            Some(&5)
        );
        let week = output.find("This week").expect("this week");
        let month = output.find("This month").expect("this month");
        let later = output.find("Later").expect("later");
        assert!(week < month && month < later, "{output}");
        let week_slice = &output[week..month];
        assert!(week_slice.contains("This week"), "{week_slice}");
        assert!(week_slice.contains("Week end"), "{week_slice}");
        assert!(!week_slice.contains("This month"), "{week_slice}");
        let month_slice = &output[month..later];
        assert!(month_slice.contains("This month"), "{month_slice}");
        assert!(!month_slice.contains("Later year"), "{month_slice}");
        let later_slice = &output[later..];
        assert!(later_slice.contains("Later year"), "{later_slice}");
        assert!(later_slice.contains("Later month"), "{later_slice}");
        assert!(later_slice.contains("chip-accent"), "{later_slice}");
        assert!(output.contains("{.compact}"), "{output}");
    }

    #[test]
    fn terminal_project_without_prj_uses_frontmatter_and_keeps_unrelated_prose()
    {
        let directory = vault(&[
            ("Portfolio.md", "# Portfolio\n"),
            (
                "Complete.md",
                "---\ntype: [[project]]\nstatus: done\nparent: [[Portfolio]]\ncreated: 2025-11-07\ndescription: A finished synthetic project.\n---\n# Complete project\n\nIntro remains visible.\n\n## Notes\nNo generated status badge row was authored.\n",
            ),
        ]);
        let projection = project_bob(&directory, "Complete.md", false, true);
        let output = markdown(&projection);

        assert_eq!(
            projection.view.masthead.kicker[0],
            Inline::Text("PROJECT · Portfolio".to_string())
        );
        assert_eq!(
            plain_inline_text(projection.view.masthead.dek.as_deref().unwrap()),
            "A finished synthetic project."
        );
        assert!(output.contains("[done]{.chip}"), "{output}");
        assert!(output.contains("Started Nov 7\\, 2025"), "{output}");
        assert!(output.contains("Intro remains visible"), "{output}");
        assert!(
            output.contains("No generated status badge row was authored"),
            "{output}"
        );
        assert_eq!(projection.view.summary.omitted_closed_tasks, 0);
        assert_transclusion_golden(&output, "bob-terminal-project");
    }

    #[test]
    fn ordinary_control_and_repeated_selected_embeds_keep_their_own_policy() {
        let directory = vault(&[
            (
                "Child.md",
                "# Child\n\n## Focus\n- [ ] Embedded open #task ^open\n\
\t- 🗓️ **SCHEDULE LOG**\n\
\t\t- *2026-10-01 → 2026-10-02* — moved\n\
\t- 🛠️ **WORK LOG**\n\
\t\t- *2026-10-15* — newest\n\
- [x] Embedded closed #task ^closed\n\
\t- 🗓️ **SCHEDULE LOG**\n\
\t\t- *2026-10-02 → 2026-10-03* — moved while closed\n\
\t- 🛠️ **WORK LOG**\n\
\t\t- *2026-10-14* — closed history\n",
            ),
            (
                "Area.md",
                "---\ntype: [[area]]\n---\n# Area\n\nA Bob host intro.\n\n## Materials\n![[Child#Focus]]\n\n![[Child#Focus]]\n",
            ),
            (
                "Ordinary.md",
                "# Plain note\n\nOrdinary control prose.\n\n## Section\nThis remains the ordinary Obsidian view.\n",
            ),
        ]);
        let before = read_tree(directory.path());
        let bob_reading = project_bob(&directory, "Area.md", false, true);
        let bob_reading_markdown = markdown(&bob_reading);
        let bob_closed = project_bob(&directory, "Area.md", true, true);
        let bob_closed_markdown = markdown(&bob_closed);
        let ordinary = project_bob(&directory, "Ordinary.md", false, true);
        let ordinary_markdown = markdown(&ordinary);

        assert_eq!(
            bob_reading
                .view
                .summary
                .task_counts
                .get(&SnapshotTaskStatus::Open),
            Some(&2)
        );
        assert_eq!(bob_reading.view.summary.omitted_closed_tasks, 2);
        assert_eq!(bob_reading.view.summary.schedule_logs, 2);
        assert_eq!(bob_reading.view.summary.work_logs, 2);
        assert_eq!(bob_closed.view.summary.omitted_closed_tasks, 0);
        assert_eq!(
            bob_closed
                .view
                .summary
                .task_counts
                .get(&SnapshotTaskStatus::Done),
            Some(&2)
        );
        assert_eq!(bob_closed.view.summary.schedule_logs, 4);
        assert_eq!(bob_closed.view.summary.work_logs, 4);
        assert!(
            bob_closed_markdown.contains("Closed tasks"),
            "{bob_closed_markdown}"
        );
        assert!(
            bob_closed_markdown.contains("MOVED"),
            "{bob_closed_markdown}"
        );
        assert!(
            bob_closed_markdown.contains("WORKED"),
            "{bob_closed_markdown}"
        );
        let anchor_ids = bob_closed
            .view
            .anchors
            .iter()
            .map(|anchor| anchor.id.as_str())
            .collect::<BTreeSet<_>>();
        assert_eq!(anchor_ids.len(), bob_closed.view.anchors.len());
        assert!(
            !ordinary_markdown.contains("# Plain note"),
            "the selected source H1 should be consumed when it supplies the title:\n{ordinary_markdown}"
        );
        assert!(
            ordinary_markdown.contains("Ordinary control prose"),
            "{ordinary_markdown}"
        );
        assert!(!ordinary_markdown.contains("AREA"), "{ordinary_markdown}");
        assert!(
            !ordinary_markdown.contains("Omitted:"),
            "{ordinary_markdown}"
        );
        assert_eq!(ordinary.view.summary.omitted_closed_tasks, 0);
        assert_eq!(before, read_tree(directory.path()));
        assert_transclusion_golden(
            &bob_reading_markdown,
            "bob-repeated-embed-reading",
        );
        assert_transclusion_golden(
            &bob_closed_markdown,
            "bob-repeated-embed-closed",
        );
        assert_transclusion_golden(&ordinary_markdown, "ordinary-control");
    }

    fn embeds(blocks: &[Block]) -> Vec<&Block> {
        let mut found = Vec::new();
        fn walk_inlines<'a>(inlines: &'a [Inline], found: &mut Vec<&'a Block>) {
            for inline in inlines {
                match inline {
                    Inline::EmbeddedBlock(block) => {
                        if matches!(block.as_ref(), Block::Embed { .. }) {
                            found.push(block.as_ref());
                        }
                        walk(std::slice::from_ref(block.as_ref()), found);
                    }
                    Inline::Link { label, .. }
                    | Inline::Emphasis(label)
                    | Inline::Strong(label)
                    | Inline::Strike(label)
                    | Inline::Muted(label) => walk_inlines(label, found),
                    Inline::Chip(chip) => walk_inlines(&chip.content, found),
                    _ => {}
                }
            }
        }
        fn walk<'a>(blocks: &'a [Block], found: &mut Vec<&'a Block>) {
            for block in blocks {
                match block {
                    Block::Embed { .. } => found.push(block),
                    Block::Paragraph(inlines) | Block::EmptyState(inlines) => {
                        walk_inlines(inlines, found)
                    }
                    Block::Heading(heading) => {
                        walk_inlines(&heading.content, found)
                    }
                    Block::Task(task) => {
                        walk_inlines(&task.content, found);
                        walk(&task.children, found)
                    }
                    Block::List { items, .. } => {
                        for item in items {
                            walk_inlines(&item.content, found);
                            walk(&item.children, found);
                        }
                    }
                    Block::Callout { blocks, .. }
                    | Block::Compact { blocks }
                    | Block::LiveQuery { blocks } => walk(blocks, found),
                    Block::LabeledRow { label, items } => {
                        walk_inlines(label, found);
                        for item in items {
                            walk_inlines(item, found);
                        }
                    }
                    Block::LogRow {
                        label,
                        date,
                        content,
                        ..
                    } => {
                        walk_inlines(label, found);
                        if let Some(date) = date {
                            walk_inlines(date, found);
                        }
                        walk_inlines(content, found);
                    }
                    _ => {}
                }
            }
        }
        walk(blocks, &mut found);
        found
    }

    fn view_text(blocks: &[Block], output: &mut Vec<String>) {
        for block in blocks {
            match block {
                Block::Paragraph(inlines) | Block::EmptyState(inlines) => {
                    output.push(flatten(inlines));
                    view_inlines(inlines, output);
                }
                Block::Heading(heading) => {
                    output.push(flatten(&heading.content));
                    view_inlines(&heading.content, output);
                }
                Block::Task(task) => {
                    output.push(flatten(&task.content));
                    view_inlines(&task.content, output);
                    view_text(&task.children, output);
                }
                Block::List { items, .. } => {
                    for item in items {
                        output.push(flatten(&item.content));
                        view_inlines(&item.content, output);
                        view_text(&item.children, output);
                    }
                }
                Block::Embed { source, blocks } => {
                    output.push(source.clone());
                    view_text(blocks, output);
                }
                Block::Callout { title, blocks, .. } => {
                    output.push(flatten(title));
                    view_text(blocks, output);
                }
                Block::Compact { blocks } | Block::LiveQuery { blocks } => {
                    view_text(blocks, output);
                }
                Block::LabeledRow { label, items } => {
                    output.push(flatten(label));
                    output.extend(items.iter().map(|item| flatten(item)));
                }
                Block::LogRow { content, .. } => output.push(flatten(content)),
                Block::MissingImage { label } => output.push(flatten(label)),
                Block::Code { text, .. } => output.push(text.clone()),
                Block::Image { alt, .. } => output.push(alt.clone()),
            }
        }
    }

    fn view_inlines(inlines: &[Inline], output: &mut Vec<String>) {
        for inline in inlines {
            match inline {
                Inline::EmbeddedBlock(block) => {
                    view_text(std::slice::from_ref(block.as_ref()), output)
                }
                Inline::Link { label, .. }
                | Inline::Emphasis(label)
                | Inline::Strong(label)
                | Inline::Strike(label)
                | Inline::Muted(label) => view_inlines(label, output),
                Inline::Chip(chip) => view_inlines(&chip.content, output),
                _ => {}
            }
        }
    }

    fn flatten(inlines: &[Inline]) -> String {
        let mut output = String::new();
        for inline in inlines {
            match inline {
                Inline::Text(text) | Inline::Code(text) => {
                    output.push_str(text)
                }
                Inline::Anchor(_) => {}
                Inline::Link { label, .. }
                | Inline::Emphasis(label)
                | Inline::Strong(label)
                | Inline::Strike(label)
                | Inline::Muted(label) => output.push_str(&flatten(label)),
                Inline::Chip(chip) => output.push_str(&flatten(&chip.content)),
                Inline::Image { alt, .. } => output.push_str(alt),
                Inline::EmbeddedBlock(block) => match block.as_ref() {
                    Block::Embed { source, .. } => output.push_str(source),
                    Block::Paragraph(content) | Block::EmptyState(content) => {
                        output.push_str(&flatten(content))
                    }
                    _ => {}
                },
                Inline::Break => output.push(' '),
            }
        }
        output
    }

    fn all_text(view: &SnapshotView) -> String {
        let mut output = Vec::new();
        for section in &view.sections {
            output.push(flatten(&section.heading.content));
            view_text(&section.blocks, &mut output);
        }
        output.join("\n")
    }

    fn warning_kinds(view: &SnapshotView) -> Vec<SnapshotWarningKind> {
        view.warnings.iter().map(|warning| warning.kind).collect()
    }

    fn ast_node_count(value: &serde_json::Value, tag: &str) -> usize {
        match value {
            serde_json::Value::Array(items) => {
                items.iter().map(|item| ast_node_count(item, tag)).sum()
            }
            serde_json::Value::Object(object) => {
                usize::from(
                    object.get("t").and_then(|value| value.as_str())
                        == Some(tag),
                ) + object
                    .values()
                    .map(|item| ast_node_count(item, tag))
                    .sum::<usize>()
            }
            _ => 0,
        }
    }

    fn collect_pandoc_anchor_ids(
        value: &serde_json::Value,
        ids: &mut Vec<String>,
    ) {
        fn attr_id(value: &serde_json::Value) -> Option<&str> {
            value
                .as_array()?
                .first()?
                .as_str()
                .filter(|id| !id.is_empty())
        }
        match value {
            serde_json::Value::Array(items) => {
                for item in items {
                    collect_pandoc_anchor_ids(item, ids);
                }
            }
            serde_json::Value::Object(object) => {
                let tag = object.get("t").and_then(|value| value.as_str());
                let content = object.get("c");
                let attr =
                    match (tag, content.and_then(|value| value.as_array())) {
                        (Some("Header"), Some(items)) => items.get(1),
                        (
                            Some(
                                "CodeBlock" | "Div" | "Span" | "Link" | "Image",
                            ),
                            Some(items),
                        ) => items.first(),
                        _ => None,
                    };
                if let Some(id) = attr.and_then(attr_id) {
                    ids.push(id.to_string());
                }
                for item in object.values() {
                    collect_pandoc_anchor_ids(item, ids);
                }
            }
            _ => {}
        }
    }

    #[test]
    fn full_heading_and_block_embeds_project_selected_content() {
        let directory = vault(&[
            (
                "Host.md",
                "# Host\n\n![[Full]]\n\n![[Target#Details]]\n\n![[Target#^task-id]]\n",
            ),
            (
                "Full.md",
                "---\ntitle: Hidden metadata\n---\n\n# Full Included\n\nFull body.\n",
            ),
            (
                "Target.md",
                "# Target\n\n## Details\n\nHeading body.\n\n- [ ] Block task #task ^task-id\n  Nested child.\n\n## Outside\n\nOutside selected range.\n",
            ),
        ]);
        let projection = project(&directory, "Host.md");
        let section = &projection.view.sections[0];
        let included = embeds(&section.blocks);
        assert_eq!(included.len(), 3);
        assert_eq!(projection.view.summary.transclusions, 3);
        let text = all_text(&projection.view);
        assert!(text.contains("Full Included"), "{text}");
        assert!(text.contains("Full body."), "{text}");
        assert!(text.contains("Heading body."), "{text}");
        assert!(text.contains("Block task"), "{text}");
        assert!(text.contains("Nested child."), "{text}");
        assert!(!text.contains("Hidden metadata"), "{text}");
        assert!(!text.contains("Outside selected range."), "{text}");
        assert!(projection
            .view
            .anchors
            .iter()
            .any(|anchor| anchor.id.starts_with("embed-")));
        // The heading and block embeds both render the same source task, so
        // the summary counts its two visible occurrences.
        assert_eq!(
            projection.view.summary.task_counts[&SnapshotTaskStatus::Open],
            2
        );
    }

    #[test]
    fn direct_self_and_multinote_cycles_become_links_and_nested_embeds_stop() {
        let directory = vault(&[
            ("Host.md", "# Host\n\n![[Host]]\n\n![[A]]\n"),
            ("A.md", "# A\n\n![[Host]]\n\n![[B]]\n"),
            ("B.md", "# B\n\nB body.\n"),
        ]);
        let projection = project(&directory, "Host.md");
        let kinds = warning_kinds(&projection.view);
        assert_eq!(
            kinds
                .iter()
                .filter(|kind| **kind == SnapshotWarningKind::CyclicTransclusion)
                .count(),
            2,
            "{kinds:?}"
        );
        assert_eq!(
            kinds
                .iter()
                .filter(|kind| **kind == SnapshotWarningKind::NestedTransclusion)
                .count(),
            1,
            "{kinds:?}"
        );
        assert_eq!(projection.view.summary.guarded_omissions, 3);
        assert_eq!(projection.view.summary.transclusions, 1);
        assert!(all_text(&projection.view).contains("B"));
        let serialized =
            super::super::serialize_projected_markdown(&projection.view)
                .expect("serialize cycle fallback");
        assert!(serialized.contains("obsidian://open"), "{serialized}");
    }

    #[test]
    fn missing_and_ambiguous_fragments_fall_back_without_fake_anchors() {
        let missing = vault(&[
            ("Host.md", "# Host\n\n![[Target#Absent]]\n"),
            ("Target.md", "# Target\n\nBody.\n"),
        ]);
        let missing_projection = project(&missing, "Host.md");
        assert!(warning_kinds(&missing_projection.view)
            .contains(&SnapshotWarningKind::MissingFragment));
        assert!(all_text(&missing_projection.view).contains("Target › Absent"));
        assert!(!missing_projection.view.anchors.iter().any(|anchor| {
            anchor.identity.source.route == "Target"
                && anchor.identity.fragment == "absent"
        }));

        let ambiguous = vault(&[
            ("Host.md", "# Host\n\n![[Target#Details]]\n"),
            (
                "Target.md",
                "# Target\n\n## Details\n\nOne.\n\n## Details\n\nTwo.\n",
            ),
        ]);
        let ambiguous_projection = project(&ambiguous, "Host.md");
        assert!(warning_kinds(&ambiguous_projection.view)
            .contains(&SnapshotWarningKind::AmbiguousFragment));
        assert!(!all_text(&ambiguous_projection.view).contains("One."));
        assert!(!all_text(&ambiguous_projection.view).contains("Two."));
    }

    #[test]
    fn source_block_anchors_are_emitted_for_headings_prose_and_lists() {
        let directory = vault(&[(
            "Host.md",
            "# Head ^heading\n\nParagraph ^paragraph\n\n- item ^list\n\n[[#^heading]] [[#^paragraph]] [[#^list]]\n",
        )]);
        let result = project(&directory, "Host.md");
        let ids = result
            .view
            .anchors
            .iter()
            .map(|anchor| anchor.id.as_str())
            .collect::<Vec<_>>();
        for expected in ["t-heading", "t-paragraph", "t-list"] {
            assert!(ids.contains(&expected), "lost block anchor: {ids:?}");
        }
        let serialized =
            super::super::serialize_projected_markdown(&result.view)
                .expect("serialize anchored view");
        for expected in ["t-heading", "t-paragraph", "t-list"] {
            assert!(serialized.contains(&format!("[]{{#{expected}}}")));
            assert!(serialized.contains(&format!("](<#{expected}>)")));
        }
    }

    #[test]
    fn inline_embeds_and_images_expand_inside_prose_tasks_and_lists() {
        let directory = vault(&[
            (
                "Host.md",
                "# Host\n\nBefore ![[Other]] after.\n\n- [ ] Task before ![[Other]] after.\n\n- Item before ![[Other]] after.\n\nImage before ![[diagram.png|400]] after.\n",
            ),
            ("Other.md", "Other before ![[Host]] after.\n"),
        ]);
        fs::write(directory.path().join("diagram.png"), b"synthetic image")
            .expect("write synthetic image asset");
        let projection = project(&directory, "Host.md");
        assert_eq!(projection.view.summary.transclusions, 3);
        assert_eq!(
            warning_kinds(&projection.view)
                .iter()
                .filter(|kind| **kind == SnapshotWarningKind::CyclicTransclusion)
                .count(),
            3
        );
        let serialized =
            super::super::serialize_projected_markdown(&projection.view)
                .expect("serialize inline embeds");
        let before = serialized.find("Before").expect("leading prose");
        let embed = serialized
            .find(".embed source=\"Other\"")
            .expect("embed div");
        let close = embed
            + serialized[embed..]
                .find("\n:::")
                .expect("embed closing fence");
        let after = close
            + serialized[close..].find("\n\n after").unwrap_or_else(|| {
                panic!("trailing prose not after embed: {serialized}")
            });
        assert!(
            before < embed && embed < close && close < after,
            "{serialized}"
        );
        assert!(serialized.contains("{width=62%}"), "{serialized}");

        let Some(pandoc) = super::super::super::create::pandoc_command() else {
            eprintln!("skipping inline embed AST test: pandoc is required");
            return;
        };
        let mut child = Command::new(pandoc)
            .args(["--from=commonmark_x-tex_math_dollars-emoji", "--to=json"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("start pandoc AST parser");
        child
            .stdin
            .take()
            .expect("pandoc stdin")
            .write_all(serialized.as_bytes())
            .expect("write projected Markdown");
        let output = child.wait_with_output().expect("wait for pandoc");
        assert!(
            output.status.success(),
            "pandoc could not parse inline embeds: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let ast: serde_json::Value =
            serde_json::from_slice(&output.stdout).expect("pandoc AST");
        assert!(ast_node_count(&ast, "Image") >= 1, "{ast}");
        assert!(ast_node_count(&ast, "Div") >= 3, "{ast}");
        assert!(ast_node_count(&ast, "Link") >= 3, "{ast}");
    }

    #[test]
    fn prelude_heading_uses_a_collision_safe_emitted_id() {
        let directory =
            vault(&[("Host.md", "Prelude\n\n# Snapshot content\nBody\n")]);
        let projection = project(&directory, "Host.md");
        let ids = projection
            .view
            .anchors
            .iter()
            .map(|anchor| anchor.id.clone())
            .collect::<Vec<_>>();
        assert!(ids.contains(&"snapshot-content".to_string()), "{ids:?}");
        assert!(ids.contains(&"snapshot-content-2".to_string()), "{ids:?}");
        assert_eq!(
            ids.len(),
            ids.iter().collect::<BTreeSet<_>>().len(),
            "{ids:?}"
        );

        let serialized =
            super::super::serialize_projected_markdown(&projection.view)
                .expect("serialize prelude view");
        let Some(pandoc) = super::super::super::create::pandoc_command() else {
            eprintln!("skipping prelude anchor AST test: pandoc is required");
            return;
        };
        let output = Command::new(pandoc)
            .args(["--from=commonmark_x-tex_math_dollars-emoji", "--to=json"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .and_then(|mut child| {
                child
                    .stdin
                    .take()
                    .expect("pandoc stdin")
                    .write_all(serialized.as_bytes())?;
                child.wait_with_output()
            })
            .expect("run pandoc AST parser");
        assert!(
            output.status.success(),
            "pandoc could not parse anchors: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let ast: serde_json::Value =
            serde_json::from_slice(&output.stdout).expect("pandoc AST");
        let mut emitted_ids = Vec::new();
        collect_pandoc_anchor_ids(&ast, &mut emitted_ids);
        assert!(
            emitted_ids.contains(&"snapshot-content".to_string()),
            "{emitted_ids:?}"
        );
        assert!(
            emitted_ids.contains(&"snapshot-content-2".to_string()),
            "{emitted_ids:?}"
        );
        assert_eq!(
            emitted_ids.len(),
            emitted_ids.iter().collect::<BTreeSet<_>>().len(),
            "Pandoc emitted duplicate anchors: {emitted_ids:?}"
        );
    }

    #[test]
    fn explicit_ambiguous_self_headings_and_blocks_never_invent_targets() {
        let directory = vault(&[(
            "Host.md",
            "# Same\nOne\n\n# Same\nTwo [[Host#Same]] [[Host#Same|alias]] [[#Same]] [[#Same|host alias]]\n\nFirst ^duplicate\n\nSecond ^duplicate\n\n[[Host#^duplicate]] [[#^duplicate]]\n",
        )]);
        let projection = project(&directory, "Host.md");
        let serialized =
            super::super::serialize_projected_markdown(&projection.view)
                .expect("serialize ambiguous links");
        assert!(!serialized.contains("](<#same>)"), "{serialized}");
        assert!(!serialized.contains("](<#t-duplicate>)"), "{serialized}");
        assert!(serialized.contains("host alias"), "{serialized}");
        assert!(serialized.contains("alias"), "{serialized}");
        assert!(warning_kinds(&projection.view)
            .contains(&SnapshotWarningKind::AmbiguousFragment));
    }

    #[test]
    fn repeated_selected_embeds_namespace_prose_and_list_block_anchors() {
        let directory = vault(&[
            (
                "Host.md",
                "# Host\n\n![[Target#Details]]\n\n![[Target#Details]]\n",
            ),
            (
                "Target.md",
                "## Details ^section\n\nParagraph ^paragraph\n\n- item ^list\n\n[[#^paragraph]] [[#^list]]\n",
            ),
        ]);
        let projection = project(&directory, "Host.md");
        let ids = projection
            .view
            .anchors
            .iter()
            .map(|anchor| anchor.id.as_str())
            .collect::<Vec<_>>();
        for expected in [
            "embed-1-t-paragraph",
            "embed-1-t-list",
            "embed-2-t-paragraph",
            "embed-2-t-list",
        ] {
            assert!(ids.contains(&expected), "missing {expected}: {ids:?}");
        }
        let serialized =
            super::super::serialize_projected_markdown(&projection.view)
                .expect("serialize repeated selected blocks");
        for expected in [
            "embed-1-t-paragraph",
            "embed-1-t-list",
            "embed-2-t-paragraph",
            "embed-2-t-list",
        ] {
            assert!(serialized.contains(&format!("[]{{#{expected}}}")));
        }
        assert!(
            serialized.contains("](<#embed-1-t-paragraph>)"),
            "{serialized}"
        );
        assert!(serialized.contains("](<#embed-2-t-list>)"), "{serialized}");
    }

    #[test]
    fn complete_transclusion_projection_matches_a_generated_golden() {
        let directory = vault(&[
            (
                "Host.md",
                "# Host\n\nBefore the embed.\n\n![[Included#Part]]\n",
            ),
            (
                "Included.md",
                "# Included\n\n## Part\n\nIncluded [[#Part]] content.\n",
            ),
        ]);
        let projection = project(&directory, "Host.md");
        let projected =
            super::super::serialize_projected_markdown(&projection.view)
                .expect("serialize expanded view");
        super::super::tests::assert_no_obsidian_residue(&projected);
        super::super::tests::assert_serialized_internal_targets_exist(
            &projection.view,
            &projected,
        );
        assert_transclusion_golden(&projected, "transclusions");
    }

    #[test]
    fn exact_size_and_line_limits_are_allowed_and_each_limit_is_enforced() {
        let exact_bytes = "x".repeat(MAX_TRANSCLUSION_BYTES);
        let allowed_bytes = vault(&[
            ("Host.md", "# Host\n\n![[Target]]\n"),
            ("Target.md", exact_bytes.as_str()),
        ]);
        let allowed_bytes_projection = project(&allowed_bytes, "Host.md");
        assert_eq!(allowed_bytes_projection.view.summary.transclusions, 1);
        assert!(!warning_kinds(&allowed_bytes_projection.view)
            .contains(&SnapshotWarningKind::OversizedTransclusion));

        let too_many_bytes = format!("{}x", "x".repeat(MAX_TRANSCLUSION_BYTES));
        let too_many_bytes_vault = vault(&[
            ("Host.md", "# Host\n\n![[Target]]\n"),
            ("Target.md", too_many_bytes.as_str()),
        ]);
        let too_many_bytes_projection =
            project(&too_many_bytes_vault, "Host.md");
        assert_eq!(too_many_bytes_projection.view.summary.transclusions, 0);
        assert!(warning_kinds(&too_many_bytes_projection.view)
            .contains(&SnapshotWarningKind::OversizedTransclusion));

        let exact_lines = "x\n".repeat(MAX_TRANSCLUSION_LINES);
        let allowed_lines = vault(&[
            ("Host.md", "# Host\n\n![[Target]]\n"),
            ("Target.md", exact_lines.as_str()),
        ]);
        let allowed_lines_projection = project(&allowed_lines, "Host.md");
        assert_eq!(allowed_lines_projection.view.summary.transclusions, 1);

        let too_many_lines =
            format!("{}x\n", "x\n".repeat(MAX_TRANSCLUSION_LINES));
        let too_many_lines_vault = vault(&[
            ("Host.md", "# Host\n\n![[Target]]\n"),
            ("Target.md", too_many_lines.as_str()),
        ]);
        let too_many_lines_projection =
            project(&too_many_lines_vault, "Host.md");
        assert_eq!(too_many_lines_projection.view.summary.transclusions, 0);
        assert!(warning_kinds(&too_many_lines_projection.view)
            .contains(&SnapshotWarningKind::OversizedTransclusion));
    }

    #[test]
    fn repeated_embeds_namespace_colliding_headings_and_task_ids() {
        let directory = vault(&[
            (
                "Host.md",
                "# Common\n\n- [ ] Host task #task ^shared\n\n![[Target#Common]]\n\n![[Target#Common]]\n",
            ),
            (
                "Target.md",
                "# Common\n\nSee [[#Common]].\n\n- [ ] Included task #task ^shared\n",
            ),
        ]);
        let projection = project(&directory, "Host.md");
        let included = embeds(&projection.view.sections[0].blocks);
        assert_eq!(included.len(), 2);
        let ids = projection
            .view
            .anchors
            .iter()
            .map(|anchor| anchor.id.clone())
            .collect::<BTreeSet<_>>();
        assert_eq!(ids.len(), projection.view.anchors.len());
        assert!(ids.contains("t-shared"));
        assert!(ids.contains("embed-1-common"));
        assert!(ids.contains("embed-2-common"));
        for embed in included {
            let Block::Embed { blocks, .. } = embed else {
                unreachable!()
            };
            let heading_id = blocks.iter().find_map(|block| match block {
                Block::Heading(heading) => Some(heading.anchor.id.clone()),
                _ => None,
            });
            let heading_id = heading_id.expect("included heading");
            let mut scoped_ids = Vec::new();
            fn links(blocks: &[Block], ids: &mut Vec<String>) {
                for block in blocks {
                    if let Block::Paragraph(inlines) = block {
                        for inline in inlines {
                            if let Inline::Link {
                                target:
                                    LinkTarget::ScopedInternal {
                                        presentation_id,
                                        ..
                                    },
                                ..
                            } = inline
                            {
                                ids.push(presentation_id.clone());
                            }
                        }
                    }
                }
            }
            links(blocks, &mut scoped_ids);
            assert_eq!(scoped_ids, vec![heading_id]);
        }
        let projected =
            super::super::serialize_projected_markdown(&projection.view)
                .expect("serialize repeated transclusions");
        super::super::tests::assert_no_obsidian_residue(&projected);
        super::super::tests::assert_serialized_internal_targets_exist(
            &projection.view,
            &projected,
        );
    }

    #[test]
    fn source_bytes_and_block_excerpts_stay_consistent_after_file_mutation() {
        let directory = vault(&[
            (
                "Host.md",
                "# Host\n\nSee [[Target#^task-id]].\n\n![[Target#^task-id]]\n",
            ),
            (
                "Target.md",
                "# Target\n\n- [ ] Original task [custom:: kept] #task ^task-id\n",
            ),
        ]);
        let root = directory.path().canonicalize().expect("canonical root");
        let mut store = SourceStore::open(&root).expect("source store");
        let host_path = root.join("Host.md").canonicalize().expect("host");
        let target_path =
            root.join("Target.md").canonicalize().expect("target");
        let original_host = fs::read(&host_path).expect("read host source");
        store.read_canonical(&host_path).expect("cache root bytes");
        store
            .read_canonical(&target_path)
            .expect("cache target bytes");
        fs::write(&host_path, "# Changed Host\n\nChanged.\n")
            .expect("mutate host");
        fs::write(&target_path, "# Changed Target\n\nChanged task.\n")
            .expect("mutate target");
        let identity =
            super::super::classify::source_identity(&root, &host_path)
                .expect("host identity");
        let note = ClassifiedNote {
            display_path: "Host.md".to_string(),
            canonical_path: host_path,
            identity,
            kind: NoteKind::Ordinary,
        };
        let projection = project_snapshot(
            &mut store,
            &note,
            &settings(),
            SnapshotProjectionOptions {
                masthead: masthead(),
                snapshot_date: test_snapshot_date(),
                include_closed: false,
                consumed_title_heading: None,
            },
        )
        .expect("project cached bytes");
        assert_eq!(projection.parsed_source.raw_bytes.as_ref(), original_host);
        assert_eq!(projection.parsed_source.sha256, sha256_hex(&original_host));
        let text = all_text(&projection.view);
        assert!(text.contains("Original task custom: kept"), "{text}");
        assert!(!text.contains("Changed task"), "{text}");
        assert!(text.contains("Original task"), "{text}");
    }

    #[test]
    fn source_cache_reads_root_and_repeated_embed_once_and_projection_is_read_only(
    ) {
        use crate::native::highlights_ref::note_snapshot::classify::CountingLoader;

        let directory = vault(&[
            ("Host.md", "# Host\n\n![[Target]]\n\n![[Target]]\n"),
            ("Target.md", "# Target\n\nTarget body.\n"),
        ]);
        let root = directory.path().canonicalize().expect("canonical root");
        let host_path = root.join("Host.md").canonicalize().expect("host");
        let target_path =
            root.join("Target.md").canonicalize().expect("target");
        let host_bytes = fs::read(&host_path).expect("host bytes");
        let target_bytes = fs::read(&target_path).expect("target bytes");
        let loader = CountingLoader::with_files(vec![
            (host_path.clone(), host_bytes),
            (target_path.clone(), target_bytes),
        ]);
        let mut store = SourceStore::with_loader(&root, loader)
            .expect("counting source store");
        let identity =
            super::super::classify::source_identity(&root, &host_path)
                .expect("host identity");
        let note = ClassifiedNote {
            display_path: "Host.md".to_string(),
            canonical_path: host_path,
            identity,
            kind: NoteKind::Ordinary,
        };
        let before = read_tree(&root);
        let projection = project_snapshot(
            &mut store,
            &note,
            &settings(),
            SnapshotProjectionOptions {
                masthead: masthead(),
                snapshot_date: test_snapshot_date(),
                include_closed: false,
                consumed_title_heading: None,
            },
        )
        .expect("project synthetic vault");
        assert_eq!(store.loader().reads(), 2);
        assert_eq!(projection.view.summary.transclusions, 2);
        assert_eq!(before, read_tree(&root));
    }

    fn read_tree(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
        let mut result = BTreeMap::new();
        let mut pending = vec![root.to_path_buf()];
        while let Some(directory) = pending.pop() {
            for entry in fs::read_dir(directory).expect("read tree") {
                let path = entry.expect("entry").path();
                if path.is_dir() {
                    pending.push(path);
                } else {
                    result.insert(
                        path.strip_prefix(root)
                            .expect("relative path")
                            .to_path_buf(),
                        fs::read(path).expect("read snapshot file"),
                    );
                }
            }
        }
        result
    }

    fn assert_transclusion_golden(actual: &str, name: &str) {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/note_snapshot")
            .join(format!("{name}.projected.md"));
        if std::env::var_os("BOB_UPDATE_GOLDENS").as_deref()
            == Some(std::ffi::OsStr::new("1"))
        {
            fs::write(&path, actual).expect("update transclusion golden");
            return;
        }
        let expected = fs::read_to_string(&path).unwrap_or_else(|error| {
            panic!("read transclusion golden {}: {error}", path.display());
        });
        assert_eq!(actual, expected, "transclusion golden: {}", path.display());
    }

    #[test]
    fn land_audit_inline_embed_policy() {
        let directory = vault(&[
            ("Area.md", "---\ntype: [[area]]\n---\n# Area\n## Tasks\nContext ![[Child]] after.\n\nRepeat ![[Child]] again.\n\nSelected ![[Child#Focus]] done.\n"),
            (
                "Child.md",
                "# Child\n\n## Focus\n- [ ] Focus open [priority:: high] ^focus\n\n- [ ] Child open [priority:: high] ^open\n- [x] Child closed ^closed\n",
            ),
        ]);
        // Reading mode: closed occurrences omitted, open chips applied inline.
        let projection = project_bob(&directory, "Area.md", false, true);
        let output = markdown(&projection);
        assert!(!output.contains("Child closed"), "{output}");
        assert!(output.contains("Child open"), "{output}");
        assert!(output.contains("Focus open"), "{output}");
        assert!(output.contains("Context"), "{output}");
        assert!(output.contains("after"), "{output}");
        // Two full embeds plus the Focus slice each contribute one closed
        // omission (the Focus extent runs to the end of Child).
        assert_eq!(projection.view.summary.omitted_closed_tasks, 3);
        // Two full embeds x (Focus open + Child open) + Focus slice x 2 opens.
        assert_eq!(
            projection
                .view
                .summary
                .task_counts
                .get(&SnapshotTaskStatus::Open),
            Some(&6)
        );
        assert!(
            output.contains("[high]{.chip"),
            "priority chips must survive inline embeds: {output}"
        );
        assert!(
            !output.contains("No open tasks"),
            "inline tasks count as visible: {output}"
        );
        // Repeated occurrences keep distinct anchors.
        let mut ids = Vec::new();
        for section in &projection.view.sections {
            collect_test_anchors(&section.blocks, &mut ids);
        }
        collect_test_anchors(&projection.view.preamble, &mut ids);
        let mut seen = std::collections::BTreeSet::new();
        for id in &ids {
            assert!(
                seen.insert(id.clone()),
                "duplicate anchor {id} in {ids:?}"
            );
        }
        assert!(ids.iter().any(|id| id.starts_with("embed-1-")), "{ids:?}");
        assert!(ids.iter().any(|id| id.starts_with("embed-2-")), "{ids:?}");

        // Include-closed mode restores inline closed rows to the appendix
        // with occurrence-specific anchors while leaving prose intact.
        let restored = project_bob(&directory, "Area.md", true, true);
        let restored_output = markdown(&restored);
        assert!(
            restored_output.contains("Child closed"),
            "{restored_output}"
        );
        assert!(restored_output.contains("Context"), "{restored_output}");
        assert!(restored_output.contains("after"), "{restored_output}");
        let closed = restored.view.closed.as_ref().expect("appendix");
        let mut appendix_ids = Vec::new();
        collect_test_anchors(&closed.blocks, &mut appendix_ids);
        assert_eq!(appendix_ids.len(), 3, "{appendix_ids:?}");
        let mut uniq = std::collections::BTreeSet::new();
        for id in &appendix_ids {
            assert!(uniq.insert(id.clone()), "duplicate appendix {id}");
        }

        // Ordinary notes keep plain projection inside inline embeds.
        let ordinary_dir = vault(&[
            ("Ordinary.md", "Context ![[Child]] after.\n"),
            (
                "Child.md",
                "- [ ] Child open [priority:: high] ^open\n- [x] Child closed ^closed\n",
            ),
        ]);
        let ordinary = project(&ordinary_dir, "Ordinary.md");
        let ordinary_md = markdown(&ordinary);
        assert!(ordinary_md.contains("Child closed"), "{ordinary_md}");
        assert!(
            !ordinary_md.contains("[high]{.chip"),
            "ordinary hosts must not mint Bob chips: {ordinary_md}"
        );
    }

    fn collect_test_anchors(blocks: &[Block], ids: &mut Vec<String>) {
        for block in blocks {
            match block {
                Block::Task(task) => {
                    if let Some(anchor) = &task.anchor {
                        ids.push(anchor.id.clone());
                    }
                    collect_test_anchors(&task.children, ids);
                }
                Block::List { items, .. } => {
                    for item in items {
                        collect_test_anchors(&item.children, ids);
                    }
                }
                Block::Embed { blocks, .. }
                | Block::LiveQuery { blocks }
                | Block::Compact { blocks } => {
                    collect_test_anchors(blocks, ids)
                }
                Block::Callout { blocks, .. } => {
                    collect_test_anchors(blocks, ids)
                }
                Block::Paragraph(inlines) | Block::EmptyState(inlines) => {
                    collect_test_inline_anchors(inlines, ids);
                }
                _ => {}
            }
        }
    }

    fn collect_test_inline_anchors(inlines: &[Inline], ids: &mut Vec<String>) {
        for inline in inlines {
            match inline {
                Inline::Anchor(anchor) => ids.push(anchor.id.clone()),
                Inline::EmbeddedBlock(inner) => collect_test_anchors(
                    std::slice::from_ref(inner.as_ref()),
                    ids,
                ),
                Inline::Link { label, .. }
                | Inline::Emphasis(label)
                | Inline::Strong(label)
                | Inline::Strike(label)
                | Inline::Muted(label) => {
                    collect_test_inline_anchors(label, ids)
                }
                Inline::Chip(chip) => {
                    collect_test_inline_anchors(&chip.content, ids)
                }
                _ => {}
            }
        }
    }
    #[test]
    fn land_audit_relationship_child_once() {
        let directory = vault(&[(
            "Area.md",
            "---\ntype: [[area]]\n---\n# Area\n## Tasks\n- [ ] Parent ^parent\n\t- ⛓️ **DEPENDS ON:** [[Area#^other]]\n\t\t- [ ] Child [priority:: high] [scheduled:: 2026-10-20] [repeat:: every week] ^child\n- [ ] Other ^other\n",
        )]);
        let projection = project_bob(&directory, "Area.md", false, true);
        let output = markdown(&projection);
        assert_eq!(output.matches("[high]{.chip").count(), 1, "{output}");
        assert_eq!(output.matches("sched Oct 20").count(), 1, "{output}");
        assert_eq!(output.matches("repeats every week").count(), 1, "{output}");
        assert!(output.contains("Waits on"), "{output}");
    }
    #[test]
    fn land_audit_comment_fields_hidden() {
        let directory = vault(&[
            (
                "Area.md",
                "---\ntype: [[area]]\n---\n# Area\n## Tasks\n- [ ] Visible %% [priority:: high] [repeat:: secret] %% ^task\n- [ ] Html <!-- [priority:: high] #ref [[Snap]] --> ^html\n- [ ] Code `[priority:: high]` #task ^code\n",
            ),
            (
                "Snap.md",
                "---\nsource_note: \"[[Elsewhere]]\"\n---\n\n# Snap\n",
            ),
        ]);
        let projection = project_bob(&directory, "Area.md", false, true);
        let output = markdown(&projection);
        assert!(!output.contains("secret"), "{output}");
        assert!(!output.contains("[high]{.chip"), "{output}");
        // HTML-hidden fields, refs, and links never leak either.
        assert!(!output.contains("[[Snap]]"), "{output}");
        assert_eq!(output.matches("[ref]").count(), 0, "{output}");
        // Protected literal code stays readable and mints no chips.
        assert!(output.contains("[priority:: high]"), "{output}");
        // Multiline comment spans hide fields on later lines without
        // resurrecting them via per-line stripping.
        let multiline = vault(&[(
            "Area.md",
            "---\ntype: [[area]]\n---\n# Area\n## Tasks\n%% start\n[priority:: high] hidden\nend %%\n- [ ] After ^after\n",
        )]);
        let multi_proj = project_bob(&multiline, "Area.md", false, true);
        let multi_out = markdown(&multi_proj);
        assert!(!multi_out.contains("[high]{.chip"), "{multi_out}");
        assert!(multi_out.contains("After"), "{multi_out}");
    }
    #[test]
    fn land_audit_terminal_outcome_cancel_history() {
        let directory = vault(&[(
            "Project.md",
            "---\ntype: [[project]]\nstatus: canceled\n---\n# Project\n## Tasks\n- [-] Outcome #task [cancelled:: 2026-10-14] ^prj\n\t- ❌ **CANCEL LOG**\n\t\t- *2026-10-15* — cancellation explanation\n\t- Ordinary outcome child prose remains.\n",
        )]);
        // Full mode preserves the cancellation explanation and history.
        let projection = project_bob(&directory, "Project.md", true, true);
        let output = markdown(&projection);
        assert!(output.contains("cancellation explanation"), "{output}");
        assert!(
            output.contains("Ordinary outcome child prose remains"),
            "{output}"
        );
        assert!(output.contains("[canceled]{.chip}"), "{output}");
        assert!(output.contains("Outcome history"), "{output}");
        assert!(!output.contains("#t-prj"), "{output}");
        // Reading mode keeps status meaning via masthead chips while
        // respecting condensed-log policy.
        let reading = project_bob(&directory, "Project.md", false, true);
        let reading_out = markdown(&reading);
        assert!(reading_out.contains("[canceled]{.chip}"), "{reading_out}");
        assert!(
            reading_out.contains("Ordinary outcome child prose remains")
                || reading_out.contains("Outcome history"),
            "{reading_out}"
        );
        // Completed outcomes preserve completion chips and work history.
        let done_dir = vault(&[(
            "Done.md",
            "---\ntype: [[project]]\nstatus: done\n---\n# Done\n## Tasks\n- [x] Shipped #task [completion:: 2026-10-15] ^prj\n\t- 🛠️ **WORK LOG**\n\t\t- *2026-10-15* — final polish\n",
        )]);
        let done_full = project_bob(&done_dir, "Done.md", true, true);
        let done_out = markdown(&done_full);
        assert!(done_out.contains("final polish"), "{done_out}");
        assert!(done_out.contains("[done]{.chip}"), "{done_out}");
        let done_reading = project_bob(&done_dir, "Done.md", false, true);
        let done_reading_out = markdown(&done_reading);
        assert!(
            done_reading_out.contains("[done]{.chip}"),
            "{done_reading_out}"
        );
    }
    #[test]
    fn land_audit_closed_appendix_source_order() {
        let directory = vault(&[
            (
                "Area.md",
                "---\ntype: [[area]]\n---\n# Area\n- [x] Preamble closed ^before\n## Tasks\nBody ![[Embed]] tail.\n- [x] Section closed ^after\n",
            ),
            ("Embed.md", "- [x] Embedded closed ^embedded\n"),
        ]);
        let projection = project_bob(&directory, "Area.md", true, true);
        let output = markdown(&projection);
        let preamble_pos = output.find("Preamble closed").expect("{output}");
        let section_pos = output.find("Section closed").expect("{output}");
        let embedded_pos = output.find("Embedded closed").expect("{output}");
        // Document order is preserved in the appendix.
        assert!(preamble_pos < embedded_pos, "{output}");
        assert!(embedded_pos < section_pos, "{output}");
        // No duplicate descendants or anchors.
        assert_eq!(output.matches("Preamble closed").count(), 1, "{output}");
        assert_eq!(output.matches("Section closed").count(), 1, "{output}");
        assert_eq!(output.matches("Embedded closed").count(), 1, "{output}");
        assert!(output.contains("Body"), "{output}");
        assert!(output.contains("tail"), "{output}");
    }
    #[test]
    fn land_audit_nested_self_omission() {
        let directory = vault(&[
            ("Area.md", "---\ntype: [[area]]\n---\n# Area\n## Tasks\n- [x] Parent closed ^parent\n\t- [x] Read [[Snapshot]] #ref ^self\n"),
            ("Snapshot.md", "---\ntype: [[ref]]\nsource_note: '[[Area]]'\n---\n# Snapshot\n"),
        ]);
        // Reading mode omits both; include-closed restores the closed
        // parent to the appendix while the self child stays omitted.
        let reading = project_bob(&directory, "Area.md", false, true);
        assert_eq!(reading.view.summary.omitted_self_snapshot_tasks, 1);
        assert_eq!(reading.view.summary.omitted_closed_tasks, 1);
        let restored = project_bob(&directory, "Area.md", true, true);
        assert_eq!(restored.view.summary.omitted_self_snapshot_tasks, 1);
        assert_eq!(restored.view.summary.omitted_closed_tasks, 0);
        // Self-ref ancestors omit once as self; nested closed children
        // still count as closed with no overlap.
        let ancestor = vault(&[
            (
                "Area.md",
                "---\ntype: [[area]]\n---\n# Area\n## Tasks\n- [ ] Self parent [[Snapshot]] #ref ^selfparent\n\t- [x] Nested closed ^nested\n",
            ),
            ("Snapshot.md", "---\ntype: [[ref]]\nsource_note: '[[Area]]'\n---\n# Snapshot\n"),
        ]);
        for include_closed in [false, true] {
            let projection =
                project_bob(&ancestor, "Area.md", include_closed, true);
            assert_eq!(
                projection.view.summary.omitted_self_snapshot_tasks, 1,
                "include_closed={include_closed}"
            );
            assert_eq!(
                projection.view.summary.omitted_closed_tasks, 1,
                "include_closed={include_closed}"
            );
        }
        // Source bytes stay unchanged.
        let root = directory.path().canonicalize().expect("vault root");
        let before = fs::read(root.join("Area.md")).expect("read host");
        let _ = project_bob(&directory, "Area.md", false, true);
        assert_eq!(fs::read(root.join("Area.md")).expect("reread"), before);
    }
    #[test]
    fn land_audit_parent_readable_title() {
        let directory = vault(&[
            ("Area.md", "---\ntype: [[area]]\nparent: '[[Parent]]'\n---\n# Area\nPurpose.\n"),
            ("Parent.md", "# [[Goals|Project Goals]]\n"),
            ("Goals.md", "# Goals\n"),
        ]);
        let projection = project_bob(&directory, "Area.md", false, true);
        assert_eq!(
            plain_inline_text(&projection.view.masthead.kicker),
            "AREA · Project Goals"
        );
        // Emphasized titles project to readable text.
        let emphasized = vault(&[
            (
                "Area.md",
                "---\ntype: [[area]]\nparent: '[[Parent]]'\n---\n# Area\n",
            ),
            ("Parent.md", "# **Bold Goals**\n"),
        ]);
        assert_eq!(
            plain_inline_text(
                &project_bob(&emphasized, "Area.md", false, true)
                    .view
                    .masthead
                    .kicker
            ),
            "AREA · Bold Goals"
        );
        // Comment-hidden title text stays hidden.
        let commented = vault(&[
            (
                "Area.md",
                "---\ntype: [[area]]\nparent: '[[Parent]]'\n---\n# Area\n",
            ),
            ("Parent.md", "# Visible %%secret%%\n"),
        ]);
        assert_eq!(
            plain_inline_text(
                &project_bob(&commented, "Area.md", false, true)
                    .view
                    .masthead
                    .kicker
            ),
            "AREA · Visible"
        );
        // Fenced H1 lookalikes never become the parent label; the route
        // fallback applies.
        let fenced = vault(&[
            (
                "Area.md",
                "---\ntype: [[area]]\nparent: '[[Parent]]'\n---\n# Area\n",
            ),
            ("Parent.md", "```\n# Fenced\n```\n"),
        ]);
        assert_eq!(
            plain_inline_text(
                &project_bob(&fenced, "Area.md", false, true)
                    .view
                    .masthead
                    .kicker
            ),
            "AREA · Parent"
        );
        // Explicit parent aliases are preserved.
        let aliased = vault(&[(
            "Area.md",
            "---\ntype: [[area]]\nparent: '[[Parent|Custom]]'\n---\n# Area\n",
        )]);
        assert_eq!(
            plain_inline_text(
                &project_bob(&aliased, "Area.md", false, true)
                    .view
                    .masthead
                    .kicker
            ),
            "AREA · Custom"
        );
    }
}
