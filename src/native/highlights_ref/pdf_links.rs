//! Match recorded URI links to clickable PDF annotations.

use std::path::Path;

use lopdf::{Document, Object};

use super::code_catalog::CatalogLink;

#[derive(Debug, Clone, PartialEq)]
struct UriAnnotation {
    page: u32,
    annotation_index: usize,
    uri: String,
    left: Option<f64>,
    bottom: Option<f64>,
}

fn resolve_object(document: &Document, object: &Object) -> Option<Object> {
    match object {
        Object::Reference(id) => document.get_object(*id).ok().cloned(),
        other => Some(other.clone()),
    }
}

fn dictionary(
    document: &Document,
    object: &Object,
) -> Option<lopdf::Dictionary> {
    match resolve_object(document, object)? {
        Object::Dictionary(dictionary) => Some(dictionary),
        _ => None,
    }
}

fn number(object: &Object) -> Option<f64> {
    match object {
        Object::Integer(value) => Some(*value as f64),
        Object::Real(value) => Some(f64::from(*value)),
        _ => None,
    }
}

fn annotation_uri(document: &Document, annotation: &Object) -> Option<String> {
    let annotation = dictionary(document, annotation)?;
    if !matches!(annotation.get(b"Subtype"), Ok(Object::Name(name)) if name == b"Link")
    {
        return None;
    }
    let action = dictionary(document, annotation.get(b"A").ok()?)?;
    if !matches!(action.get(b"S"), Ok(Object::Name(name)) if name == b"URI") {
        return None;
    }
    let uri = resolve_object(document, action.get(b"URI").ok()?)?;
    let Object::String(bytes, _) = uri else {
        return None;
    };
    Some(String::from_utf8_lossy(&bytes).into_owned())
}

fn annotation_position(
    document: &Document,
    annotation: &Object,
) -> (Option<f64>, Option<f64>) {
    let Some(dictionary) = dictionary(document, annotation) else {
        return (None, None);
    };
    let Some(Object::Array(rect)) = dictionary
        .get(b"Rect")
        .ok()
        .and_then(|object| resolve_object(document, object))
    else {
        return (None, None);
    };
    (rect.first().and_then(number), rect.get(1).and_then(number))
}

/// Set annotation pages on links, keeping unmatched links pageless.
/// Returns `(link records, links with a page)` for the command summary.
pub(crate) fn annotate_catalog_links(
    links: &mut [CatalogLink],
    pdf_path: &Path,
) -> Result<(usize, usize), String> {
    let document = Document::load(pdf_path)
        .map_err(|error| format!("read PDF {}: {error}", pdf_path.display()))?;
    let annotations = uri_annotations(&document);
    let matched = map_links_to_annotations(links, &annotations);
    Ok((links.len(), matched))
}

fn uri_annotations(document: &Document) -> Vec<UriAnnotation> {
    let mut annotations = Vec::new();
    for (page, page_id) in document.get_pages() {
        let Ok(page_dictionary) = document.get_dictionary(page_id) else {
            continue;
        };
        let Some(Object::Array(items)) = page_dictionary
            .get(b"Annots")
            .ok()
            .and_then(|object| resolve_object(document, object))
        else {
            continue;
        };
        for (annotation_index, item) in items.iter().enumerate() {
            let Some(uri) = annotation_uri(document, item) else {
                continue;
            };
            let (left, bottom) = annotation_position(document, item);
            annotations.push(UriAnnotation {
                page,
                annotation_index,
                uri,
                left,
                bottom,
            });
        }
    }
    annotations
}

fn same_uri(left: &str, right: &str) -> bool {
    if left == right {
        return true;
    }
    percent_decode(left) == percent_decode(right)
}

fn percent_decode(value: &str) -> Vec<u8> {
    fn hex_digit(byte: u8) -> Option<u8> {
        match byte {
            b'0'..=b'9' => Some(byte - b'0'),
            b'a'..=b'f' => Some(byte - b'a' + 10),
            b'A'..=b'F' => Some(byte - b'A' + 10),
            _ => None,
        }
    }

    let input = value.as_bytes();
    let mut decoded = Vec::with_capacity(input.len());
    let mut index = 0;
    while index < input.len() {
        if input[index] == b'%'
            && index + 2 < input.len()
            && let (Some(high), Some(low)) =
                (hex_digit(input[index + 1]), hex_digit(input[index + 2]))
        {
            decoded.push((high << 4) | low);
            index += 3;
        } else {
            decoded.push(input[index]);
            index += 1;
        }
    }
    decoded
}

fn continuation(previous: &UriAnnotation, next: &UriAnnotation) -> bool {
    if previous.uri != next.uri {
        return false;
    }
    if next.page == previous.page {
        return matches!(
            (previous.left, previous.bottom, next.left, next.bottom),
            (Some(previous_left), Some(previous_bottom), Some(next_left), Some(next_bottom))
                if next_bottom < previous_bottom && next_left < previous_left
        );
    }
    next.page == previous.page + 1 && next.annotation_index == 0
}

fn map_links_to_annotations(
    links: &mut [CatalogLink],
    annotations: &[UriAnnotation],
) -> usize {
    let mut cursor = 0;
    let mut matched = 0;
    for link in links {
        let exact = (cursor..annotations.len())
            .find(|index| annotations[*index].uri == link.url);
        let index = exact.or_else(|| {
            (cursor..annotations.len())
                .find(|index| same_uri(&annotations[*index].uri, &link.url))
        });
        let Some(index) = index else {
            continue;
        };
        let first = &annotations[index];
        link.page = Some(first.page);
        link.page_source = Some("annotation".to_string());
        matched += 1;
        let mut last = index;
        while last + 1 < annotations.len()
            && continuation(&annotations[last], &annotations[last + 1])
        {
            last += 1;
        }
        cursor = last + 1;
    }
    matched
}

#[cfg(test)]
mod tests {
    use super::*;
    use lopdf::{dictionary, Stream};

    fn link(n: u32, url: &str) -> CatalogLink {
        CatalogLink {
            n,
            url: url.to_string(),
            text: format!("link {n}"),
            title: None,
            autolink: false,
            context: None,
            section: None,
            page: None,
            page_source: None,
        }
    }

    fn pdf_with_annotations(pages: &[Vec<(&str, [i64; 4])>]) -> Document {
        let mut document = Document::with_version("1.5");
        let pages_id = document.new_object_id();
        let mut page_ids = Vec::new();
        for annotations in pages {
            let mut annotation_ids = Vec::new();
            for (uri, rect) in annotations {
                let action = dictionary! {
                    "S" => "URI",
                    "URI" => Object::string_literal(*uri),
                };
                annotation_ids.push(Object::Reference(document.add_object(
                    dictionary! {
                        "Type" => "Annot",
                        "Subtype" => "Link",
                        "Rect" => Object::Array(
                            rect.iter()
                                .map(|value| Object::Integer(*value))
                                .collect(),
                        ),
                        "A" => action,
                    },
                )));
            }
            let contents =
                document.add_object(Stream::new(dictionary! {}, Vec::new()));
            let page_id = document.add_object(dictionary! {
                "Type" => "Page",
                "Parent" => pages_id,
                "MediaBox" => Object::Array(vec![
                    Object::Integer(0),
                    Object::Integer(0),
                    Object::Integer(612),
                    Object::Integer(792),
                ]),
                "Contents" => contents,
                "Annots" => annotation_ids,
            });
            page_ids.push(Object::Reference(page_id));
        }
        document.set_object(
            pages_id,
            dictionary! {
                "Type" => "Pages",
                "Kids" => page_ids,
                "Count" => pages.len() as i64,
            },
        );
        let catalog_id = document.add_object(dictionary! {
            "Type" => "Catalog",
            "Pages" => pages_id,
        });
        document.trailer.set("Root", catalog_id);
        document
    }

    #[test]
    fn matches_wrapped_pieces_and_page_spanning_pieces() {
        let document = pdf_with_annotations(&[
            vec![
                ("https://example.test/wrap", [100, 700, 220, 720]),
                ("https://example.test/wrap", [40, 680, 100, 700]),
            ],
            vec![("https://example.test/wrap", [35, 740, 100, 760])],
        ]);
        let annotations = uri_annotations(&document);
        assert_eq!(annotations.len(), 3);
        let mut links = vec![link(1, "https://example.test/wrap")];
        assert_eq!(map_links_to_annotations(&mut links, &annotations), 1);
        assert_eq!(links[0].page, Some(1));
        assert_eq!(links[0].page_source.as_deref(), Some("annotation"));
    }

    #[test]
    fn adjacent_same_uri_items_stay_separate_and_missing_links_stay_pageless() {
        let document = pdf_with_annotations(&[vec![
            ("https://example.test/repeat", [100, 700, 200, 720]),
            ("https://example.test/repeat", [100, 660, 200, 680]),
        ]]);
        let annotations = uri_annotations(&document);
        let mut links = vec![
            link(1, "https://example.test/repeat"),
            link(2, "https://example.test/repeat"),
            link(3, "https://example.test/missing"),
        ];
        assert_eq!(map_links_to_annotations(&mut links, &annotations), 2);
        assert_eq!(links[0].page, Some(1));
        assert_eq!(links[1].page, Some(1));
        assert_eq!(links[2].page, None);
        assert_eq!(links[2].page_source, None);
    }

    #[test]
    fn percent_decodes_only_after_exact_match_search() {
        let document = pdf_with_annotations(&[
            vec![("https://example.test/a b", [10, 700, 100, 720])],
            vec![("https://example.test/a%20b", [10, 700, 100, 720])],
        ]);
        let annotations = uri_annotations(&document);
        let mut links = vec![link(1, "https://example.test/a%20b")];
        assert_eq!(map_links_to_annotations(&mut links, &annotations), 1);
        // Exact equality wins even when a decoded-equivalent URI came first.
        assert_eq!(links[0].page, Some(2));

        let mut links = vec![link(1, "https://example.test/a%20b")];
        assert_eq!(map_links_to_annotations(&mut links, &annotations[..1]), 1);
        assert_eq!(links[0].page, Some(1));
        assert_eq!(links[0].page_source.as_deref(), Some("annotation"));
    }
}
