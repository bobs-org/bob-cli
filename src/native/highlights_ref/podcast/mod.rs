//! Podcast episode references: browserless Spotify episode metadata
//! ([`metadata`]), show-notes segmentation ([`notes`]), cover art
//! ([`cover`]), the publisher feed match ([`feed`]), publisher transcript
//! parsing ([`transcript`]), and the episode dossier builder
//! ([`dossier`]) that `bob ref create` (and URL ingest) print episodes
//! through.
//!
//! Every network step goes through [`super::fetch`], so each hop is
//! validated and pinned like every other Highlights fetch, and every
//! parser treats Spotify's undocumented page shapes defensively: an
//! unknown shape yields nothing and never panics.

pub(crate) mod cover;
pub(crate) mod dossier;
pub(crate) mod feed;
pub(crate) mod metadata;
pub(crate) mod notes;
pub(crate) mod transcript;

/// Remove the invisible characters Spotify descriptions carry (U+2060
/// word joiner, U+200B zero-width space, U+FEFF byte-order mark).
pub(crate) fn strip_invisibles(text: &str) -> String {
    text.chars()
        .filter(|c| !matches!(c, '\u{2060}' | '\u{200B}' | '\u{FEFF}'))
        .collect()
}

/// [`strip_invisibles`], then collapse every whitespace run (including
/// no-break spaces) to one space and trim. `None` when nothing is left.
pub(crate) fn clean_line(text: &str) -> Option<String> {
    let cleaned = strip_invisibles(text)
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    (!cleaned.is_empty()).then_some(cleaned)
}

#[cfg(test)]
pub(crate) mod test_support {
    //! A fake `curl` that serves canned Spotify responses by URL shape,
    //! and any other response by its exact URL.

    use std::{
        fs,
        path::{Path, PathBuf},
    };

    use crate::native::env::TestEnvGuard;

    /// One canned surface: `page`, `embed`, `oembed`, `itunes`, or
    /// `cover` (any other URL), unless [`Self::serve_url`] claimed the
    /// exact URL.
    pub(crate) struct FakeSpotify {
        pub(crate) root: tempfile::TempDir,
        _guard: TestEnvGuard,
    }

    impl FakeSpotify {
        /// A fake curl whose every surface answers 404 until
        /// [`Self::serve`] sets it.
        pub(crate) fn new() -> Self {
            let root = tempfile::tempdir().expect("fake curl root");
            let script = write_fake_curl(root.path());
            let guard = TestEnvGuard::set(&[
                ("FAKE_CURL_ROOT", Some(root.path().as_os_str())),
                (
                    super::super::fetch::ENV_CURL_OVERRIDE,
                    Some(script.as_os_str()),
                ),
                (
                    super::super::fetch::ENV_RESOLVE_OVERRIDE,
                    Some(std::ffi::OsStr::new("*=203.0.113.1")),
                ),
            ]);
            Self {
                root,
                _guard: guard,
            }
        }

        /// Serve `body` with `status` and `content_type` for `surface`.
        pub(crate) fn serve(
            &self,
            surface: &str,
            status: u16,
            content_type: &str,
            body: &[u8],
        ) {
            let root = self.root.path();
            fs::write(
                root.join(format!("{surface}.status")),
                status.to_string(),
            )
            .expect("write fake status");
            fs::write(root.join(format!("{surface}.type")), content_type)
                .expect("write fake content type");
            fs::write(root.join(format!("{surface}.body")), body)
                .expect("write fake body");
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

        /// Make `surface` fail the way curl exits with `code`.
        pub(crate) fn fail(&self, surface: &str, code: i32) {
            fs::write(
                self.root.path().join(format!("{surface}.exit")),
                code.to_string(),
            )
            .expect("write fake exit");
        }

        /// The requested URLs, in order.
        pub(crate) fn requests(&self) -> Vec<String> {
            fs::read_to_string(self.root.path().join("requests.log"))
                .unwrap_or_default()
                .lines()
                .map(str::to_string)
                .collect()
        }

        /// A fresh scratch directory under the fake root.
        pub(crate) fn scratch(&self, name: &str) -> PathBuf {
            let dir = self.root.path().join("scratch").join(name);
            fs::create_dir_all(&dir).expect("create scratch");
            dir
        }
    }

    /// The surface name the fake curl uses for an exact URL: `url-`
    /// plus every byte that is not ASCII alphanumeric turned into `_`
    /// (the script's `tr -c 'A-Za-z0-9' '_'`).
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

    fn write_fake_curl(dir: &Path) -> PathBuf {
        let path = dir.join("fake-curl.sh");
        let script = r#"#!/bin/sh
# Fake curl for podcast tests: picks the exact-URL surface when one is
# set, else a surface by URL shape, logs the URL, and serves
# $ROOT/<surface>.{status,type,body} (404 when unset).
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
if [ -f "$ROOT/$surface.exit" ]; then
  printf 'curl: fake failure\n' >&2
  exit "$(cat "$ROOT/$surface.exit")"
fi
status=404
type="text/html"
if [ -f "$ROOT/$surface.status" ]; then status="$(cat "$ROOT/$surface.status")"; fi
if [ -f "$ROOT/$surface.type" ]; then type="$(cat "$ROOT/$surface.type")"; fi
if [ -f "$ROOT/$surface.body" ]; then cp "$ROOT/$surface.body" "$dest"; else : > "$dest"; fi
printf '%s\n%s\n\n' "$status" "$type"
"#;
        crate::native::test_stub::write_executable_stub(&path, script);
        path
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invisibles_are_stripped_and_lines_collapse() {
        assert_eq!(
            strip_invisibles("a\u{2060}b\u{200B}c\u{FEFF}d"),
            "abcd".to_string()
        );
        assert_eq!(
            clean_line("  Software\u{2060} factories \n\u{a0} and\tmore "),
            Some("Software factories and more".to_string())
        );
        assert_eq!(clean_line("\u{2060}\u{2060} \n"), None);
    }
}
