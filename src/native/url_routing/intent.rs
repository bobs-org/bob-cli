//! URL-intent classifier, display formatting, and URL-list detection.
//!
//! `classify_token` is pure and makes no I/O calls. It accepts only a
//! strict bare URL: no whitespace, an optional single `<…>` wrapper, an
//! `http`/`https` scheme, a successful `validate_and_clean`, and a host
//! that is not an IP literal and contains at least one `.` with
//! non-empty labels.

use crate::native::highlights_ref::{
    dedupe_key_for, validate_and_clean, ArxivPaper,
};

/// How the URL would likely be clipped. Only a hint; the real route is
/// chosen at fetch time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RouteHint {
    Arxiv,
    Pdf,
    Article,
}

impl RouteHint {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            RouteHint::Arxiv => "arxiv",
            RouteHint::Pdf => "pdf",
            RouteHint::Article => "article",
        }
    }
}

/// A classified bare URL.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct UrlIntent {
    /// As typed, minus any `<>` wrapper.
    pub(crate) original: String,
    /// The cleaned URL (`validate_and_clean` output).
    pub(crate) cleaned: String,
    /// Exactly create's dedupe key.
    pub(crate) dedupe_key: String,
    /// Lowercase host without a leading `www.`.
    pub(crate) host: String,
    /// Scheme-stripped display form.
    pub(crate) display: String,
    /// Clip-route hint.
    pub(crate) route_hint: RouteHint,
}

/// Classify one token as URL intent. Returns `None` unless every
/// condition in the plan's URL-intent section holds.
pub(crate) fn classify_token(token: &str) -> Option<UrlIntent> {
    if token.is_empty() || token.chars().any(char::is_whitespace) {
        return None;
    }
    let original = strip_single_brackets(token)?;
    if original.is_empty() || original.chars().any(char::is_whitespace) {
        return None;
    }
    let cleaned = validate_and_clean(&original).ok()?;
    let parsed = url::Url::parse(&cleaned.cleaned).ok()?;
    let scheme = parsed.scheme().to_lowercase();
    if scheme != "http" && scheme != "https" {
        return None;
    }
    let host_raw = parsed.host_str()?;
    if host_raw.parse::<std::net::IpAddr>().is_ok() {
        return None;
    }
    let labels: Vec<&str> = host_raw.split('.').collect();
    if labels.len() < 2 || labels.iter().any(|label| label.is_empty()) {
        return None;
    }
    let dedupe_key = match ArxivPaper::parse(&cleaned.cleaned) {
        Some(paper) => paper.dedupe_key(),
        None => dedupe_key_for(&parsed),
    };
    let route_hint = if ArxivPaper::parse(&cleaned.cleaned).is_some() {
        RouteHint::Arxiv
    } else if parsed.path().to_lowercase().ends_with(".pdf") {
        RouteHint::Pdf
    } else {
        RouteHint::Article
    };
    Some(UrlIntent {
        original,
        display: display_url(&cleaned.cleaned),
        dedupe_key,
        host: cleaned.host,
        cleaned: cleaned.cleaned,
        route_hint,
    })
}

/// Strip one optional `<…>` wrapper. Returns `None` when the shape is
/// not a single wrapper around a non-empty inner token.
fn strip_single_brackets(token: &str) -> Option<String> {
    if token.starts_with('<') && token.ends_with('>') && token.len() >= 2 {
        Some(token[1..token.len() - 1].to_string())
    } else if token.contains('<') || token.contains('>') {
        None
    } else {
        Some(token.to_string())
    }
}

/// Display form: cleaned URL without its scheme and without a leading
/// `www.`; no trailing `/`; the query is kept. Longer than 60
/// characters is elided in the middle with `…`.
pub(crate) fn display_url(cleaned: &str) -> String {
    let parsed = match url::Url::parse(cleaned) {
        Ok(parsed) => parsed,
        Err(_) => return cleaned.to_string(),
    };
    let mut host = parsed.host_str().unwrap_or_default().to_lowercase();
    if let Some(stripped) = host.strip_prefix("www.") {
        host = stripped.to_string();
    }
    let mut text = host;
    if let Some(port) = parsed.port()
        && Some(port) != parsed.port_or_known_default()
    {
        text.push_str(&format!(":{port}"));
    }
    let path = parsed.path();
    if path != "/" && !path.is_empty() {
        text.push_str(path.trim_end_matches('/'));
    }
    if let Some(query) = parsed.query() {
        text.push('?');
        text.push_str(query);
    }
    if text.chars().count() <= 60 {
        return text;
    }
    let chars: Vec<char> = text.chars().collect();
    let head: String = chars[..30].iter().collect();
    let tail: String = chars[chars.len() - 29..].iter().collect();
    format!("{head}…{tail}")
}

/// Lexical URL-list line test: a single whitespace-free token,
/// optionally `<…>`-wrapped, with an `http`/`https` scheme that parses
/// with `url::Url::parse`. No policy, validation, or config.
pub(crate) fn is_url_list_line(line: &str) -> bool {
    if line.is_empty() {
        return false;
    }
    // Column zero: no leading whitespace. Single token: no
    // whitespace anywhere.
    if line.starts_with(char::is_whitespace)
        || line.chars().any(char::is_whitespace)
    {
        return false;
    }
    let inner = match strip_single_brackets(line) {
        Some(inner) => inner,
        None => return false,
    };
    if inner.is_empty() || inner.chars().any(char::is_whitespace) {
        return false;
    }
    let lower = inner.to_lowercase();
    if !lower.starts_with("http://") && !lower.starts_with("https://") {
        return false;
    }
    url::Url::parse(&inner).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifier_table() {
        // Accepted bare URLs.
        let intent =
            classify_token("https://example.com/post").expect("classify");
        assert_eq!(intent.original, "https://example.com/post");
        assert_eq!(intent.display, "example.com/post");
        assert_eq!(intent.route_hint, RouteHint::Article);

        // Uppercase scheme and host classify; the host normalizes.
        let upper = classify_token("HTTPS://Example.com/Post")
            .expect("uppercase classifies");
        assert_eq!(upper.host, "example.com");

        // Single <> wrapper is stripped.
        let wrapped =
            classify_token("<https://x.org/a>").expect("wrapped classifies");
        assert_eq!(wrapped.original, "https://x.org/a");

        // Query punctuation and tracking parameters survive
        // classification; cleaning strips the fragment.
        let query = classify_token("https://example.com/a?a=1&b=2#frag")
            .expect("query classifies");
        assert!(query.cleaned.contains("?a=1&b=2"));
        assert!(!query.cleaned.contains("#frag"));

        // arXiv and PDF hints.
        let arxiv = classify_token("https://arxiv.org/abs/2602.16844v2")
            .expect("arxiv classifies");
        assert_eq!(arxiv.route_hint, RouteHint::Arxiv);
        assert_eq!(arxiv.dedupe_key, "https://arxiv.org/abs/2602.16844");
        let pdf = classify_token("https://example.com/paper.PDF")
            .expect("pdf classifies");
        assert_eq!(pdf.route_hint, RouteHint::Pdf);

        // Rejected: corporate short links, localhost, IP literals.
        for raw in [
            "http://go/x",
            "http://localhost:8080/a",
            "http://10.0.0.1/a",
            "http://[::1]/a",
            "http://[::ffff:10.0.0.1]/a",
            "https://a.b/c d",
            "ftp://example.com/a",
            "",
            "example.com",
            "https://user@example.com/a",
        ] {
            assert!(classify_token(raw).is_none(), "expected rejection: {raw}");
        }
        // No guessed trailing punctuation is removed: the dot stays.
        let dotted = classify_token("https://example.com/trailing.")
            .expect("trailing dot classifies");
        assert!(dotted.cleaned.contains("trailing."));

        // A trailing dot leaves an empty host label and is rejected.
        assert!(classify_token("https://example.com.").is_none());
    }

    #[test]
    fn display_elides_long_urls() {
        let long = format!("https://example.com/{}", "a".repeat(80));
        let intent = classify_token(&long).expect("long classifies");
        assert!(intent.display.chars().count() <= 60);
        assert!(intent.display.contains('…'));
    }

    #[test]
    fn url_list_lines_are_lexical() {
        assert!(is_url_list_line("https://example.com/a"));
        assert!(is_url_list_line("HTTP://example.com/a"));
        assert!(is_url_list_line("<https://example.com/a>"));
        // Lexical only: hosts that classify_token rejects still count.
        assert!(is_url_list_line("http://go/x"));
        assert!(!is_url_list_line("  https://example.com/a"));
        assert!(!is_url_list_line("https://example.com/a b"));
        assert!(!is_url_list_line("example.com"));
        assert!(!is_url_list_line(""));
        assert!(!is_url_list_line("ftp://example.com/a"));
    }
}
