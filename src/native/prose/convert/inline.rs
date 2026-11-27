//! Planner and CLI for folding a prose note back into its parent section.

use std::{
    collections::{BTreeMap, BTreeSet},
    ffi::OsString,
    fs,
    path::{Component, Path, PathBuf},
};

use super::super::{
    findings::{Finding, Severity},
    format, marker,
    plan::sha256_hex,
    relocate::{self, Direction, RelocationError, RelocationMap},
    scan::{self, Region},
    section_model::{self, HostKind, ProseSection},
    sections::{heading_identity, heading_plain_identity, plan_headings},
    vault, ProseContext,
};
use super::extract::{
    self, ConversionChange, ConversionCommit, ConversionCounts,
    ConversionFocus, ConversionLink, ConversionRefusal, ConversionReport,
    ConversionSection, ConversionSync, ConversionTarget,
};
use crate::native::{
    env as bob_env, is_always_excluded_note_directory_name, ob,
    projects::{
        frontmatter_is_prose, frontmatter_value, parse_frontmatter,
        wikilink_path_target,
    },
    vault_links::{LinkResolution, VaultLinkResolver},
    vault_rewrite,
    yaml_spans::{collect_key_spans, Line as YamlLine},
};

const ABSORBED_FRONTMATTER: &[&str] = &[
    "parent",
    "type",
    "created",
    "title",
    "section_numbers",
    "template",
    "extracted_from",
    "extracted_heading",
    "extracted_intro",
];

#[derive(Debug, Clone)]
pub(crate) struct InlinePlan {
    pub(crate) parent_path: PathBuf,
    pub(crate) parent_before: String,
    pub(crate) parent_after: String,
    pub(crate) child_path: PathBuf,
    pub(crate) child_before: String,
    pub(crate) child_stem: String,
    pub(crate) linker_files: Vec<(PathBuf, String, String)>,
    pub(crate) links: Vec<ConversionLink>,
    pub(crate) section_line: usize,
    pub(crate) section_end: usize,
    pub(crate) section_level: usize,
    pub(crate) section_title: String,
    pub(crate) fingerprint: String,
    pub(crate) layout_count: usize,
    pub(crate) replaced_count: usize,
    pub(crate) findings: Vec<Finding>,
    pub(crate) focus_line: usize,
}

#[derive(Debug, Clone)]
pub(crate) struct InlineRefusal {
    pub(crate) code: String,
    pub(crate) message: String,
    pub(crate) hint: String,
    pub(crate) path: Option<PathBuf>,
    pub(crate) line: Option<usize>,
    pub(crate) section: Option<ConversionSection>,
    pub(crate) target: Option<ConversionTarget>,
}

impl InlineRefusal {
    fn new(
        code: &str,
        message: impl Into<String>,
        hint: impl Into<String>,
        path: Option<&Path>,
        line: Option<usize>,
    ) -> Self {
        Self {
            code: code.to_string(),
            message: message.into(),
            hint: hint.into(),
            path: path.map(Path::to_path_buf),
            line,
            section: None,
            target: None,
        }
    }

    fn at_section(mut self, section: &ProseSection<'_>) -> Self {
        self.section = Some(ConversionSection {
            line: section.heading_line,
            end_line: section.body_range.end,
            level: section.heading_level,
            title: section.raw_title.clone(),
        });
        self
    }
}

fn display_relative(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

fn empty_fingerprint() -> String {
    format!("sha256:{}", sha256_hex("empty"))
}

fn fingerprint_for(
    touched: &BTreeMap<String, String>,
    heading_line: usize,
    end_line: usize,
    stem: &str,
) -> String {
    let mut canonical = String::new();
    for (path, hash) in touched {
        canonical.push_str(path);
        canonical.push(':');
        canonical.push_str(hash);
        canonical.push('\n');
    }
    canonical.push_str(&format!("section:{heading_line}:{end_line}\n"));
    canonical.push_str(&format!("stem:{stem}\n"));
    canonical.push_str("stub:link\n");
    format!("sha256:{}", sha256_hex(&canonical))
}

fn excluded_path(path: &Path) -> bool {
    path.components().any(|component| match component {
        Component::Normal(name) => {
            is_always_excluded_note_directory_name(name)
                || matches!(
                    name.to_string_lossy().to_lowercase().as_str(),
                    "done" | "ref" | "refs"
                )
        }
        _ => false,
    })
}

fn parent_target(text: &str) -> Option<String> {
    let frontmatter = parse_frontmatter(marker::strip_bom(text))?;
    let raw = frontmatter_value(&frontmatter, "parent")?;
    wikilink_path_target(raw)
}

fn frontmatter_title(text: &str) -> Option<String> {
    let frontmatter = parse_frontmatter(marker::strip_bom(text))?;
    let yaml = frontmatter.lines.join("\n");
    let value = serde_yaml::from_str::<serde_yaml::Value>(&yaml).ok()?;
    let mapping = value.as_mapping()?;
    mapping
        .get(serde_yaml::Value::String("title".to_string()))?
        .as_str()
        .map(ToOwned::to_owned)
}

fn unknown_frontmatter(text: &str, path: &Path) -> Result<(), InlineRefusal> {
    let frontmatter =
        parse_frontmatter(marker::strip_bom(text)).ok_or_else(|| {
            InlineRefusal::new(
                "not-prose",
                "note has no complete frontmatter",
                "Repair the prose note's frontmatter, then retry",
                Some(path),
                None,
            )
        })?;
    let yaml_lines = frontmatter
        .lines
        .iter()
        .map(|line| YamlLine {
            text: (*line).to_string(),
            ending: String::new(),
        })
        .collect::<Vec<_>>();
    let spans = collect_key_spans(&yaml_lines, 0).map_err(|message| {
        InlineRefusal::new(
            "unknown-frontmatter",
            format!("frontmatter cannot be safely absorbed: {message}"),
            "Remove duplicate or malformed frontmatter keys before inlining",
            Some(path),
            None,
        )
    })?;
    let unknown = spans
        .iter()
        .filter(|span| !ABSORBED_FRONTMATTER.contains(&span.name.as_str()))
        .collect::<Vec<_>>();
    if let Some(first) = unknown.first() {
        let names = unknown
            .iter()
            .map(|span| format!("`{}`", span.name))
            .collect::<Vec<_>>()
            .join(", ");
        return Err(InlineRefusal::new(
            "unknown-frontmatter",
            format!("inline would discard unsupported frontmatter: {names}"),
            "Move those properties to another note before inlining",
            Some(path),
            Some(first.start + 2),
        ));
    }
    Ok(())
}

fn child_notes(
    root: &Path,
    child_path: &Path,
) -> Result<Vec<PathBuf>, InlineRefusal> {
    let paths = vault::list_markdown(root).map_err(|error| {
        InlineRefusal::new(
            "read-failed",
            error.to_string(),
            "Check the vault and retry",
            Some(&error.path),
            None,
        )
    })?;
    let resolver = VaultLinkResolver::new(root);
    let mut children = Vec::new();
    for path in paths {
        if path == child_path {
            continue;
        }
        let bytes = fs::read(root.join(&path)).map_err(|error| {
            InlineRefusal::new(
                "read-failed",
                format!("could not read {}: {error}", display_relative(&path)),
                "Check the vault and retry",
                Some(&path),
                None,
            )
        })?;
        let Ok(text) = String::from_utf8(bytes) else {
            continue;
        };
        let Some(target) = parent_target(&text) else {
            continue;
        };
        if resolver.resolve(&path, &target).path() == Some(child_path) {
            children.push(path);
        }
    }
    Ok(children)
}

fn resolve_parent(
    root: &Path,
    child_path: &Path,
    child_text: &str,
) -> Result<(PathBuf, String, HostKind), InlineRefusal> {
    let Some(target) = parent_target(child_text) else {
        return Err(InlineRefusal::new(
            "no-parent",
            "prose note has no resolvable `parent` link",
            "Add a `parent: \"[[note]]\"` link to an area, project, or prose note",
            Some(child_path),
            None,
        ));
    };
    let resolver = VaultLinkResolver::new(root);
    let parent_path = match resolver.resolve(child_path, &target) {
        LinkResolution::Found(path) => path,
        LinkResolution::Ambiguous | LinkResolution::Missing => {
            return Err(InlineRefusal::new(
                "no-parent",
                format!("parent link {target:?} does not resolve to one note"),
                "Repair the parent link, then retry",
                Some(child_path),
                None,
            ));
        }
    };
    let parent_text =
        fs::read_to_string(root.join(&parent_path)).map_err(|error| {
            InlineRefusal::new(
                "read-failed",
                format!(
                    "could not read {}: {error}",
                    display_relative(&parent_path)
                ),
                "Check the vault and retry",
                Some(&parent_path),
                None,
            )
        })?;
    let host_kind = section_model::classify_host(&parent_path, &parent_text)
        .map_err(|_| {
            InlineRefusal::new(
                "parent-not-host",
                format!(
                    "parent {} is not an area, project, or prose note",
                    display_relative(&parent_path)
                ),
                "Set `parent` to an eligible area, project, or prose note",
                Some(&parent_path),
                None,
            )
        })?;
    Ok((parent_path, parent_text, host_kind))
}

fn conflict_marker(text: &str) -> Option<usize> {
    let document = scan::classify(text);
    document.lines.iter().find_map(|line| {
        marker::body_refusals(std::slice::from_ref(line))
            .filter(|refusal| refusal.code == "conflict-markers")
            .map(|_| line.index + 1)
    })
}

fn section_stubs<'a>(
    parent_path: &Path,
    parent_text: &'a str,
    child_path: &Path,
    root: &Path,
) -> Result<Vec<(ProseSection<'a>, section_model::StubTarget)>, InlineRefusal> {
    let sections = section_model::all_sections(parent_path, parent_text)
        .map_err(|refusal| {
            InlineRefusal::new(
                "parent-not-host",
                refusal.message,
                refusal.hint,
                Some(parent_path),
                refusal.line,
            )
        })?;
    let resolver = VaultLinkResolver::new(root);
    Ok(sections
        .into_iter()
        .filter_map(|section| {
            let stub = section_model::stub_target(&section)?;
            let LinkResolution::Found(resolved) =
                resolver.resolve(parent_path, &stub.target)
            else {
                return None;
            };
            (resolved == child_path).then_some((section, stub))
        })
        .collect())
}

fn demoted_body(
    text: &str,
    section_level: usize,
    path: &Path,
) -> Result<(Vec<(String, String)>, usize), InlineRefusal> {
    let document = scan::classify(text);
    let numbering = plan_headings(&document.lines, false);
    check_depth(text, section_level, path)?;

    let mut body = Vec::new();
    let mut visible = BTreeMap::new();
    for line in &document.lines {
        if line.region == Region::Frontmatter {
            continue;
        }
        visible.insert(line.index, body.len());
        body.push((line.content.to_string(), line.ending.to_string()));
    }
    for heading in &numbering.headings {
        if let Some(index) = visible.get(&heading.line).copied() {
            let level = heading.original_level + section_level - 1;
            body[index].0 = format!("{} {}", "#".repeat(level), heading.title);
        }
        if let Some(underline) = heading.underline
            && let Some(index) = visible.get(&underline)
        {
            body[*index].0.clear();
        }
    }
    let replaced = numbering.replaced.len();
    while body.first().is_some_and(|(line, _)| line.trim().is_empty()) {
        body.remove(0);
    }
    while body.last().is_some_and(|(line, _)| line.trim().is_empty()) {
        body.pop();
    }
    Ok((body, replaced))
}

fn check_depth(
    text: &str,
    section_level: usize,
    path: &Path,
) -> Result<(), InlineRefusal> {
    for section in section_model::all_sections(path, text).map_err(|_| {
        InlineRefusal::new(
            "not-prose",
            "note no longer classifies as prose",
            "Repair the prose note's type and frontmatter",
            Some(path),
            None,
        )
    })? {
        let level = section.heading_level;
        if level < 2 || level + section_level.saturating_sub(1) > 6 {
            return Err(InlineRefusal::new(
                "depth-overflow",
                format!(
                    "heading at line {} cannot be demoted under this section",
                    section.heading_line
                ),
                "Flatten the prose note's headings before inlining",
                Some(path),
                Some(section.heading_line),
            ));
        }
    }
    Ok(())
}

fn splice_parent(
    parent_text: &str,
    section: &ProseSection<'_>,
    body: &[(String, String)],
) -> String {
    let document = scan::classify(parent_text);
    let default_ending = document.default_ending;
    let mut lines = document
        .lines
        .iter()
        .map(|line| (line.content.to_string(), line.ending.to_string()))
        .collect::<Vec<_>>();
    let mut replacement = Vec::new();
    replacement.push((String::new(), default_ending.to_string()));
    replacement.extend(body.iter().cloned());
    if let Some((_, ending)) = replacement.last_mut()
        && ending.is_empty()
    {
        *ending = default_ending.to_string();
    }
    replacement.push((String::new(), default_ending.to_string()));
    let at_eof = section.body_range.end >= lines.len();
    if at_eof {
        replacement.pop();
    }
    if replacement.is_empty() {
        replacement.push((String::new(), default_ending.to_string()));
    }
    lines.splice(section.body_range.clone(), replacement);
    super::super::layout::join(
        document.bom,
        &lines,
        document.unclosed_fence.is_some(),
        default_ending,
    )
}

fn unified_diff(path: &str, before: &str, after: &str) -> String {
    let diff = similar::TextDiff::from_lines(before, after);
    let mut out = String::new();
    out.push_str(&format!("--- a/{path}\n+++ b/{path}\n"));
    for hunk in diff.unified_diff().context_radius(3).iter_hunks() {
        out.push_str(&hunk.header().to_string());
        for change in hunk.iter_changes() {
            let sign = match change.tag() {
                similar::ChangeTag::Delete => "-",
                similar::ChangeTag::Insert => "+",
                similar::ChangeTag::Equal => " ",
            };
            out.push_str(sign);
            out.push_str(change.value());
            if !change.value().ends_with('\n') {
                out.push('\n');
            }
        }
    }
    out
}

fn links_from(
    path: &Path,
    links: Vec<relocate::LinkRewrite>,
) -> Vec<ConversionLink> {
    links
        .into_iter()
        .map(|link| ConversionLink {
            path: display_relative(path),
            line: link.line,
            before: link.before,
            after: link.after,
        })
        .collect()
}

pub(crate) fn plan_inline(
    root: &Path,
    note_arg: &str,
) -> Result<InlinePlan, InlineRefusal> {
    let child_path =
        vault::resolve_note(root, note_arg).map_err(|message| {
            let code = if message.contains("ambiguous") {
                "note-ambiguous"
            } else {
                "note-not-found"
            };
            InlineRefusal::new(
                code,
                message,
                "Pass a vault-relative path or a unique note stem",
                None,
                None,
            )
        })?;
    if excluded_path(&child_path) {
        return Err(InlineRefusal::new(
            "excluded-directory",
            "prose note is in an excluded directory",
            "Move the note outside excluded, done, and reference folders",
            Some(&child_path),
            None,
        ));
    }
    let child_before =
        fs::read_to_string(root.join(&child_path)).map_err(|error| {
            InlineRefusal::new(
                "read-failed",
                format!(
                    "could not read {}: {error}",
                    display_relative(&child_path)
                ),
                "Check the vault path and retry",
                Some(&child_path),
                None,
            )
        })?;
    let child_kind = section_model::classify_host(&child_path, &child_before);
    if !matches!(child_kind, Ok(HostKind::Prose))
        || !parse_frontmatter(marker::strip_bom(&child_before))
            .is_some_and(|frontmatter| frontmatter_is_prose(&frontmatter))
    {
        return Err(InlineRefusal::new(
            "not-prose",
            "note is not a prose note",
            "Choose a note with type [[prose]] or legacy type [[article]]",
            Some(&child_path),
            None,
        ));
    }
    if let Some(line) = conflict_marker(&child_before) {
        return Err(InlineRefusal::new(
            "conflict-markers",
            "prose note contains Git conflict markers",
            "Resolve the conflict markers before inlining",
            Some(&child_path),
            Some(line),
        ));
    }

    let (parent_path, parent_before, parent_kind) =
        resolve_parent(root, &child_path, &child_before)?;
    let children = child_notes(root, &child_path)?;
    if !children.is_empty() {
        let list = children
            .iter()
            .take(5)
            .map(|path| display_relative(path))
            .collect::<Vec<_>>()
            .join(", ");
        let extra = if children.len() > 5 {
            format!(" and {} more", children.len() - 5)
        } else {
            String::new()
        };
        return Err(InlineRefusal::new(
            "has-children",
            format!("prose note has child notes: {list}{extra}"),
            "Inline or reparent its child notes first",
            Some(&child_path),
            None,
        ));
    }
    unknown_frontmatter(&child_before, &child_path)?;

    let candidates =
        section_stubs(&parent_path, &parent_before, &child_path, root)?;
    let [(section, stub)] = candidates.as_slice() else {
        let (code, message, hint) = if candidates.is_empty() {
            (
                "stub-missing",
                "parent has no section stub for this prose note".to_string(),
                "Add one section with a single link to this prose note",
            )
        } else {
            let lines = candidates
                .iter()
                .map(|(section, _)| section.heading_line.to_string())
                .collect::<Vec<_>>()
                .join(", ");
            (
                "stub-ambiguous",
                format!("parent has multiple section stubs for this prose note at lines {lines}"),
                "Keep one section stub for this prose note",
            )
        };
        return Err(InlineRefusal::new(
            code,
            message,
            hint,
            Some(&parent_path),
            candidates.first().map(|(section, _)| section.heading_line),
        ));
    };
    let section = section.clone();
    let stub = stub.clone();

    check_depth(&child_before, section.heading_level, &child_path)?;

    let child_doc = scan::classify(&child_before);
    let parent_doc = scan::classify(&parent_before);
    let child_blocks = section_model::collect_block_ids(&child_doc.lines);
    let parent_blocks = section_model::collect_block_ids(&parent_doc.lines)
        .into_iter()
        .filter(|block| !section.body_range.contains(&(block.line - 1)))
        .map(|block| block.id)
        .collect::<BTreeSet<_>>();
    if let Some(block) = child_blocks
        .iter()
        .find(|block| parent_blocks.contains(&block.id))
    {
        return Err(InlineRefusal::new(
            "block-id-collision",
            format!("block id ^{} already exists in the parent", block.id),
            "Rename one of the duplicate block ids before inlining",
            Some(&child_path),
            Some(block.line),
        )
        .at_section(&section));
    }
    let child_definitions = section_model::definition_index(&child_before);
    let parent_definitions = section_model::definition_index(&parent_before);
    for (label, lines) in &child_definitions.definitions {
        if parent_definitions.definitions.contains_key(label) {
            return Err(InlineRefusal::new(
                "definition-collision",
                format!("reference label {label:?} is defined in both notes"),
                "Rename one definition and its references before inlining",
                Some(&child_path),
                lines.first().copied(),
            )
            .at_section(&section));
        }
    }

    let child_stem = child_path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or_default()
        .to_string();
    let map =
        RelocationMap::for_inline_path(&section, &child_path, &child_before);
    let child_rewrite = map
        .rewrite_note(root, &child_path, &child_before, Direction::Inline, &[])
        .map_err(|error| relocation_refusal(error, &child_path, &section))?;
    let (body, replaced_count) = demoted_body(
        &child_rewrite.contents,
        section.heading_level,
        &child_path,
    )?;
    let parent_after_stub = splice_parent(&parent_before, &section, &body);
    let parent_rewrite = map
        .rewrite_note(
            root,
            &parent_path,
            &parent_after_stub,
            Direction::Inline,
            &[],
        )
        .map_err(|error| relocation_refusal(error, &parent_path, &section))?;
    let mut parent_after = parent_rewrite.contents;
    let mut layout_count = 0;
    let mut parent_replaced = 0;
    let mut findings = Vec::new();
    if matches!(parent_kind, HostKind::Prose) {
        let plan = format(&parent_after, &ProseContext::new(&parent_path));
        if matches!(
            plan.status,
            super::super::plan::ProseStatus::Skipped
                | super::super::plan::ProseStatus::Error
        ) {
            let (code, message) = plan
                .reason
                .as_ref()
                .map(|reason| (reason.code.clone(), reason.message.clone()))
                .unwrap_or_else(|| {
                    (
                        "words-changed".to_string(),
                        "parent failed validation".to_string(),
                    )
                });
            return Err(InlineRefusal::new(
                &code,
                message,
                "Repair the parent prose note before inlining",
                Some(&parent_path),
                Some(section.heading_line),
            )
            .at_section(&section));
        }
        layout_count = plan.changes.layout;
        parent_replaced = plan.replaced.len();
        findings = plan.findings;
        parent_after = plan.formatted.unwrap_or(parent_after);
    }

    if let Some(title) = frontmatter_title(&child_before)
        && heading_identity(&title) != section.identity
        && heading_plain_identity(&title)
            != heading_plain_identity(&section.raw_title)
    {
        findings.push(Finding {
            code: "title-dropped".to_string(),
            severity: Severity::Warning,
            line: section.heading_line,
            message: format!("prose title {title:?} differs from the parent section title"),
            hint: "The parent section heading stays in place; rename it by hand if needed".to_string(),
        });
    }
    findings.sort_by_key(|finding| finding.line);

    let vault_links =
        relocate::plan_vault(root, &map, Direction::Inline, Some(stub.line))
            .map_err(|error| {
                relocation_refusal(error, &parent_path, &section)
            })?;
    let mut linker_files = Vec::new();
    for file in &vault_links.files {
        if file.path == parent_path || file.path == child_path {
            continue;
        }
        let before =
            fs::read_to_string(root.join(&file.path)).map_err(|error| {
                InlineRefusal::new(
                    "read-failed",
                    format!(
                        "could not read {}: {error}",
                        display_relative(&file.path)
                    ),
                    "Check the vault and retry",
                    Some(&file.path),
                    None,
                )
            })?;
        linker_files.push((file.path.clone(), before, file.contents.clone()));
    }

    let mut links = links_from(&child_path, child_rewrite.links);
    links.extend(links_from(&parent_path, parent_rewrite.links));
    links.extend(
        vault_links
            .links
            .iter()
            .filter(|link| link.path != parent_path && link.path != child_path)
            .map(|link| ConversionLink {
                path: display_relative(&link.path),
                line: link.line,
                before: link.before.clone(),
                after: link.after.clone(),
            }),
    );
    links.sort_by(|left, right| {
        (&left.path, left.line, &left.before).cmp(&(
            &right.path,
            right.line,
            &right.before,
        ))
    });
    links.dedup_by(|left, right| {
        left.path == right.path
            && left.line == right.line
            && left.before == right.before
            && left.after == right.after
    });

    let mut touched = BTreeMap::new();
    touched.insert(display_relative(&parent_path), sha256_hex(&parent_before));
    touched.insert(display_relative(&child_path), sha256_hex(&child_before));
    for (path, before, _) in &linker_files {
        touched.insert(display_relative(path), sha256_hex(before));
    }
    let section_end = section.body_range.end;
    let fingerprint = fingerprint_for(
        &touched,
        section.heading_line,
        section_end,
        &child_stem,
    );
    let focus_line = find_heading_line(&parent_after, &section.identity)
        .unwrap_or(section.heading_line);
    Ok(InlinePlan {
        parent_path,
        parent_before: parent_before.clone(),
        parent_after,
        child_path,
        child_before,
        child_stem,
        linker_files,
        links,
        section_line: section.heading_line,
        section_end,
        section_level: section.heading_level,
        section_title: section.raw_title,
        fingerprint,
        layout_count,
        replaced_count: replaced_count + parent_replaced,
        findings,
        focus_line,
    })
}

fn relocation_refusal(
    error: RelocationError,
    fallback_path: &Path,
    section: &ProseSection<'_>,
) -> InlineRefusal {
    let (path, line) = match &error {
        RelocationError::AmbiguousLink { path, line, .. } => {
            (path.as_path(), Some(*line))
        }
        RelocationError::ReadFile { path, .. } => (path.as_path(), None),
        RelocationError::Vault(_) => {
            (fallback_path, Some(section.heading_line))
        }
    };
    InlineRefusal::new(
        error.code(),
        error.to_string(),
        "Disambiguate the fragment so it names one heading",
        Some(path),
        line,
    )
    .at_section(section)
}

fn find_heading_line(text: &str, identity: &str) -> Option<usize> {
    section_model::all_sections(Path::new("note.md"), text)
        .ok()?
        .into_iter()
        .find(|section| section.identity == identity)
        .map(|section| section.heading_line)
}

fn report_for_preview(plan: &InlinePlan, note_arg: &str) -> ConversionReport {
    let parent_rel = display_relative(&plan.parent_path);
    let child_rel = display_relative(&plan.child_path);
    let mut changes = vec![ConversionChange {
        path: parent_rel.clone(),
        action: "update".to_string(),
        sha256: Some(sha256_hex(&plan.parent_before)),
        diff: unified_diff(
            &parent_rel,
            &plan.parent_before,
            &plan.parent_after,
        ),
    }];
    changes.push(ConversionChange {
        path: child_rel.clone(),
        action: "trash".to_string(),
        sha256: Some(sha256_hex(&plan.child_before)),
        diff: unified_diff(&child_rel, &plan.child_before, ""),
    });
    for (path, before, after) in &plan.linker_files {
        let rel = display_relative(path);
        changes.push(ConversionChange {
            path: rel.clone(),
            action: "update".to_string(),
            sha256: Some(sha256_hex(before)),
            diff: unified_diff(&rel, before, after),
        });
    }
    changes.sort_by(|left, right| left.path.cmp(&right.path));
    let parent_stem = plan
        .parent_path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or("note");
    let notice = format!(
        "Inlined “{}” into {} § {} · {} {} repaired",
        plan.child_stem,
        parent_stem,
        plan.section_title,
        plan.links.len(),
        if plan.links.len() == 1 {
            "link"
        } else {
            "links"
        },
    );
    ConversionReport {
        schema_version: 1,
        contract: "prose-conversion".to_string(),
        command: "inline".to_string(),
        status: "preview".to_string(),
        fingerprint: plan.fingerprint.clone(),
        note: note_arg.to_string(),
        section: Some(ConversionSection {
            line: plan.section_line,
            end_line: plan.section_end,
            level: plan.section_level,
            title: plan.section_title.clone(),
        }),
        target: Some(ConversionTarget {
            path: display_relative(&plan.parent_path),
            stem: parent_stem.to_string(),
        }),
        changes,
        links: plan.links.clone(),
        counts: ConversionCounts {
            notes: 2 + plan.linker_files.len(),
            links: plan.links.len(),
            layout: plan.layout_count,
            replaced: plan.replaced_count,
        },
        findings: plan.findings.clone(),
        refusal: None,
        focus: Some(ConversionFocus {
            path: display_relative(&plan.parent_path),
            line: plan.focus_line,
        }),
        notice,
        commit: None,
        sync: ConversionSync {
            pre: "skipped".to_string(),
            post: "skipped".to_string(),
        },
        warnings: Vec::new(),
    }
}

fn report_for_refusal(
    refusal: &InlineRefusal,
    note_arg: &str,
) -> ConversionReport {
    ConversionReport {
        schema_version: 1,
        contract: "prose-conversion".to_string(),
        command: "inline".to_string(),
        status: "refused".to_string(),
        fingerprint: empty_fingerprint(),
        note: note_arg.to_string(),
        section: refusal.section.clone(),
        target: refusal.target.clone(),
        changes: Vec::new(),
        links: Vec::new(),
        counts: ConversionCounts {
            notes: 0,
            links: 0,
            layout: 0,
            replaced: 0,
        },
        findings: Vec::new(),
        refusal: Some(ConversionRefusal {
            code: refusal.code.clone(),
            message: refusal.message.clone(),
            hint: refusal.hint.clone(),
            path: refusal.path.as_ref().map(|path| display_relative(path)),
            line: refusal.line,
            next: None,
        }),
        focus: None,
        notice: refusal.message.clone(),
        commit: None,
        sync: ConversionSync {
            pre: "skipped".to_string(),
            post: "skipped".to_string(),
        },
        warnings: Vec::new(),
    }
}

fn print_refusal(
    report: &ConversionReport,
    refusal: &InlineRefusal,
    format: extract::OutputFormat,
) -> i32 {
    match format {
        extract::OutputFormat::Json => {
            println!("{}", serde_json::to_string(report).unwrap_or_default());
        }
        extract::OutputFormat::Human => {
            eprintln!("bob prose inline: {}", refusal.message);
            eprintln!("hint: {}", refusal.hint);
        }
    }
    2
}

fn verify_written(root: &Path, plan: &InlinePlan) -> Result<(), String> {
    let parent =
        fs::read_to_string(root.join(&plan.parent_path)).map_err(|error| {
            format!("could not read parent after inline: {error}")
        })?;
    if parent != plan.parent_after {
        return Err(
            "parent after-image differs from the inline plan".to_string()
        );
    }
    if root.join(&plan.child_path).exists() {
        return Err(
            "inlined prose note still exists at its original path".to_string()
        );
    }
    if parse_frontmatter(marker::strip_bom(&parent))
        .is_some_and(|frontmatter| frontmatter_is_prose(&frontmatter))
    {
        let formatted = format(&parent, &ProseContext::new(&plan.parent_path));
        if matches!(
            formatted.status,
            super::super::plan::ProseStatus::Skipped
                | super::super::plan::ProseStatus::Error
        ) || formatted
            .formatted
            .as_deref()
            .is_some_and(|after| after != parent)
        {
            return Err(
                "parent prose note is not idempotent after inline".to_string()
            );
        }
    }
    Ok(())
}

#[cfg(test)]
thread_local! {
    static FAIL_NEXT_VERIFY: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

#[cfg(test)]
fn fail_next_verify() -> bool {
    FAIL_NEXT_VERIFY.with(|slot| slot.replace(false))
}

#[cfg(test)]
fn set_fail_next_verify(fail: bool) {
    FAIL_NEXT_VERIFY.with(|slot| slot.set(fail));
}

pub(crate) fn run_inline(matches: &clap::ArgMatches) -> i32 {
    let root = matches
        .get_one::<OsString>("bob-dir")
        .map(PathBuf::from)
        .map(|path| bob_env::expand_tilde(&path))
        .unwrap_or_else(bob_env::bob_dir);
    let note_arg = matches
        .get_one::<String>("note")
        .map(String::as_str)
        .unwrap_or("");
    let write = matches.get_flag("write");
    let expect = matches.get_one::<String>("expect").map(String::as_str);
    let offline = matches.get_flag("offline");
    let output_format = extract::output_format_from_matches(matches);
    let json_mode = output_format == extract::OutputFormat::Json;
    let child_env = ob::child_env();
    let recovery_warning_before_plan = if write {
        match vault_rewrite::recover_before_planning(
            &root,
            &child_env,
            "bob prose inline",
            offline,
            json_mode,
        ) {
            Ok(warning) => warning,
            Err(failure) => {
                let refusal = InlineRefusal::new(
                    failure.code,
                    failure.message,
                    failure
                        .hint
                        .unwrap_or_else(|| "Retry the inline".to_string()),
                    None,
                    None,
                );
                let report = report_for_refusal(&refusal, note_arg);
                return print_refusal(&report, &refusal, output_format);
            }
        }
    } else {
        None
    };
    let plan = match plan_inline(&root, note_arg) {
        Ok(plan) => plan,
        Err(refusal) => {
            let report = report_for_refusal(&refusal, note_arg);
            return print_refusal(&report, &refusal, output_format);
        }
    };
    if let Some(expected) = expect
        && expected != plan.fingerprint
    {
        let refusal = InlineRefusal::new(
            "plan-changed",
            "the vault changed since the preview; re-preview before writing",
            "Re-run without `-w` to preview the new plan",
            Some(&plan.child_path),
            Some(plan.section_line),
        );
        let report = report_for_refusal(&refusal, note_arg);
        return print_refusal(&report, &refusal, output_format);
    }
    if !write {
        let report = report_for_preview(&plan, note_arg);
        extract::print_preview(&report, output_format);
        return 0;
    }

    let note_owned = note_arg.to_string();
    let expected_owned = expect.map(str::to_string);
    let outcome = vault_rewrite::run(
        &root,
        &child_env,
        "bob prose inline",
        "prose section inlinings",
        offline,
        json_mode,
        || {
            let fresh = plan_inline(&root, &note_owned).map_err(|refusal| {
                vault_rewrite::TxnFailure::new(
                    Box::leak(refusal.code.clone().into_boxed_str()) as &str,
                    refusal.message.clone(),
                    Some(Box::leak(refusal.hint.clone().into_boxed_str())
                        as &str),
                )
            })?;
            if let Some(expected) = expected_owned.as_deref()
                && expected != fresh.fingerprint
            {
                return Err(vault_rewrite::TxnFailure::new(
                    "plan-changed",
                    "the vault changed since the preview; re-preview before writing",
                    Some("re-run without `-w` to preview the new plan"),
                ));
            }
            let parent_stem = fresh
                .parent_path
                .file_stem()
                .and_then(|stem| stem.to_str())
                .unwrap_or("note");
            let subject = format!(
                "bob prose inline: {} → {} § {}",
                fresh.child_stem, parent_stem, fresh.section_title
            );
            let mut files = vec![vault_rewrite::FileRewrite {
                path: display_relative(&fresh.parent_path),
                before_contents: Some(fresh.parent_before.clone()),
                after_contents: fresh.parent_after.clone(),
                trash: false,
            }];
            files.push(vault_rewrite::FileRewrite {
                path: display_relative(&fresh.child_path),
                before_contents: Some(fresh.child_before.clone()),
                after_contents: String::new(),
                trash: true,
            });
            for (path, before, after) in &fresh.linker_files {
                files.push(vault_rewrite::FileRewrite {
                    path: display_relative(path),
                    before_contents: Some(before.clone()),
                    after_contents: after.clone(),
                    trash: false,
                });
            }
            let verify_root = root.clone();
            let verify_plan = fresh.clone();
            Ok(vault_rewrite::ReadyPlan {
                files,
                subject,
                post_write: Box::new(|| Ok(Vec::new())),
                verify: Box::new(move || {
                    #[cfg(test)]
                    if fail_next_verify() {
                        return Err(
                            "injected inline verification failure".to_string()
                        );
                    }
                    verify_written(&verify_root, &verify_plan)
                }),
                on_committed: Box::new(|| {}),
                context: fresh,
            })
        },
    );
    match outcome {
        Ok(outcome) => {
            let mut report = report_for_preview(&outcome.context, note_arg);
            report.status = "written".to_string();
            report.commit = Some(ConversionCommit {
                sha: outcome.sha,
                subject: outcome.subject,
            });
            report.sync = ConversionSync {
                pre: outcome.sync_pre.to_string(),
                post: outcome.sync_post.to_string(),
            };
            if let Some(warning) = outcome.dirty_warning {
                report.warnings.push(warning);
            }
            if let Some(warning) = outcome.post_sync_warning {
                report.warnings.push(warning);
            }
            if let Some(warning) = outcome.recovery_warning {
                report.warnings.push(warning);
            }
            if let Some(warning) = recovery_warning_before_plan {
                report.warnings.push(warning);
            }
            extract::print_written(&report, output_format);
            0
        }
        Err(failure) => {
            let refusal = InlineRefusal::new(
                failure.code,
                failure.message,
                failure
                    .hint
                    .unwrap_or_else(|| "Retry the inline".to_string()),
                None,
                None,
            );
            let report = report_for_refusal(&refusal, note_arg);
            match output_format {
                extract::OutputFormat::Json => {
                    println!(
                        "{}",
                        serde_json::to_string(&report).unwrap_or_default()
                    );
                }
                extract::OutputFormat::Human => {
                    eprintln!("bob prose inline: {}", refusal.message);
                    eprintln!("hint: {}", refusal.hint);
                }
            }
            2
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;
    use std::process::Command;

    fn write_vault(parent: &str, child: &str) -> tempfile::TempDir {
        let root = tempfile::tempdir().expect("temporary vault");
        fs::create_dir_all(root.path().join("notes")).expect("notes folder");
        fs::write(root.path().join("area.md"), parent).expect("parent note");
        fs::write(root.path().join("notes/draft.md"), child)
            .expect("child note");
        root
    }

    fn parent_with_stub(stub: &str) -> String {
        format!(
            "---\ntype: \"[[area]]\"\n---\n## Topic\n\n{stub}\n\n## Other\n\nKept prose.\n"
        )
    }

    fn child_note(extra_frontmatter: &str, body: &str) -> String {
        format!(
            "---\nparent: \"[[area]]\"\ntype: \"[[prose]]\"\ncreated: 2026-11-27\ntitle: Topic\nsection_numbers: false\n{extra_frontmatter}---\n\n{body}"
        )
    }

    fn run_inline_cli(root: &Path) -> i32 {
        let command = crate::native::prose::cli::build_cli();
        let matches = command
            .try_get_matches_from(vec![
                OsString::from("bob prose"),
                OsString::from("inline"),
                OsString::from("notes/draft"),
                OsString::from("-b"),
                OsString::from(root.to_string_lossy().as_ref()),
                OsString::from("-w"),
                OsString::from("-o"),
                OsString::from("-f"),
                OsString::from("json"),
            ])
            .expect("inline CLI args");
        run_inline(matches.subcommand_matches("inline").expect("inline args"))
    }

    fn git(root: &Path, args: &[&str]) -> String {
        let output = Command::new("git")
            .arg("-C")
            .arg(root)
            .args(args)
            .output()
            .expect("run git");
        assert!(output.status.success(), "git {args:?}: {:?}", output);
        String::from_utf8_lossy(&output.stdout).trim().to_string()
    }

    fn seed_git(root: &Path) -> String {
        git(root, &["init", "-q"]);
        git(root, &["config", "user.email", "t@t"]);
        git(root, &["config", "user.name", "t"]);
        git(root, &["add", "-A"]);
        git(root, &["commit", "-qm", "seed"]);
        git(root, &["rev-parse", "HEAD"])
    }

    #[test]
    fn inline_restores_section_and_repairs_external_links() {
        let parent = parent_with_stub("- [[notes/draft|Topic]]");
        let child = child_note(
            "extracted_from: \"[[area]]\"\nextracted_heading: Topic\n",
            "## Idea\n\nChild prose [[#Idea]]. ^moved\n",
        );
        let root = write_vault(&parent, &child);
        fs::create_dir_all(root.path().join("done")).expect("done folder");
        fs::write(
            root.path().join("done/day.md"),
            "[[notes/draft#Idea|Read]]\n[[area#^moved]]\n",
        )
        .expect("external linker");

        let plan =
            plan_inline(root.path(), "notes/draft").expect("inline plan");
        assert!(
            plan.parent_after.contains("### Idea"),
            "{}",
            plan.parent_after
        );
        assert!(plan.parent_after.contains("Child prose [[#Idea]]"));
        assert_eq!(plan.layout_count, 0);
        let external = plan
            .linker_files
            .iter()
            .find(|(path, _, _)| path == Path::new("done/day.md"))
            .expect("third-party linker planned")
            .2
            .as_str();
        assert!(external.contains("[[area#Idea|Read]]"), "{external}");
        assert!(external.contains("[[area#^moved]]"), "{external}");
    }

    #[test]
    fn root_prose_inlines_into_a_handwritten_embed_stub() {
        let parent = parent_with_stub("- ![[notes/draft]]");
        let child = child_note("", "A bullet-only note.\n");
        let root = write_vault(&parent, &child);
        let plan =
            plan_inline(root.path(), "notes/draft").expect("inline plan");
        assert!(plan.parent_after.contains("A bullet-only note."));
        assert!(!plan.parent_after.contains("![[notes/draft]]"));
    }

    #[test]
    fn inline_refuses_child_notes_and_reports_dropped_title() {
        let root = write_vault(
            &parent_with_stub("- [[notes/draft|Topic]]"),
            &child_note("", "## Detail\n\nRestored prose.\n"),
        );
        for index in 0..6 {
            fs::write(
                root.path().join(format!("notes/child-{index}.md")),
                "---\nparent: \"[[notes/draft]]\"\n---\n\nChild.\n",
            )
            .expect("child note");
        }
        let refusal = plan_inline(root.path(), "notes/draft")
            .expect_err("children prevent inline");
        assert_eq!(refusal.code, "has-children");
        assert!(
            refusal.message.contains("child-0.md"),
            "{}",
            refusal.message
        );
        assert!(
            refusal.message.contains("and 1 more"),
            "{}",
            refusal.message
        );
        assert!(
            !refusal.message.contains("child-5.md"),
            "{}",
            refusal.message
        );

        let root = write_vault(
            &parent_with_stub("- [[notes/draft|Topic]]"),
            &child_note("", "## Detail\n\nRestored prose.\n")
                .replace("title: Topic", "title: Different title"),
        );
        let plan = plan_inline(root.path(), "notes/draft")
            .expect("inline with title warning");
        let warning = plan
            .findings
            .iter()
            .find(|finding| finding.code == "title-dropped")
            .expect("title-dropped finding");
        assert_eq!(warning.severity, Severity::Warning);
    }

    #[test]
    fn inline_verify_failure_rolls_back_parent_and_restores_trashed_note() {
        let parent = parent_with_stub("- [[notes/draft|Topic]]");
        let child = child_note("", "## Detail\n\nRestored prose.\n");
        let root = write_vault(&parent, &child);
        let initial_head = seed_git(root.path());
        set_fail_next_verify(true);

        assert_eq!(run_inline_cli(root.path()), 2);
        assert_eq!(
            fs::read_to_string(root.path().join("area.md")).unwrap(),
            parent
        );
        assert_eq!(
            fs::read_to_string(root.path().join("notes/draft.md")).unwrap(),
            child
        );
        assert!(!root.path().join(".trash/notes/draft.md").exists());
        assert_eq!(git(root.path(), &["rev-parse", "HEAD"]), initial_head);
    }

    #[test]
    fn inline_write_recovers_after_an_injected_crash() {
        let parent = parent_with_stub("- [[notes/draft|Topic]]");
        let child = child_note("", "## Detail\n\nRestored prose.\n");
        let root = write_vault(&parent, &child);
        seed_git(root.path());
        crate::native::vault_rewrite::set_crash_before_write(Some(1));

        assert_eq!(run_inline_cli(root.path()), 2);
        assert!(
            fs::read_to_string(root.path().join("area.md")).unwrap() != parent,
            "the injected crash happens after the parent after-image is installed"
        );
        assert!(root.path().join("notes/draft.md").exists());

        crate::native::vault_rewrite::set_crash_before_write(None);
        assert_eq!(run_inline_cli(root.path()), 0);
        let restored = fs::read_to_string(root.path().join("area.md")).unwrap();
        assert!(restored.contains("### Detail"), "{restored}");
        assert!(restored.contains("Restored prose."), "{restored}");
        assert!(!root.path().join("notes/draft.md").exists());
        assert!(root.path().join(".trash").exists());
    }

    #[test]
    fn inline_into_numbered_prose_parent_recalculates_heading_numbers() {
        let parent = concat!(
            "---\n",
            "type: \"[[prose]]\"\n",
            "section_numbers: true\n",
            "---\n\n",
            "# 1. Parent\n\n",
            "## 1.1 Topic\n\n",
            "- [[notes/draft|Topic]]\n\n",
            "## 1.2 Other\n\n",
            "Keep this text.\n",
        );
        let child = child_note("", "## 9. Detail\n\nRestored prose.\n")
            .replace("section_numbers: false", "section_numbers: true");
        let root = write_vault(parent, &child);

        let plan = plan_inline(root.path(), "notes/draft")
            .expect("inline into numbered prose parent");

        assert!(
            plan.parent_after.contains("### 1.1.1 Detail"),
            "{}",
            plan.parent_after
        );
        assert!(
            !plan.parent_after.contains("9. Detail"),
            "{}",
            plan.parent_after
        );
        assert!(plan.parent_after.contains("section_numbers: true"));
    }

    #[test]
    fn inline_refuses_the_contract_ineligible_cases() {
        let valid_parent = parent_with_stub("- [[notes/draft|Topic]]");
        let valid_child = child_note("", "Prose.\n");

        let root = write_vault(
            &valid_parent,
            &valid_child.replace("[[prose]]", "[[area]]"),
        );
        assert_eq!(
            plan_inline(root.path(), "notes/draft").unwrap_err().code,
            "not-prose"
        );

        let root = write_vault(
            &valid_parent,
            &valid_child.replace("parent: \"[[area]]\"\n", ""),
        );
        assert_eq!(
            plan_inline(root.path(), "notes/draft").unwrap_err().code,
            "no-parent"
        );

        let nonhost =
            "---\ntype: \"[[notebook]]\"\n---\n## Topic\n\n- [[notes/draft]]\n";
        let root = write_vault(nonhost, &valid_child);
        assert_eq!(
            plan_inline(root.path(), "notes/draft").unwrap_err().code,
            "parent-not-host"
        );

        let root = write_vault(
            &valid_parent,
            &valid_child.replace(
                "section_numbers: false\n",
                "section_numbers: false\nsummary: Keep me\n",
            ),
        );
        assert_eq!(
            plan_inline(root.path(), "notes/draft").unwrap_err().code,
            "unknown-frontmatter"
        );

        let root = write_vault(
            &parent_with_stub("- [[somewhere_else]]"),
            &valid_child,
        );
        assert_eq!(
            plan_inline(root.path(), "notes/draft").unwrap_err().code,
            "stub-missing"
        );

        let duplicate_stubs = "---\ntype: \"[[area]]\"\n---\n## Topic\n\n- [[notes/draft]]\n\n## Other\n\n- [[notes/draft]]\n".to_string();
        let root = write_vault(&duplicate_stubs, &valid_child);
        assert_eq!(
            plan_inline(root.path(), "notes/draft").unwrap_err().code,
            "stub-ambiguous"
        );

        let child = child_note("", "###### Too deep\n\nText.\n");
        let root = write_vault(&valid_parent, &child);
        assert_eq!(
            plan_inline(root.path(), "notes/draft").unwrap_err().code,
            "depth-overflow"
        );

        let parent = format!(
            "{}\n## Kept\n\nExisting. ^same\n",
            parent_with_stub("- [[notes/draft]]")
        );
        let child = child_note("", "Text. ^same\n");
        let root = write_vault(&parent, &child);
        assert_eq!(
            plan_inline(root.path(), "notes/draft").unwrap_err().code,
            "block-id-collision"
        );

        let parent = format!(
            "{}\n[ref]: /parent\n",
            parent_with_stub("- [[notes/draft]]")
        );
        let child = child_note("", "[ref]: /child\n\nText.\n");
        let root = write_vault(&parent, &child);
        assert_eq!(
            plan_inline(root.path(), "notes/draft").unwrap_err().code,
            "definition-collision"
        );

        let child = child_note(
            "",
            "<<<<<<< ours\nconflict\n=======\nother\n>>>>>>> theirs\n",
        );
        let root = write_vault(&valid_parent, &child);
        assert_eq!(
            plan_inline(root.path(), "notes/draft").unwrap_err().code,
            "conflict-markers"
        );
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(24))]

        #[test]
        fn extract_then_inline_preserves_eligible_sections(
            host_kind in 0u8..3,
            nested in any::<bool>(),
            shape in 0u8..4,
            embed_stub in any::<bool>(),
            layout_dirty in any::<bool>(),
        ) {
            let root = tempfile::tempdir().expect("temporary vault");
            fs::create_dir_all(root.path().join("done")).expect("done folder");
            fs::create_dir_all(root.path().join("2026/daily")).expect("daily folder");
            let (type_line, host_is_prose) = match host_kind {
                0 => ("type: \"[[area]]\"\n", false),
                1 => ("type: \"[[project]]\"\n", false),
                _ => ("type: \"[[prose]]\"\nsection_numbers: false\n", true),
            };
            let (prefix, heading_level, following_level) = if nested {
                ("## Container\n\n", 3usize, 3usize)
            } else {
                ("", 2usize, 2usize)
            };
            let heading = format!("{} Topic", "#".repeat(heading_level));
            let detail_heading = format!("{} Detail", "#".repeat(heading_level + 1));
            let shape_text = match shape {
                0 => "\n\n- first item\n- second item\n",
                1 => "\n\n> quoted prose\n",
                2 => "\n\n```text\ncode sample\n```\n",
                _ => "\n\n| left | right |\n| --- | --- |\n| one | two |\n",
            };
            let dirty_text = if layout_dirty && host_kind < 2 {
                "\n\nTrailing spaces.  "
            } else {
                ""
            };
            let body = format!(
                "{detail_heading}\n\nDetail text [[#Detail]] [[#Outside]]. ^moved{shape_text}{dirty_text}"
            );
            let following = format!("{} Following", "#".repeat(following_level));
            let original = format!(
                "---\n{type_line}---\n\n{prefix}{heading}\n\n{body}\n{following}\n\nKeep this text [[#Detail]].\n\n## Outside\n\nOutside prose.\n"
            );
            let original = if host_is_prose {
                let formatted = format(
                    &original,
                    &ProseContext::new(Path::new("owner.md")),
                );
                prop_assert!(!matches!(
                    formatted.status,
                    super::super::super::plan::ProseStatus::Skipped
                        | super::super::super::plan::ProseStatus::Error
                ));
                formatted.formatted.expect("formatted prose host")
            } else {
                original
            };
            fs::create_dir_all(root.path().join("areas")).expect("areas folder");
            fs::write(root.path().join("areas/owner.md"), &original).expect("host");
            fs::write(
                root.path().join("done/day.md"),
                "[[areas/owner#Detail|Read]]\n[[../areas/owner#Detail]]\n[[areas/owner#^moved]]\n",
            )
            .expect("third-party linker");
            fs::write(
                root.path().join("2026/daily/day.md"),
                "[[areas/owner#Detail]]\n[[../../areas/owner#Detail]]\n[Read](../../areas/owner.md#Detail)\n",
            )
            .expect("daily linker");

            let extracted = extract::plan_extract(
                root.path(),
                "areas/owner",
                Some("Topic"),
                None,
                None,
            )
            .expect("eligible extraction plan");
            let report = extract::report_for_preview(&extracted, "owner");
            prop_assert_eq!(report.counts.layout, extracted.layout_count);
            if layout_dirty && host_kind < 2 {
                prop_assert!(report.counts.layout > 0);
            } else {
                prop_assert_eq!(report.counts.layout, 0);
            }

            let link_stub = format!("- [[{}|Topic]]", extracted.new_stem);
            let embed = format!("- ![[{}]]", extracted.new_stem);
            let mut host_after = extracted.host_after.clone();
            if embed_stub {
                prop_assert!(host_after.contains(&link_stub));
                host_after = host_after.replace(&link_stub, &embed);
            }
            fs::write(root.path().join("areas/owner.md"), host_after).expect("stubbed host");
            fs::write(root.path().join(&extracted.new_path), &extracted.new_contents)
                .expect("extracted prose note");
            for (path, _, after) in &extracted.linker_files {
                fs::write(root.path().join(path), after).expect("rewritten linker");
            }

            let inlined = plan_inline(
                root.path(),
                extracted.new_path.to_str().expect("UTF-8 child path"),
            )
            .expect("inline plan");
            prop_assert_eq!(inlined.parent_after, original);
            let third_party = inlined
                .linker_files
                .iter()
                .find(|(path, _, _)| path == Path::new("done/day.md"))
                .expect("third-party linker is restored")
                .2
                .clone();
            prop_assert!(third_party.contains("[[areas/owner#Detail|Read]]"), "{third_party}");
            prop_assert!(third_party.contains("[[../areas/owner#Detail]]"), "{third_party}");
            prop_assert!(third_party.contains("[[areas/owner#^moved]]"), "{third_party}");
            let daily = inlined
                .linker_files
                .iter()
                .find(|(path, _, _)| path == Path::new("2026/daily/day.md"))
                .expect("daily linker is restored")
                .2
                .clone();
            prop_assert!(daily.contains("[[areas/owner#Detail]]"), "{daily}");
            prop_assert!(daily.contains("[[../../areas/owner#Detail]]"), "{daily}");
            prop_assert!(daily.contains("[Read](../../areas/owner.md#Detail)"), "{daily}");
        }
    }
}
