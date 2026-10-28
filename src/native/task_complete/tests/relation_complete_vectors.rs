//! Shared-vector coverage for complete labels
//! (`tests/fixtures/relation_complete/vectors.json`).

use serde::Deserialize;

use super::super::tree::{
    add_or_replace_completion_field, line_text_at, RootPolicy,
};
use super::super::{complete_task_tree, is_recurring_task_line};
use super::MemoryVault;
use crate::native::capture_task_toggle::set_task_line_status;

#[derive(Debug, Deserialize)]
struct VectorsFile {
    done: Vec<DoneVector>,
    rule: Vec<serde_json::Value>,
    key: Vec<serde_json::Value>,
    config: Vec<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
struct DoneVector {
    name: String,
    line: String,
    date: String,
    #[serde(default)]
    eol: Option<String>,
    expected: DoneExpected,
}

#[derive(Debug, Deserialize)]
struct DoneExpected {
    ok: bool,
    #[serde(default)]
    unchanged: bool,
    #[serde(default)]
    reason: Option<String>,
    line: String,
}

fn vectors() -> VectorsFile {
    serde_json::from_str(include_str!(
        "../../../../tests/fixtures/relation_complete/vectors.json"
    ))
    .expect("typed relation-complete vectors parse")
}

fn trailing_block_id(line: &str) -> Option<String> {
    let trimmed = line.trim_end_matches([' ', '\t', '\r']);
    let start = trimmed.rfind('^')?;
    (start > 0 && trimmed.as_bytes()[start - 1].is_ascii_whitespace())
        .then(|| trimmed[start + 1..].to_string())
        .filter(|id| !id.is_empty())
}

fn root_rewrite(line: &str, date: &str) -> String {
    let Some(updated) = set_task_line_status(line, 'x') else {
        return line.to_string();
    };
    add_or_replace_completion_field(&updated, date)
}

#[test]
fn relation_complete_vector_sections_exist() {
    let vectors = vectors();
    assert!(!vectors.done.is_empty(), "done");
    assert!(!vectors.rule.is_empty(), "rule");
    assert!(!vectors.key.is_empty(), "key");
    assert!(!vectors.config.is_empty(), "config");
}

#[test]
fn complete_task_tree_root_rewrite_matches_every_non_refused_done_case() {
    let vectors = vectors();
    for vector in vectors.done {
        if !vector.expected.ok {
            assert_eq!(
                vector.expected.reason.as_deref(),
                Some("recurring"),
                "{}",
                vector.name
            );
            assert!(is_recurring_task_line(&vector.line), "{}", vector.name);
            assert_eq!(vector.expected.line, vector.line, "{}", vector.name);
            let Some(block_id) = trailing_block_id(&vector.line) else {
                continue;
            };
            let mut vault = MemoryVault::new();
            vault.insert("note.md", &format!("{}\n", vector.line));
            let path = vault.absolute("note.md");
            let outcome = complete_task_tree(
                &vault,
                &path,
                &block_id,
                &vector.date,
                RootPolicy::Explicit,
            )
            .unwrap_or_else(|error| panic!("{}: {error}", vector.name));
            assert!(
                outcome.root.is_none(),
                "{}: recurring root stayed open",
                vector.name
            );
            assert!(
                !outcome.changed_files.contains_key(&path),
                "{}: recurring rewrite wrote nothing",
                vector.name
            );
            continue;
        }

        if vector.expected.unchanged {
            assert_eq!(vector.expected.line, vector.line, "{}", vector.name);
            let Some(block_id) = trailing_block_id(&vector.line) else {
                continue;
            };
            let mut vault = MemoryVault::new();
            vault.insert("note.md", &format!("{}\n", vector.line));
            let path = vault.absolute("note.md");
            let outcome = complete_task_tree(
                &vault,
                &path,
                &block_id,
                &vector.date,
                RootPolicy::Explicit,
            )
            .unwrap_or_else(|error| panic!("{}: {error}", vector.name));
            assert!(
                outcome.root.is_none(),
                "{}: already-closed root stayed closed",
                vector.name
            );
            assert!(
                !outcome.changed_files.contains_key(&path),
                "{}: already-closed rewrite wrote nothing",
                vector.name
            );
            continue;
        }

        let helper = root_rewrite(&vector.line, &vector.date);
        assert_eq!(
            helper, vector.expected.line,
            "{}: helper rewrite",
            vector.name
        );

        let Some(block_id) = trailing_block_id(&vector.line) else {
            continue;
        };
        let crlf = vector.eol.as_deref() == Some("crlf");
        let body = if crlf {
            format!("{}\r\n", vector.line)
        } else {
            format!("{}\n", vector.line)
        };
        let mut vault = MemoryVault::new();
        vault.insert("note.md", &body);
        let path = vault.absolute("note.md");
        let outcome = complete_task_tree(
            &vault,
            &path,
            &block_id,
            &vector.date,
            RootPolicy::Explicit,
        )
        .unwrap_or_else(|error| panic!("{}: {error}", vector.name));
        let rewritten = outcome
            .changed_files
            .get(&path)
            .unwrap_or_else(|| panic!("{}: expected a write", vector.name));
        let actual = line_text_at(rewritten, 0).unwrap_or_default();
        assert_eq!(actual, vector.expected.line, "{}", vector.name);
        if crlf {
            assert!(
                rewritten.contains("\r\n"),
                "{}: CRLF terminator preserved",
                vector.name
            );
        }
        assert!(outcome.root.is_some(), "{}: root closed", vector.name);
    }
}
