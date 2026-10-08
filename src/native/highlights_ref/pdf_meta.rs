//! Plausible PDF Info title/author reads for Highlights targets.
//!
//! Remote PDFs often carry junk Info metadata (an exporter default, the
//! filename, or nothing at all), so the raw `/Title` and `/Author` are
//! filtered before they can become a ref-note title. Moved out of
//! `clip.rs` so every PDF route shares the same plausibility rule.

use std::path::Path;

/// Read the PDF Info `/Title` and `/Author`, returning each only when it
/// is plausible: trimmed and whitespace-collapsed, at least 3 characters
/// with a letter, not equal to the target `stem` (case-insensitive), not
/// a bare filename, not an exporter default, and not `untitled`/`title`.
pub(super) fn pdf_info_metadata(
    path: &Path,
    stem: &str,
) -> (Option<String>, Option<String>) {
    let document = match lopdf::Document::load(path) {
        Ok(document) => document,
        Err(_) => return (None, None),
    };
    let info = document.trailer.get(b"Info").ok();
    let dict = match info {
        Some(lopdf::Object::Reference(id)) => document.get_dictionary(*id).ok(),
        Some(lopdf::Object::Dictionary(dict)) => Some(dict),
        _ => None,
    };
    let raw = |key: &[u8]| -> Option<String> {
        let object = dict?.get(key).ok()?;
        lopdf::decode_text_string(object).ok()
    };
    let title = raw(b"Title").and_then(|title| plausible_title(&title, stem));
    let author = raw(b"Author").and_then(|author| {
        let normalized = normalize(&author);
        (!normalized.is_empty()).then_some(normalized)
    });
    (title, author)
}

/// Collapse whitespace after trimming.
fn normalize(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Read the raw PDF Info `/Title` without any plausibility filtering:
/// trimmed and whitespace-collapsed, returned even when it equals the
/// stem. Used for local-route default-target identity, where the planned
/// title must match the occupant's stamped title exactly.
pub(super) fn raw_info_title(path: &Path) -> Option<String> {
    let document = match lopdf::Document::load(path) {
        Ok(document) => document,
        Err(_) => return None,
    };
    let info = document.trailer.get(b"Info").ok();
    let dict = match info {
        Some(lopdf::Object::Reference(id)) => {
            document.get_dictionary(*id).ok()?
        }
        Some(lopdf::Object::Dictionary(dict)) => dict,
        _ => return None,
    };
    let object = dict.get(b"Title").ok()?;
    let raw = lopdf::decode_text_string(object).ok()?;
    let title = normalize(&raw);
    (!title.is_empty()).then_some(title)
}

/// File extensions that mark a title as a bare filename.
const FILENAME_EXTENSIONS: &[&str] =
    &[".pdf", ".doc", ".docx", ".tex", ".dvi", ".ps", ".pages"];

fn plausible_title(raw: &str, stem: &str) -> Option<String> {
    let title = normalize(raw);
    if title.chars().count() < 3
        || !title.chars().any(|char| char.is_alphabetic())
        || title.eq_ignore_ascii_case(stem)
        || FILENAME_EXTENSIONS
            .iter()
            .any(|extension| title.to_lowercase().ends_with(extension))
        || title.starts_with("Microsoft Word - ")
        || title.starts_with("Microsoft PowerPoint - ")
        || title.eq_ignore_ascii_case("untitled")
        || title.eq_ignore_ascii_case("title")
    {
        return None;
    }
    Some(title)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    /// Write a one-page PDF carrying the given Info title/author.
    fn write_info_pdf(path: &Path, title: Option<&str>, author: Option<&str>) {
        use lopdf::{dictionary, Document, Object, Stream};

        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("create info PDF parent");
        }
        let mut doc = Document::with_version("1.4");
        let pages_id = doc.new_object_id();
        let content_id =
            doc.add_object(Stream::new(dictionary! {}, Vec::new()));
        let page_id = doc.add_object(dictionary! {
            "Type" => "Page",
            "Parent" => pages_id,
            "MediaBox" => vec![
                Object::Integer(0),
                Object::Integer(0),
                Object::Integer(612),
                Object::Integer(792),
            ],
            "Contents" => content_id,
        });
        doc.set_object(
            pages_id,
            dictionary! {
                "Type" => "Pages",
                "Kids" => vec![Object::Reference(page_id)],
                "Count" => 1,
            },
        );
        let catalog_id = doc.add_object(dictionary! {
            "Type" => "Catalog",
            "Pages" => pages_id,
        });
        doc.trailer.set("Root", catalog_id);
        let mut info = dictionary! {};
        if let Some(title) = title {
            info.set("Title", Object::string_literal(title));
        }
        if let Some(author) = author {
            info.set("Author", Object::string_literal(author));
        }
        if !info.is_empty() {
            let info_id = doc.add_object(Object::Dictionary(info));
            doc.trailer.set("Info", info_id);
        }
        doc.save(path).expect("write info PDF");
    }

    fn test_dir(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "bob-pdf-meta-test-{}-{}-{name}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|duration| duration.as_nanos())
                .unwrap_or(0),
        ));
        fs::create_dir_all(&dir).expect("create pdf-meta test dir");
        dir
    }

    #[test]
    fn info_metadata_accepts_plausible_values() {
        let dir = test_dir("plausible");
        let path = dir.join("paper.pdf");
        write_info_pdf(
            &path,
            Some("  Attention   Is All You Need  "),
            Some("Ashish Vaswani"),
        );
        let (title, author) = pdf_info_metadata(&path, "something_else");
        assert_eq!(title.as_deref(), Some("Attention Is All You Need"));
        assert_eq!(author.as_deref(), Some("Ashish Vaswani"));
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn info_titles_reject_junk() {
        let dir = test_dir("junk");
        // A title equal to the stem and an exporter default are both
        // rejected; the author is unaffected by the title rule.
        let cases: &[(&str, &str)] = &[
            ("my_paper", "my_paper"),
            ("MY_PAPER", "my_paper"),
            ("Microsoft Word - draft.docx", "draft"),
            ("Microsoft PowerPoint - slides", "slides"),
            ("Untitled", "notes"),
            ("TITLE", "notes"),
            ("report.PDF", "notes"),
            ("ab", "notes"),
            ("12345", "notes"),
            ("", "notes"),
        ];
        for (index, (title, stem)) in cases.iter().enumerate() {
            let path = dir.join(format!("case-{index}.pdf"));
            write_info_pdf(&path, Some(title), Some("Jane Doe"));
            let (kept, author) = pdf_info_metadata(&path, stem);
            assert_eq!(kept, None, "junk title must be rejected: {title:?}");
            assert_eq!(
                author.as_deref(),
                Some("Jane Doe"),
                "the author rule only normalizes"
            );
        }
        // No Info dictionary at all reads as no metadata.
        let bare = dir.join("bare.pdf");
        write_info_pdf(&bare, None, None);
        assert_eq!(pdf_info_metadata(&bare, "bare"), (None, None));
        fs::remove_dir_all(&dir).ok();
    }
}
