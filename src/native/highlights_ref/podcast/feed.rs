//! Publisher feed discovery and the proven item match.
//!
//! [`find_feed_item`] looks for the show's own public RSS feed and the one
//! item in it that provably is the Spotify episode:
//!
//! 1. **Finding the feed.** iTunes Search for the show name. A result is a
//!    candidate only when its normalized `collectionName` equals the
//!    normalized show name. Candidates are ranked by how well `artistName`
//!    names the publisher, and up to three `feedUrl`s are tried in order:
//!    a feed that answers 404 (or cannot be read) moves on to the next.
//! 2. **Parsing the feed.** A 25 MiB, 30 s capped fetch, parsed with
//!    `roxmltree`. Its default refusal of DTDs means no entity is ever
//!    expanded.
//! 3. **Matching an item.** Each item scores up to three signals (title,
//!    date, duration). Only an item that is alone in scoring at least two
//!    is a match; a missing field just doesn't score.
//!
//! The lookup fails open: every miss is an outcome with a reason, never an
//! error, so the dossier keeps Spotify's show notes. Every request shares
//! one deadline, and `highlights.podcasts.enrich: off` makes no request at
//! all. [`fetch_feed_chapters`] reads a matched item's Podcasting 2.0
//! chapters file under the same deadline.

use std::{fs, path::Path, sync::OnceLock, time::Instant};

use chrono::{DateTime, Duration, Utc};
use regex::Regex;
use roxmltree::Node;
use serde_json::Value;

use super::super::clip_url::validate_and_clean;
use super::super::fetch::{deadline_spent, fetch_url_before, FetchResult};
use super::metadata::EpisodeMetadata;
use super::notes::Chapter;
use super::{clean_line, strip_invisibles};
use crate::native::config::PodcastEnrich;

/// The iTunes Search endpoint.
const ITUNES_SEARCH_URL: &str = "https://itunes.apple.com/search";

/// Size cap for the iTunes Search response.
const ITUNES_MAX_BYTES: u64 = 2 * 1024 * 1024;

/// Size cap for a podcast feed.
pub(crate) const FEED_MAX_BYTES: u64 = 25 * 1024 * 1024;

/// Size cap for a Podcasting 2.0 chapters file.
const CHAPTERS_MAX_BYTES: u64 = 2 * 1024 * 1024;

/// Size cap for a publisher transcript file.
const TRANSCRIPT_MAX_BYTES: u64 = 10 * 1024 * 1024;

/// Per-request time cap; the shared deadline may cut it shorter.
const FETCH_MAX_TIME_SECS: u64 = 30;

/// At most this many candidate feeds are fetched.
const MAX_FEED_CANDIDATES: usize = 3;

/// A feed `pubDate` this close to Spotify's release scores the date.
const DATE_WINDOW_HOURS: i64 = 36;

/// An `itunes:duration` within max(5 s, 2 %) of Spotify's scores the
/// duration.
const DURATION_TOLERANCE_MIN_MS: u64 = 5_000;

const ITUNES_NS: &str = "http://www.itunes.com/dtds/podcast-1.0.dtd";
const CONTENT_NS: &str = "http://purl.org/rss/1.0/modules/content/";

/// The Podcasting 2.0 namespace, as feeds spell it.
const PODCAST_NAMESPACES: [&str; 3] = [
    "https://podcastindex.org/namespace/1.0",
    "http://podcastindex.org/namespace/1.0",
    "https://github.com/Podcastindex-org/podcast-namespace/blob/main/docs/1.0.md",
];

/// One signal a feed item agreed on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) enum MatchSignal {
    /// The normalized titles are equal.
    Title,
    /// The `pubDate` is within 36 h of Spotify's release.
    Date,
    /// The `itunes:duration` is within max(5 s, 2 %) of Spotify's.
    Duration,
}

impl MatchSignal {
    /// `title`, `date`, or `duration`.
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Title => "title",
            Self::Date => "date",
            Self::Duration => "duration",
        }
    }
}

/// A `podcast:transcript` the feed offers for the item.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FeedTranscript {
    /// Absolute http(s) URL.
    pub(crate) url: String,
    /// The declared MIME type, trimmed and lowercased (empty when the
    /// feed omits it).
    pub(crate) mime: String,
    pub(crate) language: Option<String>,
    /// `captions` for timed captions; usually absent.
    pub(crate) rel: Option<String>,
}

/// The item's `podcast:chapters` file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FeedChaptersLink {
    /// Absolute http(s) URL.
    pub(crate) url: String,
    /// The declared MIME type, trimmed and lowercased (empty when the
    /// feed omits it).
    pub(crate) mime: String,
}

/// The feed item that proved it is the Spotify episode.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FeedItem {
    /// The feed URL the item came from (after redirects).
    pub(crate) feed_url: String,
    /// That URL's host, for provenance.
    pub(crate) feed_host: String,
    pub(crate) guid: Option<String>,
    pub(crate) title: String,
    pub(crate) pub_date: Option<DateTime<Utc>>,
    /// `itunes:duration`, rounded to whole seconds.
    pub(crate) duration_secs: Option<u64>,
    /// `content:encoded`, else `description`: invisibles stripped,
    /// otherwise raw (the renderer sanitizes).
    pub(crate) notes_html: Option<String>,
    pub(crate) transcripts: Vec<FeedTranscript>,
    pub(crate) chapters: Option<FeedChaptersLink>,
    /// The signals the item agreed on, in [`MatchSignal`] order.
    pub(crate) matched_on: Vec<MatchSignal>,
}

impl FeedItem {
    /// The agreed signals as a report reads them: `title, date`.
    pub(crate) fn matched_on_label(&self) -> String {
        self.matched_on
            .iter()
            .map(|signal| signal.label())
            .collect::<Vec<_>>()
            .join(", ")
    }
}

/// What the publisher feed lookup found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum FeedOutcome {
    /// Exactly one feed item agreed on at least two signals.
    Matched(FeedItem),
    /// No usable feed: no iTunes candidate, or no candidate feed could be
    /// fetched and parsed.
    NoFeed(String),
    /// A feed was read, but no single item proved the match (a tie reads
    /// `ambiguous: …`).
    NoMatch(String),
    /// The shared deadline ran out before the lookup finished.
    DeadlineExceeded,
    /// `highlights.podcasts.enrich: off`: nothing was requested.
    Off,
}

impl FeedOutcome {
    /// Why there is no match, for [`Self::NoFeed`] and
    /// [`Self::NoMatch`].
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn reason(&self) -> Option<&str> {
        match self {
            Self::NoFeed(reason) | Self::NoMatch(reason) => Some(reason),
            Self::Matched(_) | Self::DeadlineExceeded | Self::Off => None,
        }
    }
}

/// What [`fetch_feed_chapters`] produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ChaptersOutcome {
    /// Valid chapters with strictly increasing start times.
    Chapters(Vec<Chapter>),
    /// The file was missing or invalid, and why.
    Ignored(String),
    /// The shared deadline ran out first.
    DeadlineExceeded,
}

/// What [`fetch_feed_transcript`] produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum TranscriptFileOutcome {
    /// The file's bytes.
    Bytes(Vec<u8>),
    /// The file was missing or unreadable, and why.
    Ignored(String),
    /// The shared deadline ran out first.
    DeadlineExceeded,
}

/// Why a step stopped the lookup or skipped a candidate.
enum Stop {
    Deadline,
    Reason(String),
}

/// Reclassify a failure as the deadline when the budget is spent.
fn stop_for(deadline: Instant, reason: String) -> Stop {
    if deadline_spent(deadline) {
        Stop::Deadline
    } else {
        Stop::Reason(reason)
    }
}

/// Find the feed item that is `meta`'s episode, downloading into
/// `scratch` and stopping at `deadline`. Never fails: every miss is an
/// outcome with a reason.
pub(crate) fn find_feed_item(
    meta: &EpisodeMetadata,
    enrich: PodcastEnrich,
    scratch: &Path,
    deadline: Instant,
    progress: Option<&dyn Fn(&str)>,
) -> FeedOutcome {
    if enrich == PodcastEnrich::Off {
        return FeedOutcome::Off;
    }
    let Some(show) = meta.show.as_deref().and_then(clean_line) else {
        return FeedOutcome::NoFeed(
            "Spotify named no show for this episode".to_string(),
        );
    };
    if deadline_spent(deadline) {
        return FeedOutcome::DeadlineExceeded;
    }
    let results = match search_itunes(&show, scratch, deadline, progress) {
        Ok(results) => results,
        Err(Stop::Deadline) => return FeedOutcome::DeadlineExceeded,
        Err(Stop::Reason(reason)) => return FeedOutcome::NoFeed(reason),
    };
    let candidates = rank_candidates(results, &show, meta.publisher.as_deref());
    if candidates.is_empty() {
        return FeedOutcome::NoFeed(format!(
            "iTunes Search lists no podcast named \"{show}\" with a feed"
        ));
    }

    let mut failures = Vec::new();
    let mut no_match: Option<String> = None;
    for (index, candidate) in
        candidates.iter().take(MAX_FEED_CANDIDATES).enumerate()
    {
        let dest = scratch.join(format!("podcast-feed-{index}.xml"));
        let feed =
            match fetch_feed(&candidate.feed_url, &dest, deadline, progress) {
                Ok(feed) => feed,
                Err(Stop::Deadline) => return FeedOutcome::DeadlineExceeded,
                Err(Stop::Reason(reason)) => {
                    failures.push(reason);
                    continue;
                }
            };
        match match_items(meta, &feed.items) {
            ItemMatch::Unique { index, signals } => {
                let ParsedFeed {
                    url,
                    host,
                    mut items,
                } = feed;
                return FeedOutcome::Matched(
                    items
                        .swap_remove(index)
                        .into_feed_item(&url, &host, signals),
                );
            }
            ItemMatch::Ambiguous(count) => {
                return FeedOutcome::NoMatch(format!(
                    "ambiguous: {count} items in the {} feed agree on two of title, date, and duration",
                    feed.host
                ));
            }
            ItemMatch::Nothing => {
                no_match.get_or_insert_with(|| {
                    format!(
                        "no item in the {} feed agrees on two of title, date, and duration",
                        feed.host
                    )
                });
            }
        }
    }
    match no_match {
        Some(reason) => FeedOutcome::NoMatch(reason),
        None => FeedOutcome::NoFeed(failures.join("; ")),
    }
}

/// Download and parse a matched item's Podcasting 2.0 chapters file.
pub(crate) fn fetch_feed_chapters(
    link: &FeedChaptersLink,
    scratch: &Path,
    deadline: Instant,
    progress: Option<&dyn Fn(&str)>,
) -> ChaptersOutcome {
    let dest = scratch.join("podcast-chapters.json");
    let body = match fetch_body(
        &link.url,
        &dest,
        CHAPTERS_MAX_BYTES,
        deadline,
        progress,
        "chapters file",
    ) {
        Ok((_, body)) => body,
        Err(Stop::Deadline) => return ChaptersOutcome::DeadlineExceeded,
        Err(Stop::Reason(reason)) => return ChaptersOutcome::Ignored(reason),
    };
    match parse_chapters_json(&body) {
        Ok(chapters) => ChaptersOutcome::Chapters(chapters),
        Err(reason) => ChaptersOutcome::Ignored(reason),
    }
}

/// Download a matched item's transcript file under the shared deadline.
pub(crate) fn fetch_feed_transcript(
    offer: &FeedTranscript,
    scratch: &Path,
    deadline: Instant,
    progress: Option<&dyn Fn(&str)>,
) -> TranscriptFileOutcome {
    let dest = scratch.join("podcast-transcript");
    match fetch_body(
        &offer.url,
        &dest,
        TRANSCRIPT_MAX_BYTES,
        deadline,
        progress,
        "transcript",
    ) {
        Ok((_, body)) => TranscriptFileOutcome::Bytes(body),
        Err(Stop::Deadline) => TranscriptFileOutcome::DeadlineExceeded,
        Err(Stop::Reason(reason)) => TranscriptFileOutcome::Ignored(reason),
    }
}

/// Fetch `url` under the deadline and return its 200 body. `what` names
/// the resource in every reason.
fn fetch_body(
    url: &str,
    dest: &Path,
    max_bytes: u64,
    deadline: Instant,
    progress: Option<&dyn Fn(&str)>,
    what: &str,
) -> Result<(FetchResult, Vec<u8>), Stop> {
    if let Err(error) = validate_and_clean(url) {
        return Err(Stop::Reason(format!("{what} {url}: {}", error.message)));
    }
    if deadline_spent(deadline) {
        return Err(Stop::Deadline);
    }
    let fetched = fetch_url_before(
        url,
        dest,
        FETCH_MAX_TIME_SECS,
        max_bytes,
        deadline,
        progress,
    )
    .map_err(|error| {
        stop_for(deadline, format!("{what} {url}: {}", error.message()))
    })?;
    if fetched.status != 200 {
        return Err(Stop::Reason(format!(
            "{what} {url} answered HTTP {}",
            fetched.status
        )));
    }
    let body = fs::read(&fetched.path).map_err(|error| {
        Stop::Reason(format!("{what} {url}: read download: {error}"))
    })?;
    Ok((fetched, body))
}

/// One iTunes Search result with a feed.
#[derive(Debug, Clone, PartialEq, Eq)]
struct ItunesResult {
    collection_name: String,
    artist_name: Option<String>,
    feed_url: String,
}

/// `https://itunes.apple.com/search?media=podcast&entity=podcast&limit=10&term=<show>`.
fn itunes_search_url(show: &str) -> String {
    let term: String =
        url::form_urlencoded::byte_serialize(show.as_bytes()).collect();
    format!(
        "{ITUNES_SEARCH_URL}?media=podcast&entity=podcast&limit=10&term={term}"
    )
}

fn search_itunes(
    show: &str,
    scratch: &Path,
    deadline: Instant,
    progress: Option<&dyn Fn(&str)>,
) -> Result<Vec<ItunesResult>, Stop> {
    let url = itunes_search_url(show);
    let (_, body) = fetch_body(
        &url,
        &scratch.join("podcast-itunes-search.json"),
        ITUNES_MAX_BYTES,
        deadline,
        progress,
        "iTunes Search",
    )?;
    parse_itunes_results(&body).ok_or_else(|| {
        Stop::Reason("iTunes Search returned unreadable JSON".to_string())
    })
}

/// Every result that has a collection name and a feed URL. `None` when
/// the body is not an iTunes Search response.
fn parse_itunes_results(body: &[u8]) -> Option<Vec<ItunesResult>> {
    let value: Value = serde_json::from_slice(body).ok()?;
    let results = value.get("results")?.as_array()?;
    Some(
        results
            .iter()
            .filter_map(|result| {
                let text = |key: &str| {
                    result.get(key).and_then(Value::as_str).and_then(clean_line)
                };
                Some(ItunesResult {
                    collection_name: text("collectionName")?,
                    artist_name: text("artistName"),
                    feed_url: text("feedUrl")?,
                })
            })
            .collect(),
    )
}

/// The results whose normalized collection name is the show's, best
/// publisher match first (iTunes order otherwise), one per feed URL.
fn rank_candidates(
    results: Vec<ItunesResult>,
    show: &str,
    publisher: Option<&str>,
) -> Vec<ItunesResult> {
    let wanted = normalize_show(show);
    if wanted.is_empty() {
        return Vec::new();
    }
    let mut scored: Vec<(u8, ItunesResult)> = results
        .into_iter()
        .filter(|result| normalize_show(&result.collection_name) == wanted)
        .map(|result| {
            (
                artist_score(result.artist_name.as_deref(), publisher),
                result,
            )
        })
        .collect();
    // A stable sort keeps iTunes's own order within a score.
    scored.sort_by_key(|(score, _)| std::cmp::Reverse(*score));
    let mut seen = std::collections::HashSet::new();
    scored
        .into_iter()
        .map(|(_, result)| result)
        .filter(|result| seen.insert(result.feed_url.clone()))
        .collect()
}

/// 2 when the artist is the publisher, 1 when one names the other
/// (whole words), else 0.
fn artist_score(artist: Option<&str>, publisher: Option<&str>) -> u8 {
    let (Some(artist), Some(publisher)) = (artist, publisher) else {
        return 0;
    };
    let artist = normalize_words(artist);
    let publisher = normalize_words(publisher);
    if artist.is_empty() || publisher.is_empty() {
        return 0;
    }
    if artist == publisher {
        return 2;
    }
    let padded = |text: &str| format!(" {text} ");
    if padded(&artist).contains(&padded(&publisher))
        || padded(&publisher).contains(&padded(&artist))
    {
        1
    } else {
        0
    }
}

/// A fetched, parsed feed.
struct ParsedFeed {
    /// The final URL after redirects.
    url: String,
    host: String,
    items: Vec<RawItem>,
}

/// Everything one `<item>` says, before matching.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct RawItem {
    guid: Option<String>,
    title: Option<String>,
    itunes_title: Option<String>,
    pub_date: Option<DateTime<Utc>>,
    duration_secs: Option<u64>,
    notes_html: Option<String>,
    transcripts: Vec<FeedTranscript>,
    chapters: Option<FeedChaptersLink>,
}

impl RawItem {
    fn into_feed_item(
        self,
        feed_url: &str,
        feed_host: &str,
        matched_on: Vec<MatchSignal>,
    ) -> FeedItem {
        FeedItem {
            feed_url: feed_url.to_string(),
            feed_host: feed_host.to_string(),
            guid: self.guid,
            title: self.title.or(self.itunes_title).unwrap_or_default(),
            pub_date: self.pub_date,
            duration_secs: self.duration_secs,
            notes_html: self.notes_html,
            transcripts: self.transcripts,
            chapters: self.chapters,
            matched_on,
        }
    }
}

fn fetch_feed(
    url: &str,
    dest: &Path,
    deadline: Instant,
    progress: Option<&dyn Fn(&str)>,
) -> Result<ParsedFeed, Stop> {
    let (fetched, body) =
        fetch_body(url, dest, FEED_MAX_BYTES, deadline, progress, "feed")?;
    let text = String::from_utf8_lossy(&body);
    let (host, items) = parse_feed(&text, &fetched.final_url)
        .map_err(|reason| Stop::Reason(format!("feed {url}: {reason}")))?;
    Ok(ParsedFeed {
        url: fetched.final_url,
        host,
        items,
    })
}

/// Parse an RSS feed into its host and items. Relative links resolve
/// against `feed_url`. A DTD is refused (roxmltree's default), so no
/// entity is ever expanded.
fn parse_feed(
    xml: &str,
    feed_url: &str,
) -> Result<(String, Vec<RawItem>), String> {
    let xml = xml.strip_prefix('\u{FEFF}').unwrap_or(xml);
    let document =
        roxmltree::Document::parse(xml).map_err(|error| match error {
            roxmltree::Error::DtdDetected => {
                "the feed declares a DTD, which bob refuses".to_string()
            }
            other => format!("malformed XML ({other})"),
        })?;
    let root = document.root_element();
    if !is_plain(&root, "rss") {
        return Err(format!(
            "not an RSS feed (root element <{}>)",
            root.tag_name().name()
        ));
    }
    let channel = root
        .children()
        .find(|node| is_plain(node, "channel"))
        .ok_or_else(|| "the feed has no <channel>".to_string())?;
    let base = url::Url::parse(feed_url).ok();
    let host = base
        .as_ref()
        .and_then(|url| url.host_str())
        .unwrap_or_default()
        .to_string();
    let items = channel
        .children()
        .filter(|node| is_plain(node, "item"))
        .map(|node| parse_item(node, base.as_ref()))
        .collect();
    Ok((host, items))
}

fn parse_item(node: Node<'_, '_>, base: Option<&url::Url>) -> RawItem {
    let mut item = RawItem::default();
    let mut description = None;
    let mut encoded = None;
    for child in node.children().filter(Node::is_element) {
        if is_plain(&child, "title") {
            item.title = item.title.or_else(|| clean_title(&text_of(child)));
        } else if is_plain(&child, "guid") {
            item.guid = item.guid.or_else(|| clean_line(&text_of(child)));
        } else if is_plain(&child, "pubDate") {
            item.pub_date =
                item.pub_date.or_else(|| parse_pub_date(&text_of(child)));
        } else if is_plain(&child, "description") {
            description = description.or_else(|| clean_notes(&text_of(child)));
        } else if is_in(&child, &[CONTENT_NS], "encoded") {
            encoded = encoded.or_else(|| clean_notes(&text_of(child)));
        } else if is_in(&child, &[ITUNES_NS], "title") {
            item.itunes_title =
                item.itunes_title.or_else(|| clean_title(&text_of(child)));
        } else if is_in(&child, &[ITUNES_NS], "duration") {
            item.duration_secs = item
                .duration_secs
                .or_else(|| parse_duration_secs(&text_of(child)));
        } else if is_in(&child, &PODCAST_NAMESPACES, "transcript") {
            if let Some(url) = resolve_link(base, child.attribute("url")) {
                item.transcripts.push(FeedTranscript {
                    url,
                    mime: mime_of(child),
                    language: child.attribute("language").and_then(clean_line),
                    rel: child.attribute("rel").and_then(clean_line),
                });
            }
        } else if is_in(&child, &PODCAST_NAMESPACES, "chapters")
            && item.chapters.is_none()
        {
            item.chapters =
                resolve_link(base, child.attribute("url")).map(|url| {
                    FeedChaptersLink {
                        url,
                        mime: mime_of(child),
                    }
                });
        }
    }
    item.notes_html = encoded.or(description);
    item
}

/// An element in no namespace (plain RSS) named `name`.
fn is_plain(node: &Node<'_, '_>, name: &str) -> bool {
    node.is_element()
        && node.tag_name().namespace().is_none()
        && node.tag_name().name() == name
}

/// An element named `name` in one of `namespaces`.
fn is_in(node: &Node<'_, '_>, namespaces: &[&str], name: &str) -> bool {
    node.is_element()
        && node.tag_name().name() == name
        && node
            .tag_name()
            .namespace()
            .is_some_and(|namespace| namespaces.contains(&namespace))
}

/// Every text and CDATA child, concatenated.
fn text_of(node: Node<'_, '_>) -> String {
    node.children()
        .filter(Node::is_text)
        .filter_map(|child| child.text())
        .collect()
}

fn mime_of(node: Node<'_, '_>) -> String {
    node.attribute("type")
        .map(|mime| mime.trim().to_ascii_lowercase())
        .unwrap_or_default()
}

/// An absolute http(s) URL for `raw`, resolved against the feed URL.
fn resolve_link(base: Option<&url::Url>, raw: Option<&str>) -> Option<String> {
    let raw = raw?.trim();
    if raw.is_empty() {
        return None;
    }
    let url = match base {
        Some(base) => base.join(raw).ok()?,
        None => url::Url::parse(raw).ok()?,
    };
    matches!(url.scheme(), "http" | "https").then(|| url.to_string())
}

/// A title with HTML character references decoded (feeds double-escape
/// them) and whitespace collapsed.
fn clean_title(raw: &str) -> Option<String> {
    clean_line(&decode_char_refs(raw))
}

/// Notes HTML with invisibles stripped and outer whitespace trimmed.
fn clean_notes(raw: &str) -> Option<String> {
    let notes = strip_invisibles(raw).trim().to_string();
    (!notes.is_empty()).then_some(notes)
}

/// Decode `&amp;`, `&quot;`, `&apos;`, `&lt;`, `&gt;`, `&nbsp;`, and
/// numeric character references; anything else stays as written.
fn decode_char_refs(text: &str) -> String {
    static REFS: OnceLock<Regex> = OnceLock::new();
    let refs = REFS.get_or_init(|| {
        Regex::new(r"&(?:#(\d{1,7})|#[xX]([0-9A-Fa-f]{1,6})|(amp|quot|apos|lt|gt|nbsp));")
            .expect("valid character reference regex")
    });
    refs.replace_all(text, |caps: &regex::Captures<'_>| {
        let decoded = if let Some(decimal) = caps.get(1) {
            decimal
                .as_str()
                .parse::<u32>()
                .ok()
                .and_then(char::from_u32)
        } else if let Some(hex) = caps.get(2) {
            u32::from_str_radix(hex.as_str(), 16)
                .ok()
                .and_then(char::from_u32)
        } else {
            caps.get(3).map(|name| match name.as_str() {
                "amp" => '&',
                "quot" => '"',
                "apos" => '\'',
                "lt" => '<',
                "gt" => '>',
                _ => ' ',
            })
        };
        decoded.map_or_else(|| caps[0].to_string(), String::from)
    })
    .into_owned()
}

/// A feed `pubDate`: RFC 2822 (also with a wrong weekday name, which
/// strict parsing refuses), else RFC 3339.
fn parse_pub_date(raw: &str) -> Option<DateTime<Utc>> {
    let raw = raw.trim();
    if raw.is_empty() {
        return None;
    }
    let rfc2822 = |text: &str| {
        DateTime::parse_from_rfc2822(text.trim())
            .ok()
            .map(|date| date.with_timezone(&Utc))
    };
    rfc2822(raw)
        .or_else(|| raw.split_once(',').and_then(|(_, rest)| rfc2822(rest)))
        .or_else(|| {
            DateTime::parse_from_rfc3339(raw)
                .ok()
                .map(|date| date.with_timezone(&Utc))
        })
}

/// An `itunes:duration`: seconds (`5197`, `5197.4`), `MM:SS`, or
/// `HH:MM:SS`, rounded to whole seconds. `None` for anything else or
/// zero.
fn parse_duration_secs(raw: &str) -> Option<u64> {
    let parts: Vec<&str> = raw.trim().split(':').collect();
    if parts.is_empty() || parts.len() > 3 {
        return None;
    }
    let mut total = 0.0_f64;
    for (index, part) in parts.iter().enumerate() {
        let part = part.trim();
        let value = if index + 1 == parts.len() {
            part.parse::<f64>().ok()?
        } else {
            part.parse::<u64>().ok()? as f64
        };
        if !value.is_finite() || value < 0.0 {
            return None;
        }
        total = total * 60.0 + value;
    }
    let secs = total.round();
    (secs >= 1.0 && secs < u64::MAX as f64).then_some(secs as u64)
}

/// How the items scored.
enum ItemMatch {
    /// Exactly one item agreed on at least two signals.
    Unique {
        index: usize,
        signals: Vec<MatchSignal>,
    },
    /// This many items agreed on at least two signals.
    Ambiguous(usize),
    /// No item agreed on two signals.
    Nothing,
}

fn match_items(meta: &EpisodeMetadata, items: &[RawItem]) -> ItemMatch {
    let wanted_title = normalize_title(&meta.title);
    let mut qualified = items
        .iter()
        .enumerate()
        .map(|(index, item)| (index, score_item(meta, &wanted_title, item)))
        .filter(|(_, signals)| signals.len() >= 2)
        .collect::<Vec<_>>();
    match qualified.len() {
        0 => ItemMatch::Nothing,
        1 => {
            let (index, signals) = qualified.remove(0);
            ItemMatch::Unique { index, signals }
        }
        count => ItemMatch::Ambiguous(count),
    }
}

/// The signals `item` agrees on, in [`MatchSignal`] order.
fn score_item(
    meta: &EpisodeMetadata,
    wanted_title: &str,
    item: &RawItem,
) -> Vec<MatchSignal> {
    let mut signals = Vec::new();
    let title_matches = !wanted_title.is_empty()
        && [&item.title, &item.itunes_title]
            .into_iter()
            .flatten()
            .any(|title| normalize_title(title) == wanted_title);
    if title_matches {
        signals.push(MatchSignal::Title);
    }
    let date_matches = meta.released.as_ref().zip(item.pub_date).is_some_and(
        |(released, pub_date)| {
            (pub_date - released.instant).abs()
                <= Duration::hours(DATE_WINDOW_HOURS)
        },
    );
    if date_matches {
        signals.push(MatchSignal::Date);
    }
    let duration_matches = meta
        .duration_ms
        .zip(item.duration_secs)
        .is_some_and(|(spotify_ms, feed_secs)| {
            let tolerance = DURATION_TOLERANCE_MIN_MS.max(spotify_ms / 50);
            spotify_ms.abs_diff(feed_secs.saturating_mul(1000)) <= tolerance
        });
    if duration_matches {
        signals.push(MatchSignal::Duration);
    }
    signals
}

/// Case-folded words: invisibles and apostrophes dropped, `&` read as
/// "and", every other non-alphanumeric character a word break.
fn normalize_words(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in strip_invisibles(text).chars() {
        if matches!(c, '\'' | '\u{2018}' | '\u{2019}' | '\u{02BC}' | '`') {
            continue;
        }
        if c == '&' {
            out.push_str(" and ");
        } else if c.is_alphanumeric() {
            out.extend(c.to_lowercase());
        } else {
            out.push(' ');
        }
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// A show name for comparison: [`normalize_words`] without a leading
/// "the".
fn normalize_show(text: &str) -> String {
    let words = normalize_words(text);
    match words.strip_prefix("the ") {
        Some(rest) => rest.to_string(),
        None => words,
    }
}

/// An episode title for comparison: [`normalize_words`] after dropping a
/// leading episode number (`#123:`, `Ep. 12 -`, `Episode 7:`, `42 |`).
fn normalize_title(text: &str) -> String {
    static NUMBER: OnceLock<Regex> = OnceLock::new();
    let number = NUMBER.get_or_init(|| {
        Regex::new(
            r"(?i)^\s*(?:(?:#\s*\d+|ep(?:isode)?\.?\s*#?\s*\d+)\s*[:.\-–—|]?|\d+\s*[:\-–—|])\s*",
        )
        .expect("valid episode number regex")
    });
    let text = strip_invisibles(text);
    let stripped = number.replace(&text, "");
    if normalize_words(&stripped).is_empty() {
        normalize_words(&text)
    } else {
        normalize_words(&stripped)
    }
}

/// Parse a Podcasting 2.0 chapters file (`chapters[].startTime/title`).
///
/// Chapters marked `"toc": false` and untitled chapters are skipped; the
/// rest must start at non-negative times that strictly increase. Any
/// other shape is refused with the reason.
pub(crate) fn parse_chapters_json(body: &[u8]) -> Result<Vec<Chapter>, String> {
    let value: Value = serde_json::from_slice(body)
        .map_err(|_| "the chapters file is not valid JSON".to_string())?;
    let entries = value
        .get("chapters")
        .and_then(Value::as_array)
        .ok_or_else(|| "the chapters file has no chapters list".to_string())?;
    let mut chapters: Vec<Chapter> = Vec::new();
    for (index, entry) in entries.iter().enumerate() {
        let ordinal = index + 1;
        if entry.get("toc").and_then(Value::as_bool) == Some(false) {
            continue;
        }
        let start = entry
            .get("startTime")
            .and_then(Value::as_f64)
            .filter(|start| {
                start.is_finite() && *start >= 0.0 && *start < u32::MAX as f64
            })
            .ok_or_else(|| {
                format!("chapter {ordinal} has no valid startTime")
            })?;
        let Some(title) = entry
            .get("title")
            .and_then(Value::as_str)
            .and_then(clean_title)
        else {
            continue;
        };
        let start_secs = start.floor() as u32;
        if chapters
            .last()
            .is_some_and(|previous| start_secs <= previous.start_secs)
        {
            return Err(format!(
                "chapter {ordinal} does not start after the one before it"
            ));
        }
        chapters.push(Chapter { start_secs, title });
    }
    if chapters.is_empty() {
        return Err("the chapters file lists no titled chapters".to_string());
    }
    Ok(chapters)
}

#[cfg(test)]
mod tests {
    use super::super::metadata::{ReleasePrecision, Released};
    use super::super::test_support::{url_surface, FakeSpotify};
    use super::*;
    use std::time::Duration as StdDuration;

    const ITUNES: &str = include_str!(
        "../../../../tests/fixtures/podcast/itunes_pragmatic_engineer.json"
    );
    const FEED: &str = include_str!(
        "../../../../tests/fixtures/podcast/feed_pragmatic_engineer.rss"
    );
    const CHAPTERS: &str =
        include_str!("../../../../tests/fixtures/podcast/chapters.json");
    const FEED_URL: &str = "https://api.substack.com/feed/podcast/458709.rss";
    const TITLE: &str = "Software factories and code reviews with Harjot Gill";

    /// The episode as the Spotify metadata ladder reads it.
    fn episode() -> EpisodeMetadata {
        EpisodeMetadata {
            id: "4UgVFWty2I8hGAgSvT7pjC".to_string(),
            canonical_url:
                "https://open.spotify.com/episode/4UgVFWty2I8hGAgSvT7pjC"
                    .to_string(),
            title: TITLE.to_string(),
            show: Some("The Pragmatic Engineer".to_string()),
            show_uri: Some("spotify:show:2Bho9xCbOQMWMJ7UKmqCzD".to_string()),
            publisher: Some("Gergely Orosz".to_string()),
            released: Some(Released {
                instant: DateTime::parse_from_rfc3339("2026-11-18T18:08:00Z")
                    .expect("instant")
                    .with_timezone(&Utc),
                precision: ReleasePrecision::Time,
            }),
            duration_ms: Some(5_197_032),
            language: Some("en".to_string()),
            cover_url: None,
            notes_html: None,
            notes_text: None,
            sources: Default::default(),
            warnings: Vec::new(),
        }
    }

    fn later() -> Instant {
        Instant::now() + StdDuration::from_secs(120)
    }

    fn itunes_with(results: &[(&str, &str, &str)]) -> String {
        let results = results
            .iter()
            .map(|(name, artist, feed)| {
                serde_json::json!({
                    "wrapperType": "track",
                    "kind": "podcast",
                    "artistName": artist,
                    "collectionName": name,
                    "feedUrl": feed,
                })
            })
            .collect::<Vec<_>>();
        serde_json::json!({ "resultCount": results.len(), "results": results })
            .to_string()
    }

    fn items(feed: &str) -> Vec<RawItem> {
        parse_feed(feed, FEED_URL).expect("feed parses").1
    }

    fn lookup(fake: &FakeSpotify, name: &str) -> FeedOutcome {
        find_feed_item(
            &episode(),
            PodcastEnrich::Auto,
            &fake.scratch(name),
            later(),
            None,
        )
    }

    #[test]
    fn the_real_feed_proves_the_episode_on_all_three_signals() {
        let fake = FakeSpotify::new();
        fake.serve("itunes", 200, "text/javascript", ITUNES.as_bytes());
        fake.serve_url(FEED_URL, 200, "application/rss+xml", FEED.as_bytes());
        let FeedOutcome::Matched(item) = lookup(&fake, "real") else {
            panic!("expected a match");
        };
        assert_eq!(item.feed_url, FEED_URL);
        assert_eq!(item.feed_host, "api.substack.com");
        assert_eq!(item.guid.as_deref(), Some("substack:post:225268679"));
        assert_eq!(item.title, TITLE);
        assert_eq!(
            item.pub_date.map(|date| date.to_rfc3339()).as_deref(),
            Some("2026-11-18T18:08:52+00:00")
        );
        assert_eq!(item.duration_secs, Some(5197));
        assert_eq!(
            item.matched_on,
            vec![MatchSignal::Title, MatchSignal::Date, MatchSignal::Duration]
        );
        assert_eq!(item.matched_on_label(), "title, date, duration");
        let notes = item.notes_html.as_deref().expect("notes");
        assert!(notes.starts_with("<p><strong>Brought to You By:</strong></p>"));
        assert!(!notes.contains('\u{2060}'), "invisibles are stripped");
        assert_eq!(
            item.transcripts,
            vec![FeedTranscript {
                url: "https://api.substack.com/feed/podcast/transcript/225268679/5716fdd023036d0a64c51e7e8330539c.vtt".to_string(),
                mime: "text/vtt".to_string(),
                language: None,
                rel: None,
            }]
        );
        assert_eq!(item.chapters, None);
        assert_eq!(
            fake.requests(),
            vec![
                "https://itunes.apple.com/search?media=podcast&entity=podcast&limit=10&term=The+Pragmatic+Engineer".to_string(),
                FEED_URL.to_string(),
            ]
        );
    }

    #[test]
    fn enrich_off_makes_no_request() {
        let fake = FakeSpotify::new();
        fake.serve("itunes", 200, "text/javascript", ITUNES.as_bytes());
        let outcome = find_feed_item(
            &episode(),
            PodcastEnrich::Off,
            &fake.scratch("off"),
            later(),
            None,
        );
        assert_eq!(outcome, FeedOutcome::Off);
        assert!(fake.requests().is_empty(), "{:?}", fake.requests());
    }

    #[test]
    fn a_spent_deadline_stops_before_any_request() {
        let fake = FakeSpotify::new();
        fake.serve("itunes", 200, "text/javascript", ITUNES.as_bytes());
        let outcome = find_feed_item(
            &episode(),
            PodcastEnrich::Auto,
            &fake.scratch("spent"),
            Instant::now(),
            None,
        );
        assert_eq!(outcome, FeedOutcome::DeadlineExceeded);
        assert!(fake.requests().is_empty(), "{:?}", fake.requests());
    }

    #[test]
    fn no_candidate_means_no_feed() {
        let fake = FakeSpotify::new();
        fake.serve(
            "itunes",
            200,
            "text/javascript",
            itunes_with(&[(
                "Pragmatic Engineering Weekly",
                "Someone Else",
                "https://feeds.example.com/other.rss",
            )])
            .as_bytes(),
        );
        assert_eq!(
            lookup(&fake, "none"),
            FeedOutcome::NoFeed(
                "iTunes Search lists no podcast named \"The Pragmatic Engineer\" with a feed"
                    .to_string()
            )
        );
        // Only the search ran.
        assert_eq!(fake.requests().len(), 1);
    }

    #[test]
    fn an_episode_without_a_show_has_no_feed() {
        let fake = FakeSpotify::new();
        let mut meta = episode();
        meta.show = Some("\u{2060} ".to_string());
        let outcome = find_feed_item(
            &meta,
            PodcastEnrich::Auto,
            &fake.scratch("no-show"),
            later(),
            None,
        );
        assert!(matches!(outcome, FeedOutcome::NoFeed(_)), "{outcome:?}");
        assert!(fake.requests().is_empty());
    }

    #[test]
    fn itunes_failures_mean_no_feed() {
        let fake = FakeSpotify::new();
        fake.serve("itunes", 503, "text/html", b"busy");
        let outcome = lookup(&fake, "itunes-503");
        assert_eq!(
            outcome.reason(),
            Some("iTunes Search https://itunes.apple.com/search?media=podcast&entity=podcast&limit=10&term=The+Pragmatic+Engineer answered HTTP 503")
        );
        assert!(matches!(outcome, FeedOutcome::NoFeed(_)));

        fake.serve("itunes", 200, "text/html", b"<html>not json</html>");
        assert_eq!(
            lookup(&fake, "itunes-html"),
            FeedOutcome::NoFeed(
                "iTunes Search returned unreadable JSON".to_string()
            )
        );
    }

    #[test]
    fn a_404_feed_moves_on_to_the_next_candidate() {
        let fake = FakeSpotify::new();
        let dead = "https://feeds.example.com/dead.rss";
        fake.serve(
            "itunes",
            200,
            "text/javascript",
            itunes_with(&[
                ("The Pragmatic Engineer", "Gergely Orosz", dead),
                ("Pragmatic Engineer", "Gergely Orosz", FEED_URL),
            ])
            .as_bytes(),
        );
        fake.serve_url(dead, 404, "text/html", b"gone");
        fake.serve_url(FEED_URL, 200, "application/rss+xml", FEED.as_bytes());
        let outcome = lookup(&fake, "second");
        assert!(matches!(outcome, FeedOutcome::Matched(_)), "{outcome:?}");
        let requests = fake.requests();
        assert_eq!(&requests[1..], [dead.to_string(), FEED_URL.to_string()]);
    }

    #[test]
    fn candidates_rank_by_publisher_and_stop_at_three() {
        let feeds: Vec<String> = (0..5)
            .map(|n| format!("https://feeds.example.com/{n}.rss"))
            .collect();
        let results = vec![
            ("The Pragmatic Engineer", "Someone Else", feeds[0].as_str()),
            (
                "Pragmatic Engineer",
                "Gergely Orosz Media",
                feeds[1].as_str(),
            ),
            (
                "THE PRAGMATIC ENGINEER!",
                "Gergely Orosz",
                feeds[2].as_str(),
            ),
            (
                "The Pragmatic Engineers",
                "Gergely Orosz",
                feeds[3].as_str(),
            ),
            ("The Pragmatic Engineer", "Gergely Orosz", feeds[2].as_str()),
            ("The Pragmatic Engineer", "", feeds[4].as_str()),
        ];
        let fake = FakeSpotify::new();
        fake.serve(
            "itunes",
            200,
            "text/javascript",
            itunes_with(&results).as_bytes(),
        );
        // Every candidate 404s: the outcome names each failure.
        let outcome = lookup(&fake, "ranked");
        let requests = fake.requests();
        assert_eq!(
            &requests[1..],
            [feeds[2].clone(), feeds[1].clone(), feeds[0].clone()],
            "exact publisher, then a partial one, then iTunes order; at most three"
        );
        let FeedOutcome::NoFeed(reason) = outcome else {
            panic!("expected no feed: {outcome:?}");
        };
        assert_eq!(
            reason,
            format!(
                "feed {} answered HTTP 404; feed {} answered HTTP 404; feed {} answered HTTP 404",
                feeds[2], feeds[1], feeds[0]
            )
        );
    }

    #[test]
    fn a_single_candidate_without_a_match_is_no_match() {
        let fake = FakeSpotify::new();
        fake.serve("itunes", 200, "text/javascript", ITUNES.as_bytes());
        fake.serve_url(FEED_URL, 200, "application/rss+xml", FEED.as_bytes());
        let mut meta = episode();
        meta.title = "An episode this feed never had".to_string();
        meta.released = None;
        let outcome = find_feed_item(
            &meta,
            PodcastEnrich::Auto,
            &fake.scratch("unmatched"),
            later(),
            None,
        );
        assert_eq!(
            outcome,
            FeedOutcome::NoMatch(
                "no item in the api.substack.com feed agrees on two of title, date, and duration"
                    .to_string()
            )
        );
    }

    #[test]
    fn duplicate_title_items_are_ambiguous() {
        let fake = FakeSpotify::new();
        fake.serve("itunes", 200, "text/javascript", ITUNES.as_bytes());
        // A re-run of the episode under the same title and length.
        let rerun = FEED.replace(
            "<title><![CDATA[Software engineering after code becomes cheap with Satya Nadella]]></title>",
            &format!("<title><![CDATA[{TITLE}]]></title>"),
        )
        .replace("<itunes:duration>3760</itunes:duration>", "<itunes:duration>5197</itunes:duration>");
        fake.serve_url(FEED_URL, 200, "application/rss+xml", rerun.as_bytes());
        let outcome = lookup(&fake, "tie");
        let FeedOutcome::NoMatch(reason) = outcome else {
            panic!("expected no match: {outcome:?}");
        };
        assert!(reason.starts_with("ambiguous: 2 items"), "{reason}");
    }

    #[test]
    fn dynamic_ad_insertion_drift_still_matches_on_title_and_date() {
        let fake = FakeSpotify::new();
        fake.serve("itunes", 200, "text/javascript", ITUNES.as_bytes());
        // Ads stitched in at download time add two minutes.
        let drifted = FEED.replace(
            "<itunes:duration>5197</itunes:duration>",
            "<itunes:duration>01:28:37</itunes:duration>",
        );
        fake.serve_url(
            FEED_URL,
            200,
            "application/rss+xml",
            drifted.as_bytes(),
        );
        let FeedOutcome::Matched(item) = lookup(&fake, "dai") else {
            panic!("expected a match");
        };
        assert_eq!(
            item.matched_on,
            vec![MatchSignal::Title, MatchSignal::Date]
        );
        assert_eq!(item.matched_on_label(), "title, date");
        assert_eq!(item.duration_secs, Some(5317));
    }

    #[test]
    fn a_feed_over_the_cap_is_skipped() {
        let fake = FakeSpotify::new();
        fake.serve("itunes", 200, "text/javascript", ITUNES.as_bytes());
        fake.serve_url(FEED_URL, 200, "application/rss+xml", b"");
        // A sparse body one byte over the cap.
        let body = fake
            .root
            .path()
            .join(format!("{}.body", url_surface(FEED_URL)));
        fs::File::options()
            .write(true)
            .open(&body)
            .expect("open body")
            .set_len(FEED_MAX_BYTES + 1)
            .expect("grow body");
        let outcome = lookup(&fake, "big");
        let FeedOutcome::NoFeed(reason) = outcome else {
            panic!("expected no feed: {outcome:?}");
        };
        assert!(reason.contains("download larger than 25 MiB"), "{reason}");
    }

    #[test]
    fn malformed_xml_and_dtds_are_refused() {
        let fake = FakeSpotify::new();
        fake.serve("itunes", 200, "text/javascript", ITUNES.as_bytes());
        fake.serve_url(
            FEED_URL,
            200,
            "application/rss+xml",
            &FEED.as_bytes()[..FEED.len() / 2],
        );
        let FeedOutcome::NoFeed(reason) = lookup(&fake, "malformed") else {
            panic!("expected no feed");
        };
        assert!(
            reason.starts_with(&format!("feed {FEED_URL}: malformed XML (")),
            "{reason}"
        );

        let bomb = FEED.replacen(
            "<rss ",
            "<!DOCTYPE rss [<!ENTITY a \"aaaaaaaa\"><!ENTITY b \"&a;&a;&a;&a;\">]><rss ",
            1,
        )
        .replacen("<title><![CDATA[The Pragmatic Engineer]]></title>", "<title>&b;</title>", 1);
        fake.serve_url(FEED_URL, 200, "application/rss+xml", bomb.as_bytes());
        assert_eq!(
            lookup(&fake, "dtd"),
            FeedOutcome::NoFeed(format!(
                "feed {FEED_URL}: the feed declares a DTD, which bob refuses"
            ))
        );

        fake.serve_url(
            FEED_URL,
            200,
            "application/atom+xml",
            b"<feed xmlns=\"http://www.w3.org/2005/Atom\"/>",
        );
        assert_eq!(
            lookup(&fake, "atom"),
            FeedOutcome::NoFeed(format!(
                "feed {FEED_URL}: not an RSS feed (root element <feed>)"
            ))
        );
    }

    #[test]
    fn a_private_feed_url_is_never_fetched() {
        let fake = FakeSpotify::new();
        fake.serve(
            "itunes",
            200,
            "text/javascript",
            itunes_with(&[(
                "The Pragmatic Engineer",
                "Gergely Orosz",
                "http://127.0.0.1/feed.rss",
            )])
            .as_bytes(),
        );
        let outcome = lookup(&fake, "private");
        assert!(matches!(outcome, FeedOutcome::NoFeed(_)), "{outcome:?}");
        assert_eq!(fake.requests().len(), 1, "only the search ran");
    }

    #[test]
    fn items_read_every_namespace_the_match_needs() {
        let feed = r#"<?xml version="1.0"?>
<rss version="2.0" xmlns:itunes="http://www.itunes.com/dtds/podcast-1.0.dtd"
     xmlns:content="http://purl.org/rss/1.0/modules/content/"
     xmlns:podcast="https://podcastindex.org/namespace/1.0">
<channel><title>Show</title>
<item>
  <title>#123: Tom &amp;amp; Jerry&#8217;s Day</title>
  <itunes:title>Tom &amp; Jerry’s Day</itunes:title>
  <guid>abc-123</guid>
  <pubDate>Fri, 18 Nov 2026 18:00:00 +0000</pubDate>
  <description><![CDATA[<p>Short</p>]]></description>
  <content:encoded><![CDATA[<p>Long {WJ}notes</p>]]></content:encoded>
  <itunes:duration>1:02:03</itunes:duration>
  <podcast:transcript url="/t/123.json" type="Application/JSON" language="en" rel="captions"/>
  <podcast:transcript url="ftp://example.com/t.srt" type="application/srt"/>
  <podcast:chapters url="https://example.com/c/123.json" type="application/json+chapters"/>
</item>
<item><description>   </description><itunes:duration>bogus</itunes:duration></item>
</channel></rss>"#
            .replace("{WJ}", "\u{2060}");
        let parsed = items(&feed);
        assert_eq!(parsed.len(), 2);
        let item = &parsed[0];
        assert_eq!(item.title.as_deref(), Some("#123: Tom & Jerry’s Day"));
        assert_eq!(item.itunes_title.as_deref(), Some("Tom & Jerry’s Day"));
        assert_eq!(item.guid.as_deref(), Some("abc-123"));
        // The weekday is wrong (18 Nov 2026 is a Wednesday); the date
        // still parses.
        assert_eq!(
            item.pub_date.map(|date| date.to_rfc3339()).as_deref(),
            Some("2026-11-18T18:00:00+00:00")
        );
        assert_eq!(item.duration_secs, Some(3723));
        assert_eq!(item.notes_html.as_deref(), Some("<p>Long notes</p>"));
        assert_eq!(
            item.transcripts,
            vec![FeedTranscript {
                url: "https://api.substack.com/t/123.json".to_string(),
                mime: "application/json".to_string(),
                language: Some("en".to_string()),
                rel: Some("captions".to_string()),
            }]
        );
        assert_eq!(
            item.chapters,
            Some(FeedChaptersLink {
                url: "https://example.com/c/123.json".to_string(),
                mime: "application/json+chapters".to_string(),
            })
        );
        assert_eq!(parsed[1], RawItem::default());

        // The numbered title still matches the plain Spotify title.
        let mut meta = episode();
        meta.title = "Tom & Jerry's Day".to_string();
        meta.released = None;
        meta.duration_ms = Some(3_723_400);
        let ItemMatch::Unique { index, signals } = match_items(&meta, &parsed)
        else {
            panic!("expected a unique match");
        };
        assert_eq!(index, 0);
        assert_eq!(signals, vec![MatchSignal::Title, MatchSignal::Duration]);
    }

    #[test]
    fn titles_normalize_numbers_punctuation_and_invisibles() {
        let plain = normalize_title(TITLE);
        for spelling in [
            "#12: Software factories and code reviews with Harjot Gill",
            "# 12 - Software factories and code reviews with Harjot Gill",
            "Ep. 12 - Software factories and code reviews with Harjot Gill",
            "Episode 12: Software Factories & Code Reviews with Harjot Gill",
            "EP12 | Software factories, and code reviews with Harjot Gill!",
            "12 — Software\u{2060} factories and code reviews with Harjot Gill",
        ] {
            assert_eq!(normalize_title(spelling), plain, "{spelling}");
        }
        assert_eq!(
            normalize_title("Don’t panic"),
            normalize_title("Dont panic")
        );
        // A title that is only a number keeps it.
        assert_eq!(normalize_title("#42:"), "42");
        // A leading year without a separator is not an episode number.
        assert_eq!(normalize_title("2025 in review"), "2025 in review");
        assert_ne!(normalize_title("Epic stories"), normalize_title("stories"));
        assert_eq!(
            normalize_show("The Pragmatic Engineer"),
            "pragmatic engineer"
        );
        assert_eq!(normalize_show("Pragmatic Engineer"), "pragmatic engineer");
    }

    #[test]
    fn signals_follow_their_windows() {
        let parsed = items(FEED);
        let meta = episode();
        let wanted = normalize_title(&meta.title);
        let score =
            |meta: &EpisodeMetadata| score_item(meta, &wanted, &parsed[0]);
        assert_eq!(score(&meta).len(), 3);

        // 36 h either side scores the date; past it does not.
        let mut shifted = meta.clone();
        let base = meta.released.clone().expect("released").instant;
        for (offset, scores) in [
            (36 * 60 - 1, true),
            (-(36 * 60 - 1), true),
            (36 * 60 + 2, false),
        ] {
            shifted.released = Some(Released {
                instant: base + Duration::minutes(offset),
                precision: ReleasePrecision::Time,
            });
            assert_eq!(
                score(&shifted).contains(&MatchSignal::Date),
                scores,
                "{offset} minutes"
            );
        }

        // Duration: max(5 s, 2 %) of Spotify's.
        let mut timed = meta.clone();
        for (spotify_ms, scores) in [
            (5_197_000 + 103_000, true),  // within 2 % of 5300 s
            (5_197_000 + 107_000, false), // just over it
            (5_197_000, true),
        ] {
            timed.duration_ms = Some(spotify_ms);
            assert_eq!(
                score(&timed).contains(&MatchSignal::Duration),
                scores,
                "{spotify_ms}"
            );
        }
        let short = RawItem {
            duration_secs: Some(100),
            ..RawItem::default()
        };
        let mut brief = meta.clone();
        brief.duration_ms = Some(104_900);
        assert_eq!(
            score_item(&brief, &wanted, &short),
            vec![MatchSignal::Duration]
        );
        brief.duration_ms = Some(105_100);
        assert!(score_item(&brief, &wanted, &short).is_empty());

        // Missing fields score nothing.
        let mut bare = meta.clone();
        bare.released = None;
        bare.duration_ms = None;
        assert_eq!(score(&bare), vec![MatchSignal::Title]);
    }

    #[test]
    fn durations_and_dates_parse_feed_spellings() {
        assert_eq!(parse_duration_secs("5197"), Some(5197));
        assert_eq!(parse_duration_secs(" 5197.6 "), Some(5198));
        assert_eq!(parse_duration_secs("86:37"), Some(5197));
        assert_eq!(parse_duration_secs("1:26:37"), Some(5197));
        assert_eq!(parse_duration_secs("01:26:37.4"), Some(5197));
        for bad in ["", "0", "-5", "1:2:3:4", "abc", "NaN", "inf", "1.5:00"] {
            assert_eq!(parse_duration_secs(bad), None, "{bad}");
        }
        let date =
            |raw: &str| parse_pub_date(raw).map(|date| date.to_rfc3339());
        assert_eq!(
            date("Wed, 18 Nov 2026 18:08:52 GMT").as_deref(),
            Some("2026-11-18T18:08:52+00:00")
        );
        assert_eq!(
            date("Wed, 18 Nov 2026 13:08:52 EST").as_deref(),
            Some("2026-11-18T18:08:52+00:00")
        );
        assert_eq!(
            date("2026-11-18T13:08:52-05:00").as_deref(),
            Some("2026-11-18T18:08:52+00:00")
        );
        assert_eq!(date("sometime last week"), None);
    }

    #[test]
    fn chapters_json_parses_and_validates() {
        let chapters =
            parse_chapters_json(CHAPTERS.as_bytes()).expect("chapters");
        assert_eq!(
            chapters,
            vec![
                Chapter {
                    start_secs: 0,
                    title: "Intro".to_string()
                },
                Chapter {
                    start_secs: 168,
                    title: "What a software factory is".to_string()
                },
                Chapter {
                    start_secs: 1235,
                    title: "Code review at agent speed".to_string()
                },
                Chapter {
                    start_secs: 5070,
                    title: "Book recommendation".to_string()
                },
            ]
        );

        for (body, reason) in [
            ("{", "the chapters file is not valid JSON"),
            ("{\"version\":\"1.2.0\"}", "the chapters file has no chapters list"),
            ("{\"chapters\":[]}", "the chapters file lists no titled chapters"),
            (
                "{\"chapters\":[{\"title\":\"No time\"}]}",
                "chapter 1 has no valid startTime",
            ),
            (
                "{\"chapters\":[{\"startTime\":-1,\"title\":\"Before\"}]}",
                "chapter 1 has no valid startTime",
            ),
            (
                "{\"chapters\":[{\"startTime\":60,\"title\":\"A\"},{\"startTime\":60.5,\"title\":\"B\"}]}",
                "chapter 2 does not start after the one before it",
            ),
            (
                "{\"chapters\":[{\"startTime\":90,\"title\":\"A\"},{\"startTime\":30,\"title\":\"B\"}]}",
                "chapter 2 does not start after the one before it",
            ),
        ] {
            assert_eq!(parse_chapters_json(body.as_bytes()), Err(reason.to_string()), "{body}");
        }
    }

    #[test]
    fn chapters_fetch_fails_open_with_a_reason() {
        let fake = FakeSpotify::new();
        let link = FeedChaptersLink {
            url: "https://example.com/c/123.json".to_string(),
            mime: "application/json+chapters".to_string(),
        };
        fake.serve_url(&link.url, 200, "application/json", CHAPTERS.as_bytes());
        let ChaptersOutcome::Chapters(chapters) = fetch_feed_chapters(
            &link,
            &fake.scratch("chapters"),
            later(),
            None,
        ) else {
            panic!("expected chapters");
        };
        assert_eq!(chapters.len(), 4);

        fake.serve_url(&link.url, 200, "application/json", b"<html/>");
        assert_eq!(
            fetch_feed_chapters(&link, &fake.scratch("bad"), later(), None),
            ChaptersOutcome::Ignored(
                "the chapters file is not valid JSON".to_string()
            )
        );
        fake.serve_url(&link.url, 404, "text/html", b"");
        assert_eq!(
            fetch_feed_chapters(&link, &fake.scratch("gone"), later(), None),
            ChaptersOutcome::Ignored(
                "chapters file https://example.com/c/123.json answered HTTP 404".to_string()
            )
        );
        let before = fake.requests().len();
        assert_eq!(
            fetch_feed_chapters(
                &link,
                &fake.scratch("late"),
                Instant::now(),
                None
            ),
            ChaptersOutcome::DeadlineExceeded
        );
        assert_eq!(fake.requests().len(), before);
    }

    #[test]
    fn itunes_results_need_a_name_and_a_feed() {
        let parsed = parse_itunes_results(
            br#"{"results":[
                {"collectionName":"A","artistName":"X","feedUrl":"https://a.example/f"},
                {"collectionName":"B","artistName":"Y"},
                {"artistName":"Z","feedUrl":"https://c.example/f"},
                {"collectionName":" ","feedUrl":"https://d.example/f"},
                {"collectionName":"E","feedUrl":"https://e.example/f"}
            ]}"#,
        )
        .expect("results");
        assert_eq!(
            parsed
                .iter()
                .map(|r| r.collection_name.as_str())
                .collect::<Vec<_>>(),
            ["A", "E"]
        );
        assert_eq!(parsed[1].artist_name, None);
        assert_eq!(parse_itunes_results(b"[]"), None);
        assert_eq!(
            itunes_search_url("Lex Fridman & Co./ü"),
            "https://itunes.apple.com/search?media=podcast&entity=podcast&limit=10&term=Lex+Fridman+%26+Co.%2F%C3%BC"
        );
    }
}
