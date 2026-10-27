//! Pure naming and metadata rules for multi-URL Highlights collections.
//!
//! This stays separate from the capture route so every naming decision can
//! run before network or adapter work begins.

use std::collections::BTreeSet;

use chrono::NaiveDate;

use super::clip_url::{
    clean_segment, humanize_stem, percent_decode, snake_case, validate_name,
    WebUrl,
};
use super::{CommandError, Result};

// These lists intentionally stay small and strict. A false refusal only asks
// for an explicit name; a false positive becomes a durable filename and link.
const FILLER_WORDS: &[&str] = &[
    "a", "an", "and", "at", "by", "for", "from", "in", "of", "on", "or", "the",
    "to", "vs", "with",
];
const SERIES_WORDS: &[&str] = &[
    "part", "pt", "chapter", "ch", "section", "episode", "ep", "lesson", "vol",
    "volume", "no", "series", "page",
];
const GENERIC_WORDS: &[&str] = &[
    "about",
    "api",
    "basics",
    "book",
    "config",
    "configuration",
    "core",
    "doc",
    "docs",
    "documentation",
    "faq",
    "getting",
    "guide",
    "guides",
    "handbook",
    "help",
    "home",
    "index",
    "install",
    "installation",
    "intro",
    "introduction",
    "latest",
    "main",
    "manual",
    "notes",
    "overview",
    "quickstart",
    "readme",
    "reference",
    "setup",
    "started",
    "tutorial",
    "tutorials",
    "usage",
    "user",
    "wiki",
];
const NUMBERED_QUERY_PARAMS: &[&str] = &["page", "part", "p"];
const TITLE_ORDINAL_WORDS: &[&str] = &[
    "part", "pt", "chapter", "ch", "episode", "ep", "lesson", "vol", "volume",
    "no",
];
const ROMAN_NUMERALS: &[&str] = &[
    "i", "ii", "iii", "iv", "v", "vi", "vii", "viii", "ix", "x", "xi", "xii",
    "xiii", "xiv", "xv", "xvi", "xvii", "xviii", "xix", "xx",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum NameSource {
    NameFlag,
    OutputFile,
    SlugWords,
    Folder,
    TitleFlag,
    Reread,
    Overwrite,
}

impl NameSource {
    pub(super) fn report_phrase(self) -> &'static str {
        match self {
            Self::NameFlag => "from -N",
            Self::OutputFile => "from the -o filename",
            Self::SlugWords => "words every URL starts with; -N overrides",
            Self::Folder => "the folder every URL shares; -N overrides",
            Self::TitleFlag => "from -T; -N overrides",
            Self::Reread => {
                "reread of the exact URL set; -N selects the family"
            }
            Self::Overwrite => {
                "overwrite of the exact URL set; -N selects the reading"
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum NoNameReason {
    DifferentSites(Vec<String>),
    NoSlug(usize),
    TooThin(String),
    NothingShared,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum NameOutcome {
    Derived { name: String, source: NameSource },
    NoName(NoNameReason),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ResolvedName {
    pub(super) name: String,
    pub(super) source: NameSource,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SharedWords {
    words: Vec<String>,
    /// First token of the retained prefix in the source slug. This lets the
    /// part-order check look beyond leading filler words.
    start: usize,
}

/// Derive a collection name from cleaned URLs without performing I/O.
pub(super) fn derive_collection_name(urls: &[WebUrl]) -> NameOutcome {
    if urls.is_empty() {
        return NameOutcome::NoName(NoNameReason::NothingShared);
    }

    let sites = unique_sites(urls);
    if sites.len() != 1 {
        return NameOutcome::NoName(NoNameReason::DifferentSites(sites));
    }

    let parsed: Vec<url::Url> = urls
        .iter()
        .filter_map(|web_url| url::Url::parse(&web_url.cleaned).ok())
        .collect();
    if parsed.len() != urls.len() {
        let index = parsed.len() + 1;
        return NameOutcome::NoName(NoNameReason::NoSlug(index));
    }

    // A path such as /item?id=1 is not a reliable slug. When only the query
    // varies, accept it as a series only when the changing keys are page-like.
    if same_path(&parsed) && query_changes_are_not_numbered(&parsed) {
        return NameOutcome::NoName(NoNameReason::NoSlug(1));
    }

    let slugs: Vec<Vec<String>> = parsed.iter().map(slug_words).collect();
    let Some((missing_index, _)) =
        slugs.iter().enumerate().find(|(_, words)| words.is_empty())
    else {
        let shared = shared_words(&slugs);
        if is_useful_name(&shared.words, allows_one_word_series(urls, &parsed))
            && let Some(name) = checked_derived_name(&shared.words)
        {
            return NameOutcome::Derived {
                name,
                source: NameSource::SlugWords,
            };
        }

        let mut fallback_reason = if shared.words.is_empty() {
            NoNameReason::NothingShared
        } else {
            NoNameReason::TooThin(shared.words.join(" "))
        };

        if let Some(folder) = shared_folder_words(&parsed)
            && !folder.is_empty()
        {
            if is_useful_name(&folder, false)
                && let Some(name) = checked_derived_name(&folder)
            {
                return NameOutcome::Derived {
                    name,
                    source: NameSource::Folder,
                };
            }
            if fallback_reason == NoNameReason::NothingShared {
                fallback_reason = NoNameReason::TooThin(folder.join(" "));
            }
        }
        return NameOutcome::NoName(fallback_reason);
    };

    NameOutcome::NoName(NoNameReason::NoSlug(missing_index + 1))
}

/// Apply the collection name waterfall: `-N`, `-o`, shared URL words, `-T`.
pub(super) fn resolve_collection_name(
    name_flag: Option<&str>,
    output_stem: Option<&str>,
    title_flag: Option<&str>,
    urls: &[WebUrl],
) -> Result<ResolvedName> {
    if let Some(name) = name_flag {
        return Ok(ResolvedName {
            name: validate_name(name)?,
            source: NameSource::NameFlag,
        });
    }
    if let Some(stem) = output_stem {
        return Ok(ResolvedName {
            name: validate_name(stem)?,
            source: NameSource::OutputFile,
        });
    }

    match derive_collection_name(urls) {
        NameOutcome::Derived { name, source } => {
            Ok(ResolvedName { name, source })
        }
        NameOutcome::NoName(reason) => {
            if let Some(title) = title_flag {
                let name = snake_case(title);
                if !name.is_empty() {
                    return Ok(ResolvedName {
                        name: validate_name(&name)?,
                        source: NameSource::TitleFlag,
                    });
                }
            }
            Err(CommandError::new(name_error_message(urls, &reason))
                .with_exit_code(2))
        }
    }
}

/// Render the plain, stable missing-name diagnostic for the create command.
pub(super) fn name_error_message(
    urls: &[WebUrl],
    reason: &NoNameReason,
) -> String {
    let mut lines = vec![format!(
        "bob ref create: error: these {} URLs need a reference name",
        urls.len()
    )];
    for (index, web_url) in urls.iter().enumerate() {
        lines.push(format!(
            "  {}  {}",
            index + 1,
            without_scheme(&web_url.cleaned)
        ));
    }
    let explanation = match reason {
        NoNameReason::DifferentSites(sites) => {
            format!("they come from different sites ({})", sites.join(", "))
        }
        NoNameReason::NoSlug(index) => {
            format!("URL {index} has no slug to name from")
        }
        NoNameReason::TooThin(shared) => {
            format!("they only share \"{shared}\"")
        }
        NoNameReason::NothingShared => {
            "their slugs share no common start".into()
        }
    };
    lines.push(format!(
        "hint: {explanation}; pass -N <name> (or -T <title>)"
    ));
    lines.join("\n")
}

/// Warn when URL arguments disagree with the numbered part order.
pub(super) fn part_order_warning(urls: &[WebUrl]) -> Option<String> {
    let parsed: Vec<url::Url> = urls
        .iter()
        .filter_map(|web_url| url::Url::parse(&web_url.cleaned).ok())
        .collect();
    if parsed.len() != urls.len() || parsed.len() < 2 {
        return None;
    }
    let slugs: Vec<Vec<String>> = parsed.iter().map(slug_words).collect();
    let shared = shared_words(&slugs);
    let query_numbers: Vec<Option<u64>> =
        parsed.iter().map(numbered_query_value).collect();
    let query_numbers_are_series = query_numbers.iter().all(Option::is_some)
        && query_numbers.windows(2).any(|pair| pair[0] != pair[1]);
    let numbers: Option<Vec<u64>> = parsed
        .iter()
        .zip(&slugs)
        .zip(&query_numbers)
        .map(|((_, words), query_number)| {
            query_numbers_are_series
                .then_some(*query_number)
                .flatten()
                .or_else(|| {
                    words
                        .iter()
                        .skip(shared.start + shared.words.len())
                        .find_map(|word| word.parse().ok())
                })
        })
        .collect();
    let numbers = numbers?;
    if numbers.windows(2).all(|pair| pair[0] <= pair[1]) {
        return None;
    }
    Some(format!(
        "warning: the URLs are in part order {}; the PDF follows argument order",
        numbers
            .iter()
            .map(u64::to_string)
            .collect::<Vec<_>>()
            .join(", ")
    ))
}

/// Select the shared title, an explicit override, or a humanized-name fallback.
pub(super) fn collection_title(
    part_titles: &[String],
    title_flag: Option<&str>,
    name: &str,
) -> String {
    if let Some(title) =
        title_flag.map(str::trim).filter(|title| !title.is_empty())
    {
        return title.to_string();
    }
    let shared = shared_title_prefix(part_titles);
    let shared = trim_title_ordinals(&shared);
    if meaningful_title_word_count(&shared) >= 2 {
        shared
    } else {
        humanize_stem(name)
    }
}

/// Choose an author override or combine distinct per-part authors in order.
pub(super) fn collection_author(
    part_authors: &[Option<String>],
    author_flag: Option<&str>,
) -> Option<String> {
    if let Some(author) =
        author_flag.map(str::trim).filter(|value| !value.is_empty())
    {
        return Some(author.to_string());
    }
    let mut seen = BTreeSet::new();
    let mut distinct = Vec::new();
    for author in part_authors.iter().flatten() {
        let author = author.trim();
        if author.is_empty() {
            continue;
        }
        if seen.insert(author.to_lowercase()) {
            distinct.push(author.to_string());
        }
    }
    match distinct.len() {
        0 => None,
        1 => distinct.into_iter().next(),
        _ => Some(author_display(&distinct)),
    }
}

/// Choose a published-date override or the earliest valid part date.
pub(super) fn collection_published(
    part_dates: &[Option<String>],
    published_flag: Option<&str>,
) -> Option<String> {
    if let Some(date) = published_flag
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        return Some(date.to_string());
    }
    part_dates
        .iter()
        .flatten()
        .filter_map(|date| {
            let trimmed = date.trim();
            NaiveDate::parse_from_str(trimmed, "%Y-%m-%d")
                .ok()
                .map(|parsed| (parsed, trimmed.to_string()))
        })
        .min_by_key(|(date, _)| *date)
        .map(|(_, date)| date)
}

/// Shared author formatter used by collections and arXiv metadata.
pub(super) fn author_display(authors: &[String]) -> String {
    match authors {
        [] => String::new(),
        [only] => only.clone(),
        [first, second] => format!("{first} and {second}"),
        [first, second, third] => format!("{first}, {second}, and {third}"),
        [first, ..] => format!("{first} et al."),
    }
}

fn unique_sites(urls: &[WebUrl]) -> Vec<String> {
    let mut sites = Vec::new();
    for web_url in urls {
        let lower = web_url.host.to_lowercase();
        let site = lower.strip_prefix("www.").unwrap_or(&lower).to_string();
        if !sites.contains(&site) {
            sites.push(site);
        }
    }
    sites
}

fn same_path(urls: &[url::Url]) -> bool {
    urls.first().is_some_and(|first| {
        let first_path = first.path().trim_end_matches('/');
        urls.iter()
            .all(|url| url.path().trim_end_matches('/') == first_path)
    })
}

/// Return true when some query key changes and at least one changed key is
/// outside the page/part family. Identical query strings do not trigger this
/// guard; normal collection validation handles duplicate targets separately.
fn query_changes_are_not_numbered(urls: &[url::Url]) -> bool {
    let Some(first) = urls.first() else {
        return false;
    };
    let first_query = query_map(first);
    if urls.iter().skip(1).all(|url| query_map(url) == first_query) {
        return false;
    }
    let names: BTreeSet<String> = urls
        .iter()
        .flat_map(query_map)
        .map(|(name, _)| name)
        .collect();
    names.into_iter().any(|name| {
        let values: Vec<Vec<String>> = urls
            .iter()
            .map(|url| {
                let mut values: Vec<String> = url
                    .query_pairs()
                    .filter(|(key, _)| key.eq_ignore_ascii_case(&name))
                    .map(|(_, value)| value.into_owned())
                    .collect();
                values.sort();
                values
            })
            .collect();
        values.windows(2).any(|pair| pair[0] != pair[1])
            && !NUMBERED_QUERY_PARAMS.contains(&name.to_lowercase().as_str())
    })
}

fn query_map(url: &url::Url) -> Vec<(String, String)> {
    let mut pairs: Vec<(String, String)> = url
        .query_pairs()
        .map(|(name, value)| (name.to_lowercase(), value.into_owned()))
        .collect();
    pairs.sort();
    pairs
}

fn slug_words(url: &url::Url) -> Vec<String> {
    let Some(segments) = url.path_segments() else {
        return Vec::new();
    };
    let segments: Vec<String> = segments.map(percent_decode).collect();
    for segment in segments.iter().rev() {
        if segment.starts_with('@') || segment.starts_with('~') {
            continue;
        }
        if let Some(cleaned) = clean_segment(segment) {
            return split_words(&cleaned);
        }
    }
    Vec::new()
}

fn shared_words(slugs: &[Vec<String>]) -> SharedWords {
    let Some(first) = slugs.first() else {
        return SharedWords {
            words: Vec::new(),
            start: 0,
        };
    };
    let prefix_len = (0..first.len())
        .take_while(|index| {
            slugs
                .iter()
                .all(|slug| slug.get(*index) == first.get(*index))
        })
        .count();
    let mut start = 0;
    let mut end = prefix_len;
    while start < end && is_trim_word(&first[start]) {
        start += 1;
    }
    while start < end && is_trim_word(&first[end - 1]) {
        end -= 1;
    }
    SharedWords {
        words: first[start..end].to_vec(),
        start,
    }
}

fn shared_folder_words(urls: &[url::Url]) -> Option<Vec<String>> {
    let paths: Vec<Vec<String>> = urls
        .iter()
        .map(|url| {
            url.path_segments().map(|segments| {
                segments
                    .map(percent_decode)
                    .filter(|segment| !segment.is_empty())
                    .collect()
            })
        })
        .collect::<Option<_>>()?;
    let first = paths.first()?;
    if first.len() < 2
        || paths.iter().any(|segments| {
            segments.len() < 2
                || segments[..segments.len() - 1] != first[..first.len() - 1]
        })
    {
        return None;
    }
    let folder = first.get(first.len() - 2)?;
    if folder.starts_with('@') || folder.starts_with('~') {
        return None;
    }
    let cleaned = clean_segment(folder)?;
    Some(trim_words(&split_words(&cleaned)))
}

fn trim_words(words: &[String]) -> Vec<String> {
    let mut start = 0;
    let mut end = words.len();
    while start < end && is_trim_word(&words[start]) {
        start += 1;
    }
    while start < end && is_trim_word(&words[end - 1]) {
        end -= 1;
    }
    words[start..end].to_vec()
}

fn is_trim_word(word: &str) -> bool {
    FILLER_WORDS.contains(&word)
        || SERIES_WORDS.contains(&word)
        || is_number_word(word)
        || is_hex_id_word(word)
}

fn is_number_word(word: &str) -> bool {
    !word.is_empty() && word.bytes().all(|byte| byte.is_ascii_digit())
}

pub(super) fn is_hex_id_word(word: &str) -> bool {
    word.len() >= 6
        && word.bytes().any(|byte| byte.is_ascii_digit())
        && word
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn split_words(text: &str) -> Vec<String> {
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

fn is_useful_name(words: &[String], allow_one_word_series: bool) -> bool {
    let meaningful: Vec<&str> = words
        .iter()
        .map(String::as_str)
        .filter(|word| !is_trim_word(word))
        .collect();
    (meaningful.len() >= 2
        && meaningful.iter().any(|word| !GENERIC_WORDS.contains(word)))
        || (allow_one_word_series
            && meaningful.len() == 1
            && meaningful[0]
                .bytes()
                .filter(u8::is_ascii_alphabetic)
                .count()
                >= 4
            && !GENERIC_WORDS.contains(&meaningful[0]))
}

fn checked_derived_name(words: &[String]) -> Option<String> {
    let name = snake_case(&words.join(" "));
    if name.is_empty() {
        return None;
    }
    validate_name(&name).ok()
}

fn allows_one_word_series(urls: &[WebUrl], parsed: &[url::Url]) -> bool {
    let Some(first) = urls.first() else {
        return false;
    };
    let first = replace_digit_runs(&first.dedupe_key);
    let all_share_numberless_key = urls
        .iter()
        .all(|url| replace_digit_runs(&url.dedupe_key) == first);
    let keys_differ =
        urls.iter().any(|url| url.dedupe_key != urls[0].dedupe_key);
    all_share_numberless_key && keys_differ && parsed.len() == urls.len()
}

fn replace_digit_runs(value: &str) -> String {
    let mut output = String::with_capacity(value.len());
    let mut in_digits = false;
    for byte in value.bytes() {
        if byte.is_ascii_digit() {
            if !in_digits {
                output.push('#');
            }
            in_digits = true;
        } else {
            output.push(byte as char);
            in_digits = false;
        }
    }
    output
}

fn numbered_query_value(url: &url::Url) -> Option<u64> {
    url.query_pairs().find_map(|(name, value)| {
        NUMBERED_QUERY_PARAMS
            .contains(&name.to_lowercase().as_str())
            .then(|| value.parse::<u64>().ok())
            .flatten()
    })
}

fn without_scheme(value: &str) -> String {
    url::Url::parse(value)
        .map(|url| {
            let value = url.as_str();
            value
                .split_once("://")
                .map(|(_, rest)| rest.to_string())
                .unwrap_or_else(|| value.to_string())
        })
        .unwrap_or_else(|_| value.to_string())
}

fn shared_title_prefix(titles: &[String]) -> String {
    let Some(first) = titles.first().map(String::as_str) else {
        return String::new();
    };
    if titles.iter().any(|title| title.trim().is_empty()) {
        return String::new();
    }
    let mut common_len = 0;
    let chars = first.char_indices().peekable();
    for (byte_index, first_char) in chars {
        let char_len = first_char.len_utf8();
        let offset = byte_index + char_len;
        if titles.iter().skip(1).all(|title| {
            title.get(byte_index..offset).is_some_and(|candidate| {
                candidate.eq_ignore_ascii_case(&first[byte_index..offset])
            })
        }) {
            common_len = offset;
        } else {
            break;
        }
    }
    let common = &first[..common_len];
    if common.is_empty() {
        return String::new();
    }

    let shared_chars = common.chars().count();
    let ends_at_boundary = titles.iter().all(|title| {
        title
            .chars()
            .nth(shared_chars)
            .is_none_or(is_title_boundary)
    });
    if ends_at_boundary {
        return common.trim_end().to_string();
    }

    let mut last_boundary = None;
    for (index, character) in common.char_indices() {
        if is_title_boundary(character) {
            last_boundary = Some(index);
        }
    }
    last_boundary
        .map(|index| common[..index].trim_end().to_string())
        .unwrap_or_default()
}

fn is_title_boundary(character: char) -> bool {
    character.is_whitespace()
        || matches!(character, '—' | '–' | ':' | '|' | '(' | ',')
}

fn trim_title_ordinals(value: &str) -> String {
    let mut end = value.len();
    loop {
        while end > 0 {
            let (index, character) = value[..end]
                .char_indices()
                .next_back()
                .expect("non-empty title prefix");
            if character.is_whitespace()
                || matches!(character, '—' | '–' | ':' | '|' | '(' | ',' | '#')
            {
                end = index;
            } else {
                break;
            }
        }
        if end == 0 {
            break;
        }
        let token_start = value[..end]
            .char_indices()
            .rev()
            .find(|(_, character)| !character.is_alphanumeric())
            .map_or(0, |(index, character)| index + character.len_utf8());
        let token = value[token_start..end].to_lowercase();
        if is_title_ordinal(&token) {
            end = token_start;
            continue;
        }
        break;
    }
    value[..end].trim_end().to_string()
}

fn is_title_ordinal(token: &str) -> bool {
    token.starts_with('#') && token[1..].parse::<u64>().is_ok()
        || token.parse::<u64>().is_ok()
        || TITLE_ORDINAL_WORDS.contains(&token)
        || ROMAN_NUMERALS.contains(&token)
}

fn meaningful_title_word_count(value: &str) -> usize {
    split_words(value)
        .iter()
        .filter(|word| {
            !is_trim_word(word)
                && !TITLE_ORDINAL_WORDS.contains(&word.as_str())
                && !ROMAN_NUMERALS.contains(&word.as_str())
        })
        .count()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn urls(values: &[&str]) -> Vec<WebUrl> {
        values
            .iter()
            .map(|value| {
                super::super::clip_url::validate_and_clean(value).unwrap()
            })
            .collect()
    }

    fn derived(values: &[&str]) -> (String, NameSource) {
        match derive_collection_name(&urls(values)) {
            NameOutcome::Derived { name, source } => (name, source),
            other => panic!("expected derived name, got {other:?}"),
        }
    }

    fn no_name(values: &[&str]) -> NoNameReason {
        match derive_collection_name(&urls(values)) {
            NameOutcome::NoName(reason) => reason,
            other => panic!("expected no name, got {other:?}"),
        }
    }

    #[test]
    fn required_naming_table() {
        let cases = [
            (
                vec![
                    "https://example.com/2024/01/tokio-internals-part-1",
                    "https://example.com/2024/02/tokio-internals-part-2",
                ],
                Some(("tokio_internals".into(), NameSource::SlugWords)),
            ),
            (
                vec![
                    "https://stripe.dev/blog/minions-stripes-one-shot-end-to-end-coding-agents",
                    "https://stripe.dev/blog/minions-stripes-one-shot-end-to-end-coding-agents-part-2",
                ],
                Some((
                    "minions_stripes_one_shot_end_to_end_coding_agents".into(),
                    NameSource::SlugWords,
                )),
            ),
            (
                vec![
                    "https://medium.com/@alice/building-a-raft-kv-store-part-1-abc123def456",
                    "https://medium.com/@alice/building-a-raft-kv-store-part-2-0fe1d2c3b4a5",
                ],
                Some(("building_a_raft_kv_store".into(), NameSource::SlugWords)),
            ),
            (
                vec![
                    "https://example.com/articles/exploring-gen-ai/01-intro.html",
                    "https://example.com/articles/exploring-gen-ai/02-prompting.html",
                ],
                Some(("exploring_gen_ai".into(), NameSource::Folder)),
            ),
            (
                vec![
                    "https://example.com/raft-part-1",
                    "https://example.com/raft-part-2",
                ],
                Some(("raft".into(), NameSource::SlugWords)),
            ),
            (
                vec![
                    "https://example.com/essay?page=1",
                    "https://example.com/essay?page=2",
                ],
                Some(("essay".into(), NameSource::SlugWords)),
            ),
        ];
        for (inputs, expected) in cases {
            let actual = derived(&inputs);
            assert_eq!(actual, expected.unwrap());
        }

        assert_eq!(
            no_name(&[
                "https://news.ycombinator.com/item?id=1",
                "https://news.ycombinator.com/item?id=2",
            ]),
            NoNameReason::NoSlug(1)
        );
        assert_eq!(
            no_name(&[
                "https://engineering.atspotify.com/2025/11/spotifys-background-coding-agent-part-1",
                "https://engineering.atspotify.com/2025/11/context-engineering-background-coding-agents-part-2",
                "https://engineering.atspotify.com/2025/12/feedback-loops-background-coding-agents-part-3",
            ]),
            NoNameReason::NothingShared
        );
        assert_eq!(
            no_name(&[
                "https://example.com/core/configuration",
                "https://example.com/core/usage",
            ]),
            NoNameReason::TooThin("core".into())
        );
        assert_eq!(
            no_name(&[
                "https://example.com/getting-started-1",
                "https://example.com/getting-started-2",
            ]),
            NoNameReason::TooThin("getting started".into())
        );
        assert_eq!(
            no_name(&[
                "https://example.com/introduction-to-x",
                "https://example.com/introduction-to-y",
            ]),
            NoNameReason::TooThin("introduction".into())
        );
        assert_eq!(
            no_name(&[
                "https://a.example.com/x-y-z",
                "https://b.example.org/x-y-z",
            ]),
            NoNameReason::DifferentSites(vec![
                "a.example.com".into(),
                "b.example.org".into()
            ])
        );
        assert_eq!(
            derived(&[
                "https://example.com/tokio-internals-part-2",
                "https://example.com/tokio-internals-part-1",
            ]),
            ("tokio_internals".into(), NameSource::SlugWords)
        );
    }

    #[test]
    fn naming_is_order_independent_and_normalizes_site_spelling() {
        let first = derive_collection_name(&urls(&[
            "https://example.com/2024/01/tokio-internals-part-1",
            "https://example.com/2024/02/tokio-internals-part-2",
        ]));
        let permuted = derive_collection_name(&urls(&[
            "https://EXAMPLE.com/2024/02/tokio-internals-part-2/",
            "https://www.example.com/2024/01/tokio-internals-part-1/",
        ]));
        assert_eq!(first, permuted);

        let query_only = derive_collection_name(&urls(&[
            "https://news.ycombinator.com/item?id=1",
            "https://news.ycombinator.com/item?id=2",
        ]));
        let query_only_spelled = derive_collection_name(&urls(&[
            "https://WWW.NEWS.YCOMBINATOR.COM/ITEM/?id=1",
            "https://news.ycombinator.com/ITEM?id=2",
        ]));
        assert_eq!(query_only, query_only_spelled);
        assert_eq!(query_only, NameOutcome::NoName(NoNameReason::NoSlug(1)));

        let folder = derive_collection_name(&urls(&[
            "https://example.com/articles/exploring-gen-ai/01-intro.html",
            "https://example.com/articles/exploring-gen-ai/02-prompting.html",
        ]));
        let folder_spelled = derive_collection_name(&urls(&[
            "https://WWW.EXAMPLE.COM/articles/exploring-gen-ai/01-intro.html/",
            "https://example.com/articles/exploring-gen-ai/02-prompting.html",
        ]));
        assert_eq!(folder, folder_spelled);
        assert_eq!(
            folder,
            NameOutcome::Derived {
                name: "exploring_gen_ai".into(),
                source: NameSource::Folder,
            }
        );

        let one_word = derive_collection_name(&urls(&[
            "https://example.com/raft-part-1",
            "https://example.com/raft-part-2",
        ]));
        let one_word_spelled = derive_collection_name(&urls(&[
            "https://WWW.EXAMPLE.COM/raft-part-1/",
            "https://example.com/raft-part-2",
        ]));
        assert_eq!(one_word, one_word_spelled);
        assert_eq!(
            one_word,
            NameOutcome::Derived {
                name: "raft".into(),
                source: NameSource::SlugWords,
            }
        );
    }

    #[test]
    fn one_word_gate_counts_letters_and_part_order_sees_uncapped_slug_words() {
        assert_eq!(
            no_name(&[
                "https://example.com/gpt4-part-1",
                "https://example.com/gpt4-part-2",
            ]),
            NoNameReason::TooThin("gpt4".into())
        );
        assert_eq!(
            derived(&[
                "https://example.com/raft-part-1",
                "https://example.com/raft-part-2",
            ]),
            ("raft".into(), NameSource::SlugWords)
        );

        let long_prefix = "distributed-systems-architecture-for-highly-available-services-with-resilient-consensus-and-durable-storage-patterns";
        assert!(long_prefix.len() > 80);
        let reversed = [
            format!("https://example.com/{long_prefix}-part-2"),
            format!("https://example.com/{long_prefix}-part-1"),
        ];
        let urls = urls(&[&reversed[0], &reversed[1]]);
        assert_eq!(
            part_order_warning(&urls).as_deref(),
            Some("warning: the URLs are in part order 2, 1; the PDF follows argument order")
        );
    }

    #[test]
    fn no_name_hints_and_scheme_less_rows_are_exact() {
        let cases = [
            (
                vec!["https://a.example.com/x", "https://b.example.org/y"],
                NoNameReason::DifferentSites(vec![
                    "a.example.com".into(),
                    "b.example.org".into(),
                ]),
                "they come from different sites (a.example.com, b.example.org)",
            ),
            (
                vec![
                    "https://news.ycombinator.com/item?id=1",
                    "https://news.ycombinator.com/item?id=2",
                ],
                NoNameReason::NoSlug(1),
                "URL 1 has no slug to name from",
            ),
            (
                vec![
                    "https://example.com/core/configuration",
                    "https://example.com/core/usage",
                ],
                NoNameReason::TooThin("core".into()),
                "they only share \"core\"",
            ),
            (
                vec!["https://example.com/a", "https://example.org/b"],
                NoNameReason::NothingShared,
                "their slugs share no common start",
            ),
        ];
        for (inputs, reason, expected_hint) in cases {
            let urls = urls(&inputs);
            let rendered = name_error_message(&urls, &reason);
            assert!(rendered.ends_with(&format!(
                "hint: {expected_hint}; pass -N <name> (or -T <title>)"
            )));
        }
        let urls = urls(&[
            "https://example.com/core/configuration",
            "https://example.com/core/usage",
        ]);
        assert_eq!(
            name_error_message(&urls, &NoNameReason::TooThin("core".into())),
            "bob ref create: error: these 2 URLs need a reference name\n  1  example.com/core/configuration\n  2  example.com/core/usage\nhint: they only share \"core\"; pass -N <name> (or -T <title>)"
        );
    }

    #[test]
    fn waterfall_sources_and_report_phrases_are_stable() {
        let source_phrases = [
            (NameSource::NameFlag, "from -N"),
            (NameSource::OutputFile, "from the -o filename"),
            (
                NameSource::SlugWords,
                "words every URL starts with; -N overrides",
            ),
            (
                NameSource::Folder,
                "the folder every URL shares; -N overrides",
            ),
            (NameSource::TitleFlag, "from -T; -N overrides"),
        ];
        for (source, expected) in source_phrases {
            assert_eq!(source.report_phrase(), expected);
        }

        let collection_urls = urls(&[
            "https://example.com/2024/01/tokio-internals-part-1",
            "https://example.com/2024/02/tokio-internals-part-2",
        ]);
        assert_eq!(
            resolve_collection_name(
                Some("chosen.pdf"),
                Some("output"),
                Some("Title"),
                &collection_urls
            )
            .unwrap(),
            ResolvedName {
                name: "chosen".into(),
                source: NameSource::NameFlag,
            }
        );
        assert_eq!(
            resolve_collection_name(
                None,
                Some("output.pdf"),
                Some("Title"),
                &collection_urls
            )
            .unwrap(),
            ResolvedName {
                name: "output".into(),
                source: NameSource::OutputFile,
            }
        );
        assert_eq!(
            resolve_collection_name(
                None,
                None,
                Some("Title Name"),
                &collection_urls,
            )
            .unwrap(),
            ResolvedName {
                name: "tokio_internals".into(),
                source: NameSource::SlugWords,
            }
        );
        let folder_urls = urls(&[
            "https://example.com/articles/exploring-gen-ai/01-intro.html",
            "https://example.com/articles/exploring-gen-ai/02-prompting.html",
        ]);
        assert_eq!(
            resolve_collection_name(None, None, None, &folder_urls).unwrap(),
            ResolvedName {
                name: "exploring_gen_ai".into(),
                source: NameSource::Folder,
            }
        );
        let no_derived = urls(&[
            "https://example.com/intro-one",
            "https://example.com/intro-two",
        ]);
        assert_eq!(
            resolve_collection_name(
                None,
                None,
                Some("Fallback Name"),
                &no_derived
            )
            .unwrap(),
            ResolvedName {
                name: "fallback_name".into(),
                source: NameSource::TitleFlag,
            }
        );
        let error =
            resolve_collection_name(None, None, None, &no_derived).unwrap_err();
        assert_eq!(error.exit_code, Some(2));
    }

    #[test]
    fn part_order_warning_only_appears_for_a_decreasing_sequence() {
        let descending = urls(&[
            "https://example.com/tokio-internals-part-2",
            "https://example.com/tokio-internals-part-1",
        ]);
        assert_eq!(
            part_order_warning(&descending).as_deref(),
            Some("warning: the URLs are in part order 2, 1; the PDF follows argument order")
        );
        let ascending = urls(&[
            "https://example.com/tokio-internals-part-1",
            "https://example.com/tokio-internals-part-2",
        ]);
        assert_eq!(part_order_warning(&ascending), None);
        let query_descending = urls(&[
            "https://example.com/essay?page=2",
            "https://example.com/essay?page=1",
        ]);
        assert_eq!(
            part_order_warning(&query_descending).as_deref(),
            Some("warning: the URLs are in part order 2, 1; the PDF follows argument order")
        );
    }

    #[test]
    fn collection_title_uses_shared_title_then_fallback_and_override() {
        let stripe = vec![
            "Minions: Stripe’s one-shot, end-to-end coding agents".into(),
            "Minions: Stripe’s one-shot, end-to-end coding agents—Part 2"
                .into(),
        ];
        assert_eq!(
            collection_title(&stripe, None, "fallback_name"),
            "Minions: Stripe’s one-shot, end-to-end coding agents"
        );
        let tokio = vec![
            "Tokio Internals, Part 1: The Scheduler".into(),
            "Tokio Internals, Part 2: The Reactor".into(),
        ];
        assert_eq!(
            collection_title(&tokio, None, "fallback_name"),
            "Tokio Internals"
        );
        let no_shared =
            vec!["First Article".into(), "Completely Different".into()];
        assert_eq!(
            collection_title(&no_shared, None, "fallback_name"),
            "fallback name"
        );
        let roman = vec![
            "Tokio Internals Chapter II: Part 1".into(),
            "Tokio Internals Chapter II: Part 2".into(),
        ];
        assert_eq!(
            collection_title(&roman, None, "fallback_name"),
            "Tokio Internals"
        );
        assert_eq!(
            collection_title(
                &tokio,
                Some("  Explicit Title  "),
                "fallback_name"
            ),
            "Explicit Title"
        );
    }

    #[test]
    fn collection_authors_are_deduplicated_in_part_order() {
        assert_eq!(
            collection_author(
                &[Some(" Ada Lovelace ".into()), Some("ada lovelace".into())],
                None
            ),
            Some("Ada Lovelace".into())
        );
        assert_eq!(
            collection_author(&[Some("Ada".into()), Some("Alan".into())], None),
            Some("Ada and Alan".into())
        );
        assert_eq!(
            collection_author(
                &[
                    Some("Ada".into()),
                    Some("Alan".into()),
                    Some("Grace".into())
                ],
                None
            ),
            Some("Ada, Alan, and Grace".into())
        );
        assert_eq!(
            collection_author(
                &[
                    Some("Ada".into()),
                    Some("Alan".into()),
                    Some("Grace".into()),
                    Some("Edsger".into()),
                ],
                None
            ),
            Some("Ada et al.".into())
        );
        assert_eq!(collection_author(&[None, Some("  ".into())], None), None);
        assert_eq!(
            collection_author(
                &[Some("Ada".into()), Some("Alan".into())],
                Some("Override")
            ),
            Some("Override".into())
        );
    }

    #[test]
    fn collection_published_uses_override_or_earliest_date() {
        let dates = vec![
            Some("2024-04-03".into()),
            Some("2024-01-08".into()),
            Some("2024-03-17".into()),
        ];
        assert_eq!(
            collection_published(&dates, None).as_deref(),
            Some("2024-01-08")
        );
        assert_eq!(
            collection_published(&dates, Some("2025-12-30")).as_deref(),
            Some("2025-12-30")
        );
        assert_eq!(collection_published(&[None, None], None), None);
    }

    #[test]
    fn query_only_slug_needs_page_like_variation() {
        assert_eq!(
            derive_collection_name(&urls(&[
                "https://example.com/essay?page=1",
                "https://example.com/essay?page=2",
            ])),
            NameOutcome::Derived {
                name: "essay".into(),
                source: NameSource::SlugWords,
            }
        );
        assert_eq!(
            derive_collection_name(&urls(&[
                "https://example.com/item?id=1",
                "https://example.com/item?id=2",
            ])),
            NameOutcome::NoName(NoNameReason::NoSlug(1))
        );
        assert_eq!(
            derive_collection_name(&urls(&[
                "https://example.com/essay?page=1",
                "https://example.com/essay?page=1",
            ])),
            NameOutcome::NoName(NoNameReason::TooThin("essay".into()))
        );
    }
}
