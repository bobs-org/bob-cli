//! Single web-article saved-source store tests: `bob ref create <URL>`
//! saves `source.md` plus a code/link catalog at create time.

use super::fake_clip::{
    web_fixture_pdf, web_fixture_pdf_with_blocks, FakeClip,
};
use super::ingest::seed_synced_vault;
use super::scan_intake::scan_command;
use crate::support::*;
use std::{fs, path::Path, process::Command};

const ARTICLE_URL: &str = "https://example.com/article/hello";
const LINK_URI: &str = "https://example.com/docs";

fn skip_unless_pandoc(test_name: &str) -> bool {
    if Command::new("pandoc")
        .arg("--version")
        .output()
        .is_ok_and(|output| output.status.success())
    {
        return false;
    }
    eprintln!("skipping {test_name}: pandoc is required");
    true
}

/// Fake curl for article tests: every fetch is an HTML 200.
fn write_fake_curl(dir: &Path) -> std::path::PathBuf {
    let path = dir.join("fake-curl.sh");
    let script = r#"#!/bin/sh
dest=""
for arg in "$@"; do
  if [ "$prev" = "-o" ]; then dest="$arg"; fi
  prev="$arg"
done
printf '<html><body>article</body></html>' > "$dest"
printf '200\ntext/html; charset=utf-8\n\n'
"#;
    write_executable(&path, script);
    path
}

fn web_create_command(
    temp: &TempDir,
    vault: &Path,
    curl: &Path,
    adapter: &Path,
) -> Command {
    let mut command = bob_command();
    command
        .arg("ref")
        .arg("create")
        .arg(ARTICLE_URL)
        .arg("-P")
        .arg("sase")
        .arg("-b")
        .arg(vault)
        .env("BOB_DIR", vault)
        .env("BOB_HIGHLIGHTS_INGEST_ON_CREATE", "1")
        .env("BOB_REF_INTAKE_LOCK_WAIT_SECONDS", "0")
        .env(
            "BOB_VAULT_SYNC_LOCK_FILE",
            temp.path().join("bob_sync.lock"),
        )
        .env(
            "BOB_VAULT_SYNC_STATE_FILE",
            temp.path().join("vault-sync.json"),
        )
        .env("XDG_STATE_HOME", temp.path().join("state"))
        .env("BOB_HIGHLIGHTS_CURL", curl)
        .env("BOB_WEB_CLIP_ADAPTER", adapter);
    command
}

fn run_ref(vault: &Path, args: &[&str]) -> std::process::Output {
    bob_command()
        .arg("ref")
        .args(args)
        .env("BOB_DIR", vault)
        .output()
        .expect("run bob ref")
}

fn parse_json(output: &std::process::Output) -> serde_json::Value {
    serde_json::from_str(&stdout(output)).unwrap_or_else(|error| {
        panic!("parse JSON: {error}\n{}", format_output(output))
    })
}

const SOURCE_HTML: &str = r#"<!DOCTYPE html>
<html>
<head><title>Fixture Article</title></head>
<body>
<article>
<h1>Fixture Article</h1>
<p>By Tester</p>
<p>Read the <a href="https://example.com/docs">docs</a> first.</p>
<pre><code class="language-rust">fn main() {
	println!("hello");
}
</code></pre>
</article>
</body>
</html>
"#;

/// The DOM `textContent` of the fixture's `<pre>`: what the adapter
/// records in `code-anchors.json`.
const ANCHOR_TEXT: &str = "fn main() {\n\tprintln!(\"hello\");\n}\n";

/// Point the fake adapter at a source view plus anchors and swap its
/// fixture PDF for one carrying Chrome-style destinations.
fn stage_source_view(temp: &TempDir, fake: &FakeClip) {
    let render = temp.path().join("render");
    fs::create_dir_all(&render).expect("create render dir");
    fs::write(render.join("source.html"), SOURCE_HTML)
        .expect("write source view");
    let anchors = serde_json::json!({
        "version": 1,
        "blocks": [{"k": 1, "text": ANCHOR_TEXT}],
    });
    fs::write(
        render.join("code-anchors.json"),
        serde_json::to_string(&anchors).expect("anchors JSON"),
    )
    .expect("write code anchors");
    fs::write(fake.root.join("fixture.pdf"), web_fixture_pdf(LINK_URI))
        .expect("write web fixture PDF");
    let response = serde_json::json!({
        "protocol": 1,
        "ok": true,
        "op": "capture",
        "kind": "article",
        "final_url": ARTICLE_URL,
        "title": "Fixture Article",
        "author": "Tester",
        "published": "2026-10-26",
        "site": "Example",
        "description": "",
        "metadata_sources": {},
        "word_count": 42,
        "capture": {
            "browser": "chrome",
            "browser_version": "154.0.0",
            "mode": "headless",
            "retried_after_challenge": false,
        },
        "fidelity": {"status": "ok"},
        "images": {"total": 0, "kept": 0, "skipped_small": 0, "failed": 0},
        "pdf_bytes": null,
        "source_html": render.join("source.html").to_string_lossy(),
        "code_anchors": render.join("code-anchors.json").to_string_lossy(),
        "warnings": [],
    });
    fake.respond(&serde_json::to_string(&response).expect("response JSON"));
}

fn reported_pointer(report: &str) -> String {
    report
        .lines()
        .find_map(|line| {
            line.trim()
                .strip_prefix("source_markdown:")
                .map(|pointer| pointer.trim().trim_matches('"').to_string())
        })
        .unwrap_or_else(|| panic!("no source_markdown in report:\n{report}"))
}

#[test]
fn web_article_create_saves_source_and_catalog() {
    if skip_unless_pandoc("web article source store") {
        return;
    }
    let temp = TempDir::new("bob-cli-web-article-store");
    let (vault, _peer) = seed_synced_vault(&temp);
    let curl = write_fake_curl(temp.path());
    let fake = FakeClip::new(&temp, "adapter");
    stage_source_view(&temp, &fake);

    let output = web_create_command(&temp, &vault, &curl, &fake.path)
        .output()
        .expect("run web create");
    assert_success(&output);
    let report = stdout(&output);
    let pointer = reported_pointer(&report);
    assert!(
        pointer.starts_with(".bob/ref-sources/blogs/hello--")
            && pointer.ends_with("/source.md"),
        "unexpected pointer: {pointer}\n{report}"
    );
    assert!(
        report.contains("code: 1 block (rust)"),
        "missing code summary:\n{report}"
    );
    assert!(
        report.contains("links: 1 link (example.com)"),
        "missing links summary:\n{report}"
    );

    let source_path = vault.join(&pointer);
    assert!(source_path.is_file(), "missing saved source: {pointer}");
    let saved = fs::read_to_string(&source_path).expect("read source.md");
    assert!(
        saved.contains("# Fixture Article")
            && saved.contains("``` rust")
            && saved.contains("\tprintln!(\"hello\");"),
        "unexpected saved GFM:\n{saved}"
    );
    let store_dir = source_path.parent().expect("store dir").to_path_buf();
    let catalog = fs::read_to_string(store_dir.join("code.json"))
        .expect("read code.json");
    assert!(
        catalog.contains("\"route\": \"web_article\""),
        "wrong route in catalog:\n{catalog}"
    );

    let note = fs::read_to_string(vault.join("ref/blogs/hello.md"))
        .expect("read filed note");
    assert!(
        note.lines()
            .any(|line| line.trim() == format!("source_markdown: {pointer}")),
        "note must carry the pointer:\n{note}"
    );

    let output = run_ref(&vault, &["code", "hello", "-f", "json"]);
    assert_success(&output);
    let value = parse_json(&output);
    assert_eq!(value["status"], "available", "{value}");
    let blocks = value["blocks"].as_array().expect("blocks array");
    assert_eq!(blocks.len(), 1, "{value}");
    assert_eq!(
        blocks[0]["text"], "fn main() {\n\tprintln!(\"hello\");\n}",
        "exact tabbed bytes: {value}"
    );
    assert_eq!(blocks[0]["lang"], "rust", "{value}");
    assert_eq!(blocks[0]["page"], 1, "{value}");
    assert_eq!(blocks[0]["page_end"], 1, "{value}");
    assert_eq!(blocks[0]["page_source"], "anchor", "{value}");

    let output = run_ref(&vault, &["code", "hello", "--source"]);
    assert_success(&output);
    assert_eq!(stdout(&output), saved, "source bytes must round-trip");

    let output = run_ref(&vault, &["links", "hello", "-f", "json"]);
    assert_success(&output);
    let value = parse_json(&output);
    assert_eq!(value["status"], "available", "{value}");
    let links = value["links"].as_array().expect("links array");
    assert_eq!(links.len(), 1, "{value}");
    assert_eq!(links[0]["url"], LINK_URI, "{value}");
    assert_eq!(links[0]["page"], 1, "{value}");
    assert_eq!(links[0]["page_source"], "annotation", "{value}");
}

#[test]
fn web_article_create_without_source_view_files_without_pointer() {
    let temp = TempDir::new("bob-cli-web-article-no-view");
    let (vault, _peer) = seed_synced_vault(&temp);
    let curl = write_fake_curl(temp.path());
    // The default fake response carries no source_html/code_anchors.
    let fake = FakeClip::new(&temp, "adapter");

    let output = web_create_command(&temp, &vault, &curl, &fake.path)
        .output()
        .expect("run web create without source view");
    assert_success(&output);
    let report = stdout(&output);
    let combined = format_output(&output);
    assert!(
        report.contains("source_markdown: none"),
        "missing none line:\n{report}"
    );
    assert!(
        combined.contains("adapter reported no source view")
            && combined.contains("skipping source store"),
        "missing skip warning:\n{combined}"
    );
    assert!(
        !vault.join(".bob/ref-sources").exists(),
        "no store may be written without a source view"
    );
    let note = fs::read_to_string(vault.join("ref/blogs/hello.md"))
        .expect("read filed note");
    assert!(
        !note.contains("source_markdown"),
        "note must not carry a pointer:\n{note}"
    );
}

#[test]
fn web_article_exact_output_saves_nothing() {
    if skip_unless_pandoc("web article exact output") {
        return;
    }
    let temp = TempDir::new("bob-cli-web-article-output");
    let (vault, _peer) = seed_synced_vault(&temp);
    let curl = write_fake_curl(temp.path());
    let fake = FakeClip::new(&temp, "adapter");
    stage_source_view(&temp, &fake);
    let outside = temp.path().join("article-export.pdf");

    let output = web_create_command(&temp, &vault, &curl, &fake.path)
        .arg("-o")
        .arg(&outside)
        .output()
        .expect("run web create to exact output");
    assert_success(&output);
    assert!(
        stdout(&output)
            .contains("source_markdown: none (output outside the library)"),
        "{}",
        format_output(&output)
    );
    assert!(
        !vault.join(".bob/ref-sources").exists(),
        "exact output must not write a store"
    );
}

#[test]
fn web_article_queued_create_birth_adopts_store() {
    if skip_unless_pandoc("web article queued birth") {
        return;
    }
    let temp = TempDir::new("bob-cli-web-article-queued");
    let (vault, _peer) = seed_synced_vault(&temp);
    let curl = write_fake_curl(temp.path());
    let fake = FakeClip::new(&temp, "adapter");
    stage_source_view(&temp, &fake);

    let output = web_create_command(&temp, &vault, &curl, &fake.path)
        .arg("-k")
        .output()
        .expect("queue web create");
    assert_success(&output);
    let report = stdout(&output);
    let pointer = reported_pointer(&report);
    assert!(
        pointer.starts_with(".bob/ref-sources/blogs/hello--"),
        "unexpected queued pointer: {pointer}\n{report}"
    );
    assert!(
        vault.join("xlib/blogs/hello.pdf").is_file(),
        "create -k must leave the PDF queued"
    );

    let output = scan_command(&temp, &vault, &["-i"])
        .output()
        .expect("drain queued web PDF");
    assert_success(&output);
    let filed = fs::read_to_string(vault.join("ref/blogs/hello.md"))
        .expect("read drained note");
    assert!(
        filed
            .lines()
            .any(|line| line.trim() == format!("source_markdown: {pointer}")),
        "drained note must carry the birth pointer:\n{filed}"
    );
    assert_eq!(
        filed.matches("source_markdown:").count(),
        1,
        "drained note must not duplicate the pointer:\n{filed}"
    );
    let saved =
        fs::read_to_string(vault.join(&pointer)).expect("read saved GFM");
    assert!(
        saved.contains("# Fixture Article")
            && saved.contains("\tprintln!(\"hello\");"),
        "drain must keep the converted GFM bytes:\n{saved}"
    );
}

const COLLECTION_URL_1: &str = "https://example.com/series/part-one";
const COLLECTION_URL_2: &str = "https://example.com/series/part-two";

const COLLECTION_SOURCE_HTML: &str = r#"<!DOCTYPE html>
<html>
<head><title>Reading Packet</title></head>
<body>
<h1>Reading Packet</h1>
<h2>Part One</h2>
<p>Read the <a href="https://example.com/docs">docs</a> first.</p>
<pre><code class="language-rust">fn one() {
	println!("one");
}
</code></pre>
<h2>Part Two</h2>
<pre><code class="language-python">def two():
    pass
</code></pre>
</body>
</html>
"#;

/// The DOM `textContent` of each part's `<pre>`: anchors number across
/// parts in document order.
const COLLECTION_ANCHOR_1: &str = "fn one() {\n\tprintln!(\"one\");\n}\n";
const COLLECTION_ANCHOR_2: &str = "def two():\n    pass\n";

fn collection_store_command(
    temp: &TempDir,
    vault: &Path,
    curl: &Path,
    adapter: &Path,
) -> Command {
    let mut command = bob_command();
    command
        .arg("ref")
        .arg("create")
        .arg(COLLECTION_URL_1)
        .arg(COLLECTION_URL_2)
        .arg("-N")
        .arg("packet")
        .arg("-P")
        .arg("sase")
        .arg("-b")
        .arg(vault)
        .env("BOB_DIR", vault)
        .env("BOB_HIGHLIGHTS_INGEST_ON_CREATE", "1")
        .env("BOB_REF_INTAKE_LOCK_WAIT_SECONDS", "0")
        .env(
            "BOB_VAULT_SYNC_LOCK_FILE",
            temp.path().join("bob_sync.lock"),
        )
        .env(
            "BOB_VAULT_SYNC_STATE_FILE",
            temp.path().join("vault-sync.json"),
        )
        .env("XDG_STATE_HOME", temp.path().join("state"))
        .env("BOB_HIGHLIGHTS_CURL", curl)
        .env("BOB_WEB_CLIP_ADAPTER", adapter);
    command
}

/// Stage the composed source view plus its cross-part anchors, and swap
/// the compose fixture PDF for one carrying both anchor destinations.
fn stage_collection_view(temp: &TempDir, fake: &FakeClip) {
    let render = temp.path().join("render");
    fs::create_dir_all(&render).expect("create render dir");
    fs::write(render.join("source.html"), COLLECTION_SOURCE_HTML)
        .expect("write composed source view");
    let anchors = serde_json::json!({
        "version": 1,
        "blocks": [
            {"k": 1, "text": COLLECTION_ANCHOR_1},
            {"k": 2, "text": COLLECTION_ANCHOR_2},
        ],
    });
    fs::write(
        render.join("code-anchors.json"),
        serde_json::to_string(&anchors).expect("anchors JSON"),
    )
    .expect("write code anchors");
    fs::write(
        fake.root.join("fixture.pdf"),
        web_fixture_pdf_with_blocks(LINK_URI, 2),
    )
    .expect("write collection fixture PDF");
    let compose = serde_json::json!({
        "protocol": 1,
        "ok": true,
        "op": "compose",
        "pdf_bytes": 2400,
        "source_html": render.join("source.html").to_string_lossy(),
        "code_anchors": render.join("code-anchors.json").to_string_lossy(),
        "images": {"total": 0, "kept": 0, "skipped_small": 0, "failed": 0},
        "warnings": [],
    });
    fs::write(
        fake.root.join("compose.json"),
        serde_json::to_string(&compose).expect("compose JSON"),
    )
    .expect("write fake compose response");
}

#[test]
fn web_collection_create_saves_one_source_and_catalog() {
    if skip_unless_pandoc("web collection source store") {
        return;
    }
    let temp = TempDir::new("bob-cli-web-collection-store");
    let (vault, _peer) = seed_synced_vault(&temp);
    let curl = write_fake_curl(temp.path());
    let fake = FakeClip::new(&temp, "adapter");
    stage_collection_view(&temp, &fake);

    let output = collection_store_command(&temp, &vault, &curl, &fake.path)
        .output()
        .expect("run web collection create");
    assert_success(&output);
    let report = stdout(&output);
    let pointer = reported_pointer(&report);
    assert!(
        pointer.starts_with(".bob/ref-sources/blogs/packet--")
            && pointer.ends_with("/source.md"),
        "unexpected pointer: {pointer}\n{report}"
    );
    assert!(
        report.contains("code: 2 blocks (rust, python)"),
        "missing code summary:\n{report}"
    );
    assert!(
        report.contains("links: 1 link (example.com)"),
        "missing links summary:\n{report}"
    );

    let source_path = vault.join(&pointer);
    assert!(source_path.is_file(), "missing saved source: {pointer}");
    let saved = fs::read_to_string(&source_path).expect("read source.md");
    assert!(
        saved.contains("## Part One")
            && saved.contains("## Part Two")
            && saved.contains("``` rust")
            && saved.contains("``` python")
            && saved.contains("\tprintln!(\"one\");"),
        "one store must hold both parts:\n{saved}"
    );
    let store_dir = source_path.parent().expect("store dir").to_path_buf();
    let catalog = fs::read_to_string(store_dir.join("code.json"))
        .expect("read code.json");
    assert!(
        catalog.contains("\"route\": \"web_collection\""),
        "wrong route in catalog:\n{catalog}"
    );

    let note = fs::read_to_string(vault.join("ref/blogs/packet.md"))
        .expect("read filed note");
    assert!(
        note.lines()
            .any(|line| line.trim() == format!("source_markdown: {pointer}")),
        "note must carry the pointer:\n{note}"
    );

    let output = run_ref(&vault, &["code", "packet", "-f", "json"]);
    assert_success(&output);
    let value = parse_json(&output);
    assert_eq!(value["status"], "available", "{value}");
    let blocks = value["blocks"].as_array().expect("blocks array");
    assert_eq!(blocks.len(), 2, "{value}");
    assert_eq!(
        blocks[0]["text"], "fn one() {\n\tprintln!(\"one\");\n}",
        "exact tabbed bytes: {value}"
    );
    assert_eq!(blocks[1]["text"], "def two():\n    pass", "{value}");
    for block in blocks {
        assert_eq!(block["page"], 1, "{value}");
        assert_eq!(block["page_end"], 1, "{value}");
        assert_eq!(block["page_source"], "anchor", "{value}");
    }

    let output = run_ref(&vault, &["links", "packet", "-f", "json"]);
    assert_success(&output);
    let value = parse_json(&output);
    assert_eq!(value["status"], "available", "{value}");
    let links = value["links"].as_array().expect("links array");
    assert_eq!(links.len(), 1, "{value}");
    assert_eq!(links[0]["url"], LINK_URI, "{value}");
    assert_eq!(links[0]["page"], 1, "{value}");
    assert_eq!(links[0]["page_source"], "annotation", "{value}");
}
