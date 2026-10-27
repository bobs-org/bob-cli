//! Saved Markdown source and code/link catalog for web-article captures.
//!
//! A web capture has no Markdown that a renderer consumed: the adapter
//! prints Bob-owned reader HTML. This module converts the renderer's
//! source view (`render/source.html`) to GFM with the shared pandoc
//! command, catalogs it with the existing record filters, maps code
//! pages from Chrome's print-time anchor destinations and link pages
//! from the rendered PDF's URI annotations, and writes the `web_article`
//! store plus its note pointer on every single-article create path.
//! Overwrites reuse [`prepare_web_store`] and stage the bytes into the
//! same transaction as Markdown.

use std::{collections::BTreeMap, fs, path::Path, process::Command};

use lopdf::Document;
use serde::Deserialize;

use super::code_catalog::{CatalogOutcome, CodeBlock};
use super::model::Config;
use super::stamp::{TargetPlan, TargetWorkflow};

/// Route recorded in `code.json` for single-URL web-article captures.
pub(crate) const WEB_ARTICLE_ROUTE: &str = "web_article";

/// Route recorded in `code.json` for multi-URL web-collection captures.
/// Anchors number across all parts in document order, so one composed
/// source view yields one store.
pub(crate) const WEB_COLLECTION_ROUTE: &str = "web_collection";

/// One printed code block reported by the adapter's anchor pass, in
/// document order.
#[derive(Debug, Deserialize)]
struct AnchorBlock {
    #[serde(default)]
    text: String,
}

/// The adapter's `render/code-anchors.json` report.
#[derive(Debug, Deserialize)]
struct AnchorReport {
    #[serde(default)]
    blocks: Vec<AnchorBlock>,
}

/// A converted web source with its catalog, ready to persist.
pub(super) struct PreparedWebStore {
    pub(super) source_bytes: Vec<u8>,
    pub(super) catalog: CatalogOutcome,
    warnings: Vec<String>,
}

impl PreparedWebStore {
    pub(super) fn print_warnings(&self) {
        for warning in &self.warnings {
            eprintln!("warning: {warning}");
        }
    }
}

/// Why no web store is written: the short label goes in the
/// `source_markdown: none (...)` line, the detail in a `warning:` line.
pub(super) struct WebStoreSkip {
    short: String,
    detail: String,
}

/// Normalize anchor and catalog block text the same way before comparing:
/// Chrome's DOM text and pandoc's GFM round-trip agree up to line-ending
/// style and trailing newlines.
fn normalize_anchor_text(text: &str) -> String {
    text.replace("\r\n", "\n")
        .replace('\r', "\n")
        .trim_end_matches('\n')
        .to_string()
}

/// Convert the renderer's source view to GFM with the shared pandoc
/// command, exactly like the Markdown route converts with its reader.
fn convert_source_view(source_html: &Path) -> Result<Vec<u8>, String> {
    let pandoc = super::create::pandoc_command().ok_or_else(|| {
        "pandoc command not found; install pandoc or set BOB_PANDOC_COMMAND"
            .to_string()
    })?;
    let output = Command::new(&pandoc)
        .arg(source_html)
        .arg("-f")
        .arg("html")
        .arg("-t")
        .arg("gfm")
        .arg("--wrap=none")
        .arg("--preserve-tabs")
        .output()
        .map_err(|error| {
            format!("run pandoc for {}: {error}", source_html.display())
        })?;
    if !output.status.success() {
        let detail = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(format!(
            "pandoc failed on {}: {}",
            source_html.display(),
            if detail.is_empty() {
                "pandoc produced no diagnostic output".to_string()
            } else {
                detail
            }
        ));
    }
    Ok(output.stdout)
}

/// Read the adapter's anchor report. A missing or unreadable report
/// yields no anchors: the catalog stays pageless but copyable, and the
/// unpaged-blocks warning names the gap.
fn read_anchor_texts(code_anchors: Option<&str>) -> Vec<String> {
    let Some(path) = code_anchors else {
        return Vec::new();
    };
    let raw = fs::read_to_string(path).unwrap_or_default();
    if raw.is_empty() {
        return Vec::new();
    }
    serde_json::from_str::<AnchorReport>(&raw)
        .map(|report| {
            report.blocks.into_iter().map(|block| block.text).collect()
        })
        .unwrap_or_default()
}

/// Match catalog blocks to printed anchors greedily in document order by
/// normalized equal text. A text match takes its page range from the
/// `bob-code-k` / `bob-code-k-end` destinations with `page_source:
/// "anchor"`; every other block keeps `page: None` and stays copyable.
/// Returns the count of pageless blocks and of printed anchors that
/// never matched the saved Markdown.
fn assign_anchor_pages(
    blocks: &mut [CodeBlock],
    anchors: &[String],
    dests: &BTreeMap<String, u32>,
) -> (usize, usize) {
    let normalized: Vec<String> = anchors
        .iter()
        .map(|text| normalize_anchor_text(text))
        .collect();
    let mut cursor = 0;
    let mut matched_anchors = 0;
    let mut unpaged = 0;
    for block in blocks.iter_mut() {
        let wanted = normalize_anchor_text(&block.text);
        let mut anchor = None;
        while cursor < normalized.len() {
            let candidate = cursor;
            cursor += 1;
            if normalized[candidate] == wanted {
                anchor = Some(candidate);
                matched_anchors += 1;
                break;
            }
        }
        let Some(index) = anchor else {
            unpaged += 1;
            continue;
        };
        let key = index + 1;
        let page = dests.get(&format!("bob-code-{key}")).copied();
        let page_end = dests.get(&format!("bob-code-{key}-end")).copied();
        if page.is_none() && page_end.is_none() {
            unpaged += 1;
            continue;
        }
        block.page = page;
        block.page_end = page_end;
        block.page_source = Some("anchor".to_string());
    }
    (unpaged, normalized.len().saturating_sub(matched_anchors))
}

/// Build the converted source and its catalog from one capture response.
/// Never fails the create: every failure becomes a [`WebStoreSkip`].
pub(super) fn prepare_web_store(
    source_html: Option<&str>,
    code_anchors: Option<&str>,
    rendered_pdf: &Path,
    route: &str,
) -> Result<PreparedWebStore, WebStoreSkip> {
    let Some(source_html) = source_html else {
        return Err(WebStoreSkip {
            short: "no source view".to_string(),
            detail: "adapter reported no source view".to_string(),
        });
    };
    let source_path = Path::new(source_html);
    if !source_path.is_file() {
        return Err(WebStoreSkip {
            short: "no source view".to_string(),
            detail: format!(
                "adapter source view is missing: {}",
                source_path.display()
            ),
        });
    }
    let source_bytes = convert_source_view(source_path).map_err(|detail| {
        let short = if detail.starts_with("pandoc command not found") {
            "pandoc unavailable"
        } else {
            "conversion failed"
        };
        WebStoreSkip {
            short: short.to_string(),
            detail,
        }
    })?;
    let mut catalog =
        crate::native::ref_library::attach::record_source_catalog(
            &source_bytes,
            route,
        )
        .map_err(|detail| WebStoreSkip {
            short: "catalog failed".to_string(),
            detail,
        })?;
    let mut warnings = Vec::new();
    let anchors = read_anchor_texts(code_anchors);
    let dests = Document::load(rendered_pdf)
        .map(|document| super::pdf_dests::named_dest_pages(&document))
        .unwrap_or_default();
    let (unpaged, unmatched) =
        assign_anchor_pages(&mut catalog.blocks, &anchors, &dests);
    if unpaged > 0 {
        let noun = if catalog.blocks.len() == 1 {
            "block"
        } else {
            "blocks"
        };
        warnings.push(format!(
            "{} of {} code {} have no page",
            unpaged,
            catalog.blocks.len(),
            noun
        ));
    }
    if unmatched > 0 {
        let noun = if unmatched == 1 { "block" } else { "blocks" };
        warnings.push(format!(
            "{unmatched} printed code {noun} are not in the saved Markdown"
        ));
    }
    let mut links = catalog.links.take().unwrap_or_default();
    if !links.is_empty()
        && let Err(error) =
            super::pdf_links::annotate_catalog_links(&mut links, rendered_pdf)
    {
        warnings.push(format!("link pages unavailable: {error}"));
    }
    catalog.links = Some(links);
    Ok(PreparedWebStore {
        source_bytes,
        catalog: CatalogOutcome::Catalog(catalog),
        warnings,
    })
}

/// Mirror `persist_markdown_store` for a converted web source: print the
/// `source_markdown:`, `code:`, and `links:` lines and hand the verified
/// pointer to intake. External targets print `none` and write nothing.
/// Store failures warn and never fail the create.
pub(super) fn persist_prepared_store(
    config: &Config,
    plan: &TargetPlan,
    installed_pdf: &Path,
    origin: String,
    route: &str,
    prepared: &PreparedWebStore,
) -> Option<String> {
    if matches!(plan.workflow, TargetWorkflow::External) {
        println!("source_markdown: none (output outside the library)");
        return None;
    }
    for warning in &prepared.warnings {
        eprintln!("warning: {warning}");
    }
    match super::source_store::write_for_create(
        config,
        plan,
        installed_pdf,
        &prepared.source_bytes,
        route,
        origin,
        None,
        None,
        &prepared.catalog,
    ) {
        Some(outcome) => {
            println!("source_markdown: {}", outcome.pointer);
            println!(
                "code: {}",
                super::source_store::code_summary(&prepared.catalog)
            );
            println!(
                "links: {}",
                super::source_store::link_summary(&prepared.catalog)
            );
            Some(outcome.pointer)
        }
        None => {
            // The warning is already printed; keep the pointer line honest.
            println!("source_markdown: none (store unavailable)");
            None
        }
    }
}

/// Report a skipped web store: a `warning:` line plus an honest
/// `source_markdown: none (...)` line. The reference still files.
pub(super) fn print_web_store_skip(skip: &WebStoreSkip) {
    eprintln!(
        "warning: {}; skipping source store (reference still filed)",
        skip.detail
    );
    println!("source_markdown: none ({})", skip.short);
}

/// Overwrite fallback when the source view or conversion is unavailable:
/// keep Clear and warn. The success report does not print a `none` line.
pub(super) fn print_web_overwrite_skip(skip: &WebStoreSkip) {
    eprintln!(
        "warning: {}; skipping source store (clearing source_markdown)",
        skip.detail
    );
}

/// Convert, catalog, map, and persist the `web_collection` store for
/// one finished collection compose. `origin` is the cleaned URLs in
/// argument order, one per line. Returns the verified pointer for
/// intake, or `None` when no store was written.
pub(super) fn persist_collection_store(
    config: &Config,
    plan: &TargetPlan,
    rendered_pdf: &Path,
    installed_pdf: &Path,
    source_html: Option<&str>,
    code_anchors: Option<&str>,
    origin: String,
) -> Option<String> {
    match prepare_web_store(
        source_html,
        code_anchors,
        rendered_pdf,
        WEB_COLLECTION_ROUTE,
    ) {
        Ok(prepared) => persist_prepared_store(
            config,
            plan,
            installed_pdf,
            origin,
            WEB_COLLECTION_ROUTE,
            &prepared,
        ),
        Err(skip) => {
            if !matches!(plan.workflow, TargetWorkflow::External) {
                print_web_store_skip(&skip);
            } else {
                println!("source_markdown: none (output outside the library)");
            }
            None
        }
    }
}

/// Convert, catalog, map, and persist the `web_article` store for one
/// finished article capture. Returns the verified pointer for intake, or
/// `None` when no store was written.
pub(super) fn persist_article_store(
    config: &Config,
    plan: &TargetPlan,
    rendered_pdf: &Path,
    installed_pdf: &Path,
    source_html: Option<&str>,
    code_anchors: Option<&str>,
    origin: String,
) -> Option<String> {
    match prepare_web_store(
        source_html,
        code_anchors,
        rendered_pdf,
        WEB_ARTICLE_ROUTE,
    ) {
        Ok(prepared) => persist_prepared_store(
            config,
            plan,
            installed_pdf,
            origin,
            WEB_ARTICLE_ROUTE,
            &prepared,
        ),
        Err(skip) => {
            if !matches!(plan.workflow, TargetWorkflow::External) {
                print_web_store_skip(&skip);
            } else {
                println!("source_markdown: none (output outside the library)");
            }
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn anchor_normalization_ignores_endings_and_trailing_newlines() {
        assert_eq!(normalize_anchor_text("let x = 1;\r\n"), "let x = 1;");
        assert_eq!(normalize_anchor_text("let x = 1;\r"), "let x = 1;");
        assert_eq!(normalize_anchor_text("let x = 1;\n\n"), "let x = 1;");
        assert_eq!(
            normalize_anchor_text("a\nb\n"),
            normalize_anchor_text("a\r\nb")
        );
        assert_ne!(normalize_anchor_text("a\nb"), normalize_anchor_text("a b"));
    }

    #[test]
    fn anchor_matching_is_greedy_in_order() {
        fn block(text: &str) -> CodeBlock {
            CodeBlock {
                n: 0,
                kind: "code".to_string(),
                lang: None,
                classes: Vec::new(),
                attrs: BTreeMap::new(),
                text: text.to_string(),
                section: None,
                page: None,
                page_end: None,
                page_source: None,
            }
        }
        let mut blocks =
            vec![block("first"), block("second"), block("missing")];
        let anchors = vec![
            "stale".to_string(),
            "first\n".to_string(),
            "second".to_string(),
        ];
        let mut dests = BTreeMap::new();
        dests.insert("bob-code-2".to_string(), 3);
        dests.insert("bob-code-2-end".to_string(), 4);
        dests.insert("bob-code-3".to_string(), 5);
        let (unpaged, unmatched) =
            assign_anchor_pages(&mut blocks, &anchors, &dests);
        assert_eq!(blocks[0].page, Some(3));
        assert_eq!(blocks[0].page_end, Some(4));
        assert_eq!(blocks[0].page_source.as_deref(), Some("anchor"));
        assert_eq!(blocks[1].page, Some(5));
        assert_eq!(blocks[1].page_end, None);
        assert_eq!(blocks[1].page_source.as_deref(), Some("anchor"));
        assert_eq!(blocks[2].page, None);
        assert_eq!(blocks[2].page_source, None);
        assert_eq!((unpaged, unmatched), (1, 1));
    }
}
