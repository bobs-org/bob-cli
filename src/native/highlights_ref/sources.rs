//! Shared dedupe over already-captured sources for Highlights targets.
//!
//! Every URL target dedupes before any fetch: the key is syntactic (see
//! [`dedupe_key_for`](super::clip_url::dedupe_key_for)), so no network is
//! needed. Recorded sources are ref-note frontmatter `source_url` **and
//! legacy `url`** keys plus queued intake-PDF markers carrying either key,
//! so the existing paper refs (which use the ad hoc `url:` key) take part
//! in dedupe instead of being duplicated. Moved out of `clip.rs` so the
//! `create` PDF routes share it.

use std::{
    fs,
    path::{Path, PathBuf},
};

use super::{
    clip_url::validate_and_clean,
    frontmatter::{parse_frontmatter_entry, split_frontmatter},
    marker::{parse_marker, read_pdf_marker},
    model::Config,
    stamp::normalize_lexically,
    CommandError,
};

/// A dedupe failure: the message goes after `error:`, the hint after
/// `hint:`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct SourcesError {
    pub(super) message: String,
    pub(super) hint: Option<String>,
}

impl SourcesError {
    pub(super) fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            hint: None,
        }
    }

    pub(super) fn with_hint(mut self, hint: impl Into<String>) -> Self {
        self.hint = Some(hint.into());
        self
    }
}

impl From<CommandError> for SourcesError {
    fn from(error: CommandError) -> Self {
        Self::new(error.message)
    }
}

/// Every source URL already recorded: ref-note frontmatter and queued
/// intake-PDF markers, paired with their dedupe keys.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct RecordedSource {
    pub(super) dedupe_key: String,
    pub(super) path: PathBuf,
    pub(super) is_ref_note: bool,
    /// The note's `source_pdf`, as written (vault-relative), if any.
    pub(super) source_pdf: Option<String>,
    /// Whether the ref note carries an `audio` field.
    pub(super) has_audio: bool,
}

pub(super) fn collect_recorded_source_urls(
    config: &Config,
) -> std::result::Result<Vec<RecordedSource>, SourcesError> {
    let mut recorded = Vec::new();
    if config.ref_dir.is_dir() {
        collect_ref_note_sources(&config.ref_dir, &mut recorded)?;
    }
    if config.xlib_dir.is_dir() {
        collect_intake_sources(&config.xlib_dir, &mut recorded)?;
    }
    Ok(recorded)
}

fn collect_ref_note_sources(
    dir: &Path,
    recorded: &mut Vec<RecordedSource>,
) -> std::result::Result<(), SourcesError> {
    let entries = fs::read_dir(dir).map_err(|error| {
        SourcesError::new(format!("scan {}: {error}", dir.display()))
    })?;
    for entry in entries {
        let entry = entry.map_err(|error| {
            SourcesError::new(format!("scan {}: {error}", dir.display()))
        })?;
        let path = entry.path();
        let file_type = entry.file_type().map_err(|error| {
            SourcesError::new(format!("stat {}: {error}", path.display()))
        })?;
        if file_type.is_dir() {
            if path
                .file_name()
                .is_none_or(|name| name != std::ffi::OsStr::new(".git"))
            {
                collect_ref_note_sources(&path, recorded)?;
            }
            continue;
        }
        if !file_type.is_file()
            || !path
                .extension()
                .and_then(std::ffi::OsStr::to_str)
                .is_some_and(|extension| extension.eq_ignore_ascii_case("md"))
        {
            continue;
        }
        let contents = fs::read_to_string(&path).map_err(|error| {
            SourcesError::new(format!("read note {}: {error}", path.display()))
        })?;
        let Some((frontmatter, _)) = split_frontmatter(&contents) else {
            continue;
        };
        let mut urls = Vec::new();
        let mut source_pdf = None;
        let mut has_audio = false;
        for raw in frontmatter {
            let entry = parse_frontmatter_entry(&raw);
            let key = entry.key.as_deref().unwrap_or_default();
            let value = entry
                .value
                .as_ref()
                .and_then(|value| value.as_string().map(str::to_string));
            match key {
                "source_url" | "url" => {
                    if let Some(url) = value {
                        urls.push(url);
                    }
                }
                "source_pdf" => {
                    if source_pdf.is_none() {
                        source_pdf = value;
                    }
                }
                "audio" => {
                    if value.is_some_and(|value| !value.is_empty()) {
                        has_audio = true;
                    }
                }
                _ => {}
            }
        }
        for url in urls {
            if let Ok(cleaned) = validate_and_clean(&url) {
                recorded.push(RecordedSource {
                    dedupe_key: cleaned.dedupe_key,
                    path: path.clone(),
                    is_ref_note: true,
                    source_pdf: source_pdf.clone(),
                    has_audio,
                });
            }
        }
    }
    Ok(())
}

fn collect_intake_sources(
    dir: &Path,
    recorded: &mut Vec<RecordedSource>,
) -> std::result::Result<(), SourcesError> {
    let entries = fs::read_dir(dir).map_err(|error| {
        SourcesError::new(format!("scan {}: {error}", dir.display()))
    })?;
    for entry in entries {
        let entry = entry.map_err(|error| {
            SourcesError::new(format!("scan {}: {error}", dir.display()))
        })?;
        let path = entry.path();
        let file_type = entry.file_type().map_err(|error| {
            SourcesError::new(format!("stat {}: {error}", path.display()))
        })?;
        if file_type.is_dir() {
            if path
                .file_name()
                .is_none_or(|name| name != std::ffi::OsStr::new(".git"))
            {
                collect_intake_sources(&path, recorded)?;
            }
            continue;
        }
        if !file_type.is_file()
            || !path
                .extension()
                .and_then(std::ffi::OsStr::to_str)
                .is_some_and(|extension| extension.eq_ignore_ascii_case("pdf"))
        {
            continue;
        }
        let Ok(marker) = read_pdf_marker(&path) else {
            continue;
        };
        let Ok(projection) = parse_marker(&marker.contents) else {
            continue;
        };
        for key in ["source_url", "url"] {
            if let Some(url) =
                projection.get(key).and_then(|value| value.as_string())
                && let Ok(cleaned) = validate_and_clean(&url)
            {
                recorded.push(RecordedSource {
                    dedupe_key: cleaned.dedupe_key,
                    path: path.clone(),
                    is_ref_note: false,
                    source_pdf: None,
                    has_audio: false,
                });
            }
        }
    }
    Ok(())
}

pub(super) fn check_dedupe(
    recorded: &[RecordedSource],
    dedupe_key: &str,
    planned_target: Option<&Path>,
    force: bool,
) -> std::result::Result<(), SourcesError> {
    for hit in recorded {
        if hit.dedupe_key != dedupe_key {
            continue;
        }
        if hit.is_ref_note {
            return Err(SourcesError::new(format!(
                "already captured as {} (source_url {})",
                hit.path.display(),
                dedupe_key,
            ))
            .with_hint("open the note, or delete it to recapture"));
        }
        let same_target = planned_target.is_some_and(|target| {
            normalize_lexically(target) == normalize_lexically(&hit.path)
        });
        if same_target && force {
            continue;
        }
        if planned_target.is_none() && force {
            // The stem is only known after capture; the post-capture
            // check with the final target decides.
            continue;
        }
        return Err(SourcesError::new(format!(
            "already queued in {} (source_url {})",
            hit.path.display(),
            dedupe_key,
        ))
        .with_hint(
            "pass --force to overwrite the same intake target, or delete the queued PDF to recapture",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "bob-sources-test-{}-{}-{name}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|duration| duration.as_nanos())
                .unwrap_or(0),
        ));
        fs::create_dir_all(&dir).expect("create sources test dir");
        dir
    }

    fn test_config(root: &Path) -> Config {
        Config {
            bob_dir: root.to_path_buf(),
            lib_dir: root.join("lib"),
            ref_dir: root.join("ref"),
            xlib_dir: root.join("xlib"),
        }
    }

    #[test]
    fn legacy_url_notes_are_recorded_with_source_pdf_and_audio() {
        let root = test_dir("legacy-url");
        let config = test_config(&root);
        fs::create_dir_all(config.ref_dir.join("papers"))
            .expect("create ref dir");
        fs::write(
            config.ref_dir.join("papers/ea_graph.md"),
            "---\ntitle: EA-Graph\nurl: https://arxiv.org/pdf/2608.04278\nsource_pdf: lib/papers/ea_graph.pdf\naudio: \"[[lib/papers/ea_graph.mp3]]\"\n---\n\n# EA-Graph\n",
        )
        .expect("write legacy note");
        // A note with an unrelated key takes no part in dedupe.
        fs::write(
            config.ref_dir.join("papers/other.md"),
            "---\ntitle: Other\n---\n\n# Other\n",
        )
        .expect("write plain note");

        let recorded =
            collect_recorded_source_urls(&config).expect("collect sources");
        assert_eq!(recorded.len(), 1, "only the legacy-url note records");
        let hit = &recorded[0];
        assert_eq!(
            hit.dedupe_key, "https://arxiv.org/abs/2608.04278",
            "the pdf spelling keys as its abs identity"
        );
        assert_eq!(hit.source_pdf.as_deref(), Some("lib/papers/ea_graph.pdf"));
        assert!(hit.has_audio, "the audio field is recorded");
        assert!(hit.is_ref_note);

        // Every spelling of the same paper hits the legacy note.
        for raw in [
            "https://arxiv.org/abs/2608.04278",
            "https://arxiv.org/abs/2608.04278v2",
            "https://arxiv.org/html/2608.04278v1/",
        ] {
            let cleaned =
                validate_and_clean(raw).expect("clean arXiv spelling");
            let error =
                check_dedupe(&recorded, &cleaned.dedupe_key, None, false)
                    .expect_err("legacy url must refuse every spelling");
            assert!(
                error.message.contains("already captured as"),
                "unexpected message: {}",
                error.message
            );
        }
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn non_arxiv_keys_pass_through_unchanged() {
        let recorded = vec![RecordedSource {
            dedupe_key: "https://example.com/a?a=1&b=2".to_string(),
            path: PathBuf::from("ref/blogs/existing.md"),
            is_ref_note: true,
            source_pdf: None,
            has_audio: false,
        }];
        check_dedupe(&recorded, "https://example.com/a?a=1&b=2", None, false)
            .expect_err("an exact key still refuses");
        check_dedupe(&recorded, "https://example.com/other", None, false)
            .expect("a different key passes");
    }
}
