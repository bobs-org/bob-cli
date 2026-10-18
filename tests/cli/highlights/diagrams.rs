//! Stub-adapter tests for Mermaid figures on both Markdown PDF routes.

use super::fake_clip::FakeDiagrams;
use crate::support::*;
use std::process::Command;

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
        eprintln!("skipping {test_name}: pandoc/xelatex is required");
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
    if Command::new("pdftotext").arg("-v").output().is_ok() {
        let output = Command::new("pdftotext")
            .arg("-layout")
            .arg(path)
            .arg("-")
            .output()
            .expect("pdftotext");
        if output.status.success() {
            return String::from_utf8_lossy(&output.stdout).into_owned();
        }
    }
    String::from_utf8_lossy(&std::fs::read(path).expect("read pdf"))
        .into_owned()
}

fn mermaid_source(extra: &str) -> String {
    format!("# Diagrams\n\n```mermaid\nflowchart LR\n  A --> B\n```\n{extra}")
}

fn seed_parent(vault: &std::path::Path) {
    write_file(&vault.join("sase.md"), "---\ntype: [[area]]\n---\n");
}

#[test]
fn highlights_create_renders_mermaid_via_stub_adapter() {
    if skip_pandoc_xelatex("mermaid success") {
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
    if skip_pandoc_xelatex("mermaid partial") {
        return;
    }
    let temp = TempDir::new("bob-cli-diagrams-partial");
    let vault = temp.path().join("vault");
    seed_parent(&vault);
    let source = temp.path().join("diagrams.md");
    write_file(
        &source,
        &mermaid_source("\n```mermaid\nthis is not valid mermaid\n```\n"),
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
        report.contains("diagrams: rendered 1 of 2 Mermaid diagrams"),
        "{report}"
    );
    let diagnostic = stderr(&output);
    assert!(
        diagnostic.contains(
            "warning: Mermaid diagram 2 of 2 not rendered: Parse error on line 1: bad"
        ),
        "{diagnostic}"
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
    if skip_pandoc_xelatex("mermaid degrade") {
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
    if skip_pandoc_xelatex("mermaid missing adapter") {
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
    if skip_pandoc_xelatex("mermaid kill switch") {
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
    if skip_pandoc_xelatex("snapshot mermaid") {
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
        .env("BOB_FAKE_DIAGRAMS_WIDTH_PT", "1200")
        .env("BOB_FAKE_DIAGRAMS_HEIGHT_PT", "100")
        .output()
        .expect("create wide mermaid");

    assert_success(&output);
    let diagnostic = stderr(&output);
    assert!(
        diagnostic.contains(
            "warning: Mermaid diagram 1 of 1 (flowchart LR) prints at"
        ) && diagnostic.contains("% scale; labels may be small"),
        "{diagnostic}"
    );
}
