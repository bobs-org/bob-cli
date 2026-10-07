//! Shared fake `BOB_WEB_CLIP_ADAPTER` for highlights clip/create tests.
//!
//! The fake adapter saves each request to `request.json`, copies a tiny
//! fixture PDF to the requested `out_pdf`, and prints a canned response.
//! No test touches the network or a real browser.

use crate::support::*;
use std::fs;

pub(crate) struct FakeClip {
    pub(crate) root: std::path::PathBuf,
    pub(crate) path: std::path::PathBuf,
}

impl FakeClip {
    pub(crate) fn new(temp: &TempDir, name: &str) -> Self {
        let root = temp.path().join(name);
        fs::create_dir_all(&root).expect("create fake clip dir");
        let path = root.join("fake-clip-adapter.sh");
        let script = format!(
            r#"#!/bin/sh
# Fake web-clip adapter: one JSON request on stdin, one JSON response on
# stdout. Saves stdin to request.json, copies the fixture PDF to out_pdf,
# then prints the canned response file.
ROOT={root}
request=$(cat)
printf '%s' "$request" > "$ROOT/request.json"
case "$request" in
  *'"op":"ping"'*)
    if [ -f "$ROOT/ping.json" ]; then cat "$ROOT/ping.json"; else printf '%s' '{PING}'; fi
    ;;
  *)
    out_pdf=$(printf '%s' "$request" | sed -n 's/.*"out_pdf":"\([^"]*\)".*/\1/p')
    if [ -n "$out_pdf" ] && [ -f "$ROOT/fixture.pdf" ]; then cp "$ROOT/fixture.pdf" "$out_pdf"; fi
    html_path=$(printf '%s' "$request" | sed -n 's/.*"html_path":"\([^"]*\)".*/\1/p')
    if [ -n "$html_path" ] && [ -f "$html_path" ]; then cp "$html_path" "$ROOT/staged.html"; fi
    cat "$ROOT/response.json"
    ;;
esac
"#,
            root = shell_single_quote(path_str(&root)),
            PING = default_ping(),
        );
        write_executable(&path, &script);
        // The fixture must carry no annotations: the adapter's render is a
        // fresh PDF, and clip stamps the first page-1 marker onto it.
        write_bare_pdf(&root.join("fixture.pdf"));
        fs::write(root.join("response.json"), default_capture())
            .expect("write default capture response");
        Self { root, path }
    }

    pub(crate) fn respond(&self, body: &str) {
        fs::write(self.root.join("response.json"), body)
            .expect("write fake capture response");
    }

    pub(crate) fn request(&self) -> String {
        fs::read_to_string(self.root.join("request.json"))
            .expect("read saved adapter request")
    }

    pub(crate) fn called(&self) -> bool {
        self.root.join("request.json").exists()
    }
}

/// A one-page PDF with no annotations, standing in for the adapter's
/// fresh render.
fn write_bare_pdf(path: &std::path::Path) {
    use lopdf::{dictionary, Document, Stream};

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
            lopdf::Object::Integer(0),
            lopdf::Object::Integer(0),
            lopdf::Object::Integer(612),
            lopdf::Object::Integer(792),
        ],
        "Contents" => content_id,
    });
    doc.set_object(
        pages_id,
        dictionary! {
            "Type" => "Pages",
            "Kids" => vec![lopdf::Object::Reference(page_id)],
            "Count" => 1,
        },
    );
    let catalog_id = doc.add_object(dictionary! {
        "Type" => "Catalog",
        "Pages" => pages_id,
    });
    doc.trailer.set("Root", catalog_id);
    doc.save(path).expect("write bare PDF");
}

pub(crate) fn default_ping() -> &'static str {
    r#"{"protocol":1,"ok":true,"op":"ping","python":"3.12.3","playwright":"1.62.0","pillow":"12.3.0","nh3":"0.3.7","defuddle":"0.19.4","browser":{"kind":"chrome","path":"/usr/bin/google-chrome","version":"154.0.0"},"headed":"xvfb","fonts":["Source Serif 4"]}"#
}

pub(crate) fn default_capture() -> &'static str {
    r#"{"protocol":1,"ok":true,"op":"capture","kind":"article","final_url":"https://example.com/index/open-source-codex-orchestration-symphony/","title":"Symphony Spec","author":"Jane Doe","published":"2026-04-27","site":"Example","description":"A spec.","metadata_sources":{"title":"h1","author":"byline","published":"visible-date","site":"og:site_name"},"word_count":8921,"capture":{"browser":"chrome","browser_version":"154.0.0","mode":"headless","retried_after_challenge":false},"fidelity":{"status":"ok","page_large_media":2,"kept_large_media":2,"page_code_blocks":1,"kept_code_blocks":1,"page_words":10858,"kept_words":8921},"images":{"total":2,"kept":2,"skipped_small":0,"failed":0},"pdf_bytes":2400,"warnings":[]}"#
}

pub(crate) fn failure_response(
    kind: &str,
    message: &str,
    hint: &str,
) -> String {
    format!(
        r#"{{"protocol":1,"ok":false,"op":"capture","error":{{"kind":"{kind}","message":"{message}","hint":"{hint}"}}}}"#
    )
}
