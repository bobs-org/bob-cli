//! Highlights clip tests with a fake `BOB_WEB_CLIP_ADAPTER` (see
//! `fake_clip.rs` for the shared fake).

use super::fake_clip::*;
use crate::support::*;
use std::fs;

const CLIP_URL: &str =
    "https://example.com/index/open-source-codex-orchestration-symphony/";
const CLIP_STEM: &str = "open_source_codex_orchestration_symphony";

#[test]
fn highlights_clip_success_writes_stamped_intake_pdf() {
    let temp = TempDir::new("bob-cli-highlights-clip-success");
    let vault = temp.path().join("vault");
    let fake = FakeClip::new(&temp, "fake");

    let output = bob_command()
        .arg("highlights")
        .arg("clip")
        .arg(CLIP_URL)
        .arg("-b")
        .arg(&vault)
        .env("BOB_WEB_CLIP_ADAPTER", &fake.path)
        .output()
        .expect("run bob highlights clip");

    assert_success(&output);
    let report = stdout(&output);
    let pdf = vault.join(format!("xlib/blogs/{CLIP_STEM}.pdf"));
    assert!(
        report.contains("created Highlights-ready web PDF")
            && report.contains(path_str(&pdf))
            && report.contains("title: Symphony Spec")
            && report.contains("author: Jane Doe")
            && report.contains("published: 2026-04-27")
            && report.contains("captured: ")
            && report.contains("source_url: https://example.com/index/open-source-codex-orchestration-symphony/")
            && report.contains("status: ready")
            && report.contains("parent: obsidian_ref")
            && report.contains(&format!("id: {CLIP_STEM}"))
            && report.contains("capture: chrome 154.0.0 · headless")
            && report.contains("images: 2/2")
            && report.contains("fidelity: ok")
            && report.contains("next: bob ref scan"),
        "{report}"
    );
    assert!(pdf.is_file(), "clip must install the intake PDF");

    let marker = bob_command()
        .arg("highlights")
        .arg("marker")
        .arg(&pdf)
        .arg("-b")
        .arg(&vault)
        .output()
        .expect("run bob highlights marker");
    assert_success(&marker);
    let marker_report = stdout(&marker);
    assert!(
        marker_report.contains("source_url: https://example.com/index/open-source-codex-orchestration-symphony/")
            && marker_report.contains("author: Jane Doe")
            && marker_report.contains("published: 2026-04-27")
            && marker_report.contains("captured: ")
            && marker_report.contains(&format!("id: {CLIP_STEM}")),
        "{marker_report}"
    );

    let document = lopdf::Document::load(&pdf).expect("load clipped PDF");
    let info = match document.trailer.get(b"Info") {
        Ok(lopdf::Object::Reference(id)) => {
            document.get_dictionary(*id).expect("read Info dict")
        }
        _ => panic!("expected an Info dictionary"),
    };
    let title = info.get(b"Title").expect("Info Title");
    assert!(
        lopdf::decode_text_string(title)
            .expect("decode Info Title")
            .contains("Symphony Spec"),
        "Info Title must carry the article title"
    );
}

#[test]
fn highlights_clip_supports_ref_type_output_and_name() {
    let temp = TempDir::new("bob-cli-highlights-clip-targets");
    let vault = temp.path().join("vault");
    let fake = FakeClip::new(&temp, "fake");

    let output = bob_command()
        .arg("highlights")
        .arg("clip")
        .arg(CLIP_URL)
        .arg("-b")
        .arg(&vault)
        .arg("-t")
        .arg("papers")
        .env("BOB_WEB_CLIP_ADAPTER", &fake.path)
        .output()
        .expect("run bob highlights clip -t");
    assert_success(&output);
    assert!(
        vault.join(format!("xlib/papers/{CLIP_STEM}.pdf")).is_file(),
        "-t papers must select the intake subdirectory"
    );

    let named = temp.path().join("vault-named");
    let output = bob_command()
        .arg("highlights")
        .arg("clip")
        .arg(CLIP_URL)
        .arg("-b")
        .arg(&named)
        .arg("-N")
        .arg("custom-stem.pdf")
        .env("BOB_WEB_CLIP_ADAPTER", &fake.path)
        .output()
        .expect("run bob highlights clip -N");
    assert_success(&output);
    assert!(
        stdout(&output).contains("id: custom-stem"),
        "-N must strip .pdf and become the marker id:\n{}",
        format_output(&output)
    );
    assert!(named.join("xlib/blogs/custom-stem.pdf").is_file());

    let external_vault = temp.path().join("vault-external");
    let external = temp.path().join("outside.pdf");
    let output = bob_command()
        .arg("highlights")
        .arg("clip")
        .arg(CLIP_URL)
        .arg("-b")
        .arg(&external_vault)
        .arg("-o")
        .arg(&external)
        .env("BOB_WEB_CLIP_ADAPTER", &fake.path)
        .output()
        .expect("run bob highlights clip -o");
    assert_success(&output);
    assert!(external.is_file(), "-o must write the exact path");
    assert!(
        stdout(&output).contains("next: bob ref sync"),
        "-o outside the vault must point at sync:\n{}",
        format_output(&output)
    );
}

#[test]
fn highlights_clip_forwards_overrides_and_html() {
    let temp = TempDir::new("bob-cli-highlights-clip-overrides");
    let vault = temp.path().join("vault");
    let fake = FakeClip::new(&temp, "fake");
    let html = temp.path().join("saved.html");
    write_file(&html, "<html><body><h1>Saved</h1></body></html>");

    let output = bob_command()
        .arg("highlights")
        .arg("clip")
        .arg(CLIP_URL)
        .arg("-b")
        .arg(&vault)
        .arg("-A")
        .arg("Custom Author")
        .arg("-T")
        .arg("Custom Title")
        .arg("-p")
        .arg("2026-05-01")
        .arg("-H")
        .arg(&html)
        .env("BOB_WEB_CLIP_ADAPTER", &fake.path)
        .output()
        .expect("run bob highlights clip with overrides");
    assert_success(&output);
    let request = fake.request();
    assert!(
        request.contains(r#""title":"Custom Title""#)
            && request.contains(r#""author":"Custom Author""#)
            && request.contains(r#""published":"2026-05-01""#)
            && request.contains(&format!(
                r#""html_path":{}"#,
                serde_json::to_string(path_str(&html)).expect("quote path")
            )),
        "overrides and --html must reach the adapter:\n{request}"
    );
    let report = stdout(&output);
    assert!(
        report.contains("title: Custom Title")
            && report.contains("author: Custom Author")
            && report.contains("published: 2026-05-01")
            && report.contains("capture: "),
        "{report}"
    );

    let output = bob_command()
        .arg("highlights")
        .arg("clip")
        .arg(CLIP_URL)
        .arg("-b")
        .arg(temp.path().join("vault-override-dry"))
        .arg("-A")
        .arg("Custom Author")
        .arg("-T")
        .arg("Custom Title")
        .arg("-p")
        .arg("2026-05-01")
        .arg("-d")
        .env("BOB_WEB_CLIP_ADAPTER", &fake.path)
        .output()
        .expect("run bob highlights clip overrides --dry-run");
    assert_success(&output);
    let report = stdout(&output);
    assert!(
        report.contains("title: Custom Title (override)")
            && report.contains("author: Custom Author (override)")
            && report.contains("published: 2026-05-01 (override)"),
        "{report}"
    );

    let piped = temp.path().join("vault-piped");
    let mut command = bob_command();
    command
        .arg("highlights")
        .arg("clip")
        .arg(CLIP_URL)
        .arg("-b")
        .arg(&piped)
        .arg("--html")
        .arg("-")
        .env("BOB_WEB_CLIP_ADAPTER", &fake.path);
    let output =
        run_with_stdin(&mut command, "<html><body>stdin page</body></html>");
    assert_success(&output);
    let request = fake.request();
    assert!(
        request.contains("input.html"),
        "--html - must stage stdin in the workdir:\n{request}"
    );
    assert!(
        fs::read_to_string(fake.root.join("staged.html"))
            .expect("read staged stdin")
            .contains("stdin page"),
        "staged stdin must hold the piped HTML"
    );
}

#[test]
fn highlights_clip_dry_run_writes_nothing() {
    let temp = TempDir::new("bob-cli-highlights-clip-dry-run");
    let vault = temp.path().join("vault");
    let fake = FakeClip::new(&temp, "fake");

    let output = bob_command()
        .arg("highlights")
        .arg("clip")
        .arg(CLIP_URL)
        .arg("-b")
        .arg(&vault)
        .arg("-d")
        .env("BOB_WEB_CLIP_ADAPTER", &fake.path)
        .output()
        .expect("run bob highlights clip --dry-run");

    assert_success(&output);
    let report = stdout(&output);
    assert!(
        report.contains("would create Highlights-ready web PDF")
            && report.contains("source_url: https://example.com/index/open-source-codex-orchestration-symphony/")
            && report.contains(&format!("xlib/blogs/{CLIP_STEM}.pdf"))
            && report.contains("sidecar_guard:")
            && report.contains("library_destination:")
            && report.contains("published: 2026-04-27 (visible-date)")
            && report.contains("fidelity: ok")
            && report.contains("marker:")
            && report.contains("writes: none"),
        "{report}"
    );
    let request = fake.request();
    assert!(
        request.contains(r#""dry_run":true"#),
        "dry-run must reach the adapter:\n{request}"
    );
    assert!(!vault.exists(), "dry-run must not create the vault");
}

#[test]
fn highlights_clip_reports_adapter_failures_with_hints() {
    let temp = TempDir::new("bob-cli-highlights-clip-failures");
    let fake = FakeClip::new(&temp, "fake");

    for (kind, message, hint) in [
        (
            "blocked",
            "the bot challenge did not clear",
            "save the page from your browser and pass --html FILE",
        ),
        (
            "thin",
            "extraction too small",
            "retry with --html, or the page is not an article",
        ),
    ] {
        let vault = temp.path().join(format!("vault-{kind}"));
        fake.respond(&failure_response(kind, message, hint));
        let output = bob_command()
            .arg("highlights")
            .arg("clip")
            .arg(CLIP_URL)
            .arg("-b")
            .arg(&vault)
            .env("BOB_WEB_CLIP_ADAPTER", &fake.path)
            .output()
            .expect("run failing bob highlights clip");

        assert_eq!(
            output.status.code(),
            Some(1),
            "{kind} must exit 1:\n{}",
            format_output(&output)
        );
        let diagnostic = stderr(&output);
        assert!(
            diagnostic.contains("bob ref: error:")
                && diagnostic.contains(message)
                && diagnostic.contains(&format!("hint: {hint}")),
            "{kind} must print error and hint:\n{}",
            format_output(&output)
        );
        assert!(!vault.exists(), "{kind} must write nothing");
    }
}

#[test]
fn highlights_clip_rejects_before_the_adapter_runs() {
    let temp = TempDir::new("bob-cli-highlights-clip-prefail");
    let fake = FakeClip::new(&temp, "fake");

    for raw in [
        "ftp://example.com/article",
        "https://user@example.com/article",
        "http://localhost/article",
        "http://10.0.0.1/article",
        "http://100.101.1.2/article",
        "http://[::1]/article",
        "http://foo.local/article",
    ] {
        let vault = temp.path().join("vault-bad-url");
        let output = bob_command()
            .arg("highlights")
            .arg("clip")
            .arg(raw)
            .arg("-b")
            .arg(&vault)
            .env("BOB_WEB_CLIP_ADAPTER", &fake.path)
            .output()
            .expect("run bob highlights clip with bad URL");
        assert_eq!(
            output.status.code(),
            Some(1),
            "{raw} must exit 1:\n{}",
            format_output(&output)
        );
    }
    assert!(!fake.called(), "URL validation must precede the adapter");

    for args in [
        vec!["-p", "2026-13-40"],
        vec!["-N", "has space"],
        vec!["-t", "nested/dir"],
    ] {
        let vault = temp.path().join("vault-bad-option");
        let output = bob_command()
            .arg("highlights")
            .arg("clip")
            .arg(CLIP_URL)
            .arg("-b")
            .arg(&vault)
            .args(&args)
            .env("BOB_WEB_CLIP_ADAPTER", &fake.path)
            .output()
            .expect("run bob highlights clip with bad option");
        assert_eq!(
            output.status.code(),
            Some(1),
            "{args:?} must exit 1:\n{}",
            format_output(&output)
        );
    }
    assert!(!fake.called(), "option validation must precede the adapter");

    for args in [
        vec!["-o", "x.pdf", "-t", "papers"],
        vec!["-o", "x.pdf", "-N", "stem"],
    ] {
        let vault = temp.path().join("vault-conflict");
        let output = bob_command()
            .arg("highlights")
            .arg("clip")
            .arg(CLIP_URL)
            .arg("-b")
            .arg(&vault)
            .args(&args)
            .env("BOB_WEB_CLIP_ADAPTER", &fake.path)
            .output()
            .expect("run bob highlights clip with conflicts");
        assert_eq!(
            output.status.code(),
            Some(2),
            "{args:?} must be a clap conflict:\n{}",
            format_output(&output)
        );
    }
    assert!(!fake.called(), "clap conflicts must precede the adapter");
}

#[test]
fn highlights_clip_refuses_library_and_dedupe_collisions_early() {
    let temp = TempDir::new("bob-cli-highlights-clip-collisions");
    let vault = temp.path().join("vault");
    let fake = FakeClip::new(&temp, "fake");

    write_highlights_pdf(
        &vault.join(format!("lib/blogs/{CLIP_STEM}.pdf")),
        "- status: ready\n- parent: obsidian\n- title: Archived\n",
    );
    for force in [false, true] {
        let mut command = bob_command();
        command
            .arg("highlights")
            .arg("clip")
            .arg(CLIP_URL)
            .arg("-b")
            .arg(&vault);
        if force {
            command.arg("--force");
        }
        let output = command
            .env("BOB_WEB_CLIP_ADAPTER", &fake.path)
            .output()
            .expect("run bob highlights clip over library PDF");
        assert_eq!(
            output.status.code(),
            Some(1),
            "force={force} must refuse the library destination:\n{}",
            format_output(&output)
        );
    }
    assert!(
        !fake.called(),
        "the library collision must precede the adapter"
    );

    // A PDF-backed note refuses even with --force. A note without a
    // `source_pdf` instead warns and captures (see
    // `highlights_clip_captures_a_legacy_only_url_note_with_a_warning`).
    let dedupe_vault = temp.path().join("dedupe-vault");
    write_file(
        &dedupe_vault.join("ref/blogs/existing.md"),
        "---\nsource_url: https://example.com/index/open-source-codex-orchestration-symphony/\nsource_pdf: lib/blogs/existing.pdf\ntitle: Existing\n---\n\n# Existing\n",
    );
    let output = bob_command()
        .arg("highlights")
        .arg("clip")
        .arg(CLIP_URL)
        .arg("-b")
        .arg(&dedupe_vault)
        .arg("--force")
        .env("BOB_WEB_CLIP_ADAPTER", &fake.path)
        .output()
        .expect("run bob highlights clip over captured URL");
    assert_eq!(
        output.status.code(),
        Some(1),
        "a captured URL must refuse even with --force:\n{}",
        format_output(&output)
    );
    assert!(
        stderr(&output).contains("already captured as"),
        "expected the dedupe error:\n{}",
        format_output(&output)
    );
    assert!(
        !fake.called(),
        "the ref-note dedupe must precede the adapter"
    );
}

#[test]
fn highlights_clip_captures_a_legacy_only_url_note_with_a_warning() {
    let temp = TempDir::new("bob-cli-highlights-clip-legacy-url");
    let vault = temp.path().join("vault");
    let fake = FakeClip::new(&temp, "fake");

    // A ref note carrying only the legacy `url:` key and no `source_pdf`
    // (as the zorg-migrated notes do) no longer refuses: the URL
    // captures with a warning, and scan later treats the old note as
    // superseded.
    write_file(
        &vault.join("ref/ai/old.md"),
        "---\ntitle: Old\nurl: https://example.com/index/open-source-codex-orchestration-symphony/\n---\n\n# Old\n",
    );
    let output = bob_command()
        .arg("highlights")
        .arg("clip")
        .arg(CLIP_URL)
        .arg("-b")
        .arg(&vault)
        .env("BOB_WEB_CLIP_ADAPTER", &fake.path)
        .output()
        .expect("run bob highlights clip over legacy-only note");
    assert_success(&output);
    let diagnostic = stderr(&output);
    assert!(
        diagnostic.contains("already in the library as")
            && diagnostic.contains("ref/ai/old.md")
            && diagnostic.contains("capturing a fresh copy"),
        "expected the legacy warning:\n{}",
        format_output(&output)
    );
    assert_eq!(
        diagnostic
            .matches("hint: bob ref find and bob ref list")
            .count(),
        1,
        "the legacy hint prints once per capture:\n{diagnostic}",
    );
    assert!(
        fake.called(),
        "the legacy-only hit must not precede the adapter"
    );
    assert!(
        vault.join(format!("xlib/blogs/{CLIP_STEM}.pdf")).is_file(),
        "the fresh copy must install:\n{}",
        format_output(&output)
    );

    // A dry run reports the legacy hit without writing.
    let dry_vault = temp.path().join("dry-vault");
    write_file(
        &dry_vault.join("ref/ai/old.md"),
        "---\ntitle: Old\nurl: https://example.com/index/open-source-codex-orchestration-symphony/\n---\n\n# Old\n",
    );
    let output = bob_command()
        .arg("highlights")
        .arg("clip")
        .arg(CLIP_URL)
        .arg("-b")
        .arg(&dry_vault)
        .arg("--dry-run")
        .env("BOB_WEB_CLIP_ADAPTER", &fake.path)
        .output()
        .expect("run bob highlights clip --dry-run over legacy-only note");
    assert_success(&output);
    let report = stdout(&output);
    assert!(
        report.contains("legacy:")
            && report.contains("ref/ai/old.md")
            && report.contains("(superseded by this capture)"),
        "expected the dry-run legacy line:\n{}",
        format_output(&output)
    );
    assert!(
        stderr(&output).contains("already in the library as"),
        "the dry run still warns on stderr:\n{}",
        format_output(&output)
    );
}

#[test]
fn highlights_clip_still_refuses_a_pdf_backed_url_note() {
    let temp = TempDir::new("bob-cli-highlights-clip-pdf-backed-url");
    let vault = temp.path().join("vault");
    let fake = FakeClip::new(&temp, "fake");

    // A `url:` on a PDF-backed note is still an already-captured URL.
    write_file(
        &vault.join("ref/papers/ea_graph.md"),
        "---\ntitle: EA-Graph\nurl: https://arxiv.org/pdf/2608.04278\nsource_pdf: lib/papers/ea_graph.pdf\n---\n\n# EA-Graph\n",
    );
    let output = bob_command()
        .arg("highlights")
        .arg("clip")
        .arg("https://arxiv.org/abs/2608.04278v2")
        .arg("-b")
        .arg(&vault)
        .env("BOB_WEB_CLIP_ADAPTER", &fake.path)
        .output()
        .expect("run bob highlights clip over PDF-backed url note");
    assert_eq!(
        output.status.code(),
        Some(1),
        "a PDF-backed url: note must refuse the same paper:\n{}",
        format_output(&output)
    );
    assert!(
        stderr(&output).contains("already captured as"),
        "expected the dedupe error:\n{}",
        format_output(&output)
    );
    assert!(
        !fake.called(),
        "the PDF-backed dedupe must precede the adapter"
    );
}

#[test]
fn highlights_clip_force_overwrites_the_same_intake_target() {
    let temp = TempDir::new("bob-cli-highlights-clip-force");
    let vault = temp.path().join("vault");
    let fake = FakeClip::new(&temp, "fake");
    let target = vault.join(format!("xlib/blogs/{CLIP_STEM}.pdf"));
    write_highlights_pdf(
        &target,
        "- status: ready\n- parent: obsidian\n- title: Queued\n- source_url: https://example.com/index/open-source-codex-orchestration-symphony/\n",
    );

    let output = bob_command()
        .arg("highlights")
        .arg("clip")
        .arg(CLIP_URL)
        .arg("-b")
        .arg(&vault)
        .env("BOB_WEB_CLIP_ADAPTER", &fake.path)
        .output()
        .expect("run bob highlights clip over queued PDF");
    assert_eq!(
        output.status.code(),
        Some(1),
        "same-URL queue hit must refuse without --force:\n{}",
        format_output(&output)
    );
    assert!(
        stderr(&output).contains("already queued in"),
        "{}",
        format_output(&output)
    );

    let output = bob_command()
        .arg("highlights")
        .arg("clip")
        .arg(CLIP_URL)
        .arg("-b")
        .arg(&vault)
        .arg("--force")
        .env("BOB_WEB_CLIP_ADAPTER", &fake.path)
        .output()
        .expect("run bob highlights clip --force over queued PDF");
    assert_success(&output);
    assert!(
        stdout(&output).contains("created Highlights-ready web PDF"),
        "{}",
        format_output(&output)
    );
}

#[test]
fn highlights_clip_direct_pdf_falls_back_to_slug_title() {
    let temp = TempDir::new("bob-cli-highlights-clip-pdf-kind");
    let vault = temp.path().join("vault");
    let fake = FakeClip::new(&temp, "fake");
    fake.respond(
        r#"{"protocol":1,"ok":true,"op":"capture","kind":"pdf","final_url":"https://example.com/files/report.pdf","title":null,"author":null,"published":null,"metadata_sources":{},"word_count":0,"capture":{"browser":"chrome","browser_version":"154.0.0","mode":"direct-pdf","retried_after_challenge":false},"fidelity":{"status":"ok","page_large_media":0,"kept_large_media":0,"page_code_blocks":0,"kept_code_blocks":0,"page_words":0,"kept_words":0},"images":{"total":0,"kept":0,"skipped_small":0,"failed":0},"pdf_bytes":1200,"warnings":[]}"#,
    );

    let output = bob_command()
        .arg("highlights")
        .arg("clip")
        .arg("https://example.com/files/quarterly-report.pdf")
        .arg("-b")
        .arg(&vault)
        .env("BOB_WEB_CLIP_ADAPTER", &fake.path)
        .output()
        .expect("run bob highlights clip for a direct PDF");

    assert_success(&output);
    let report = stdout(&output);
    assert!(
        report.contains("title: quarterly report")
            && report.contains("capture: chrome 154.0.0 · direct PDF")
            && report.contains("id: quarterly_report"),
        "{report}"
    );
    assert!(vault.join("xlib/blogs/quarterly_report.pdf").is_file());
}

#[test]
fn highlights_clip_rejects_protocol_errors() {
    let temp = TempDir::new("bob-cli-highlights-clip-protocol");
    let vault = temp.path().join("vault");
    let fake = FakeClip::new(&temp, "fake");
    fake.respond("this is not json");

    let output = bob_command()
        .arg("highlights")
        .arg("clip")
        .arg(CLIP_URL)
        .arg("-b")
        .arg(&vault)
        .env("BOB_WEB_CLIP_ADAPTER", &fake.path)
        .output()
        .expect("run bob highlights clip with garbage adapter");

    assert_eq!(
        output.status.code(),
        Some(1),
        "non-JSON stdout must exit 1:\n{}",
        format_output(&output)
    );
    assert!(
        stderr(&output).contains("invalid JSON"),
        "expected the protocol error:\n{}",
        format_output(&output)
    );
    assert!(!vault.exists(), "protocol errors must write nothing");
}

#[test]
fn highlights_clip_round_trips_through_scan() {
    let temp = TempDir::new("bob-cli-highlights-clip-scan");
    let vault = temp.path().join("vault");
    let fake = FakeClip::new(&temp, "fake");

    let output = bob_command()
        .arg("highlights")
        .arg("clip")
        .arg(CLIP_URL)
        .arg("-b")
        .arg(&vault)
        .env("BOB_WEB_CLIP_ADAPTER", &fake.path)
        .output()
        .expect("run bob highlights clip for scan");
    assert_success(&output);

    let output = bob_command()
        .arg("highlights")
        .arg("--no-hooks")
        .arg("scan")
        .arg("-b")
        .arg(&vault)
        .output()
        .expect("run bob highlights scan on clipped PDF");
    assert_success(&output);

    let note = vault.join(format!("ref/blogs/{CLIP_STEM}.md"));
    assert!(note.is_file(), "scan must write the ref note");
    let contents = fs::read_to_string(&note).expect("read scanned ref note");
    assert!(
        contents.contains("https://example.com/index/open-source-codex-orchestration-symphony/")
            && contents.contains("Jane Doe")
            && contents.contains("2026-04-27")
            && contents.contains("captured:")
            && contents.contains(CLIP_STEM)
            && contents.contains("ref_type: blogs")
            && contents.contains("^ref"),
        "{contents}"
    );

    let output = bob_command()
        .arg("highlights")
        .arg("--no-hooks")
        .arg("scan")
        .arg("-b")
        .arg(&vault)
        .output()
        .expect("rerun bob highlights scan");
    assert_success(&output);
    assert_eq!(
        fs::read_to_string(&note).expect("reread scanned ref note"),
        contents,
        "a second scan must be a no-op"
    );
}

#[test]
fn highlights_doctor_reports_web_clip_rows() {
    let temp = TempDir::new("bob-cli-highlights-doctor-web-clip");
    let vault = temp.path().join("vault");
    fs::create_dir_all(vault.join("lib")).expect("create lib");
    fs::create_dir_all(vault.join("ref")).expect("create ref");
    fs::create_dir_all(vault.join("xlib")).expect("create xlib");
    git_in(&vault, ["init", "-q"]);
    configure_test_git_identity(&vault);
    let fake = FakeClip::new(&temp, "fake");
    fs::write(
        fake.root.join("ping.json"),
        r#"{"protocol":1,"ok":true,"op":"ping","playwright":"1.62.0","defuddle":"0.19.4","browser":{"kind":"chrome","path":"/usr/bin/google-chrome","version":"154.0.0"},"headed":"display"}"#,
    )
    .expect("write fake ping");

    let output = bob_command()
        .arg("highlights")
        .arg("doctor")
        .arg("--no-hooks")
        .env("BOB_DIR", &vault)
        .env("BOB_WEB_CLIP_ADAPTER", &fake.path)
        .output()
        .expect("run highlights doctor with fake adapter");

    assert_success(&output);
    let report = stdout(&output);
    assert!(
        report.contains(
            "web clip adapter: ok (playwright 1.62.0, defuddle 0.19.4)"
        ) && report.contains(
            "web clip browser: chrome 154.0.0 (/usr/bin/google-chrome)"
        ) && report.contains("web clip headed fallback: display")
            && report.contains("result: ok"),
        "{report}"
    );
}
