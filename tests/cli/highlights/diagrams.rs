//! Stub-adapter tests for Mermaid figures on both Markdown PDF routes.

use super::fake_clip::FakeDiagrams;
use crate::support::*;
use std::{
    io::Write,
    process::{Command, Stdio},
};

fn skip_pandoc_xelatex(test_name: &str) -> bool {
    let pandoc_available = Command::new("pandoc")
        .arg("--version")
        .output()
        .is_ok_and(|output| output.status.success());
    let xelatex_available = Command::new("xelatex")
        .arg("--version")
        .output()
        .is_ok_and(|output| output.status.success());
    if !pandoc_available || !xelatex_available {
        eprintln!("skipping {test_name}: pandoc/xelatex are required");
        return true;
    }
    false
}

/// Gate for tests that assert on rendered PDF text: they also need
/// pdftotext, while render-only tests keep running without it.
fn skip_pdf_text_render(test_name: &str) -> bool {
    if skip_pandoc_xelatex(test_name) {
        return true;
    }
    if Command::new("pdftotext").arg("-v").output().is_err() {
        eprintln!("skipping {test_name}: pdftotext is required");
        return true;
    }
    false
}

fn skip_if_missing_note_profile_packages(vault: &std::path::Path) -> bool {
    let output = bob_command()
        .arg("highlights")
        .arg("doctor")
        .arg("--no-hooks")
        .env("BOB_DIR", vault)
        .output();
    let Ok(output) = output else {
        eprintln!("skipping vault-note diagram test: doctor failed to run");
        return true;
    };
    let report = stdout(&output);
    let Some(row) = report
        .lines()
        .find(|line| line.starts_with("latex_packages: "))
    else {
        return false;
    };
    if let Some(missing) = row
        .strip_prefix("latex_packages: warn (missing ")
        .and_then(|row| row.strip_suffix(')'))
    {
        eprintln!(
            "skipping vault-note diagram test: missing TeX packages: {missing}"
        );
        return true;
    }
    false
}

fn pdf_haystack(path: &std::path::Path) -> String {
    // Callers gate on `skip_pdf_text_render`, which requires pdftotext.
    // There is no raw-bytes fallback: searching compressed PDF bytes for
    // visible text is not a valid extraction and lets negative assertions
    // pass falsely.
    let output = Command::new("pdftotext")
        .arg("-layout")
        .arg(path)
        .arg("-")
        .output()
        .expect("pdftotext is required for PDF text assertions");
    assert!(
        output.status.success(),
        "pdftotext failed on {}: {output:?}",
        path.display()
    );
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn mermaid_source(extra: &str) -> String {
    format!("# Diagrams\n\n```mermaid\nflowchart LR\n  A --> B\n```\n{extra}")
}

fn definition_list_mermaid_source() -> &'static str {
    "# Definition diagram\n\n\
     Term\n\
     : ```mermaid\n\
       flowchart LR\n\
         A --> B\n\
       ```\n"
}

fn seed_parent(vault: &std::path::Path) {
    write_file(&vault.join("sase.md"), "---\ntype: [[area]]\n---\n");
}

#[test]
fn highlights_create_renders_mermaid_via_stub_adapter() {
    if skip_pdf_text_render("mermaid success") {
        return;
    }
    let temp = TempDir::new("bob-cli-diagrams-success");
    let vault = temp.path().join("vault");
    seed_parent(&vault);
    let source = temp.path().join("diagrams.md");
    write_file(&source, &mermaid_source(""));
    let fake = FakeDiagrams::new(&temp, "fake-diagrams");

    let output = bob_command()
        .arg("highlights")
        .arg("create")
        .arg("-P")
        .arg("sase")
        .arg(&source)
        .arg("-b")
        .arg(&vault)
        .env("BOB_WEB_CLIP_ADAPTER", &fake.path)
        .output()
        .expect("create mermaid markdown");

    assert_success(&output);
    let report = stdout(&output);
    assert!(
        report.contains("diagrams: rendered 1 of 1 Mermaid diagrams"),
        "{report}"
    );
    assert!(fake.called(), "adapter should run for Mermaid");
    let request = fake.request();
    assert!(
        request.contains(r#""op":"diagrams""#)
            || request.contains(r#""op": "diagrams""#),
        "{request}"
    );
    let pdf = vault.join("xlib/chat/diagrams.pdf");
    let text = pdf_haystack(&pdf);
    assert!(!text.contains("flowchart LR"), "source gone: {text}");
    assert!(
        !text.contains("Mermaid diagram not rendered"),
        "no fallback note: {text}"
    );
}

#[test]
fn highlights_create_mermaid_partial_failure_keeps_note_and_exits_0() {
    if skip_pdf_text_render("mermaid partial") {
        return;
    }
    let temp = TempDir::new("bob-cli-diagrams-partial");
    let vault = temp.path().join("vault");
    seed_parent(&vault);
    let source = temp.path().join("diagrams.md");
    write_file(
        &source,
        "# Interleaved diagrams\n\n\
         ```mermaid\n\
         flowchart LR\n\
           A --> B\n\
         ```\n\
         \n\
         ```mermaid\n\
         this is not valid mermaid\n\
         ```\n\
         \n\
         ```mermaid\n\
         flowchart LR\n\
           A --> B\n\
         ```\n",
    );
    let fake = FakeDiagrams::new(&temp, "fake-diagrams");

    let output = bob_command()
        .arg("highlights")
        .arg("create")
        .arg("-P")
        .arg("sase")
        .arg(&source)
        .arg("-b")
        .arg(&vault)
        .env("BOB_WEB_CLIP_ADAPTER", &fake.path)
        .env("BOB_FAKE_DIAGRAMS_MODE", "partial")
        .output()
        .expect("create partial mermaid");

    assert_success(&output);
    let report = stdout(&output);
    assert!(
        report.contains("diagrams: rendered 2 of 3 Mermaid diagrams"),
        "{report}"
    );
    let diagnostic = stderr(&output);
    assert!(
        diagnostic.contains(
            "warning: Mermaid diagram 2 of 3 not rendered: Parse error on line 1: bad"
        ),
        "{diagnostic}"
    );
    assert_eq!(
        diagnostic.matches("Mermaid diagram").count(),
        1,
        "only the rejected unique source warns: {diagnostic}"
    );
    let request: serde_json::Value =
        serde_json::from_str(&fake.request()).expect("parse adapter request");
    assert_eq!(
        request["items"].as_array().map(Vec::len),
        Some(2),
        "A/B/A sends two unique diagrams: {request}"
    );
    let text = pdf_haystack(&vault.join("xlib/chat/diagrams.pdf"));
    assert!(
        text.contains("Mermaid diagram not rendered: Parse error on line 1"),
        "{text}"
    );
    assert!(
        text.contains("not valid mermaid"),
        "bad source kept: {text}"
    );
}

#[test]
fn highlights_create_mermaid_adapter_crash_invalid_and_timeout_degrade() {
    if skip_pdf_text_render("mermaid degrade") {
        return;
    }
    let source_body = mermaid_source("");
    for (name, mode, extra_env, needle) in [
        (
            "crash",
            "crash",
            None,
            "Mermaid diagrams printed as source: the web clip adapter crashed",
        ),
        (
            "invalid",
            "invalid",
            None,
            "Mermaid diagrams printed as source: the web clip adapter returned invalid JSON",
        ),
        (
            "timeout",
            "sleep",
            Some(("BOB_WEB_CLIP_TIMEOUT_SECS", "1")),
            "Mermaid diagrams printed as source: the web clip adapter timed out",
        ),
    ] {
        let temp = TempDir::new(&format!("bob-cli-diagrams-{name}"));
        let vault = temp.path().join("vault");
        seed_parent(&vault);
        let source = temp.path().join("diagrams.md");
        write_file(&source, &source_body);
        let fake = FakeDiagrams::new(&temp, "fake-diagrams");
        let mut command = bob_command();
        command
            .arg("highlights")
            .arg("create")
            .arg("-P")
            .arg("sase")
            .arg(&source)
            .arg("-b")
            .arg(&vault)
            .env("BOB_WEB_CLIP_ADAPTER", &fake.path)
            .env("BOB_FAKE_DIAGRAMS_MODE", mode);
        if let Some((key, value)) = extra_env {
            command.env(key, value);
        }
        let output = command
            .output()
            .unwrap_or_else(|error| panic!("create {name}: {error}"));
        assert_success(&output);
        let report = stdout(&output);
        assert!(
            report.contains("diagrams: rendered 0 of 1 Mermaid diagrams"),
            "{name}: {report}"
        );
        let diagnostic = stderr(&output);
        assert!(
            diagnostic.contains(needle),
            "{name}: {diagnostic}"
        );
        if name == "timeout" {
            assert!(
                diagnostic.contains("BOB_HIGHLIGHTS_DIAGRAMS=off"),
                "timeout hint: {diagnostic}"
            );
        }
        let text = pdf_haystack(&vault.join("xlib/chat/diagrams.pdf"));
        assert!(
            text.contains("Mermaid diagram not rendered:"),
            "{name} note: {text}"
        );
        assert!(
            text.contains("flowchart LR"),
            "{name} source kept: {text}"
        );
    }
}

#[test]
fn highlights_create_mermaid_missing_adapter_warns_and_exits_0() {
    if skip_pdf_text_render("mermaid missing adapter") {
        return;
    }
    let temp = TempDir::new("bob-cli-diagrams-missing-adapter");
    let vault = temp.path().join("vault");
    seed_parent(&vault);
    let source = temp.path().join("diagrams.md");
    write_file(&source, &mermaid_source(""));

    let output = bob_command()
        .arg("highlights")
        .arg("create")
        .arg("-P")
        .arg("sase")
        .arg(&source)
        .arg("-b")
        .arg(&vault)
        .env(
            "BOB_WEB_CLIP_ADAPTER",
            temp.path().join("missing-diagrams-adapter"),
        )
        .output()
        .expect("create without adapter");

    assert_success(&output);
    let diagnostic = stderr(&output);
    assert!(
        diagnostic.contains("Mermaid diagrams printed as source:"),
        "{diagnostic}"
    );
    let text = pdf_haystack(&vault.join("xlib/chat/diagrams.pdf"));
    assert!(text.contains("Mermaid diagram not rendered:"), "{text}");
    assert!(text.contains("flowchart LR"), "{text}");
}

#[test]
fn highlights_create_mermaid_kill_switch_skips_stub_and_prints_source() {
    if skip_pdf_text_render("mermaid kill switch") {
        return;
    }
    let temp = TempDir::new("bob-cli-diagrams-kill-switch");
    let vault = temp.path().join("vault");
    seed_parent(&vault);
    let source = temp.path().join("diagrams.md");
    write_file(&source, &mermaid_source(""));
    let fake = FakeDiagrams::new(&temp, "fake-diagrams");

    let output = bob_command()
        .arg("highlights")
        .arg("create")
        .arg("-P")
        .arg("sase")
        .arg(&source)
        .arg("-b")
        .arg(&vault)
        .env("BOB_WEB_CLIP_ADAPTER", &fake.path)
        .env("BOB_HIGHLIGHTS_DIAGRAMS", "off")
        .output()
        .expect("create with kill switch");

    assert_success(&output);
    let report = stdout(&output);
    assert!(
        !report.contains("diagrams: rendered"),
        "no summary when skipped: {report}"
    );
    assert!(!fake.called(), "kill switch must not spawn the adapter");
    let text = pdf_haystack(&vault.join("xlib/chat/diagrams.pdf"));
    assert!(text.contains("flowchart LR"), "source printed: {text}");
    assert!(
        !text.contains("Mermaid diagram not rendered"),
        "no note when skipped: {text}"
    );
}

#[test]
fn highlights_create_without_mermaid_does_not_spawn_stub() {
    if skip_pandoc_xelatex("no mermaid") {
        return;
    }
    let temp = TempDir::new("bob-cli-diagrams-none");
    let vault = temp.path().join("vault");
    seed_parent(&vault);
    let source = temp.path().join("plain.md");
    write_file(&source, "# Plain\n\nNo diagrams here.\n");
    let fake = FakeDiagrams::new(&temp, "fake-diagrams");

    let output = bob_command()
        .arg("highlights")
        .arg("create")
        .arg("-P")
        .arg("sase")
        .arg(&source)
        .arg("-b")
        .arg(&vault)
        .env("BOB_WEB_CLIP_ADAPTER", &fake.path)
        .output()
        .expect("create without mermaid");

    assert_success(&output);
    assert!(!fake.called(), "no Mermaid must not spawn the adapter");
    let report = stdout(&output);
    assert!(!report.contains("diagrams: rendered"), "{report}");
}

#[test]
fn highlights_create_snapshot_mermaid_success_and_failure_shape() {
    if skip_pdf_text_render("snapshot mermaid") {
        return;
    }
    let temp = TempDir::new("bob-cli-diagrams-snapshot");
    let vault = temp.path().join("vault");
    write_file(
        &vault.join("diagrams_area.md"),
        "---\ntype: [[area]]\n---\n# Diagrams Area\n\n```mermaid\nflowchart LR\n  A --> B\n```\n",
    );
    if skip_if_missing_note_profile_packages(&vault) {
        return;
    }
    let fake = FakeDiagrams::new(&temp, "fake-diagrams");

    let success = bob_command()
        .arg("highlights")
        .arg("create")
        .arg("diagrams_area")
        .arg("-b")
        .arg(&vault)
        .env("BOB_NOW", "2026-10-16 12:00:00")
        .env("BOB_WEB_CLIP_ADAPTER", &fake.path)
        .output()
        .expect("snapshot mermaid success");
    assert_success(&success);
    let report = stdout(&success);
    assert!(
        report.contains("diagrams: rendered 1 of 1 Mermaid diagrams"),
        "{report}"
    );
    let pdf = vault.join("xlib/notes/diagrams_area_261016.pdf");
    let text = pdf_haystack(&pdf);
    assert!(
        !text.contains("flowchart LR"),
        "snapshot source gone: {text}"
    );

    write_file(
        &vault.join("broken_area.md"),
        "---\ntype: [[area]]\n---\n# Broken Area\n\n```mermaid\nflowchart LR\n  A --> B\n```\n",
    );
    let fail_fake = FakeDiagrams::new(&temp, "fake-diagrams-fail");
    let failed = bob_command()
        .arg("highlights")
        .arg("create")
        .arg("broken_area")
        .arg("-b")
        .arg(&vault)
        .env("BOB_NOW", "2026-10-16 12:00:00")
        .env("BOB_WEB_CLIP_ADAPTER", &fail_fake.path)
        .env("BOB_FAKE_DIAGRAMS_MODE", "fail-all")
        .output()
        .expect("snapshot mermaid failure");
    assert_success(&failed);
    let fail_report = stdout(&failed);
    assert!(
        fail_report.contains("diagrams: rendered 0 of 1 Mermaid diagrams"),
        "{fail_report}"
    );
    let fail_text =
        pdf_haystack(&vault.join("xlib/notes/broken_area_261016.pdf"));
    assert!(
        fail_text.contains("Mermaid diagram not rendered:"),
        "{fail_text}"
    );
    assert!(
        fail_text.contains("flowchart LR"),
        "snapshot source kept: {fail_text}"
    );
}

#[test]
fn highlights_create_mermaid_scale_warning_when_figure_is_wide() {
    if skip_pandoc_xelatex("mermaid scale") {
        return;
    }
    let temp = TempDir::new("bob-cli-diagrams-scale");
    let vault = temp.path().join("vault");
    seed_parent(&vault);
    let source = temp.path().join("diagrams.md");
    write_file(
        &source,
        "# Interleaved diagrams\n\n\
         ```mermaid\n\
         flowchart LR\n\
           A --> B\n\
         ```\n\
         \n\
         ```mermaid\n\
         sequenceDiagram\n\
           Alice->>Bob: hi\n\
         ```\n\
         \n\
         ```mermaid\n\
         flowchart LR\n\
           A --> B\n\
         ```\n",
    );
    let fake = FakeDiagrams::new(&temp, "fake-diagrams");

    let output = bob_command()
        .arg("highlights")
        .arg("create")
        .arg("-P")
        .arg("sase")
        .arg(&source)
        .arg("-b")
        .arg(&vault)
        .env("BOB_WEB_CLIP_ADAPTER", &fake.path)
        .env("BOB_FAKE_DIAGRAMS_WIDTH_PT", "1200")
        .env("BOB_FAKE_DIAGRAMS_HEIGHT_PT", "100")
        .output()
        .expect("create wide mermaid");

    assert_success(&output);
    let diagnostic = stderr(&output);
    assert!(
        diagnostic.contains(
            "warning: Mermaid diagram 1 of 3 (flowchart LR) prints at"
        ) && diagnostic.contains(
            "warning: Mermaid diagram 2 of 3 (sequenceDiagram) prints at"
        ) && diagnostic.matches("prints at").count() == 2,
        "{diagnostic}"
    );
}

#[test]
fn highlights_create_recognizes_definition_list_mermaid_fences() {
    if skip_pdf_text_render("definition-list Mermaid") {
        return;
    }
    let markdown = definition_list_mermaid_source();

    let success_temp = TempDir::new("bob-cli-diagrams-definition-success");
    let success_vault = success_temp.path().join("vault");
    seed_parent(&success_vault);
    let success_source = success_temp.path().join("definition.md");
    write_file(&success_source, markdown);
    let success_fake = FakeDiagrams::new(&success_temp, "fake-diagrams");
    let success = bob_command()
        .arg("highlights")
        .arg("create")
        .arg("-P")
        .arg("sase")
        .arg(&success_source)
        .arg("-b")
        .arg(&success_vault)
        .env("BOB_WEB_CLIP_ADAPTER", &success_fake.path)
        .output()
        .expect("create definition-list Mermaid");
    assert_success(&success);
    let report = stdout(&success);
    assert!(
        report.contains("diagrams: rendered 1 of 1 Mermaid diagrams"),
        "{report}"
    );
    let request: serde_json::Value =
        serde_json::from_str(&success_fake.request())
            .expect("parse definition-list adapter request");
    assert_eq!(request["items"].as_array().map(Vec::len), Some(1));
    let rendered =
        pdf_haystack(&success_vault.join("xlib/chat/definition.pdf"));
    assert!(
        !rendered.contains("flowchart LR"),
        "source replaced: {rendered}"
    );

    let unavailable_temp =
        TempDir::new("bob-cli-diagrams-definition-unavailable");
    let unavailable_vault = unavailable_temp.path().join("vault");
    seed_parent(&unavailable_vault);
    let unavailable_source = unavailable_temp.path().join("definition.md");
    write_file(&unavailable_source, markdown);
    let missing_adapter = unavailable_temp.path().join("missing-adapter");
    let unavailable = bob_command()
        .arg("highlights")
        .arg("create")
        .arg("-P")
        .arg("sase")
        .arg(&unavailable_source)
        .arg("-b")
        .arg(&unavailable_vault)
        .env("BOB_WEB_CLIP_ADAPTER", &missing_adapter)
        .output()
        .expect("create definition-list Mermaid without adapter");
    assert_success(&unavailable);
    let diagnostic = stderr(&unavailable);
    assert!(
        diagnostic.contains("Mermaid diagrams printed as source:"),
        "{diagnostic}"
    );
    assert_eq!(
        diagnostic
            .lines()
            .filter(|line| line.contains("warning:"))
            .count(),
        1,
        "one unavailable-renderer warning: {diagnostic}"
    );
    let fallback =
        pdf_haystack(&unavailable_vault.join("xlib/chat/definition.pdf"));
    assert!(
        fallback.contains("Mermaid diagram not rendered:"),
        "{fallback}"
    );
    assert!(
        fallback.contains("flowchart LR"),
        "source retained: {fallback}"
    );
}

#[test]
fn highlights_create_definition_list_dry_run_stays_browser_free() {
    let temp = TempDir::new("bob-cli-diagrams-definition-dry-run");
    let vault = temp.path().join("vault");
    seed_parent(&vault);
    let source = temp.path().join("definition.md");
    write_file(&source, definition_list_mermaid_source());
    let fake = FakeDiagrams::new(&temp, "fake-diagrams");
    let missing_pandoc = temp.path().join("missing-pandoc");

    let output = bob_command()
        .arg("highlights")
        .arg("create")
        .arg("--dry-run")
        .arg("-P")
        .arg("sase")
        .arg(&source)
        .arg("-b")
        .arg(&vault)
        .env("BOB_PANDOC_COMMAND", &missing_pandoc)
        .env("BOB_WEB_CLIP_ADAPTER", &fake.path)
        .output()
        .expect("run definition-list Mermaid dry-run");

    assert_success(&output);
    let report = stdout(&output);
    assert!(
        report
            .contains("diagrams: 1 Mermaid fence(s) (rendered at create time)")
            && report.contains("writes: none"),
        "{report}"
    );
    assert!(!fake.called(), "dry-run must not start adapter or browser");
}

#[test]
fn highlights_create_mermaid_dry_run_counts_without_starting_tools() {
    let temp = TempDir::new("bob-cli-diagrams-dry-run");
    let vault = temp.path().join("vault");
    seed_parent(&vault);
    write_file(
        &vault.join("diagram_area.md"),
        "---\ntype: [[area]]\n---\n# Diagram Area\n\n```mermaid\nflowchart LR\n A --> B\n```\n",
    );
    let source = temp.path().join("diagram report.md");
    write_file(&source, &mermaid_source(""));
    let fake = FakeDiagrams::new(&temp, "fake-diagrams");
    let missing_pandoc = temp.path().join("missing-pandoc");

    let enabled = bob_command()
        .arg("highlights")
        .arg("create")
        .arg("--dry-run")
        .arg("-P")
        .arg("sase")
        .arg(&source)
        .arg("-b")
        .arg(&vault)
        .env("BOB_PANDOC_COMMAND", &missing_pandoc)
        .env("BOB_WEB_CLIP_ADAPTER", &fake.path)
        .output()
        .expect("run generic Mermaid dry-run");
    assert_success(&enabled);
    let report = stdout(&enabled);
    assert!(
        report
            .contains("diagrams: 1 Mermaid fence(s) (rendered at create time)")
            && report.contains("writes: none"),
        "{report}"
    );
    assert!(!fake.called(), "dry-run must not start the adapter");

    let disabled = bob_command()
        .arg("highlights")
        .arg("create")
        .arg("--dry-run")
        .arg("-P")
        .arg("sase")
        .arg(&source)
        .arg("-b")
        .arg(&vault)
        .env("BOB_PANDOC_COMMAND", &missing_pandoc)
        .env("BOB_WEB_CLIP_ADAPTER", &fake.path)
        .env("BOB_HIGHLIGHTS_DIAGRAMS", "off")
        .output()
        .expect("run disabled Mermaid dry-run");
    assert_success(&disabled);
    let report = stdout(&disabled);
    assert!(
        report.contains(
            "diagrams: 1 Mermaid fence(s) (rendering disabled by BOB_HIGHLIGHTS_DIAGRAMS=off)"
        ) && report.contains("writes: none"),
        "{report}"
    );
    assert!(
        !fake.called(),
        "disabled dry-run must not start the adapter"
    );

    let snapshot = bob_command()
        .arg("highlights")
        .arg("create")
        .arg("--dry-run")
        .arg("diagram_area")
        .arg("-b")
        .arg(&vault)
        .env("BOB_NOW", "2026-10-16 12:00:00")
        .env("BOB_PANDOC_COMMAND", &missing_pandoc)
        .env("BOB_WEB_CLIP_ADAPTER", &fake.path)
        .output()
        .expect("run snapshot Mermaid dry-run");
    assert_success(&snapshot);
    let report = stdout(&snapshot);
    assert!(
        report
            .contains("diagrams: 1 Mermaid fence(s) (rendered at create time)")
            && report.contains("writes: none"),
        "{report}"
    );
    assert!(
        !fake.called(),
        "snapshot dry-run must not start the adapter"
    );
}

fn tool_available(program: &str, version_flag: &str) -> bool {
    Command::new(program)
        .arg(version_flag)
        .output()
        .is_ok_and(|output| output.status.success())
}

fn adapter_discovers_browser() -> bool {
    let adapter = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("scripts/web_clip/web_clip_adapter.py");
    let mut child = match Command::new("uv")
        .args(["run", "--quiet", "--script"])
        .arg(adapter)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
    {
        Ok(child) => child,
        Err(_) => return false,
    };
    if child.stdin.take().is_none_or(|mut stdin| {
        stdin.write_all(br#"{"protocol":1,"op":"ping"}"#).is_err()
    }) {
        let _ = child.kill();
        return false;
    }
    child
        .wait_with_output()
        .ok()
        .filter(|output| output.status.success())
        .and_then(|output| {
            serde_json::from_slice::<serde_json::Value>(&output.stdout).ok()
        })
        .and_then(|value| value.get("browser").cloned())
        .is_some_and(|browser| browser.is_object())
}

#[test]
fn highlights_create_live_mermaid_pdf_is_highlights_safe() {
    let required = [
        ("uv", "--version"),
        ("pandoc", "--version"),
        ("xelatex", "--version"),
        ("pdftotext", "-v"),
        ("pdffonts", "-v"),
        ("pdfimages", "-v"),
    ];
    let missing = required
        .iter()
        .filter_map(|(program, flag)| {
            (!tool_available(program, flag)).then_some(*program)
        })
        .collect::<Vec<_>>();
    if !missing.is_empty() {
        eprintln!(
            "skipping live Mermaid PDF test: missing {}",
            missing.join(", ")
        );
        return;
    }
    if !adapter_discovers_browser() {
        eprintln!("skipping live Mermaid PDF test: no browser discovered");
        return;
    }

    let temp = TempDir::new("bob-cli-diagrams-live");
    let long_tmpdir = temp
        .path()
        .join("long temporary directory with spaces and 日本語");
    std::fs::create_dir_all(&long_tmpdir).expect("create long TMPDIR");
    assert!(long_tmpdir.to_string_lossy().len() > 40);

    let vault = temp.path().join("scratch vault");
    seed_parent(&vault);
    let source_dir = temp.path().join("source files 日本語 with spaces");
    std::fs::create_dir_all(&source_dir).expect("create source directory");
    let source = source_dir.join("merge queue report.md");
    let markdown = concat!(
        "# Live Mermaid acceptance\n\n",
        "This prose remains available to read and highlight after rendering.\n\n",
        "```mermaid\n",
        "flowchart LR\n",
        "    A[\"Agent finishes its work\"] --> B[\"Master Gate tests run\"]\n",
        "    B --> C{\"Green?\"}\n",
        "    C -- \"yes\" --> D[\"Fine, for now\"]\n",
        "```\n\n",
        "```mermaid\n",
        "sequenceDiagram\n",
        "    participant Agent\n",
        "    participant Reviewer\n",
        "    Agent->>Reviewer: Send the result\n",
        "    Reviewer-->>Agent: Approve the change\n",
        "```\n\n",
        "- Nested diagram:\n\n",
        "  ~~~ {.mermaid}\n",
        "  flowchart TD\n",
        "      L[\"List item\"] --> R[\"Rendered\"]\n",
        "  ~~~\n\n",
        "```mermaid\n",
        "this is not valid mermaid {\n",
        "```\n\n",
        "This final paragraph also remains available after the figure pass.\n",
    );
    write_file(&source, markdown);

    let output = bob_command()
        .arg("highlights")
        .arg("create")
        .arg(&source)
        .arg("-P")
        .arg("sase")
        .arg("-b")
        .arg(&vault)
        .arg("-l")
        .arg(vault.join("lib"))
        .arg("-x")
        .arg(vault.join("xlib"))
        .arg("-r")
        .arg(vault.join("ref"))
        .arg("-N")
        .arg("mermaid_live_acceptance")
        .env("TMPDIR", &long_tmpdir)
        .env_remove("BOB_WEB_CLIP_ADAPTER")
        .env_remove("BOB_HIGHLIGHTS_DIAGRAMS")
        .env("NO_COLOR", "1")
        .output()
        .expect("run live Mermaid PDF render");
    assert_success(&output);
    let report = stdout(&output);
    assert!(
        report.contains("diagrams: rendered 3 of 4 Mermaid diagrams"),
        "{report}"
    );
    assert!(
        stderr(&output).contains("Mermaid diagram 4 of 4 not rendered:"),
        "{}",
        stderr(&output)
    );

    let pdf = vault.join("xlib/chat/mermaid_live_acceptance.pdf");
    let extracted = Command::new("pdftotext")
        .arg(&pdf)
        .arg("-")
        .output()
        .expect("extract live Mermaid PDF text");
    assert!(extracted.status.success());
    let text = String::from_utf8_lossy(&extracted.stdout);
    let normalized_text = text.split_whitespace().collect::<Vec<_>>().join(" ");
    assert!(
        normalized_text.contains("Agent finishes its work"),
        "{text}"
    );
    assert!(normalized_text.contains("Send the result"), "{text}");
    assert!(normalized_text.contains("List item"), "{text}");
    assert!(
        normalized_text.contains("This prose remains available"),
        "{text}"
    );
    assert!(
        normalized_text.contains("final paragraph also remains available"),
        "{text}"
    );
    assert!(
        !normalized_text.contains("flowchart LR"),
        "Mermaid source leaked: {text}"
    );
    assert!(
        normalized_text.contains("Mermaid diagram not rendered:"),
        "{text}"
    );
    assert!(
        normalized_text.contains("this is not valid mermaid"),
        "{text}"
    );

    let fonts = Command::new("pdffonts")
        .arg(&pdf)
        .output()
        .expect("inspect live Mermaid PDF fonts");
    assert!(fonts.status.success());
    let font_report = String::from_utf8_lossy(&fonts.stdout);
    assert!(!font_report.contains("Type 3"), "{font_report}");

    let images = Command::new("pdfimages")
        .arg("-list")
        .arg(&pdf)
        .output()
        .expect("inspect live Mermaid PDF images");
    assert!(images.status.success());
    let image_report = String::from_utf8_lossy(&images.stdout);
    let image_rows = image_report
        .lines()
        .filter(|line| {
            line.split_whitespace()
                .next()
                .is_some_and(|field| field.parse::<usize>().is_ok())
        })
        .collect::<Vec<_>>();
    assert!(image_rows.is_empty(), "{image_report}");
}
