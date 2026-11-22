//! Shared fake curl for Spotify episode CLI tests.
//!
//! A fake curl serves the trimmed Spotify fixtures by URL shape (episode
//! page, embed, oEmbed, cover). No test touches the network.

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
/// The exact `source_url` one legacy Spotify ref stores.
pub(crate) const LEGACY_SOURCE: &str = "https://open.spotify.com/episode/4UgVFWty2I8hGAgSvT7pjC?si=wMZ6dnubS8ib7JhCQ-6vLQ&sci=spotify%3Acard-config%3A2A4FnHWC7nYa7BcALmtBOK";

pub(crate) fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/spotify")
        .join(name)
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
case "$url" in
  *"/oembed?"*) surface=oembed ;;
  *"/embed/episode/"*) surface=embed ;;
  *"open.spotify.com/episode/"*) surface=page ;;
  *) surface=cover ;;
esac
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
        fake
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
