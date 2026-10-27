//! Shared fake `BOB_WEB_CLIP_ADAPTER` for highlights clip/create tests.
//!
//! The fake adapter saves each request to `request.json` and `request-N.json`,
//! copies a tiny fixture PDF to the requested `out_pdf`, and prints a canned
//! response. Capture N reads `response-N.json` when present. Compose copies
//! the fixture PDF and prints a compose response. No test touches the
//! network or a real browser.

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
        let diagrams = FakeDiagrams::new(temp, &format!("{name}-diagrams"));
        let path = root.join("fake-clip-adapter.sh");
        let script = format!(
            r#"#!/bin/sh
# Fake web-clip adapter: one JSON request on stdin, one JSON response on
# stdout. Saves stdin to request.json and request-N.json, copies the
# fixture PDF to out_pdf, then prints the canned response file.
ROOT={root}
request=$(cat)
n=1
if [ -f "$ROOT/request-count" ]; then
  n=$(cat "$ROOT/request-count")
  n=$((n + 1))
fi
printf '%s\n' "$n" > "$ROOT/request-count"
printf '%s' "$request" > "$ROOT/request.json"
printf '%s' "$request" > "$ROOT/request-$n.json"
case "$request" in
  *'"op":"ping"'*)
    if [ -f "$ROOT/ping.json" ]; then cat "$ROOT/ping.json"; else printf '%s' '{PING}'; fi
    ;;
  *'"op":"diagrams"'*)
    printf '%s' "$request" | {diagrams}
    ;;
  *'"op":"compose"'*)
    out_pdf=$(printf '%s' "$request" | sed -n 's/.*"out_pdf":"\([^"]*\)".*/\1/p')
    if [ -n "$out_pdf" ] && [ -f "$ROOT/fixture.pdf" ]; then
      mkdir -p "$(dirname "$out_pdf")"
      cp "$ROOT/fixture.pdf" "$out_pdf"
    fi
    if [ -f "$ROOT/compose.json" ]; then cat "$ROOT/compose.json"; else printf '%s' '{COMPOSE}'; fi
    ;;
  *)
    out_pdf=$(printf '%s' "$request" | sed -n 's/.*"out_pdf":"\([^"]*\)".*/\1/p')
    if [ -n "$out_pdf" ] && [ -f "$ROOT/fixture.pdf" ]; then
      mkdir -p "$(dirname "$out_pdf")"
      cp "$ROOT/fixture.pdf" "$out_pdf"
    fi
    html_path=$(printf '%s' "$request" | sed -n 's/.*"html_path":"\([^"]*\)".*/\1/p')
    if [ -n "$html_path" ] && [ -f "$html_path" ]; then cp "$html_path" "$ROOT/staged.html"; fi
    workdir=$(printf '%s' "$request" | sed -n 's/.*"workdir":"\([^"]*\)".*/\1/p')
    if [ -n "$workdir" ]; then
      mkdir -p "$workdir"
      printf '%s' '{{}}' > "$workdir/article.json"
    fi
    if [ -f "$ROOT/response-$n.json" ]; then cat "$ROOT/response-$n.json"; else cat "$ROOT/response.json"; fi
    ;;
esac
"#,
            root = shell_single_quote(path_str(&root)),
            diagrams = shell_single_quote(path_str(&diagrams.path)),
            PING = default_ping(),
            COMPOSE = default_compose(),
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

    pub(crate) fn respond_n(&self, n: usize, body: &str) {
        fs::write(self.root.join(format!("response-{n}.json")), body)
            .expect("write numbered fake capture response");
    }

    pub(crate) fn request(&self) -> String {
        fs::read_to_string(self.root.join("request.json"))
            .expect("read saved adapter request")
    }

    pub(crate) fn request_n(&self, n: usize) -> String {
        fs::read_to_string(self.root.join(format!("request-{n}.json")))
            .unwrap_or_else(|error| {
                panic!("read saved adapter request {n}: {error}")
            })
    }

    pub(crate) fn request_count(&self) -> usize {
        fs::read_to_string(self.root.join("request-count"))
            .ok()
            .and_then(|value| value.trim().parse().ok())
            .unwrap_or(0)
    }

    pub(crate) fn compose_called(&self) -> bool {
        (1..=self.request_count())
            .any(|n| self.request_n(n).contains(r#""op":"compose""#))
    }

    pub(crate) fn called(&self) -> bool {
        self.root.join("request.json").exists()
    }
}

/// Stub `BOB_WEB_CLIP_ADAPTER` for Mermaid `diagrams` requests.
///
/// Writes one-page PDFs for requested ids (XeLaTeX-includable, unlike
/// the capture fixture), records stdin to `request.json`, and honors
/// `BOB_FAKE_DIAGRAMS_MODE` (`success`, `partial`, `fail-all`, `crash`,
/// `invalid`, `sleep`, `unavailable`).
pub(crate) struct FakeDiagrams {
    pub(crate) root: std::path::PathBuf,
    pub(crate) path: std::path::PathBuf,
}

impl FakeDiagrams {
    pub(crate) fn new(temp: &TempDir, name: &str) -> Self {
        let root = temp.path().join(name);
        fs::create_dir_all(&root).expect("create fake diagrams dir");
        fs::write(
            root.join("fixture.pdf"),
            include_bytes!(
                "../../../src/native/highlights_ref/diagrams_blank.pdf"
            ),
        )
        .expect("write diagrams fixture pdf");
        let path = root.join("fake-diagrams-adapter.py");
        let script = format!(
            r#"#!/usr/bin/env python3
import hashlib, json, os, shutil, sys, time
from pathlib import Path
ROOT = Path({root})
request = json.load(sys.stdin)
(ROOT / "request.json").write_text(json.dumps(request))
mode = os.environ.get("BOB_FAKE_DIAGRAMS_MODE", "success")
if mode == "unavailable":
    json.dump({{"protocol": 1, "ok": False, "op": "diagrams", "error": {{
        "kind": "browser", "message": "browser launch failed: missing executable\n╔══ playwright install ══╗"
    }}}}, sys.stdout)
    raise SystemExit(0)
if mode == "crash":
    sys.stderr.write("boom\n")
    raise SystemExit(3)
if mode == "invalid":
    sys.stdout.write("not-json")
    raise SystemExit(0)
if mode == "sleep":
    time.sleep(float(os.environ.get("BOB_FAKE_DIAGRAMS_SLEEP_SECS", "30")))
workdir = request["workdir"]
os.makedirs(workdir, exist_ok=True)
fixture = ROOT / "fixture.pdf"
width = float(os.environ.get("BOB_FAKE_DIAGRAMS_WIDTH_PT", "200"))
height = float(os.environ.get("BOB_FAKE_DIAGRAMS_HEIGHT_PT", "100"))
results = []
for item in request.get("items") or []:
    source = item.get("source") or ""
    assert item["id"] == hashlib.sha1(source.encode()).hexdigest()
    fail = mode in ("fail-all",) or mode == "partial" and "not valid" in source
    if fail:
        results.append({{
            "id": item["id"],
            "ok": False,
            "error": "Parse error on line 1: bad",
        }})
        continue
    pdf = os.path.join(workdir, item["id"] + ".pdf")
    shutil.copy(fixture, pdf)
    results.append({{
        "id": item["id"],
        "ok": True,
        "pdf": pdf,
        "width_pt": width,
        "height_pt": height,
    }})
json.dump({{
    "protocol": 1,
    "ok": True,
    "op": "diagrams",
    "mermaid": "12.2.0",
    "browser": {{"kind": "chrome", "path": "/fake", "version": "1"}},
    "results": results,
    "warnings": [],
}}, sys.stdout)
"#,
            root = json_string(&path_str(&root)),
        );
        write_executable(&path, &script);
        Self { root, path }
    }

    pub(crate) fn called(&self) -> bool {
        self.root.join("request.json").exists()
    }

    pub(crate) fn request(&self) -> String {
        fs::read_to_string(self.root.join("request.json"))
            .expect("read saved diagrams request")
    }
}

fn json_string(value: &str) -> String {
    serde_json::to_string(value).expect("quote python path")
}

/// Fixture PDF for web-source tests: one page whose catalog `/Dests`
/// names the first printed code block Chrome-style (explicit destination
/// arrays, plus a `/D` dictionary for the end anchor), with one URI link
/// annotation. Mirrors the adapter's print output for a source view with
/// one fenced block and one hyperlink.
pub(crate) fn web_fixture_pdf(link_uri: &str) -> Vec<u8> {
    use lopdf::{dictionary, Document, Object, Stream};

    let mut document = Document::with_version("1.4");
    let pages_id = document.new_object_id();
    let content_id =
        document.add_object(Stream::new(dictionary! {}, Vec::new()));
    let annotation_id = document.add_object(dictionary! {
        "Type" => "Annot",
        "Subtype" => "Link",
        "Rect" => vec![
            Object::Integer(72),
            Object::Integer(700),
            Object::Integer(200),
            Object::Integer(712),
        ],
        "A" => dictionary! {
            "S" => "URI",
            "URI" => Object::string_literal(link_uri),
        },
    });
    let page_id = document.add_object(dictionary! {
        "Type" => "Page",
        "Parent" => pages_id,
        "MediaBox" => vec![
            Object::Integer(0),
            Object::Integer(0),
            Object::Integer(612),
            Object::Integer(792),
        ],
        "Contents" => content_id,
        "Annots" => vec![Object::Reference(annotation_id)],
    });
    document.set_object(
        pages_id,
        dictionary! {
            "Type" => "Pages",
            "Kids" => vec![Object::Reference(page_id)],
            "Count" => 1,
        },
    );
    let xyz = || {
        vec![
            Object::Reference(page_id),
            Object::Name(b"XYZ".to_vec()),
            Object::Null,
            Object::Null,
            Object::Null,
        ]
    };
    let catalog_id = document.add_object(dictionary! {
        "Type" => "Catalog",
        "Pages" => pages_id,
        "Dests" => dictionary! {
            "bob-code-1" => Object::Array(xyz()),
            "bob-code-1-end" => dictionary! {
                "D" => Object::Array(xyz()),
            },
        },
    });
    document.trailer.set("Root", catalog_id);
    let mut bytes = Vec::new();
    document
        .save_to(&mut bytes)
        .expect("serialize web fixture PDF");
    bytes
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
    r#"{"protocol":1,"ok":true,"op":"ping","python":"3.12.3","playwright":"1.62.0","pillow":"12.3.0","nh3":"0.3.7","defuddle":"0.19.4","mermaid":"12.2.0","browser":{"kind":"chrome","path":"/usr/bin/google-chrome","version":"154.0.0"},"headed":"xvfb","fonts":["Source Serif 4"]}"#
}

pub(crate) fn default_capture() -> &'static str {
    r#"{"protocol":1,"ok":true,"op":"capture","kind":"article","final_url":"https://example.com/index/open-source-codex-orchestration-symphony/","title":"Symphony Spec","author":"Jane Doe","published":"2026-04-27","site":"Example","description":"A spec.","metadata_sources":{"title":"h1","author":"byline","published":"visible-date","site":"og:site_name"},"word_count":8921,"capture":{"browser":"chrome","browser_version":"154.0.0","mode":"headless","retried_after_challenge":false},"fidelity":{"status":"ok","page_large_media":2,"kept_large_media":2,"page_code_blocks":1,"kept_code_blocks":1,"page_words":10858,"kept_words":8921},"images":{"total":2,"kept":2,"skipped_small":0,"failed":0},"pdf_bytes":2400,"warnings":[]}"#
}

pub(crate) fn default_compose() -> &'static str {
    r#"{"protocol":1,"ok":true,"op":"compose","pdf_bytes":2400,"images":{"total":4,"kept":4,"skipped_small":0,"failed":0},"warnings":[]}"#
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
