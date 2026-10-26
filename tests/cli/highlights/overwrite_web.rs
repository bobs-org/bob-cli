//! Single-URL web overwrite: article, saved HTML, PDF URL, and arXiv.
//!
//! Each URL kind replaces its filed member through the same staged
//! overwrite transaction as local routes, preserving redirect,
//! duplicate-before-fetch, and listen semantics.

use super::fake_clip::FakeClip;
use super::overwrite_local::snapshot_tree;
use crate::support::*;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

const FIXED_NOW: &str = "2026-10-16 12:00:00";

struct WebVault {
    temp: TempDir,
    vault: PathBuf,
}

fn web_vault(prefix: &str) -> WebVault {
    let temp = TempDir::new(prefix);
    let (vault, _remote, _peer) = init_vault_sync_pair(&temp);
    write_area_note(&vault.join("sase.md"), "SASE");
    git_in(&vault, ["add", "sase.md"]);
    git_in(&vault, ["commit", "-q", "-m", "seed parent"]);
    git_in(&vault, ["push", "-q", "origin", "master"]);
    WebVault { temp, vault }
}

fn file_web_ref(
    vault: &Path,
    stem: &str,
    ref_type: &str,
    title: &str,
    source_url: &str,
) {
    let pdf = vault.join(format!("lib/{ref_type}/{stem}.pdf"));
    write_highlights_pdf(
        &pdf,
        &format!(
            concat!(
                "- status: ready\n",
                "- parent: sase\n",
                "- title: {title}\n",
                "- id: {stem}\n",
                "- source_url: {source_url}\n",
                "- captured: 2026-01-01\n",
            ),
            title = title,
            stem = stem,
            source_url = source_url,
        ),
    );
    let output = bob_command()
        .args(["ref", "sync"])
        .arg(&pdf)
        .env("BOB_DIR", vault)
        .env("BOB_NOW", FIXED_NOW)
        .output()
        .expect("file the reference");
    assert_success(&output);
}

fn commit_vault(vault: &Path, message: &str) {
    git_in(vault, ["add", "-A"]);
    git_in(vault, ["commit", "-q", "-m", message]);
    git_in(vault, ["push", "-q", "origin", "master"]);
}

/// Intake files its own commit, so a post-create stabilization must not
/// fail when the tree is already clean.
fn commit_if_dirty(vault: &Path, message: &str) {
    git_in(vault, ["add", "-A"]);
    let status = Command::new("git")
        .arg("-c")
        .arg("color.ui=false")
        .arg("-C")
        .arg(vault)
        .args(["status", "--porcelain"])
        .output()
        .expect("git status");
    if !status.stdout.is_empty() {
        git_in(vault, ["commit", "-q", "-m", message]);
    }
    git_in(vault, ["push", "-q", "origin", "master"]);
}

fn web_command(fixture: &WebVault, args: &[&str]) -> Command {
    let mut command = bob_command();
    command.arg("ref").arg("create");
    for arg in args {
        command.arg(arg);
    }
    command
        .arg("-b")
        .arg(&fixture.vault)
        .env("BOB_DIR", &fixture.vault)
        .env("BOB_NOW", FIXED_NOW)
        .env("NO_COLOR", "1")
        .env("HOME", fixture.temp.path().join("home"))
        .env("XDG_STATE_HOME", fixture.temp.path().join("state"))
        .env("XDG_CACHE_HOME", fixture.temp.path().join("cache"))
        .env(
            "BOB_VAULT_SYNC_LOCK_FILE",
            fixture.temp.path().join("bob_sync.lock"),
        )
        .env(
            "BOB_VAULT_SYNC_STATE_FILE",
            fixture.temp.path().join("vault-sync.json"),
        )
        .env("BOB_HIGHLIGHTS_INGEST_ON_CREATE", "1");
    command
}

fn with_network(
    command: &mut Command,
    adapter: &Path,
    curl: &Path,
    root: &Path,
    log: &Path,
) {
    command
        .env("BOB_WEB_CLIP_ADAPTER", adapter)
        .env("BOB_HIGHLIGHTS_CURL", curl)
        .env("FAKE_CURL_ROOT", root)
        .env("FAKE_CURL_LOG", log);
}

fn write_bare_pdf(path: &Path, title: Option<&str>, author: Option<&str>) {
    use lopdf::{dictionary, Document, Object, Stream};
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("create bare PDF parent");
    }
    let mut doc = Document::with_version("1.4");
    let pages_id = doc.new_object_id();
    let content_id = doc.add_object(Stream::new(dictionary! {}, Vec::new()));
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
    if info.len() > 0 {
        let info_id = doc.add_object(info);
        doc.trailer.set("Info", info_id);
    }
    doc.save(path).expect("write bare PDF");
}

fn add_pdf_annotation(path: &Path, subtype: &[u8]) {
    use lopdf::{dictionary, Document, Object, StringFormat};

    let mut document = Document::load(path).expect("load reference PDF");
    let page_id = document.page_iter().next().expect("first PDF page");
    let mut annotation = dictionary! {
        "Type" => "Annot",
        "Subtype" => Object::Name(subtype.to_vec()),
        "Rect" => vec![
            Object::Integer(0),
            Object::Integer(0),
            Object::Integer(20),
            Object::Integer(20),
        ],
        "Contents" => Object::String(b"test annotation".to_vec(), StringFormat::Literal),
    };
    if subtype == b"Link" {
        let action_id = document.add_object(dictionary! {
            "S" => "URI",
            "URI" => Object::String(b"https://example.com/".to_vec(), StringFormat::Literal),
        });
        annotation.set("A", Object::Reference(action_id));
    }
    let annotation_id = document.add_object(annotation);
    let page = document
        .get_object_mut(page_id)
        .expect("page object")
        .as_dict_mut()
        .expect("page dictionary");
    let mut annotations = page
        .get(b"Annots")
        .ok()
        .and_then(|value| value.as_array().ok())
        .cloned()
        .unwrap_or_default();
    annotations.push(Object::Reference(annotation_id));
    page.set("Annots", Object::Array(annotations));
    document.save(path).expect("save reference PDF annotation");
}

fn write_fake_curl(dir: &Path) -> PathBuf {
    let path = dir.join("fake-curl.sh");
    let script = r#"#!/bin/sh
# Fake curl for web overwrite tests. Serves canned bodies by URL,
# logs the request URL to $FAKE_CURL_LOG, and prints the -w trailer.
dest=""
url=""
prev=""
for arg in "$@"; do
  if [ "$prev" = "-o" ]; then dest="$arg"; fi
  prev="$arg"
  url="$arg"
done
echo "$url" >> "$FAKE_CURL_LOG"
case "$url" in
  *"export.arxiv.org/api/query"*id_list=1706.03762*)
    cat "$FAKE_CURL_ROOT/arxiv-api.xml" > "$dest"
    printf '200\napplication/atom+xml; charset=utf-8\n\n'
    ;;
  *"export.arxiv.org/api/query"*)
    printf '500\ntext/plain\n\n'
    ;;
  *"arxiv.org/pdf/1706.03762"*)
    cp "$FAKE_CURL_ROOT/arxiv.pdf" "$dest"
    printf '200\napplication/pdf\n\n'
    ;;
  *"example.com/paper.pdf"*)
    cp "$FAKE_CURL_ROOT/paper.pdf" "$dest"
    printf '200\napplication/pdf\n\n'
    ;;
  *"example.com/article"*)
    printf '<html><body>article</body></html>' > "$dest"
    printf '200\ntext/html; charset=utf-8\n\n'
    ;;
  *)
    printf '404\ntext/html\n\n'
    ;;
esac
"#;
    fs::write(&path, script).expect("write fake curl");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755))
            .expect("chmod fake curl");
    }
    path
}

fn write_arxiv_api_fixture(root: &Path, title: &str) {
    let xml = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<feed xmlns="http://www.w3.org/2005/Atom">
<entry>
<id>http://arxiv.org/abs/1706.03762v7</id>
<updated>2023-08-10T00:00:00Z</updated>
<published>2017-06-12T17:57:34Z</published>
<title>{title}</title>
<author><name>Ashish Vaswani</name></author>
<author><name>Noam Shazeer</name></author>
</entry>
</feed>"#
    );
    fs::write(root.join("arxiv-api.xml"), xml).expect("write arxiv api");
}

fn article_capture(title: &str, final_url: &str) -> String {
    format!(
        r#"{{"protocol":1,"ok":true,"op":"capture","kind":"article","final_url":"{final_url}","title":"{title}","author":"Jane Doe","published":"2026-04-27","site":"Example","description":"","metadata_sources":{{}},"word_count":8921,"capture":{{"browser":"chrome","browser_version":"154.0.0","mode":"headless","retried_after_challenge":false}},"fidelity":{{"status":"ok","page_large_media":0,"kept_large_media":0,"page_code_blocks":0,"kept_code_blocks":0,"page_words":8921,"kept_words":8921}},"images":{{"total":1,"kept":1,"skipped_small":0,"failed":0}},"pdf_bytes":null,"warnings":[]}}"#
    )
}

fn write_fake_sase(dir: &Path) -> PathBuf {
    let bin = dir.join("fake-sase-bin");
    fs::create_dir_all(&bin).expect("fake sase bin");
    write_executable(
        &bin.join("sase"),
        r#"#!/bin/sh
echo "$@" >> "$FAKE_LISTEN_LOG"
if [ "$1" != "listen" ] || [ "$2" != "render" ]; then
  echo "bad-launcher:$1:$2" >> "$FAKE_LISTEN_LOG"
  exit 99
fi
shift 2
audio_path=""
for arg in "$@"; do
  case "$arg" in
    *.mp3) audio_path="$arg" ;;
  esac
done
if [ -n "$audio_path" ]; then
  printf 'ID3 fake episode' > "$audio_path"
fi
exit 0
"#,
    );
    bin
}

fn curl_log_is_empty(log: &Path) -> bool {
    fs::read_to_string(log).expect("read curl log").is_empty()
}

#[test]
fn article_overwrite_replaces_filed_reference() {
    let fixture = web_vault("bob-cli-overwrite-web-article");
    file_web_ref(
        &fixture.vault,
        "hello",
        "blogs",
        "Symphony Spec",
        "https://example.com/article/hello",
    );
    commit_vault(&fixture.vault, "file reference");
    let root = fixture.temp.path().join("curl-root");
    fs::create_dir_all(&root).expect("create curl root");
    let curl = write_fake_curl(fixture.temp.path());
    let log = fixture.temp.path().join("curl.log");
    fs::write(&log, "").expect("init log");
    let fake = FakeClip::new(&fixture.temp, "adapter");
    fake.respond(&article_capture(
        "Symphony Spec Revised",
        "https://example.com/article/hello",
    ));

    let mut command = web_command(
        &fixture,
        &[
            "https://example.com/article/hello",
            "-P",
            "sase",
            "-f",
            "overwrite",
        ],
    );
    with_network(&mut command, &fake.path, &curl, &root, &log);
    let output = command.output().expect("overwrite article");
    assert_success(&output);
    let report = stdout(&output);
    assert!(
        report.contains("ok replaced Highlights-ready PDF")
            && report.contains("overwrite: blogs/hello")
            && report.contains("title: Symphony Spec Revised")
            && report.contains("earlier: nothing to archive")
            && report.contains("task: kept sase.md ^ref-hello [ ] ready")
            && report.contains("committed"),
        "article overwrite replaces in place:\n{report}"
    );
    assert!(fake.called(), "the adapter renders the replacement");
    assert!(
        !curl_log_is_empty(&log),
        "the URL is fetched to classify the route"
    );
    let note = fs::read_to_string(fixture.vault.join("ref/blogs/hello.md"))
        .expect("read replaced note");
    assert!(
        note.contains("Symphony Spec Revised"),
        "the live note carries the new title:\n{note}"
    );
    assert!(
        !fixture.vault.join("ref/blogs/hello__v2.md").exists(),
        "no reread reading is allocated"
    );
}

#[test]
fn saved_html_overwrite_replays_without_fetch() {
    let fixture = web_vault("bob-cli-overwrite-web-saved-html");
    file_web_ref(
        &fixture.vault,
        "hello",
        "blogs",
        "Symphony Spec",
        "https://example.com/article/hello",
    );
    commit_vault(&fixture.vault, "file reference");
    let html = fixture.temp.path().join("saved.html");
    write_file(&html, "<html><body>saved article</body></html>");
    let fake = FakeClip::new(&fixture.temp, "adapter");
    fake.respond(&article_capture(
        "Symphony Spec Saved",
        "https://example.com/article/hello",
    ));

    let output = web_command(
        &fixture,
        &[
            "https://example.com/article/hello",
            "-P",
            "sase",
            "-f",
            "overwrite",
            "-H",
            path_str(&html),
        ],
    )
    .env("BOB_WEB_CLIP_ADAPTER", &fake.path)
    .env("BOB_HIGHLIGHTS_CURL", "/definitely/missing/curl")
    .output()
    .expect("overwrite saved HTML");
    assert_success(&output);
    let report = stdout(&output);
    assert!(
        report.contains("ok replaced Highlights-ready PDF")
            && report.contains("overwrite: blogs/hello")
            && report.contains("title: Symphony Spec Saved"),
        "saved HTML replays through the adapter:\n{report}"
    );
    assert!(fake.called(), "the adapter replays the saved page");
}

#[test]
fn pdf_url_overwrite_stamps_fetched_pdf() {
    let fixture = web_vault("bob-cli-overwrite-web-pdf-url");
    file_web_ref(
        &fixture.vault,
        "paper",
        "papers",
        "Remote Paper Title",
        "https://example.com/paper.pdf",
    );
    commit_vault(&fixture.vault, "file reference");
    let root = fixture.temp.path().join("curl-root");
    fs::create_dir_all(&root).expect("create curl root");
    write_bare_pdf(
        &root.join("paper.pdf"),
        Some("Remote Paper Title Revised"),
        Some("Jane Doe"),
    );
    let curl = write_fake_curl(fixture.temp.path());
    let log = fixture.temp.path().join("curl.log");
    fs::write(&log, "").expect("init log");
    let fake = FakeClip::new(&fixture.temp, "adapter");

    let mut command = web_command(
        &fixture,
        &[
            "https://example.com/paper.pdf",
            "-P",
            "sase",
            "-f",
            "overwrite",
        ],
    );
    with_network(&mut command, &fake.path, &curl, &root, &log);
    let output = command.output().expect("overwrite PDF URL");
    assert_success(&output);
    let report = stdout(&output);
    assert!(
        report.contains("ok replaced Highlights-ready PDF")
            && report.contains("overwrite: papers/paper")
            && report.contains("title: Remote Paper Title Revised")
            && report.contains("task: kept sase.md ^ref-paper [ ] ready"),
        "PDF URL overwrite stamps the fetched bytes:\n{report}"
    );
    assert!(
        !fake.called(),
        "a PDF route must not run the article adapter"
    );
    assert!(
        !curl_log_is_empty(&log),
        "the PDF body is fetched before stamping"
    );
}

#[test]
fn arxiv_overwrite_keeps_api_metadata() {
    let fixture = web_vault("bob-cli-overwrite-web-arxiv");
    file_web_ref(
        &fixture.vault,
        "attention_is_all_you_need",
        "papers",
        "Attention Is All You Need",
        "https://arxiv.org/abs/1706.03762",
    );
    commit_vault(&fixture.vault, "file reference");
    let root = fixture.temp.path().join("curl-root");
    fs::create_dir_all(&root).expect("create curl root");
    write_bare_pdf(&root.join("paper.pdf"), Some("Remote"), None);
    fs::copy(root.join("paper.pdf"), root.join("arxiv.pdf"))
        .expect("copy arxiv pdf");
    write_arxiv_api_fixture(&root, "Attention Is All You Need Revised");
    let curl = write_fake_curl(fixture.temp.path());
    let log = fixture.temp.path().join("curl.log");
    fs::write(&log, "").expect("init log");
    let fake = FakeClip::new(&fixture.temp, "adapter");

    let mut command = web_command(
        &fixture,
        &[
            "https://arxiv.org/abs/1706.03762",
            "-P",
            "sase",
            "-f",
            "overwrite",
        ],
    );
    with_network(&mut command, &fake.path, &curl, &root, &log);
    let output = command.output().expect("overwrite arXiv");
    assert_success(&output);
    let report = stdout(&output);
    assert!(
        report.contains("ok replaced Highlights-ready PDF")
            && report.contains("overwrite: papers/attention_is_all_you_need")
            && report.contains("title: Attention Is All You Need Revised"),
        "arXiv overwrite refreshes from the API:\n{report}"
    );
    assert!(
        !fake.called(),
        "an arXiv route must not run the article adapter"
    );
}

#[test]
fn web_overwrite_dry_run_writes_nothing_without_network() {
    let fixture = web_vault("bob-cli-overwrite-web-dry-run");
    file_web_ref(
        &fixture.vault,
        "hello",
        "blogs",
        "Symphony Spec",
        "https://example.com/article/hello",
    );
    commit_vault(&fixture.vault, "file reference");
    let fake = FakeClip::new(&fixture.temp, "adapter");

    let output = web_command(
        &fixture,
        &[
            "https://example.com/article/hello",
            "-P",
            "sase",
            "-f",
            "overwrite",
            "-d",
        ],
    )
    .env("BOB_WEB_CLIP_ADAPTER", &fake.path)
    .env("BOB_HIGHLIGHTS_CURL", "/definitely/missing/curl")
    .output()
    .expect("dry-run web overwrite");
    assert_success(&output);
    let report = stdout(&output);
    assert!(
        report.contains("ok would replace")
            && report.contains("overwrite: blogs/hello")
            && report.contains("task: kept sase.md ^ref-hello [ ] ready")
            && report.contains("writes: none"),
        "dry runs describe the plan and stop before fetch:\n{report}"
    );
    assert!(!fake.called(), "dry runs must not run the adapter");
}

#[test]
fn web_overwrite_refuses_disabled_ingest_before_dry_run_or_network() {
    let fixture = web_vault("bob-cli-overwrite-web-ingest-disabled");
    file_web_ref(
        &fixture.vault,
        "hello",
        "blogs",
        "Symphony Spec",
        "https://example.com/article/hello",
    );
    commit_vault(&fixture.vault, "file reference");
    let before_tree = snapshot_tree(&fixture.vault);
    let before_head = stdout(&git_in(&fixture.vault, ["rev-parse", "HEAD"]))
        .trim()
        .to_string();
    let root = fixture.temp.path().join("curl-root");
    fs::create_dir_all(&root).expect("create curl root");
    let curl = write_fake_curl(fixture.temp.path());
    let log = fixture.temp.path().join("curl.log");
    fs::write(&log, "").expect("init curl log");
    let fake = FakeClip::new(&fixture.temp, "adapter");
    let mut command = web_command(
        &fixture,
        &[
            "https://example.com/article/hello",
            "-P",
            "sase",
            "-f",
            "overwrite",
            "-d",
        ],
    );
    command.env("BOB_HIGHLIGHTS_INGEST_ON_CREATE", "0");
    with_network(&mut command, &fake.path, &curl, &root, &log);
    let output = command.output().expect("disabled-ingest dry-run");
    assert_eq!(output.status.code(), Some(1), "{}", format_output(&output));
    assert!(
        stderr(&output).contains("intake_unavailable")
            && stderr(&output)
                .contains("disabled by BOB_HIGHLIGHTS_INGEST_ON_CREATE")
            && stderr(&output).contains("BOB_HIGHLIGHTS_INGEST_ON_CREATE=1"),
        "refusal names the reason and enabling hint:\n{}",
        format_output(&output)
    );
    assert!(!fake.called(), "disabled ingest must refuse before adapter");
    assert!(fs::read_to_string(&log).expect("curl log").is_empty());
    assert_eq!(snapshot_tree(&fixture.vault), before_tree);
    assert_eq!(
        stdout(&git_in(&fixture.vault, ["rev-parse", "HEAD"])).trim(),
        before_head
    );
}

#[test]
fn web_overwrite_redirect_to_other_family_refuses() {
    let fixture = web_vault("bob-cli-overwrite-web-redirect");
    file_web_ref(
        &fixture.vault,
        "essay_a",
        "blogs",
        "Essay A",
        "https://example.com/article/a",
    );
    file_web_ref(
        &fixture.vault,
        "essay_b",
        "blogs",
        "Essay B",
        "https://example.com/article/b",
    );
    commit_vault(&fixture.vault, "file references");
    let before_a = fs::read(fixture.vault.join("ref/blogs/essay_a.md"))
        .expect("read essay A note");
    let root = fixture.temp.path().join("curl-root");
    fs::create_dir_all(&root).expect("create curl root");
    let curl = write_fake_curl(fixture.temp.path());
    let log = fixture.temp.path().join("curl.log");
    fs::write(&log, "").expect("init log");
    let fake = FakeClip::new(&fixture.temp, "adapter");
    // The adapter claims the fetch landed on the other filed reference.
    fake.respond(&article_capture("Essay B", "https://example.com/article/b"));

    let mut command = web_command(
        &fixture,
        &[
            "https://example.com/article/a",
            "-P",
            "sase",
            "-f",
            "overwrite",
        ],
    );
    with_network(&mut command, &fake.path, &curl, &root, &log);
    let output = command.output().expect("overwrite with redirect");
    assert_ne!(
        output.status.code(),
        Some(0),
        "a redirect into another family must refuse:\n{}",
        format_output(&output)
    );
    let diagnostic = format!("{}{}", stdout(&output), stderr(&output));
    assert!(
        diagnostic.contains("redirected to https://example.com/article/b")
            && diagnostic.contains("essay_b"),
        "the refusal names the redirect and the other family:\n{diagnostic}"
    );
    assert!(fake.called(), "the redirect is detected after the adapter");
    assert_eq!(
        fs::read(fixture.vault.join("ref/blogs/essay_a.md"))
            .expect("reread essay A note"),
        before_a,
        "a refused redirect leaves the filed note untouched"
    );
}

#[test]
fn annotated_web_target_refuses_without_network() {
    let fixture = web_vault("bob-cli-overwrite-web-annotated");
    file_web_ref(
        &fixture.vault,
        "hello",
        "blogs",
        "Symphony Spec",
        "https://example.com/article/hello",
    );
    let assets = fixture.vault.join("ref/blogs/hello.assets");
    fs::create_dir_all(&assets).expect("create assets dir");
    write_file(&assets.join("clip.mp4"), "evidence of annotation");
    commit_vault(&fixture.vault, "file annotated reference");
    let fake = FakeClip::new(&fixture.temp, "adapter");
    let curl_log = fixture.temp.path().join("curl.log");
    fs::write(&curl_log, "").expect("init log");

    let output = web_command(
        &fixture,
        &[
            "https://example.com/article/hello",
            "-P",
            "sase",
            "-f",
            "overwrite",
        ],
    )
    .env("BOB_WEB_CLIP_ADAPTER", &fake.path)
    .env("BOB_HIGHLIGHTS_CURL", "/definitely/missing/curl")
    .env("FAKE_CURL_LOG", &curl_log)
    .output()
    .expect("overwrite annotated reference");
    assert_eq!(
        output.status.code(),
        Some(2),
        "annotated overwrite must refuse:\n{}",
        format_output(&output)
    );
    assert!(
        stderr(&output).contains(
            "this reference has 0 highlights, which Highlights holds on the Mac; run the overwrite there"
        ),
        "the host-coordination gate wins:\n{}",
        format_output(&output)
    );
    assert!(!fake.called(), "annotated targets do no adapter work");
}

#[test]
fn every_annotation_source_refuses_with_the_contract_remedy() {
    for kind in ["sidecar", "pdf", "fragment", "follow-up"] {
        let fixture =
            web_vault(&format!("bob-cli-overwrite-annotation-{kind}"));
        file_web_ref(
            &fixture.vault,
            "hello",
            "blogs",
            "Symphony Spec",
            "https://example.com/article/hello",
        );
        commit_vault(&fixture.vault, "file reference before annotation");
        match kind {
            "sidecar" => write_file(
                &fixture.vault.join("lib/blogs/hello.md"),
                "## Page 1\n\n> [!quote] Highlighted words\n>\n> Sidecar evidence.\n",
            ),
            "pdf" => add_pdf_annotation(
                &fixture.vault.join("lib/blogs/hello.pdf"),
                b"Highlight",
            ),
            "fragment" => {
                let parent = fixture.vault.join("sase.md");
                let mut contents = fs::read_to_string(&parent).expect("parent");
                contents.push_str("\nA link to [[ref/blogs/hello#^h-test]].\n");
                fs::write(parent, contents).expect("write inbound highlight link");
            }
            "follow-up" => {
                let parent = fixture.vault.join("sase.md");
                let mut contents = fs::read_to_string(&parent).expect("parent");
                contents.push_str(
                    "\n- [ ] #task Follow up [[ref/blogs/hello#^h-test]] [h:: note]\n",
                );
                fs::write(parent, contents).expect("write inbound follow-up");
            }
            _ => unreachable!(),
        }
        if matches!(kind, "sidecar" | "pdf") {
            let sync = bob_command()
                .args(["ref", "sync"])
                .arg(fixture.vault.join("lib/blogs/hello.pdf"))
                .env("BOB_DIR", &fixture.vault)
                .output()
                .expect("sync annotation evidence");
            assert_success(&sync);
        }
        commit_vault(&fixture.vault, "add annotation evidence");

        let before_tree = snapshot_tree(&fixture.vault);
        let before_head =
            stdout(&git_in(&fixture.vault, ["rev-parse", "HEAD"]))
                .trim()
                .to_string();
        let before_status = stdout(&git_in(
            &fixture.vault,
            ["status", "--porcelain=v1", "--untracked-files=all"],
        ));
        let state = fixture.temp.path().join("state");
        let intent =
            super::overwrite_recovery::intent_path(&state, &fixture.vault);
        let before_intent = fs::read(&intent).ok();
        let root = fixture.temp.path().join("curl-root");
        fs::create_dir_all(&root).expect("create curl root");
        let curl = write_fake_curl(fixture.temp.path());
        let log = fixture.temp.path().join("curl.log");
        fs::write(&log, "").expect("init curl log");
        let fake = FakeClip::new(&fixture.temp, "adapter");
        let mut command = web_command(
            &fixture,
            &[
                "https://example.com/article/hello",
                "-P",
                "sase",
                "-f",
                "overwrite",
            ],
        );
        with_network(&mut command, &fake.path, &curl, &root, &log);
        let output = command.output().expect("attempt annotated overwrite");
        assert_eq!(
            output.status.code(),
            Some(2),
            "{kind} evidence must refuse:\n{}",
            format_output(&output)
        );
        let diagnostic = stderr(&output);
        assert!(
            diagnostic.contains("annotated_reference")
                && diagnostic.contains(
                    "which Highlights holds on the Mac; run the overwrite there"
                )
                && diagnostic.contains("-f starts a new reading from any host"),
            "{kind} refusal must include its code and host remedy:\n{diagnostic}"
        );
        assert!(!fake.called(), "{kind} refusal must precede adapters");
        assert!(
            fs::read_to_string(&log).expect("curl log").is_empty(),
            "{kind} refusal must precede fetch"
        );
        assert_eq!(snapshot_tree(&fixture.vault), before_tree);
        assert_eq!(
            stdout(&git_in(&fixture.vault, ["rev-parse", "HEAD"])).trim(),
            before_head
        );
        assert_eq!(
            stdout(&git_in(
                &fixture.vault,
                ["status", "--porcelain=v1", "--untracked-files=all"]
            )),
            before_status
        );
        assert_eq!(fs::read(intent).ok(), before_intent);
    }
}

#[test]
fn pdf_link_annotations_for_navigation_and_audio_do_not_refuse() {
    let fixture = web_vault("bob-cli-overwrite-web-link-annotations");
    file_web_ref(
        &fixture.vault,
        "hello",
        "blogs",
        "Symphony Spec",
        "https://example.com/article/hello",
    );
    commit_vault(&fixture.vault, "file reference before PDF links");
    let pdf = fixture.vault.join("lib/blogs/hello.pdf");
    add_pdf_annotation(&pdf, b"Link");
    add_pdf_annotation(&pdf, b"Link");
    let sync = bob_command()
        .args(["ref", "sync"])
        .arg(&pdf)
        .env("BOB_DIR", &fixture.vault)
        .output()
        .expect("sync PDF Link annotations");
    assert_success(&sync);
    commit_vault(&fixture.vault, "add navigation and audio links");

    let root = fixture.temp.path().join("curl-root");
    fs::create_dir_all(&root).expect("create curl root");
    let curl = write_fake_curl(fixture.temp.path());
    let log = fixture.temp.path().join("curl.log");
    fs::write(&log, "").expect("init curl log");
    let fake = FakeClip::new(&fixture.temp, "adapter");
    fake.respond(&article_capture(
        "Symphony Spec Revised",
        "https://example.com/article/hello",
    ));
    let mut command = web_command(
        &fixture,
        &[
            "https://example.com/article/hello",
            "-P",
            "sase",
            "-f",
            "overwrite",
        ],
    );
    with_network(&mut command, &fake.path, &curl, &root, &log);
    let output = command
        .output()
        .expect("overwrite with PDF Link annotations");
    assert_success(&output);
    assert!(
        stdout(&output).contains("ok replaced Highlights-ready PDF"),
        "Link annotations alone must allow overwrite:\n{}",
        format_output(&output)
    );
}

#[test]
fn web_overwrite_listen_narrates_replacement() {
    let fixture = web_vault("bob-cli-overwrite-web-listen");
    file_web_ref(
        &fixture.vault,
        "hello",
        "blogs",
        "Symphony Spec",
        "https://example.com/article/hello",
    );
    commit_vault(&fixture.vault, "file reference");
    let root = fixture.temp.path().join("curl-root");
    fs::create_dir_all(&root).expect("create curl root");
    let curl = write_fake_curl(fixture.temp.path());
    let log = fixture.temp.path().join("curl.log");
    fs::write(&log, "").expect("init log");
    let fake = FakeClip::new(&fixture.temp, "adapter");
    fake.respond(&article_capture(
        "Symphony Spec Revised",
        "https://example.com/article/hello",
    ));
    let bin = write_fake_sase(fixture.temp.path());
    let listen_log = fixture.temp.path().join("listen.log");

    // Without force, `-L` still attaches a new episode to the filed capture.
    let mut attach = web_command(
        &fixture,
        &["https://example.com/article/hello", "-P", "sase", "-L"],
    );
    with_network(&mut attach, &fake.path, &curl, &root, &log);
    let attached = attach
        .env("PATH", path_with_prefix(&bin))
        .env(
            "BOB_HIGHLIGHTS_LISTEN_COMMAND",
            "sase listen render {target} -o {audio}",
        )
        .env("FAKE_LISTEN_LOG", &listen_log)
        .output()
        .expect("attach listen episode");
    assert_success(&attached);

    // With `-f overwrite`, `-L` narrates the in-place replacement instead.
    let mut command = web_command(
        &fixture,
        &[
            "https://example.com/article/hello",
            "-P",
            "sase",
            "-f",
            "overwrite",
            "-L",
        ],
    );
    with_network(&mut command, &fake.path, &curl, &root, &log);
    let output = command
        .env("PATH", path_with_prefix(&bin))
        .env(
            "BOB_HIGHLIGHTS_LISTEN_COMMAND",
            "sase listen render {target} -o {audio}",
        )
        .env("FAKE_LISTEN_LOG", &listen_log)
        .output()
        .expect("overwrite with listen");
    assert_success(&output);
    let report = stdout(&output);
    assert!(
        report.contains("ok replaced Highlights-ready PDF")
            && report.contains("audio: replaced lib/blogs/hello.mp3"),
        "overwrite -L narrates the replacement:\n{report}"
    );
    let audio = fs::read(fixture.vault.join("lib/blogs/hello.mp3"))
        .expect("replacement audio");
    assert_eq!(audio, b"ID3 fake episode", "listen writes the new episode");
}

#[test]
fn bare_web_reread_hints_at_dry_run() {
    let fixture = web_vault("bob-cli-overwrite-web-reread-hint");
    let target = "https://example.com/paper.pdf";
    file_web_ref(&fixture.vault, "hello", "papers", "Symphony Spec", target);
    let note_path = fixture.vault.join("ref/papers/hello.md");
    let note = fs::read_to_string(&note_path).expect("read filed reference");
    fs::write(
        &note_path,
        note.replacen(
            "status: ready\n",
            "status: ready\nhighlights_count: 0\n",
            1,
        ),
    )
    .expect("set known zero highlight count");
    commit_vault(&fixture.vault, "file reference");
    let expected =
        "hint: hello is still ready with 0 highlights; -f overwrite would replace it instead";

    let root = fixture.temp.path().join("curl-root");
    fs::create_dir_all(&root).expect("create curl root");
    write_bare_pdf(&root.join("paper.pdf"), Some("Symphony Spec"), None);
    let curl = write_fake_curl(fixture.temp.path());
    let log = fixture.temp.path().join("curl.log");
    fs::write(&log, "").expect("init curl log");
    let fake = FakeClip::new(&fixture.temp, "adapter");

    let mut command =
        web_command(&fixture, &[target, "-P", "sase", "-f", "-d"]);
    with_network(&mut command, &fake.path, &curl, &root, &log);
    let dry = command.output().expect("dry-run bare URL reread");
    assert_success(&dry);
    assert!(
        stdout(&dry).contains(expected),
        "dry run must print the overwrite hint:\n{}",
        stdout(&dry)
    );

    let mut reread = web_command(&fixture, &[target, "-P", "sase", "-f"]);
    with_network(&mut reread, &fake.path, &curl, &root, &log);
    let reread = reread.output().expect("real bare URL reread");
    assert_success(&reread);
    assert!(
        stdout(&reread).contains(expected),
        "real reread must print the overwrite hint:\n{}",
        stdout(&reread)
    );
}

#[test]
fn web_overwrite_selects_older_member_with_name() {
    let fixture = web_vault("bob-cli-overwrite-web-named-member");
    file_web_ref(
        &fixture.vault,
        "hello",
        "blogs",
        "Symphony Spec",
        "https://example.com/article/hello",
    );
    commit_vault(&fixture.vault, "file reference");
    let root = fixture.temp.path().join("curl-root");
    fs::create_dir_all(&root).expect("create curl root");
    let curl = write_fake_curl(fixture.temp.path());
    let log = fixture.temp.path().join("curl.log");
    fs::write(&log, "").expect("init log");
    let fake = FakeClip::new(&fixture.temp, "adapter");

    // Read the source again so the family holds two readings.
    fake.respond(&article_capture(
        "Symphony Spec Second",
        "https://example.com/article/hello",
    ));
    let mut reread = web_command(
        &fixture,
        &["https://example.com/article/hello", "-P", "sase", "-f"],
    );
    with_network(&mut reread, &fake.path, &curl, &root, &log);
    let reread_output = reread.output().expect("reread the article");
    assert_success(&reread_output);
    commit_if_dirty(&fixture.vault, "file reread");
    let older_note = fixture.vault.join("ref/blogs/hello.md");
    let newer_note = fixture.vault.join("ref/blogs/hello__v2.md");
    let older_pdf = fixture.vault.join("lib/blogs/hello.pdf");
    let newer_pdf = fixture.vault.join("lib/blogs/hello__v2.pdf");
    let newer_note_before = fs::read(&newer_note).expect("newer note before");
    let newer_pdf_before = fs::read(&newer_pdf).expect("newer PDF before");
    let older_note_before = fs::read(&older_note).expect("older note before");
    let older_pdf_before = fs::read(&older_pdf).expect("older PDF before");

    // An explicit `-N` replaces the named older reading in place.
    fake.respond(&article_capture(
        "Symphony Spec Revised",
        "https://example.com/article/hello",
    ));
    let mut command = web_command(
        &fixture,
        &[
            "https://example.com/article/hello",
            "-P",
            "sase",
            "-f",
            "overwrite",
            "-N",
            "hello",
        ],
    );
    with_network(&mut command, &fake.path, &curl, &root, &log);
    let output = command.output().expect("overwrite named member");
    assert_success(&output);
    let report = stdout(&output);
    assert!(
        report
            .lines()
            .any(|line| line == "overwrite: blogs/hello (ready · 0 highlights)")
            && report.contains("title: Symphony Spec Revised"),
        "the named member is replaced in place:\n{report}"
    );
    assert_ne!(
        fs::read(&older_note).expect("older note after"),
        older_note_before,
        "the selected root note must be replaced"
    );
    assert_ne!(
        fs::read(&older_pdf).expect("older PDF after"),
        older_pdf_before,
        "the selected root PDF must be replaced"
    );
    assert_eq!(
        fs::read(&newer_note).expect("newer note after"),
        newer_note_before,
        "the unselected newer note stays byte-identical"
    );
    assert_eq!(
        fs::read(&newer_pdf).expect("newer PDF after"),
        newer_pdf_before,
        "the unselected newer PDF stays byte-identical"
    );
}
