//! Stored identity keys and query classification.
//!
//! URL keys reuse the `highlights_ref` cleaning (already arXiv-aware) and add
//! `arxiv:<id>` / `doi:<doi>` keys plus opaque `raw:` keys for values that
//! fail validation. Query classification follows the plan order: url, arxiv,
//! doi, path, name, title.
use regex::Regex;
use std::sync::OnceLock;

use crate::native::highlights_ref::{validate_and_clean, ArxivPaper};

/// Stored identity for one row's `source_url` + `url` values.
#[derive(Debug, Clone, Default)]
pub(crate) struct StoredIdentity {
    pub keys: Vec<String>,
    pub arxiv: Option<String>,
    pub doi: Option<String>,
    /// Values that failed URL validation (each earns an `opaque_url`).
    pub opaque: Vec<String>,
}

/// Derive identity keys from `source_url` values followed by `url` values.
pub(crate) fn stored_identity(urls: &[String]) -> StoredIdentity {
    let mut identity = StoredIdentity::default();
    for raw in urls {
        let value = raw.trim();
        if value.is_empty() {
            continue;
        }
        match validate_and_clean(value) {
            Ok(cleaned) => {
                push_unique(&mut identity.keys, cleaned.dedupe_key);
                if identity.arxiv.is_none()
                    && let Some(paper) = ArxivPaper::parse(value)
                {
                    identity.arxiv = Some(paper.id.clone());
                    push_unique(
                        &mut identity.keys,
                        format!("arxiv:{}", paper.id),
                    );
                }
                if let Some(doi) = doi_from_url(value) {
                    if identity.doi.is_none() {
                        identity.doi = Some(doi.clone());
                    }
                    push_unique(&mut identity.keys, format!("doi:{doi}"));
                    if identity.arxiv.is_none()
                        && let Some(arxiv_id) = arxiv_id_from_doi(&doi)
                    {
                        identity.arxiv = Some(arxiv_id.clone());
                        push_unique(
                            &mut identity.keys,
                            format!("arxiv:{arxiv_id}"),
                        );
                    }
                }
            }
            Err(_) => {
                push_unique(&mut identity.keys, raw_identity_key(value));
                identity.opaque.push(value.to_string());
            }
        }
    }
    identity
}

/// The opaque key for a value that fails URL validation: lowercased,
/// trimmed, scheme-less, and trailing-slash-trimmed.
pub(crate) fn raw_identity_key(value: &str) -> String {
    let mut key = value.trim().to_lowercase();
    if let Some(rest) = key
        .strip_prefix("https://")
        .or_else(|| key.strip_prefix("http://"))
    {
        key = rest.to_string();
    }
    while key.ends_with('/') && key.len() > 1 {
        key.pop();
    }
    format!("raw:{key}")
}

/// A classified library query.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum QueryKind {
    Url,
    Arxiv,
    Doi,
    Path,
    Name,
    Title,
}

/// Classify one raw query string, in plan order.
pub(crate) fn classify_query(raw: &str) -> QueryKind {
    let query = raw.trim();
    let lower = query.to_lowercase();
    if lower.starts_with("http://") || lower.starts_with("https://") {
        return QueryKind::Url;
    }
    if parse_arxiv_query(query).is_some() {
        return QueryKind::Arxiv;
    }
    if normalize_doi_query(query).is_some() {
        return QueryKind::Doi;
    }
    if query.contains('/')
        || query.to_lowercase().ends_with(".md")
        || query.to_lowercase().ends_with(".pdf")
        || query.starts_with('/')
    {
        return QueryKind::Path;
    }
    if !query.is_empty() && !query.chars().any(char::is_whitespace) {
        return QueryKind::Name;
    }
    QueryKind::Title
}

/// The versionless arXiv base id for a bare id or `arxiv:`-prefixed query,
/// or `None` when the query is not an arXiv id. The id grammar mirrors the
/// canonical [`ArxivPaper`] path grammar (new style plus old archive style,
/// with an optional `vN` suffix that is stripped).
pub(crate) fn parse_arxiv_query(query: &str) -> Option<String> {
    let trimmed = query.trim();
    let bare = trimmed
        .get(..6)
        .filter(|prefix| prefix.eq_ignore_ascii_case("arxiv:"))
        .map(|_| trimmed[6..].trim())
        .unwrap_or(trimmed);
    if bare.is_empty() || bare.contains(char::is_whitespace) {
        return None;
    }
    let (id, version) = split_arxiv_version(bare)?;
    if !(is_new_style_arxiv_id(&id) || is_old_style_arxiv_id(&id)) {
        return None;
    }
    // The tail after the base id must be empty or a bare `vN` suffix;
    // anything else (a `.pdf` suffix, a path) is not an arXiv id query.
    let tail = &bare[id.len()..];
    let version_ok = match version {
        None => tail.is_empty(),
        Some(_) => {
            tail.len() > 1
                && tail.starts_with('v')
                && tail[1..].chars().all(|c| c.is_ascii_digit())
        }
    };
    version_ok.then_some(id)
}

/// The lowercased DOI for a bare or `doi:`-prefixed query.
pub(crate) fn normalize_doi_query(query: &str) -> Option<String> {
    let trimmed = query.trim();
    let bare = trimmed
        .get(..4)
        .filter(|prefix| prefix.eq_ignore_ascii_case("doi:"))
        .map(|_| trimmed[4..].trim())
        .unwrap_or(trimmed);
    let bare = bare.trim();
    if bare.contains(char::is_whitespace) {
        return None;
    }
    doi_regex().is_match(bare).then(|| bare.to_lowercase())
}

fn doi_regex() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| {
        Regex::new(r"^10\.\d{4,9}/\S+$").expect("valid DOI grammar")
    })
}

fn is_new_style_arxiv_id(id: &str) -> bool {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX
        .get_or_init(|| {
            Regex::new(r"^\d{4}\.\d{4,5}$").expect("valid arXiv grammar")
        })
        .is_match(id)
}

fn is_old_style_arxiv_id(id: &str) -> bool {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX
        .get_or_init(|| {
            Regex::new(r"(?i)^[a-z]+(?:-[a-z]+)*(?:\.[a-z]{2})?/\d{7}$")
                .expect("valid legacy arXiv grammar")
        })
        .is_match(id)
}

/// Split a full arXiv id into its base id and optional `vN` suffix.
fn split_arxiv_version(full_id: &str) -> Option<(String, Option<String>)> {
    if full_id.is_empty() {
        return None;
    }
    let bytes = full_id.as_bytes();
    let mut digits_start = bytes.len();
    while digits_start > 0 && bytes[digits_start - 1].is_ascii_digit() {
        digits_start -= 1;
    }
    if digits_start >= 2
        && digits_start < bytes.len()
        && bytes[digits_start - 1] == b'v'
    {
        Some((
            full_id[..digits_start - 1].to_string(),
            Some(full_id[digits_start - 1..].to_string()),
        ))
    } else {
        Some((full_id.to_string(), None))
    }
}

/// Extract a `doi.org` / `dx.doi.org` DOI from a stored URL value.
fn doi_from_url(value: &str) -> Option<String> {
    let url = url::Url::parse(value.trim()).ok()?;
    if !matches!(url.scheme(), "http" | "https") {
        return None;
    }
    let host = url.host_str()?.to_lowercase();
    let host = host.strip_prefix("www.").unwrap_or(&host);
    if !matches!(host, "doi.org" | "dx.doi.org") {
        return None;
    }
    let path = url.path().trim_matches('/');
    if path.is_empty() {
        return None;
    }
    let decoded = percent_decode(path).to_lowercase();
    doi_regex().is_match(&decoded).then_some(decoded)
}

/// The arXiv id behind an arXiv DOI of the form `10.48550/arXiv.<id>`.
fn arxiv_id_from_doi(doi: &str) -> Option<String> {
    let rest = doi.strip_prefix("10.48550/arxiv.")?;
    if rest.is_empty() || rest.contains('/') || rest.contains(' ') {
        return None;
    }
    let (base, _) = split_arxiv_version(rest)?;
    if is_new_style_arxiv_id(&base) || is_old_style_arxiv_id(&base) {
        Some(base)
    } else {
        None
    }
}

fn percent_decode(segment: &str) -> String {
    let mut decoded = Vec::with_capacity(segment.len());
    let bytes = segment.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%'
            && index + 2 < bytes.len()
            && let (Some(high), Some(low)) =
                (hex_value(bytes[index + 1]), hex_value(bytes[index + 2]))
        {
            decoded.push(high << 4 | low);
            index += 3;
        } else {
            decoded.push(bytes[index]);
            index += 1;
        }
    }
    String::from_utf8_lossy(&decoded).into_owned()
}

fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

fn push_unique(keys: &mut Vec<String>, key: String) {
    if !keys.contains(&key) {
        keys.push(key);
    }
}
