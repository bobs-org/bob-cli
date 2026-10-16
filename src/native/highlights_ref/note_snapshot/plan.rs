//! Pure note-snapshot planning: title/date defaults and projection assembly.

use chrono::NaiveDate;
use serde_yaml::Value as YamlValue;

use super::{
    classify::{ClassifiedNote, NoteKind, SourceLoader, SourceStore},
    obsidian::project_obsidian_syntax,
    resolve::AttachmentIndex,
    serialize::serialize_projected_markdown,
    transclusion::{
        project_snapshot_from_syntax, SnapshotProjection,
        SnapshotProjectionOptions,
    },
    view::{Inline, SnapshotMasthead, SnapshotSummary, SnapshotWarning},
};
use crate::native::{
    parent_notes::ResolvedParent, task_status_hooks::TasksSettings,
};

/// Where the selected note display title came from.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum SnapshotTitleSource {
    Override,
    Frontmatter,
    Heading,
    Stem,
}

impl SnapshotTitleSource {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Override => "override",
            Self::Frontmatter => "frontmatter",
            Self::Heading => "first H1",
            Self::Stem => "humanized stem",
        }
    }
}

/// A source-and-projection plan, independent of target collision/install policy.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct SnapshotPlan {
    pub note: ClassifiedNote,
    pub projection: SnapshotProjection,
    pub projected_markdown: String,
    pub source_markdown: String,
    pub source_digest: String,
    pub source_kind: &'static str,
    pub display_title: String,
    pub identity_title: String,
    pub title_source: SnapshotTitleSource,
    pub consumed_title_heading: Option<String>,
    pub snapshot_date: NaiveDate,
    pub dated_stem: String,
    pub parent: Option<ResolvedParent>,
    pub parent_default_reason: Option<String>,
    pub summary: SnapshotSummary,
    pub warnings: Vec<SnapshotWarning>,
}

/// Select defaults and build the snapshot projection from one cached syntax pass.
/// An explicit parent, when present, is already resolved by create's preflight.
pub(crate) fn plan_snapshot<L: SourceLoader>(
    store: &mut SourceStore<L>,
    note: ClassifiedNote,
    settings: &TasksSettings,
    snapshot_date: NaiveDate,
    title_override: Option<&str>,
    include_closed: bool,
    section_names: &[String],
    parent: ResolvedParent,
    parent_default_reason: Option<String>,
) -> Result<SnapshotPlan, String> {
    let mut attachments =
        AttachmentIndex::new(store.vault_root().to_path_buf());
    let syntax = project_obsidian_syntax(
        store,
        &mut attachments,
        &note.identity,
        settings,
    )?;

    let source_stem = note
        .identity
        .route
        .rsplit('/')
        .next()
        .filter(|stem| !stem.is_empty())
        .ok_or_else(|| {
            format!("note route has no filename stem: {}", note.identity.route)
        })?;
    let (display_title, title_source, consumed_title_heading) =
        select_title(&syntax, source_stem, title_override);
    let identity_title = if title_source == SnapshotTitleSource::Override {
        display_title.clone()
    } else {
        format!("{} · {}", display_title, snapshot_date.format("%Y-%m-%d"))
    };
    let date_stem = snapshot_date.format("%y%m%d");
    let dated_stem = format!("{source_stem}_{date_stem}");
    let source_markdown = String::from_utf8(syntax.parsed.raw_bytes.to_vec())
        .map_err(|error| {
        format!(
            "read vault note {} as UTF-8: {error}",
            note.canonical_path.display()
        )
    })?;
    let source_digest = syntax.parsed.sha256.clone();
    let masthead = note_masthead(
        &note,
        &identity_title,
        &display_title,
        snapshot_date,
        include_closed,
        syntax
            .parsed
            .frontmatter
            .as_ref()
            .map(|front| &front.metadata),
    );
    let mut projection = project_snapshot_from_syntax(
        store,
        &note,
        settings,
        SnapshotProjectionOptions {
            masthead,
            snapshot_date,
            include_closed,
            consumed_title_heading: consumed_title_heading.clone(),
        },
        attachments,
        syntax,
    )?;
    if !section_names.is_empty() {
        let requested = section_names
            .iter()
            .map(|name| name.trim().to_lowercase())
            .collect::<std::collections::BTreeSet<_>>();
        if requested.iter().any(String::is_empty) {
            return Err("section names cannot be empty".to_string());
        }
        let mut available = projection
            .view
            .sections
            .iter()
            .map(|section| {
                section
                    .heading
                    .content
                    .iter()
                    .filter_map(|inline| match inline {
                        Inline::Text(text) => Some(text.as_str()),
                        _ => None,
                    })
                    .collect::<String>()
            })
            .collect::<Vec<_>>();
        for section in &projection.view.sections {
            available.extend(section.blocks.iter().filter_map(|block| {
                match block {
                    super::view::Block::Heading(heading) => Some(
                        heading
                            .content
                            .iter()
                            .filter_map(|inline| match inline {
                                Inline::Text(text) => Some(text.as_str()),
                                _ => None,
                            })
                            .collect::<String>(),
                    ),
                    _ => None,
                }
            }));
        }
        let matched = available
            .iter()
            .any(|name| requested.contains(&name.to_lowercase()));
        if !matched {
            return Err(format!(
                "none of the requested sections exist: {}; available sections: {}",
                section_names.join(", "),
                if available.is_empty() { "none".to_string() } else { available.join(" · ") },
            ));
        }
        let mut selected_nested = Vec::new();
        for section in &projection.view.sections {
            collect_named_heading_sections(
                section,
                &requested,
                &mut selected_nested,
            );
        }
        projection.view.sections.retain(|section| {
            let name = section
                .heading
                .content
                .iter()
                .filter_map(|inline| match inline {
                    Inline::Text(text) => Some(text.as_str()),
                    _ => None,
                })
                .collect::<String>();
            requested.contains(&name.to_lowercase())
        });
        if !selected_nested.is_empty() {
            projection.view.sections = selected_nested;
        }
        projection.view.summary.sections =
            projection.view.sections.len() as u64;
        projection.view.summary.task_counts.clear();
        for section in &projection.view.sections {
            count_section_tasks(
                &section.blocks,
                &mut projection.view.summary.task_counts,
            );
        }
    }
    let projected_markdown = serialize_projected_markdown(&projection.view)
        .map_err(|error| format!("serialize note snapshot: {error}"))?;
    let summary = projection.view.summary.clone();
    let warnings = projection.view.warnings.clone();
    let source_kind = match note.kind {
        NoteKind::Ordinary => "note",
        NoteKind::Area => "area",
        NoteKind::Project | NoteKind::TerminalProject => "project",
    };

    Ok(SnapshotPlan {
        note,
        projection,
        projected_markdown,
        source_markdown,
        source_digest,
        source_kind,
        display_title,
        identity_title,
        title_source,
        consumed_title_heading,
        snapshot_date,
        dated_stem,
        parent: Some(parent),
        parent_default_reason,
        summary,
        warnings,
    })
}

fn collect_named_heading_sections(
    section: &super::view::SnapshotSection,
    requested: &std::collections::BTreeSet<String>,
    output: &mut Vec<super::view::SnapshotSection>,
) {
    use super::view::{Block, SnapshotSection};
    let headings = section
        .blocks
        .iter()
        .enumerate()
        .filter_map(|(index, block)| {
            if let Block::Heading(heading) = block {
                let name = heading
                    .content
                    .iter()
                    .filter_map(|inline| match inline {
                        Inline::Text(text) => Some(text.as_str()),
                        _ => None,
                    })
                    .collect::<String>();
                Some((index, heading.clone(), name))
            } else {
                None
            }
        })
        .collect::<Vec<_>>();
    for (position, (index, heading, name)) in headings.iter().enumerate() {
        if !requested.contains(&name.to_lowercase()) {
            continue;
        }
        let end = headings
            .get(position + 1)
            .map(|(next, _, _)| *next)
            .unwrap_or(section.blocks.len());
        output.push(SnapshotSection {
            source: section.source.clone(),
            heading: heading.clone(),
            blocks: section.blocks[index + 1..end].to_vec(),
        });
    }
}

fn count_section_tasks(
    blocks: &[super::view::Block],
    counts: &mut std::collections::BTreeMap<
        super::view::SnapshotTaskStatus,
        u64,
    >,
) {
    use super::view::Block;
    for block in blocks {
        match block {
            Block::Task(task) => {
                *counts.entry(task.status).or_default() += 1;
                count_section_tasks(&task.children, counts);
            }
            Block::List { items, .. } => {
                for item in items {
                    count_section_tasks(&item.children, counts);
                }
            }
            Block::Embed { blocks, .. }
            | Block::LiveQuery { blocks }
            | Block::Compact { blocks }
            | Block::Callout { blocks, .. } => {
                count_section_tasks(blocks, counts)
            }
            _ => {}
        }
    }
}

fn select_title(
    syntax: &super::obsidian::SyntaxProjection,
    source_stem: &str,
    title_override: Option<&str>,
) -> (String, SnapshotTitleSource, Option<String>) {
    if let Some(title) = title_override.filter(|title| !title.is_empty()) {
        return (title.to_string(), SnapshotTitleSource::Override, None);
    }
    if let Some(title) = syntax
        .parsed
        .frontmatter
        .as_ref()
        .and_then(|front| front.metadata.get("title"))
        .and_then(yaml_scalar_text)
        .filter(|title| !title.trim().is_empty())
    {
        return (title, SnapshotTitleSource::Frontmatter, None);
    }
    if let Some(candidate) = &syntax.title_candidate {
        return (
            candidate.display_text.clone(),
            SnapshotTitleSource::Heading,
            Some(candidate.anchor_id.clone()),
        );
    }
    (
        crate::native::ref_library::migrate_zorg::titleize_id(source_stem),
        SnapshotTitleSource::Stem,
        None,
    )
}

fn yaml_scalar_text(value: &YamlValue) -> Option<String> {
    match value {
        YamlValue::String(value) => Some(value.clone()),
        YamlValue::Number(value) => Some(value.to_string()),
        YamlValue::Bool(value) => Some(value.to_string()),
        _ => None,
    }
}

fn note_masthead(
    note: &ClassifiedNote,
    identity_title: &str,
    display_title: &str,
    snapshot_date: NaiveDate,
    include_closed: bool,
    metadata: Option<&YamlValue>,
) -> SnapshotMasthead {
    let date = snapshot_date.format("%b %-d, %Y");
    let return_links = metadata
        .and_then(|metadata| metadata.get("bob-return-links"))
        .is_none_or(|value| match value {
            YamlValue::Bool(value) => *value,
            YamlValue::String(value) => !value.eq_ignore_ascii_case("false"),
            _ => true,
        });
    SnapshotMasthead {
        title: identity_title.to_string(),
        kicker: vec![Inline::Text("NOTE".to_string())],
        display_title: vec![Inline::Text(display_title.to_string())],
        dek: None,
        meta: Vec::new(),
        stats: Vec::new(),
        omitted: None,
        snapshot_line: vec![Inline::Text(format!(
            "Snapshot of {display_title} · {} · {date}",
            note.identity.route
        ))],
        footer: vec![Inline::Text(format!(
            "Source: {} · {date}{}",
            note.identity.route,
            if include_closed {
                " · closed tasks included"
            } else {
                ""
            }
        ))],
        glyph_table: "glyphs.lua".to_string(),
        return_links,
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;
    use crate::native::highlights_ref::note_snapshot::{
        classify_target, default_parent, CountingLoader, SnapshotTarget,
        SourceStore,
    };

    fn plan_for_source(
        contents: &str,
        title_override: Option<&str>,
    ) -> (SnapshotPlan, usize) {
        let directory = tempfile::tempdir().expect("temp vault");
        let root = directory.path();
        let source = root.join("zk/harbor_cranes.md");
        let parent_path = root.join("Area.md");
        fs::create_dir_all(source.parent().expect("source parent"))
            .expect("create source directory");
        fs::write(&source, contents).expect("write source");
        fs::write(&parent_path, "---\ntype: [[area]]\n---\n# Area\n")
            .expect("write parent");
        let source = source.canonicalize().expect("canonical source");
        let parent_path = parent_path.canonicalize().expect("canonical parent");
        let root = root.canonicalize().expect("canonical root");
        let loader = CountingLoader::with_files(vec![
            (source, contents.as_bytes().to_vec()),
            (parent_path, b"---\ntype: [[area]]\n---\n# Area\n".to_vec()),
        ]);
        let mut store =
            SourceStore::with_loader(&root, loader).expect("source store");
        let SnapshotTarget::VaultNote(note) =
            classify_target(&mut store, "zk/harbor_cranes")
                .expect("classify source")
        else {
            panic!("expected vault note");
        };
        let parent =
            default_parent(&mut store, &note).expect("infer parent from cache");
        let settings =
            crate::native::task_status_hooks::read_tasks_settings(&root);
        let plan = plan_snapshot(
            &mut store,
            note,
            &settings,
            NaiveDate::from_ymd_opt(2026, 10, 16).expect("date"),
            title_override,
            false,
            &[],
            parent.resolved,
            Some(parent.reason),
        )
        .expect("build snapshot plan");
        (plan, store.loader().reads())
    }

    #[test]
    fn plan_uses_one_cached_source_for_title_parent_and_projection() {
        let (plan, reads) = plan_for_source(
            "---\nparent: [[Area]]\n---\n# Harbor Cranes\n\nA note.\n",
            None,
        );

        assert_eq!(reads, 2, "the note and its parent are each read once");
        assert_eq!(plan.display_title, "Harbor Cranes");
        assert_eq!(plan.title_source, SnapshotTitleSource::Heading);
        assert!(plan.consumed_title_heading.is_some());
        assert_eq!(plan.identity_title, "Harbor Cranes · 2026-10-16");
        assert_eq!(plan.dated_stem, "harbor_cranes_261016");
        assert_eq!(
            plan.parent.as_ref().map(|parent| parent.route.as_str()),
            Some("Area")
        );
        assert_eq!(plan.source_digest.len(), 64);
        assert!(plan.projected_markdown.contains("Harbor Cranes"));
    }

    #[test]
    fn plan_title_precedence_keeps_frontmatter_heading_and_exact_override() {
        let with_frontmatter = concat!(
            "---\nparent: [[Area]]\ntitle: Frontmatter Name\n---\n",
            "# Body Heading\n\nA note.\n",
        );
        let (frontmatter, _) = plan_for_source(with_frontmatter, None);
        assert_eq!(frontmatter.display_title, "Frontmatter Name");
        assert_eq!(frontmatter.title_source, SnapshotTitleSource::Frontmatter);
        assert!(frontmatter.consumed_title_heading.is_none());
        assert!(frontmatter.projected_markdown.contains("Body Heading"));

        let (override_plan, _) =
            plan_for_source(with_frontmatter, Some("Exact Identity"));
        assert_eq!(override_plan.display_title, "Exact Identity");
        assert_eq!(override_plan.identity_title, "Exact Identity");
        assert_eq!(override_plan.title_source, SnapshotTitleSource::Override);
        assert!(override_plan.consumed_title_heading.is_none());
    }
}
