//! Spotify link identity for references, capture, and Keep.
//!
//! [`SpotifyLink::parse`] is pure: it recognizes `open.spotify.com` and
//! `play.spotify.com` web links (with optional `/intl-<lang>` and
//! `/embed` prefixes), `spotify:<kind>:<id>` URIs, and the short-link
//! hosts, and returns `None` for everything else, which keeps today's
//! routes. Entity ids keep their case (the lowercased id 404s), and no
//! query parameter or fragment is ever part of identity: every spelling
//! of one entity shares `https://open.spotify.com/<kind>/<id>` as its
//! cleaned URL and dedupe key. `?t=<seconds>` is kept apart as the
//! shared moment.
//!
//! Only [`resolve_short_link`] touches the network.

use std::path::{Path, PathBuf};

use super::fetch::{fetch_url, FetchError};

/// The canonical web origin for every Spotify entity.
const CANONICAL_ORIGIN: &str = "https://open.spotify.com";

/// Web hosts whose paths name Spotify entities (matched exactly).
const WEB_HOSTS: &[&str] = &["open.spotify.com", "play.spotify.com"];

/// Short-link hosts resolved by following redirects (matched exactly).
const SHORT_HOSTS: &[&str] = &["spotify.link", "spotify.app.link", "spoti.fi"];

/// Entity ids are exactly this many `[0-9A-Za-z]` characters.
const ENTITY_ID_LEN: usize = 22;

/// Per-hop time cap for short-link resolution.
const SHORT_LINK_MAX_TIME_SECS: u64 = 30;

/// The kind of thing a Spotify link names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SpotifyKind {
    Episode,
    Show,
    Track,
    Album,
    Playlist,
    Artist,
    Audiobook,
    Chapter,
    User,
    /// A well-formed `<word>/<id>` entity of a kind bob does not know.
    Other,
}

impl SpotifyKind {
    fn from_word(word: &str) -> Option<Self> {
        if word.is_empty()
            || !word.chars().all(|c| c.is_ascii_alphabetic() || c == '-')
        {
            return None;
        }
        Some(match word.to_ascii_lowercase().as_str() {
            "episode" => Self::Episode,
            "show" => Self::Show,
            "track" => Self::Track,
            "album" => Self::Album,
            "playlist" => Self::Playlist,
            "artist" => Self::Artist,
            "audiobook" => Self::Audiobook,
            "chapter" => Self::Chapter,
            "user" => Self::User,
            _ => Self::Other,
        })
    }

    /// The path word of the canonical URL; `None` for [`Self::Other`],
    /// whose word is not kept.
    fn path_word(self) -> Option<&'static str> {
        Some(match self {
            Self::Episode => "episode",
            Self::Show => "show",
            Self::Track => "track",
            Self::Album => "album",
            Self::Playlist => "playlist",
            Self::Artist => "artist",
            Self::Audiobook => "audiobook",
            Self::Chapter => "chapter",
            Self::User => "user",
            Self::Other => return None,
        })
    }
}

/// A parsed Spotify link.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SpotifyLink {
    /// A link naming one entity. The id keeps its case.
    Entity {
        kind: SpotifyKind,
        id: String,
        shared_at: Option<u32>,
    },
    /// A short link (`spotify.link`, `spotify.app.link`, `spoti.fi`)
    /// that only names an entity after its redirects are followed.
    Short { url: String },
    /// A Spotify host or URI whose path names no entity.
    Unsupported { url: String },
}

impl SpotifyLink {
    /// Parse `raw` as a Spotify link. `None` means it is not a Spotify
    /// link at all and stays on today's routes.
    pub(crate) fn parse(raw: &str) -> Option<Self> {
        let raw = raw.trim();
        if raw
            .get(..8)
            .is_some_and(|scheme| scheme.eq_ignore_ascii_case("spotify:"))
        {
            return Some(parse_uri(raw));
        }
        let url = url::Url::parse(raw).ok()?;
        if !matches!(url.scheme(), "http" | "https") {
            return None;
        }
        let host = url.host_str()?.to_ascii_lowercase();
        if SHORT_HOSTS.contains(&host.as_str()) {
            return Some(Self::Short {
                url: raw.to_string(),
            });
        }
        if !WEB_HOSTS.contains(&host.as_str()) {
            return None;
        }
        Some(match parse_web_path(url.path()) {
            Some((kind, id)) => Self::Entity {
                kind,
                id,
                shared_at: shared_at(&url),
            },
            None => Self::Unsupported {
                url: raw.to_string(),
            },
        })
    }

    /// The episode this link names, if it names one.
    pub(crate) fn episode(&self) -> Option<SpotifyEpisode> {
        match self {
            Self::Entity {
                kind: SpotifyKind::Episode,
                id,
                shared_at,
            } => Some(SpotifyEpisode {
                id: id.clone(),
                shared_at: *shared_at,
            }),
            _ => None,
        }
    }

    /// `https://open.spotify.com/<kind>/<id>` for an entity of a known
    /// kind; `None` for short links, unsupported paths, and
    /// [`SpotifyKind::Other`].
    pub(crate) fn canonical_url(&self) -> Option<String> {
        match self {
            Self::Entity { kind, id, .. } => {
                let word = kind.path_word()?;
                Some(format!("{CANONICAL_ORIGIN}/{word}/{id}"))
            }
            _ => None,
        }
    }

    /// Whether capture and Keep may queue this link as a reference job:
    /// episodes, and short links that the worker resolves. Every other
    /// Spotify entity or path stays an ordinary item.
    pub(crate) fn is_routable(&self) -> bool {
        match self {
            Self::Entity { kind, .. } => *kind == SpotifyKind::Episode,
            Self::Short { .. } => true,
            Self::Unsupported { .. } => false,
        }
    }

    /// The teaching refusal for anything that is not one episode, with
    /// `url` as the link to name. `None` for episodes and short links
    /// (a short link is resolved first, then refused by what it names).
    pub(crate) fn refusal(&self, url: &str) -> Option<SpotifyRefusal> {
        match self {
            Self::Entity {
                kind: SpotifyKind::Episode,
                ..
            }
            | Self::Short { .. } => None,
            Self::Entity { kind, .. } => Some(kind_refusal(*kind, url)),
            Self::Unsupported { .. } => {
                Some(kind_refusal(SpotifyKind::Other, url))
            }
        }
    }
}

/// One Spotify episode: its case-preserved id plus the shared moment
/// (`?t=<seconds>`) of the link it came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SpotifyEpisode {
    pub(crate) id: String,
    pub(crate) shared_at: Option<u32>,
}

impl SpotifyEpisode {
    /// `https://open.spotify.com/episode/<id>`.
    pub(crate) fn canonical_url(&self) -> String {
        format!("{CANONICAL_ORIGIN}/episode/{}", self.id)
    }

    /// The dedupe key: the canonical URL.
    pub(crate) fn dedupe_key(&self) -> String {
        self.canonical_url()
    }

    /// Capture/Keep `%l` on an episode: the job is planned without
    /// listen, and this line is the item's `listen_hint`.
    pub(crate) fn listen_ignored_hint(url: &str) -> String {
        format!(
            "Spotify episodes are already audio · %l ignored · to bind your own audio run: bob ref create {url} --audio FILE"
        )
    }

    /// The canonical URL opened at `secs` seconds (`?t=<secs>`).
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn at(&self, secs: u32) -> String {
        format!("{}?t={secs}", self.canonical_url())
    }
}

/// A refusal's `error:` and `hint:` text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SpotifyRefusal {
    pub(crate) message: String,
    pub(crate) hint: &'static str,
}

impl SpotifyRefusal {
    /// The `message\nhint: hint` body the error renderers expect.
    pub(crate) fn render(&self) -> String {
        format!("{}\nhint: {}", self.message, self.hint)
    }
}

fn kind_refusal(kind: SpotifyKind, url: &str) -> SpotifyRefusal {
    let (message, hint) = match kind {
        SpotifyKind::Show => (
            format!("Spotify show links are not references: {url}"),
            "capture one episode (⋯ → Share → Copy link); follow the show as a record in your podcasts album (docs/records.md)",
        ),
        SpotifyKind::Track
        | SpotifyKind::Album
        | SpotifyKind::Playlist
        | SpotifyKind::Artist => (
            format!("Spotify music links are not references: {url}"),
            "bob files Spotify podcast episodes only",
        ),
        SpotifyKind::Audiobook | SpotifyKind::Chapter => (
            format!("Spotify audiobooks are not references: {url}"),
            "bob files Spotify podcast episodes only",
        ),
        SpotifyKind::Episode | SpotifyKind::User | SpotifyKind::Other => (
            format!("not a Spotify episode link: {url}"),
            "share the episode link (open.spotify.com/episode/…)",
        ),
    };
    SpotifyRefusal { message, hint }
}

/// `--listen` refusal for any Spotify TARGET.
pub(crate) fn listen_refusal() -> SpotifyRefusal {
    SpotifyRefusal {
        message: "--listen does not apply to Spotify episodes: the episode is already audio".to_string(),
        hint: "drop -L; pass --audio FILE to bind audio you already have",
    }
}

/// `--html` refusal for any Spotify TARGET.
pub(crate) fn html_refusal() -> SpotifyRefusal {
    SpotifyRefusal {
        message: "--html does not apply to Spotify links: bob reads episodes from Spotify's public episode metadata".to_string(),
        hint: "drop -H",
    }
}

/// Refusal for a Spotify link among several create TARGETs.
pub(crate) fn collection_refusal(url: &str) -> SpotifyRefusal {
    SpotifyRefusal {
        message: format!("Spotify episodes are captured one at a time: {url}"),
        hint: "run bob ref create once per episode",
    }
}

/// The canonical URL for any Spotify entity spelling (web link or
/// `spotify:` URI), or `None` when `raw` names no entity of a known kind.
pub(crate) fn canonical_entity_url(raw: &str) -> Option<String> {
    SpotifyLink::parse(raw)?.canonical_url()
}

/// `spotify:<kind>:<id>`: the scheme and kind are case-insensitive, the
/// id keeps its case. `spotify:user:<name>[:…]` is a user. Anything else
/// under the scheme is unsupported.
fn parse_uri(raw: &str) -> SpotifyLink {
    let unsupported = || SpotifyLink::Unsupported {
        url: raw.to_string(),
    };
    let parts: Vec<&str> = raw[8..].split(':').collect();
    let Some(kind) =
        parts.first().and_then(|word| SpotifyKind::from_word(word))
    else {
        return unsupported();
    };
    let Some(id) = parts.get(1).filter(|id| !id.is_empty()) else {
        return unsupported();
    };
    if kind == SpotifyKind::User {
        return SpotifyLink::Entity {
            kind,
            id: (*id).to_string(),
            shared_at: None,
        };
    }
    if parts.len() != 2 || !is_entity_id(id) {
        return unsupported();
    }
    SpotifyLink::Entity {
        kind,
        id: (*id).to_string(),
        shared_at: None,
    }
}

/// `[/intl-<lang>[-<REGION>]][/embed|/embed-podcast]/<kind>/<id>[/]`,
/// or `…/user/<anything>`. Returns `None` for any other path.
fn parse_web_path(path: &str) -> Option<(SpotifyKind, String)> {
    let mut segments: Vec<&str> = path.strip_prefix('/')?.split('/').collect();
    if segments.last() == Some(&"") {
        segments.pop();
    }
    if segments.iter().any(|segment| segment.is_empty()) {
        return None;
    }
    let mut rest = segments.as_slice();
    if rest.first().is_some_and(|segment| is_intl_segment(segment)) {
        rest = &rest[1..];
    }
    if rest.first().is_some_and(|segment| {
        segment.eq_ignore_ascii_case("embed")
            || segment.eq_ignore_ascii_case("embed-podcast")
    }) {
        rest = &rest[1..];
    }
    let (word, tail) = rest.split_first()?;
    let kind = SpotifyKind::from_word(word)?;
    let id = tail.first()?;
    if kind == SpotifyKind::User {
        return Some((kind, (*id).to_string()));
    }
    (tail.len() == 1 && is_entity_id(id)).then(|| (kind, (*id).to_string()))
}

/// `intl-<lang>` or `intl-<lang>-<REGION>` (case-insensitive), e.g.
/// `intl-de` or `intl-pt-BR`.
fn is_intl_segment(segment: &str) -> bool {
    let Some(rest) = segment
        .get(..5)
        .filter(|prefix| prefix.eq_ignore_ascii_case("intl-"))
        .map(|_| &segment[5..])
    else {
        return false;
    };
    let mut parts = rest.split('-');
    let lang_ok = parts.next().is_some_and(|lang| {
        (2..=3).contains(&lang.len())
            && lang.chars().all(|c| c.is_ascii_alphabetic())
    });
    let region_ok = match parts.next() {
        None => true,
        Some(region) => {
            (2..=4).contains(&region.len())
                && region.chars().all(|c| c.is_ascii_alphanumeric())
        }
    };
    lang_ok && region_ok && parts.next().is_none()
}

/// Exactly 22 ASCII alphanumeric characters.
fn is_entity_id(id: &str) -> bool {
    id.len() == ENTITY_ID_LEN && id.bytes().all(|b| b.is_ascii_alphanumeric())
}

/// The first `t` query value when it is a non-negative integer number of
/// seconds; any other value is ignored.
fn shared_at(url: &url::Url) -> Option<u32> {
    let (_, value) = url.query_pairs().find(|(name, _)| name == "t")?;
    if value.is_empty() || !value.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    value.parse().ok()
}

/// A short link resolved to the entity link it redirects to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ResolvedShortLink {
    /// The parsed final link (always an [`SpotifyLink::Entity`]).
    pub(super) link: SpotifyLink,
    /// The final URL after redirects, as served (query intact, so its
    /// `?t=` survives).
    pub(super) final_url: String,
    /// The final HTTP status.
    pub(super) status: u16,
    /// The final `Content-Type`.
    pub(super) content_type: String,
    /// The downloaded final body, reusable as the first metadata source.
    pub(super) body: PathBuf,
}

/// Why a short link did not resolve.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum ShortLinkError {
    /// The fetch itself failed (network, timeout, a refused redirect).
    Fetch(FetchError),
    /// The redirects ended somewhere that names no Spotify entity.
    NotEntity(SpotifyRefusal),
}

impl ShortLinkError {
    /// The `message\nhint: hint` body.
    pub(super) fn render(&self) -> String {
        match self {
            Self::Fetch(error) => match error.hint() {
                Some(hint) => format!("{}\nhint: {hint}", error.message()),
                None => error.message().to_string(),
            },
            Self::NotEntity(refusal) => refusal.render(),
        }
    }
}

/// Follow `url`'s redirects (every hop validated by [`fetch_url`]) into
/// `scratch` and parse the final URL, which must name an entity.
pub(super) fn resolve_short_link(
    url: &str,
    scratch: &Path,
    progress: Option<&dyn Fn(&str)>,
) -> Result<ResolvedShortLink, ShortLinkError> {
    let dest = scratch.join("spotify-short-link.body");
    let fetch = fetch_url(url, &dest, SHORT_LINK_MAX_TIME_SECS, progress)
        .map_err(ShortLinkError::Fetch)?;
    match SpotifyLink::parse(&fetch.final_url) {
        Some(link @ SpotifyLink::Entity { .. }) => Ok(ResolvedShortLink {
            link,
            final_url: fetch.final_url,
            status: fetch.status,
            content_type: fetch.content_type,
            body: fetch.path,
        }),
        _ => Err(ShortLinkError::NotEntity(SpotifyRefusal {
            message: format!(
                "could not resolve Spotify short link {url} to an open.spotify.com link"
            ),
            hint: "open it in Spotify and share the open.spotify.com link instead",
        })),
    }
}

/// Normalize a single `bob ref create` TARGET once, before preflight and
/// every syntactic URL step, so later steps see an ordinary
/// `open.spotify.com` URL. Returns `Ok(None)` when `raw` is not a
/// Spotify link.
///
/// `-L` and `-H` refuse for any Spotify link, then anything that is not
/// one episode refuses with its teaching hint (exit 2). A `spotify:` URI
/// becomes the canonical https URL; a short link is resolved, re-parsed,
/// and refused unless it names an episode (its final URL, `?t=` intact,
/// becomes the TARGET); an episode web link is kept as typed (cleaning
/// canonicalizes it).
pub(super) fn prepare_create_target(
    raw: &str,
    listen: bool,
    html: bool,
    progress: Option<&dyn Fn(&str)>,
) -> super::Result<Option<String>> {
    let raw = raw.trim();
    let Some(link) = SpotifyLink::parse(raw) else {
        return Ok(None);
    };
    let usage = |refusal: SpotifyRefusal| {
        super::CommandError::new(refusal.render()).with_exit_code(2)
    };
    if listen {
        return Err(usage(listen_refusal()));
    }
    if html {
        return Err(usage(html_refusal()));
    }
    if let Some(refusal) = link.refusal(raw) {
        return Err(usage(refusal));
    }
    match &link {
        SpotifyLink::Short { url } => {
            let scratch = super::workdir::ScratchDir::create("spotify-link")?;
            let resolved = resolve_short_link(url, scratch.path(), progress)
                .map_err(|error| super::CommandError::new(error.render()))?;
            let named = resolved
                .link
                .canonical_url()
                .unwrap_or_else(|| resolved.final_url.clone());
            if let Some(refusal) = resolved.link.refusal(&named) {
                return Err(usage(refusal));
            }
            Ok(Some(resolved.final_url))
        }
        SpotifyLink::Entity { .. } if !looks_like_web_url(raw) => {
            Ok(link.canonical_url())
        }
        _ => Ok(Some(raw.to_string())),
    }
}

fn looks_like_web_url(raw: &str) -> bool {
    let lower = raw.to_ascii_lowercase();
    lower.starts_with("http://") || lower.starts_with("https://")
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;

    const ID: &str = "4UgVFWty2I8hGAgSvT7pjC";

    fn entity(kind: SpotifyKind, shared_at: Option<u32>) -> SpotifyLink {
        SpotifyLink::Entity {
            kind,
            id: ID.to_string(),
            shared_at,
        }
    }

    #[test]
    fn every_kind_parses_from_web_links() {
        for (word, kind) in [
            ("episode", SpotifyKind::Episode),
            ("show", SpotifyKind::Show),
            ("track", SpotifyKind::Track),
            ("album", SpotifyKind::Album),
            ("playlist", SpotifyKind::Playlist),
            ("artist", SpotifyKind::Artist),
            ("audiobook", SpotifyKind::Audiobook),
            ("chapter", SpotifyKind::Chapter),
            ("concert", SpotifyKind::Other),
        ] {
            let raw = format!("https://open.spotify.com/{word}/{ID}");
            assert_eq!(
                SpotifyLink::parse(&raw),
                Some(entity(kind, None)),
                "{raw}"
            );
        }
    }

    #[test]
    fn prefixes_hosts_and_trailing_slash_parse() {
        for raw in [
            format!("https://open.spotify.com/intl-de/episode/{ID}"),
            format!("https://open.spotify.com/intl-pt-BR/episode/{ID}"),
            format!("https://open.spotify.com/embed/episode/{ID}"),
            format!("https://open.spotify.com/embed-podcast/episode/{ID}"),
            format!("https://open.spotify.com/intl-ja/embed/episode/{ID}"),
            format!("https://play.spotify.com/episode/{ID}"),
            format!("http://OPEN.Spotify.COM/episode/{ID}"),
            format!("https://open.spotify.com/episode/{ID}/"),
            format!("https://open.spotify.com/episode/{ID}#t=1"),
            format!("spotify:episode:{ID}"),
            format!("SPOTIFY:Episode:{ID}"),
        ] {
            assert_eq!(
                SpotifyLink::parse(&raw),
                Some(entity(SpotifyKind::Episode, None)),
                "{raw}"
            );
        }
    }

    #[test]
    fn shared_moment_comes_only_from_integer_t() {
        let raw = format!(
            "https://open.spotify.com/episode/{ID}?si=abc123&sci=def&t=754"
        );
        let link = SpotifyLink::parse(&raw).expect("episode parses");
        assert_eq!(link, entity(SpotifyKind::Episode, Some(754)));
        assert_eq!(
            link.canonical_url().as_deref(),
            Some(format!("https://open.spotify.com/episode/{ID}").as_str())
        );
        for ignored in ["1m30s", "-5", "", "12.5", "99999999999"] {
            let raw =
                format!("https://open.spotify.com/episode/{ID}?t={ignored}");
            assert_eq!(
                SpotifyLink::parse(&raw),
                Some(entity(SpotifyKind::Episode, None)),
                "{raw}"
            );
        }
        let episode = link.episode().expect("episode");
        assert_eq!(episode.shared_at, Some(754));
        assert_eq!(
            episode.at(90),
            format!("https://open.spotify.com/episode/{ID}?t=90")
        );
        assert_eq!(episode.dedupe_key(), episode.canonical_url());
    }

    #[test]
    fn ids_keep_case_and_need_exactly_22_characters() {
        let link = SpotifyLink::parse(&format!(
            "https://open.spotify.com/episode/{ID}"
        ))
        .expect("episode");
        assert_eq!(link.episode().expect("episode").id, ID);
        for bad in [&ID[..21], &format!("{ID}x")[..], "4UgVFWty2I8hGAgSvT7p-C"]
        {
            let raw = format!("https://open.spotify.com/episode/{bad}");
            assert!(
                matches!(
                    SpotifyLink::parse(&raw),
                    Some(SpotifyLink::Unsupported { .. })
                ),
                "{raw}"
            );
            assert!(
                matches!(
                    SpotifyLink::parse(&format!("spotify:episode:{bad}")),
                    Some(SpotifyLink::Unsupported { .. })
                ),
                "uri {bad}"
            );
        }
    }

    #[test]
    fn users_and_unsupported_paths() {
        assert_eq!(
            SpotifyLink::parse("https://open.spotify.com/user/someone"),
            Some(SpotifyLink::Entity {
                kind: SpotifyKind::User,
                id: "someone".to_string(),
                shared_at: None,
            })
        );
        assert!(matches!(
            SpotifyLink::parse("spotify:user:someone:playlist:x"),
            Some(SpotifyLink::Entity {
                kind: SpotifyKind::User,
                ..
            })
        ));
        for raw in [
            "https://open.spotify.com/",
            "https://open.spotify.com/search/podcasts",
            "https://open.spotify.com/intl-de/",
            &format!("https://open.spotify.com/episode/{ID}/extra"),
            &format!("https://open.spotify.com//episode/{ID}"),
            "spotify:",
            "spotify:episode",
        ] {
            assert!(
                matches!(
                    SpotifyLink::parse(raw),
                    Some(SpotifyLink::Unsupported { .. })
                ),
                "{raw}"
            );
        }
    }

    #[test]
    fn short_links_and_non_spotify_hosts() {
        for raw in [
            "https://spotify.link/AbC123",
            "https://spotify.app.link/AbC123?_p=x",
            "http://spoti.fi/3xyz",
        ] {
            assert_eq!(
                SpotifyLink::parse(raw),
                Some(SpotifyLink::Short {
                    url: raw.to_string()
                }),
                "{raw}"
            );
        }
        for raw in [
            format!("https://open.spotify.com.evil.example/episode/{ID}"),
            format!("https://evilspotify.com/episode/{ID}"),
            format!("https://atspotify.com/episode/{ID}"),
            format!("https://newsroom.spotify.com/episode/{ID}"),
            format!("https://podcasters.spotify.com/episode/{ID}"),
            format!("https://www.spotify.com/episode/{ID}"),
            format!("https://x.spotify.link/{ID}"),
            format!("ftp://open.spotify.com/episode/{ID}"),
            "https://example.com/episode".to_string(),
            "not a url".to_string(),
        ] {
            assert_eq!(SpotifyLink::parse(&raw), None, "{raw}");
        }
    }

    #[test]
    fn routing_and_refusals_follow_the_kind() {
        let episode = entity(SpotifyKind::Episode, None);
        assert!(episode.is_routable());
        assert!(episode.refusal("u").is_none());
        let short = SpotifyLink::Short {
            url: "https://spotify.link/x".to_string(),
        };
        assert!(short.is_routable());
        assert!(short.refusal("u").is_none());
        for (kind, message) in [
            (
                SpotifyKind::Show,
                "Spotify show links are not references: u",
            ),
            (
                SpotifyKind::Track,
                "Spotify music links are not references: u",
            ),
            (
                SpotifyKind::Album,
                "Spotify music links are not references: u",
            ),
            (
                SpotifyKind::Playlist,
                "Spotify music links are not references: u",
            ),
            (
                SpotifyKind::Artist,
                "Spotify music links are not references: u",
            ),
            (
                SpotifyKind::Audiobook,
                "Spotify audiobooks are not references: u",
            ),
            (
                SpotifyKind::Chapter,
                "Spotify audiobooks are not references: u",
            ),
            (SpotifyKind::User, "not a Spotify episode link: u"),
            (SpotifyKind::Other, "not a Spotify episode link: u"),
        ] {
            let link = entity(kind, None);
            assert!(!link.is_routable(), "{kind:?}");
            assert_eq!(
                link.refusal("u").expect("refused").message,
                message,
                "{kind:?}"
            );
        }
        let unsupported = SpotifyLink::Unsupported {
            url: "u".to_string(),
        };
        assert!(!unsupported.is_routable());
        assert_eq!(
            unsupported.refusal("u").expect("refused").render(),
            "not a Spotify episode link: u\nhint: share the episode link (open.spotify.com/episode/…)"
        );
    }

    /// A fake `curl` that redirects `spotify.link/<slug>` per the case
    /// table and serves every `open.spotify.com` page as HTML.
    pub(in crate::native::highlights_ref) fn write_short_link_curl(
        dir: &Path,
    ) -> PathBuf {
        let path = dir.join("fake-curl.sh");
        let script = format!(
            r#"#!/bin/sh
dest=""
prev=""
for arg in "$@"; do
  if [ "$prev" = "-o" ]; then dest="$arg"; fi
  prev="$arg"
  url="$arg"
done
: > "$dest"
case "$url" in
  *"spotify.link/episode"*)
    printf '307\ntext/html\nhttps://open.spotify.com/episode/{ID}?si=abc&t=42\n'
    ;;
  *"spotify.link/show"*)
    printf '302\ntext/html\nhttps://open.spotify.com/show/{ID}?si=abc\n'
    ;;
  *"spotify.link/home"*)
    printf '302\ntext/html\nhttps://open.spotify.com/\n'
    ;;
  *"open.spotify.com"*)
    printf '<html>episode</html>' > "$dest"
    printf '200\ntext/html; charset=utf-8\n\n'
    ;;
  *)
    printf '404\ntext/html\n\n'
    ;;
esac
"#
        );
        std::fs::write(&path, script).expect("write fake curl");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(
                &path,
                std::fs::Permissions::from_mode(0o755),
            )
            .expect("chmod fake curl");
        }
        path
    }

    pub(in crate::native::highlights_ref) fn short_link_test_dir(
        name: &str,
    ) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "bob-cli-spotify-{}-{}-{name}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|duration| duration.as_nanos())
                .unwrap_or(0),
        ));
        std::fs::create_dir_all(&dir).expect("create test dir");
        dir
    }

    #[test]
    fn short_links_resolve_through_validated_redirects() {
        let dir = short_link_test_dir("resolve");
        let fake = write_short_link_curl(&dir);
        let _guard = crate::native::env::TestEnvGuard::set(&[
            (
                super::super::fetch::ENV_CURL_OVERRIDE,
                Some(fake.as_os_str()),
            ),
            (
                super::super::fetch::ENV_RESOLVE_OVERRIDE,
                Some(std::ffi::OsStr::new("*=203.0.113.1")),
            ),
        ]);

        let resolved =
            resolve_short_link("https://spotify.link/episode", &dir, None)
                .expect("episode short link resolves");
        assert_eq!(
            resolved.final_url,
            format!("https://open.spotify.com/episode/{ID}?si=abc&t=42")
        );
        assert_eq!(resolved.link, entity(SpotifyKind::Episode, Some(42)));
        assert_eq!(resolved.status, 200);
        assert!(resolved.content_type.starts_with("text/html"));
        assert_eq!(
            std::fs::read_to_string(&resolved.body).expect("read body"),
            "<html>episode</html>"
        );

        // A show resolves; refusing it is the caller's job.
        let show = resolve_short_link("https://spotify.link/show", &dir, None)
            .expect("show short link resolves");
        assert_eq!(
            show.link
                .refusal(&show.link.canonical_url().expect("canonical"))
                .expect("show refuses")
                .message,
            format!("Spotify show links are not references: https://open.spotify.com/show/{ID}")
        );

        for slug in ["home", "dead"] {
            let url = format!("https://spotify.link/{slug}");
            match resolve_short_link(&url, &dir, None) {
                Err(ShortLinkError::NotEntity(refusal)) => assert_eq!(
                    refusal.render(),
                    format!("could not resolve Spotify short link {url} to an open.spotify.com link\nhint: open it in Spotify and share the open.spotify.com link instead")
                ),
                other => panic!("{slug}: expected NotEntity, got {other:?}"),
            }
        }
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn canonical_urls_cover_known_kinds_only() {
        assert_eq!(
            canonical_entity_url(&format!("spotify:show:{ID}")).as_deref(),
            Some(format!("https://open.spotify.com/show/{ID}").as_str())
        );
        assert_eq!(
            canonical_entity_url(&format!(
                "https://open.spotify.com/intl-fr/embed/episode/{ID}/?si=x"
            ))
            .as_deref(),
            Some(format!("https://open.spotify.com/episode/{ID}").as_str())
        );
        assert_eq!(
            canonical_entity_url(&format!(
                "https://open.spotify.com/concert/{ID}"
            )),
            None
        );
        assert_eq!(canonical_entity_url("https://spotify.link/x"), None);
        assert_eq!(canonical_entity_url("https://example.com/x"), None);
    }
}
