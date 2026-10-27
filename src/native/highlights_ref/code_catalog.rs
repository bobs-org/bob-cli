//! Exact code-block records from the Markdown AST and their rendered PDF pages.

use std::{collections::BTreeMap, fs, path::Path};

use lopdf::Document;
use serde::{Deserialize, Serialize};

use super::pdf_dests;

/// Shared tab expansion prelude prepended to both Lua filters that parse code.
pub(crate) const TAB_EXPANSION_PRELUDE: &str =
    include_str!("code_catalog_tabs.lua");
/// First pass, run before Mermaid blocks are swapped for figures.
pub(crate) const FILTER: &str = include_str!("code_catalog.lua");
/// Last pass, run after return-link transformations.
pub(crate) const REPORT_FILTER: &str = include_str!("code_catalog_report.lua");
/// TeX destinations added around each code block.
pub(crate) const HEADER_INCLUDES: &str = include_str!("code_catalog.tex");

pub(crate) const REPORT_METADATA_KEY: &str = "bob-code-report";
/// Explicit Markdown reader for the plain `.md` and `-M` vault-note routes.
/// Pandoc infers `markdown` from the file extension today; the explicit flag
/// also enables bare-URL autolinks so new PDFs make them clickable.
pub(crate) const MARKDOWN_PANDOC_READER: &str = "markdown+autolink_bare_uris";
/// Reader for the web-article and web-collection saved sources, which are
/// pandoc GFM converted from the printed article HTML (no extensions).
pub(crate) const WEB_PANDOC_READER: &str = "gfm";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct CodeSection {
    pub(crate) title: String,
    pub(crate) number: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct CodeBlock {
    pub(crate) n: u32,
    pub(crate) kind: String,
    pub(crate) lang: Option<String>,
    #[serde(default)]
    pub(crate) classes: Vec<String>,
    #[serde(default)]
    pub(crate) attrs: BTreeMap<String, String>,
    pub(crate) text: String,
    pub(crate) section: Option<CodeSection>,
    pub(crate) page: Option<u32>,
    pub(crate) page_end: Option<u32>,
    pub(crate) page_source: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct CatalogLink {
    pub(crate) n: u32,
    pub(crate) url: String,
    pub(crate) text: String,
    pub(crate) title: Option<String>,
    #[serde(default)]
    pub(crate) autolink: bool,
    pub(crate) context: Option<String>,
    pub(crate) section: Option<CodeSection>,
    pub(crate) page: Option<u32>,
    pub(crate) page_source: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct CodeCatalog {
    pub(crate) version: u32,
    #[serde(default)]
    pub(crate) blocks: Vec<CodeBlock>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) links: Option<Vec<CatalogLink>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum CatalogOutcome {
    Catalog(CodeCatalog),
    Missing,
    Invalid(String),
}

#[derive(Debug, Deserialize)]
struct CatalogReport {
    version: u32,
    #[serde(default)]
    blocks: Vec<ReportBlock>,
    #[serde(default)]
    links: Vec<ReportLink>,
}

#[derive(Debug, Deserialize)]
struct ReportBlock {
    n: u32,
    kind: String,
    #[serde(default)]
    lang: Option<String>,
    #[serde(default)]
    classes: Vec<String>,
    #[serde(default)]
    attrs: BTreeMap<String, String>,
    text: String,
    #[serde(default)]
    section: Option<CodeSection>,
}

#[derive(Debug, Deserialize)]
struct ReportLink {
    n: u32,
    url: String,
    #[serde(default)]
    text: String,
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    autolink: bool,
    #[serde(default)]
    context: Option<String>,
    #[serde(default)]
    section: Option<CodeSection>,
}

/// Read the report written by the final Lua filter without mapping pages:
/// for sources verified against an existing PDF (attach), page ranges come
/// from text matching instead of anchors, so every block starts pageless.
pub(crate) fn read_report_catalog(
    report_path: &Path,
) -> Result<CodeCatalog, String> {
    let raw = fs::read_to_string(report_path).map_err(|error| {
        format!(
            "read code catalog report {}: {error}",
            report_path.display()
        )
    })?;
    let report: CatalogReport =
        serde_json::from_str(&raw).map_err(|error| {
            format!(
                "parse code catalog report {}: {error}",
                report_path.display()
            )
        })?;
    if report.version != 1 {
        return Err(format!(
            "unsupported code catalog report version {}",
            report.version
        ));
    }
    Ok(CodeCatalog {
        version: report.version,
        blocks: report
            .blocks
            .into_iter()
            .map(|block| CodeBlock {
                n: block.n,
                kind: block.kind,
                lang: block.lang,
                classes: block.classes,
                attrs: block.attrs,
                text: block.text,
                section: block.section,
                page: None,
                page_end: None,
                page_source: None,
            })
            .collect(),
        links: Some(
            report
                .links
                .into_iter()
                .map(|link| CatalogLink {
                    n: link.n,
                    url: link.url,
                    text: link.text,
                    title: link.title,
                    autolink: link.autolink,
                    context: link.context,
                    section: link.section,
                    page: None,
                    page_source: None,
                })
                .collect(),
        ),
    })
}

/// Attach PDF page ranges to the report written by the final Lua filter.
pub(crate) fn build(report_path: &Path, rendered_pdf: &Path) -> CatalogOutcome {
    let raw = match fs::read_to_string(report_path) {
        Ok(raw) => raw,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return CatalogOutcome::Missing;
        }
        Err(error) => {
            return CatalogOutcome::Invalid(format!(
                "read code catalog report {}: {error}",
                report_path.display()
            ));
        }
    };
    let report: CatalogReport = match serde_json::from_str(&raw) {
        Ok(report) => report,
        Err(error) => {
            return CatalogOutcome::Invalid(format!(
                "parse code catalog report {}: {error}",
                report_path.display()
            ));
        }
    };
    if report.version != 1 {
        return CatalogOutcome::Invalid(format!(
            "unsupported code catalog report version {}",
            report.version
        ));
    }
    let document = match Document::load(rendered_pdf) {
        Ok(document) => document,
        Err(error) => {
            return CatalogOutcome::Invalid(format!(
                "read rendered PDF {} for code catalog: {error}",
                rendered_pdf.display()
            ));
        }
    };
    let destinations = pdf_dests::named_dest_pages(&document);
    let blocks = report
        .blocks
        .into_iter()
        .map(|block| {
            let page =
                destinations.get(&format!("bob-code-{}", block.n)).copied();
            let page_end = destinations
                .get(&format!("bob-code-{}-end", block.n))
                .copied();
            let page_source = (page.is_some() || page_end.is_some())
                .then(|| "anchor".to_string());
            CodeBlock {
                n: block.n,
                kind: block.kind,
                lang: block.lang,
                classes: block.classes,
                attrs: block.attrs,
                text: block.text,
                section: block.section,
                page,
                page_end,
                page_source,
            }
        })
        .collect();
    let links = report
        .links
        .into_iter()
        .map(|link| {
            let page =
                destinations.get(&format!("bob-link-{}", link.n)).copied();
            let page_source = page.map(|_| "anchor".to_string());
            CatalogLink {
                n: link.n,
                url: link.url,
                text: link.text,
                title: link.title,
                autolink: link.autolink,
                context: link.context,
                section: link.section,
                page,
                page_source,
            }
        })
        .collect();
    CatalogOutcome::Catalog(CodeCatalog {
        version: report.version,
        blocks,
        links: Some(links),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use lopdf::{dictionary, Object, Stream};
    use std::{
        path::PathBuf,
        sync::atomic::{AtomicUsize, Ordering},
    };

    static NEXT_TEMP: AtomicUsize = AtomicUsize::new(0);

    struct TempDir(PathBuf);

    impl TempDir {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "bob-code-catalog-{}-{}",
                std::process::id(),
                NEXT_TEMP.fetch_add(1, Ordering::Relaxed)
            ));
            let _ = fs::remove_dir_all(&path);
            fs::create_dir_all(&path).expect("create temporary directory");
            Self(path)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn build_reports_missing_and_invalid_inputs() {
        let temp = TempDir::new();
        let pdf = temp.0.join("render.pdf");
        assert_eq!(
            build(&temp.0.join("missing.json"), &pdf),
            CatalogOutcome::Missing
        );
        let report = temp.0.join("report.json");
        fs::write(&report, "{broken").expect("write report");
        assert!(matches!(build(&report, &pdf), CatalogOutcome::Invalid(_)));
        fs::write(&report, r#"{"version":2,"blocks":[]}"#)
            .expect("write report");
        assert!(matches!(build(&report, &pdf), CatalogOutcome::Invalid(_)));
    }

    #[test]
    fn build_keeps_report_records_without_matching_destinations() {
        let temp = TempDir::new();
        let report = temp.0.join("report.json");
        let pdf = temp.0.join("render.pdf");
        // A one-page PDF without names exercises records with no matching
        // anchors; real destinations are covered by render tests.
        let mut document = Document::with_version("1.4");
        let pages_id = document.new_object_id();
        let content_id =
            document.add_object(Stream::new(dictionary! {}, Vec::new()));
        let page_id = document.add_object(dictionary! {
            "Type" => "Page",
            "Parent" => pages_id,
            "MediaBox" => vec![
                Object::Integer(0),
                Object::Integer(0),
                Object::Integer(612),
                Object::Integer(792),
            ],
            "Contents" => content_id,
        });
        document.set_object(
            pages_id,
            dictionary! {
                "Type" => "Pages",
                "Kids" => vec![Object::Reference(page_id)],
                "Count" => 1,
            },
        );
        let catalog_id = document.add_object(dictionary! {
            "Type" => "Catalog",
            "Pages" => pages_id,
        });
        document.trailer.set("Root", catalog_id);
        document.save(&pdf).expect("write fixture PDF");
        fs::write(
            &report,
            r#"{"version":1,"blocks":[{"n":1,"kind":"code","lang":"rust","classes":["rust"],"attrs":{},"text":"fn main() {}","section":{"title":"Start","number":"1"}}]}"#,
        )
        .expect("write report");
        let CatalogOutcome::Catalog(catalog) = build(&report, &pdf) else {
            panic!("valid report should build");
        };
        assert_eq!(catalog.version, 1);
        assert_eq!(catalog.blocks.len(), 1);
        assert_eq!(catalog.blocks[0].text, "fn main() {}");
        assert_eq!(
            catalog.blocks[0]
                .section
                .as_ref()
                .unwrap()
                .number
                .as_deref(),
            Some("1")
        );
        assert_eq!(catalog.blocks[0].page, None);
        assert_eq!(catalog.blocks[0].page_end, None);
        assert_eq!(catalog.blocks[0].page_source, None);
    }
}
