//! Offline library verdicts that agree exactly with create's dedupe.
//!
//! One `build_index` and one source scan per call. A missing ref dir
//! counts as empty; only a failed source scan is `unknown`.

use std::{collections::HashMap, path::Path};

use crate::native::{
    highlights_ref::{
        collect_intake_records, collect_recorded_source_urls, Config,
    },
    ref_library::{build_index, LibraryConfig},
};

use super::UrlIntent;

/// Library-only verdicts (this phase; `clipping` and `duplicate` land
/// with the job spool and the capture planner).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Verdict {
    InLibrary,
    InIntake,
    Legacy,
    NotFound,
    Unknown,
}

impl Verdict {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Verdict::InLibrary => "in_library",
            Verdict::InIntake => "in_intake",
            Verdict::Legacy => "legacy",
            Verdict::NotFound => "not_found",
            Verdict::Unknown => "unknown",
        }
    }
}

/// One offline verdict per intent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LibraryVerdict {
    pub(crate) verdict: Verdict,
    pub(crate) path: Option<String>,
    pub(crate) title: Option<String>,
    pub(crate) reading_state: Option<String>,
    pub(crate) message: Option<String>,
}

/// Offline verdicts for `intents` against `bob_dir`. Reads the library
/// once per batch and never touches the network.
pub(crate) fn library_verdicts(
    bob_dir: &Path,
    intents: &[&UrlIntent],
) -> Vec<LibraryVerdict> {
    let config = Config::for_vault(bob_dir);
    let recorded = match collect_recorded_source_urls(&config) {
        Ok(recorded) => recorded,
        Err(error) => {
            return intents
                .iter()
                .map(|_| LibraryVerdict {
                    verdict: Verdict::Unknown,
                    path: None,
                    title: None,
                    reading_state: None,
                    message: Some(error.to_string()),
                })
                .collect();
        }
    };
    let index = build_index(&LibraryConfig {
        bob_dir: config.bob_dir.clone(),
        ref_dir: config.ref_dir.clone(),
        xlib_dir: config.xlib_dir.clone(),
    })
    .ok();
    let rows: HashMap<&str, _> = index
        .as_ref()
        .map(|index| {
            index
                .rows
                .iter()
                .map(|row| (row.path.as_str(), row))
                .collect()
        })
        .unwrap_or_default();
    let intake_titles: HashMap<String, Option<String>> =
        collect_intake_records(&config.xlib_dir)
            .and_then(|result| result.ok())
            .map(|records| {
                records
                    .into_iter()
                    .map(|record| {
                        let relative = vault_relative(bob_dir, &record.path)
                            .unwrap_or_else(|| {
                                record.path.to_string_lossy().replace('\\', "/")
                            });
                        (relative, record.title)
                    })
                    .collect()
            })
            .unwrap_or_default();

    intents
        .iter()
        .map(|intent| {
            decide_one(
                bob_dir,
                intent,
                &recorded
                    .iter()
                    .filter(|hit| hit.dedupe_key == intent.dedupe_key)
                    .collect::<Vec<_>>(),
                &rows,
                &intake_titles,
            )
        })
        .collect()
}

fn decide_one(
    bob_dir: &Path,
    intent: &UrlIntent,
    hits: &[&crate::native::highlights_ref::RecordedSource],
    rows: &HashMap<&str, &crate::native::ref_library::RefRow>,
    intake_titles: &HashMap<String, Option<String>>,
) -> LibraryVerdict {
    let mut pdf_backed: Vec<&&crate::native::highlights_ref::RecordedSource> =
        hits.iter()
            .filter(|hit| hit.is_ref_note && hit.source_pdf.is_some())
            .collect();
    pdf_backed.sort_by(|a, b| a.path.cmp(&b.path));
    if let Some(hit) = pdf_backed.first() {
        let path = vault_relative(bob_dir, &hit.path);
        let (title, reading_state) = path
            .as_deref()
            .and_then(|path| rows.get(path))
            .map(|row| {
                (
                    Some(row.title.clone()),
                    reading_state_of(row.reading_state.as_str()),
                )
            })
            .unwrap_or((None, None));
        return LibraryVerdict {
            verdict: Verdict::InLibrary,
            path,
            title,
            reading_state,
            message: None,
        };
    }
    let mut intake: Vec<&&crate::native::highlights_ref::RecordedSource> =
        hits.iter().filter(|hit| !hit.is_ref_note).collect();
    intake.sort_by(|a, b| a.path.cmp(&b.path));
    if let Some(hit) = intake.first() {
        let path = vault_relative(bob_dir, &hit.path);
        let title = path
            .as_ref()
            .and_then(|path| intake_titles.get(path))
            .and_then(|title| title.clone());
        return LibraryVerdict {
            verdict: Verdict::InIntake,
            path,
            title,
            reading_state: None,
            message: None,
        };
    }
    let mut legacy: Vec<&&crate::native::highlights_ref::RecordedSource> = hits
        .iter()
        .filter(|hit| hit.is_ref_note && hit.source_pdf.is_none())
        .collect();
    legacy.sort_by(|a, b| a.path.cmp(&b.path));
    if let Some(hit) = legacy.first() {
        let path = vault_relative(bob_dir, &hit.path);
        let (title, reading_state) = path
            .as_deref()
            .and_then(|path| rows.get(path))
            .map(|row| {
                (
                    Some(row.title.clone()),
                    reading_state_of(row.reading_state.as_str()),
                )
            })
            .unwrap_or((None, None));
        return LibraryVerdict {
            verdict: Verdict::Legacy,
            path,
            title,
            reading_state,
            message: None,
        };
    }
    let _ = intent;
    LibraryVerdict {
        verdict: Verdict::NotFound,
        path: None,
        title: None,
        reading_state: None,
        message: None,
    }
}

fn reading_state_of(state: &str) -> Option<String> {
    (!state.is_empty() && state != "unknown").then(|| state.to_string())
}

fn vault_relative(bob_dir: &Path, path: &Path) -> Option<String> {
    path.strip_prefix(bob_dir)
        .ok()
        .map(|relative| relative.to_string_lossy().replace('\\', "/"))
}

#[cfg(test)]
mod tests {
    use std::{fs, path::PathBuf};

    use super::*;
    use crate::native::url_routing::classify_token;

    fn test_vault(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "bob-verdict-test-{}-{}-{name}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|duration| duration.as_nanos())
                .unwrap_or(0),
        ));
        fs::create_dir_all(dir.join("ref/papers")).expect("ref dir");
        fs::create_dir_all(dir.join("xlib/blogs")).expect("xlib dir");
        dir
    }

    fn intent(url: &str) -> UrlIntent {
        classify_token(url).expect("fixture URL classifies")
    }

    #[test]
    fn verdicts_cover_every_library_state() {
        let vault = test_vault("states");
        let pdf_url = "https://example.com/captured";
        let queued_url = "https://example.com/queued";
        let legacy_url = "https://example.com/legacy-note";
        let fresh_url = "https://example.com/fresh";

        // PDF-backed ref note: in_library.
        fs::write(
            vault.join("ref/papers/captured.md"),
            "---\ntitle: Captured Post\nstatus: ready\nsource_url: https://example.com/captured\nsource_pdf: lib/papers/captured.pdf\n---\n\n- [ ] ^ref\n",
        )
        .expect("write captured note");
        // Legacy-only note: legacy.
        fs::write(
            vault.join("ref/papers/legacy-note.md"),
            "---\ntitle: Legacy Note\nsource_url: https://example.com/legacy-note\n---\n\n# Legacy\n",
        )
        .expect("write legacy note");
        // Intake PDF marker: in_intake (a minimal PDF with a marker).
        write_intake_pdf(
            &vault.join("xlib/blogs/queued.pdf"),
            "- source_url: https://example.com/queued\n- title: Queued Post\n- status: ready\n- parent: obsidian_ref\n",
        );

        let captured = intent(pdf_url);
        let queued = intent(queued_url);
        let legacy = intent(legacy_url);
        let fresh = intent(fresh_url);
        let verdicts =
            library_verdicts(&vault, &[&captured, &queued, &legacy, &fresh]);
        assert_eq!(verdicts[0].verdict, Verdict::InLibrary);
        assert_eq!(verdicts[0].path.as_deref(), Some("ref/papers/captured.md"));
        assert_eq!(verdicts[0].title.as_deref(), Some("Captured Post"));
        assert_eq!(verdicts[1].verdict, Verdict::InIntake);
        assert_eq!(verdicts[1].path.as_deref(), Some("xlib/blogs/queued.pdf"));
        assert_eq!(verdicts[2].verdict, Verdict::Legacy);
        assert_eq!(
            verdicts[2].path.as_deref(),
            Some("ref/papers/legacy-note.md")
        );
        assert_eq!(verdicts[3].verdict, Verdict::NotFound);
    }

    #[test]
    fn missing_ref_dir_counts_as_empty() {
        let vault = std::env::temp_dir().join(format!(
            "bob-verdict-test-{}-{}-missing",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|duration| duration.as_nanos())
                .unwrap_or(0),
        ));
        fs::create_dir_all(&vault).expect("vault dir");
        let fresh = intent("https://example.com/fresh");
        let verdicts = library_verdicts(&vault, &[&fresh]);
        assert_eq!(verdicts[0].verdict, Verdict::NotFound);
    }

    /// Minimal one-page intake PDF: page 1 carries a standalone
    /// `/Text` note whose `Contents` is the marker, mirroring the
    /// stamp fixture in `highlights_ref`.
    fn write_intake_pdf(path: &Path, marker: &str) {
        use lopdf::{dictionary, Document, Object, Stream};

        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("create intake parent");
        }
        let mut document = Document::with_version("1.4");
        let pages_id = document.new_object_id();
        let content_id =
            document.add_object(Stream::new(dictionary! {}, Vec::new()));
        let marker_id = document.add_object(dictionary! {
            "Type" => "Annot",
            "Subtype" => "Text",
            "Rect" => vec![
                Object::Integer(0),
                Object::Integer(0),
                Object::Integer(24),
                Object::Integer(24),
            ],
            "Contents" => Object::string_literal(marker),
        });
        let page_id = document.add_object(dictionary! {
            "Type" => "Page",
            "Parent" => pages_id,
            "MediaBox" => vec![
                Object::Integer(0),
                Object::Integer(0),
                Object::Integer(612),
                Object::Integer(792),
            ],
            "Contents" => content_id,
            "Annots" => vec![Object::Reference(marker_id)],
        });
        document.set_object(
            pages_id,
            dictionary! {
                "Type" => "Pages",
                "Kids" => vec![Object::Reference(page_id)],
                "Count" => 1,
            },
        );
        let catalog_id = document.add_object(dictionary! {
            "Type" => "Catalog",
            "Pages" => pages_id,
        });
        document.trailer.set("Root", catalog_id);
        document.save(path).expect("save intake pdf");
    }
}
