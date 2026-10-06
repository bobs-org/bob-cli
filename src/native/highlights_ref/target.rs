//! `create` TARGET classification.
//!
//! Resolution is syntactic first and needs no network until the URL
//! fetch step: `http(s)` URLs are validated, local paths are checked
//! for existence, and only generic (non-arXiv) URLs are fetched into
//! scratch to decide between the PDF-URL and article routes.

use std::{
    fs,
    path::{Path, PathBuf},
};

use super::arxiv::ArxivPaper;
use super::clip_url::{validate_and_clean, WebUrl};
use super::fetch::{fetch_url, FetchResult};
use super::workdir::ScratchDir;
use super::{CommandError, Result};

/// Every `create` target kind.
#[derive(Debug, Clone)]
pub(super) enum CreateSource {
    /// Existing local Markdown file.
    Markdown(PathBuf),
    /// Existing local PDF file.
    LocalPdf(PathBuf),
    /// A generic URL that fetched as a PDF (downloaded body in scratch).
    PdfUrl {
        url: WebUrl,
        downloaded: PathBuf,
        fetch: FetchResult,
    },
    /// An arXiv paper URL (no fetch yet; the PDF fetch uses
    /// [`ArxivPaper::pdf_url`]).
    Arxiv { paper: ArxivPaper, url: WebUrl },
    /// A web article URL (clip engine; next phase).
    WebArticle { url: WebUrl },
}

/// True when `raw` starts with `http://` or `https://`
/// (case-insensitive).
pub(super) fn looks_like_url(raw: &str) -> bool {
    let lower = raw.trim().to_lowercase();
    lower.starts_with("http://") || lower.starts_with("https://")
}

/// Classify one `TARGET` string. Generic URLs are fetched into `scratch`
/// to decide between the PDF-URL and article routes.
pub(super) fn resolve_target(
    raw: &str,
    scratch: &ScratchDir,
) -> Result<CreateSource> {
    if looks_like_url(raw) {
        let url = validate_and_clean(raw)?;
        return resolve_url_target(url, scratch);
    }
    resolve_local_target(Path::new(raw))
}

/// Classify a local path target.
pub(super) fn resolve_local_target(path: &Path) -> Result<CreateSource> {
    if !path.is_file() {
        return Err(CommandError::new(format!(
            "TARGET does not exist or is not a file: {} (URLs must start with http:// or https://)",
            path.display()
        )));
    }
    let extension = path
        .extension()
        .and_then(|ext| ext.to_str())
        .unwrap_or_default();
    if extension.eq_ignore_ascii_case("md") {
        return Ok(CreateSource::Markdown(path.to_path_buf()));
    }
    if extension.eq_ignore_ascii_case("pdf") || file_starts_with_pdf(path) {
        return Ok(CreateSource::LocalPdf(path.to_path_buf()));
    }
    Err(CommandError::new(format!(
        "TARGET must be a Markdown file (.md), a PDF file, or an http(s) URL: {}",
        path.display()
    )))
}

/// Classify a validated URL: arXiv URLs go straight to the arXiv route,
/// every other URL is fetched into scratch.
fn resolve_url_target(
    url: WebUrl,
    scratch: &ScratchDir,
) -> Result<CreateSource> {
    if let Some(paper) = ArxivPaper::parse(&url.cleaned) {
        return Ok(CreateSource::Arxiv { paper, url });
    }
    fetch_and_route(url, scratch)
}

/// Validate a URL string syntactically (no fetch). Returns the cleaned URL
/// plus its arXiv identity, if any.
pub(super) fn resolve_url_syntactic(
    raw: &str,
) -> Result<(WebUrl, Option<ArxivPaper>)> {
    let url = validate_and_clean(raw)?;
    let paper = ArxivPaper::parse(&url.cleaned);
    Ok((url, paper))
}

/// Fetch a generic URL into scratch and route it by status/content-type.
pub(super) fn fetch_and_route(
    url: WebUrl,
    scratch: &ScratchDir,
) -> Result<CreateSource> {
    let dest = scratch.path().join("download");
    let fetch = fetch_url(&url.cleaned, &dest, 30).map_err(|error| {
        let mut message = error.message().to_string();
        if let Some(hint) = error.hint() {
            message.push_str(&format!(" (hint: {hint})"));
        }
        CommandError::new(message)
    })?;
    let status = fetch.status;
    let content_type = fetch.content_type.clone();
    let normalized = normalize_content_type(&content_type);

    // Bot-wall statuses route to the article engine even when the body
    // claims otherwise.
    if matches!(status, 403 | 429 | 503) {
        return Ok(CreateSource::WebArticle { url });
    }
    if !(200..300).contains(&status) {
        return Err(CommandError::new(format!(
            "server returned HTTP {status} for {}",
            url.cleaned
        )));
    }

    match normalized.as_str() {
        "application/pdf" | "application/x-pdf" => {
            if !file_starts_with_pdf(&fetch.path) {
                return Err(CommandError::new(format!(
                    "server said PDF but sent something else: {}",
                    url.cleaned
                )));
            }
            Ok(CreateSource::PdfUrl {
                url,
                downloaded: fetch.path.clone(),
                fetch,
            })
        }
        "text/html" | "application/xhtml+xml" => {
            Ok(CreateSource::WebArticle { url })
        }
        "application/octet-stream" | "binary/octet-stream" | "" => {
            if file_starts_with_pdf(&fetch.path) {
                Ok(CreateSource::PdfUrl {
                    url,
                    downloaded: fetch.path.clone(),
                    fetch,
                })
            } else if normalized.is_empty() {
                Err(CommandError::new(format!(
                    "unsupported content type (missing): create accepts Markdown files, PDFs, PDF URLs, arXiv paper URLs, and web article URLs"
                )))
            } else {
                Err(CommandError::new(format!(
                    "unsupported content type {content_type}: create accepts Markdown files, PDFs, PDF URLs, arXiv paper URLs, and web article URLs"
                )))
            }
        }
        _ => Err(CommandError::new(format!(
            "unsupported content type {content_type}: create accepts Markdown files, PDFs, PDF URLs, arXiv paper URLs, and web article URLs"
        ))),
    }
}

/// Lowercase content type without parameters (`; charset=…`).
fn normalize_content_type(raw: &str) -> String {
    raw.split(';').next().unwrap_or("").trim().to_lowercase()
}

/// True when the first 1024 bytes contain `%PDF-`.
pub(super) fn file_starts_with_pdf(path: &Path) -> bool {
    let Ok(bytes) = fs::read(path) else {
        return false;
    };
    let head = &bytes[..bytes.len().min(1024)];
    head.windows(5).any(|window| window == b"%PDF-")
}
