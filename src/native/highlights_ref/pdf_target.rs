//! Stamp-as-is PDF route for `create` (local, PDF URL, arXiv).
//!
//! A PDF is never re-rendered: bob copies it, stamps the page-1 marker,
//! sets the document Info Title/Author, verifies the marker round-trips,
//! then installs it into `xlib/<ref-type>/<stem>.pdf`. `scan` writes the
//! ref note.

use std::{
    fs,
    path::{Path, PathBuf},
};

use super::arxiv::{author_display, ArxivMetadata, ArxivPaper};
use super::clip_url::{
    humanize_stem, short_title_stem, snake_case, stem_from_url, validate_name,
    WebUrl,
};
use super::marker::{parse_marker, read_pdf_marker};
use super::pdf_meta::pdf_info_metadata;
use super::workdir::ScratchDir;
use super::{
    atomic_copy, atomic_save_pdf, compose_marker, embed_marker,
    set_pdf_info_for_route, CommandError, Config, MarkerValue, PdfInfo, Result,
};

/// 95 MiB vault-sync refusal limit.
pub(super) const PDF_MAX_BYTES: u64 = 95 * 1024 * 1024;
/// Warning threshold.
const PDF_WARN_BYTES: u64 = 50 * 1024 * 1024;

/// One resolved PDF target: stem, title, author, and marker extras.
#[derive(Debug, Clone)]
pub(super) struct PdfPlan {
    pub(super) stem: String,
    pub(super) title: String,
    pub(super) author: Option<String>,
    pub(super) published: Option<String>,
    pub(super) source_url: Option<String>,
    pub(super) captured: Option<String>,
    /// Provenance tag for dry runs: override, arxiv api, pdf info, filename.
    pub(super) title_source: &'static str,
    pub(super) author_source: Option<&'static str>,
    pub(super) published_source: Option<&'static str>,
}

/// Plan a local PDF target.
pub(super) fn plan_local_pdf(
    source: &Path,
    name_override: Option<&str>,
    title_override: Option<&str>,
    author_override: Option<&str>,
    published_override: Option<&str>,
    progress: Option<&dyn Fn(&str)>,
) -> Result<PdfPlan> {
    validate_pdf_file(source, progress)?;
    let file_stem = source
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or_default()
        .to_string();
    let stem = match name_override {
        Some(name) => validate_name(name)?,
        None => match validate_name(&file_stem) {
            Ok(valid) => valid,
            Err(_) => {
                let slug = snake_case(&file_stem);
                if slug.is_empty() {
                    return Err(CommandError::new(format!(
                        "could not derive a filename stem from {}",
                        source.display()
                    )));
                }
                slug
            }
        },
    };
    // Reject an Info title that equals the source file stem, not the
    // final (possibly `-N`/snake-cased) stem.
    let (info_title, info_author) = pdf_info_metadata(source, &file_stem);
    let title;
    let title_source;
    if let Some(override_title) = title_override.filter(|t| !t.is_empty()) {
        title = override_title.to_string();
        title_source = "override";
    } else if let Some(info) = info_title {
        title = info;
        title_source = "pdf info";
    } else {
        title = humanize_stem(&stem);
        title_source = "filename";
    }
    let (author, author_source) =
        match author_override.filter(|value| !value.is_empty()) {
            Some(override_author) => {
                (Some(override_author.to_string()), Some("override"))
            }
            None => {
                let source = info_author.as_ref().map(|_| "pdf info");
                (info_author, source)
            }
        };
    let (published, published_source) =
        match published_override.filter(|value| !value.is_empty()) {
            Some(override_published) => {
                (Some(override_published.to_string()), Some("override"))
            }
            None => (None, None),
        };
    Ok(PdfPlan {
        stem,
        title,
        author,
        published,
        source_url: None,
        captured: None,
        title_source,
        author_source,
        published_source,
    })
}

/// Plan a PDF-URL target (`downloaded` is the fetched body in scratch).
pub(super) fn plan_pdf_url(
    url: &WebUrl,
    downloaded: &Path,
    name_override: Option<&str>,
    title_override: Option<&str>,
    author_override: Option<&str>,
    published_override: Option<&str>,
    captured: &str,
    progress: Option<&dyn Fn(&str)>,
) -> Result<PdfPlan> {
    validate_pdf_file(downloaded, progress)?;
    let stem = match name_override {
        Some(name) => validate_name(name)?,
        None => {
            if let Some(from_url) = stem_from_url(url) {
                from_url
            } else {
                // Fall back to the short-title stem of the resolved title,
                // then to <host>_<YYYYMMDD>.
                let (info_title, _) = pdf_info_metadata(downloaded, "");
                let candidate = title_override
                    .filter(|t| !t.is_empty())
                    .map(|t| t.to_string())
                    .or(info_title)
                    .unwrap_or_default();
                let short = short_title_stem(&candidate);
                if !short.is_empty() {
                    short
                } else {
                    format!(
                        "{}_{}",
                        url.host.replace('.', "_").replace('-', "_"),
                        captured.replace('-', "")
                    )
                }
            }
        }
    };
    // Reject an Info title that equals the URL stem, not the final
    // (possibly `-N`-overridden) stem.
    let url_stem = stem_from_url(url).unwrap_or_default();
    let (info_title, info_author) = pdf_info_metadata(downloaded, &url_stem);
    let title;
    let title_source;
    if let Some(override_title) = title_override.filter(|t| !t.is_empty()) {
        title = override_title.to_string();
        title_source = "override";
    } else if let Some(info) = info_title {
        title = info;
        title_source = "pdf info";
    } else {
        title = humanize_stem(&stem);
        title_source = "filename";
    }
    let (author, author_source) =
        match author_override.filter(|value| !value.is_empty()) {
            Some(override_author) => {
                (Some(override_author.to_string()), Some("override"))
            }
            None => {
                let source = info_author.as_ref().map(|_| "pdf info");
                (info_author, source)
            }
        };
    let (published, published_source) =
        match published_override.filter(|value| !value.is_empty()) {
            Some(override_published) => {
                (Some(override_published.to_string()), Some("override"))
            }
            None => (None, None),
        };
    Ok(PdfPlan {
        stem,
        title,
        author,
        published,
        source_url: Some(url.cleaned.clone()),
        captured: Some(captured.to_string()),
        title_source,
        author_source,
        published_source,
    })
}

/// Plan an arXiv target.
pub(super) fn plan_arxiv(
    paper: &ArxivPaper,
    name_override: Option<&str>,
    title_override: Option<&str>,
    author_override: Option<&str>,
    published_override: Option<&str>,
    metadata: Option<&ArxivMetadata>,
    downloaded: &Path,
    captured: &str,
    progress: Option<&dyn Fn(&str)>,
) -> Result<PdfPlan> {
    validate_pdf_file(downloaded, progress)?;
    let stem = match name_override {
        Some(name) => validate_name(name)?,
        None => {
            if let Some(meta) = metadata {
                let short = short_title_stem(&meta.title);
                if !short.is_empty() {
                    short
                } else {
                    arxiv_fallback_stem(&paper.id)
                }
            } else {
                // Without API metadata, try the PDF Info title's short
                // stem before the arxiv_<id> fallback.
                let (info_title, _) = pdf_info_metadata(downloaded, "");
                if let Some(info) = info_title {
                    let short = short_title_stem(&info);
                    if !short.is_empty() {
                        short
                    } else {
                        arxiv_fallback_stem(&paper.id)
                    }
                } else {
                    arxiv_fallback_stem(&paper.id)
                }
            }
        }
    };
    let (info_title, info_author) = pdf_info_metadata(downloaded, &stem);
    let title;
    let title_source;
    if let Some(override_title) = title_override.filter(|t| !t.is_empty()) {
        title = override_title.to_string();
        title_source = "override";
    } else if let Some(meta) = metadata {
        title = meta.title.clone();
        title_source = "arxiv api";
    } else if let Some(info) = info_title.clone() {
        title = info;
        title_source = "pdf info";
    } else {
        title = format!("arXiv {}", paper.id);
        title_source = "filename";
    }
    let (derived_author, derived_author_source) = match metadata
        .map(|meta| author_display(&meta.authors))
        .filter(|display| !display.is_empty())
    {
        Some(display) => (Some(display), Some("arxiv api")),
        None => {
            let source = info_author.as_ref().map(|_| "pdf info");
            (info_author, source)
        }
    };
    let derived_published = metadata.map(|meta| meta.published.clone());
    let (derived_published_source, has_derived_published) =
        match &derived_published {
            Some(_) => (Some("arxiv api"), true),
            None => (None, false),
        };
    let (author, author_source) =
        match author_override.filter(|value| !value.is_empty()) {
            Some(override_author) => {
                (Some(override_author.to_string()), Some("override"))
            }
            None => (derived_author, derived_author_source),
        };
    let (published, published_source) =
        match published_override.filter(|value| !value.is_empty()) {
            Some(override_published) => {
                (Some(override_published.to_string()), Some("override"))
            }
            None => match has_derived_published {
                true => (derived_published, derived_published_source),
                false => (None, None),
            },
        };
    Ok(PdfPlan {
        stem,
        title,
        author,
        published,
        source_url: Some(paper.abs_url()),
        captured: Some(captured.to_string()),
        title_source,
        author_source,
        published_source,
    })
}

fn arxiv_fallback_stem(full_id: &str) -> String {
    format!("arxiv_{}", full_id.replace('.', "_").replace('/', "_"))
}

/// Validate a PDF file: size cap, loadable, at least one page, not
/// encrypted. Files of 50 MiB or more report a warning through
/// `progress`; `None` drops it (ingest stays silent when it has no
/// progress reporter, `bob ref create` passes an eprintln reporter).
pub(super) fn validate_pdf_file(
    path: &Path,
    progress: Option<&dyn Fn(&str)>,
) -> Result<()> {
    let bytes = fs::metadata(path)
        .map(|metadata| metadata.len())
        .unwrap_or(0);
    if bytes > PDF_MAX_BYTES {
        return Err(CommandError::new(format!(
            "PDF larger than 95 MiB ({} bytes): {}",
            bytes,
            path.display()
        )));
    }
    if bytes >= PDF_WARN_BYTES
        && let Some(report) = progress
    {
        report(&format!(
            "warning: PDF is {} bytes (>= 50 MiB); vault sync may be slow: {}",
            bytes,
            path.display()
        ));
    }
    let document = lopdf::Document::load(path).map_err(|error| {
        // `lopdf` reports junk magic as a parse error; surface it as a
        // non-PDF refusal.
        let message = error.to_string();
        if message.contains("Invalid PDF")
            || message.contains("missing")
            || message.contains("header")
        {
            CommandError::new(format!(
                "TARGET must be a Markdown file (.md), a PDF file, or an http(s) URL: {}",
                path.display()
            ))
        } else {
            CommandError::new(format!(
                "read PDF {}: {error}",
                path.display()
            ))
        }
    })?;
    if document.is_encrypted() {
        return Err(CommandError::new(
            "PDF is encrypted; Highlights markers cannot be embedded — save an unencrypted copy and pass that file",
        ));
    }
    if document.get_pages().is_empty() {
        return Err(CommandError::new(format!(
            "PDF has no pages: {}",
            path.display()
        )));
    }
    Ok(())
}

/// True when the PDF already carries a Highlights marker (already
/// captured). Any other page-1 note is kept.
pub(super) fn pdf_already_captured(path: &Path) -> bool {
    let Ok(marker) = read_pdf_marker(path) else {
        return false;
    };
    parse_marker(&marker.contents).is_ok()
}

/// Refuse a PDF that already carries a Highlights marker: out-of-vault
/// sources and downloaded bodies are already captured upstream. Refuses
/// before any write, naming the marked file and hinting at the library
/// copy.
pub(super) fn refuse_marked_pdf(
    marked: &Path,
    library_pdf: Option<&Path>,
) -> Result<()> {
    if !pdf_already_captured(marked) {
        return Ok(());
    }
    let hint = match library_pdf {
        Some(library) => format!(
            "bob ref sync {} re-syncs it, or bob ref create {} --listen to add audio",
            marked.display(),
            library.display()
        ),
        None => format!(
            "bob ref sync {} re-syncs it, or bob ref create <library PDF> --listen to add audio",
            marked.display()
        ),
    };
    Err(CommandError::new(format!(
        "PDF already carries a Highlights marker: {}\nhint: {hint}",
        marked.display()
    )))
}

/// Refuse a local PDF inside the library/intake: a marked one is an
/// existing capture, an unmarked one must be moved out first. Uses one
/// shared canonical containment helper so relative (`-b vault`) and
/// symlinked vaults still match.
pub(super) fn check_local_pdf_identity(
    config: &Config,
    source: &Path,
) -> Result<()> {
    let Ok(canonical) = fs::canonicalize(source) else {
        return Ok(());
    };
    let inside = super::relative_inside_canonical(&canonical, &config.lib_dir)
        .is_some()
        || super::relative_inside_canonical(&canonical, &config.xlib_dir)
            .is_some()
        || super::path_is_inside_canonical(source, &config.lib_dir)
        || super::path_is_inside_canonical(source, &config.xlib_dir);
    if !inside {
        return Ok(());
    }
    if pdf_already_captured(&canonical) {
        return Err(CommandError::new(format!(
            "already captured; bob ref sync {} re-syncs it\nhint: add --listen to narrate it and attach the episode to the existing capture",
            canonical.display()
        )));
    }
    Err(CommandError::new(format!(
        "{} is inside the Highlights library/intake but has no marker; move it out and rerun create on it",
        canonical.display()
    )))
}

/// Stamp `source` into `<scratch>/stamped.pdf`, verify the marker
/// round-trips and the page count is unchanged, and return the stamped
/// path plus the page count.
pub(super) fn stamp_pdf_to_scratch(
    source: &Path,
    scratch: &ScratchDir,
    marker: &str,
    info: &PdfInfo,
) -> Result<(PathBuf, usize)> {
    let mut document = lopdf::Document::load(source).map_err(|error| {
        CommandError::new(format!("read PDF {}: {error}", source.display()))
    })?;
    let page_count = document.get_pages().len();
    embed_marker(&mut document, marker)?;
    if info.title.is_some() || info.author.is_some() {
        set_pdf_info_for_route(&mut document, info)?;
    }
    let stamped = scratch.path().join("stamped.pdf");
    atomic_save_pdf(&stamped, &mut document)?;
    // Verify: reload, check the marker round-trips and the page count is
    // unchanged.
    let reloaded = lopdf::Document::load(&stamped).map_err(|error| {
        CommandError::new(format!(
            "verify stamped PDF {}: {error}",
            stamped.display()
        ))
    })?;
    if reloaded.get_pages().len() != page_count {
        return Err(CommandError::new(format!(
            "stamped PDF page count changed for {}",
            source.display()
        )));
    }
    let marker_back = read_pdf_marker(&stamped).map_err(|error| {
        CommandError::new(format!(
            "verify stamped PDF marker for {}: {error}",
            source.display()
        ))
    })?;
    let parsed = parse_marker(&marker_back.contents).map_err(|error| {
        CommandError::new(format!(
            "verify stamped PDF marker for {}: {error}",
            source.display()
        ))
    })?;
    let expected = parse_marker(marker).map_err(|error| {
        CommandError::new(format!("verify composed marker: {error}"))
    })?;
    if parsed != expected {
        return Err(CommandError::new(format!(
            "stamped PDF marker did not round-trip for {}",
            source.display()
        )));
    }
    Ok((stamped, page_count))
}

/// Compose the marker for a PDF route.
pub(super) fn compose_pdf_marker(
    status: &str,
    parent: &str,
    plan: &PdfPlan,
    id: Option<&str>,
) -> Result<String> {
    let mut extras: Vec<(&str, MarkerValue)> = Vec::new();
    if let Some(source_url) = &plan.source_url {
        extras.push(("source_url", MarkerValue::String(source_url.clone())));
    }
    if let Some(author) = &plan.author {
        extras.push(("author", MarkerValue::String(author.clone())));
    }
    if let Some(published) = &plan.published {
        extras.push(("published", MarkerValue::String(published.clone())));
    }
    if let Some(captured) = &plan.captured {
        extras.push(("captured", MarkerValue::String(captured.clone())));
    }
    compose_marker(status, parent, &plan.title, id, &extras)
}

/// Install a stamped scratch PDF plus its companion audio.
pub(super) fn install_stamped_pdf(
    stamped: &Path,
    target: &Path,
    audio: Option<&super::companion::AudioCopyPlan>,
    page_count: usize,
) -> Result<usize> {
    let created = super::companion::copy_audio_for_install(audio)?;
    let installed = atomic_copy(stamped, target);
    if let Err(error) = installed {
        super::companion::cleanup_audio_on_failure(created.as_ref());
        return Err(error);
    }
    Ok(page_count)
}
