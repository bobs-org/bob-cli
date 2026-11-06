//! Offline Gmail URL identity: every Gmail URL form, the base-40 token
//! decode to API ids, and a Markdown scanner that finds Gmail URLs.
//!
//! Parsing happens before any generic URL cleaning, because cleaning strips
//! the fragment that holds the id. Nothing here touches the network: a
//! decode Gmail has not confirmed is only a candidate for `gmail-save` to
//! verify with a fetch before writing.

use chrono::{DateTime, Utc};

/// Base-40 alphabet Gmail uses for `#inbox/<token>` style fragments.
pub(crate) const BASE40_ALPHABET: &str =
    "BCDFGHJKLMNPQRSTVWXZbcdfghjklmnpqrstvwxz";

/// Standard base-64 alphabet used as the intermediate encoding.
const BASE64_ALPHABET: &str =
    "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// A Gmail thread id (`f:` / `thread-f:`) or message id (`msg-f:`) that Gmail
/// can confirm, or an `rfc822msgid:` search key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum GmailRef {
    Thread { hex: String },
    Message { hex: String },
    RfcMessageId { id: String },
}

/// The verdict for one URL that is on `mail.google.com`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum GmailVerdict {
    Resolvable(GmailRef),
    Unresolvable { reason: String },
    Refused { reason: String },
}

/// A URL on `mail.google.com` plus its offline verdict.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct GmailUrl {
    pub(crate) url: String,
    pub(crate) verdict: GmailVerdict,
}

/// How a Gmail URL was wrapped in Markdown source.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum GmailWrapper {
    Bare,
    Angle,
    MarkdownLink { label: String },
}

/// One Gmail URL found in Markdown: the byte range of the whole replaceable
/// token, the URL itself, its wrapper, and its offline verdict.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct GmailUrlMatch {
    pub(crate) start: usize,
    pub(crate) end: usize,
    pub(crate) url: String,
    pub(crate) wrapper: GmailWrapper,
    pub(crate) verdict: GmailVerdict,
}

/// Decode a Gmail base-40 token to its ASCII form (for example
/// `f:1876768160496397206`). Returns `None` for empty input, characters
/// outside the base-40 alphabet, or bytes that are not UTF-8.
pub(crate) fn decode_token(token: &str) -> Option<String> {
    if token.is_empty() {
        return None;
    }
    let mut digits = Vec::with_capacity(token.len());
    for character in token.chars() {
        let value = BASE40_ALPHABET.find(character)?;
        digits.push(value);
    }
    // Arbitrary-precision base conversion over a digit vector, so no new
    // crate is needed. First accumulate the base-40 number into
    // little-endian base-1_000_000_000 limbs.
    let mut limbs: Vec<u32> = vec![0];
    for digit in digits {
        let mut carry = digit as u64;
        for limb in limbs.iter_mut() {
            let current = u64::from(*limb) * 40 + carry;
            *limb = (current % 1_000_000_000) as u32;
            carry = current / 1_000_000_000;
        }
        while carry > 0 {
            limbs.push((carry % 1_000_000_000) as u32);
            carry /= 1_000_000_000;
        }
    }
    // Then re-encode the number in base-64, most significant digit first.
    let mut working = limbs;
    let mut base64_digits: Vec<u8> = Vec::new();
    loop {
        if working.iter().all(|limb| *limb == 0) {
            break;
        }
        let mut remainder: u64 = 0;
        for limb in working.iter_mut().rev() {
            let current = remainder * 1_000_000_000 + u64::from(*limb);
            *limb = (current / 64) as u32;
            remainder = current % 64;
        }
        base64_digits.push(remainder as u8);
        while working.len() > 1 && working.last() == Some(&0) {
            working.pop();
        }
    }
    if base64_digits.is_empty() {
        base64_digits.push(0);
    }
    base64_digits.reverse();
    let mut encoded: String = base64_digits
        .iter()
        .map(|digit| BASE64_ALPHABET.as_bytes()[usize::from(*digit)] as char)
        .collect();
    while !encoded.len().is_multiple_of(4) {
        encoded.push('=');
    }
    let bytes = base64_decode(&encoded)?;
    String::from_utf8(bytes).ok()
}

fn base64_decode(encoded: &str) -> Option<Vec<u8>> {
    let mut output = Vec::with_capacity(encoded.len() / 4 * 3);
    let mut buffer: u32 = 0;
    let mut collected = 0;
    for character in encoded.chars() {
        if character == '=' {
            break;
        }
        let value = BASE64_ALPHABET.find(character)? as u32;
        buffer = (buffer << 6) | value;
        collected += 1;
        if collected == 4 {
            output.push((buffer >> 16) as u8);
            output.push((buffer >> 8) as u8);
            output.push(buffer as u8);
            buffer = 0;
            collected = 0;
        }
    }
    match collected {
        0 => {}
        2 => {
            output.push((buffer >> 4) as u8);
        }
        3 => {
            output.push((buffer >> 10) as u8);
            output.push((buffer >> 2) as u8);
        }
        _ => return None,
    }
    Some(output)
}

fn is_hex_id(text: &str) -> bool {
    text.len() == 16 && text.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn parse_dec_to_hex(dec: &str) -> Option<String> {
    let trimmed = dec.trim();
    if trimmed.is_empty() || !trimmed.bytes().all(|byte| byte.is_ascii_digit())
    {
        return None;
    }
    let value: u64 = trimmed.parse().ok()?;
    Some(format!("{value:x}"))
}

fn percent_decode_value(encoded: &str) -> String {
    let bytes = encoded.as_bytes();
    let mut output = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%'
            && index + 2 < bytes.len()
            && bytes[index + 1].is_ascii_hexdigit()
            && bytes[index + 2].is_ascii_hexdigit()
        {
            let high = (bytes[index + 1] as char).to_digit(16).unwrap_or(0);
            let low = (bytes[index + 2] as char).to_digit(16).unwrap_or(0);
            output.push((high * 16 + low) as u8);
            index += 3;
        } else if bytes[index] == b'+' {
            output.push(b' ');
            index += 1;
        } else {
            output.push(bytes[index]);
            index += 1;
        }
    }
    String::from_utf8_lossy(&output).into_owned()
}

fn canonical_percent_encode(text: &str) -> String {
    let mut encoded = String::with_capacity(text.len());
    for byte in text.bytes() {
        if byte.is_ascii_alphanumeric()
            || matches!(byte, b'-' | b'_' | b'.' | b'~')
        {
            encoded.push(byte as char);
        } else {
            encoded.push_str(&format!("%{byte:02X}"));
        }
    }
    encoded
}

/// The date hint for a thread hex id: `id >> 20` is epoch milliseconds.
pub(crate) fn thread_date_hint(hex: &str) -> Option<DateTime<Utc>> {
    let trimmed = hex.trim();
    if trimmed.is_empty()
        || !trimmed.bytes().all(|byte| byte.is_ascii_hexdigit())
    {
        return None;
    }
    let value = u64::from_str_radix(trimmed, 16).ok()?;
    let millis = value >> 20;
    DateTime::from_timestamp_millis(i64::try_from(millis).ok()?)
}

/// The canonical link back to a thread for the configured account.
pub(crate) fn canonical_url(account: &str, hex: &str) -> String {
    format!(
        "https://mail.google.com/mail/?authuser={}#all/{hex}",
        canonical_percent_encode(account.trim()),
    )
}

fn verdict_for_token(token: &str) -> GmailVerdict {
    if token.is_empty() {
        return GmailVerdict::Unresolvable {
            reason: "no_token".to_string(),
        };
    }
    if is_hex_id(token) {
        return GmailVerdict::Resolvable(GmailRef::Thread {
            hex: token.to_ascii_lowercase(),
        });
    }
    let Some(decoded) = decode_token(token) else {
        return GmailVerdict::Unresolvable {
            reason: "bad_token".to_string(),
        };
    };
    if let Some(rest) = decoded.strip_prefix("thread-f:") {
        return match parse_dec_to_hex(rest) {
            Some(hex) => GmailVerdict::Resolvable(GmailRef::Thread { hex }),
            None => GmailVerdict::Unresolvable {
                reason: "bad_thread_id".to_string(),
            },
        };
    }
    if let Some(rest) = decoded.strip_prefix("f:") {
        return match parse_dec_to_hex(rest) {
            Some(hex) => GmailVerdict::Resolvable(GmailRef::Thread { hex }),
            None => GmailVerdict::Unresolvable {
                reason: "bad_thread_id".to_string(),
            },
        };
    }
    if let Some(rest) = decoded.strip_prefix("msg-f:") {
        return match parse_dec_to_hex(rest) {
            Some(hex) => GmailVerdict::Resolvable(GmailRef::Message { hex }),
            None => GmailVerdict::Unresolvable {
                reason: "bad_message_id".to_string(),
            },
        };
    }
    if decoded.starts_with("thread-a:") || decoded.starts_with("a:") {
        return GmailVerdict::Unresolvable {
            reason: "attachment_token".to_string(),
        };
    }
    GmailVerdict::Unresolvable {
        reason: "unknown_token".to_string(),
    }
}

/// Parse one URL string. Returns `None` when the input is not a Gmail URL;
/// otherwise the offline verdict: `Resolvable`, `Unresolvable`, or `Refused`.
pub(crate) fn parse_gmail_url(raw: &str) -> Option<GmailUrl> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    let parsed = url::Url::parse(trimmed).ok()?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return None;
    }
    let host = parsed.host_str().unwrap_or_default().to_ascii_lowercase();
    if host != "mail.google.com" {
        return None;
    }
    let path = parsed.path();
    if path == "/chat" || path.starts_with("/chat/") {
        return Some(GmailUrl {
            url: trimmed.to_string(),
            verdict: GmailVerdict::Refused {
                reason: "chat links are not email threads".to_string(),
            },
        });
    }
    let query_raw = parsed.query().unwrap_or_default();
    if query_raw
        .split('&')
        .any(|pair| pair == "compose" || pair.starts_with("compose="))
        || query_raw.contains("compose=")
    {
        return Some(GmailUrl {
            url: trimmed.to_string(),
            verdict: GmailVerdict::Refused {
                reason: "compose links are not email threads".to_string(),
            },
        });
    }
    let mut query_thread: Option<GmailVerdict> = None;
    let mut query_message: Option<GmailVerdict> = None;
    for (key, value) in parsed.query_pairs() {
        match key.as_ref() {
            "th" => {
                let candidate = value.trim().to_string();
                if is_hex_id(&candidate) {
                    query_thread =
                        Some(GmailVerdict::Resolvable(GmailRef::Thread {
                            hex: candidate.to_ascii_lowercase(),
                        }));
                } else if !candidate.is_empty() {
                    query_thread = Some(GmailVerdict::Unresolvable {
                        reason: "bad_th".to_string(),
                    });
                }
            }
            "permthid" => {
                let candidate = value.trim().to_string();
                match candidate.strip_prefix("thread-f:") {
                    Some(rest) => match parse_dec_to_hex(rest) {
                        Some(hex) => {
                            query_thread = Some(GmailVerdict::Resolvable(
                                GmailRef::Thread { hex },
                            ));
                        }
                        None => {
                            query_thread = Some(GmailVerdict::Unresolvable {
                                reason: "bad_thread_id".to_string(),
                            });
                        }
                    },
                    None => {
                        query_thread = Some(GmailVerdict::Unresolvable {
                            reason: "bad_permthid".to_string(),
                        });
                    }
                }
            }
            "permmsgid" => {
                let candidate = value.trim().to_string();
                match candidate.strip_prefix("msg-f:") {
                    Some(rest) => match parse_dec_to_hex(rest) {
                        Some(hex) => {
                            query_message = Some(GmailVerdict::Resolvable(
                                GmailRef::Message { hex },
                            ));
                        }
                        None => {
                            query_message = Some(GmailVerdict::Unresolvable {
                                reason: "bad_message_id".to_string(),
                            });
                        }
                    },
                    None => {
                        query_message = Some(GmailVerdict::Unresolvable {
                            reason: "bad_permmsgid".to_string(),
                        });
                    }
                }
            }
            _ => {}
        }
    }
    if let Some(verdict) = query_thread {
        return Some(GmailUrl {
            url: trimmed.to_string(),
            verdict,
        });
    }
    if let Some(verdict) = query_message {
        return Some(GmailUrl {
            url: trimmed.to_string(),
            verdict,
        });
    }
    let Some(fragment) = parsed.fragment() else {
        return Some(GmailUrl {
            url: trimmed.to_string(),
            verdict: GmailVerdict::Unresolvable {
                reason: "no_token".to_string(),
            },
        });
    };
    let decoded_fragment = percent_decode_value(fragment);
    if decoded_fragment.contains("compose=") || fragment.contains("compose=") {
        return Some(GmailUrl {
            url: trimmed.to_string(),
            verdict: GmailVerdict::Refused {
                reason: "compose links are not email threads".to_string(),
            },
        });
    }
    if let Some(index) = decoded_fragment.find("rfc822msgid:") {
        let after = decoded_fragment[index + "rfc822msgid:".len()..]
            .split(['?', '&'])
            .next()
            .unwrap_or_default();
        let decoded = after.trim().to_string();
        if decoded.is_empty() {
            return Some(GmailUrl {
                url: trimmed.to_string(),
                verdict: GmailVerdict::Unresolvable {
                    reason: "bad_rfc822msgid".to_string(),
                },
            });
        }
        return Some(GmailUrl {
            url: trimmed.to_string(),
            verdict: GmailVerdict::Resolvable(GmailRef::RfcMessageId {
                id: decoded,
            }),
        });
    }
    let main = fragment.split('?').next().unwrap_or_default();
    let token = main.rsplit('/').next().unwrap_or_default();
    if token.is_empty() || main.trim().is_empty() {
        return Some(GmailUrl {
            url: trimmed.to_string(),
            verdict: GmailVerdict::Unresolvable {
                reason: "no_token".to_string(),
            },
        });
    }
    Some(GmailUrl {
        url: trimmed.to_string(),
        verdict: verdict_for_token(token),
    })
}

fn frontmatter_byte_end(markdown: &str) -> Option<usize> {
    let mut offset = 0;
    let mut lines = markdown.split_inclusive('\n').peekable();
    let first = lines.next()?;
    if first.trim_end_matches(['\r', '\n']) != "---" {
        return None;
    }
    offset += first.len();
    for line in lines {
        let content = line.trim_end_matches(['\r', '\n']);
        offset += line.len();
        if content == "---" {
            return Some(offset);
        }
    }
    None
}

fn fence_marker(line: &str) -> Option<(u8, usize)> {
    let indentation = line.bytes().take_while(|byte| *byte == b' ').count();
    if indentation > 3 {
        return None;
    }
    let rest = &line[indentation..];
    let character = *rest.as_bytes().first()?;
    if !matches!(character, b'`' | b'~') {
        return None;
    }
    let length = rest.bytes().take_while(|byte| *byte == character).count();
    (length >= 3).then_some((character, length))
}

fn closes_fence(line: &str, open: (u8, usize)) -> bool {
    let Some((character, length)) = fence_marker(line) else {
        return false;
    };
    if character != open.0 || length < open.1 {
        return false;
    }
    let trimmed = line.trim_start();
    trimmed[length..].trim().is_empty()
}

fn inline_code_ranges(line: &str, offset: usize) -> Vec<(usize, usize)> {
    let bytes = line.as_bytes();
    let mut ranges = Vec::new();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] != b'`' || is_escaped(line, index) {
            index += 1;
            continue;
        }
        let tick_count = bytes[index..]
            .iter()
            .take_while(|byte| **byte == b'`')
            .count();
        let content_start = index + tick_count;
        let mut close = content_start;
        let mut found = false;
        while close < bytes.len() {
            if bytes[close] == b'`'
                && !is_escaped(line, close)
                && bytes[close..]
                    .iter()
                    .take_while(|byte| **byte == b'`')
                    .count()
                    == tick_count
            {
                ranges.push((offset + index, offset + close + tick_count));
                index = close + tick_count;
                found = true;
                break;
            }
            close += 1;
        }
        if !found {
            ranges.push((offset + index, offset + bytes.len()));
            break;
        }
    }
    ranges
}

fn is_escaped(text: &str, index: usize) -> bool {
    let bytes = text.as_bytes();
    let mut cursor = index;
    let mut count = 0;
    while cursor > 0 && bytes[cursor - 1] == b'\\' {
        count += 1;
        cursor -= 1;
    }
    count % 2 == 1
}

fn code_ranges(markdown: &str) -> Vec<(usize, usize)> {
    let mut ranges = Vec::new();
    let mut offset = 0;
    let mut fence: Option<(u8, usize)> = None;
    for line in markdown.split_inclusive('\n') {
        let content = line.trim_end_matches(['\r', '\n']);
        let length = line.len();
        if let Some(open) = fence {
            ranges.push((offset, offset + length));
            if closes_fence(content, open) {
                fence = None;
            }
        } else if let Some(open) = fence_marker(content) {
            ranges.push((offset, offset + length));
            fence = Some(open);
        } else {
            ranges.extend(inline_code_ranges(content, offset));
        }
        offset += length;
    }
    ranges
}

fn wikilink_ranges(markdown: &str) -> Vec<(usize, usize)> {
    let bytes = markdown.as_bytes();
    let mut ranges = Vec::new();
    let mut index = 0;
    while index + 1 < bytes.len() {
        if bytes[index] == b'['
            && bytes[index + 1] == b'['
            && !is_escaped(markdown, index)
        {
            let mut close = index + 2;
            let mut found = None;
            while close + 1 < bytes.len() {
                if bytes[close] == b']'
                    && bytes[close + 1] == b']'
                    && !is_escaped(markdown, close)
                {
                    found = Some(close + 2);
                    break;
                }
                close += 1;
            }
            if let Some(end) = found {
                ranges.push((index, end));
                index = end;
                continue;
            }
            break;
        }
        index += 1;
    }
    ranges
}

fn overlaps(ranges: &[(usize, usize)], start: usize, end: usize) -> bool {
    ranges.iter().any(|(range_start, range_end)| {
        *range_start < end && start < *range_end
    })
}

fn is_url_terminator(byte: u8) -> bool {
    matches!(
        byte,
        b' ' | b'\t' | b'\n' | b'\r' | b'<' | b'>' | b'"' | b'\'' | b'`'
    )
}

fn trim_bare_end(candidate: &str) -> &str {
    let mut end = candidate.len();
    // Strip trailing sentence punctuation first.
    while end > 0 {
        let byte = candidate.as_bytes()[end - 1];
        if matches!(byte, b'.' | b',' | b';' | b':' | b'!' | b'?') {
            end -= 1;
        } else {
            break;
        }
    }
    // Strip an unbalanced closing bracket or paren.
    while end > 0 {
        let byte = candidate.as_bytes()[end - 1];
        if byte == b')' {
            let slice = &candidate[..end];
            let opens = slice.bytes().filter(|unit| *unit == b'(').count();
            let closes = slice.bytes().filter(|unit| *unit == b')').count();
            if closes > opens {
                end -= 1;
                continue;
            }
            break;
        }
        if byte == b']' {
            let slice = &candidate[..end];
            let opens = slice.bytes().filter(|unit| *unit == b'[').count();
            let closes = slice.bytes().filter(|unit| *unit == b']').count();
            if closes > opens {
                end -= 1;
                continue;
            }
            break;
        }
        break;
    }
    &candidate[..end]
}

/// Find every Gmail URL in Markdown source, with its wrapper and verdict.
///
/// Skips fenced code, inline code, frontmatter, and existing wikilinks.
/// Trailing sentence punctuation (`.,;:!?`) is not part of a bare URL
/// unless it is balanced; an unbalanced closing `)` or `]` is trimmed.
pub(crate) fn find_gmail_urls(markdown: &str) -> Vec<GmailUrlMatch> {
    let mut excluded: Vec<(usize, usize)> = Vec::new();
    if let Some(end) = frontmatter_byte_end(markdown) {
        excluded.push((0, end));
    }
    excluded.extend(code_ranges(markdown));
    excluded.extend(wikilink_ranges(markdown));

    let bytes = markdown.as_bytes();
    let mut matches = Vec::new();
    let mut index = 0;
    while index < bytes.len() {
        if !markdown.is_char_boundary(index) {
            index += 1;
            continue;
        }
        let rest = &markdown[index..];
        let is_https = rest.starts_with("https://");
        let is_http =
            !is_https && rest.starts_with("http://") && rest.len() > 7;
        if !is_https && !is_http {
            index += 1;
            continue;
        }
        let url_start = index;
        // Markdown-link wrapper: `[...](url)` immediately before the URL.
        let mut link_label_start: Option<usize> = None;
        if url_start >= 2
            && bytes[url_start - 2] == b']'
            && bytes[url_start - 1] == b'('
        {
            let before = &markdown[..url_start - 2];
            let line_start = before.rfind('\n').map_or(0, |pos| pos + 1);
            if let Some(relative) = before[line_start..].rfind('[') {
                let candidate = line_start + relative;
                if !is_escaped(markdown, candidate) {
                    link_label_start = Some(candidate);
                }
            }
        }
        // Angle wrapper: `<url` immediately before the URL.
        let angled = url_start > 0 && bytes[url_start - 1] == b'<';
        if let Some(label_start) = link_label_start {
            let mut close = url_start;
            while close < bytes.len()
                && !matches!(bytes[close], b')' | b'"' | b'\'')
                && !bytes[close].is_ascii_whitespace()
            {
                close += 1;
            }
            let url_end = close;
            // Skip whitespace to the closing paren (titles are not Gmail).
            let mut paren = url_end;
            while paren < bytes.len() && matches!(bytes[paren], b' ' | b'\t') {
                paren += 1;
            }
            if paren < bytes.len() && bytes[paren] == b')' {
                let url = markdown[url_start..url_end].to_string();
                let end = paren + 1;
                if !url.is_empty()
                    && let Some(parsed) = parse_gmail_url(&url)
                    && !overlaps(&excluded, label_start, end)
                {
                    matches.push(GmailUrlMatch {
                        start: label_start,
                        end,
                        url: url.clone(),
                        wrapper: GmailWrapper::MarkdownLink {
                            label: markdown[label_start + 1..url_start - 2]
                                .to_string(),
                        },
                        verdict: parsed.verdict,
                    });
                }
                index = end.max(url_start + 1);
                continue;
            }
            // Fall through to bare handling when the paren never closes.
        }
        if angled {
            let mut end = url_start;
            while end < bytes.len()
                && !is_url_terminator(bytes[end])
                && end - url_start < 4096
            {
                end += 1;
            }
            let has_close = end < bytes.len() && bytes[end] == b'>';
            let url = markdown[url_start..end].to_string();
            let token_end = if has_close { end + 1 } else { end };
            if !url.is_empty()
                && let Some(parsed) = parse_gmail_url(&url)
                && !overlaps(&excluded, url_start - 1, token_end)
            {
                matches.push(GmailUrlMatch {
                    start: url_start - 1,
                    end: token_end,
                    url: url.clone(),
                    wrapper: GmailWrapper::Angle,
                    verdict: parsed.verdict,
                });
            }
            index = token_end.max(url_start + 1);
            continue;
        }
        let mut end = url_start;
        while end < bytes.len()
            && !is_url_terminator(bytes[end])
            && end - url_start < 4096
        {
            end += 1;
        }
        let raw = &markdown[url_start..end];
        let trimmed = trim_bare_end(raw);
        if trimmed.is_empty() {
            index = url_start + 1;
            continue;
        }
        let token_end = url_start + trimmed.len();
        if let Some(parsed) = parse_gmail_url(trimmed)
            && !overlaps(&excluded, url_start, token_end)
        {
            matches.push(GmailUrlMatch {
                start: url_start,
                end: token_end,
                url: trimmed.to_string(),
                wrapper: GmailWrapper::Bare,
                verdict: parsed.verdict,
            });
        }
        index = token_end.max(url_start + 1);
    }
    matches
}

#[cfg(test)]
mod tests {
    use super::*;

    fn thread_hex(url: &str) -> Option<String> {
        match parse_gmail_url(url)?.verdict {
            GmailVerdict::Resolvable(GmailRef::Thread { hex }) => Some(hex),
            _ => None,
        }
    }

    #[test]
    fn section_4_1_test_vector_decodes() {
        assert_eq!(
            decode_token("FMfcgzQhWTsNRgxMhQNwhZDfgqwXpPsx").as_deref(),
            Some("f:1876768160496397206"),
        );
        assert_eq!(
            thread_hex(
                "https://mail.google.com/mail/u/0/#inbox/FMfcgzQhWTsNRgxMhQNwhZDfgqwXpPsx"
            )
            .as_deref(),
            Some("1a0b9eb2cc26ab96"),
        );
        let hint = thread_date_hint("1a0b9eb2cc26ab96").expect("hint");
        assert_eq!(
            hint.to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
            "2026-09-19T13:46:32Z",
        );
    }

    #[test]
    fn round_trip_random_ids() {
        // Simple xorshift generator: no dev-dependency needed.
        let mut state: u64 = 0x9E37_79B9_7F4A_7C15;
        let mut next = move || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state
        };
        for _ in 0..32 {
            let id = next();
            let ascii = format!("f:{id}");
            let token = encode_base40(&ascii).expect("encode");
            assert_eq!(decode_token(&token).as_deref(), Some(ascii.as_str()));
        }
    }

    fn encode_base40(ascii: &str) -> Option<String> {
        if ascii.is_empty() {
            return None;
        }
        let encoded = base64_encode(ascii.as_bytes());
        let stripped = encoded.trim_end_matches('=');
        let mut digits = Vec::with_capacity(stripped.len());
        for character in stripped.chars() {
            digits.push(BASE64_ALPHABET.find(character)?);
        }
        let mut limbs: Vec<u32> = vec![0];
        for digit in digits {
            let mut carry = digit as u64;
            for limb in limbs.iter_mut() {
                let current = u64::from(*limb) * 64 + carry;
                *limb = (current % 1_000_000_000) as u32;
                carry = current / 1_000_000_000;
            }
            while carry > 0 {
                limbs.push((carry % 1_000_000_000) as u32);
                carry /= 1_000_000_000;
            }
        }
        let mut working = limbs;
        let mut out: Vec<usize> = Vec::new();
        loop {
            if working.iter().all(|limb| *limb == 0) {
                break;
            }
            let mut remainder: u64 = 0;
            for limb in working.iter_mut().rev() {
                let current = remainder * 1_000_000_000 + u64::from(*limb);
                *limb = (current / 40) as u32;
                remainder = current % 40;
            }
            out.push(remainder as usize);
            while working.len() > 1 && working.last() == Some(&0) {
                working.pop();
            }
        }
        out.reverse();
        if out.is_empty() {
            out.push(0);
        }
        Some(
            out.into_iter()
                .map(|digit| BASE40_ALPHABET.as_bytes()[digit] as char)
                .collect(),
        )
    }

    fn base64_encode(bytes: &[u8]) -> String {
        let mut out = String::new();
        for chunk in bytes.chunks(3) {
            let a = u32::from(chunk[0]);
            let b = if chunk.len() > 1 {
                u32::from(chunk[1])
            } else {
                0
            };
            let c = if chunk.len() > 2 {
                u32::from(chunk[2])
            } else {
                0
            };
            let triple = (a << 16) | (b << 8) | c;
            let alphabet = BASE64_ALPHABET.as_bytes();
            out.push(alphabet[((triple >> 18) & 63) as usize] as char);
            out.push(alphabet[((triple >> 12) & 63) as usize] as char);
            if chunk.len() > 1 {
                out.push(alphabet[((triple >> 6) & 63) as usize] as char);
            } else {
                out.push('=');
            }
            if chunk.len() > 2 {
                out.push(alphabet[(triple & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
        out
    }

    #[test]
    fn every_section_4_1_form_parses() {
        // Every view shape resolves the same thread token to the hex id.
        for view in [
            "inbox",
            "all",
            "sent",
            "starred",
            "imp",
            "snoozed",
            "label/Work",
            "search/query",
            "category/social",
        ] {
            let url = format!(
                "https://mail.google.com/mail/u/0/#{view}/FMfcgzQhWTsNRgxMhQNwhZDfgqwXpPsx"
            );
            assert_eq!(
                thread_hex(&url).as_deref(),
                Some("1a0b9eb2cc26ab96"),
                "view {view}",
            );
        }
        // /u/N/ and authuser= are ignored.
        assert_eq!(
            thread_hex(
                "https://mail.google.com/mail/u/1/#all/1a0b9eb2cc26ab96"
            )
            .as_deref(),
            Some("1a0b9eb2cc26ab96"),
        );
        assert_eq!(
            thread_hex(
                "https://mail.google.com/mail/?authuser=you%40example.com#all/1a0b9eb2cc26ab96"
            )
            .as_deref(),
            Some("1a0b9eb2cc26ab96"),
        );
        // Legacy fragment hex and query forms.
        assert_eq!(
            thread_hex(
                "https://mail.google.com/mail/u/0/#all/1A0B9EB2CC26AB96"
            )
            .as_deref(),
            Some("1a0b9eb2cc26ab96"),
        );
        assert_eq!(
            thread_hex(
                "https://mail.google.com/mail/?th=1a0b9eb2cc26ab96#inbox"
            )
            .as_deref(),
            Some("1a0b9eb2cc26ab96"),
        );
        assert_eq!(
            thread_hex(
                "https://mail.google.com/mail/?permthid=thread-f%3A1876768160496397206"
            )
            .as_deref(),
            Some("1a0b9eb2cc26ab96"),
        );
        // permmsgid is a message verdict needing messages.get.
        match parse_gmail_url(
            "https://mail.google.com/mail/?permmsgid=msg-f%3A1876768160496397206",
        )
        .expect("parse")
        .verdict
        {
            GmailVerdict::Resolvable(GmailRef::Message { hex }) => {
                assert_eq!(hex, "1a0b9eb2cc26ab96");
            }
            other => panic!("expected message verdict, got {other:?}"),
        }
        // rfc822msgid search, percent-decoded.
        match parse_gmail_url(
            "https://mail.google.com/mail/u/0/#search/rfc822msgid%3ACAH8xQ%40mail.gmail.com",
        )
        .expect("parse")
        .verdict
        {
            GmailVerdict::Resolvable(GmailRef::RfcMessageId { id }) => {
                assert_eq!(id, "CAH8xQ@mail.gmail.com");
            }
            other => panic!("expected rfc verdict, got {other:?}"),
        }
        // a: tokens are unresolvable offline.
        let attachment_token =
            encode_base40("a:12345").expect("encode attachment");
        match parse_gmail_url(&format!(
            "https://mail.google.com/mail/u/0/#inbox/{attachment_token}"
        ))
        .expect("parse")
        .verdict
        {
            GmailVerdict::Unresolvable { .. } => {}
            other => panic!("expected unresolvable, got {other:?}"),
        }
    }

    #[test]
    fn refusals_and_non_gmail() {
        assert!(parse_gmail_url("https://example.com/post").is_none());
        for url in [
            "https://mail.google.com/mail/u/0/?compose=new",
            "https://mail.google.com/mail/u/0/#inbox?compose=new",
            "https://mail.google.com/chat/u/0/#chat/welcome",
        ] {
            match parse_gmail_url(url).expect("parse").verdict {
                GmailVerdict::Refused { .. } => {}
                other => panic!("expected refused for {url}, got {other:?}"),
            }
        }
    }

    #[test]
    fn malformed_tokens_never_yield_ids() {
        for url in [
            "https://mail.google.com/mail/u/0/#inbox/",
            "https://mail.google.com/mail/u/0/#inbox",
            "https://mail.google.com/mail/u/0/",
            "https://mail.google.com/mail/u/0/#inbox/!!!",
            "https://mail.google.com/mail/u/0/#inbox/BBQ",
            "https://mail.google.com/mail/?th=xyz#inbox",
            "https://mail.google.com/mail/?permthid=bogus",
            "https://mail.google.com/mail/?permmsgid=bogus",
            "https://mail.google.com/mail/u/0/#inbox/zzz",
        ] {
            let parsed = parse_gmail_url(url).expect("gmail url");
            assert!(
                !matches!(parsed.verdict, GmailVerdict::Resolvable(_)),
                "must not resolve {url}: {:?}",
                parsed.verdict
            );
        }
        assert!(decode_token("").is_none());
        assert!(decode_token("***").is_none());
        assert!(decode_token("ABC0").is_none());
        assert!(thread_date_hint("xyz").is_none());
        assert!(thread_date_hint("").is_none());
    }

    #[test]
    fn canonical_url_encodes_account() {
        assert_eq!(
            canonical_url("you@example.com", "1a0b9eb2cc26ab96"),
            "https://mail.google.com/mail/?authuser=you%40example.com#all/1a0b9eb2cc26ab96",
        );
    }

    #[test]
    fn scanner_covers_wrappers_and_skips() {
        let inbox = "https://mail.google.com/mail/u/0/#inbox/FMfcgzQhWTsNRgxMhQNwhZDfgqwXpPsx";
        let hex = "https://mail.google.com/mail/u/0/#all/1a0b9eb2cc26ab96";
        let markdown = format!(
            "First {inbox} and again {inbox}.\n\
             <{hex}>\n\
             [their words]({hex})\n\
             `code {inbox}`\n\
             ```md\n{inbox}\n```\n\
             (see {hex}.)\n\
             Unicode déjà {hex} fin.\n\
             [[{hex}]]\n"
        );
        let found = find_gmail_urls(&markdown);
        // Two identical bare URLs + angle + markdown link + parens + unicode.
        assert_eq!(found.len(), 6, "found: {found:?}");
        assert!(matches!(found[0].wrapper, GmailWrapper::Bare));
        assert_eq!(found[0].url, inbox);
        assert_eq!(&markdown[found[0].start..found[0].end], inbox);
        assert!(matches!(found[2].wrapper, GmailWrapper::Angle));
        assert!(matches!(
            found[3].wrapper,
            GmailWrapper::MarkdownLink { .. }
        ));
        // Trailing sentence punctuation is trimmed.
        let punctuated = format!("See {hex}.");
        let single = find_gmail_urls(&punctuated);
        assert_eq!(single.len(), 1);
        assert_eq!(single[0].url, hex);
        // Frontmatter is skipped.
        let with_frontmatter =
            format!("---\ntitle: x\n---\n{inbox}\n---\n{inbox}\n");
        let front = find_gmail_urls(&with_frontmatter);
        assert_eq!(front.len(), 2, "frontmatter skip: {front:?}");
        assert!(
            front.iter().all(|item| item.start > 15),
            "no match inside frontmatter: {front:?}"
        );
    }
}
