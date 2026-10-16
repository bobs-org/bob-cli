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
    classify::{source_identity, ClassifiedNote, SourceLoader, SourceStore},
    obsidian::{
        project_obsidian_syntax, project_obsidian_syntax_selection,
        PendingEmbed, SyntaxProjection, TitleCandidate,
    },
    resolve::{
        build_fragment_index, obsidian_uri_with_fragment, AttachmentIndex,
        FragmentIndex, FragmentLookup, LinkFragment,
    },
    view::{
        AnchorIdentity, AnchorKind, Block, HeadingClass, HeadingLevel, Inline,
        LinkTarget, ListItem, ParsedSource, SnapshotAnchor, SnapshotAsset,
        SnapshotHeading, SnapshotMasthead, SnapshotSection, SnapshotSummary,
        SnapshotTaskStatus, SnapshotView, SnapshotWarning, SnapshotWarningKind,
        SourceIdentity, SourceLocation,
    },
};
use crate::native::{
    markdown::strictly_closed_frontmatter_end, task_status_hooks::TasksSettings,
};

const MAX_TRANSCLUSION_BYTES: usize = 64 * 1024;
const MAX_TRANSCLUSION_LINES: usize = 1_500;

/// Caller-owned, deterministic metadata for one snapshot projection.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct SnapshotProjectionOptions {
    pub masthead: SnapshotMasthead,
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

    let (sections, synthetic_anchor) = sectionize_root(&note.identity, blocks);
    if let Some(anchor) = synthetic_anchor {
        expander.used_anchor_ids.insert(anchor.id);
    }
    let mut anchors = Vec::new();
    for section in &sections {
        collect_section_anchors(section, &mut anchors);
    }
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
    count_task_statuses(&sections, &mut summary.task_counts);

    let view = SnapshotView {
        source: note.identity.clone(),
        masthead: options.masthead,
        sections,
        closed: None,
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
                        self.expand_blocks(
                            &mut item.children,
                            requests,
                            current_source,
                            request_index,
                            may_expand,
                        );
                    }
                }
                Block::Task(task) => self.expand_blocks(
                    &mut task.children,
                    requests,
                    current_source,
                    request_index,
                    may_expand,
                ),
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
                let heading = content_heading(source);
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
        let heading = content_heading(source);
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

fn content_heading(source: &SourceIdentity) -> SnapshotHeading {
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
            id: "snapshot-content".to_string(),
        },
    }
}

fn collect_section_anchors(
    section: &SnapshotSection,
    anchors: &mut Vec<SnapshotAnchor>,
) {
    anchors.push(section.heading.anchor.clone());
    collect_block_anchors(&section.blocks, anchors);
}

fn collect_block_anchors(blocks: &[Block], anchors: &mut Vec<SnapshotAnchor>) {
    for block in blocks {
        match block {
            Block::Heading(heading) => anchors.push(heading.anchor.clone()),
            Block::Task(task) => {
                if let Some(anchor) = &task.anchor {
                    anchors.push(anchor.clone());
                }
                collect_block_anchors(&task.children, anchors);
            }
            Block::List { items, .. } => {
                for item in items {
                    collect_block_anchors(&item.children, anchors);
                }
            }
            Block::Embed { blocks, .. }
            | Block::LiveQuery { blocks }
            | Block::Callout { blocks, .. }
            | Block::Compact { blocks } => {
                collect_block_anchors(blocks, anchors)
            }
            _ => {}
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
                count_blocks(&task.children, counts);
            }
            Block::List { items, .. } => {
                for item in items {
                    count_blocks(&item.children, counts);
                }
            }
            Block::Embed { blocks, .. }
            | Block::LiveQuery { blocks }
            | Block::Callout { blocks, .. }
            | Block::Compact { blocks } => count_blocks(blocks, counts),
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
            }
            Block::Task(task) => {
                if let Some(original) = &task.anchor {
                    if let Some(anchor) = mapping
                        .get(&(original.identity.clone(), original.id.clone()))
                    {
                        task.anchor = Some(anchor.clone());
                    }
                }
                remap_block_anchors(&mut task.children, mapping);
            }
            Block::List { items, .. } => {
                for ListItem { children, .. } in items {
                    remap_block_anchors(children, mapping);
                }
            }
            Block::Embed { blocks, .. }
            | Block::LiveQuery { blocks }
            | Block::Callout { blocks, .. }
            | Block::Compact { blocks } => remap_block_anchors(blocks, mapping),
            _ => {}
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
            Inline::Text(_) | Inline::Code(_) | Inline::Break => {}
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
        path::{Path, PathBuf},
    };

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
            },
        )
        .expect("project snapshot")
    }

    fn embeds(blocks: &[Block]) -> Vec<&Block> {
        let mut found = Vec::new();
        fn walk<'a>(blocks: &'a [Block], found: &mut Vec<&'a Block>) {
            for block in blocks {
                match block {
                    Block::Embed { .. } => found.push(block),
                    Block::Task(task) => walk(&task.children, found),
                    Block::List { items, .. } => {
                        for item in items {
                            walk(&item.children, found);
                        }
                    }
                    Block::Callout { blocks, .. }
                    | Block::Compact { blocks }
                    | Block::LiveQuery { blocks } => walk(blocks, found),
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
                }
                Block::Heading(heading) => {
                    output.push(flatten(&heading.content))
                }
                Block::Task(task) => {
                    output.push(flatten(&task.content));
                    view_text(&task.children, output);
                }
                Block::List { items, .. } => {
                    for item in items {
                        output.push(flatten(&item.content));
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

    fn flatten(inlines: &[Inline]) -> String {
        let mut output = String::new();
        for inline in inlines {
            match inline {
                Inline::Text(text) | Inline::Code(text) => {
                    output.push_str(text)
                }
                Inline::Link { label, .. }
                | Inline::Emphasis(label)
                | Inline::Strong(label)
                | Inline::Strike(label)
                | Inline::Muted(label) => output.push_str(&flatten(label)),
                Inline::Chip(chip) => output.push_str(&flatten(&chip.content)),
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
}
