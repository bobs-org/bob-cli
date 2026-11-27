//! Extract planner and hidden `bob prose extract`.
//!
//! Implements X1–X7 on top of `section_model` and `relocate`: the
//! ProseConversion v1 JSON types, the fingerprint (W2), and a hidden CLI
//! that writes through `vault_rewrite`.

use std::{
    collections::{BTreeMap, BTreeSet},
    ffi::OsString,
    fs,
    path::{Component, Path, PathBuf},
};

use serde::Serialize;

use super::super::{
    format,
    plan::sha256_hex,
    relocate::{self, Direction, RelocationMap},
    scan, section_model,
    sections::strip_section_number,
    vault, ProseContext,
};
use crate::native::{
    capture_project_note::format_created_timestamp,
    env as bob_env,
    notebooks::scan::link_destination,
    ob,
    projects::{
        frontmatter_is_prose, frontmatter_value, parse_frontmatter,
        wikilink_path_target,
    },
    vault_links::VaultLinkResolver,
    vault_rewrite,
    yaml_spans::yaml_string_scalar,
};

const CONTRACT: &str = "prose-conversion";
const SCHEMA_VERSION: u32 = 1;
const STUB_FORM: &str = "link";

#[derive(Debug, Clone, Serialize)]
pub(crate) struct ConversionSection {
    pub(crate) line: usize,
    pub(crate) end_line: usize,
    pub(crate) level: usize,
    pub(crate) title: String,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct ConversionTarget {
    pub(crate) path: String,
    pub(crate) stem: String,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct ConversionChange {
    pub(crate) path: String,
    pub(crate) action: String,
    pub(crate) sha256: Option<String>,
    pub(crate) diff: String,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct ConversionLink {
    pub(crate) path: String,
    pub(crate) line: usize,
    pub(crate) before: String,
    pub(crate) after: String,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct ConversionCounts {
    pub(crate) notes: usize,
    pub(crate) links: usize,
    pub(crate) layout: usize,
    pub(crate) replaced: usize,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct ConversionFocus {
    pub(crate) path: String,
    pub(crate) line: usize,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct ConversionNext {
    pub(crate) command: String,
    pub(crate) note: String,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct ConversionRefusal {
    pub(crate) code: String,
    pub(crate) message: String,
    pub(crate) hint: String,
    pub(crate) path: Option<String>,
    pub(crate) line: Option<usize>,
    pub(crate) next: Option<ConversionNext>,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct ConversionCommit {
    pub(crate) sha: String,
    pub(crate) subject: String,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct ConversionSync {
    pub(crate) pre: String,
    pub(crate) post: String,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct ConversionReport {
    pub(crate) schema_version: u32,
    pub(crate) contract: String,
    pub(crate) command: String,
    pub(crate) status: String,
    pub(crate) fingerprint: String,
    pub(crate) note: String,
    pub(crate) section: Option<ConversionSection>,
    pub(crate) target: Option<ConversionTarget>,
    pub(crate) changes: Vec<ConversionChange>,
    pub(crate) links: Vec<ConversionLink>,
    pub(crate) counts: ConversionCounts,
    pub(crate) findings: Vec<super::super::findings::Finding>,
    pub(crate) refusal: Option<ConversionRefusal>,
    pub(crate) focus: Option<ConversionFocus>,
    pub(crate) notice: String,
    pub(crate) commit: Option<ConversionCommit>,
    pub(crate) sync: ConversionSync,
    pub(crate) warnings: Vec<String>,
}

#[derive(Debug, Clone)]
pub(crate) struct ExtractPlan {
    pub(crate) host_path: PathBuf,
    pub(crate) host_before: String,
    pub(crate) host_after: String,
    pub(crate) new_path: PathBuf,
    pub(crate) new_stem: String,
    pub(crate) new_contents: String,
    pub(crate) linker_files: Vec<(PathBuf, String, String)>,
    pub(crate) links: Vec<ConversionLink>,
    pub(crate) section_line: usize,
    pub(crate) section_end: usize,
    pub(crate) section_level: usize,
    pub(crate) section_title: String,
    pub(crate) fingerprint: String,
    pub(crate) layout_count: usize,
    pub(crate) replaced_count: usize,
    pub(crate) findings: Vec<super::super::findings::Finding>,
    pub(crate) focus_line: usize,
}

#[derive(Debug, Clone)]
pub(crate) struct ExtractRefusal {
    pub(crate) code: String,
    pub(crate) message: String,
    pub(crate) hint: String,
    pub(crate) path: Option<PathBuf>,
    pub(crate) line: Option<usize>,
    pub(crate) next: Option<(String, PathBuf)>,
    pub(crate) section: Option<ConversionSection>,
    pub(crate) target: Option<ConversionTarget>,
}

impl ExtractRefusal {
    fn from_section(
        refusal: &section_model::SectionRefusal,
        host_path: &Path,
    ) -> Self {
        Self {
            code: refusal.code.clone(),
            message: refusal.message.clone(),
            hint: refusal.hint.clone(),
            path: Some(host_path.to_path_buf()),
            line: refusal.line,
            next: refusal
                .next
                .as_ref()
                .map(|next| (next.command.to_string(), next.note.clone())),
            section: None,
            target: None,
        }
    }
}

fn display_relative(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

fn is_prose_note_text(text: &str) -> bool {
    let Some(frontmatter) = parse_frontmatter(text) else {
        return false;
    };
    frontmatter_is_prose(&frontmatter)
}

fn parent_link_target(text: &str) -> Option<String> {
    let frontmatter = parse_frontmatter(text)?;
    let raw = frontmatter_value(&frontmatter, "parent")?;
    wikilink_path_target(raw)
}

fn resolve_child_stub(
    root: &Path,
    host_path: &Path,
    stub: &section_model::StubTarget,
) -> Option<PathBuf> {
    let resolver = VaultLinkResolver::new(root);
    let target = stub.target.trim();
    if target.is_empty() {
        return None;
    }
    let resolved = match resolver.resolve(host_path, target) {
        crate::native::vault_links::LinkResolution::Found(path) => path,
        _ => return None,
    };
    let text = fs::read_to_string(root.join(&resolved)).ok()?;
    if !is_prose_note_text(&text) {
        return None;
    }
    let parent = parent_link_target(&text)?;
    let parent_path = match resolver.resolve(&resolved, &parent) {
        crate::native::vault_links::LinkResolution::Found(path) => path,
        _ => return None,
    };
    (parent_path == host_path).then_some(resolved)
}

fn host_prose_parent(
    root: &Path,
    host_path: &Path,
    host_text: &str,
) -> Option<PathBuf> {
    if !is_prose_note_text(host_text) {
        return None;
    }
    let parent = parent_link_target(host_text)?;
    let resolver = VaultLinkResolver::new(root);
    match resolver.resolve(host_path, &parent) {
        crate::native::vault_links::LinkResolution::Found(path) => {
            let parent_text = fs::read_to_string(root.join(&path)).ok()?;
            parse_frontmatter(&parent_text)?;
            Some(path)
        }
        _ => None,
    }
}

fn derive_slug(raw_title: &str) -> String {
    let denumbered = strip_section_number(raw_title).rest;
    let lower = denumbered.to_lowercase();
    let mut slug = String::new();
    let mut in_gap = false;
    for ch in lower.chars() {
        if matches!(ch, 'a'..='z' | '0'..='9') {
            if in_gap && !slug.is_empty() {
                slug.push('_');
            }
            slug.push(ch);
            in_gap = false;
        } else if !slug.is_empty() || !in_gap {
            in_gap = true;
        } else {
            in_gap = true;
        }
    }
    let trimmed = slug.trim_matches('_').to_string();
    if trimmed.len() > 60 {
        trimmed[..60].trim_matches('_').to_string()
    } else {
        trimmed
    }
}

fn validate_stem(stem: &str) -> bool {
    if stem.is_empty() || stem.len() > 200 {
        return false;
    }
    if stem == "." || stem == ".." {
        return false;
    }
    if stem.contains('/') || stem.contains('\\') || stem.contains('\0') {
        return false;
    }
    if stem.chars().any(|ch| ch.is_control()) {
        return false;
    }
    if stem.ends_with('.') || stem.ends_with(' ') {
        return false;
    }
    true
}

fn case_insensitive_exists(root: &Path, new_path: &Path) -> bool {
    if root.join(new_path).exists() {
        return true;
    }
    let Ok(markdown) = vault::list_markdown(root) else {
        return false;
    };
    let needle = display_relative(new_path).to_lowercase();
    markdown
        .iter()
        .any(|path| display_relative(path).to_lowercase() == needle)
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
    canonical.push_str(&format!("stub:{STUB_FORM}\n"));
    format!("sha256:{}", sha256_hex(&canonical))
}

fn unified_diff(path: &str, before: &str, after: &str) -> String {
    let diff = similar::TextDiff::from_lines(before, after);
    let mut out = String::new();
    out.push_str(&format!("--- a/{path}\n"));
    out.push_str(&format!("+++ b/{path}\n"));
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

fn stub_bullet(stem: &str, title: &str) -> String {
    if title.contains('|') || title.contains("[[") || title.contains("]]") {
        format!("- [[{stem}]]")
    } else {
        format!("- [[{stem}|{title}]]")
    }
}

fn promote_body(
    section: &section_model::ProseSection<'_>,
    host_is_prose: bool,
) -> Vec<String> {
    let physical = scan::split_physical_lines(section.text);
    let body: Vec<&str> = section
        .body_range
        .clone()
        .filter_map(|index| physical.get(index).map(|(content, _)| *content))
        .collect();
    let out: Vec<String> = body.iter().map(|line| line.to_string()).collect();
    let mut by_line: BTreeMap<usize, &section_model::MovedHeading> =
        BTreeMap::new();
    for heading in &section.moved_headings {
        let offset = heading
            .line
            .saturating_sub(1)
            .saturating_sub(section.body_range.start);
        by_line.insert(offset, heading);
    }
    let doc = scan::classify(section.text);
    let mut underline_to_drop: BTreeSet<usize> = BTreeSet::new();
    for heading in &section.moved_headings {
        if heading.setext {
            let title_idx = heading.line - 1;
            for line in doc.lines.iter() {
                if line.region == scan::Region::SetextUnderline
                    && line.index > title_idx
                    && section.body_range.contains(&line.index)
                    && line.index == title_idx + 1
                {
                    underline_to_drop
                        .insert(line.index - section.body_range.start);
                    break;
                }
            }
        }
    }
    let mut filtered: Vec<String> = Vec::new();
    for (offset, line) in out.into_iter().enumerate() {
        if underline_to_drop.contains(&offset) {
            continue;
        }
        if let Some(heading) = by_line.get(&offset) {
            let promoted = heading
                .original_level
                .saturating_sub(section.heading_level)
                .saturating_add(1)
                .clamp(1, 6);
            let title = if host_is_prose {
                strip_section_number(&heading.raw_title).rest.to_string()
            } else {
                heading.raw_title.clone()
            };
            filtered.push(format!("{} {title}", "#".repeat(promoted)));
        } else {
            filtered.push(line);
        }
    }
    filtered
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn plan_extract(
    root: &Path,
    note_arg: &str,
    section: Option<&str>,
    line: Option<usize>,
    stem_override: Option<&str>,
) -> Result<ExtractPlan, ExtractRefusal> {
    let host_path = match vault::resolve_note(root, note_arg) {
        Ok(path) => path,
        Err(message) => {
            let code = if message.contains("ambiguous") {
                "note-ambiguous"
            } else {
                "note-not-found"
            };
            return Err(ExtractRefusal {
                code: code.to_string(),
                message,
                hint: "Pass a vault-relative path or a unique note stem"
                    .to_string(),
                path: None,
                line: None,
                next: None,
                section: None,
                target: None,
            });
        }
    };
    if host_path
        .components()
        .any(|component| matches!(component, Component::Normal(name) if name.to_string_lossy() == ".trash"))
    {
        return Err(ExtractRefusal {
            code: "not-host-note".to_string(),
            message: "note is not an eligible area, project, or prose host".to_string(),
            hint: "Select an area, project, or prose note outside excluded, done, and reference folders".to_string(),
            path: Some(host_path),
            line,
            next: None,
            section: None,
            target: None,
        });
    }
    let host_before =
        fs::read_to_string(root.join(&host_path)).map_err(|error| {
            ExtractRefusal {
                code: "read-failed".to_string(),
                message: format!(
                    "could not read {}: {error}",
                    display_relative(&host_path)
                ),
                hint: "Check the vault path and retry".to_string(),
                path: Some(host_path.clone()),
                line,
                next: None,
                section: None,
                target: None,
            }
        })?;
    let host_doc = scan::classify(&host_before);
    let host_owned = host_doc;
    let _ = host_owned;
    // Resolve the section with vault-aware stub detection.
    let section_result = {
        let root_clone = root.to_path_buf();
        let host_clone = host_path.clone();
        section_model::select_section_with_options(
            &host_path,
            &host_before,
            section,
            line,
            |stub| resolve_child_stub(&root_clone, &host_clone, stub),
        )
    };
    let prose_section = match section_result {
        Ok(section) => section,
        Err(mut refusal) => {
            // Toggle dispatch: a preamble cursor in a prose note with a parent
            // inlines the note itself.
            if refusal.code == "no-section"
                && let Some(_parent) =
                    host_prose_parent(root, &host_path, &host_before)
            {
                refusal.next = Some(section_model::NextAction {
                    command: "inline",
                    note: host_path.clone(),
                });
            }
            let mut converted =
                ExtractRefusal::from_section(&refusal, &host_path);
            if converted.code == "no-section"
                && let Some((command, note)) = refusal
                    .next
                    .as_ref()
                    .map(|next| (next.command.to_string(), next.note.clone()))
            {
                converted.next = Some((command, note));
            }
            return Err(converted);
        }
    };
    let host_is_prose =
        matches!(prose_section.host_kind, section_model::HostKind::Prose);
    let raw_title = prose_section.raw_title.clone();
    let title = if host_is_prose {
        strip_section_number(&raw_title).rest.to_string()
    } else {
        raw_title.clone()
    };
    let owner_stem = host_path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or("note")
        .to_string();
    let slug = derive_slug(&prose_section.raw_title);
    let mut stem = match stem_override {
        Some(custom) => custom.trim().to_string(),
        None => {
            if slug.is_empty() {
                String::new()
            } else {
                format!("{owner_stem}_{slug}")
            }
        }
    };
    // Strip a trailing .md the user may have typed.
    if stem.to_lowercase().ends_with(".md") {
        stem = stem[..stem.len() - 3].to_string();
    }
    if stem.trim().is_empty() || !validate_stem(stem.trim()) {
        return Err(ExtractRefusal {
            code: "bad-name".to_string(),
            message: format!("stem {stem:?} is not a valid note name"),
            hint: "Use letters, numbers, `_`, or `-`; no slashes".to_string(),
            path: Some(host_path.clone()),
            line: Some(prose_section.heading_line),
            next: None,
            section: Some(ConversionSection {
                line: prose_section.heading_line,
                end_line: prose_section.body_range.end,
                level: prose_section.heading_level,
                title: raw_title.clone(),
            }),
            target: None,
        });
    }
    stem = stem.trim().to_string();
    let new_path = host_path
        .parent()
        .unwrap_or_else(|| Path::new(""))
        .join(format!("{stem}.md"));
    if new_path.to_str().is_none() {
        return Err(ExtractRefusal {
            code: "bad-name".to_string(),
            message: format!("stem {stem:?} is not a valid note name"),
            hint: "Use letters, numbers, `_`, or `-`; no slashes".to_string(),
            path: Some(host_path.clone()),
            line: Some(prose_section.heading_line),
            next: None,
            section: None,
            target: None,
        });
    }
    if case_insensitive_exists(root, &new_path) {
        return Err(ExtractRefusal {
            code: "name-taken".to_string(),
            message: format!(
                "{} already exists (names compare ignoring case)",
                display_relative(&new_path)
            ),
            hint: "Pass another stem with `-n`".to_string(),
            path: Some(host_path.clone()),
            line: Some(prose_section.heading_line),
            next: None,
            section: Some(ConversionSection {
                line: prose_section.heading_line,
                end_line: prose_section.body_range.end,
                level: prose_section.heading_level,
                title: raw_title.clone(),
            }),
            target: Some(ConversionTarget {
                path: display_relative(&new_path),
                stem: stem.clone(),
            }),
        });
    }
    // Build the relocation map before rewriting anything.
    let map = RelocationMap::new(&prose_section, &stem);
    // Vault-wide link scan (host excluded below; child does not exist yet).
    let vault_links = relocate::plan_vault(
        root,
        &map,
        Direction::Extract,
        None,
    )
    .map_err(|error| ExtractRefusal {
        code: error.code().to_string(),
        message: error.to_string(),
        hint: "Disambiguate the fragment so it names one heading".to_string(),
        path: Some(host_path.clone()),
        line: match error {
            relocate::RelocationError::AmbiguousLink { line, .. } => Some(line),
            _ => Some(prose_section.heading_line),
        },
        next: None,
        section: Some(ConversionSection {
            line: prose_section.heading_line,
            end_line: prose_section.body_range.end,
            level: prose_section.heading_level,
            title: raw_title.clone(),
        }),
        target: Some(ConversionTarget {
            path: display_relative(&new_path),
            stem: stem.clone(),
        }),
    })?;
    // Render the new note per X2–X3.
    let resolver = VaultLinkResolver::new(root);
    let index = resolver.note_index();
    let parent_link = link_destination(index, &host_path);
    let created = format_created_timestamp(
        bob_env::current_datetime(),
        bob_env::configured_timezone(),
    );
    let mut front = String::new();
    front.push_str("---\n");
    front.push_str(&format!(
        "parent: {}\n",
        yaml_string_scalar(&format!("[[{parent_link}]]"))
    ));
    front.push_str(&format!("type: {}\n", yaml_string_scalar("[[prose]]")));
    front.push_str(&format!("created: {created}\n"));
    front.push_str(&format!("title: {}\n", yaml_string_scalar(&title)));
    front.push_str(&format!(
        "extracted_from: {}\n",
        yaml_string_scalar(&format!("[[{parent_link}]]"))
    ));
    front.push_str(&format!(
        "extracted_heading: {}\n",
        yaml_string_scalar(&title)
    ));
    front.push_str("section_numbers: false\n");
    front.push_str("---\n");
    let host_classified = scan::classify(&host_before);
    let default_ending = host_classified.default_ending;
    let bom = host_classified.bom;
    let promoted = promote_body(&prose_section, host_is_prose);
    // Join promoted body with the host default ending for the first pass.
    let mut new_initial = String::new();
    if bom {
        new_initial.push('\u{feff}');
    }
    new_initial.push_str(&front.replace('\n', default_ending));
    // Exactly one blank line between frontmatter and body (F1).
    if !new_initial.ends_with(default_ending) {
        new_initial.push_str(default_ending);
    }
    new_initial.push_str(default_ending);
    for content in &promoted {
        new_initial.push_str(content);
        // Preserve the body's original endings where possible; new
        // heading lines use the host default ending.
        new_initial.push_str(default_ending);
    }
    // Link-relocate the new note, then format it with numbering off.
    let relocated_new = map
        .rewrite_note(root, &new_path, &new_initial, Direction::Extract, &[])
        .map_err(|error| ExtractRefusal {
            code: error.code().to_string(),
            message: error.to_string(),
            hint: "Disambiguate the fragment so it names one heading"
                .to_string(),
            path: Some(host_path.clone()),
            line: Some(prose_section.heading_line),
            next: None,
            section: None,
            target: None,
        })?;
    let new_ctx = ProseContext::new(&new_path);
    let new_plan = format(&relocated_new.contents, &new_ctx);
    if matches!(
        new_plan.status,
        super::super::plan::ProseStatus::Skipped
            | super::super::plan::ProseStatus::Error
    ) {
        let (code, message) = new_plan
            .reason
            .as_ref()
            .map(|reason| (reason.code.clone(), reason.message.clone()))
            .unwrap_or_else(|| {
                (
                    "words-changed".to_string(),
                    "new note failed validation".to_string(),
                )
            });
        return Err(ExtractRefusal {
            code,
            message,
            hint: "Choose another section".to_string(),
            path: Some(host_path.clone()),
            line: Some(prose_section.heading_line),
            next: None,
            section: None,
            target: None,
        });
    }
    let new_contents =
        new_plan.formatted.clone().unwrap_or(relocated_new.contents);
    // Build the host after-image: stub insertion, then link relocation,
    // then a format pass when the host is prose.
    let host_after_stub =
        build_host_after(&host_before, &prose_section, &stem, &title);
    let relocated_host = map
        .rewrite_note(
            root,
            &host_path,
            &host_after_stub,
            Direction::Extract,
            &[],
        )
        .map_err(|error| ExtractRefusal {
            code: error.code().to_string(),
            message: error.to_string(),
            hint: "Disambiguate the fragment so it names one heading"
                .to_string(),
            path: Some(host_path.clone()),
            line: Some(prose_section.heading_line),
            next: None,
            section: None,
            target: None,
        })?;
    let mut host_after = relocated_host.contents;
    let mut host_layout_extra = 0usize;
    let mut host_replaced_extra = 0usize;
    let mut host_findings_extra: Vec<super::super::findings::Finding> =
        Vec::new();
    if host_is_prose {
        let host_ctx = ProseContext::new(&host_path);
        let host_plan = format(&host_after, &host_ctx);
        if matches!(
            host_plan.status,
            super::super::plan::ProseStatus::Skipped
                | super::super::plan::ProseStatus::Error
        ) {
            let (code, message) = host_plan
                .reason
                .as_ref()
                .map(|reason| (reason.code.clone(), reason.message.clone()))
                .unwrap_or_else(|| {
                    (
                        "words-changed".to_string(),
                        "host failed validation".to_string(),
                    )
                });
            return Err(ExtractRefusal {
                code,
                message,
                hint: "Choose another section".to_string(),
                path: Some(host_path.clone()),
                line: Some(prose_section.heading_line),
                next: None,
                section: None,
                target: None,
            });
        }
        host_layout_extra = host_plan.changes.layout;
        host_replaced_extra = host_plan.replaced.len();
        host_findings_extra = host_plan.findings;
        host_after = host_plan.formatted.clone().unwrap_or(host_after);
    }
    // Verify the stub classifies as stub-section.
    {
        let after_section = section_model::select_section(
            &host_path,
            &host_after,
            section_model::Selector::Section(&prose_section.identity),
            |_| None,
        );
        match after_section {
            Err(refusal) if refusal.code == "stub-section" => {}
            _ => {
                // Fall back to the syntactic check when identity matching
                // drifts (for example numbering rewrote the heading).
                let headings_doc = scan::classify(&host_after);
                let mut found_stub = false;
                // Re-select by line: the kept heading line still holds.
                if let Ok(kept) = section_model::select_section(
                    &host_path,
                    &host_after,
                    section_model::Selector::Line(prose_section.heading_line),
                    |_| None,
                ) && section_model::stub_target(&kept).is_some()
                {
                    found_stub = true;
                }
                let _ = headings_doc;
                if !found_stub {
                    return Err(ExtractRefusal {
                        code: "verify_failed".to_string(),
                        message:
                            "extracted stub did not classify as stub-section"
                                .to_string(),
                        hint: "Retry the extraction".to_string(),
                        path: Some(host_path.clone()),
                        line: Some(prose_section.heading_line),
                        next: None,
                        section: None,
                        target: None,
                    });
                }
            }
        }
    }
    // Collect linker rewrites (everything except host).
    let mut linker_files: Vec<(PathBuf, String, String)> = Vec::new();
    let mut links: Vec<ConversionLink> = Vec::new();
    for file in &vault_links.files {
        if file.path == host_path {
            continue;
        }
        if file.path == new_path {
            continue;
        }
        // Never touch excluded or trash paths even if the scanner did.
        if file.path.components().any(|component| match component {
            Component::Normal(name) => {
                let name = name.to_string_lossy();
                name == ".bob"
                    || name == ".trash"
                    || name == ".obsidian"
                    || name == ".git"
                    || name == "_conflicts"
                    || name == "_templates"
                    || name == "_generated"
            }
            _ => false,
        }) {
            continue;
        }
        let before =
            fs::read_to_string(root.join(&file.path)).unwrap_or_default();
        linker_files.push((file.path.clone(), before, file.contents.clone()));
    }
    // Host + new link rows.
    for rewrite in relocated_host.links {
        links.push(ConversionLink {
            path: display_relative(&rewrite.path),
            line: rewrite.line,
            before: rewrite.before,
            after: rewrite.after,
        });
    }
    for rewrite in relocated_new.links {
        links.push(ConversionLink {
            path: display_relative(&new_path),
            line: rewrite.line,
            before: rewrite.before,
            after: rewrite.after,
        });
    }
    for link in &vault_links.links {
        if link.path == host_path {
            continue;
        }
        if link.path.components().any(|component| match component {
            Component::Normal(name) => {
                let name = name.to_string_lossy();
                name == ".bob"
                    || name == ".trash"
                    || name == ".obsidian"
                    || name == ".git"
                    || name == "_conflicts"
                    || name == "_templates"
                    || name == "_generated"
            }
            _ => false,
        }) {
            continue;
        }
        links.push(ConversionLink {
            path: display_relative(&link.path),
            line: link.line,
            before: link.before.clone(),
            after: link.after.clone(),
        });
    }
    links.sort_by(|a, b| {
        (&a.path, a.line, &a.before).cmp(&(&b.path, b.line, &b.before))
    });
    links.dedup_by(|a, b| {
        a.path == b.path
            && a.line == b.line
            && a.before == b.before
            && a.after == b.after
    });
    // Fingerprint over every touched path plus selector, stem, stub form.
    let mut touched: BTreeMap<String, String> = BTreeMap::new();
    touched.insert(display_relative(&host_path), sha256_hex(&host_before));
    touched.insert(display_relative(&new_path), "absent".to_string());
    for (path, before, _) in &linker_files {
        touched.insert(display_relative(path), sha256_hex(before));
    }
    let end_line = prose_section.body_range.end;
    let fingerprint =
        fingerprint_for(&touched, prose_section.heading_line, end_line, &stem);
    let layout_count = new_plan.changes.layout + host_layout_extra;
    let replaced_count = new_plan.replaced.len() + host_replaced_extra;
    let mut findings = new_plan.findings.clone();
    findings.extend(host_findings_extra);
    findings.sort_by_key(|finding| finding.line);
    // Focus is the new note's first body line (first non-frontmatter line).
    let focus_line = {
        let doc = scan::classify(&new_contents);
        doc.lines
            .iter()
            .find(|line| {
                line.region != scan::Region::Frontmatter
                    && !line.content.trim().is_empty()
            })
            .map(|line| line.index + 1)
            .unwrap_or(1)
    };
    // Word-invariant spot check: formatting the new note again is a no-op.
    {
        let second = format(&new_contents, &new_ctx);
        if second.formatted.as_deref() != Some(new_contents.as_str())
            || matches!(
                second.status,
                super::super::plan::ProseStatus::Skipped
                    | super::super::plan::ProseStatus::Error
            )
        {
            return Err(ExtractRefusal {
                code: "not-idempotent".to_string(),
                message: "second format pass still edits the note".to_string(),
                hint: "Retry the extraction".to_string(),
                path: Some(host_path.clone()),
                line: Some(prose_section.heading_line),
                next: None,
                section: None,
                target: None,
            });
        }
    }
    // The new path is used below; keep the binding for the fingerprint scope.
    let _ = &new_path;
    let section_line = prose_section.heading_line;
    let section_level = prose_section.heading_level;
    let section_title = raw_title.clone();
    let host_before_owned = host_before.clone();
    Ok(ExtractPlan {
        host_path,
        host_before: host_before_owned,
        host_after,
        new_path,
        new_stem: stem,
        new_contents,
        linker_files,
        links,
        section_line,
        section_end: end_line,
        section_level,
        section_title,
        fingerprint,
        layout_count,
        replaced_count,
        findings,
        focus_line,
    })
}

fn build_host_after(
    host_before: &str,
    section: &section_model::ProseSection<'_>,
    stem: &str,
    title: &str,
) -> String {
    let doc = scan::classify(host_before);
    let default_ending = doc.default_ending;
    let bullet = stub_bullet(stem, title);
    let mut lines: Vec<(String, String)> = doc
        .lines
        .iter()
        .map(|line| (line.content.to_string(), line.ending.to_string()))
        .collect();
    let replacement: Vec<(String, String)> = vec![
        (String::new(), default_ending.to_string()),
        (bullet, default_ending.to_string()),
        (String::new(), default_ending.to_string()),
    ];
    // When the section runs to end of file, drop the trailing blank so the
    // file ends with the stub line plus one newline (F8).
    let at_eof = section.body_range.end >= lines.len();
    let mut final_replacement = replacement;
    if at_eof
        && final_replacement
            .last()
            .is_some_and(|(content, _)| content.is_empty())
    {
        final_replacement.pop();
    }
    lines.splice(section.body_range.clone(), final_replacement);
    super::super::layout::join(
        doc.bom,
        &lines
            .iter()
            .map(|(content, ending)| (content.clone(), ending.clone()))
            .collect::<Vec<_>>(),
        doc.unclosed_fence.is_some(),
        default_ending,
    )
}

fn report_for_preview(plan: &ExtractPlan, host_arg: &str) -> ConversionReport {
    let host_rel = display_relative(&plan.host_path);
    let new_rel = display_relative(&plan.new_path);
    let mut changes = Vec::new();
    changes.push(ConversionChange {
        path: host_rel.clone(),
        action: "update".to_string(),
        sha256: Some(sha256_hex(&plan.host_before)),
        diff: unified_diff(&host_rel, &plan.host_before, &plan.host_after),
    });
    changes.push(ConversionChange {
        path: new_rel.clone(),
        action: "create".to_string(),
        sha256: None,
        diff: unified_diff(&new_rel, "", &plan.new_contents),
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
    changes.sort_by(|a, b| a.path.cmp(&b.path));
    let title = plan.section_title.clone();
    let notice = format!(
        "Extracted “{title}” into {stem} · {} {} repaired",
        plan.links.len(),
        if plan.links.len() == 1 {
            "link"
        } else {
            "links"
        },
        stem = plan.new_stem,
    );
    ConversionReport {
        schema_version: SCHEMA_VERSION,
        contract: CONTRACT.to_string(),
        command: "extract".to_string(),
        status: "preview".to_string(),
        fingerprint: plan.fingerprint.clone(),
        note: host_arg.to_string(),
        section: Some(ConversionSection {
            line: plan.section_line,
            end_line: plan.section_end,
            level: plan.section_level,
            title: plan.section_title.clone(),
        }),
        target: Some(ConversionTarget {
            path: new_rel.clone(),
            stem: plan.new_stem.clone(),
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
            path: new_rel,
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
    refusal: &ExtractRefusal,
    host_arg: &str,
    fingerprint: &str,
) -> ConversionReport {
    ConversionReport {
        schema_version: SCHEMA_VERSION,
        contract: CONTRACT.to_string(),
        command: "extract".to_string(),
        status: "refused".to_string(),
        fingerprint: fingerprint.to_string(),
        note: host_arg.to_string(),
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
            next: refusal.next.as_ref().map(|(command, note)| ConversionNext {
                command: command.clone(),
                note: display_relative(note),
            }),
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

fn empty_fingerprint() -> String {
    format!("sha256:{}", sha256_hex("empty"))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OutputFormat {
    Human,
    Json,
}

fn output_format_from_matches(matches: &clap::ArgMatches) -> OutputFormat {
    match matches
        .get_one::<String>("format")
        .map(String::as_str)
        .unwrap_or("human")
    {
        "json" => OutputFormat::Json,
        _ => OutputFormat::Human,
    }
}

pub(crate) fn run_extract(matches: &clap::ArgMatches) -> i32 {
    let root = matches
        .get_one::<OsString>("bob-dir")
        .map(PathBuf::from)
        .map(|path| bob_env::expand_tilde(&path))
        .unwrap_or_else(bob_env::bob_dir);
    let note = matches
        .get_one::<String>("note")
        .map(String::as_str)
        .unwrap_or("");
    let section = matches.get_one::<String>("section").map(String::as_str);
    let line = matches.get_one::<usize>("line").copied().or_else(|| {
        matches
            .get_one::<String>("line")
            .and_then(|value| value.parse::<usize>().ok())
    });
    let stem = matches.get_one::<String>("name").map(String::as_str);
    let write = matches.get_flag("write");
    let expect = matches.get_one::<String>("expect").map(String::as_str);
    let offline = matches.get_flag("offline");
    let format = output_format_from_matches(matches);
    let json_mode = format == OutputFormat::Json;

    // Preview plans once from disk.
    let planned = plan_extract(&root, note, section, line, stem);
    let plan = match planned {
        Ok(plan) => plan,
        Err(refusal) => {
            let fingerprint = refusal
                .section
                .as_ref()
                .map(|_| empty_fingerprint())
                .unwrap_or_else(empty_fingerprint);
            // Use the refusal's own fingerprint when available: recompute
            // a stable placeholder from the host path.
            let _ = &fingerprint;
            let report = report_for_refusal(&refusal, note, &fingerprint);
            return print_refusal(&report, &refusal, format);
        }
    };

    if let Some(expected) = expect
        && expected != plan.fingerprint
    {
        let refusal = ExtractRefusal {
            code: "plan-changed".to_string(),
            message:
                "the vault changed since the preview; re-preview before writing"
                    .to_string(),
            hint: "Re-run without `-w` to preview the new plan".to_string(),
            path: Some(plan.host_path.clone()),
            line: Some(plan.section_line),
            next: None,
            section: Some(ConversionSection {
                line: plan.section_line,
                end_line: plan.section_end,
                level: plan.section_level,
                title: plan.section_title.clone(),
            }),
            target: Some(ConversionTarget {
                path: display_relative(&plan.new_path),
                stem: plan.new_stem.clone(),
            }),
        };
        let report = report_for_refusal(&refusal, note, &plan.fingerprint);
        return print_refusal(&report, &refusal, format);
    }

    if !write {
        let report = report_for_preview(&plan, note);
        print_preview(&report, format);
        return 0;
    }

    // Write through vault_rewrite: plan again from disk after pre-sync.
    let child_env = ob::child_env();
    let command_name = "bob prose extract";
    let host_arg = note.to_string();
    let section_owned = section.map(str::to_string);
    let stem_owned = stem.map(str::to_string);
    let expect_owned = expect.map(str::to_string);
    let outcome = vault_rewrite::run(
        &root,
        &child_env,
        command_name,
        "prose section extractions",
        offline,
        json_mode,
        || {
            let fresh = plan_extract(
                &root,
                &host_arg,
                section_owned.as_deref(),
                line,
                stem_owned.as_deref(),
            )
            .map_err(|refusal| {
                vault_rewrite::TxnFailure::new(
                    Box::leak(refusal.code.clone().into_boxed_str()) as &str,
                    refusal.message.clone(),
                    Some(refusal.hint.clone()),
                )
            })?;
            if let Some(expected) = expect_owned.as_deref()
                && expected != fresh.fingerprint
            {
                return Err(vault_rewrite::TxnFailure::new(
                    "plan-changed",
                    "the vault changed since the preview; re-preview before writing",
                    Some("re-run without `-w` to preview the new plan"),
                ));
            }
            let owner_stem = fresh
                .host_path
                .file_stem()
                .and_then(|stem| stem.to_str())
                .unwrap_or("note");
            let subject = format!(
                "bob prose extract: {} § {} → {}",
                owner_stem, fresh.section_title, fresh.new_stem
            );
            let mut files = Vec::new();
            files.push(vault_rewrite::FileRewrite {
                path: display_relative(&fresh.host_path),
                before_contents: Some(fresh.host_before.clone()),
                after_contents: fresh.host_after.clone(),
                trash: false,
            });
            files.push(vault_rewrite::FileRewrite {
                path: display_relative(&fresh.new_path),
                before_contents: None,
                after_contents: fresh.new_contents.clone(),
                trash: false,
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
            let verify_host = fresh.host_path.clone();
            let verify_host_after = fresh.host_after.clone();
            let verify_new = fresh.new_path.clone();
            let verify_new_after = fresh.new_contents.clone();
            let verify_section_line = fresh.section_line;
            Ok(vault_rewrite::ReadyPlan {
                files,
                subject,
                post_write: Box::new(|| Ok(Vec::new())),
                verify: Box::new(move || {
                    verify_written(
                        &verify_root,
                        &verify_host,
                        &verify_host_after,
                        &verify_new,
                        &verify_new_after,
                        verify_section_line,
                    )
                }),
                on_committed: Box::new(|| {}),
                context: fresh,
            })
        },
    );
    match outcome {
        Ok(outcome) => {
            let plan = outcome.context;
            let mut report = report_for_preview(&plan, note);
            report.status = "written".to_string();
            report.commit = Some(ConversionCommit {
                sha: outcome.sha.clone(),
                subject: outcome.subject.clone(),
            });
            report.sync = ConversionSync {
                pre: outcome.sync_pre.to_string(),
                post: outcome.sync_post.to_string(),
            };
            let mut warnings = Vec::new();
            if let Some(warning) = outcome.dirty_warning {
                warnings.push(warning);
            }
            if let Some(warning) = outcome.post_sync_warning {
                warnings.push(warning);
            }
            if let Some(warning) = outcome.recovery_warning {
                warnings.push(warning);
            }
            report.warnings = warnings;
            match format {
                OutputFormat::Json => {
                    println!(
                        "{}",
                        serde_json::to_string(&report).unwrap_or_default()
                    );
                }
                OutputFormat::Human => {
                    print_written(&report);
                }
            }
            0
        }
        Err(failure) => {
            let refusal = ExtractRefusal {
                code: failure.code.to_string(),
                message: failure.message.clone(),
                hint: failure
                    .hint
                    .clone()
                    .unwrap_or_else(|| "Retry the extraction".to_string()),
                path: None,
                line: None,
                next: None,
                section: None,
                target: None,
            };
            let report =
                report_for_refusal(&refusal, note, &empty_fingerprint());
            match format {
                OutputFormat::Json => {
                    println!(
                        "{}",
                        serde_json::to_string(&report).unwrap_or_default()
                    );
                }
                OutputFormat::Human => {
                    eprintln!("bob prose extract: {}", refusal.message);
                    eprintln!("hint: {}", refusal.hint);
                }
            }
            2
        }
    }
}

fn print_refusal(
    report: &ConversionReport,
    refusal: &ExtractRefusal,
    format: OutputFormat,
) -> i32 {
    match format {
        OutputFormat::Json => {
            println!("{}", serde_json::to_string(report).unwrap_or_default());
        }
        OutputFormat::Human => {
            eprintln!("bob prose extract: {}", refusal.message);
            eprintln!("hint: {}", refusal.hint);
        }
    }
    2
}

fn print_preview(report: &ConversionReport, format: OutputFormat) {
    match format {
        OutputFormat::Json => {
            println!("{}", serde_json::to_string(report).unwrap_or_default());
        }
        OutputFormat::Human => {
            use crate::native::style::Styler;
            let styler = Styler::detect();
            println!("{}", styler.cyan(&report.notice));
            for change in &report.changes {
                println!("--- a/{}", change.path);
                println!("+++ b/{}", change.path);
                // The stored diff already contains a/ b/ headers; print body only.
                for line in change.diff.lines().skip(2) {
                    if line.starts_with('+') {
                        println!("{}", styler.green(line));
                    } else if line.starts_with('-') {
                        println!("{}", styler.red(line));
                    } else {
                        println!("{line}");
                    }
                }
            }
            println!(
                "  counts: {} {}, {} {}, layout {}, replaced {}",
                report.counts.notes,
                if report.counts.notes == 1 {
                    "note"
                } else {
                    "notes"
                },
                report.counts.links,
                if report.counts.links == 1 {
                    "link"
                } else {
                    "links"
                },
                report.counts.layout,
                report.counts.replaced,
            );
            for finding in &report.findings {
                println!(
                    "  line {} · {} ({})",
                    finding.line, finding.message, finding.hint
                );
            }
            println!("{}", styler.dim("Preview only · rerun with -w to write"));
        }
    }
}

fn print_written(report: &ConversionReport) {
    use crate::native::style::Styler;
    let styler = Styler::detect();
    println!("{} {}", styler.green("✓"), report.notice);
    if let Some(commit) = &report.commit {
        println!("  committed {} {}", commit.sha, commit.subject);
    }
    if report.sync.post == "failed" {
        println!("  sync pending");
    } else if report.sync.post == "ok" {
        println!("  extracted locally; sync {}", report.sync.post);
    }
    for warning in &report.warnings {
        eprintln!("{}: {warning}", styler.warning_prefix());
    }
}

fn verify_written(
    root: &Path,
    host_path: &Path,
    host_after: &str,
    new_path: &Path,
    new_after: &str,
    heading_line: usize,
) -> Result<(), String> {
    let live_host =
        fs::read_to_string(root.join(host_path)).map_err(|error| {
            format!("reread {}: {error}", display_relative(host_path))
        })?;
    if live_host != host_after {
        return Err(format!(
            "{} changed during the extract",
            display_relative(host_path)
        ));
    }
    let live_new =
        fs::read_to_string(root.join(new_path)).map_err(|error| {
            format!("reread {}: {error}", display_relative(new_path))
        })?;
    if live_new != new_after {
        return Err(format!(
            "{} changed during the extract",
            display_relative(new_path)
        ));
    }
    // The written stub must classify as stub-section.
    let kept = section_model::select_section(
        host_path,
        &live_host,
        section_model::Selector::Line(heading_line),
        |_| None,
    )
    .map_err(|error| format!("stub check failed: {}", error.message))?;
    if section_model::stub_target(&kept).is_none() {
        return Err(
            "extracted stub did not classify as stub-section".to_string()
        );
    }
    // Formatting the new note again must be a no-op.
    let ctx = ProseContext::new(new_path);
    let second = format(&live_new, &ctx);
    if second.formatted.as_deref() != Some(live_new.as_str()) {
        return Err("new note is not idempotent".to_string());
    }
    Ok(())
}
