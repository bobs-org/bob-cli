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
