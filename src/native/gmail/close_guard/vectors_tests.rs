//! Conformance vectors in `tests/fixtures/mail_close/vectors.json`.
//!
//! The file is authoritative and shared: bob-plugins copies it byte for
//! byte and runs the same cases against its JS twin. Every vector runs
//! here through the real Rust predicate, copy, and digest functions, so a
//! behavior change fails this test instead of drifting silently.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use serde_yaml::Value;

use super::copy::{build_copy, task_text, CloseTarget};
use super::digest::{close_digest, DigestRow};
use super::predicate::{
    direct_link_only_children, is_mail_note_frontmatter,
    linked_mail_notes_in_block, test_resolver,
};
use crate::native::gmail::sync::scan as watch_scan;

static VECTORS: std::sync::LazyLock<serde_json::Value> =
    std::sync::LazyLock::new(|| {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/mail_close/vectors.json");
        let text =
            std::fs::read_to_string(&path).expect("read mail close vectors");
        serde_json::from_str(&text).expect("parse mail close vectors")
    });

fn expect_text(value: &serde_json::Value) -> &str {
    value.as_str().expect("vector string")
}

fn optional_text(value: &serde_json::Value) -> Option<String> {
    if value.is_null() {
        None
    } else {
        Some(value.as_str().expect("vector string or null").to_string())
    }
}

fn string_map(value: &serde_json::Value) -> BTreeMap<String, String> {
    value
        .as_object()
        .expect("vector string map")
        .iter()
        .map(|(key, member)| (key.clone(), expect_text(member).to_string()))
        .collect()
}

fn frontmatter_value(
    fields: &serde_json::Map<String, serde_json::Value>,
) -> Value {
    let mut mapping = serde_yaml::Mapping::new();
    for (key, member) in fields {
        if member.is_null() {
            continue;
        }
        mapping.insert(
            Value::String(key.clone()),
            Value::String(expect_text(member).to_string()),
        );
    }
    Value::Mapping(mapping)
}

fn run_predicate(vector: &serde_json::Value) {
    let id = expect_text(&vector["id"]);
    let line = expect_text(&vector["line"]);
    let children: Vec<String> = vector["children"]
        .as_array()
        .expect("children array")
        .iter()
        .map(|child| expect_text(child).to_string())
        .collect();
    let task_hidden = vector
        .get("task_hidden")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);
    let link_to_path = if vector
        .get("link_to_path")
        .is_some_and(|value| !value.is_null())
    {
        string_map(&vector["link_to_path"])
    } else {
        BTreeMap::new()
    };
    let vault_files: BTreeSet<String> = vector["vault_files"]
        .as_array()
        .expect("vault_files array")
        .iter()
        .map(|path| expect_text(path).to_string())
        .collect();
    let notes = vector["notes"].as_object().expect("notes map");

    let resolve = test_resolver(&link_to_path, &vault_files);
    let is_mail = |note: &str| {
        notes.get(note).is_some_and(|fields| {
            is_mail_note_frontmatter(&frontmatter_value(
                fields.as_object().expect("note fields"),
            ))
        })
    };

    let block: Vec<&str> = std::iter::once(line)
        .chain(children.iter().map(String::as_str))
        .collect();
    let found = if task_hidden {
        Vec::new()
    } else {
        match watch_scan::parse_task_row(line) {
            None => Vec::new(),
            Some(task) => {
                let fenced = watch_scan::fenced_lines(&block, 0);
                let hidden = |index: usize| fenced.contains(&index);
                let inners =
                    direct_link_only_children(&block, 0, task.width, &hidden);
                let inner_refs: Vec<&str> =
                    inners.iter().map(String::as_str).collect();
                linked_mail_notes_in_block(
                    line,
                    &inner_refs,
                    &resolve,
                    &is_mail,
                )
            }
        }
    };
    let expected: Vec<String> = vector["expect_notes"]
        .as_array()
        .expect("expect_notes array")
        .iter()
        .map(|note| expect_text(note).to_string())
        .collect();
    assert_eq!(found, expected, "{id}: mail notes");
    assert_eq!(
        !found.is_empty(),
        vector["expect_gmail_task"].as_bool().expect("verdict"),
        "{id}: gmail task verdict"
    );
}

fn run_copy(vector: &serde_json::Value) {
    let id = expect_text(&vector["id"]);
    let now = expect_text(&vector["now"]);
    let archive = vector["archive"].as_bool().expect("archive flag");
    let linker_counts = if vector
        .get("other_open_linkers")
        .is_some_and(|value| !value.is_null())
    {
        vector["other_open_linkers"]
            .as_object()
            .expect("linker map")
            .iter()
            .map(|(note, count)| {
                (note.clone(), count.as_u64().expect("linker count") as usize)
            })
            .collect::<BTreeMap<String, usize>>()
    } else {
        BTreeMap::new()
    };
    let targets: Vec<CloseTarget> = vector["targets"]
        .as_array()
        .expect("targets array")
        .iter()
        .map(|target| {
            let line_text = expect_text(&target["line_text"]);
            let mails = target["mails"]
                .as_array()
                .expect("mails array")
                .iter()
                .map(|mail| super::predicate::MailNoteFacts {
                    note: expect_text(&mail["note"]).to_string(),
                    title: expect_text(&mail["title"]).to_string(),
                    who: expect_text(&mail["who"]).to_string(),
                    turn: optional_text(&mail["turn"]),
                    turn_since: optional_text(&mail["turn_since"]),
                    gmail_url: mail
                        .get("gmail_url")
                        .map(expect_text)
                        .unwrap_or_default()
                        .to_string(),
                })
                .collect();
            CloseTarget {
                path: expect_text(&target["path"]).to_string(),
                line: target["line"].as_u64().expect("line number") as usize,
                before: expect_text(&target["before"])
                    .chars()
                    .next()
                    .expect("before status"),
                after: expect_text(&target["after"])
                    .chars()
                    .next()
                    .expect("after status"),
                task_text: task_text(line_text),
                mails,
            }
        })
        .collect();
    let copy = build_copy(
        &targets,
        now,
        &|note| linker_counts.get(note).copied().unwrap_or(0),
        archive,
    );
    let expected = &vector["expect"];
    assert_eq!(copy.title, expect_text(&expected["title"]), "{id}: title");
    assert_eq!(
        copy.subtitle,
        expect_text(&expected["subtitle"]),
        "{id}: subtitle"
    );
    assert_eq!(copy.tone, expect_text(&expected["tone"]), "{id}: tone");
    assert_eq!(copy.verb, expect_text(&expected["verb"]), "{id}: verb");
    let expected_rows = expected["rows"].as_array().expect("rows array");
    assert_eq!(copy.rows.len(), expected_rows.len(), "{id}: row count");
    for (row, expected_row) in copy.rows.iter().zip(expected_rows) {
        assert_eq!(
            row.path,
            expect_text(&expected_row["path"]),
            "{id}: row path"
        );
        assert_eq!(
            row.line,
            expected_row["line"].as_u64().expect("row line") as usize,
            "{id}: row line"
        );
        assert_eq!(
            row.label,
            expect_text(&expected_row["label"]),
            "{id}: row label"
        );
        assert_eq!(
            row.turn_line.as_deref(),
            expected_row["turn_line"].as_str(),
            "{id}: turn line"
        );
        assert_eq!(
            row.note,
            expect_text(&expected_row["note"]),
            "{id}: row note"
        );
    }
    assert_eq!(
        copy.more,
        expected["more"].as_u64().expect("more count") as usize,
        "{id}: more"
    );
    assert_eq!(
        copy.consequence,
        expect_text(&expected["consequence"]),
        "{id}: consequence"
    );
    let digest = close_digest(
        targets
            .iter()
            .map(|target| DigestRow {
                path: target.path.as_str(),
                line: target.line,
                before: target.before,
                after: target.after,
                mails: target
                    .mails
                    .iter()
                    .map(|mail| mail.note.as_str())
                    .collect(),
            })
            .collect(),
    );
    assert_eq!(digest, expect_text(&expected["digest"]), "{id}: digest");
    assert_eq!(digest.len(), 12, "{id}: digest length");
}

/// Every vector in the shared fixture runs through the real model.
#[test]
fn mail_close_vectors_match_rust_model() {
    let vectors = VECTORS["vectors"].as_array().expect("vectors array");
    assert!(!vectors.is_empty(), "fixture must carry vectors");
    for vector in vectors {
        let id = expect_text(&vector["id"]);
        match expect_text(&vector["kind"]) {
            "predicate" => run_predicate(vector),
            "copy" => run_copy(vector),
            kind => panic!("{id}: unknown vector kind {kind}"),
        }
    }
}
