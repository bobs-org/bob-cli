//! The episode dossier builder, shared by `bob ref create` and URL
//! ingest.
//!
//! [`prepare`] climbs the metadata ladder, downloads the cover, segments
//! the show notes, and settles the reference fields (an explicit override
//! always wins). A dry run stops there. [`Dossier::render`] then writes
//! the `podcast_episode` `article.json` (cover under `assets/`) and
//! prints it through the adapter's offline `render` op.
//!
//! Nothing here prints. Every degradation becomes exactly one entry in
//! [`Dossier::warnings`] (a metadata fallback, a missing cover), and the
//! caller decides how to report it: create writes `warning:` lines,
//! while ingest routes them through its progress channel.

use std::{
    fs,
    path::{Path, PathBuf},
};

use serde_json::{json, Value};

use super::super::clip_adapter::{
    AdapterFailure, ClipAdapterClient, RenderRequest, TablesSettings,
};
use super::super::clip_url::title_stem;
use super::super::spotify::SpotifyEpisode;
use super::cover::{fetch_cover, CoverOutcome};
use super::metadata::{
    fetch_episode, EpisodeError, EpisodeField, EpisodeMetadata, MetadataSource,
};
use super::notes::{segment_notes, segment_text, NotesBlock, ShowNotes};

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
    /// lost, including missing show notes) and a missing cover.
    pub(crate) warnings: Vec<String>,
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
/// with one warning), and the segmented show notes.
///
/// `prefetched_page` is an already-downloaded 200 episode page (a
/// resolved short link's final body); `captured` is the capture date.
pub(crate) fn prepare(
    episode: &SpotifyEpisode,
    overrides: &EpisodeOverrides,
    captured: &str,
    scratch: &Path,
    progress: Option<&dyn Fn(&str)>,
    prefetched_page: Option<&Path>,
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
    Ok(Dossier {
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
        workdir,
    })
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
    /// `episode dossier · 16 chapters · show notes 512 words`.
    pub(crate) fn capture_summary(&self) -> String {
        let mut parts = vec!["episode dossier".to_string()];
        match self.chapter_count() {
            0 => {}
            1 => parts.push("1 chapter".to_string()),
            count => parts.push(format!("{count} chapters")),
        }
        if self.notes.is_empty() {
            parts.push("no show notes".to_string());
        } else {
            parts.push(format!("show notes {} words", self.notes.words));
        }
        parts.join(" · ")
    }

    /// The provenance footer: who supplied the details and the notes.
    fn provenance(&self) -> Vec<Value> {
        let details = self
            .metadata
            .source_of(EpisodeField::Title)
            .unwrap_or(MetadataSource::Page);
        let notes = if self.notes.is_empty() {
            None
        } else if self.metadata.notes_html.is_some() {
            self.metadata.source_of(EpisodeField::NotesHtml)
        } else {
            self.metadata.source_of(EpisodeField::NotesText)
        };
        let entry = |label: &str, source: MetadataSource| json!({"label": label, "source": provenance_source(source)});
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
                "chapter_links": true,
                "blocks": self.blocks(),
                "sponsors_html": self.notes.sponsors_html(),
                "provenance": self.provenance(),
                "transcript": Value::Null,
            },
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

    #[test]
    fn the_real_episode_settles_every_field_from_the_page() {
        let fake = FakeSpotify::new();
        real_episode(&fake);
        let scratch = fake.scratch("real");
        let dossier = prepare(
            &episode(Some(1235)),
            &EpisodeOverrides::default(),
            "2026-11-21",
            &scratch,
            None,
            None,
        )
        .expect("dossier");
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
        assert!(
            dossier
                .capture_summary()
                .starts_with("episode dossier · 16 chapters · show notes "),
            "{}",
            dossier.capture_summary()
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
        let scratch = fake.scratch("overrides");
        let overrides = EpisodeOverrides {
            title: Some("Harjot on software factories".to_string()),
            author: Some("Harjot Gill".to_string()),
            published: Some("2026-01-02".to_string()),
        };
        let dossier = prepare(
            &episode(None),
            &overrides,
            "2026-11-21",
            &scratch,
            None,
            None,
        )
        .expect("dossier");
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
        let scratch = fake.scratch("non-ascii");
        let overrides = EpisodeOverrides {
            title: Some("ソフトウェア工場".to_string()),
            ..EpisodeOverrides::default()
        };
        let dossier = prepare(
            &episode(None),
            &overrides,
            "2026-11-21",
            &scratch,
            None,
            None,
        )
        .expect("dossier");
        assert_eq!(dossier.stem(), "spotify_episode_4ugvfwty2i8hgagsvt7pjc");
    }

    #[test]
    fn a_missing_cover_is_one_warning() {
        let fake = FakeSpotify::new();
        fake.serve("page", 200, "text/html", PAGE.as_bytes());
        fake.serve("cover", 404, "text/html", b"");
        let scratch = fake.scratch("no-cover");
        let dossier = prepare(
            &episode(None),
            &EpisodeOverrides::default(),
            "2026-11-21",
            &scratch,
            None,
            None,
        )
        .expect("dossier");
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
        let scratch = fake.scratch("oembed");
        let dossier = prepare(
            &episode(None),
            &EpisodeOverrides::default(),
            "2026-11-21",
            &scratch,
            None,
            None,
        )
        .expect("dossier");
        assert_eq!(dossier.warnings.len(), 1, "{:?}", dossier.warnings);
        assert!(dossier.warnings[0].contains("show notes"));
        assert_eq!(dossier.author, None);
        assert_eq!(
            dossier.capture_summary(),
            "episode dossier · no show notes"
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
        let scratch = fake.scratch("embed");
        let dossier = prepare(
            &episode(None),
            &EpisodeOverrides::default(),
            "2026-11-21",
            &scratch,
            None,
            None,
        )
        .expect("dossier");
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
}
