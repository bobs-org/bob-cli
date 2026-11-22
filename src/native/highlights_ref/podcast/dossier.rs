//! The episode dossier builder, shared by `bob ref create` and URL
//! ingest.
//!
//! [`prepare`] climbs the metadata ladder, downloads the cover, segments
//! the show notes, and settles the reference fields (an explicit override
//! always wins). With `highlights.podcasts.enrich: auto` it then looks
//! up the publisher feed under a 45 s deadline and, on a proven match,
//! attaches feed notes, Podcasting 2.0 chapters, and a transcript. A dry
//! run looks the feed up but does not download the transcript.
//! [`Dossier::render`] then writes the `podcast_episode` `article.json`
//! (cover under `assets/`) and prints it through the adapter's offline
//! `render` op.
//!
//! Nothing here prints. Every degradation becomes exactly one entry in
//! [`Dossier::warnings`] (a metadata fallback, a missing cover, a feed
//! step-down), and the caller decides how to report it: create writes
//! `warning:` lines, while ingest routes them through its progress
//! channel.

use std::{
    fs,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use serde::Serialize;
use serde_json::{json, Value};

use super::super::clip_adapter::{
    AdapterFailure, ClipAdapterClient, RenderRequest, TablesSettings,
};
use super::super::clip_url::title_stem;
use super::super::collection::format_count;
use super::super::fetch::deadline_spent;
use super::super::spotify::SpotifyEpisode;
use super::cover::{fetch_cover, CoverOutcome};
use super::feed::{
    fetch_feed_chapters, fetch_feed_transcript, find_feed_item,
    ChaptersOutcome, FeedItem, FeedOutcome, TranscriptFileOutcome,
};
use super::metadata::{
    fetch_episode, EpisodeError, EpisodeField, EpisodeMetadata, MetadataSource,
};
use super::notes::{
    segment_notes, segment_text, Chapter, NotesBlock, ShowNotes,
};
use super::transcript::{
    parse_transcript, preferred_offer, Paragraph, Transcript, TranscriptFormat,
};
use crate::native::config::PodcastEnrich;

/// Total time for the feed lookup and the transcript download.
const ENRICH_DEADLINE_SECS: u64 = 45;

/// Chapter and transcript times link to Spotify `?t=` only when the
/// feed duration is this close to Spotify's.
const DRIFT_TOLERANCE_MS: u64 = 10_000;

/// A chapter mark sits on the paragraph whose start is this close.
const CHAPTER_MARK_WINDOW_SECS: u32 = 30;

/// The `kind` of a Bob-built episode payload.
pub(crate) const PAYLOAD_KIND: &str = "podcast_episode";

/// The dossier's subdirectory of the caller's scratch directory.
const WORKDIR_NAME: &str = "podcast";

/// Where the cover lands, relative to `article.json`.
const ASSETS_DIR: &str = "assets";

/// The reference fields a caller may override (`-T`, `-A`, `-p`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct EpisodeOverrides {
    pub(crate) title: Option<String>,
    pub(crate) author: Option<String>,
    pub(crate) published: Option<String>,
}

/// Where a settled reference field came from, for reports.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FieldOrigin {
    /// An explicit `-T`, `-A`, or `-p`.
    Override,
    /// The Spotify surface that supplied it.
    Spotify(MetadataSource),
}

impl FieldOrigin {
    /// The dry-run label, as in `title: … (spotify page)`.
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Override => "override",
            Self::Spotify(source) => source.label(),
        }
    }
}

/// A chapter heading parked on a transcript paragraph.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct ChapterMark {
    pub(crate) start_secs: u32,
    pub(crate) title: String,
    pub(crate) paragraph_index: usize,
}

/// Everything known about one episode before it is printed.
#[derive(Debug, Clone)]
pub(crate) struct Dossier {
    pub(crate) episode: SpotifyEpisode,
    pub(crate) metadata: EpisodeMetadata,
    pub(crate) notes: ShowNotes,
    pub(crate) cover: CoverOutcome,
    /// `-T`, else the episode title.
    pub(crate) title: String,
    pub(crate) title_origin: FieldOrigin,
    /// `-A`, else the show name.
    pub(crate) author: Option<String>,
    pub(crate) author_origin: Option<FieldOrigin>,
    /// `-p`, else the local release date (`YYYY-MM-DD`).
    pub(crate) published: Option<String>,
    pub(crate) published_origin: Option<FieldOrigin>,
    /// The capture date (`YYYY-MM-DD`).
    pub(crate) captured: String,
    /// One line per degradation: a metadata fallback (naming what it
    /// lost, including missing show notes), a missing cover, and each
    /// feed step-down.
    pub(crate) warnings: Vec<String>,
    /// `highlights.podcasts.enrich`.
    pub(crate) enrich: PodcastEnrich,
    /// Feed notes replaced Spotify's.
    pub(crate) notes_from_feed: bool,
    /// The matched feed's host, for dry-run `feed:` and provenance.
    pub(crate) feed_host: Option<String>,
    /// `title, date` (the signals the match agreed on).
    pub(crate) matched_on_label: Option<String>,
    /// The transcript format that would attach (dry run) or did.
    pub(crate) transcript_format: Option<TranscriptFormat>,
    /// The attached transcript; `None` on a dry run even when an offer
    /// was found.
    pub(crate) transcript: Option<Transcript>,
    /// Chapter and transcript times may deep-link to Spotify `?t=`.
    pub(crate) link_times: bool,
    workdir: PathBuf,
}

/// The printed dossier: the scratch PDF plus the adapter's report.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RenderedDossier {
    /// The unstamped scratch PDF.
    pub(crate) pdf: PathBuf,
    /// The source view the write-once store converts.
    pub(crate) source_html: Option<String>,
    pub(crate) code_anchors: Option<String>,
    /// Adapter warnings (for example, a cover it could not normalize).
    pub(crate) warnings: Vec<String>,
}

/// Why no dossier was built.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum DossierError {
    /// The metadata ladder failed closed (404, 429/5xx, transport, or no
    /// title).
    Episode(EpisodeError),
    /// The adapter could not print the dossier. The message starts with
    /// the adapter's error kind (`browser: …`, `render: …`).
    Render {
        message: String,
        hint: Option<String>,
    },
    /// A local filesystem step failed.
    Io { message: String },
}

impl DossierError {
    /// The text after `error:`.
    pub(crate) fn message(&self) -> String {
        match self {
            Self::Episode(error) => error.message(),
            Self::Render { message, .. } | Self::Io { message } => {
                message.clone()
            }
        }
    }

    /// The text after `hint:`, when there is one.
    pub(crate) fn hint(&self) -> Option<String> {
        match self {
            Self::Episode(error) => error.hint(),
            Self::Render { hint, .. } => hint.clone(),
            Self::Io { .. } => None,
        }
    }

    /// The `message\nhint: hint` body the error renderers expect.
    pub(crate) fn render(&self) -> String {
        match self {
            Self::Episode(error) => error.render(),
            _ => match self.hint() {
                Some(hint) => format!("{}\nhint: {hint}", self.message()),
                None => self.message(),
            },
        }
    }

    fn from_adapter(failure: AdapterFailure) -> Self {
        Self::Render {
            message: failure.message,
            hint: failure.hint,
        }
    }
}

impl From<EpisodeError> for DossierError {
    fn from(error: EpisodeError) -> Self {
        Self::Episode(error)
    }
}

/// Fetch everything the dossier needs into `<scratch>/podcast/`: the
/// metadata ladder (fails closed without a title), the cover (fails open
/// with one warning), the segmented show notes, and (when `enrich` is
/// `auto`) the publisher feed under a 45 s deadline.
///
/// `prefetched_page` is an already-downloaded 200 episode page (a
/// resolved short link's final body); `captured` is the capture date.
/// `attach_transcript` is false on a dry run: the feed is still looked
/// up, but the transcript file is not downloaded.
#[allow(clippy::too_many_arguments)]
pub(crate) fn prepare(
    episode: &SpotifyEpisode,
    overrides: &EpisodeOverrides,
    captured: &str,
    scratch: &Path,
    progress: Option<&dyn Fn(&str)>,
    prefetched_page: Option<&Path>,
    enrich: PodcastEnrich,
    attach_transcript: bool,
) -> Result<Dossier, DossierError> {
    prepare_with_deadline(
        episode,
        overrides,
        captured,
        scratch,
        progress,
        prefetched_page,
        enrich,
        attach_transcript,
        Instant::now() + Duration::from_secs(ENRICH_DEADLINE_SECS),
    )
}

/// [`prepare`] with an injected enrich deadline (tests spend it).
#[allow(clippy::too_many_arguments)]
fn prepare_with_deadline(
    episode: &SpotifyEpisode,
    overrides: &EpisodeOverrides,
    captured: &str,
    scratch: &Path,
    progress: Option<&dyn Fn(&str)>,
    prefetched_page: Option<&Path>,
    enrich: PodcastEnrich,
    attach_transcript: bool,
    deadline: Instant,
) -> Result<Dossier, DossierError> {
    let workdir = scratch.join(WORKDIR_NAME);
    fs::create_dir_all(&workdir).map_err(|error| DossierError::Io {
        message: format!("create {}: {error}", workdir.display()),
    })?;
    let metadata = fetch_episode(episode, &workdir, progress, prefetched_page)?;
    let cover = fetch_cover(
        metadata.cover_url.as_deref(),
        &workdir.join(ASSETS_DIR),
        progress,
    );
    let notes = match (&metadata.notes_html, &metadata.notes_text) {
        (Some(html), _) => segment_notes(html, metadata.duration_ms),
        (None, Some(text)) => segment_text(text, metadata.duration_ms),
        (None, None) => ShowNotes::default(),
    };
    let mut warnings = metadata.warnings.clone();
    if let Some(warning) = cover.warning() {
        warnings.push(warning.to_string());
    }

    let spotify = |field| metadata.source_of(field).map(FieldOrigin::Spotify);
    let (title, title_origin) = match nonblank(overrides.title.as_deref()) {
        Some(title) => (title, FieldOrigin::Override),
        None => (
            metadata.title.clone(),
            spotify(EpisodeField::Title)
                .unwrap_or(FieldOrigin::Spotify(MetadataSource::Page)),
        ),
    };
    let (author, author_origin) = match nonblank(overrides.author.as_deref()) {
        Some(author) => (Some(author), Some(FieldOrigin::Override)),
        None => match &metadata.show {
            Some(show) => (Some(show.clone()), spotify(EpisodeField::Show)),
            None => (None, None),
        },
    };
    let (published, published_origin) =
        match nonblank(overrides.published.as_deref()) {
            Some(published) => (Some(published), Some(FieldOrigin::Override)),
            None => match metadata.published_date() {
                Some(date) => (
                    Some(date.format("%Y-%m-%d").to_string()),
                    spotify(EpisodeField::Released),
                ),
                None => (None, None),
            },
        };
    let mut dossier = Dossier {
        episode: episode.clone(),
        metadata,
        notes,
        cover,
        title,
        title_origin,
        author,
        author_origin,
        published,
        published_origin,
        captured: captured.to_string(),
        warnings,
        enrich,
        notes_from_feed: false,
        feed_host: None,
        matched_on_label: None,
        transcript_format: None,
        transcript: None,
        link_times: true,
        workdir,
    };
    enrich_dossier(&mut dossier, attach_transcript, deadline, progress);
    Ok(dossier)
}

fn nonblank(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

/// `spotify_episode_<lowercased id>`: the stem when the title yields no
/// ASCII stem. Never the raw (mixed-case) id.
pub(crate) fn fallback_stem(id: &str) -> String {
    format!("spotify_episode_{}", id.to_ascii_lowercase())
}

/// `1 h 26 min`, `1 h`, `48 min`, or `45 s` (the renderer's format).
pub(crate) fn format_duration(duration_ms: u64) -> String {
    let total = duration_ms / 1000;
    if total < 60 {
        return format!("{total} s");
    }
    let minutes = total / 60;
    if minutes < 60 {
        return format!("{minutes} min");
    }
    let (hours, minutes) = (minutes / 60, minutes % 60);
    if minutes == 0 {
        format!("{hours} h")
    } else {
        format!("{hours} h {minutes} min")
    }
}

/// How a provenance entry names a Spotify surface.
fn provenance_source(source: MetadataSource) -> &'static str {
    match source {
        MetadataSource::Page => "Spotify's public episode page",
        MetadataSource::Embed => "Spotify's embed player",
        MetadataSource::Oembed => "Spotify's oEmbed endpoint",
    }
}

fn deadline_warning() -> String {
    format!("feed lookup stopped after {ENRICH_DEADLINE_SECS} s")
}

fn no_feed_warning(show: Option<&str>) -> String {
    let show = show
        .and_then(super::clean_line)
        .unwrap_or_else(|| "this show".to_string());
    format!(
        "no public feed matched \"{show}\" (show notes from Spotify; no transcript)"
    )
}

/// Chapter and transcript times link only when both durations are known
/// and within [`DRIFT_TOLERANCE_MS`].
fn durations_agree(spotify_ms: Option<u64>, feed_secs: Option<u64>) -> bool {
    match (spotify_ms, feed_secs) {
        (Some(spotify), Some(feed)) => {
            spotify.abs_diff(feed.saturating_mul(1000)) <= DRIFT_TOLERANCE_MS
        }
        _ => true,
    }
}

/// Park each chapter on the nearest timed paragraph within ±30 s.
fn chapter_marks(
    chapters: &[&Chapter],
    paragraphs: &[Paragraph],
) -> Vec<ChapterMark> {
    let mut marks = Vec::new();
    for chapter in chapters {
        let mut best: Option<(usize, u32)> = None;
        for (index, paragraph) in paragraphs.iter().enumerate() {
            let Some(start) = paragraph.start_secs else {
                continue;
            };
            let delta = start.abs_diff(chapter.start_secs);
            if delta <= CHAPTER_MARK_WINDOW_SECS
                && best.is_none_or(|(_, known)| delta < known)
            {
                best = Some((index, delta));
            }
        }
        if let Some((paragraph_index, _)) = best {
            marks.push(ChapterMark {
                start_secs: chapter.start_secs,
                title: chapter.title.clone(),
                paragraph_index,
            });
        }
    }
    marks.sort_by_key(|mark| (mark.paragraph_index, mark.start_secs));
    marks
}

fn enrich_dossier(
    dossier: &mut Dossier,
    attach_transcript: bool,
    deadline: Instant,
    progress: Option<&dyn Fn(&str)>,
) {
    if dossier.enrich == PodcastEnrich::Off {
        return;
    }
    match find_feed_item(
        &dossier.metadata,
        dossier.enrich,
        &dossier.workdir,
        deadline,
        progress,
    ) {
        FeedOutcome::Off => {}
        FeedOutcome::DeadlineExceeded => {
            dossier.warnings.push(deadline_warning());
        }
        FeedOutcome::NoFeed(_) | FeedOutcome::NoMatch(_) => {
            dossier
                .warnings
                .push(no_feed_warning(dossier.metadata.show.as_deref()));
        }
        FeedOutcome::Matched(item) => {
            apply_feed_match(
                dossier,
                item,
                attach_transcript,
                deadline,
                progress,
            );
        }
    }
}

fn apply_feed_match(
    dossier: &mut Dossier,
    item: FeedItem,
    attach_transcript: bool,
    deadline: Instant,
    progress: Option<&dyn Fn(&str)>,
) {
    dossier.feed_host = Some(item.feed_host.clone());
    dossier.matched_on_label = Some(item.matched_on_label());
    dossier.link_times =
        durations_agree(dossier.metadata.duration_ms, item.duration_secs);
    if let Some(html) = item
        .notes_html
        .as_deref()
        .filter(|html| !html.trim().is_empty())
    {
        dossier.notes = segment_notes(html, dossier.metadata.duration_ms);
        dossier.notes_from_feed = true;
    }
    if let Some(link) = &item.chapters {
        match fetch_feed_chapters(link, &dossier.workdir, deadline, progress) {
            ChaptersOutcome::Chapters(chapters) => {
                dossier.notes.replace_chapters(chapters);
            }
            ChaptersOutcome::Ignored(_) => {}
            ChaptersOutcome::DeadlineExceeded => {
                dossier.warnings.push(deadline_warning());
                return;
            }
        }
    }
    let Some((offer, format)) =
        preferred_offer(&item.transcripts, |offer| (&offer.mime, &offer.url))
    else {
        dossier.warnings.push(
            "the publisher feed has no transcript for this episode".into(),
        );
        return;
    };
    dossier.transcript_format = Some(format);
    if !attach_transcript {
        return;
    }
    if deadline_spent(deadline) {
        dossier.warnings.push(deadline_warning());
        return;
    }
    let bytes = match fetch_feed_transcript(
        offer,
        &dossier.workdir,
        deadline,
        progress,
    ) {
        TranscriptFileOutcome::Bytes(bytes) => bytes,
        TranscriptFileOutcome::DeadlineExceeded => {
            dossier.warnings.push(deadline_warning());
            return;
        }
        TranscriptFileOutcome::Ignored(reason) => {
            dossier
                .warnings
                .push(format!("transcript dropped: {reason}"));
            return;
        }
    };
    match parse_transcript(
        &bytes,
        &offer.mime,
        &offer.url,
        dossier.metadata.duration_ms,
    ) {
        Ok(transcript) => dossier.transcript = Some(transcript),
        Err(reason) => {
            dossier
                .warnings
                .push(format!("transcript dropped: {reason}"));
        }
    }
}

impl Dossier {
    /// `title_stem(title)` (80-character cap), else
    /// [`fallback_stem`]. `-N` is the caller's business.
    pub(crate) fn stem(&self) -> String {
        let stem = title_stem(&self.title);
        if stem.is_empty() {
            fallback_stem(&self.episode.id)
        } else {
            stem
        }
    }

    /// The canonical episode URL: the `source_url` and dedupe key.
    pub(crate) fn canonical_url(&self) -> String {
        self.episode.canonical_url()
    }

    /// The PDF Info author: the show, else the settled author.
    pub(crate) fn pdf_author(&self) -> Option<String> {
        self.metadata.show.clone().or_else(|| self.author.clone())
    }

    /// The episode duration as the dossier prints it.
    pub(crate) fn duration_display(&self) -> Option<String> {
        self.metadata.duration_ms.map(format_duration)
    }

    pub(crate) fn chapter_count(&self) -> usize {
        self.notes.chapters().count()
    }

    pub(crate) fn has_cover(&self) -> bool {
        self.cover.cover().is_some()
    }

    /// The report's capture line, as in
    /// `episode dossier · 16 chapters · publisher show notes · publisher transcript 18,736 words`.
    pub(crate) fn capture_summary(&self) -> String {
        if self.enrich == PodcastEnrich::Off {
            return "episode dossier (publisher feed lookup off)".to_string();
        }
        let mut parts = vec!["episode dossier".to_string()];
        match self.chapter_count() {
            0 => {}
            1 => parts.push("1 chapter".to_string()),
            count => parts.push(format!("{count} chapters")),
        }
        if self.notes.is_empty() {
            parts.push("no show notes".to_string());
        } else if self.notes_from_feed {
            parts.push("publisher show notes".to_string());
        } else {
            parts.push(format!("show notes {} words", self.notes.words));
        }
        if let Some(transcript) = &self.transcript {
            parts.push(format!(
                "publisher transcript {} words",
                format_count(transcript.words as u64)
            ));
        }
        parts.join(" · ")
    }

    /// Dry-run `feed: <host> (matched on title, date)`.
    pub(crate) fn dry_run_feed_line(&self) -> Option<String> {
        Some(format!(
            "feed: {} (matched on {})",
            self.feed_host.as_ref()?,
            self.matched_on_label.as_ref()?
        ))
    }

    /// Dry-run `transcript: publisher VTT (would attach)`.
    pub(crate) fn dry_run_transcript_line(&self) -> Option<String> {
        if self.transcript.is_some() {
            return None;
        }
        Some(format!(
            "transcript: publisher {} (would attach)",
            self.transcript_format?.label()
        ))
    }

    /// The provenance footer: who supplied the details, the notes, and
    /// the transcript.
    fn provenance(&self) -> Vec<Value> {
        let details = self
            .metadata
            .source_of(EpisodeField::Title)
            .unwrap_or(MetadataSource::Page);
        let entry = |label: &str, source: MetadataSource| json!({"label": label, "source": provenance_source(source)});
        let mut entries = if self.notes_from_feed {
            let host = self.feed_host.as_deref().unwrap_or("the publisher");
            vec![
                entry("Episode details", details),
                json!({
                    "label": "Show notes",
                    "source": format!("the publisher's RSS feed ({host})"),
                }),
            ]
        } else {
            let notes = if self.notes.is_empty() {
                None
            } else if self.metadata.notes_html.is_some() {
                self.metadata.source_of(EpisodeField::NotesHtml)
            } else {
                self.metadata.source_of(EpisodeField::NotesText)
            };
            match notes {
                None => vec![entry("Episode details", details)],
                Some(notes) if notes == details => {
                    vec![entry("Episode details and show notes", details)]
                }
                Some(notes) => vec![
                    entry("Episode details", details),
                    entry("Show notes", notes),
                ],
            }
        };
        if let (Some(format), Some(_)) =
            (self.transcript_format, &self.transcript)
        {
            entries.push(json!({
                "label": "Transcript",
                "source": format!(
                    "publisher {} from the show's RSS feed",
                    format.label()
                ),
            }));
        }
        entries
    }

    /// The ordered note and chapter blocks. Sponsors travel separately
    /// (`sponsors_html`), and a chapter run whose titles all came out
    /// blank is dropped rather than sent as an empty list.
    fn blocks(&self) -> Vec<Value> {
        self.notes
            .blocks
            .iter()
            .filter_map(|block| match block {
                NotesBlock::Html(html) => {
                    Some(json!({"type": "notes", "html": html}))
                }
                NotesBlock::Chapters(chapters) => {
                    let chapters: Vec<Value> = chapters
                        .iter()
                        .filter(|chapter| !chapter.title.trim().is_empty())
                        .map(|chapter| {
                            json!({
                                "start_secs": chapter.start_secs,
                                "title": chapter.title.trim(),
                            })
                        })
                        .collect();
                    (!chapters.is_empty()).then(
                        || json!({"type": "chapters", "chapters": chapters}),
                    )
                }
                NotesBlock::Sponsors(_) => None,
            })
            .collect()
    }

    /// The `podcast_episode` payload the adapter's `render` op prints.
    pub(crate) fn article_json(&self) -> Value {
        let canonical = self.canonical_url();
        let cover = self.cover.cover().and_then(|cover| {
            cover
                .path
                .file_name()
                .and_then(|name| name.to_str())
                .map(|name| format!("{ASSETS_DIR}/{name}"))
        });
        json!({
            "kind": PAYLOAD_KIND,
            "title": self.title,
            "source_url": canonical,
            "captured": self.captured,
            "published": self.published,
            "language": self.metadata.language,
            "episode": {
                "show": self.metadata.show,
                "publisher": self.metadata.publisher,
                "duration_ms": self.metadata.duration_ms,
                "cover": cover,
                "listen_url": canonical,
                "shared_at_secs": self.episode.shared_at,
                "chapter_links": self.link_times,
                "blocks": self.blocks(),
                "sponsors_html": self.notes.sponsors_html(),
                "provenance": self.provenance(),
                "transcript": self.transcript_payload(),
            },
        })
    }

    fn transcript_payload(&self) -> Value {
        let Some(transcript) = &self.transcript else {
            return Value::Null;
        };
        let chapters: Vec<&Chapter> = self.notes.chapters().collect();
        json!({
            "format": transcript.format.as_str(),
            "words": transcript.words,
            "roles_inferred": transcript.roles_inferred,
            "names_from_file": transcript.names_from_file,
            "link_times": self.link_times,
            "paragraphs": transcript.paragraphs,
            "chapter_marks": chapter_marks(&chapters, &transcript.paragraphs),
        })
    }

    /// Write `article.json` and print it through the adapter's offline
    /// `render` op into `<workdir>/episode.pdf`.
    pub(in crate::native::highlights_ref) fn render(
        &self,
        tables: TablesSettings,
    ) -> Result<RenderedDossier, DossierError> {
        let io =
            |what: &str, path: &Path, error: std::io::Error| DossierError::Io {
                message: format!("{what} {}: {error}", path.display()),
            };
        let workdir = std::path::absolute(&self.workdir)
            .map_err(|error| io("resolve", &self.workdir, error))?;
        let article_json = workdir.join("article.json");
        let payload = serde_json::to_vec_pretty(&self.article_json()).map_err(
            |error| DossierError::Io {
                message: format!("encode the episode payload: {error}"),
            },
        )?;
        fs::write(&article_json, payload)
            .map_err(|error| io("write", &article_json, error))?;
        let out_pdf = workdir.join("episode.pdf");
        let request = RenderRequest::new(
            article_json.to_string_lossy().into_owned(),
            workdir.to_string_lossy().into_owned(),
            out_pdf.to_string_lossy().into_owned(),
            tables,
        );
        let client =
            ClipAdapterClient::resolve().map_err(DossierError::from_adapter)?;
        let response = client
            .render(&request)
            .map_err(DossierError::from_adapter)?;
        if !out_pdf.is_file() {
            return Err(DossierError::Render {
                message: format!(
                    "render: the web clip adapter wrote no PDF at {}",
                    out_pdf.display()
                ),
                hint: None,
            });
        }
        Ok(RenderedDossier {
            pdf: out_pdf,
            source_html: response.source_html,
            code_anchors: response.code_anchors,
            warnings: response.warnings,
        })
    }
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
    const SHELL: &str = include_str!(
        "../../../../tests/fixtures/spotify/drift_page_shell.html"
    );
    const ITUNES: &str = include_str!(
        "../../../../tests/fixtures/podcast/itunes_pragmatic_engineer.json"
    );
    const FEED: &str = include_str!(
        "../../../../tests/fixtures/podcast/feed_pragmatic_engineer.rss"
    );
    const VTT: &str = concat!(
        include_str!(
            "../../../../tests/fixtures/podcast/pragmatic_engineer_transcript.vtt"
        ),
        "\n61\n01:20:00.000 --> 01:20:04.000\n<v SPEAKER_01>That's the episode.\n",
    );
    const FEED_URL: &str = "https://api.substack.com/feed/podcast/458709.rss";
    const TRANSCRIPT_URL: &str = "https://api.substack.com/feed/podcast/transcript/225268679/5716fdd023036d0a64c51e7e8330539c.vtt";
    const SHOW: &str = "The Pragmatic Engineer";

    fn jpeg() -> Vec<u8> {
        let mut bytes = vec![0xFF, 0xD8, 0xFF, 0xE0, 0, 16, b'J', b'F'];
        bytes.extend_from_slice(&[0; 24]);
        bytes
    }

    fn episode(shared_at: Option<u32>) -> SpotifyEpisode {
        SpotifyEpisode {
            id: ID.to_string(),
            shared_at,
        }
    }

    fn real_episode(fake: &FakeSpotify) {
        fake.serve("page", 200, "text/html", PAGE.as_bytes());
        fake.serve("cover", 200, "image/jpeg", &jpeg());
    }

    fn serve_feed(fake: &FakeSpotify) {
        fake.serve("itunes", 200, "application/json", ITUNES.as_bytes());
        fake.serve_url(FEED_URL, 200, "application/rss+xml", FEED.as_bytes());
        fake.serve_url(TRANSCRIPT_URL, 200, "text/vtt", VTT.as_bytes());
    }

    fn settle(
        fake: &FakeSpotify,
        name: &str,
        enrich: PodcastEnrich,
        attach_transcript: bool,
        shared_at: Option<u32>,
        overrides: EpisodeOverrides,
    ) -> Dossier {
        prepare(
            &episode(shared_at),
            &overrides,
            "2026-11-21",
            &fake.scratch(name),
            None,
            None,
            enrich,
            attach_transcript,
        )
        .expect("dossier")
    }

    #[test]
    fn the_real_episode_settles_every_field_from_the_page() {
        let fake = FakeSpotify::new();
        real_episode(&fake);
        let dossier = settle(
            &fake,
            "real",
            PodcastEnrich::Off,
            true,
            Some(1235),
            EpisodeOverrides::default(),
        );
        assert_eq!(
            dossier.title,
            "Software factories and code reviews with Harjot Gill"
        );
        assert_eq!(
            dossier.title_origin,
            FieldOrigin::Spotify(MetadataSource::Page)
        );
        assert_eq!(dossier.author.as_deref(), Some("The Pragmatic Engineer"));
        assert_eq!(dossier.title_origin.label(), "spotify page");
        assert!(dossier.published.is_some());
        assert_eq!(
            dossier.stem(),
            "software_factories_and_code_reviews_with_harjot_gill"
        );
        assert_eq!(dossier.chapter_count(), 16);
        assert!(dossier.has_cover());
        assert!(dossier.warnings.is_empty(), "{:?}", dossier.warnings);
        assert_eq!(dossier.duration_display().as_deref(), Some("1 h 26 min"));
        assert_eq!(
            dossier.capture_summary(),
            "episode dossier (publisher feed lookup off)"
        );

        let payload = dossier.article_json();
        assert_eq!(payload["kind"], PAYLOAD_KIND);
        assert_eq!(
            payload["source_url"],
            format!("https://open.spotify.com/episode/{ID}")
        );
        assert_eq!(payload["captured"], "2026-11-21");
        let episode = &payload["episode"];
        assert_eq!(episode["show"], "The Pragmatic Engineer");
        assert_eq!(episode["cover"], "assets/cover.jpg");
        assert_eq!(episode["shared_at_secs"], 1235);
        assert_eq!(episode["chapter_links"], true);
        assert_eq!(episode["transcript"], Value::Null);
        assert_eq!(episode["listen_url"], payload["source_url"]);
        let blocks = episode["blocks"].as_array().expect("blocks");
        let chapters: Vec<&Value> = blocks
            .iter()
            .filter(|block| block["type"] == "chapters")
            .collect();
        assert_eq!(chapters.len(), 1);
        assert_eq!(chapters[0]["chapters"].as_array().unwrap().len(), 16);
        assert!(blocks.iter().all(|block| block["type"] != "sponsors"));
        assert!(episode["sponsors_html"]
            .as_str()
            .is_some_and(|html| html.contains("turbopuffer")));
        assert_eq!(
            episode["provenance"],
            json!([{
                "label": "Episode details and show notes",
                "source": "Spotify's public episode page",
            }])
        );
    }

    #[test]
    fn overrides_win_and_name_themselves() {
        let fake = FakeSpotify::new();
        real_episode(&fake);
        let overrides = EpisodeOverrides {
            title: Some("Harjot on software factories".to_string()),
            author: Some("Harjot Gill".to_string()),
            published: Some("2026-01-02".to_string()),
        };
        let dossier = settle(
            &fake,
            "overrides",
            PodcastEnrich::Off,
            true,
            None,
            overrides,
        );
        assert_eq!(dossier.title, "Harjot on software factories");
        assert_eq!(dossier.title_origin.label(), "override");
        assert_eq!(dossier.author.as_deref(), Some("Harjot Gill"));
        assert_eq!(dossier.author_origin, Some(FieldOrigin::Override));
        assert_eq!(dossier.published.as_deref(), Some("2026-01-02"));
        assert_eq!(dossier.stem(), "harjot_on_software_factories");
        // The PDF Info author stays the show.
        assert_eq!(
            dossier.pdf_author().as_deref(),
            Some("The Pragmatic Engineer")
        );
        assert_eq!(
            dossier.article_json()["episode"]["shared_at_secs"],
            Value::Null
        );
    }

    #[test]
    fn a_title_without_ascii_falls_back_to_the_lowercased_id() {
        let fake = FakeSpotify::new();
        real_episode(&fake);
        let overrides = EpisodeOverrides {
            title: Some("ソフトウェア工場".to_string()),
            ..EpisodeOverrides::default()
        };
        let dossier = settle(
            &fake,
            "non-ascii",
            PodcastEnrich::Off,
            true,
            None,
            overrides,
        );
        assert_eq!(dossier.stem(), "spotify_episode_4ugvfwty2i8hgagsvt7pjc");
    }

    #[test]
    fn a_missing_cover_is_one_warning() {
        let fake = FakeSpotify::new();
        fake.serve("page", 200, "text/html", PAGE.as_bytes());
        fake.serve("cover", 404, "text/html", b"");
        let dossier = settle(
            &fake,
            "no-cover",
            PodcastEnrich::Off,
            true,
            None,
            EpisodeOverrides::default(),
        );
        assert_eq!(dossier.warnings.len(), 1, "{:?}", dossier.warnings);
        assert!(dossier.warnings[0].starts_with("cover art missing:"));
        assert!(!dossier.has_cover());
        assert_eq!(dossier.article_json()["episode"]["cover"], Value::Null);
    }

    #[test]
    fn an_oembed_only_capture_warns_once_and_has_no_notes() {
        let fake = FakeSpotify::new();
        fake.serve("page", 200, "text/html", SHELL.as_bytes());
        fake.serve("embed", 200, "text/html", b"<html>nothing</html>");
        fake.serve("oembed", 200, "application/json", OEMBED.as_bytes());
        fake.serve("cover", 200, "image/jpeg", &jpeg());
        let dossier = settle(
            &fake,
            "oembed",
            PodcastEnrich::Off,
            true,
            None,
            EpisodeOverrides::default(),
        );
        assert_eq!(dossier.warnings.len(), 1, "{:?}", dossier.warnings);
        assert!(dossier.warnings[0].contains("show notes"));
        assert_eq!(dossier.author, None);
        assert_eq!(
            dossier.capture_summary(),
            "episode dossier (publisher feed lookup off)"
        );
        assert_eq!(
            dossier.article_json()["episode"]["provenance"],
            json!([{
                "label": "Episode details",
                "source": "Spotify's oEmbed endpoint",
            }])
        );
    }

    #[test]
    fn notes_from_the_embed_get_their_own_provenance_line() {
        let fake = FakeSpotify::new();
        fake.serve("page", 200, "text/html", SHELL.as_bytes());
        fake.serve("embed", 200, "text/html", EMBED.as_bytes());
        fake.serve("oembed", 200, "application/json", OEMBED.as_bytes());
        let dossier = settle(
            &fake,
            "embed",
            PodcastEnrich::Off,
            true,
            None,
            EpisodeOverrides::default(),
        );
        let provenance =
            dossier.article_json()["episode"]["provenance"].clone();
        let entries = provenance.as_array().expect("provenance");
        assert!(!entries.is_empty());
        assert!(entries
            .iter()
            .all(|entry| entry["source"] != "Spotify's public episode page"));
    }

    #[test]
    fn no_metadata_fails_closed_before_any_cover() {
        let fake = FakeSpotify::new();
        fake.serve("page", 404, "text/html", b"");
        let scratch = fake.scratch("404");
        let error = prepare(
            &episode(None),
            &EpisodeOverrides::default(),
            "2026-11-21",
            &scratch,
            None,
            None,
            PodcastEnrich::Off,
            true,
        )
        .expect_err("404 fails");
        assert_eq!(
            error.render(),
            format!("Spotify has no public episode {ID} (HTTP 404)")
        );
        assert_eq!(fake.requests().len(), 1);
    }

    #[test]
    fn durations_match_the_renderer() {
        assert_eq!(format_duration(5_197_032), "1 h 26 min");
        assert_eq!(format_duration(3_600_000), "1 h");
        assert_eq!(format_duration(2_880_000), "48 min");
        assert_eq!(format_duration(45_999), "45 s");
        assert_eq!(format_duration(0), "0 s");
    }

    #[test]
    fn itunes_feed_vtt_puts_a_transcript_in_the_payload() {
        let fake = FakeSpotify::new();
        real_episode(&fake);
        serve_feed(&fake);
        let dossier = settle(
            &fake,
            "matched",
            PodcastEnrich::Auto,
            true,
            Some(1235),
            EpisodeOverrides::default(),
        );
        assert!(dossier.warnings.is_empty(), "{:?}", dossier.warnings);
        assert!(dossier.notes_from_feed);
        assert_eq!(dossier.feed_host.as_deref(), Some("api.substack.com"));
        assert_eq!(
            dossier.matched_on_label.as_deref(),
            Some("title, date, duration")
        );
        assert!(dossier.link_times);
        let transcript = dossier.transcript.as_ref().expect("transcript");
        assert_eq!(transcript.format, TranscriptFormat::Vtt);
        assert!(transcript.words >= 200, "{}", transcript.words);
        let payload = dossier.article_json();
        let attached = &payload["episode"]["transcript"];
        assert_eq!(attached["format"], "vtt");
        assert_eq!(attached["link_times"], true);
        assert!(attached["paragraphs"].as_array().unwrap().len() >= 2);
        assert!(
            !attached["chapter_marks"].as_array().unwrap().is_empty(),
            "{attached}"
        );
        assert!(
            dossier.capture_summary().starts_with(
                "episode dossier · 16 chapters · publisher show notes · publisher transcript "
            ),
            "{}",
            dossier.capture_summary()
        );
        let provenance = payload["episode"]["provenance"].as_array().unwrap();
        assert!(
            provenance.iter().any(|entry| {
                entry["label"] == "Show notes"
                    && entry["source"]
                        == "the publisher's RSS feed (api.substack.com)"
            }),
            "{provenance:?}"
        );
        assert!(
            provenance.iter().any(|entry| {
                entry["label"] == "Transcript"
                    && entry["source"]
                        == "publisher VTT from the show's RSS feed"
            }),
            "{provenance:?}"
        );
        let requests = fake.requests();
        assert!(
            requests.iter().any(|url| url.contains("itunes.apple.com")),
            "{requests:?}"
        );
        assert!(requests.iter().any(|url| url == FEED_URL), "{requests:?}");
        assert!(
            requests.iter().any(|url| url == TRANSCRIPT_URL),
            "{requests:?}"
        );
    }

    #[test]
    fn a_dry_run_looks_up_the_feed_and_does_not_download_the_transcript() {
        let fake = FakeSpotify::new();
        real_episode(&fake);
        serve_feed(&fake);
        let dossier = settle(
            &fake,
            "dry",
            PodcastEnrich::Auto,
            false,
            None,
            EpisodeOverrides::default(),
        );
        assert!(dossier.warnings.is_empty(), "{:?}", dossier.warnings);
        assert_eq!(
            dossier.dry_run_feed_line().as_deref(),
            Some("feed: api.substack.com (matched on title, date, duration)")
        );
        assert_eq!(
            dossier.dry_run_transcript_line().as_deref(),
            Some("transcript: publisher VTT (would attach)")
        );
        assert!(dossier.transcript.is_none());
        assert_eq!(
            dossier.article_json()["episode"]["transcript"],
            Value::Null
        );
        let requests = fake.requests();
        assert!(requests.iter().any(|url| url == FEED_URL), "{requests:?}");
        assert!(
            requests.iter().all(|url| url != TRANSCRIPT_URL),
            "{requests:?}"
        );
    }

    #[test]
    fn a_missed_match_warns_once() {
        let fake = FakeSpotify::new();
        real_episode(&fake);
        fake.serve("itunes", 404, "text/html", b"");
        let dossier = settle(
            &fake,
            "miss",
            PodcastEnrich::Auto,
            true,
            None,
            EpisodeOverrides::default(),
        );
        assert_eq!(dossier.warnings, vec![no_feed_warning(Some(SHOW))]);
        assert!(!dossier.notes_from_feed);
        assert!(dossier.transcript.is_none());
        assert!(dossier.link_times);
        assert_eq!(dossier.article_json()["episode"]["chapter_links"], true);
        assert!(
            dossier
                .capture_summary()
                .starts_with("episode dossier · 16 chapters · show notes "),
            "{}",
            dossier.capture_summary()
        );
    }

    #[test]
    fn drift_sets_link_times_false() {
        let fake = FakeSpotify::new();
        real_episode(&fake);
        fake.serve("itunes", 200, "application/json", ITUNES.as_bytes());
        let drifted = FEED.replace(
            "<itunes:duration>5197</itunes:duration>",
            "<itunes:duration>4000</itunes:duration>",
        );
        fake.serve_url(
            FEED_URL,
            200,
            "application/rss+xml",
            drifted.as_bytes(),
        );
        fake.serve_url(TRANSCRIPT_URL, 200, "text/vtt", VTT.as_bytes());
        let dossier = settle(
            &fake,
            "drift",
            PodcastEnrich::Auto,
            true,
            None,
            EpisodeOverrides::default(),
        );
        assert!(dossier.warnings.is_empty(), "{:?}", dossier.warnings);
        assert!(!dossier.link_times);
        assert_eq!(dossier.article_json()["episode"]["chapter_links"], false);
        assert_eq!(
            dossier.article_json()["episode"]["transcript"]["link_times"],
            false
        );
    }

    #[test]
    fn enrich_off_makes_no_itunes_request() {
        let fake = FakeSpotify::new();
        real_episode(&fake);
        serve_feed(&fake);
        let dossier = settle(
            &fake,
            "off",
            PodcastEnrich::Off,
            true,
            None,
            EpisodeOverrides::default(),
        );
        assert!(dossier.warnings.is_empty(), "{:?}", dossier.warnings);
        assert_eq!(
            dossier.capture_summary(),
            "episode dossier (publisher feed lookup off)"
        );
        assert!(dossier.transcript.is_none());
        let requests = fake.requests();
        assert!(
            requests
                .iter()
                .all(|url| !url.contains("itunes.apple.com") && url != FEED_URL),
            "{requests:?}"
        );
    }

    #[test]
    fn an_exceeded_deadline_fails_open_with_one_warning() {
        let fake = FakeSpotify::new();
        real_episode(&fake);
        serve_feed(&fake);
        let dossier = prepare_with_deadline(
            &episode(None),
            &EpisodeOverrides::default(),
            "2026-11-21",
            &fake.scratch("deadline"),
            None,
            None,
            PodcastEnrich::Auto,
            true,
            Instant::now(),
        )
        .expect("dossier");
        assert_eq!(dossier.warnings, vec![deadline_warning()]);
        assert!(!dossier.notes_from_feed);
        assert!(dossier.transcript.is_none());
        assert!(dossier.link_times);
        let requests = fake.requests();
        assert!(
            requests.iter().all(|url| !url.contains("itunes.apple.com")),
            "{requests:?}"
        );
    }

    #[test]
    fn no_transcript_offer_warns_once() {
        let fake = FakeSpotify::new();
        real_episode(&fake);
        fake.serve("itunes", 200, "application/json", ITUNES.as_bytes());
        let stripped = FEED.replace(
            r#"<podcast:transcript url="https://api.substack.com/feed/podcast/transcript/225268679/5716fdd023036d0a64c51e7e8330539c.vtt" type="text/vtt"></podcast:transcript>"#,
            "",
        );
        fake.serve_url(
            FEED_URL,
            200,
            "application/rss+xml",
            stripped.as_bytes(),
        );
        let dossier = settle(
            &fake,
            "no-transcript",
            PodcastEnrich::Auto,
            true,
            None,
            EpisodeOverrides::default(),
        );
        assert_eq!(
            dossier.warnings,
            vec!["the publisher feed has no transcript for this episode"
                .to_string(),]
        );
        assert!(dossier.notes_from_feed);
        assert!(dossier.transcript.is_none());
    }
}
