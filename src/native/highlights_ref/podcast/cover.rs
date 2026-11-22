//! Episode cover art download.
//!
//! [`fetch_cover`] downloads the cover the metadata ladder chose, capped
//! at 5 MiB, and keeps it only when its magic bytes say JPEG, PNG, or
//! WebP (the server's `Content-Type` is not trusted). Any failure yields
//! no cover plus exactly one warning naming why: a dossier without art
//! is still a dossier.

use std::{
    fs,
    io::Read,
    path::{Path, PathBuf},
};

use super::super::clip_url::validate_and_clean;
use super::super::fetch::fetch_url_capped;

/// Refuse covers larger than this.
pub(crate) const COVER_MAX_BYTES: u64 = 5 * 1024 * 1024;

/// Per-request time cap for the cover download.
const COVER_MAX_TIME_SECS: u64 = 30;

/// The image formats a cover may be.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CoverFormat {
    Jpeg,
    Png,
    Webp,
}

impl CoverFormat {
    /// Recognize a format from a file's leading bytes.
    fn sniff(head: &[u8]) -> Option<Self> {
        if head.starts_with(&[0xFF, 0xD8, 0xFF]) {
            Some(Self::Jpeg)
        } else if head.starts_with(b"\x89PNG\r\n\x1a\n") {
            Some(Self::Png)
        } else if head.len() >= 12
            && &head[..4] == b"RIFF"
            && &head[8..12] == b"WEBP"
        {
            Some(Self::Webp)
        } else {
            None
        }
    }

    /// The file extension: `jpg`, `png`, or `webp`.
    pub(crate) fn extension(self) -> &'static str {
        match self {
            Self::Jpeg => "jpg",
            Self::Png => "png",
            Self::Webp => "webp",
        }
    }

    /// The MIME type.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn mime(self) -> &'static str {
        match self {
            Self::Jpeg => "image/jpeg",
            Self::Png => "image/png",
            Self::Webp => "image/webp",
        }
    }
}

/// A downloaded cover.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Cover {
    /// `<dest_dir>/cover.<ext>`.
    pub(crate) path: PathBuf,
    pub(crate) format: CoverFormat,
    pub(crate) bytes: u64,
}

/// What [`fetch_cover`] produced: a cover, or the one warning saying
/// why there is none.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum CoverOutcome {
    Fetched(Cover),
    Missing { warning: String },
}

impl CoverOutcome {
    pub(crate) fn cover(&self) -> Option<&Cover> {
        match self {
            Self::Fetched(cover) => Some(cover),
            Self::Missing { .. } => None,
        }
    }

    pub(crate) fn warning(&self) -> Option<&str> {
        match self {
            Self::Fetched(_) => None,
            Self::Missing { warning } => Some(warning),
        }
    }
}

/// Download the cover at `url` (when the ladder found one) into
/// `dest_dir` as `cover.<ext>`.
pub(crate) fn fetch_cover(
    url: Option<&str>,
    dest_dir: &Path,
    progress: Option<&dyn Fn(&str)>,
) -> CoverOutcome {
    let missing = |reason: String| CoverOutcome::Missing {
        warning: format!("cover art missing: {reason}"),
    };
    let Some(url) = url else {
        return missing(
            "Spotify's episode metadata names no image".to_string(),
        );
    };
    let host = match validate_and_clean(url) {
        Ok(web) => web.host,
        Err(error) => return missing(error.message),
    };
    if let Err(error) = fs::create_dir_all(dest_dir) {
        return missing(format!("create {}: {error}", dest_dir.display()));
    }
    let download = dest_dir.join("cover.download");
    let fetched = match fetch_url_capped(
        url,
        &download,
        COVER_MAX_TIME_SECS,
        COVER_MAX_BYTES,
        progress,
    ) {
        Ok(fetched) => fetched,
        Err(error) => {
            fs::remove_file(&download).ok();
            return missing(error.message().to_string());
        }
    };
    if fetched.status != 200 {
        fs::remove_file(&download).ok();
        return missing(format!("HTTP {} from {host}", fetched.status));
    }
    let Some(format) =
        read_head(&download).and_then(|head| CoverFormat::sniff(&head))
    else {
        fs::remove_file(&download).ok();
        return missing(format!("{host} sent neither JPEG, PNG, nor WebP"));
    };
    let path = dest_dir.join(format!("cover.{}", format.extension()));
    if let Err(error) = fs::rename(&download, &path) {
        fs::remove_file(&download).ok();
        return missing(format!("save {}: {error}", path.display()));
    }
    CoverOutcome::Fetched(Cover {
        path,
        format,
        bytes: fetched.bytes,
    })
}

fn read_head(path: &Path) -> Option<Vec<u8>> {
    let mut head = Vec::with_capacity(12);
    fs::File::open(path)
        .ok()?
        .take(12)
        .read_to_end(&mut head)
        .ok()?;
    Some(head)
}

#[cfg(test)]
mod tests {
    use super::super::test_support::FakeSpotify;
    use super::*;

    const URL: &str =
        "https://i.scdn.co/image/ab6765630000ba8a2927546cb7ca0ae0de915103";

    fn webp() -> Vec<u8> {
        let mut bytes = b"RIFF\x24\x00\x00\x00WEBPVP8 ".to_vec();
        bytes.extend_from_slice(&[0; 16]);
        bytes
    }

    #[test]
    fn covers_are_kept_by_magic_bytes() {
        let png = {
            let mut bytes = b"\x89PNG\r\n\x1a\n".to_vec();
            bytes.extend_from_slice(&[0; 24]);
            bytes
        };
        let jpeg = [0xFF, 0xD8, 0xFF, 0xE0, 0, 16, b'J', b'F', b'I', b'F'];
        for (body, format, content_type) in [
            (jpeg.to_vec(), CoverFormat::Jpeg, "image/jpeg"),
            (png, CoverFormat::Png, "image/png"),
            // The server's content type is not trusted.
            (webp(), CoverFormat::Webp, "application/octet-stream"),
        ] {
            let fake = FakeSpotify::new();
            fake.serve("cover", 200, content_type, &body);
            let dir = fake.scratch("assets");
            let outcome = fetch_cover(Some(URL), &dir, None);
            let cover = outcome.cover().expect("cover").clone();
            assert_eq!(cover.format, format);
            assert_eq!(
                cover.path,
                dir.join(format!("cover.{}", format.extension()))
            );
            assert_eq!(fs::read(&cover.path).expect("read cover"), body);
            assert_eq!(cover.bytes, body.len() as u64);
            assert!(!dir.join("cover.download").exists());
            assert_eq!(outcome.warning(), None);
            assert!(format.mime().starts_with("image/"));
        }
    }

    #[test]
    fn every_failure_is_one_warning_and_no_cover() {
        type Setup = Box<dyn Fn(&FakeSpotify)>;
        let cases: Vec<(&str, Setup, Option<&str>, &str)> = vec![
            (
                "no url",
                Box::new(|_| {}),
                None,
                "cover art missing: Spotify's episode metadata names no image",
            ),
            (
                "not an image",
                Box::new(|fake| {
                    fake.serve("cover", 200, "image/jpeg", b"<html>")
                }),
                Some(URL),
                "cover art missing: i.scdn.co sent neither JPEG, PNG, nor WebP",
            ),
            (
                "404",
                Box::new(|fake| fake.serve("cover", 404, "text/html", b"")),
                Some(URL),
                "cover art missing: HTTP 404 from i.scdn.co",
            ),
            (
                "private host",
                Box::new(|_| {}),
                Some("http://127.0.0.1/cover.jpg"),
                "cover art missing:",
            ),
        ];
        for (name, setup, url, expected) in cases {
            let fake = FakeSpotify::new();
            setup(&fake);
            let dir = fake.scratch("assets");
            let outcome = fetch_cover(url, &dir, None);
            assert_eq!(outcome.cover(), None, "{name}");
            let warning = outcome.warning().expect("warning");
            assert!(warning.starts_with(expected), "{name}: {warning}");
            assert!(!dir.join("cover.download").exists(), "{name}");
        }
    }

    #[test]
    fn a_cover_over_the_cap_is_refused() {
        let fake = FakeSpotify::new();
        let mut big = webp();
        big.resize((COVER_MAX_BYTES + 1) as usize, 0);
        fake.serve("cover", 200, "image/webp", &big);
        let dir = fake.scratch("assets");
        let outcome = fetch_cover(Some(URL), &dir, None);
        assert_eq!(outcome.cover(), None);
        let warning = outcome.warning().expect("warning");
        assert!(
            warning
                .starts_with("cover art missing: download larger than 5 MiB"),
            "{warning}"
        );
    }
}
