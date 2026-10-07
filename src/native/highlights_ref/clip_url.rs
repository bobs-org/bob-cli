//! URL validation, cleaning, dedupe-key, and filename-stem rules for
//! `bob ref clip`.
//!
//! The stored `source_url` is the user's URL with the fragment and tracking
//! parameters removed. The dedupe key normalizes further (lowercased
//! scheme/host, no `www.`, no default port, no trailing slash, sorted
//! query) so the same article reached through two spellings dedupes.

use super::{CommandError, Result};

/// Query parameters stripped from the stored `source_url`. Any parameter
/// whose name starts with `utm_` is also stripped.
const TRACKING_PARAMS: &[&str] =
    &["fbclid", "gclid", "mc_cid", "mc_eid", "ref_src"];

/// Path segments that never contribute to the filename stem, matched after
/// stripping a known page extension.
const SKIPPED_SEGMENTS: &[&str] = &[
    "index", "default", "home", "post", "posts", "p", "article", "articles",
    "blog", "amp", "en", "en-us",
];

/// Page extensions stripped from a path segment before stem derivation.
/// `pdf` covers direct-PDF captures whose URL ends in `.pdf`.
const PAGE_EXTENSIONS: &[&str] =
    &["html", "htm", "php", "asp", "aspx", "shtml", "pdf"];

const STEM_MAX_LEN: usize = 80;

/// A validated URL plus its cleaned and dedupe forms.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct WebUrl {
    /// The URL as recorded in the marker: fragment and tracking parameters
    /// removed, everything else as given.
    pub(crate) cleaned: String,
    /// The normalized identity used for dedupe comparisons.
    pub(crate) dedupe_key: String,
    /// The lowercase host without a leading `www.`, for stem fallbacks.
    pub(crate) host: String,
}

/// Validate `raw` as a public `http(s)` URL and return its cleaned forms.
pub(crate) fn validate_and_clean(raw: &str) -> Result<WebUrl> {
    let url = url::Url::parse(raw.trim()).map_err(|error| {
        CommandError::new(format!("invalid URL {raw:?}: {error}"))
    })?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err(CommandError::new(format!(
            "unsupported URL scheme {:?} (only http and https are captured): {raw:?}",
            url.scheme()
        )));
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err(CommandError::new(format!(
            "URLs with userinfo are not captured: {raw:?}"
        )));
    }
    let host = url.host_str().ok_or_else(|| {
        CommandError::new(format!("URL has no host: {raw:?}"))
    })?;
    reject_private_host(host)?;
    let mut cleaned = url.clone();
    cleaned.set_fragment(None);
    strip_tracking_params(&mut cleaned);
    let cleaned = cleaned.to_string();
    let dedupe_key = dedupe_key_for(&url);
    Ok(WebUrl {
        cleaned,
        dedupe_key,
        host: normalize_host(host),
    })
}

/// Compute the dedupe key for an already-validated URL. Any spelling of
/// an arXiv paper (abs, html, or pdf, with or without a version) maps to
/// `https://arxiv.org/abs/<id>`; everything else uses the lowercase
/// scheme and host (no leading `www.`, no default port), path without a
/// trailing slash, and remaining query parameters sorted.
pub(super) fn dedupe_key_for(url: &url::Url) -> String {
    if let Some(paper) = super::arxiv::ArxivPaper::parse(url.as_str()) {
        return paper.dedupe_key();
    }
    let mut key = String::new();
    key.push_str(&url.scheme().to_lowercase());
    key.push_str("://");
    key.push_str(&normalize_host(url.host_str().unwrap_or_default()));
    if let Some(port) = url.port()
        && Some(port) != url.port_or_known_default()
    {
        key.push_str(&format!(":{port}"));
    }
    let path = url.path().trim_end_matches('/');
    key.push_str(path);
    let mut pairs: Vec<(String, String)> = url
        .query_pairs()
        .filter(|(name, _)| !is_tracking_param(name))
        .map(|(name, value)| (name.into_owned(), value.into_owned()))
        .collect();
    if !pairs.is_empty() {
        pairs.sort();
        key.push('?');
        key.push_str(
            &pairs
                .iter()
                .map(|(name, value)| format!("{name}={value}"))
                .collect::<Vec<_>>()
                .join("&"),
        );
    }
    key
}

fn normalize_host(host: &str) -> String {
    let lower = host.to_lowercase();
    lower.strip_prefix("www.").unwrap_or(&lower).to_string()
}

fn is_tracking_param(name: &str) -> bool {
    let lower = name.to_lowercase();
    lower.starts_with("utm_") || TRACKING_PARAMS.contains(&lower.as_str())
}

fn strip_tracking_params(url: &mut url::Url) {
    if !url.query_pairs().any(|(name, _)| is_tracking_param(&name)) {
        return;
    }
    let kept: Vec<(String, String)> = url
        .query_pairs()
        .filter(|(name, _)| !is_tracking_param(name))
        .map(|(name, value)| (name.into_owned(), value.into_owned()))
        .collect();
    if kept.is_empty() {
        url.set_query(None);
    } else {
        let mut pairs = url.query_pairs_mut();
        pairs.clear();
        for (name, value) in kept {
            pairs.append_pair(&name, &value);
        }
    }
}

fn reject_private_host(host: &str) -> Result<()> {
    let lower = host.to_lowercase();
    if lower == "localhost"
        || lower.ends_with(".localhost")
        || lower.ends_with(".local")
        || lower.ends_with(".internal")
    {
        return Err(CommandError::new(format!(
            "private host is not captured: {host:?}"
        )));
    }
    let literal = lower
        .strip_prefix('[')
        .and_then(|rest| rest.strip_suffix(']').filter(|_| host.contains(':')));
    let candidate = literal.unwrap_or(host);
    if let Ok(address) = candidate.parse::<std::net::IpAddr>()
        && is_non_global_literal(&address)
    {
        return Err(CommandError::new(format!(
            "private IP literal is not captured: {host:?}"
        )));
    }
    Ok(())
}

fn is_non_global_literal(address: &std::net::IpAddr) -> bool {
    match address {
        std::net::IpAddr::V4(ipv4) => {
            let octets = ipv4.octets();
            ipv4.is_loopback()
                || ipv4.is_private()
                || ipv4.is_link_local()
                || ipv4.is_unspecified()
                // CGNAT shared-address space, including the Tailscale range.
                || (octets[0] == 100 && octets[1] & 0b1100_0000 == 64)
        }
        std::net::IpAddr::V6(ipv6) => {
            let segments = ipv6.segments();
            ipv6.is_loopback()
                || ipv6.is_unspecified()
                // Link-local fe80::/10.
                || (segments[0] & 0xffc0 == 0xfe80)
                // Unique-local fc00::/7.
                || (segments[0] & 0xfe00 == 0xfc00)
        }
    }
}

/// Validate `--published` as a `YYYY-MM-DD` calendar date.
pub(super) fn validate_published(date: &str) -> Result<()> {
    if chrono::NaiveDate::parse_from_str(date, "%Y-%m-%d").is_err() {
        return Err(CommandError::new(format!(
            "published date must be YYYY-MM-DD: {date:?}"
        )));
    }
    Ok(())
}

/// Validate `--name`: strip one trailing `.pdf`, then require
/// `[A-Za-z0-9][A-Za-z0-9_.-]*`.
pub(super) fn validate_name(name: &str) -> Result<String> {
    let stem = name
        .strip_suffix(".pdf")
        .or_else(|| name.strip_suffix(".PDF"))
        .unwrap_or(name);
    let mut chars = stem.chars();
    let valid = matches!(chars.next(), Some('A'..='Z' | 'a'..='z' | '0'..='9'))
        && chars.all(|c| {
            c.is_ascii_alphanumeric() || c == '_' || c == '.' || c == '-'
        });
    if !valid {
        return Err(CommandError::new(format!(
            "name must match [A-Za-z0-9][A-Za-z0-9_.-]*: {name:?}"
        )));
    }
    Ok(stem.to_string())
}

/// Derive the filename stem from a validated URL's path, or `None` when
/// every segment is skipped.
pub(super) fn stem_from_url(url: &WebUrl) -> Option<String> {
    let parsed = url::Url::parse(&url.cleaned).ok()?;
    let segments: Vec<String> =
        parsed.path_segments()?.map(percent_decode).collect();
    for segment in segments.iter().rev() {
        if let Some(stem) = stem_from_segment(segment) {
            return Some(stem);
        }
    }
    None
}

fn stem_from_segment(segment: &str) -> Option<String> {
    let mut candidate = segment.to_string();
    for extension in PAGE_EXTENSIONS {
        if let Some(stripped) = candidate
            .strip_suffix(&format!(".{extension}"))
            .or_else(|| {
                candidate
                    .strip_suffix(&format!(".{}", extension.to_uppercase()))
            })
        {
            candidate = stripped.to_string();
            break;
        }
    }
    if candidate.is_empty()
        || candidate.len() <= 2
        || candidate.chars().all(|c| c.is_ascii_digit())
        || is_date_part(&candidate)
        || SKIPPED_SEGMENTS.contains(&candidate.to_lowercase().as_str())
    {
        return None;
    }
    if let Some(stripped) = strip_date_prefix(&candidate)
        && !stripped.is_empty()
        && !SKIPPED_SEGMENTS.contains(&stripped.to_lowercase().as_str())
    {
        candidate = stripped;
    } else if strip_date_prefix(&candidate).is_some() {
        return None;
    }
    let slugged = snake_case(&candidate);
    (!slugged.is_empty()).then_some(slugged)
}

fn is_date_part(candidate: &str) -> bool {
    let digits: String =
        candidate.chars().filter(|c| c.is_ascii_digit()).collect();
    (digits.len() == 8 && candidate.len() == 8
        || digits.len() == 8 && candidate.len() == 10)
        && candidate.chars().all(|c| c.is_ascii_digit() || c == '-')
}

fn strip_date_prefix(candidate: &str) -> Option<String> {
    let (prefix, rest) = candidate.split_at_checked(11)?;
    let (date, dash) = prefix.split_at_checked(10)?;
    if dash != "-"
        || chrono::NaiveDate::parse_from_str(date, "%Y-%m-%d").is_err()
    {
        return None;
    }
    Some(rest.to_string())
}

/// Lowercase `text`, turning every run of characters outside `[a-z0-9]`
/// into one `_`, trimmed and capped at 80 characters on a `_` boundary.
pub(super) fn snake_case(text: &str) -> String {
    let mut slug = String::new();
    let mut last_underscore = false;
    for c in text.to_lowercase().chars() {
        if c.is_ascii_lowercase() || c.is_ascii_digit() {
            slug.push(c);
            last_underscore = false;
        } else if !last_underscore {
            slug.push('_');
            last_underscore = true;
        }
    }
    let slug = slug.trim_matches('_').to_string();
    if slug.len() <= STEM_MAX_LEN {
        return slug;
    }
    match slug[..STEM_MAX_LEN].rfind('_') {
        Some(index) => slug[..index].to_string(),
        None => slug[..STEM_MAX_LEN].to_string(),
    }
}

/// Derive the filename stem from a paper title with the short-title
/// rule: when the text before the first colon is 1-4 words, that prefix
/// is the short name (`EA-Graph: …` becomes `ea_graph`); otherwise the
/// first 6 words stand in. The result runs through [`snake_case`], which
/// caps it at 80 characters.
pub(super) fn short_title_stem(title: &str) -> String {
    let words: Vec<&str> = title.split_whitespace().collect();
    let prefix: Vec<&str> = match title.find(':') {
        Some(index) => title[..index].split_whitespace().collect(),
        None => Vec::new(),
    };
    let source = if !prefix.is_empty() && prefix.len() <= 4 {
        prefix.join(" ")
    } else {
        words.iter().take(6).copied().collect::<Vec<_>>().join(" ")
    };
    snake_case(&source)
}

/// Turn a stem back into a human-readable title fallback.
pub(crate) fn humanize_stem(stem: &str) -> String {
    let mut title = String::new();
    let mut last_space = true;
    for c in stem.chars() {
        if c.is_ascii_alphanumeric() {
            title.push(c);
            last_space = false;
        } else if !last_space {
            title.push(' ');
            last_space = true;
        }
    }
    title.trim().to_string()
}

pub(crate) fn percent_decode(segment: &str) -> String {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validation_rejects_non_public_urls() {
        for raw in [
            "ftp://example.com/article",
            "file:///etc/passwd",
            "https://user@example.com/article",
            "https://user:pass@example.com/article",
            "http://localhost/article",
            "http://LOCALHOST:8080/article",
            "http://foo.localhost/article",
            "http://printer.local/article",
            "http://db.internal/article",
            "http://10.0.0.1/article",
            "http://192.168.1.10/article",
            "http://100.101.1.2/article",
            "http://169.254.169.254/latest",
            "http://[::1]/article",
            "http://[fc00::1]/article",
            "not a url",
        ] {
            assert!(
                validate_and_clean(raw).is_err(),
                "expected rejection: {raw}"
            );
        }
    }

    #[test]
    fn validation_accepts_public_urls_and_strips_tracking() {
        let cleaned = validate_and_clean(
            "https://example.com/posts/hello?utm_source=x&fbclid=abc&id=7#frag",
        )
        .expect("accept public URL");
        assert_eq!(cleaned.cleaned, "https://example.com/posts/hello?id=7");
        assert_eq!(cleaned.dedupe_key, "https://example.com/posts/hello?id=7");
    }

    #[test]
    fn dedupe_key_normalizes_spellings() {
        let first = validate_and_clean(
            "https://WWW.Example.COM:443/a/?b=2&a=1&utm_medium=x",
        )
        .expect("accept first spelling");
        let second = validate_and_clean("https://example.com/a?a=1&b=2")
            .expect("accept second spelling");
        assert_eq!(first.dedupe_key, second.dedupe_key);
        assert_eq!(first.dedupe_key, "https://example.com/a?a=1&b=2");
    }

    #[test]
    fn stem_derivation_skips_noise_segments() {
        let stem = |raw: &str| {
            stem_from_url(&validate_and_clean(raw).expect("accept stem URL"))
        };
        assert_eq!(
            stem("https://openai.com/index/open-source-codex-orchestration-symphony/"),
            Some("open_source_codex_orchestration_symphony".to_string())
        );
        assert_eq!(
            stem("https://example.com/blog/2023-06-23-my-post/"),
            Some("my_post".to_string())
        );
        assert_eq!(
            stem("https://example.com/blog/my-post.html"),
            Some("my_post".to_string())
        );
        assert_eq!(
            stem("https://example.com/blog/index.html"),
            None,
            "both segments are skipped"
        );
        assert_eq!(stem("https://example.com/2024/"), None);
        assert_eq!(
            stem("https://example.com/p/987654321"),
            None,
            "numeric-only paths have no slug"
        );
    }

    #[test]
    fn short_title_stem_uses_the_colon_prefix_or_six_words() {
        assert_eq!(
            short_title_stem(
                "EA-Graph: Artifact-Anchored Verification Memory for Coding Agents"
            ),
            "ea_graph"
        );
        assert_eq!(
            short_title_stem(
                "A Very Long Prefix With Seven Words Here: the rest of the title"
            ),
            "a_very_long_prefix_with_seven",
            "a seven-word prefix falls back to the first 6 words"
        );
        assert_eq!(
            short_title_stem("Attention Is All You Need"),
            "attention_is_all_you_need"
        );
        assert_eq!(
            short_title_stem(
                "One: Two Three Four Five Six Seven Eight Words Total Here"
            ),
            "one",
            "a one-word prefix wins over the word count"
        );
        assert_eq!(
            short_title_stem("Machine Learning Systems: Design and Operation"),
            "machine_learning_systems",
            "a three-word prefix is kept whole"
        );
    }

    #[test]
    fn arxiv_spellings_share_one_dedupe_key() {
        let key = |raw: &str| {
            let parsed = url::Url::parse(raw).expect("parse arXiv URL");
            dedupe_key_for(&parsed)
        };
        assert_eq!(
            key("https://arxiv.org/pdf/2608.04278"),
            "https://arxiv.org/abs/2608.04278"
        );
        assert_eq!(
            key("https://arxiv.org/abs/2608.04278v2"),
            "https://arxiv.org/abs/2608.04278"
        );
        assert_eq!(
            key("https://arxiv.org/html/2608.04278v1/"),
            "https://arxiv.org/abs/2608.04278"
        );
    }

    #[test]
    fn non_arxiv_dedupe_keys_are_unchanged() {
        let parsed = url::Url::parse("https://WWW.Example.COM:443/a/?b=2&a=1")
            .expect("parse URL");
        assert_eq!(dedupe_key_for(&parsed), "https://example.com/a?a=1&b=2");
    }

    #[test]
    fn name_validation_strips_pdf_and_rejects_junk() {
        assert_eq!(
            validate_name("my-article.pdf").expect("strip .pdf"),
            "my-article"
        );
        for bad in ["", "-lead", "_lead", ".dot", "has space", "semi;colon"] {
            assert!(validate_name(bad).is_err(), "expected rejection: {bad}");
        }
    }
}
