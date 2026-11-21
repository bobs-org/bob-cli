//! Spotify episode metadata, browserless.
//!
//! [`fetch_episode`] climbs a ladder over Spotify's public surfaces:
//!
//! 1. the episode page (`https://open.spotify.com/episode/<id>`, 5 MiB
//!    cap): the base64 `initialState` entity, then the JSON-LD
//!    `PodcastEpisode`, then the `og:`/`music:` meta tags;
//! 2. the embed page (`/embed/episode/<id>`) `__NEXT_DATA__`, when the
//!    title or the show notes are still missing;
//! 3. oEmbed, when the title is still missing.
//!
//! A later source fills only fields that are still empty, and every field
//! records the tier that filled it. Each source must name the exact
//! (case-preserved) episode id, so a page about another entity
//! contributes nothing. Plain `curl` matters here: Spotify serves the
//! full server-rendered page to a plain client and an empty app shell to
//! a browser user agent.
//!
//! The ladder fails closed: no title from any tier is an error, never a
//! stub. A 404 page is decisive, and a 429/5xx or transport failure on
//! the page is a retryable error rather than a degraded capture, because
//! a degraded reference would be permanent while a retry would be whole.

use std::{collections::BTreeMap, fs, path::Path, sync::OnceLock};

use base64::Engine as _;
use chrono::{DateTime, Local, NaiveDate, Utc};
use chrono_tz::Tz;
use regex::Regex;
use scraper::{Html, Selector};
use serde_json::Value;

use super::super::fetch::{fetch_url_capped, FetchError};
use super::super::spotify::{SpotifyEpisode, SpotifyLink};
use super::{clean_line, strip_invisibles};
use crate::native::env as bob_env;

/// Size cap for the episode and embed pages.
const PAGE_MAX_BYTES: u64 = 5 * 1024 * 1024;

/// Size cap for the oEmbed JSON.
const OEMBED_MAX_BYTES: u64 = 1024 * 1024;

/// Per-request time cap for every ladder fetch.
const FETCH_MAX_TIME_SECS: u64 = 30;

/// Cover art is chosen at most this many pixels on its longer side.
const COVER_MAX_PX: u32 = 640;

/// The Spotify surface a field came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) enum MetadataSource {
    Page,
    Embed,
    Oembed,
}

impl MetadataSource {
    /// The stable identifier: `spotify_page`, `spotify_embed`, or
    /// `spotify_oembed`.
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Page => "spotify_page",
            Self::Embed => "spotify_embed",
            Self::Oembed => "spotify_oembed",
        }
    }

    /// The report label, as in `title: … (spotify page)`.
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Page => "spotify page",
            Self::Embed => "spotify embed",
            Self::Oembed => "spotify oEmbed",
        }
    }

    /// The surface as warnings name it.
    fn surface(self) -> &'static str {
        match self {
            Self::Page => "the episode page",
            Self::Embed => "the embed player",
            Self::Oembed => "oEmbed",
        }
    }
}

/// A field of [`EpisodeMetadata`] whose source is recorded.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) enum EpisodeField {
    Title,
    Show,
    ShowUri,
    Publisher,
    Released,
    Duration,
    Language,
    Cover,
    NotesHtml,
    NotesText,
}

/// How exact a release instant is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ReleasePrecision {
    /// A calendar date (Spotify's `DAY`, `MONTH`, or `YEAR`): the date is
    /// used as given, never shifted into a zone.
    Day,
    /// A moment in time (`MINUTE`, `SECOND`, …): its local date depends
    /// on the zone.
    Time,
}

/// When an episode was released.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Released {
    /// The release instant (midnight UTC of the date for
    /// [`ReleasePrecision::Day`]).
    pub(crate) instant: DateTime<Utc>,
    pub(crate) precision: ReleasePrecision,
}

impl Released {
    /// The release date in `zone` (the host zone when `None`). A
    /// [`ReleasePrecision::Day`] release keeps its date as given.
    pub(crate) fn date_in(&self, zone: Option<Tz>) -> NaiveDate {
        match (self.precision, zone) {
            (ReleasePrecision::Day, _) => self.instant.date_naive(),
            (ReleasePrecision::Time, Some(zone)) => {
                self.instant.with_timezone(&zone).date_naive()
            }
            (ReleasePrecision::Time, None) => {
                self.instant.with_timezone(&Local).date_naive()
            }
        }
    }
}

/// Everything the ladder learned about one episode.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct EpisodeMetadata {
    /// The case-preserved Spotify episode id.
    pub(crate) id: String,
    /// `https://open.spotify.com/episode/<id>`.
    pub(crate) canonical_url: String,
    pub(crate) title: String,
    pub(crate) show: Option<String>,
    /// `spotify:show:<id>`.
    pub(crate) show_uri: Option<String>,
    pub(crate) publisher: Option<String>,
    pub(crate) released: Option<Released>,
    pub(crate) duration_ms: Option<u64>,
    /// The episode language code, e.g. `en`.
    pub(crate) language: Option<String>,
    /// Episode art at most 640 px, else show art.
    pub(crate) cover_url: Option<String>,
    /// The publisher's HTML show notes, invisibles stripped, otherwise
    /// raw (the renderer sanitizes).
    pub(crate) notes_html: Option<String>,
    /// The plain-text show notes.
    pub(crate) notes_text: Option<String>,
    /// The tier that filled each present field.
    pub(crate) sources: BTreeMap<EpisodeField, MetadataSource>,
    /// At most one warning naming what a fallback lost.
    pub(crate) warnings: Vec<String>,
}

impl EpisodeMetadata {
    /// The release date in Bob's day zone (the zone `capture_clock`
    /// uses): the configured `timezone`, else the host zone.
    pub(crate) fn published_date(&self) -> Option<NaiveDate> {
        self.released
            .as_ref()
            .map(|released| released.date_in(bob_env::configured_timezone()))
    }

    /// The tier that filled `field`, when it is present.
    pub(crate) fn source_of(
        &self,
        field: EpisodeField,
    ) -> Option<MetadataSource> {
        self.sources.get(&field).copied()
    }
}

/// Why the ladder produced no metadata.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum EpisodeError {
    /// The episode page answered 404.
    NotFound { id: String },
    /// An HTTP status that stopped the ladder: 429 and 5xx are
    /// retryable.
    HttpStatus { id: String, status: u16 },
    /// A transport failure (network, timeout, a refused redirect).
    Fetch { id: String, error: FetchError },
    /// No tier yielded a title.
    NoMetadata { id: String },
}

impl EpisodeError {
    /// The text after `error:`.
    pub(crate) fn message(&self) -> String {
        match self {
            Self::NotFound { id } => {
                format!("Spotify has no public episode {id} (HTTP 404)")
            }
            Self::HttpStatus { id, status } => {
                format!("Spotify answered HTTP {status} for episode {id}")
            }
            Self::Fetch { id, error } => format!(
                "could not fetch Spotify episode {id}: {}",
                error.message()
            ),
            Self::NoMetadata { id } => {
                format!("Spotify returned no episode metadata for {id}")
            }
        }
    }

    /// The text after `hint:`, when there is one.
    pub(crate) fn hint(&self) -> Option<String> {
        match self {
            Self::NotFound { .. } => None,
            Self::HttpStatus { status, .. } => {
                is_transient_status(*status).then(|| "retry later".to_string())
            }
            Self::Fetch { error, .. } => error.hint().map(str::to_string),
            Self::NoMetadata { .. } => Some(
                "retry later; if it persists, Spotify changed its page and bob needs an update"
                    .to_string(),
            ),
        }
    }

    /// Whether retrying later can help: 429, 5xx, and transport failures.
    pub(crate) fn retryable(&self) -> bool {
        match self {
            Self::HttpStatus { status, .. } => is_transient_status(*status),
            Self::Fetch { .. } => true,
            Self::NotFound { .. } | Self::NoMetadata { .. } => false,
        }
    }

    /// The HTTP status behind the failure, when there is one.
    pub(crate) fn status(&self) -> Option<u16> {
        match self {
            Self::NotFound { .. } => Some(404),
            Self::HttpStatus { status, .. } => Some(*status),
            Self::Fetch { .. } | Self::NoMetadata { .. } => None,
        }
    }

    /// The `message\nhint: hint` body the error renderers expect.
    pub(crate) fn render(&self) -> String {
        match self.hint() {
            Some(hint) => format!("{}\nhint: {hint}", self.message()),
            None => self.message(),
        }
    }
}

/// 429 and every 5xx.
fn is_transient_status(status: u16) -> bool {
    status == 429 || (500..600).contains(&status)
}

/// Climb the metadata ladder for `episode`, downloading into `scratch`.
///
/// `prefetched_page` is the body of an already-downloaded 200 episode
/// page (a resolved short link's final body); when given, the page is
/// not fetched again.
pub(crate) fn fetch_episode(
    episode: &SpotifyEpisode,
    scratch: &Path,
    progress: Option<&dyn Fn(&str)>,
    prefetched_page: Option<&Path>,
) -> Result<EpisodeMetadata, EpisodeError> {
    let id = episode.id.as_str();
    let canonical = episode.canonical_url();
    let mut ladder = Ladder::default();

    // Tier 1: the episode page. Its failures are decisive.
    let page = match prefetched_page {
        Some(path) => read_lossy(path),
        None => {
            match fetch_surface(
                &canonical,
                &scratch.join("spotify-episode-page.html"),
                PAGE_MAX_BYTES,
                progress,
                id,
            )? {
                Surface::Body(body) => Some(body),
                Surface::Status(404) => {
                    return Err(EpisodeError::NotFound { id: id.to_string() })
                }
                Surface::Status(status) if is_transient_status(status) => {
                    return Err(EpisodeError::HttpStatus {
                        id: id.to_string(),
                        status,
                    })
                }
                // Any other status reads as page drift: fall down the
                // ladder.
                Surface::Status(_) => None,
            }
        }
    };
    if let Some(page) = page {
        ladder.absorb(parse_page(&page, id), MetadataSource::Page);
    }

    // Later tiers fail open; a transient failure only surfaces when no
    // tier produced a title.
    let mut transient: Option<EpisodeError> = None;
    if ladder.fields.title.is_none() || !ladder.fields.has_notes() {
        let url = format!("https://open.spotify.com/embed/episode/{id}");
        match fetch_surface(
            &url,
            &scratch.join("spotify-episode-embed.html"),
            PAGE_MAX_BYTES,
            progress,
            id,
        ) {
            Ok(Surface::Body(body)) => {
                ladder.absorb(parse_embed(&body, id), MetadataSource::Embed)
            }
            Ok(Surface::Status(status)) if is_transient_status(status) => {
                transient.get_or_insert(EpisodeError::HttpStatus {
                    id: id.to_string(),
                    status,
                });
            }
            Ok(Surface::Status(_)) => {}
            Err(error) => {
                transient.get_or_insert(error);
            }
        }
    }
    if ladder.fields.title.is_none() {
        let url = format!(
            "https://open.spotify.com/oembed?url={}",
            url_encode_component(&canonical)
        );
        match fetch_surface(
            &url,
            &scratch.join("spotify-episode-oembed.json"),
            OEMBED_MAX_BYTES,
            progress,
            id,
        ) {
            Ok(Surface::Body(body)) => {
                ladder.absorb(parse_oembed(&body, id), MetadataSource::Oembed)
            }
            Ok(Surface::Status(status)) if is_transient_status(status) => {
                transient.get_or_insert(EpisodeError::HttpStatus {
                    id: id.to_string(),
                    status,
                });
            }
            Ok(Surface::Status(_)) => {}
            Err(error) => {
                transient.get_or_insert(error);
            }
        }
    }

    ladder.finish(episode).ok_or_else(|| {
        transient
            .unwrap_or_else(|| EpisodeError::NoMetadata { id: id.to_string() })
    })
}

/// What one ladder fetch produced.
enum Surface {
    /// A 200 body.
    Body(String),
    /// Any other HTTP status.
    Status(u16),
}

fn fetch_surface(
    url: &str,
    dest: &Path,
    max_bytes: u64,
    progress: Option<&dyn Fn(&str)>,
    id: &str,
) -> Result<Surface, EpisodeError> {
    let fetched =
        fetch_url_capped(url, dest, FETCH_MAX_TIME_SECS, max_bytes, progress)
            .map_err(|error| EpisodeError::Fetch {
            id: id.to_string(),
            error,
        })?;
    if fetched.status != 200 {
        return Ok(Surface::Status(fetched.status));
    }
    Ok(read_lossy(&fetched.path)
        .map(Surface::Body)
        .unwrap_or(Surface::Status(fetched.status)))
}

fn read_lossy(path: &Path) -> Option<String> {
    fs::read(path)
        .ok()
        .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
}

/// Percent-encode a query component (RFC 3986 unreserved kept).
fn url_encode_component(value: &str) -> String {
    let mut out = String::with_capacity(value.len() * 3);
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric()
            || matches!(byte, b'-' | b'_' | b'.' | b'~')
        {
            out.push(byte as char);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

/// One image candidate: its URL and pixel size when known.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Art {
    url: String,
    size: Option<u32>,
}

/// The fields one source yielded.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
struct Fields {
    title: Option<String>,
    show: Option<String>,
    show_uri: Option<String>,
    publisher: Option<String>,
    released: Option<Released>,
    duration_ms: Option<u64>,
    language: Option<String>,
    episode_art: Option<String>,
    show_art: Option<String>,
    notes_html: Option<String>,
    notes_text: Option<String>,
}

impl Fields {
    fn has_notes(&self) -> bool {
        self.notes_html.is_some() || self.notes_text.is_some()
    }

    /// Fill each still-empty field from `later` (within one tier).
    fn fill_from(&mut self, later: Fields) {
        fill(&mut self.title, later.title);
        fill(&mut self.show, later.show);
        fill(&mut self.show_uri, later.show_uri);
        fill(&mut self.publisher, later.publisher);
        fill(&mut self.released, later.released);
        fill(&mut self.duration_ms, later.duration_ms);
        fill(&mut self.language, later.language);
        fill(&mut self.episode_art, later.episode_art);
        fill(&mut self.show_art, later.show_art);
        fill(&mut self.notes_html, later.notes_html);
        fill(&mut self.notes_text, later.notes_text);
    }
}

fn fill<T>(slot: &mut Option<T>, value: Option<T>) {
    if slot.is_none() {
        *slot = value;
    }
}

/// Where each internal slot came from, for the art slots that fold into
/// one public `cover_url`.
#[derive(Debug, Default)]
struct Ladder {
    fields: Fields,
    sources: BTreeMap<EpisodeField, MetadataSource>,
    episode_art_source: Option<MetadataSource>,
    show_art_source: Option<MetadataSource>,
}

impl Ladder {
    /// Fill each still-empty field from one tier, recording the tier.
    fn absorb(&mut self, tier: Fields, source: MetadataSource) {
        let mut record = |field: EpisodeField, filled: bool| {
            if filled {
                self.sources.insert(field, source);
            }
        };
        let f = &mut self.fields;
        record(EpisodeField::Title, take(&mut f.title, tier.title));
        record(EpisodeField::Show, take(&mut f.show, tier.show));
        record(EpisodeField::ShowUri, take(&mut f.show_uri, tier.show_uri));
        record(
            EpisodeField::Publisher,
            take(&mut f.publisher, tier.publisher),
        );
        record(EpisodeField::Released, take(&mut f.released, tier.released));
        record(
            EpisodeField::Duration,
            take(&mut f.duration_ms, tier.duration_ms),
        );
        record(EpisodeField::Language, take(&mut f.language, tier.language));
        record(
            EpisodeField::NotesHtml,
            take(&mut f.notes_html, tier.notes_html),
        );
        record(
            EpisodeField::NotesText,
            take(&mut f.notes_text, tier.notes_text),
        );
        if take(&mut f.episode_art, tier.episode_art) {
            self.episode_art_source = Some(source);
        }
        if take(&mut f.show_art, tier.show_art) {
            self.show_art_source = Some(source);
        }
    }

    /// The finished metadata, or `None` when no tier yielded a title.
    fn finish(self, episode: &SpotifyEpisode) -> Option<EpisodeMetadata> {
        let Ladder {
            fields,
            mut sources,
            episode_art_source,
            show_art_source,
        } = self;
        let title = fields.title?;
        let (cover_url, cover_source) =
            match (fields.episode_art, fields.show_art) {
                (Some(url), _) => (Some(url), episode_art_source),
                (None, Some(url)) => (Some(url), show_art_source),
                (None, None) => (None, None),
            };
        if let Some(source) = cover_source {
            sources.insert(EpisodeField::Cover, source);
        }
        let mut missing = Vec::new();
        if fields.notes_html.is_none() && fields.notes_text.is_none() {
            missing.push("show notes");
        }
        if fields.show.is_none() {
            missing.push("show name");
        }
        if fields.released.is_none() {
            missing.push("release date");
        }
        if fields.duration_ms.is_none() {
            missing.push("duration");
        }
        let warnings = ladder_warning(&sources, &missing).into_iter().collect();
        Some(EpisodeMetadata {
            id: episode.id.clone(),
            canonical_url: episode.canonical_url(),
            title,
            show: fields.show,
            show_uri: fields.show_uri,
            publisher: fields.publisher,
            released: fields.released,
            duration_ms: fields.duration_ms,
            language: fields.language,
            cover_url,
            notes_html: fields.notes_html,
            notes_text: fields.notes_text,
            sources,
            warnings,
        })
    }
}

/// Fill `slot` from `value` when it is empty; whether it was filled.
fn take<T>(slot: &mut Option<T>, value: Option<T>) -> bool {
    if slot.is_none() && value.is_some() {
        *slot = value;
        true
    } else {
        false
    }
}

/// The one warning a degraded ladder adds: which fallback surfaces were
/// used and what is still missing. `None` when the page held everything.
fn ladder_warning(
    sources: &BTreeMap<EpisodeField, MetadataSource>,
    missing: &[&str],
) -> Option<String> {
    let mut fallbacks: Vec<MetadataSource> = sources
        .values()
        .copied()
        .filter(|source| *source != MetadataSource::Page)
        .collect();
    fallbacks.sort();
    fallbacks.dedup();
    let missing = missing.join(", ");
    match (fallbacks.is_empty(), missing.is_empty()) {
        (true, true) => None,
        (true, false) => {
            Some(format!("Spotify episode metadata is missing {missing}"))
        }
        (false, _) => {
            let surfaces = fallbacks
                .iter()
                .map(|source| source.surface())
                .collect::<Vec<_>>()
                .join(" and ");
            Some(if missing.is_empty() {
                format!("Spotify episode metadata fell back to {surfaces}")
            } else {
                format!(
                    "Spotify episode metadata fell back to {surfaces}; missing {missing}"
                )
            })
        }
    }
}

// ---------------------------------------------------------------------
// Tier 1: the episode page.
// ---------------------------------------------------------------------

/// Parse the episode page: `initialState`, then JSON-LD, then meta tags.
fn parse_page(html: &str, id: &str) -> Fields {
    let document = Html::parse_document(html);
    let mut fields = initial_state_fields(&document, id).unwrap_or_default();
    if let Some(json_ld) = json_ld_fields(&document, id) {
        fields.fill_from(json_ld);
    }
    if let Some(meta) = meta_fields(&document, id) {
        fields.fill_from(meta);
    }
    fields
}

fn selector(css: &str) -> Selector {
    Selector::parse(css).expect("valid selector")
}

/// The `initialState` entity under the exact key `spotify:episode:<id>`
/// whose `__typename` is `Episode`.
fn initial_state_fields(document: &Html, id: &str) -> Option<Fields> {
    let script = document.select(&selector("script#initialState")).next()?;
    let state = decode_initial_state(&script.text().collect::<String>())?;
    let entity = state
        .get("entities")?
        .get("items")?
        .get(format!("spotify:episode:{id}"))?;
    if entity.get("__typename")?.as_str()? != "Episode" {
        return None;
    }
    let show = entity
        .get("showOrAudiobook")
        .and_then(|wrapper| wrapper.get("data"));
    Some(Fields {
        title: str_at(entity, &["name"]).and_then(clean_line),
        show: show
            .and_then(|show| str_at(show, &["name"]))
            .and_then(clean_line),
        show_uri: show
            .and_then(|show| str_at(show, &["uri"]))
            .filter(|uri| uri.starts_with("spotify:show:"))
            .map(str::to_string),
        publisher: show
            .and_then(|show| str_at(show, &["publisher", "name"]))
            .and_then(clean_line),
        released: entity.get("releaseDate").and_then(release_from_value),
        duration_ms: entity
            .get("duration")
            .and_then(|duration| duration.get("totalMilliseconds"))
            .and_then(positive_u64),
        language: str_at(entity, &["language", "code"])
            .and_then(clean_language),
        episode_art: entity
            .get("coverArt")
            .and_then(|art| art.get("sources"))
            .and_then(art_from_sources),
        show_art: show
            .and_then(|show| show.get("coverArt"))
            .and_then(|art| art.get("sources"))
            .and_then(art_from_sources),
        notes_html: str_at(entity, &["htmlDescription"]).and_then(clean_notes),
        notes_text: str_at(entity, &["description"]).and_then(clean_notes),
    })
}

/// Base64-decode (tolerating padding and URL-safe variants) and parse
/// the `initialState` script; raw JSON is accepted too.
fn decode_initial_state(text: &str) -> Option<Value> {
    let compact: String = text.chars().filter(|c| !c.is_whitespace()).collect();
    if compact.starts_with('{') {
        return serde_json::from_str(&compact).ok();
    }
    use base64::engine::general_purpose::{
        STANDARD, STANDARD_NO_PAD, URL_SAFE, URL_SAFE_NO_PAD,
    };
    let bytes = [STANDARD, STANDARD_NO_PAD, URL_SAFE, URL_SAFE_NO_PAD]
        .iter()
        .find_map(|engine| engine.decode(compact.as_bytes()).ok())?;
    serde_json::from_slice(&bytes).ok()
}

/// The JSON-LD `PodcastEpisode` (an object, an array member, or a
/// `@graph` member) whose `@id`/`url` names the episode.
fn json_ld_fields(document: &Html, id: &str) -> Option<Fields> {
    for script in
        document.select(&selector(r#"script[type="application/ld+json"]"#))
    {
        let Ok(value) =
            serde_json::from_str::<Value>(&script.text().collect::<String>())
        else {
            continue;
        };
        let mut candidates = Vec::new();
        collect_json_ld_nodes(&value, &mut candidates);
        if let Some(node) = candidates
            .into_iter()
            .find(|node| is_podcast_episode(node) && json_ld_names(node, id))
        {
            return Some(json_ld_episode_fields(node));
        }
    }
    None
}

fn collect_json_ld_nodes<'a>(value: &'a Value, out: &mut Vec<&'a Value>) {
    match value {
        Value::Array(items) => {
            for item in items {
                collect_json_ld_nodes(item, out);
            }
        }
        Value::Object(map) => {
            out.push(value);
            if let Some(graph) = map.get("@graph") {
                collect_json_ld_nodes(graph, out);
            }
        }
        _ => {}
    }
}

fn is_podcast_episode(node: &Value) -> bool {
    match node.get("@type") {
        Some(Value::String(kind)) => kind == "PodcastEpisode",
        Some(Value::Array(kinds)) => kinds
            .iter()
            .any(|kind| kind.as_str() == Some("PodcastEpisode")),
        _ => false,
    }
}

/// At least one of `@id`/`url` names the episode, and none names
/// another.
fn json_ld_names(node: &Value, id: &str) -> bool {
    let named: Vec<Option<String>> = ["@id", "url"]
        .iter()
        .filter_map(|key| node.get(*key).and_then(Value::as_str))
        .map(episode_id_of)
        .collect();
    !named.is_empty() && named.iter().all(|named| named.as_deref() == Some(id))
}

/// The episode id a URL names, when it names one.
fn episode_id_of(url: &str) -> Option<String> {
    SpotifyLink::parse(url)?.episode().map(|episode| episode.id)
}

fn json_ld_episode_fields(node: &Value) -> Fields {
    let (show, notes_text) = match str_at(node, &["description"]) {
        Some(description) => split_listen_prefix(description),
        None => (None, None),
    };
    Fields {
        title: str_at(node, &["name"]).and_then(clean_line),
        show,
        released: str_at(node, &["datePublished"]).and_then(parse_release),
        notes_text,
        ..Fields::default()
    }
}

/// Split JSON-LD's `Listen to this episode from <show> on Spotify. `
/// prefix off a description: `(show, rest)`.
fn split_listen_prefix(description: &str) -> (Option<String>, Option<String>) {
    static PREFIX: OnceLock<Regex> = OnceLock::new();
    let prefix = PREFIX.get_or_init(|| {
        Regex::new(
            r"^\s*Listen to this episode from (.+?) on Spotify\.(?:\s+|$)",
        )
        .expect("valid listen prefix")
    });
    match prefix.captures(description) {
        Some(captures) => (
            clean_line(&captures[1]),
            clean_notes(&description[captures[0].len()..]),
        ),
        None => (None, clean_notes(description)),
    }
}

/// The `og:`/`music:` meta tags, trusted only when `og:url` names the
/// episode. `og:type` (which says `music.song`) is ignored.
fn meta_fields(document: &Html, id: &str) -> Option<Fields> {
    let mut tags: BTreeMap<String, String> = BTreeMap::new();
    for meta in document.select(&selector("meta")) {
        let element = meta.value();
        let Some(key) =
            element.attr("property").or_else(|| element.attr("name"))
        else {
            continue;
        };
        let Some(content) = element.attr("content") else {
            continue;
        };
        tags.entry(key.to_ascii_lowercase())
            .or_insert_with(|| content.to_string());
    }
    if episode_id_of(tags.get("og:url")?).as_deref() != Some(id) {
        return None;
    }
    Some(Fields {
        title: tags
            .get("og:title")
            .map(String::as_str)
            .and_then(clean_line),
        released: tags
            .get("music:release_date")
            .map(String::as_str)
            .and_then(parse_release),
        duration_ms: tags
            .get("music:duration")
            .and_then(|secs| secs.trim().parse::<u64>().ok())
            .filter(|secs| *secs > 0)
            .map(|secs| secs * 1000),
        episode_art: tags.get("og:image").and_then(|url| {
            let size = tags
                .get("og:image:width")
                .and_then(|width| width.trim().parse().ok());
            choose_art(vec![Art {
                url: url.clone(),
                size,
            }])
        }),
        ..Fields::default()
    })
}

// ---------------------------------------------------------------------
// Tier 2: the embed page.
// ---------------------------------------------------------------------

/// The embed page's `__NEXT_DATA__` entity, when it names the episode.
fn parse_embed(html: &str, id: &str) -> Fields {
    embed_fields(html, id).unwrap_or_default()
}

fn embed_fields(html: &str, id: &str) -> Option<Fields> {
    let document = Html::parse_document(html);
    let script = document.select(&selector("script#__NEXT_DATA__")).next()?;
    let data: Value =
        serde_json::from_str(&script.text().collect::<String>()).ok()?;
    let entity = data
        .get("props")?
        .get("pageProps")?
        .get("state")?
        .get("data")?
        .get("entity")?;
    if str_at(entity, &["type"]).is_some_and(|kind| kind != "episode") {
        return None;
    }
    let uri = format!("spotify:episode:{id}");
    let claims = [
        str_at(entity, &["id"]).map(|claimed| claimed == id),
        str_at(entity, &["uri"]).map(|claimed| claimed == uri),
    ];
    if claims.iter().all(Option::is_none) || claims.contains(&Some(false)) {
        return None;
    }
    let duration_ms = match entity.get("duration") {
        Some(Value::Object(object)) => {
            object.get("totalMilliseconds").and_then(positive_u64)
        }
        Some(value) => positive_u64(value),
        None => None,
    };
    Some(Fields {
        title: str_at(entity, &["name"])
            .or_else(|| str_at(entity, &["title"]))
            .and_then(clean_line),
        show: str_at(entity, &["subtitle"]).and_then(clean_line),
        show_uri: str_at(entity, &["relatedEntityUri"])
            .filter(|uri| uri.starts_with("spotify:show:"))
            .map(str::to_string),
        released: entity.get("releaseDate").and_then(release_from_value),
        duration_ms,
        episode_art: entity
            .get("visualIdentity")
            .and_then(|identity| identity.get("image"))
            .and_then(art_from_sources)
            .or_else(|| {
                entity
                    .get("coverArt")
                    .and_then(|art| art.get("sources"))
                    .and_then(art_from_sources)
            }),
        show_art: entity
            .get("relatedEntityCoverArt")
            .and_then(art_from_sources),
        notes_text: str_at(entity, &["description"]).and_then(clean_notes),
        ..Fields::default()
    })
}

// ---------------------------------------------------------------------
// Tier 3: oEmbed.
// ---------------------------------------------------------------------

/// The oEmbed title and thumbnail. The thumbnail host varies, so it is
/// not checked; an `iframe_url` naming another episode rejects it all.
fn parse_oembed(body: &str, id: &str) -> Fields {
    let Ok(value) = serde_json::from_str::<Value>(body) else {
        return Fields::default();
    };
    if str_at(&value, &["iframe_url"])
        .is_some_and(|iframe| episode_id_of(iframe).as_deref() != Some(id))
    {
        return Fields::default();
    }
    Fields {
        title: str_at(&value, &["title"]).and_then(clean_line),
        episode_art: str_at(&value, &["thumbnail_url"]).and_then(|url| {
            choose_art(vec![Art {
                url: url.to_string(),
                size: value
                    .get("thumbnail_width")
                    .and_then(Value::as_u64)
                    .and_then(|width| u32::try_from(width).ok()),
            }])
        }),
        ..Fields::default()
    }
}

// ---------------------------------------------------------------------
// Shared value readers.
// ---------------------------------------------------------------------

fn str_at<'a>(value: &'a Value, path: &[&str]) -> Option<&'a str> {
    let mut current = value;
    for key in path {
        current = current.get(*key)?;
    }
    current.as_str()
}

fn positive_u64(value: &Value) -> Option<u64> {
    let number = value
        .as_u64()
        .or_else(|| value.as_f64().filter(|n| *n >= 0.0).map(|n| n as u64))?;
    (number > 0).then_some(number)
}

/// Show notes keep their text and markup; only invisibles go.
fn clean_notes(text: &str) -> Option<String> {
    let cleaned = strip_invisibles(text).trim().to_string();
    (!cleaned.is_empty()).then_some(cleaned)
}

/// A plausible BCP 47 language code, e.g. `en` or `pt-BR`.
fn clean_language(code: &str) -> Option<String> {
    let code = code.trim();
    let valid = !code.is_empty()
        && code.len() <= 16
        && code
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    valid.then(|| code.replace('_', "-"))
}

/// `releaseDate` objects: `isoString` plus an optional `precision`.
fn release_from_value(value: &Value) -> Option<Released> {
    let mut released = parse_release(str_at(value, &["isoString"])?)?;
    if let Some(precision) = str_at(value, &["precision"]) {
        released.precision = match precision.to_ascii_uppercase().as_str() {
            "DAY" | "MONTH" | "YEAR" => ReleasePrecision::Day,
            "HOUR" | "MINUTE" | "SECOND" | "MILLISECOND" => {
                ReleasePrecision::Time
            }
            _ => released.precision,
        };
    }
    Some(released)
}

/// An RFC 3339 instant or a bare `YYYY-MM-DD` date. Without a stated
/// precision, a bare date or an instant at exactly midnight UTC is a
/// [`ReleasePrecision::Day`] (Spotify stores dates that way), anything
/// else a [`ReleasePrecision::Time`].
fn parse_release(raw: &str) -> Option<Released> {
    let raw = raw.trim();
    if let Ok(instant) = DateTime::parse_from_rfc3339(raw) {
        let instant = instant.with_timezone(&Utc);
        let midnight = instant.time() == chrono::NaiveTime::MIN;
        return Some(Released {
            instant,
            precision: if midnight {
                ReleasePrecision::Day
            } else {
                ReleasePrecision::Time
            },
        });
    }
    let date = NaiveDate::parse_from_str(raw, "%Y-%m-%d").ok()?;
    Some(Released {
        instant: date.and_time(chrono::NaiveTime::MIN).and_utc(),
        precision: ReleasePrecision::Day,
    })
}

/// Read `[{url, width|maxWidth, height|maxHeight}]` image sources.
fn art_from_sources(sources: &Value) -> Option<String> {
    let candidates = sources
        .as_array()?
        .iter()
        .filter_map(|source| {
            let url = str_at(source, &["url"])?.to_string();
            let dimension = |keys: &[&str]| {
                keys.iter().find_map(|key| {
                    source
                        .get(*key)
                        .and_then(Value::as_u64)
                        .and_then(|px| u32::try_from(px).ok())
                })
            };
            let width = dimension(&["width", "maxWidth"]);
            let height = dimension(&["height", "maxHeight"]);
            let size = match (width, height) {
                (Some(w), Some(h)) => Some(w.max(h)),
                (one, other) => one.or(other),
            };
            Some(Art { url, size })
        })
        .collect();
    choose_art(candidates)
}

/// The largest http(s) image at most [`COVER_MAX_PX`]; else an image of
/// unknown size; else the smallest oversized one.
fn choose_art(candidates: Vec<Art>) -> Option<String> {
    let usable: Vec<Art> = candidates
        .into_iter()
        .filter(|art| {
            url::Url::parse(&art.url)
                .is_ok_and(|url| matches!(url.scheme(), "http" | "https"))
        })
        .collect();
    let fitting = usable
        .iter()
        .filter(|art| art.size.is_some_and(|px| px > 0 && px <= COVER_MAX_PX))
        .max_by_key(|art| art.size);
    let unknown_size = usable.iter().find(|art| art.size.is_none());
    let oversized = usable
        .iter()
        .filter(|art| art.size.is_some_and(|px| px > COVER_MAX_PX))
        .min_by_key(|art| art.size);
    fitting
        .or(unknown_size)
        .or(oversized)
        .map(|art| art.url.clone())
}

#[cfg(test)]
mod tests {
    use super::super::test_support::FakeSpotify;
    use super::*;

    const ID: &str = "4UgVFWty2I8hGAgSvT7pjC";
    const PAGE: &str =
        include_str!("../../../../tests/fixtures/spotify/episode_page.html");
    const EMBED: &str =
        include_str!("../../../../tests/fixtures/spotify/episode_embed.html");
    const OEMBED: &str =
        include_str!("../../../../tests/fixtures/spotify/episode_oembed.json");
    const PAGE_NO_STATE: &str = include_str!(
        "../../../../tests/fixtures/spotify/drift_page_no_initial_state.html"
    );
    const PAGE_OTHER_ENTITY: &str = include_str!(
        "../../../../tests/fixtures/spotify/drift_page_other_entity.html"
    );
    const PAGE_JSON_LD_ARRAY: &str = include_str!(
        "../../../../tests/fixtures/spotify/drift_page_json_ld_array.html"
    );
    const PAGE_SHELL: &str = include_str!(
        "../../../../tests/fixtures/spotify/drift_page_shell.html"
    );

    fn episode() -> SpotifyEpisode {
        SpotifyEpisode {
            id: ID.to_string(),
            shared_at: None,
        }
    }

    fn eastern() -> Tz {
        "America/New_York".parse().expect("zone")
    }

    #[test]
    fn page_yields_every_field_from_initial_state() {
        let fields = parse_page(PAGE, ID);
        assert_eq!(
            fields.title.as_deref(),
            Some("Software factories and code reviews with Harjot Gill")
        );
        assert_eq!(fields.show.as_deref(), Some("The Pragmatic Engineer"));
        assert_eq!(
            fields.show_uri.as_deref(),
            Some("spotify:show:2Bho9xCbOQMWMJ7UKmqCzD")
        );
        assert_eq!(fields.publisher.as_deref(), Some("Gergely Orosz"));
        assert_eq!(fields.duration_ms, Some(5_197_032));
        assert_eq!(fields.language.as_deref(), Some("en"));
        let released = fields.released.expect("release");
        assert_eq!(released.precision, ReleasePrecision::Time);
        assert_eq!(released.instant.to_rfc3339(), "2026-11-18T18:08:00+00:00");
        assert_eq!(
            fields.episode_art.as_deref(),
            Some(
                "https://i.scdn.co/image/ab6765630000ba8a2927546cb7ca0ae0de915103"
            )
        );
        assert_eq!(
            fields.show_art.as_deref(),
            Some(
                "https://i.scdn.co/image/ab6765630000ba8af17cbdd89e0fcee0d90bd463"
            )
        );
        let html = fields.notes_html.expect("notes html");
        assert!(html.starts_with("<p>Brought to You By:</p>"), "{html}");
        assert!(html.contains("<p>1:24:30 Book recommendation</p>"));
        assert!(fields.notes_text.is_some());
    }

    #[test]
    fn notes_lose_every_invisible_character() {
        assert!(PAGE.len() > 1000);
        let fields = parse_page(PAGE, ID);
        for text in [
            fields.title.unwrap_or_default(),
            fields.show.unwrap_or_default(),
            fields.notes_html.unwrap_or_default(),
            fields.notes_text.unwrap_or_default(),
        ] {
            assert!(
                !text.contains(['\u{2060}', '\u{200B}', '\u{FEFF}']),
                "invisible characters survived: {text:?}"
            );
        }
    }

    #[test]
    fn json_ld_and_meta_fill_in_without_initial_state() {
        let fields = parse_page(PAGE_NO_STATE, ID);
        assert_eq!(
            fields.title.as_deref(),
            Some("Software factories and code reviews with Harjot Gill")
        );
        // The JSON-LD prefix yields the show and is cut from the notes.
        assert_eq!(fields.show.as_deref(), Some("The Pragmatic Engineer"));
        let text = fields.notes_text.expect("notes text");
        assert!(text.starts_with("Brought to You By:"), "{text}");
        // JSON-LD's day-precision date wins over the meta instant.
        let released = fields.released.expect("release");
        assert_eq!(released.precision, ReleasePrecision::Day);
        assert_eq!(released.date_in(Some(eastern())).to_string(), "2026-11-18");
        // Meta tags fill what JSON-LD lacks.
        assert_eq!(fields.duration_ms, Some(5_197_000));
        assert_eq!(
            fields.episode_art.as_deref(),
            Some(
                "https://i.scdn.co/image/ab6765630000ba8a2927546cb7ca0ae0de915103"
            )
        );
        assert_eq!(fields.notes_html, None);
        assert_eq!(fields.publisher, None);
    }

    #[test]
    fn a_json_ld_array_member_is_found() {
        let fields = parse_page(PAGE_JSON_LD_ARRAY, ID);
        assert_eq!(
            fields.title.as_deref(),
            Some("Software factories and code reviews with Harjot Gill")
        );
        assert_eq!(fields.show.as_deref(), Some("The Pragmatic Engineer"));
    }

    #[test]
    fn sources_naming_another_entity_are_rejected() {
        assert_eq!(parse_page(PAGE_OTHER_ENTITY, ID), Fields::default());
        // The real page names its own id, not a case-folded one.
        assert_eq!(
            parse_page(PAGE, &ID.to_ascii_lowercase()),
            Fields::default()
        );
        assert_eq!(
            parse_embed(EMBED, "1AbCdEfGhIjKlMnOpQrStU"),
            Fields::default()
        );
        assert_eq!(
            parse_oembed(OEMBED, "1AbCdEfGhIjKlMnOpQrStU"),
            Fields::default()
        );
        // A shell page with no metadata, and garbage, yield nothing.
        assert_eq!(parse_page(PAGE_SHELL, ID), Fields::default());
        assert_eq!(parse_page("<<<not html", ID), Fields::default());
        assert_eq!(parse_embed("{]", ID), Fields::default());
        assert_eq!(parse_oembed("[1, 2]", ID), Fields::default());
    }

    #[test]
    fn initial_state_needs_the_episode_typename() {
        let mut state: Value = serde_json::json!({
            "entities": {"items": {format!("spotify:episode:{ID}"): {
                "__typename": "RestrictedContent",
                "name": "Nope",
            }}}
        });
        let encode = |state: &Value| {
            base64::engine::general_purpose::STANDARD
                .encode(serde_json::to_vec(state).expect("json"))
        };
        let page = |b64: String| {
            format!(
                r#"<html><body><script id="initialState" type="text/plain">{b64}</script></body></html>"#
            )
        };
        assert_eq!(parse_page(&page(encode(&state)), ID), Fields::default());
        state["entities"]["items"][format!("spotify:episode:{ID}")]
            ["__typename"] = Value::from("Episode");
        assert_eq!(
            parse_page(&page(encode(&state)), ID).title.as_deref(),
            Some("Nope")
        );
    }

    #[test]
    fn embed_and_oembed_yield_their_fields() {
        let embed = parse_embed(EMBED, ID);
        assert_eq!(
            embed.title.as_deref(),
            Some("Software factories and code reviews with Harjot Gill")
        );
        assert_eq!(embed.show.as_deref(), Some("The Pragmatic Engineer"));
        assert_eq!(embed.duration_ms, Some(5_197_032));
        assert_eq!(
            embed.show_uri.as_deref(),
            Some("spotify:show:2Bho9xCbOQMWMJ7UKmqCzD")
        );
        assert_eq!(
            embed.episode_art.as_deref(),
            Some("https://image-cdn-fa.spotifycdn.com/image/ab6765630000ba8a2927546cb7ca0ae0de915103")
        );
        assert!(embed.notes_html.is_none() && embed.notes_text.is_none());

        let oembed = parse_oembed(OEMBED, ID);
        assert_eq!(
            oembed.title.as_deref(),
            Some("Software factories and code reviews with Harjot Gill")
        );
        assert_eq!(
            oembed.episode_art.as_deref(),
            Some("https://image-cdn-fa.spotifycdn.com/image/ab67656300005f1f2927546cb7ca0ae0de915103")
        );
        assert_eq!(oembed.show, None);
    }

    #[test]
    fn release_dates_follow_precision_and_zone() {
        // A 02:00Z release is the previous local day in New York.
        let late = parse_release("2026-11-19T02:00:00Z").expect("instant");
        assert_eq!(late.precision, ReleasePrecision::Time);
        assert_eq!(late.date_in(Some(eastern())).to_string(), "2026-11-18");
        // DAY precision keeps the date as given.
        let day = release_from_value(&serde_json::json!({
            "isoString": "2026-11-19T02:00:00Z", "precision": "DAY"
        }))
        .expect("day");
        assert_eq!(day.precision, ReleasePrecision::Day);
        assert_eq!(day.date_in(Some(eastern())).to_string(), "2026-11-19");
        // A bare date and a midnight-UTC instant are dates.
        for raw in ["2026-11-19", "2026-11-19T00:00:00Z"] {
            let released = parse_release(raw).expect("date");
            assert_eq!(released.precision, ReleasePrecision::Day, "{raw}");
            assert_eq!(
                released.date_in(Some(eastern())).to_string(),
                "2026-11-19"
            );
        }
        assert_eq!(parse_release("Wednesday"), None);
    }

    #[test]
    fn art_prefers_the_largest_within_640_px() {
        let sources = serde_json::json!([
            {"url": "https://i.scdn.co/image/64", "width": 64, "height": 64},
            {"url": "https://i.scdn.co/image/1000", "width": 1000, "height": 1000},
            {"url": "https://i.scdn.co/image/640", "width": 640, "height": 640},
            {"url": "https://i.scdn.co/image/300", "width": 300, "height": 300},
        ]);
        assert_eq!(
            art_from_sources(&sources).as_deref(),
            Some("https://i.scdn.co/image/640")
        );
        let oversized = serde_json::json!([
            {"url": "https://i.scdn.co/image/2000", "maxWidth": 2000},
            {"url": "https://i.scdn.co/image/1000", "maxWidth": 1000},
            {"url": "javascript:alert(1)", "maxWidth": 300},
        ]);
        assert_eq!(
            art_from_sources(&oversized).as_deref(),
            Some("https://i.scdn.co/image/1000")
        );
        assert_eq!(art_from_sources(&serde_json::json!({"no": 1})), None);
    }

    #[test]
    fn the_ladder_stops_at_a_whole_page() {
        let fake = FakeSpotify::new();
        fake.serve("page", 200, "text/html", PAGE.as_bytes());
        fake.serve("embed", 200, "text/html", EMBED.as_bytes());
        let meta =
            fetch_episode(&episode(), &fake.scratch("whole"), None, None)
                .expect("metadata");
        assert_eq!(
            fake.requests(),
            vec![format!("https://open.spotify.com/episode/{ID}")]
        );
        assert_eq!(
            meta.canonical_url,
            format!("https://open.spotify.com/episode/{ID}")
        );
        assert_eq!(
            meta.title,
            "Software factories and code reviews with Harjot Gill"
        );
        assert_eq!(
            meta.cover_url.as_deref(),
            Some(
                "https://i.scdn.co/image/ab6765630000ba8a2927546cb7ca0ae0de915103"
            )
        );
        assert!(meta.warnings.is_empty(), "{:?}", meta.warnings);
        assert!(meta
            .sources
            .values()
            .all(|source| *source == MetadataSource::Page));
        assert_eq!(
            meta.source_of(EpisodeField::Title)
                .map(MetadataSource::as_str),
            Some("spotify_page")
        );
        assert_eq!(
            meta.source_of(EpisodeField::Title)
                .map(MetadataSource::label),
            Some("spotify page")
        );
    }

    #[test]
    fn published_dates_use_bob_day_zone() {
        let config = tempfile::NamedTempFile::new().expect("config");
        fs::write(config.path(), "timezone: America/New_York\n")
            .expect("write config");
        let _guard = crate::native::env::TestEnvGuard::set(&[(
            "BOB_CONFIG_FILE",
            Some(config.path().as_os_str()),
        )]);
        let fake = FakeSpotify::new();
        let scratch = fake.scratch("zone");
        let body = scratch.join("page.html");
        // A 02:00Z release lands on the previous New York day.
        fs::write(
            &body,
            PAGE.replace("2026-11-18T18:08:00Z", "2026-11-19T02:00:00Z"),
        )
        .expect("write page");
        let meta = fetch_episode(&episode(), &scratch, None, Some(&body))
            .expect("metadata");
        assert_eq!(
            meta.published_date(),
            NaiveDate::from_ymd_opt(2026, 11, 18)
        );
    }

    #[test]
    fn a_prefetched_page_is_not_fetched_again() {
        let fake = FakeSpotify::new();
        let scratch = fake.scratch("prefetched");
        let body = scratch.join("short-link.body");
        fs::write(&body, PAGE).expect("write prefetched");
        let meta = fetch_episode(&episode(), &scratch, None, Some(&body))
            .expect("metadata");
        assert!(fake.requests().is_empty(), "{:?}", fake.requests());
        assert_eq!(meta.show.as_deref(), Some("The Pragmatic Engineer"));
    }

    #[test]
    fn a_page_naming_another_entity_falls_to_the_embed() {
        let fake = FakeSpotify::new();
        fake.serve("page", 200, "text/html", PAGE_OTHER_ENTITY.as_bytes());
        fake.serve("embed", 200, "text/html", EMBED.as_bytes());
        fake.serve("oembed", 200, "application/json", OEMBED.as_bytes());
        let meta =
            fetch_episode(&episode(), &fake.scratch("other"), None, None)
                .expect("metadata");
        let requests = fake.requests();
        assert_eq!(requests.len(), 2, "{requests:?}");
        assert!(requests[1].contains("/embed/episode/"), "{requests:?}");
        assert_eq!(
            meta.title,
            "Software factories and code reviews with Harjot Gill"
        );
        assert_eq!(
            meta.source_of(EpisodeField::Title),
            Some(MetadataSource::Embed)
        );
        assert_eq!(meta.publisher, None);
        assert_eq!(
            meta.warnings,
            vec![
                "Spotify episode metadata fell back to the embed player; missing show notes"
                    .to_string()
            ]
        );
    }

    #[test]
    fn json_ld_notes_skip_the_embed_but_a_missing_note_tries_it() {
        // JSON-LD + meta hold title and notes: the page is enough.
        let fake = FakeSpotify::new();
        fake.serve("page", 200, "text/html", PAGE_NO_STATE.as_bytes());
        let meta = fetch_episode(&episode(), &fake.scratch("ld"), None, None)
            .expect("metadata");
        assert_eq!(fake.requests().len(), 1);
        assert!(meta.warnings.is_empty(), "{:?}", meta.warnings);
        assert_eq!(meta.source_of(EpisodeField::NotesHtml), None);
        assert_eq!(
            meta.source_of(EpisodeField::NotesText),
            Some(MetadataSource::Page)
        );
    }

    #[test]
    fn oembed_alone_still_yields_a_title() {
        let fake = FakeSpotify::new();
        fake.serve("page", 200, "text/html", PAGE_SHELL.as_bytes());
        fake.serve("embed", 200, "text/html", b"<html>nothing</html>");
        fake.serve("oembed", 200, "application/json", OEMBED.as_bytes());
        let meta =
            fetch_episode(&episode(), &fake.scratch("oembed"), None, None)
                .expect("metadata");
        let requests = fake.requests();
        assert_eq!(requests.len(), 3, "{requests:?}");
        assert_eq!(
            requests[2],
            format!(
                "https://open.spotify.com/oembed?url=https%3A%2F%2Fopen.spotify.com%2Fepisode%2F{ID}"
            )
        );
        assert_eq!(
            meta.title,
            "Software factories and code reviews with Harjot Gill"
        );
        assert_eq!(
            meta.source_of(EpisodeField::Cover),
            Some(MetadataSource::Oembed)
        );
        assert_eq!(
            meta.warnings,
            vec![
                "Spotify episode metadata fell back to oEmbed; missing show notes, show name, release date, duration"
                    .to_string()
            ]
        );
    }

    #[test]
    fn no_title_fails_closed() {
        let fake = FakeSpotify::new();
        fake.serve("page", 200, "text/html", PAGE_SHELL.as_bytes());
        fake.serve("embed", 404, "text/html", b"");
        fake.serve("oembed", 404, "application/json", b"");
        let error =
            fetch_episode(&episode(), &fake.scratch("none"), None, None)
                .expect_err("no title");
        assert_eq!(error, EpisodeError::NoMetadata { id: ID.to_string() });
        assert_eq!(
            error.render(),
            format!(
                "Spotify returned no episode metadata for {ID}\nhint: retry later; if it persists, Spotify changed its page and bob needs an update"
            )
        );
        assert!(!error.retryable());
    }

    #[test]
    fn a_404_page_is_decisive() {
        let fake = FakeSpotify::new();
        fake.serve("page", 404, "text/html", b"not found");
        fake.serve("embed", 200, "text/html", EMBED.as_bytes());
        let error = fetch_episode(&episode(), &fake.scratch("404"), None, None)
            .expect_err("404");
        assert_eq!(
            error.message(),
            format!("Spotify has no public episode {ID} (HTTP 404)")
        );
        assert_eq!(error.status(), Some(404));
        assert!(!error.retryable());
        assert_eq!(fake.requests().len(), 1);
    }

    #[test]
    fn rate_limits_server_errors_and_transport_failures_are_retryable() {
        for status in [429_u16, 503] {
            let fake = FakeSpotify::new();
            fake.serve("page", status, "text/html", b"busy");
            fake.serve("embed", 200, "text/html", EMBED.as_bytes());
            let error =
                fetch_episode(&episode(), &fake.scratch("busy"), None, None)
                    .expect_err("transient");
            assert_eq!(error.status(), Some(status));
            assert!(error.retryable(), "{status}");
            assert_eq!(error.hint().as_deref(), Some("retry later"));
            assert_eq!(fake.requests().len(), 1);
        }

        let fake = FakeSpotify::new();
        fake.fail("page", 28);
        let error =
            fetch_episode(&episode(), &fake.scratch("timeout"), None, None)
                .expect_err("timeout");
        assert!(error.retryable());
        assert!(error.message().contains("timed out"), "{}", error.message());

        // A transient later tier surfaces only when no title came.
        let fake = FakeSpotify::new();
        fake.serve("page", 200, "text/html", PAGE_SHELL.as_bytes());
        fake.serve("embed", 503, "text/html", b"");
        fake.serve("oembed", 404, "application/json", b"");
        let error =
            fetch_episode(&episode(), &fake.scratch("late"), None, None)
                .expect_err("late transient");
        assert_eq!(error.status(), Some(503));
        assert!(error.retryable());
    }

    #[test]
    fn a_page_over_the_cap_is_refused() {
        let fake = FakeSpotify::new();
        let big = vec![b'x'; (PAGE_MAX_BYTES + 1) as usize];
        fake.serve("page", 200, "text/html", &big);
        let error = fetch_episode(&episode(), &fake.scratch("big"), None, None)
            .expect_err("over cap");
        assert!(
            error.message().contains("download larger than 5 MiB"),
            "{}",
            error.message()
        );
    }
}
