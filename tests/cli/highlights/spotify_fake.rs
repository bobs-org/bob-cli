//! Shared fake curl for Spotify episode CLI tests.
//!
//! A fake curl serves the trimmed Spotify fixtures by URL shape (episode
//! page, embed, oEmbed, iTunes, cover) and exact URLs for the publisher
//! feed and transcript. No test touches the network.

use crate::support::*;
use std::fs;
use std::path::{Path, PathBuf};

pub(crate) const ID: &str = "4UgVFWty2I8hGAgSvT7pjC";
pub(crate) const CANONICAL: &str =
    "https://open.spotify.com/episode/4UgVFWty2I8hGAgSvT7pjC";
pub(crate) const TITLE: &str =
    "Software factories and code reviews with Harjot Gill";
pub(crate) const STEM: &str =
    "software_factories_and_code_reviews_with_harjot_gill";
pub(crate) const SHOW: &str = "The Pragmatic Engineer";
pub(crate) const PUBLISHED: &str = "2026-11-18";
pub(crate) const FEED_URL: &str =
    "https://api.substack.com/feed/podcast/458709.rss";
pub(crate) const TRANSCRIPT_URL: &str = "https://api.substack.com/feed/podcast/transcript/225268679/5716fdd023036d0a64c51e7e8330539c.vtt";
/// The exact `source_url` one legacy Spotify ref stores.
pub(crate) const LEGACY_SOURCE: &str = "https://open.spotify.com/episode/4UgVFWty2I8hGAgSvT7pjC?si=wMZ6dnubS8ib7JhCQ-6vLQ&sci=spotify%3Acard-config%3A2A4FnHWC7nYa7BcALmtBOK";

pub(crate) fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/spotify")
        .join(name)
}

pub(crate) fn podcast_fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/podcast")
        .join(name)
}

/// The trimmed VTT plus a last cue past half the episode, so the
/// duration sanity check accepts it.
pub(crate) fn dossier_vtt() -> Vec<u8> {
    let mut body =
        fs::read(podcast_fixture("pragmatic_engineer_transcript.vtt"))
            .expect("read VTT fixture");
    body.extend_from_slice(
        b"\n61\n01:20:00.000 --> 01:20:04.000\n<v SPEAKER_01>That's the episode.\n",
    );
    body
}

/// The surface name the fake curl uses for an exact URL.
pub(crate) fn url_surface(url: &str) -> String {
    let key: String = url
        .bytes()
        .map(|byte| {
            if byte.is_ascii_alphanumeric() {
                byte as char
            } else {
                '_'
            }
        })
        .collect();
    format!("url-{key}")
}

/// A fake curl serving `$FAKE_CURL_ROOT/<surface>.{status,type,body}` by
/// URL shape (404 when unset) and logging every URL.
pub(crate) struct FakeSpotify {
    pub(crate) root: PathBuf,
    pub(crate) curl: PathBuf,
}

impl FakeSpotify {
    pub(crate) fn new(temp: &TempDir) -> Self {
        let root = temp.path().join("spotify");
        fs::create_dir_all(&root).expect("create fake spotify root");
        let curl = root.join("fake-curl.sh");
        write_executable(
            &curl,
            r#"#!/bin/sh
ROOT="$FAKE_CURL_ROOT"
dest=""
url=""
prev=""
for arg in "$@"; do
  if [ "$prev" = "-o" ]; then dest="$arg"; fi
  prev="$arg"
  url="$arg"
done
printf '%s\n' "$url" >> "$ROOT/requests.log"
key="url-$(printf '%s' "$url" | tr -c 'A-Za-z0-9' '_')"
if [ -f "$ROOT/$key.status" ] || [ -f "$ROOT/$key.exit" ]; then
  surface="$key"
else
  case "$url" in
    *"/oembed?"*) surface=oembed ;;
    *"/embed/episode/"*) surface=embed ;;
    *"open.spotify.com/episode/"*) surface=page ;;
    *"itunes.apple.com/search?"*) surface=itunes ;;
    *) surface=cover ;;
  esac
fi
status=404
type="text/html"
if [ -f "$ROOT/$surface.status" ]; then status="$(cat "$ROOT/$surface.status")"; fi
if [ -f "$ROOT/$surface.type" ]; then type="$(cat "$ROOT/$surface.type")"; fi
if [ -f "$ROOT/$surface.body" ]; then cp "$ROOT/$surface.body" "$dest"; else : > "$dest"; fi
printf '%s\n%s\n\n' "$status" "$type"
"#,
        );
        let fake = Self { root, curl };
        fake.serve_file(
            "page",
            200,
            "text/html",
            &fixture("episode_page.html"),
        );
        fake.serve_file(
            "embed",
            200,
            "text/html",
            &fixture("episode_embed.html"),
        );
        fake.serve_file(
            "oembed",
            200,
            "application/json",
            &fixture("episode_oembed.json"),
        );
        let mut jpeg = vec![0xFF, 0xD8, 0xFF, 0xE0, 0, 16, b'J', b'F'];
        jpeg.extend_from_slice(&[0; 24]);
        fake.serve("cover", 200, "image/jpeg", &jpeg);
        fake.serve_file(
            "itunes",
            200,
            "application/json",
            &podcast_fixture("itunes_pragmatic_engineer.json"),
        );
        fake.serve_url(
            FEED_URL,
            200,
            "application/rss+xml",
            &fs::read(podcast_fixture("feed_pragmatic_engineer.rss"))
                .expect("read feed fixture"),
        );
        fake.serve_url(TRANSCRIPT_URL, 200, "text/vtt", &dossier_vtt());
        fake
    }

    /// Serve `body` for exactly `url`, ahead of every URL shape.
    pub(crate) fn serve_url(
        &self,
        url: &str,
        status: u16,
        content_type: &str,
        body: &[u8],
    ) {
        self.serve(&url_surface(url), status, content_type, body);
    }

    pub(crate) fn serve(
        &self,
        surface: &str,
        status: u16,
        content_type: &str,
        body: &[u8],
    ) {
        fs::write(
            self.root.join(format!("{surface}.status")),
            status.to_string(),
        )
        .expect("write fake status");
        fs::write(self.root.join(format!("{surface}.type")), content_type)
            .expect("write fake type");
        fs::write(self.root.join(format!("{surface}.body")), body)
            .expect("write fake body");
    }

    pub(crate) fn serve_file(
        &self,
        surface: &str,
        status: u16,
        content_type: &str,
        path: &Path,
    ) {
        self.serve(
            surface,
            status,
            content_type,
            &fs::read(path).expect("read fixture"),
        );
    }

    pub(crate) fn requests(&self) -> Vec<String> {
        fs::read_to_string(self.root.join("requests.log"))
            .unwrap_or_default()
            .lines()
            .map(str::to_string)
            .collect()
    }
}
