//! `bob ref recatalog`: link backfill, dry-run, atomic catalog updates, and
//! scoped commits.

use crate::support::*;
use lopdf::{dictionary, Document, Object, Stream};
use serde_json::{json, Value};
use std::{fs, path::Path, process::Output};

const SOURCE: &str = "# Recatalog fixture\n\nSee [the link](https://example.test/recatalog?q=one).\n\nBare URL: https://example.test/bare-url\n";
const URL: &str = "https://example.test/recatalog?q=one";
const BARE_URL: &str = "https://example.test/bare-url";

fn git_vault(temp: &TempDir) -> std::path::PathBuf {
    let vault = temp.path().join("vault");
    fs::create_dir_all(&vault).expect("create vault");
    git_in(&vault, ["init", "-q"]);
    configure_test_git_identity(&vault);
    vault
}

fn commit_all(vault: &Path, message: &str) {
    git_in(vault, ["add", "-A"]);
    git_in(vault, ["commit", "-q", "-m", message]);
}

fn sha256(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    hex::encode(Sha256::digest(bytes))
}

fn write_uri_pdf(path: &Path, uri: &str) {
    let mut document = Document::with_version("1.5");
    let pages_id = document.new_object_id();
    let annotation = document.add_object(dictionary! {
        "Type" => "Annot",
        "Subtype" => "Link",
        "Rect" => Object::Array(vec![
            Object::Integer(10),
            Object::Integer(700),
            Object::Integer(220),
            Object::Integer(720),
        ]),
        "A" => dictionary! {
            "S" => "URI",
            "URI" => Object::string_literal(uri),
        },
    });
    let contents = document.add_object(Stream::new(dictionary! {}, Vec::new()));
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
        "Annots" => vec![Object::Reference(annotation)],
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
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("create pdf parent");
    }
    document.save(path).expect("write annotated PDF");
}

fn fixture_with_route(
    route: &str,
) -> (TempDir, std::path::PathBuf, std::path::PathBuf) {
    let temp = TempDir::new("bob-cli-recatalog");
    let vault = git_vault(&temp);
    let source_hash = sha256(SOURCE.as_bytes());
    let source_pointer = format!(
        ".bob/ref-sources/papers/recatalog_fixture--{}/source.md",
        &source_hash[..12]
    );
    let store_dir = vault.join(&source_pointer).parent().unwrap().to_path_buf();
    fs::create_dir_all(&store_dir).expect("store dir");
    write_file(&store_dir.join("source.md"), SOURCE);
    let before_catalog = json!({
        "version": 1,
        "captured_at": "2026-10-01T12:00:00Z",
        "route": route,
        "source": {
            "sha256": source_hash,
            "bytes": SOURCE.len(),
            "origin": "/tmp/recatalog_fixture.md"
        },
        "pdf": {"path": "lib/papers/recatalog_fixture.pdf", "sha256": "0".repeat(64)},
        "renderer": {"bob": "test", "pandoc": "test"},
        "future_field": {"preserve": [1, "exactly", null]},
        "blocks": [{
            "n": 1,
            "kind": "code",
            "lang": "rust",
            "classes": ["rust"],
            "attrs": {},
            "text": "fn saved_block() {}",
            "section": null,
            "page": 3,
            "page_end": 4,
            "page_source": "anchor"
        }]
    });
    write_file(
        &store_dir.join("code.json"),
        &serde_json::to_string_pretty(&before_catalog).expect("catalog JSON"),
    );
    write_uri_pdf(&vault.join("lib/papers/recatalog_fixture.pdf"), URL);
    fs::create_dir_all(vault.join("ref/papers")).expect("ref dir");
    write_file(
        &vault.join("ref/papers/recatalog_fixture.md"),
        &format!(
            "---\ntitle: Recatalog fixture\nsource_pdf: \"[[lib/papers/recatalog_fixture.pdf]]\"\nsource_markdown: {source_pointer}\n---\n\n# Recatalog fixture\n\n- [ ] #task #ref #hide ^ref\n"
        ),
    );
    commit_all(&vault, "seed recatalog fixture");
    (temp, vault, store_dir)
}

fn fixture() -> (TempDir, std::path::PathBuf, std::path::PathBuf) {
    fixture_with_route("markdown")
}

fn run_recatalog(temp: &TempDir, vault: &Path, args: &[&str]) -> Output {
    let mut command = bob_command();
    command
        .arg("ref")
        .arg("recatalog")
        .args(args)
        .env("BOB_DIR", vault)
        .env("NO_COLOR", "1")
        .env(
            "BOB_VAULT_SYNC_LOCK_FILE",
            temp.path().join("bob_sync.lock"),
        )
        .env(
            "BOB_VAULT_SYNC_STATE_FILE",
            temp.path().join("vault-sync.json"),
        )
        .env("XDG_STATE_HOME", temp.path().join("state"));
    command.output().expect("run ref recatalog")
}

fn head_subject(vault: &Path) -> String {
    String::from_utf8(git_in(vault, ["log", "--format=%s", "-1"]).stdout)
        .expect("git subject utf8")
        .trim()
        .to_string()
}

fn has_pandoc() -> bool {
    std::process::Command::new("pandoc")
        .arg("--version")
        .output()
        .is_ok()
}

#[test]
fn recatalog_dry_run_then_commits_only_additive_link_records() {
    if !has_pandoc() {
        eprintln!("skipping recatalog integration: pandoc is required");
        return;
    }
    let (temp, vault, store_dir) = fixture();
    let catalog_path = store_dir.join("code.json");
    let before: Value = serde_json::from_slice(
        &fs::read(&catalog_path).expect("original catalog"),
    )
    .expect("original catalog JSON");
    let initial_head = head_subject(&vault);

    let dry = run_recatalog(&temp, &vault, &["-d"]);
    assert_success(&dry);
    let dry_report = stdout(&dry);
    assert!(
        dry_report.contains("would add 2 links (1 on pages)"),
        "dry run output:\n{dry_report}"
    );
    assert!(dry_report.contains("writes: none"), "{dry_report}");
    assert_eq!(
        fs::read(&catalog_path).expect("dry run left catalog unchanged"),
        serde_json::to_vec_pretty(&before).expect("original JSON bytes"),
    );
    assert_eq!(head_subject(&vault), initial_head);

    let output = run_recatalog(&temp, &vault, &[]);
    assert_success(&output);
    let report = stdout(&output);
    assert!(
        report.contains("added 2 links (1 on pages)"),
        "write output:\n{report}"
    );
    assert!(report.contains("committed "), "scoped commit:\n{report}");
    assert_eq!(head_subject(&vault), "bob ref recatalog: 1 references");

    let after: Value = serde_json::from_slice(
        &fs::read(&catalog_path).expect("updated catalog"),
    )
    .expect("updated catalog JSON");
    for field in [
        "version",
        "captured_at",
        "route",
        "source",
        "pdf",
        "renderer",
        "future_field",
        "blocks",
    ] {
        assert_eq!(after[field], before[field], "field {field} is unchanged");
    }
    let links = after["links"].as_array().expect("link records");
    assert_eq!(links.len(), 2);
    assert_eq!(links[0]["url"], URL);
    assert_eq!(links[0]["page"], 1);
    assert_eq!(links[0]["page_source"], "annotation");
    assert_eq!(links[1]["url"], BARE_URL);
    assert_eq!(links[1]["page"], Value::Null);
    assert_eq!(links[1]["page_source"], Value::Null);
    let changed_paths =
        git_in(&vault, ["show", "--format=", "--name-only", "HEAD"]);
    assert_eq!(
        String::from_utf8(changed_paths.stdout)
            .expect("changed paths")
            .trim(),
        catalog_path.strip_prefix(&vault).unwrap().to_string_lossy(),
    );

    let again = run_recatalog(&temp, &vault, &["recatalog_fixture"]);
    assert_success(&again);
    assert!(
        stdout(&again).contains("skipped (already recorded)"),
        "repeat run:\n{}",
        stdout(&again)
    );
    assert_eq!(head_subject(&vault), "bob ref recatalog: 1 references");
}

#[test]
fn recatalog_uses_snapshot_reader_for_snapshot_sources() {
    if !has_pandoc() {
        eprintln!(
            "skipping snapshot recatalog integration: pandoc is required"
        );
        return;
    }
    let (temp, vault, _) = fixture_with_route("note_snapshot");
    let output = run_recatalog(&temp, &vault, &["-d"]);
    assert_success(&output);
    let report = stdout(&output);
    assert!(
        report.contains("would add 2 links (1 on pages)"),
        "snapshot reader output:\n{report}"
    );
}
