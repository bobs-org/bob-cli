//! arXiv paper identity and API metadata for Highlights targets.
//!
//! URL recognition mirrors `sase-listen`'s `web/arxiv.py` exactly: the same
//! hosts, the same path grammar, and the same version handling. The fetch
//! always goes to `https://arxiv.org/pdf/<id>` and never falls back to the
//! abstract page. `arxiv:ID` and bare ids are out of scope: sase-listen
//! rejects them with `-e full`, and so do we.

use std::{fs, path::Path, sync::OnceLock};

use regex::Regex;

use super::fetch::fetch_url;

/// An arXiv paper: the base identifier plus an optional version suffix.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ArxivPaper {
    /// The identifier without its version, e.g. `2602.16844`.
    pub(crate) id: String,
    /// The version suffix, e.g. `v2`.
    pub(crate) version: Option<String>,
}

impl ArxivPaper {
    /// Parse an arXiv paper URL (`abs`, `html`, or `pdf`), keeping the
    /// version. Query strings and fragments are ignored. Returns `None`
    /// for anything else.
    pub(crate) fn parse(raw: &str) -> Option<Self> {
        let url = url::Url::parse(raw.trim()).ok()?;
        if !matches!(url.scheme(), "http" | "https") {
            return None;
        }
        let host = url.host_str()?.to_lowercase();
        if !matches!(
            host.as_str(),
            "arxiv.org" | "www.arxiv.org" | "export.arxiv.org"
        ) {
            return None;
        }
        let captures = arxiv_path_regex().captures(url.path())?;
        let kind = &captures[1];
        let full_id = captures[2].to_string();
        if captures.get(3).is_some() && kind != "pdf" {
            return None;
        }
        let (id, version) = split_version(&full_id);
        Some(Self { id, version })
    }

    /// The full identifier with its version, e.g. `2602.16844v2`.
    pub(super) fn full_id(&self) -> String {
        match &self.version {
            Some(version) => format!("{}{}", self.id, version),
            None => self.id.clone(),
        }
    }

    /// The PDF fetch URL: `https://arxiv.org/pdf/<id>[vN]`.
    pub(super) fn pdf_url(&self) -> String {
        format!("https://arxiv.org/pdf/{}", self.full_id())
    }

    /// The landing page: `https://arxiv.org/abs/<id>[vN]`, which is the
    /// stored `source_url`.
    pub(super) fn abs_url(&self) -> String {
        format!("https://arxiv.org/abs/{}", self.full_id())
    }

    /// The dedupe key: `https://arxiv.org/abs/<id>` with no version, so
    /// every spelling of a paper (abs, html, or pdf, with or without a
    /// version) dedupes together.
    pub(crate) fn dedupe_key(&self) -> String {
        format!("https://arxiv.org/abs/{}", self.id)
    }
}

/// The path grammar from `sase-listen`'s `web/arxiv.py`: `/(abs|html|pdf)/`
/// plus a new-style (`2602.16844[v2]`) or old-style (`hep-th/9901001[v2]`)
/// id, with an optional `.pdf` suffix (allowed only on `/pdf/`) and an
/// optional trailing slash.
fn arxiv_path_regex() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| {
        Regex::new(
            r"^/(abs|html|pdf)/(\d{4}\.\d{4,5}(?:v\d+)?|[a-z]+(?:-[a-z]+)*(?:\.[A-Za-z]{2})?/\d{7}(?:v\d+)?)(\.pdf)?/?$",
        )
        .expect("valid arXiv path grammar")
    })
}

/// Split a full identifier into its base id and version suffix (`vN`).
fn split_version(full_id: &str) -> (String, Option<String>) {
    let bytes = full_id.as_bytes();
    let mut digits_start = bytes.len();
    while digits_start > 0 && bytes[digits_start - 1].is_ascii_digit() {
        digits_start -= 1;
    }
    if digits_start >= 2
        && digits_start < bytes.len()
        && bytes[digits_start - 1] == b'v'
    {
        (
            full_id[..digits_start - 1].to_string(),
            Some(full_id[digits_start - 1..].to_string()),
        )
    } else {
        (full_id.to_string(), None)
    }
}

/// Title, authors, and first-version date from the arXiv API.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ArxivMetadata {
    pub(super) title: String,
    pub(super) authors: Vec<String>,
    /// The date part of the API `<published>` (the first version).
    pub(super) published: String,
}

/// Display authors as `A`, `A and B`, `A, B, and C`, or `A et al.`.
pub(super) fn author_display(authors: &[String]) -> String {
    match authors {
        [] => String::new(),
        [only] => only.clone(),
        [first, second] => format!("{first} and {second}"),
        [first, second, third] => format!("{first}, {second}, and {third}"),
        [first, ..] => format!("{first} et al."),
    }
}

/// Fetch API metadata for `paper` into `dir`, degrading to `None` with a
/// warning on any network or parse failure. A failure here is never
/// fatal: the caller falls back to PDF Info metadata.
pub(super) fn fetch_metadata(
    paper: &ArxivPaper,
    dir: &Path,
) -> (Option<ArxivMetadata>, Option<String>) {
    let query = format!(
        "https://export.arxiv.org/api/query?id_list={}",
        paper.full_id()
    );
    let dest = dir.join("arxiv-api.xml");
    if let Err(error) = fetch_url(&query, &dest, 20) {
        return (
            None,
            Some(format!(
                "arXiv API metadata unavailable for {}: {}",
                paper.full_id(),
                error.message(),
            )),
        );
    }
    let body = fs::read_to_string(&dest).unwrap_or_default();
    match parse_atom_entry(&body) {
        Some(metadata) => (Some(metadata), None),
        None => (
            None,
            Some(format!(
                "arXiv API metadata unavailable for {}: could not parse the API response",
                paper.full_id(),
            )),
        ),
    }
}

/// Parse the first `<entry>` of an arXiv Atom response. Returns `None`
/// for an error entry (whose `<id>` contains `/api/errors`) or for a
/// response with no usable entry.
fn parse_atom_entry(xml: &str) -> Option<ArxivMetadata> {
    static ENTRY: OnceLock<Regex> = OnceLock::new();
    let entry = ENTRY.get_or_init(|| {
        Regex::new(r"(?s)<entry\b[^>]*>(.*?)</entry\s*>")
            .expect("valid entry grammar")
    });
    let body = entry.captures(xml)?.get(1)?.as_str();
    let id = tag_text(body, "id")?;
    if id.contains("/api/errors") {
        return None;
    }
    let title =
        collapse_whitespace(&decode_entities(&tag_text(body, "title")?));
    if title.is_empty() {
        return None;
    }
    let published = tag_text(body, "published")?;
    let published = published.split('T').next().unwrap_or("").to_string();
    if published.is_empty() {
        return None;
    }
    static AUTHOR: OnceLock<Regex> = OnceLock::new();
    let author = AUTHOR.get_or_init(|| {
        Regex::new(r"(?s)<author\b[^>]*>.*?<name\b[^>]*>(.*?)</name\s*>")
            .expect("valid author grammar")
    });
    let authors: Vec<String> = author
        .captures_iter(body)
        .map(|captures| {
            collapse_whitespace(&decode_entities(
                captures.get(1).map(|found| found.as_str()).unwrap_or(""),
            ))
        })
        .filter(|name| !name.is_empty())
        .collect();
    Some(ArxivMetadata {
        title,
        authors,
        published,
    })
}

/// The text of the first `<tag>…</tag>` in `body`, if any. A namespace
/// prefix on the element name is ignored (`<arxiv:title>` matches
/// `title`). The `regex` crate has no backreferences, so this is a small
/// manual scan instead.
fn tag_text(body: &str, tag: &str) -> Option<String> {
    let mut rest = body;
    while let Some(open) = rest.find('<') {
        rest = &rest[open + 1..];
        if rest.starts_with('/')
            || rest.starts_with('!')
            || rest.starts_with('?')
        {
            continue;
        }
        let name_len = rest
            .find(|char: char| {
                !char.is_alphanumeric()
                    && !matches!(char, ':' | '_' | '.' | '-')
            })
            .unwrap_or(rest.len());
        let name = &rest[..name_len];
        // Strip any namespace prefix before comparing.
        if name.rsplit(':').next() != Some(tag) {
            continue;
        }
        let tag_end = rest.find('>')?;
        if rest[..tag_end].trim_end().ends_with('/') {
            return Some(String::new());
        }
        let inner = &rest[tag_end + 1..];
        let closer = format!("</{name}");
        let close_start = inner.find(&closer)?;
        let after = inner[close_start + closer.len()..].trim_start();
        if !after.starts_with('>') {
            continue;
        }
        return Some(inner[..close_start].to_string());
    }
    None
}

/// Collapse every run of whitespace (including newlines) to one space.
fn collapse_whitespace(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Decode the XML entities that appear in arXiv Atom responses.
fn decode_entities(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find('&') {
        out.push_str(&rest[..start]);
        rest = &rest[start..];
        let end = rest.find(';').unwrap_or(rest.len());
        let entity = &rest[..end.min(rest.len())];
        let decoded = match entity {
            "&amp" => Some("&".to_string()),
            "&lt" => Some("<".to_string()),
            "&gt" => Some(">".to_string()),
            "&quot" => Some("\"".to_string()),
            "&apos" => Some("'".to_string()),
            _ if entity.starts_with("&#x") || entity.starts_with("&#X") => {
                u32::from_str_radix(entity[3..].trim(), 16)
                    .ok()
                    .and_then(char::from_u32)
                    .map(|char| char.to_string())
            }
            _ if entity.starts_with("&#") => entity[2..]
                .trim()
                .parse::<u32>()
                .ok()
                .and_then(char::from_u32)
                .map(|char| char.to_string()),
            _ => None,
        };
        match decoded {
            Some(text) => {
                out.push_str(&text);
                rest = if end < rest.len() {
                    &rest[end + 1..]
                } else {
                    ""
                };
            }
            None => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Positive table ported verbatim from sase-listen's
    /// `tests/test_arxiv.py`.
    #[test]
    fn arxiv_parsing_matches_sase_listen_positives() {
        for (url, paper_id) in [
            ("https://arxiv.org/abs/2602.16844", "2602.16844"),
            ("https://arxiv.org/abs/2602.16844v2", "2602.16844v2"),
            ("http://arxiv.org/abs/0704.0001", "0704.0001"),
            ("https://www.arxiv.org/abs/2602.16844/", "2602.16844"),
            ("https://export.arxiv.org/abs/2602.16844", "2602.16844"),
            ("HTTPS://ArXiv.org/abs/2602.16844", "2602.16844"),
            (
                "https://arxiv.org/abs/2602.16844?context=cs.AI#x",
                "2602.16844",
            ),
            ("https://arxiv.org/html/2602.16844v1/#S3", "2602.16844v1"),
            ("https://arxiv.org/pdf/2602.16844.pdf", "2602.16844"),
            ("https://arxiv.org/pdf/2602.16844v3", "2602.16844v3"),
            ("https://arxiv.org/abs/hep-th/9901001", "hep-th/9901001"),
            (
                "https://arxiv.org/abs/math.GT/0309136v2",
                "math.GT/0309136v2",
            ),
        ] {
            let paper = ArxivPaper::parse(url)
                .unwrap_or_else(|| panic!("expected an arXiv paper: {url}"));
            assert_eq!(paper.full_id(), paper_id, "wrong id for {url}");
            assert_eq!(
                paper.pdf_url(),
                format!("https://arxiv.org/pdf/{paper_id}"),
                "wrong PDF URL for {url}"
            );
        }
    }

    /// Negative table ported verbatim from sase-listen's
    /// `tests/test_arxiv.py`.
    #[test]
    fn arxiv_parsing_matches_sase_listen_negatives() {
        for url in [
            "https://arxiv.org/",
            "https://arxiv.org/list/cs.AI/recent",
            "https://arxiv.org/abs/",
            "https://arxiv.org/abs/not-an-id",
            "https://arxiv.org/src/2602.16844",
            "https://example.com/abs/2602.16844",
            "https://notarxiv.org/abs/2602.16844",
            "https://arxiv.org.evil.test/abs/2602.16844",
            "ftp://arxiv.org/abs/2602.16844",
        ] {
            assert!(
                ArxivPaper::parse(url).is_none(),
                "expected no arXiv paper: {url}"
            );
        }
    }

    #[test]
    fn arxiv_versions_split_and_dedupe_without_version() {
        let paper = ArxivPaper::parse("https://arxiv.org/abs/2602.16844v2")
            .expect("parse versioned URL");
        assert_eq!(paper.id, "2602.16844");
        assert_eq!(paper.version.as_deref(), Some("v2"));
        assert_eq!(paper.dedupe_key(), "https://arxiv.org/abs/2602.16844");
        assert_eq!(paper.abs_url(), "https://arxiv.org/abs/2602.16844v2");

        let unversioned = ArxivPaper::parse("https://arxiv.org/pdf/2602.16844")
            .expect("parse unversioned URL");
        assert_eq!(unversioned.version, None);
        assert_eq!(
            unversioned.dedupe_key(),
            paper.dedupe_key(),
            "versions share one dedupe key"
        );

        let legacy =
            ArxivPaper::parse("https://arxiv.org/abs/math.GT/0309136v2")
                .expect("parse legacy URL");
        assert_eq!(legacy.id, "math.GT/0309136");
        assert_eq!(legacy.version.as_deref(), Some("v2"));

        // A `.pdf` suffix is allowed only on `/pdf/`.
        assert!(
            ArxivPaper::parse("https://arxiv.org/abs/2602.16844.pdf").is_none()
        );
        assert!(ArxivPaper::parse("https://arxiv.org/html/2602.16844.pdf")
            .is_none());
    }

    /// A multi-line title with an entity, five authors, and a timestamped
    /// first-version date.
    const MULTI_AUTHOR_ENTRY: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<feed xmlns="http://www.w3.org/2005/Atom">
<entry>
<id>http://arxiv.org/abs/1706.03762v7</id>
<updated>2023-08-10T00:00:00Z</updated>
<published>2017-06-12T17:57:34Z</published>
<title>Attention Is
  All You Need &amp; More</title>
<author><name>Ashish Vaswani</name></author>
<author><name>Noam Shazeer</name></author>
<author><name>Niki Parmar</name></author>
<author><name>Jakob Uszkoreit</name></author>
<author><name>Llion Jones</name></author>
</entry>
</feed>"#;

    #[test]
    fn arxiv_metadata_parses_titles_authors_and_dates() {
        let metadata =
            parse_atom_entry(MULTI_AUTHOR_ENTRY).expect("parse entry");
        assert_eq!(metadata.title, "Attention Is All You Need & More");
        assert_eq!(metadata.authors.len(), 5);
        assert_eq!(metadata.authors[0], "Ashish Vaswani");
        assert_eq!(metadata.published, "2017-06-12");
        assert_eq!(author_display(&metadata.authors), "Ashish Vaswani et al.");
    }

    #[test]
    fn arxiv_author_display_covers_one_two_and_three_authors() {
        let solo = ["Ada Lovelace".to_string()];
        assert_eq!(author_display(&solo), "Ada Lovelace");
        let pair = ["Ada Lovelace".to_string(), "Alan Turing".to_string()];
        assert_eq!(author_display(&pair), "Ada Lovelace and Alan Turing");
        let trio = [
            "Ada Lovelace".to_string(),
            "Alan Turing".to_string(),
            "Grace Hopper".to_string(),
        ];
        assert_eq!(
            author_display(&trio),
            "Ada Lovelace, Alan Turing, and Grace Hopper"
        );
        assert_eq!(author_display(&[]), "");
    }

    #[test]
    fn arxiv_metadata_rejects_error_entries() {
        let error = r#"<feed xmlns="http://www.w3.org/2005/Atom">
<entry>
<id>http://arxiv.org/api/errors#incorrect_id_format_for_2602.1684</id>
<title>Error</title>
</entry>
</feed>"#;
        assert!(
            parse_atom_entry(error).is_none(),
            "an api/errors entry is not metadata"
        );
        assert!(
            parse_atom_entry("<feed></feed>").is_none(),
            "a response with no entry is not metadata"
        );
    }

    #[test]
    fn arxiv_fetch_metadata_degrades_with_a_warning() {
        let _guard = super::super::fetch::CURL_TEST_LOCK
            .lock()
            .expect("lock curl env");
        let dir = std::env::temp_dir().join(format!(
            "bob-arxiv-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|duration| duration.as_nanos())
                .unwrap_or(0),
        ));
        fs::create_dir_all(&dir).expect("create arxiv test dir");
        unsafe {
            std::env::set_var(
                super::super::fetch::ENV_CURL_OVERRIDE,
                dir.join("no-such-curl-binary"),
            )
        };
        let paper = ArxivPaper::parse("https://arxiv.org/abs/1706.03762")
            .expect("parse paper");
        let (metadata, warning) = fetch_metadata(&paper, &dir);
        assert_eq!(metadata, None);
        let warning = warning.expect("a warning accompanies the fallback");
        assert!(
            warning.contains("1706.03762"),
            "unexpected warning: {warning}"
        );
        unsafe { std::env::remove_var(super::super::fetch::ENV_CURL_OVERRIDE) };
        fs::remove_dir_all(&dir).ok();
    }
}
