//! Read named PDF destinations and map them to one-based physical pages.

use std::collections::BTreeMap;

use lopdf::{Document, Object, ObjectId};

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

/// Return every named destination that resolves to a physical page.
///
/// Chrome writes print-time anchor destinations as a catalog-level
/// `/Dests` dictionary (name to explicit destination array, or to a
/// dictionary with `/D`); the classic `/Names` → `/Dests` name tree is
/// read too, and wins on duplicates.
pub(crate) fn named_dest_pages(document: &Document) -> BTreeMap<String, u32> {
    let mut dests = BTreeMap::new();
    let page_numbers: BTreeMap<ObjectId, u32> = document
        .get_pages()
        .into_iter()
        .map(|(number, id)| (id, number))
        .collect();

    fn destination_page(
        document: &Document,
        page_numbers: &BTreeMap<ObjectId, u32>,
        value: &Object,
    ) -> Option<u32> {
        match resolve_object(document, value)? {
            Object::Array(items) => items
                .first()
                .and_then(|page| page.as_reference().ok())
                .and_then(|id| page_numbers.get(&id).copied()),
            Object::Dictionary(dictionary) => {
                let destination = dictionary.get(b"D").ok()?;
                destination_page(
                    document,
                    page_numbers,
                    &resolve_object(document, destination)?,
                )
            }
            _ => None,
        }
    }

    fn collect_tree(
        document: &Document,
        page_numbers: &BTreeMap<ObjectId, u32>,
        node: &lopdf::Dictionary,
        dests: &mut BTreeMap<String, u32>,
    ) {
        if let Some(Object::Array(entries)) = node
            .get(b"Names")
            .ok()
            .and_then(|object| resolve_object(document, object))
        {
            let mut pairs = entries.iter();
            while let (Some(key), Some(value)) = (pairs.next(), pairs.next()) {
                let Object::String(name_bytes, _) = key else {
                    continue;
                };
                let name = String::from_utf8_lossy(name_bytes).into_owned();
                if let Some(page) =
                    destination_page(document, page_numbers, value)
                {
                    dests.insert(name, page);
                }
            }
        }
        if let Some(Object::Array(kids)) = node
            .get(b"Kids")
            .ok()
            .and_then(|object| resolve_object(document, object))
        {
            for kid in &kids {
                if let Some(child) = dictionary(document, kid) {
                    collect_tree(document, page_numbers, &child, dests);
                }
            }
        }
    }

    let Ok(catalog) = document.catalog() else {
        return dests;
    };
    // Chrome's print path writes a plain dictionary here: each name maps
    // to an explicit destination array (or a dictionary carrying `/D`).
    // Keys may be name or string objects depending on the writer.
    if let Some(flat) = catalog
        .get(b"Dests")
        .ok()
        .and_then(|object| dictionary(document, object))
    {
        for (key, value) in flat.iter() {
            let name = String::from_utf8_lossy(key).into_owned();
            if let Some(page) = destination_page(document, &page_numbers, value)
            {
                dests.insert(name, page);
            }
        }
    }
    let Some(names) = catalog
        .get(b"Names")
        .ok()
        .and_then(|object| dictionary(document, object))
    else {
        return dests;
    };
    let Some(tree) = names
        .get(b"Dests")
        .ok()
        .and_then(|object| dictionary(document, object))
    else {
        return dests;
    };
    collect_tree(document, &page_numbers, &tree, &mut dests);
    dests
}

#[cfg(test)]
mod tests {
    use super::*;
    use lopdf::{dictionary, Document, Object, Stream};

    /// Two-page PDF shaped like Chrome's print output: a catalog-level
    /// `/Dests` dictionary plus a conflicting `/Names` → `/Dests` tree.
    fn chrome_shaped_document() -> Document {
        let mut document = Document::with_version("1.4");
        let pages_id = document.new_object_id();
        let content_id =
            document.add_object(Stream::new(dictionary! {}, Vec::new()));
        let mut page_ids = Vec::new();
        for _ in 0..2 {
            page_ids.push(document.add_object(dictionary! {
                "Type" => "Page",
                "Parent" => pages_id,
                "MediaBox" => vec![
                    Object::Integer(0),
                    Object::Integer(0),
                    Object::Integer(612),
                    Object::Integer(792),
                ],
                "Contents" => content_id,
            }));
        }
        document.set_object(
            pages_id,
            dictionary! {
                "Type" => "Pages",
                "Kids" => vec![
                    Object::Reference(page_ids[0]),
                    Object::Reference(page_ids[1]),
                ],
                "Count" => 2,
            },
        );
        // The name tree disagrees on `bob-code-1` so the test proves it
        // wins over the flat catalog dictionary.
        let tree = document.add_object(dictionary! {
            "Names" => vec![
                Object::string_literal("bob-code-1"),
                Object::Array(vec![Object::Reference(page_ids[0])]),
                Object::string_literal("tree-only"),
                Object::Array(vec![Object::Reference(page_ids[0])]),
            ],
        });
        let names = document.add_object(dictionary! {
            "Dests" => Object::Reference(tree),
        });
        let catalog_id = document.add_object(dictionary! {
            "Type" => "Catalog",
            "Pages" => pages_id,
            "Names" => Object::Reference(names),
            "Dests" => dictionary! {
                "bob-code-1" => vec![Object::Reference(page_ids[1])],
                "bob-code-1-end" => dictionary! {
                    "D" => vec![Object::Reference(page_ids[1])],
                },
                "stray" => vec![Object::Name(b"XYZ".to_vec())],
            },
        });
        document.trailer.set("Root", catalog_id);
        document
    }

    #[test]
    fn reads_chrome_catalog_dests_with_tree_precedence() {
        let mut document = chrome_shaped_document();
        // Round-trip through the serializer so key spellings match a
        // real file on disk.
        let bytes = {
            let mut buffer = Vec::new();
            document.save_to(&mut buffer).expect("serialize dests PDF");
            buffer
        };
        let document = Document::load_mem(&bytes).expect("reload dests PDF");
        let pages = named_dest_pages(&document);
        assert_eq!(pages.get("bob-code-1"), Some(&1));
        assert_eq!(pages.get("bob-code-1-end"), Some(&2));
        assert_eq!(pages.get("tree-only"), Some(&1));
        assert!(!pages.contains_key("stray"));
    }
}
