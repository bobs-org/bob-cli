//! URL validation, cleaning, dedupe-key, and filename-stem rules for
//! the web-article engine behind `bob ref create`.
//!
//! The stored `source_url` is the user's URL with the fragment and tracking
//! parameters removed. The dedupe key normalizes further (lowercased
//! scheme/host, no `www.`, no default port, no trailing slash, sorted
//! query) so the same article reached through two spellings dedupes.

use std::borrow::Cow;

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
pub(crate) fn dedupe_key_for(url: &url::Url) -> String {
    if let Some(paper) = super::arxiv::ArxivPaper::parse(url.as_str()) {
        return paper.dedupe_key();
    }
    let host = normalize_host(url.host_str().unwrap_or_default());
    let mut key = String::new();
    key.push_str(&url.scheme().to_lowercase());
    key.push_str("://");
    key.push_str(&host);
    // `url.port()` already returns `None` for a scheme-default port, so
    // keeping it preserves explicit non-default ports while a spelled-out
    // default still normalizes away (bob-cli-c3).
    if let Some(port) = url.port() {
        key.push_str(&format!(":{port}"));
    }
    let path = url.path().trim_end_matches('/');
    key.push_str(&normalize_path_escapes(&medium_author_alias(&host, path)));
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

/// Spell a Medium author path's leading `%40` sigil as `@`
/// (bob-cli-7u).
///
/// Medium serves `/@author/...` and `/%40author/...` as the same page,
/// and share links arrive in both spellings. `@` is reserved, so this
/// is an explicit alias for that verified host, not a general
/// equivalence: it applies only to the first path segment's leading
/// sigil on the exact `medium.com` host (after `www.` normalization).
/// Other hosts, Medium subdomains, and every other `%40` keep their
/// escape.
fn medium_author_alias<'a>(host: &str, path: &'a str) -> Cow<'a, str> {
    match path.strip_prefix("/%40") {
        Some(rest) if host == "medium.com" => Cow::Owned(format!("/@{rest}")),
        _ => Cow::Borrowed(path),
    }
}

/// Normalize percent-escape spelling in a URL path for the dedupe key
/// (bob-cli-7u).
///
/// Escapes of RFC 3986 unreserved characters (`A-Z a-z 0-9 - . _ ~`)
/// decode to that literal, because both spellings name the same
/// resource. Every other escape, including reserved `%40`, is retained
/// with uppercase hex, so `%2f` and `%2F` agree while encoded
/// delimiters (`%2F`, `%3F`, `%23`, `%25`) never merge with their
/// structural or literal counterparts. Malformed `%` sequences pass
/// through untouched, and decoding is one left-to-right pass, so
/// `%2541` stays `%2541` instead of collapsing to `A`.
fn normalize_path_escapes(path: &str) -> String {
    let bytes = path.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%'
            && index + 2 < bytes.len()
            && let (Some(high), Some(low)) =
                (hex_value(bytes[index + 1]), hex_value(bytes[index + 2]))
        {
            let decoded = high << 4 | low;
            if is_unreserved(decoded) {
                out.push(decoded);
            } else {
                out.push(b'%');
                out.push(upper_hex(high));
                out.push(upper_hex(low));
            }
            index += 3;
        } else {
            out.push(bytes[index]);
            index += 1;
        }
    }
    // Only ASCII `%XX` triples are rewritten, so the UTF-8 boundary
    // structure of `path` is unchanged and this never fails.
    String::from_utf8(out).expect("path normalization keeps UTF-8")
}

/// RFC 3986 unreserved characters: safe to decode in a path because
/// the literal and escaped spellings name the same resource.
fn is_unreserved(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~')
}

fn upper_hex(nibble: u8) -> u8 {
    match nibble {
        0..=9 => b'0' + nibble,
        _ => b'A' + (nibble - 10),
    }
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

pub(crate) fn is_non_global_literal(address: &std::net::IpAddr) -> bool {
    match address {
        std::net::IpAddr::V4(ipv4) => is_non_global_ipv4(ipv4),
        std::net::IpAddr::V6(ipv6) => {
            // IPv4-mapped (::ffff:a.b.c.d) and IPv4-compatible (::a.b.c.d)
            // literals are judged by their embedded IPv4 address, so
            // [::ffff:10.0.0.1] is refused like 10.0.0.1 (bob-cli-4v).
            if let Some(embedded) = ipv6_embedded_ipv4(ipv6)
                && is_non_global_ipv4(&embedded)
            {
                return true;
            }
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

fn is_non_global_ipv4(ipv4: &std::net::Ipv4Addr) -> bool {
    let octets = ipv4.octets();
    ipv4.is_loopback()
        || ipv4.is_private()
        || ipv4.is_link_local()
        || ipv4.is_unspecified()
        // CGNAT shared-address space, including the Tailscale range.
        || (octets[0] == 100 && octets[1] & 0b1100_0000 == 64)
}

/// The embedded IPv4 address of an IPv4-mapped (`::ffff:a.b.c.d`) or
/// IPv4-compatible (`::a.b.c.d`) IPv6 literal, if it has one.
fn ipv6_embedded_ipv4(ipv6: &std::net::Ipv6Addr) -> Option<std::net::Ipv4Addr> {
    if let Some(mapped) = ipv6.to_ipv4_mapped() {
        return Some(mapped);
    }
    let segments = ipv6.segments();
    // IPv4-compatible: the first 96 bits are zero.
    if segments[..6] == [0, 0, 0, 0, 0, 0] {
        let bytes = ipv6.octets();
        return Some(std::net::Ipv4Addr::new(
            bytes[12], bytes[13], bytes[14], bytes[15],
        ));
    }
    None
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
        let candidate = strip_page_extension(segment);
        if is_opaque_segment(&candidate) {
            return None;
        }
        if let Some(stem) = stem_from_segment(segment) {
            return Some(stem);
        }
    }
    None
}

pub(super) fn stem_from_segment(segment: &str) -> Option<String> {
    let candidate = clean_segment(segment)?;
    let slugged = snake_case(&candidate);
    (!slugged.is_empty()).then_some(slugged)
}

/// Strip a page extension, date prefix, and opaque identifier words without
/// applying the filename cap. Collection naming needs every token for its
/// part-order check; callers still apply `snake_case` at the final naming step.
pub(super) fn clean_segment(segment: &str) -> Option<String> {
    let mut candidate = strip_page_extension(segment);
    if is_opaque_segment(&candidate) {
        return None;
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
    let mut words = split_stem_words(&candidate);
    strip_opaque_suffix(&mut words);
    candidate = words.join("_");
    if candidate.is_empty() {
        return None;
    }
    if candidate.len() <= 2
        || candidate.chars().all(|c| c.is_ascii_digit())
        || is_date_part(&candidate)
        || SKIPPED_SEGMENTS.contains(&candidate.to_lowercase().as_str())
    {
        return None;
    }
    Some(candidate)
}

fn strip_page_extension(segment: &str) -> String {
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
    candidate
}

fn is_opaque_segment(candidate: &str) -> bool {
    let lower = candidate.to_ascii_lowercase();
    super::collection_name::is_hex_id_word(&lower) || is_uuid(&lower)
}

fn is_uuid(candidate: &str) -> bool {
    let parts: Vec<&str> = candidate.split('-').collect();
    [8, 4, 4, 4, 12].iter().enumerate().all(|(index, len)| {
        parts.get(index).is_some_and(|part| {
            part.len() == *len
                && part.bytes().all(|byte| byte.is_ascii_hexdigit())
        })
    }) && parts.len() == 5
}

fn split_stem_words(text: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut current = String::new();
    for byte in text.bytes() {
        if byte.is_ascii_alphanumeric() {
            current.push((byte as char).to_ascii_lowercase());
        } else if !current.is_empty() {
            words.push(std::mem::take(&mut current));
        }
    }
    if !current.is_empty() {
        words.push(current);
    }
    words
}

fn strip_opaque_suffix(words: &mut Vec<String>) {
    if words.len() >= 5 {
        let suffix = &words[words.len() - 5..];
        if [8, 4, 4, 4, 12].iter().enumerate().all(|(index, len)| {
            suffix[index].len() == *len
                && suffix[index].bytes().all(|byte| byte.is_ascii_hexdigit())
        }) {
            words.truncate(words.len() - 5);
        }
    }
    while words
        .last()
        .is_some_and(|word| super::collection_name::is_hex_id_word(word))
    {
        words.pop();
    }
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

/// Strip one trailing `.md`, `.markdown`, or `.txt` from a title-derived
/// stem candidate, compared ASCII case-insensitively.
fn strip_title_extension(title: &str) -> &str {
    for extension in [".md", ".markdown", ".txt"] {
        if title.len() >= extension.len()
            && title[title.len() - extension.len()..]
                .eq_ignore_ascii_case(extension)
        {
            return &title[..title.len() - extension.len()];
        }
    }
    title
}

/// Derive the filename stem from an extracted title: drop one trailing
/// `.md`/`.markdown`/`.txt` first, then [`snake_case`].
pub(super) fn title_stem(title: &str) -> String {
    snake_case(strip_title_extension(title))
}

/// Derive the filename stem from a paper title with the short-title
/// rule: when the text before the first colon is 1-4 words, that prefix
/// is the short name (`EA-Graph: …` becomes `ea_graph`); otherwise the
/// first 6 words stand in. The result runs through [`snake_case`], which
/// caps it at 80 characters.
pub(super) fn short_title_stem(title: &str) -> String {
    let title = strip_title_extension(title);
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
            // IPv4-mapped and IPv4-compatible literals are judged by
            // their embedded IPv4 address (bob-cli-4v).
            "http://[::ffff:127.0.0.1]/article",
            "http://[::ffff:10.0.0.1]/article",
            "http://[::10.0.0.1]/article",
            "not a url",
        ] {
            assert!(
                validate_and_clean(raw).is_err(),
                "expected rejection: {raw}"
            );
        }
    }

    #[test]
    fn validation_accepts_mapped_public_literals() {
        // An IPv4-mapped literal with a global embedded address is
        // public (bob-cli-4v).
        validate_and_clean("http://[::ffff:93.184.216.34]/a")
            .expect("accept mapped public literal");
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
    fn dedupe_key_normalizes_equivalent_path_spellings() {
        let key =
            |raw: &str| validate_and_clean(raw).expect("accept URL").dedupe_key;
        // The reported Medium twin (bob-cli-7u).
        assert_eq!(
            key("https://medium.com/@edominguez.se/so-i-switched-to-neovim-in-2025-163b85aa0935"),
            key("https://medium.com/%40edominguez.se/so-i-switched-to-neovim-in-2025-163b85aa0935"),
        );
        // Unreserved escapes decode to their literal spelling.
        assert_eq!(
            key("https://example.com/a%41b"),
            key("https://example.com/aAb")
        );
        assert_eq!(
            key("https://example.com/a%7Eb"),
            key("https://example.com/a~b")
        );
        assert_eq!(
            key("https://example.com/a%7eb"),
            key("https://example.com/a~b")
        );
        assert_eq!(
            key("https://example.com/a%2Db"),
            key("https://example.com/a-b")
        );
        // Retained escapes agree across hex case; non-ASCII UTF-8 stays
        // encoded with normalized case.
        assert_eq!(
            key("https://example.com/a%2fb"),
            "https://example.com/a%2Fb"
        );
        assert_eq!(
            key("https://example.com/caf%c3%a9"),
            key("https://example.com/caf%C3%A9")
        );
    }

    #[test]
    fn dedupe_key_keeps_delimiter_encoded_resources_distinct() {
        let key =
            |raw: &str| validate_and_clean(raw).expect("accept URL").dedupe_key;
        // Encoded delimiters never merge with their structural or
        // literal counterparts (bob-cli-7u).
        assert_ne!(
            key("https://example.com/a%2Fb"),
            key("https://example.com/a/b"),
            "encoded slash is not a path separator"
        );
        assert!(
            key("https://example.com/a%2Fb").contains("%2F"),
            "the encoded slash is retained, not dropped"
        );
        assert_ne!(
            key("https://example.com/a%3Fb"),
            key("https://example.com/a?b="),
            "an encoded question mark is not a query separator"
        );
        assert_ne!(
            key("https://example.com/a%23b"),
            key("https://example.com/a"),
            "an encoded fragment marker is not a fragment"
        );
        assert_ne!(
            key("https://example.com/a%25b"),
            key("https://example.com/a%b"),
            "an encoded percent is not a literal percent"
        );
        // No recursive decoding: `%25` stays an encoded percent, so
        // `%2540` never collapses to `@`.
        assert_eq!(
            key("https://example.com/a%2540b"),
            "https://example.com/a%2540b"
        );
        assert_ne!(
            key("https://example.com/a%2540b"),
            key("https://example.com/a@b")
        );
        assert_eq!(
            key("https://example.com/a%2541b"),
            "https://example.com/a%2541b"
        );
        // Malformed escapes pass through untouched.
        assert_eq!(
            key("https://example.com/a%zzb"),
            "https://example.com/a%zzb"
        );
        assert_eq!(key("https://example.com/a%b"), "https://example.com/a%b");
    }

    #[test]
    fn reserved_at_sign_alias_is_only_the_medium_author_sigil() {
        let key =
            |raw: &str| validate_and_clean(raw).expect("accept URL").dedupe_key;
        // The verified alias survives `www.` and hex-case-free `%40`.
        assert_eq!(
            key("https://www.medium.com/%40edominguez.se/piece-163b85aa0935"),
            "https://medium.com/@edominguez.se/piece-163b85aa0935"
        );
        // `@` is reserved: other hosts keep `%40` and `@` distinct
        // (bob-cli-7u).
        assert_eq!(
            key("https://example.com/a%40b"),
            "https://example.com/a%40b"
        );
        assert_ne!(
            key("https://example.com/a%40b"),
            key("https://example.com/a@b")
        );
        assert_ne!(
            key("https://example.com/%40user/post"),
            key("https://example.com/@user/post")
        );
        // Lookalike hosts and Medium subdomains are not the verified
        // host.
        for host in
            ["notmedium.com", "medium.com.example.org", "blog.medium.com"]
        {
            assert_ne!(
                key(&format!("https://{host}/%40author/post")),
                key(&format!("https://{host}/@author/post")),
                "{host} keeps the escape"
            );
        }
        // Only the first segment's leading sigil is aliased.
        assert_ne!(
            key("https://medium.com/@author/post%40x"),
            key("https://medium.com/@author/post@x")
        );
        assert_ne!(
            key("https://medium.com/@author/%40x"),
            key("https://medium.com/@author/@x")
        );
        assert_ne!(
            key("https://medium.com/p%40x"),
            key("https://medium.com/p@x")
        );
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
        assert_eq!(
            stem("https://gist.github.com/karpathy/442a6bf555914893e9891c11519de94f"),
            None,
            "an opaque final path segment is not a useful filename stem"
        );
        assert_eq!(
            stem("https://example.com/blog/karpathy-llm-wiki-442a6bf555914893e9891c11519de94f"),
            Some("karpathy_llm_wiki".to_string()),
            "trailing opaque hex IDs are removed from an otherwise useful slug"
        );
        assert_eq!(
            stem("https://example.com/550e8400-e29b-41d4-a716-446655440000"),
            None,
            "an opaque UUID path segment is not a useful filename stem"
        );
        assert_eq!(
            stem("https://example.com/blog/a-good-title-550e8400-e29b-41d4-a716-446655440000"),
            Some("a_good_title".to_string()),
            "a trailing UUID is removed from an otherwise useful slug"
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
        assert_eq!(
            short_title_stem("Attention Is All You Need.md"),
            "attention_is_all_you_need",
            "a trailing Markdown extension is removed before title derivation"
        );
        assert_eq!(
            short_title_stem("Attention Is All You Need.txt"),
            "attention_is_all_you_need",
            "a trailing text extension is removed before title derivation"
        );
    }

    #[test]
    fn title_stem_strips_one_trailing_text_extension() {
        assert_eq!(title_stem("karpathy/llm-wiki.md"), "karpathy_llm_wiki");
        assert_eq!(
            title_stem("karpathy/llm-wiki.Markdown"),
            "karpathy_llm_wiki"
        );
        assert_eq!(title_stem("karpathy/llm-wiki.TXT"), "karpathy_llm_wiki");
        assert_eq!(
            title_stem("Attention Is All You Need"),
            "attention_is_all_you_need"
        );
        assert_eq!(
            short_title_stem("Attention Is All You Need.Md"),
            "attention_is_all_you_need"
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
    fn dedupe_key_keeps_explicit_non_default_ports() {
        let key =
            |raw: &str| validate_and_clean(raw).expect("accept URL").dedupe_key;
        // A non-default port is part of the identity (bob-cli-c3).
        assert_eq!(
            key("https://example.com:8443/a"),
            "https://example.com:8443/a"
        );
        // A spelled-out default port normalizes away: the url crate
        // reports `port()` as `None` for it.
        assert_eq!(key("https://example.com:443/a"), "https://example.com/a");
        // No port keeps the bare host.
        assert_eq!(key("https://example.com/a"), "https://example.com/a");
        // The same host and path on different ports are distinct sources.
        assert_ne!(
            key("https://example.com:8443/a"),
            key("https://example.com/a"),
            "a self-hosted :8443 copy is not the :443 site"
        );
        assert_ne!(
            key("https://example.com:8443/a"),
            key("https://example.com:9443/a"),
            "two non-default ports are distinct sources"
        );
        // An explicit default port agrees with no port.
        assert_eq!(
            key("https://example.com:443/a"),
            key("https://example.com/a"),
        );
        // The http default behaves the same way.
        assert_eq!(key("http://example.com:80/a"), "http://example.com/a");
        assert_eq!(
            key("http://example.com:8080/a"),
            "http://example.com:8080/a"
        );
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
