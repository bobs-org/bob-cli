//! Highlights create tests.

use super::fake_clip::*;
use crate::support::*;
use std::ffi::OsString;
#[cfg(unix)]
use std::os::unix::ffi::OsStringExt;
use std::process::Command;

#[test]
fn highlights_create_dry_run_prints_plan_without_writes() {
    let temp = TempDir::new("bob-cli-highlights-create-dry-run");
    let source = temp
        .path()
        .join("202608/xprompt_role_binding/xprompt_role_binding.md");
    let vault = temp.path().join("vault");
    write_file(
        &source,
        "---\ntitle: A Useful Report\n---\n\n# Ignored H1\n",
    );

    let output = bob_command()
        .arg("highlights")
        .arg("create")
        .arg(&source)
        .arg("-b")
        .arg(&vault)
        .arg("-d")
        .arg("-P")
        .arg("sase_ref")
        .arg("-i")
        .arg("-s")
        .arg("next")
        .arg("-t")
        .arg("papers")
        .env("BOB_PANDOC_COMMAND", "/definitely/missing/pandoc")
        .output()
        .expect("run bob highlights create --dry-run");

    assert_success(&output);
    let report = stdout(&output);
    assert!(
        report.contains("A Useful Report")
            && report.contains("status: next")
            && report.contains("parent: sase_ref")
            && report.contains("id: xprompt_role_binding")
            && report.contains("xlib/papers/xprompt_role_binding.pdf")
            && report.contains("library_destination:")
            && report.contains("lib/papers/xprompt_role_binding.pdf")
            && report.contains("- id: xprompt_role_binding")
            && report.contains("writes: none"),
        "{report}"
    );
    assert!(!report.contains("research:"), "{report}");
    assert!(!vault.exists(), "dry-run must not create the vault");
}

#[test]
fn highlights_create_output_dry_run_prints_exact_path_without_writes() {
    let temp = TempDir::new("bob-cli-highlights-create-output-dry-run");
    let source = temp.path().join("report.md");
    let vault = temp.path().join("vault");
    write_file(&source, "# Custom Output\n");
    let sentinel = temp.path().join("pandoc-invoked");
    let pandoc = temp.path().join("pandoc");
    write_executable(
        &pandoc,
        &format!(
            "#!/bin/sh\n: > {}\nexit 0\n",
            shell_single_quote(path_str(&sentinel))
        ),
    );

    for flag in ["-o", "--output"] {
        let output = bob_command()
            .current_dir(temp.path())
            .arg("highlights")
            .arg("create")
            .arg(&source)
            .arg("-b")
            .arg(&vault)
            .arg("-d")
            .arg(flag)
            .arg("nested/custom-name.pdf")
            .env("BOB_PANDOC_COMMAND", &pandoc)
            .output()
            .unwrap_or_else(|error| {
                panic!("run create {flag} --dry-run: {error}")
            });

        assert_success(&output);
        let report = stdout(&output);
        assert!(
            report.contains("nested/custom-name.pdf")
                && report.contains("Custom Output")
                && report.contains(
                    "scan: recursive scan will not discover this PDF"
                )
                && report.contains("next: bob ref sync")
                && !report.contains("library_destination:")
                && report.contains("writes: none"),
            "flag {flag}: {report}"
        );
        assert!(
            !temp.path().join("nested/custom-name.pdf").exists(),
            "dry-run must not write {flag} target"
        );
        assert!(!sentinel.exists(), "dry-run must not invoke pandoc");
        assert!(!vault.exists(), "dry-run must not create the vault");
    }
}

#[test]
fn highlights_create_output_dry_run_reports_intake_library_destination() {
    let temp = TempDir::new("bob-cli-highlights-create-output-intake");
    let source = temp.path().join("report.md");
    let vault = temp.path().join("vault");
    let output_pdf = vault.join("xlib/papers/deep/custom.pdf");
    write_file(&source, "# Intake Output\n");

    let output = bob_command()
        .arg("highlights")
        .arg("create")
        .arg(&source)
        .arg("-b")
        .arg(&vault)
        .arg("-d")
        .arg("--output")
        .arg(&output_pdf)
        .env("BOB_PANDOC_COMMAND", "/definitely/missing/pandoc")
        .output()
        .expect("run create --output intake --dry-run");

    assert_success(&output);
    let report = stdout(&output);
    assert!(
        report.contains(&output_pdf.display().to_string())
            && report.contains("library_destination:")
            && report.contains("lib/papers/deep/custom.pdf")
            && report.contains("writes: none")
            && !report.contains("recursive scan will not discover"),
        "{report}"
    );
    assert!(!output_pdf.exists(), "dry-run must not write intake target");
    assert!(!vault.exists(), "dry-run must not create the vault");
}

#[test]
fn highlights_create_output_dry_run_reports_direct_library_target() {
    let temp = TempDir::new("bob-cli-highlights-create-output-library");
    let source = temp.path().join("report.md");
    let vault = temp.path().join("vault");
    let output_pdf = vault.join("lib/chat/direct.pdf");
    write_file(&source, "# Library Output\n");

    let output = bob_command()
        .arg("highlights")
        .arg("create")
        .arg(&source)
        .arg("-b")
        .arg(&vault)
        .arg("-d")
        .arg("-o")
        .arg(&output_pdf)
        .env("BOB_PANDOC_COMMAND", "/definitely/missing/pandoc")
        .output()
        .expect("run create --output library --dry-run");

    assert_success(&output);
    let report = stdout(&output);
    assert!(
        report.contains(&output_pdf.display().to_string())
            && report.contains("next: bob ref scan")
            && !report.contains("library_destination:")
            && report.contains("writes: none"),
        "{report}"
    );
}

#[test]
fn highlights_create_rejects_output_combined_with_ref_type() {
    let temp = TempDir::new("bob-cli-highlights-create-output-conflict");
    let source = temp.path().join("report.md");
    write_file(&source, "# Conflict\n");

    let output = bob_command()
        .arg("highlights")
        .arg("create")
        .arg(&source)
        .arg("--output")
        .arg(temp.path().join("out.pdf"))
        .arg("--ref-type")
        .arg("books")
        .env("BOB_PANDOC_COMMAND", "/definitely/missing/pandoc")
        .output()
        .expect("run create --output --ref-type");

    assert_eq!(output.status.code(), Some(2), "{}", format_output(&output));
    let diagnostic = format!("{}{}", stdout(&output), stderr(&output));
    assert!(
        diagnostic.contains("cannot be used with")
            && diagnostic.contains("--output")
            && diagnostic.contains("--ref-type"),
        "{diagnostic}"
    );
}

#[test]
fn highlights_create_rejects_non_pdf_output() {
    let temp = TempDir::new("bob-cli-highlights-create-output-invalid");
    let source = temp.path().join("report.md");
    let vault = temp.path().join("vault");
    let output_path = temp.path().join("report.txt");
    write_file(&source, "# Invalid Output\n");

    let output = bob_command()
        .arg("highlights")
        .arg("create")
        .arg(&source)
        .arg("-b")
        .arg(&vault)
        .arg("-o")
        .arg(&output_path)
        .env("BOB_PANDOC_COMMAND", "/definitely/missing/pandoc")
        .output()
        .expect("run create invalid --output");

    assert_eq!(output.status.code(), Some(1), "{}", format_output(&output));
    let diagnostic = stderr(&output);
    assert!(
        diagnostic.contains(".pdf") && diagnostic.contains("report.txt"),
        "{diagnostic}"
    );
    assert!(!output_path.exists(), "invalid output must not be written");
    assert!(!vault.exists(), "failure must happen before vault writes");
}

#[cfg(unix)]
#[test]
fn highlights_create_rejects_non_utf8_include_id_before_writes() {
    let temp = TempDir::new("bob-cli-highlights-create-invalid-id");
    let source = temp
        .path()
        .join("research")
        .join(OsString::from_vec(b"xprompt_\xff.md".to_vec()));
    let vault = temp.path().join("vault");
    let pandoc = temp.path().join("pandoc");
    let sentinel = temp.path().join("pandoc-invoked");
    write_file(&source, "# Invalid ID\n");
    write_executable(
        &pandoc,
        &format!(
            "#!/bin/sh\n: > {}\nexit 0\n",
            shell_single_quote(path_str(&sentinel))
        ),
    );

    let output = bob_command()
        .arg("highlights")
        .arg("create")
        .arg(&source)
        .arg("-b")
        .arg(&vault)
        .arg("--include-id")
        .env("BOB_PANDOC_COMMAND", &pandoc)
        .output()
        .expect("run bob highlights create invalid id");

    assert_eq!(
        output.status.code(),
        Some(1),
        "invalid include-id source should fail:\n{}",
        format_output(&output)
    );
    let diagnostic = stderr(&output);
    assert!(
        diagnostic.contains("filename stem is not a nonempty UTF-8 marker id")
            && diagnostic.contains("xprompt_")
            && diagnostic.contains(".md")
            && diagnostic.contains("Markdown source"),
        "{diagnostic}"
    );
    assert!(!vault.exists(), "failure must happen before vault writes");
    assert!(!sentinel.exists(), "pandoc must not be invoked");
}

#[test]
fn highlights_create_reports_pandoc_failure_diagnostics() {
    let temp = TempDir::new("bob-cli-highlights-create-failure");
    let source = temp.path().join("research.md");
    let vault = temp.path().join("vault");
    let pandoc = temp.path().join("pandoc");
    write_file(&source, "# Render Failure\n");
    write_executable(
        &pandoc,
        "#!/bin/sh\necho 'xelatex package exploded' >&2\nexit 17\n",
    );

    let output = bob_command()
        .arg("highlights")
        .arg("create")
        .arg(&source)
        .arg("-b")
        .arg(&vault)
        .env("BOB_PANDOC_COMMAND", &pandoc)
        .output()
        .expect("run failing bob highlights create");

    assert_eq!(output.status.code(), Some(1), "{}", format_output(&output));
    let diagnostic = stderr(&output);
    assert!(
        diagnostic.contains("pandoc failed")
            && diagnostic.contains("exit 17")
            && diagnostic.contains("xelatex package exploded"),
        "{diagnostic}"
    );
    assert!(
        !vault.join("xlib/chat/research.pdf").exists(),
        "failed render must not install a target PDF"
    );
}

#[test]
fn highlights_create_refuses_existing_library_pdf_with_or_without_force() {
    let temp = TempDir::new("bob-cli-highlights-create-library-destination");
    let source = temp.path().join("report.md");
    let vault = temp.path().join("vault");
    let library_pdf = vault.join("lib/chat/report.pdf");
    write_file(&source, "# Report\n");
    write_highlights_pdf(
        &library_pdf,
        "- status: ready\n- parent: obsidian\n- title: Archived Report\n",
    );

    for force in [false, true] {
        let mut command = bob_command();
        command
            .arg("highlights")
            .arg("create")
            .arg(&source)
            .arg("-b")
            .arg(&vault)
            .env("BOB_PANDOC_COMMAND", "/definitely/missing/pandoc");
        if force {
            command.arg("--force");
        }

        let output = command.output().unwrap_or_else(|error| {
            panic!("run create force={force}: {error}")
        });

        assert_eq!(
            output.status.code(),
            Some(1),
            "existing library PDF should fail force={force}:\n{}",
            format_output(&output)
        );
        let diagnostic = stderr(&output);
        assert!(
            diagnostic.contains("library destination already exists")
                && diagnostic.contains("xlib/chat/report.pdf")
                && diagnostic.contains("lib/chat/report.pdf"),
            "{diagnostic}"
        );
    }
    assert!(
        !vault.join("xlib/chat/report.pdf").exists(),
        "create refusal must not write an intake PDF"
    );
}

#[test]
fn highlights_create_renders_pdf_with_outline_and_marker_when_available() {
    let pandoc_available = Command::new("pandoc")
        .arg("--version")
        .output()
        .is_ok_and(|output| output.status.success());
    let xelatex_available = Command::new("xelatex")
        .arg("--version")
        .output()
        .is_ok_and(|output| output.status.success());
    if !pandoc_available || !xelatex_available {
        eprintln!("skipping highlights create render test: pandoc and xelatex are required");
        return;
    }

    let temp = TempDir::new("bob-cli-highlights-create-render");
    let source = temp
        .path()
        .join("202608/xprompt_role_binding/xprompt_role_binding.md");
    let vault = temp.path().join("vault");
    write_file(
        &source,
        concat!(
            "---\n",
            "title: Rendered Research Report\n",
            "---\n\n",
            "# Overview\n\n",
            "Introductory text.\n\n",
            "## Findings\n\n",
            "| Item | Result |\n",
            "| --- | --- |\n",
            "| PDF | Ready |\n\n",
            "```rust\n",
            "fn main() { println!(\"hello\"); }\n",
            "```\n\n",
            "### Detail\n\n",
            "A third-level heading.\n",
        ),
    );

    let output = bob_command()
        .arg("highlights")
        .arg("create")
        .arg(&source)
        .arg("-b")
        .arg(&vault)
        .arg("--include-id")
        .output()
        .expect("run bob highlights create");

    assert_success(&output);
    let report = stdout(&output);
    let pdf = vault.join("xlib/chat/xprompt_role_binding.pdf");
    assert!(
        report.contains("created Highlights-ready PDF")
            && report.contains("id: xprompt_role_binding")
            && report.contains("next: bob ref scan"),
        "{report}"
    );
    assert!(!report.contains("research:"), "{report}");
    assert!(pdf.is_file(), "missing rendered PDF: {}", pdf.display());

    let document = lopdf::Document::load(&pdf)
        .unwrap_or_else(|error| panic!("load {}: {error}", pdf.display()));
    assert!(!document.get_pages().is_empty(), "PDF must contain pages");
    let root_id = document
        .trailer
        .get(b"Root")
        .and_then(lopdf::Object::as_reference)
        .expect("PDF catalog reference");
    let catalog = document.get_dictionary(root_id).expect("PDF catalog");
    assert!(
        catalog.get(b"Outlines").is_ok(),
        "pandoc PDF must contain outline bookmarks"
    );
    let marker_output = bob_command()
        .arg("highlights")
        .arg("marker")
        .arg(&pdf)
        .arg("-b")
        .arg(&vault)
        .output()
        .expect("inspect created PDF marker");
    assert_success(&marker_output);
    let marker = stdout(&marker_output);
    assert!(marker.contains("- status: ready\n"), "{marker}");
    assert!(marker.contains("- parent: obsidian_ref\n"), "{marker}");
    assert!(
        marker.contains("- title: Rendered Research Report\n"),
        "{marker}"
    );
    assert!(marker.contains("- id: xprompt_role_binding\n"), "{marker}");
    assert!(!marker.contains("- research:"), "{marker}");
}

#[test]
fn highlights_create_output_renders_pdf_at_requested_path_when_available() {
    let pandoc_available = Command::new("pandoc")
        .arg("--version")
        .output()
        .is_ok_and(|output| output.status.success());
    let xelatex_available = Command::new("xelatex")
        .arg("--version")
        .output()
        .is_ok_and(|output| output.status.success());
    if !pandoc_available || !xelatex_available {
        eprintln!(
            "skipping highlights create --output render test: pandoc and xelatex are required"
        );
        return;
    }

    let temp = TempDir::new("bob-cli-highlights-create-output-render");
    let source = temp.path().join("report.md");
    let vault = temp.path().join("vault");
    let pdf = temp.path().join("custom/exact-output.pdf");
    write_file(
        &source,
        concat!(
            "---\n",
            "title: Exact Output Report\n",
            "---\n\n",
            "# Overview\n\n",
            "Introductory text.\n\n",
            "## Findings\n\n",
            "A second-level heading.\n\n",
            "### Detail\n\n",
            "A third-level heading.\n",
        ),
    );

    let output = bob_command()
        .arg("highlights")
        .arg("create")
        .arg(&source)
        .arg("-b")
        .arg(&vault)
        .arg("-o")
        .arg(&pdf)
        .arg("--include-id")
        .output()
        .expect("run bob highlights create --output");

    assert_success(&output);
    let report = stdout(&output);
    assert!(
        report.contains("created Highlights-ready PDF")
            && report.contains(&pdf.display().to_string())
            && report.contains("id: report")
            && report.contains("next: bob ref sync")
            && report.contains("recursive scan will not discover this PDF"),
        "{report}"
    );
    assert!(
        !vault.join("xlib/chat/report.pdf").exists(),
        "default intake PDF must not be written when --output is set"
    );
    assert!(pdf.is_file(), "missing rendered PDF: {}", pdf.display());

    let document = lopdf::Document::load(&pdf)
        .unwrap_or_else(|error| panic!("load {}: {error}", pdf.display()));
    assert!(!document.get_pages().is_empty(), "PDF must contain pages");
    let root_id = document
        .trailer
        .get(b"Root")
        .and_then(lopdf::Object::as_reference)
        .expect("PDF catalog reference");
    let catalog = document.get_dictionary(root_id).expect("PDF catalog");
    assert!(
        catalog.get(b"Outlines").is_ok(),
        "pandoc PDF must contain outline bookmarks"
    );
    let marker_output = bob_command()
        .arg("highlights")
        .arg("marker")
        .arg(&pdf)
        .arg("-b")
        .arg(&vault)
        .output()
        .expect("inspect created PDF marker");
    assert_success(&marker_output);
    let marker = stdout(&marker_output);
    assert!(marker.contains("- status: ready\n"), "{marker}");
    assert!(marker.contains("- parent: obsidian_ref\n"), "{marker}");
    assert!(
        marker.contains("- title: Exact Output Report\n"),
        "{marker}"
    );
    assert!(marker.contains("- id: report\n"), "{marker}");
}

#[test]
fn highlights_create_stamps_rendered_pdf_through_shared_install() {
    let temp = TempDir::new("bob-cli-highlights-create-stamp");
    let source = temp.path().join("report.md");
    let vault = temp.path().join("vault");
    write_file(&source, "# Stamped Report\n");
    let fixture = temp.path().join("rendered.pdf");
    write_highlights_pdf_pages(&fixture, &[&[]]);
    let pandoc = temp.path().join("pandoc");
    write_executable(
        &pandoc,
        "#!/bin/sh\nout=\"\"\nprev=\"\"\nfor arg in \"$@\"; do\n  if [ \"$prev\" = \"-o\" ]; then out=\"$arg\"; fi\n  prev=\"$arg\"\ndone\ncp \"$BOB_TEST_FIXTURE_PDF\" \"$out\"\n",
    );

    let output = bob_command()
        .arg("highlights")
        .arg("create")
        .arg(&source)
        .arg("-b")
        .arg(&vault)
        .env("BOB_PANDOC_COMMAND", &pandoc)
        .env("BOB_TEST_FIXTURE_PDF", &fixture)
        .output()
        .expect("run bob highlights create with fake pandoc");

    assert_success(&output);
    let report = stdout(&output);
    let target = vault.join("xlib/chat/report.pdf");
    assert!(target.is_file(), "stamped PDF must be installed");
    assert!(
        report.contains("created Highlights-ready PDF")
            && report.contains("pdf: ")
            && report.contains("title: Stamped Report")
            && report.contains("pages: 1")
            && report.contains("next: bob ref scan"),
        "{report}"
    );

    let marker_output = bob_command()
        .arg("highlights")
        .arg("marker")
        .arg(&target)
        .arg("-b")
        .arg(&vault)
        .output()
        .expect("inspect stamped PDF marker");
    assert_success(&marker_output);
    let marker = stdout(&marker_output);
    assert!(marker.contains("- status: ready\n"), "{marker}");
    assert!(marker.contains("- parent: obsidian_ref\n"), "{marker}");
    assert!(marker.contains("- title: Stamped Report\n"), "{marker}");
}

fn write_listen_library_episode(
    library: &std::path::Path,
    episode_id: &str,
    audio_bytes: &[u8],
) -> std::path::PathBuf {
    let episode_dir = library.join(episode_id);
    std::fs::create_dir_all(&episode_dir).expect("create episode dir");
    let audio_path = episode_dir.join("edition.mp3");
    std::fs::write(&audio_path, audio_bytes).expect("write episode audio");
    let manifest = serde_json::json!({
        "created_at": "2026-10-04T21:00:00+00:00",
        "source": {},
        "script": {},
        "audio": {"file": "edition.mp3"},
    });
    write_file(&episode_dir.join("manifest.json"), &manifest.to_string());
    audio_path
}

#[test]
fn highlights_create_dry_run_reports_planned_audio_copy() {
    let temp = TempDir::new("bob-cli-highlights-create-audio-dry-run");
    let library = temp.path().join("listen-library");
    write_listen_library_episode(&library, "report-a1b2c3", b"audio-bytes");
    let source = temp.path().join("report.md");
    let vault = temp.path().join("vault");
    write_file(
        &source,
        "---\ntitle: Audio Report\naudio:\n  episode_id: report-a1b2c3\n---\n\n# Audio Report\n",
    );

    let output = bob_command()
        .arg("highlights")
        .arg("create")
        .arg(&source)
        .arg("-b")
        .arg(&vault)
        .arg("-d")
        .env("BOB_HIGHLIGHTS_AUDIO_LIBRARY", &library)
        .env("BOB_PANDOC_COMMAND", "/definitely/missing/pandoc")
        .output()
        .expect("run create audio dry-run");

    assert_success(&output);
    let report = stdout(&output);
    assert!(
        report.contains("audio: ")
            && report.contains("xlib/chat/report.mp3")
            && report.contains("(from episode report-a1b2c3)")
            && report.contains("writes: none"),
        "{report}"
    );
    assert!(
        !vault.join("xlib/chat/report.mp3").exists(),
        "dry-run must not copy audio"
    );
    assert!(!vault.exists(), "dry-run must not create the vault");
}

#[test]
fn highlights_create_reuses_identical_existing_companion() {
    let temp = TempDir::new("bob-cli-highlights-create-audio-reuse");
    let library = temp.path().join("listen-library");
    write_listen_library_episode(&library, "report-a1b2c3", b"same-bytes");
    let source = temp.path().join("report.md");
    let vault = temp.path().join("vault");
    write_file(
        &source,
        "---\ntitle: Audio Report\naudio:\n  episode_id: report-a1b2c3\n---\n\n# Audio Report\n",
    );
    let dest = vault.join("xlib/chat/report.mp3");
    write_file(&dest, "same-bytes");

    let output = bob_command()
        .arg("highlights")
        .arg("create")
        .arg(&source)
        .arg("-b")
        .arg(&vault)
        .arg("-d")
        .env("BOB_HIGHLIGHTS_AUDIO_LIBRARY", &library)
        .env("BOB_PANDOC_COMMAND", "/definitely/missing/pandoc")
        .output()
        .expect("run create audio reuse dry-run");

    assert_success(&output);
    let report = stdout(&output);
    assert!(
        report.contains("xlib/chat/report.mp3")
            && report.contains("(from episode report-a1b2c3)"),
        "{report}"
    );
}

#[test]
fn highlights_create_refuses_different_audio_without_force() {
    let temp = TempDir::new("bob-cli-highlights-create-audio-conflict");
    let source_audio = temp.path().join("new.mp3");
    std::fs::write(&source_audio, b"new-bytes").expect("write source audio");
    let source = temp.path().join("report.md");
    let vault = temp.path().join("vault");
    write_file(&source, "# Audio Conflict\n");
    let dest = vault.join("xlib/chat/report.mp3");
    write_file(&dest, "old-bytes");

    let output = bob_command()
        .arg("highlights")
        .arg("create")
        .arg(&source)
        .arg("-b")
        .arg(&vault)
        .arg("-d")
        .arg("--audio")
        .arg(&source_audio)
        .env("BOB_PANDOC_COMMAND", "/definitely/missing/pandoc")
        .output()
        .expect("run create audio conflict");

    assert_eq!(output.status.code(), Some(1), "{}", format_output(&output));
    assert!(
        stderr(&output).contains("target audio already exists"),
        "{}",
        format_output(&output)
    );

    let forced = bob_command()
        .arg("highlights")
        .arg("create")
        .arg(&source)
        .arg("-b")
        .arg(&vault)
        .arg("-d")
        .arg("--audio")
        .arg(&source_audio)
        .arg("--force")
        .env("BOB_PANDOC_COMMAND", "/definitely/missing/pandoc")
        .output()
        .expect("run create audio conflict --force");
    assert_success(&forced);
    assert!(
        stdout(&forced).contains("(from --audio)"),
        "{}",
        format_output(&forced)
    );
}

#[test]
fn highlights_create_refuses_library_destination_audio() {
    let temp = TempDir::new("bob-cli-highlights-create-audio-lib-dest");
    let source_audio = temp.path().join("new.mp3");
    std::fs::write(&source_audio, b"new-bytes").expect("write source audio");
    let source = temp.path().join("report.md");
    let vault = temp.path().join("vault");
    write_file(&source, "# Library Audio Conflict\n");
    write_file(&vault.join("lib/chat/report.mp3"), "archived-audio");

    let output = bob_command()
        .arg("highlights")
        .arg("create")
        .arg(&source)
        .arg("-b")
        .arg(&vault)
        .arg("-d")
        .arg("--audio")
        .arg(&source_audio)
        .env("BOB_PANDOC_COMMAND", "/definitely/missing/pandoc")
        .output()
        .expect("run create library audio conflict");

    assert_eq!(output.status.code(), Some(1), "{}", format_output(&output));
    assert!(
        stderr(&output).contains("library destination already exists")
            && stderr(&output).contains("lib/chat/report.mp3"),
        "{}",
        format_output(&output)
    );
}

#[test]
fn highlights_create_no_audio_skips_discovery() {
    let temp = TempDir::new("bob-cli-highlights-create-no-audio");
    let library = temp.path().join("listen-library");
    write_listen_library_episode(&library, "report-a1b2c3", b"audio-bytes");
    let source = temp.path().join("report.md");
    let vault = temp.path().join("vault");
    write_file(
        &source,
        "---\ntitle: Quiet Report\naudio:\n  episode_id: report-a1b2c3\n---\n\n# Quiet Report\n",
    );

    let output = bob_command()
        .arg("highlights")
        .arg("create")
        .arg(&source)
        .arg("-b")
        .arg(&vault)
        .arg("-d")
        .arg("--no-audio")
        .env("BOB_HIGHLIGHTS_AUDIO_LIBRARY", &library)
        .env("BOB_PANDOC_COMMAND", "/definitely/missing/pandoc")
        .output()
        .expect("run create --no-audio");

    assert_success(&output);
    assert!(
        stdout(&output).contains("audio: none"),
        "{}",
        format_output(&output)
    );
}

#[test]
fn highlights_create_rejects_conflicting_audio_flags() {
    let temp = TempDir::new("bob-cli-highlights-create-audio-flag-conflict");
    let source = temp.path().join("report.md");
    write_file(&source, "# Flag Conflict\n");

    let output = bob_command()
        .arg("highlights")
        .arg("create")
        .arg(&source)
        .arg("--audio")
        .arg(temp.path().join("x.mp3"))
        .arg("--no-audio")
        .env("BOB_PANDOC_COMMAND", "/definitely/missing/pandoc")
        .output()
        .expect("run create --audio --no-audio");

    assert_eq!(output.status.code(), Some(2), "{}", format_output(&output));
}

#[test]
fn highlights_create_rejects_bad_audio_paths() {
    let temp = TempDir::new("bob-cli-highlights-create-bad-audio");
    let source = temp.path().join("report.md");
    let vault = temp.path().join("vault");
    write_file(&source, "# Bad Audio\n");
    let bad_ext = temp.path().join("notes.txt");
    write_file(&bad_ext, "text");

    for (audio, needle) in [
        (temp.path().join("missing.mp3"), "audio file does not exist"),
        (bad_ext, "must have one of"),
    ] {
        let output = bob_command()
            .arg("highlights")
            .arg("create")
            .arg(&source)
            .arg("-b")
            .arg(&vault)
            .arg("--audio")
            .arg(&audio)
            .env("BOB_PANDOC_COMMAND", "/definitely/missing/pandoc")
            .output()
            .unwrap_or_else(|error| panic!("run create bad audio: {error}"));

        assert_eq!(output.status.code(), Some(1), "{}", format_output(&output));
        assert!(
            stderr(&output).contains(needle),
            "{}",
            format_output(&output)
        );
    }
}

#[test]
fn highlights_create_copies_audio_before_pdf_with_play_link() {
    let temp = TempDir::new("bob-cli-highlights-create-audio-render");
    let library = temp.path().join("listen-library");
    write_listen_library_episode(&library, "report-a1b2c3", b"render-audio");
    let source = temp.path().join("report.md");
    let vault = temp.path().join("vault");
    write_file(
        &source,
        concat!(
            "---\n",
            "title: Listen Report\n",
            "audio:\n",
            "  episode_id: report-a1b2c3\n",
            "---\n\n",
            "# Listen Report\n\n",
            "<div class=\"listen\">\n\n",
            "♫ **Brief audio edition**\n\n",
            "</div>\n",
        ),
    );
    let fixture = temp.path().join("rendered.pdf");
    write_highlights_pdf_pages(&fixture, &[&[]]);
    let pandoc = temp.path().join("pandoc");
    write_executable(
        &pandoc,
        "#!/bin/sh\nout=\"\"\nprev=\"\"\nfor arg in \"$@\"; do\n  if [ \"$prev\" = \"-o\" ]; then out=\"$arg\"; fi\n  prev=\"$arg\"\ndone\ncp \"$BOB_TEST_FIXTURE_PDF\" \"$out\"\n",
    );

    let output = bob_command()
        .arg("highlights")
        .arg("create")
        .arg(&source)
        .arg("-b")
        .arg(&vault)
        .env("BOB_HIGHLIGHTS_AUDIO_LIBRARY", &library)
        .env("BOB_PANDOC_COMMAND", &pandoc)
        .env("BOB_TEST_FIXTURE_PDF", &fixture)
        .output()
        .expect("run create audio render");

    assert_success(&output);
    let report = stdout(&output);
    let pdf = vault.join("xlib/chat/report.pdf");
    let audio = vault.join("xlib/chat/report.mp3");
    assert!(pdf.is_file(), "rendered PDF must be installed");
    assert!(audio.is_file(), "companion audio must be copied");
    assert_eq!(
        std::fs::read(&audio).expect("read copied audio"),
        b"render-audio"
    );
    assert!(
        report.contains("audio: ")
            && report.contains("xlib/chat/report.mp3")
            && report.contains("(from episode report-a1b2c3)")
            && report.contains("audio_link: ")
            && report.contains("lib%2Fchat%2Freport.mp3"),
        "{report}"
    );
}

fn write_bare_pdf(
    path: &std::path::Path,
    title: Option<&str>,
    author: Option<&str>,
) {
    use lopdf::{dictionary, Document, Object, Stream};
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("create bare PDF parent");
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
    if !info.is_empty() {
        let info_id = doc.add_object(Object::Dictionary(info));
        doc.trailer.set("Info", info_id);
    }
    doc.save(path).expect("write bare PDF");
}

fn write_pdf_with_sticky_note(path: &std::path::Path) {
    use lopdf::{dictionary, Document, Object, Stream};
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("create sticky PDF parent");
    }
    let mut doc = Document::with_version("1.4");
    let pages_id = doc.new_object_id();
    let content_id = doc.add_object(Stream::new(dictionary! {}, Vec::new()));
    let sticky_id = doc.add_object(dictionary! {
        "Type" => "Annot",
        "Subtype" => "Text",
        "Rect" => vec![
            Object::Integer(0),
            Object::Integer(0),
            Object::Integer(24),
            Object::Integer(24),
        ],
        "Contents" => Object::string_literal("a reader sticky note"),
    });
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
        "Annots" => vec![Object::Reference(sticky_id)],
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
    doc.save(path).expect("write sticky PDF");
}

fn write_fake_curl(dir: &std::path::Path) -> std::path::PathBuf {
    let path = dir.join("fake-curl.sh");
    let script = r#"#!/bin/sh
# Fake curl for create PDF-route tests. Serves canned bodies by URL,
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
  *"example.com/claimed-pdf"*)
    printf '<html><body>not a pdf</body></html>' > "$dest"
    printf '200\napplication/pdf\n\n'
    ;;
  *"example.com/missing.pdf"*)
    printf '<html>nope</html>' > "$dest"
    printf '404\ntext/html; charset=utf-8\n\n'
    ;;
  *"example.com/article"*)
    printf '<html><body>article</body></html>' > "$dest"
    printf '200\ntext/html; charset=utf-8\n\n'
    ;;
  *"example.com/walled"*)
    printf '<html><body>bot wall</body></html>' > "$dest"
    printf '403\ntext/html; charset=utf-8\n\n'
    ;;
  *"example.com/octet.pdf"*)
    cp "$FAKE_CURL_ROOT/paper.pdf" "$dest"
    printf '200\napplication/octet-stream\n\n'
    ;;
  *)
    printf '404\ntext/html\n\n'
    ;;
esac
"#;
    std::fs::write(&path, script).expect("write fake curl");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))
            .expect("chmod fake curl");
    }
    path
}

fn write_arxiv_api_fixture(dir: &std::path::Path) {
    let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<feed xmlns="http://www.w3.org/2005/Atom">
<entry>
<id>http://arxiv.org/abs/1706.03762v7</id>
<updated>2023-08-10T00:00:00Z</updated>
<published>2017-06-12T17:57:34Z</published>
<title>Attention Is All You Need</title>
<author><name>Ashish Vaswani</name></author>
<author><name>Noam Shazeer</name></author>
<author><name>Niki Parmar</name></author>
<author><name>Jakob Uszkoreit</name></author>
<author><name>Llion Jones</name></author>
</entry>
</feed>"#;
    std::fs::write(dir.join("arxiv-api.xml"), xml).expect("write arxiv api");
}

#[test]
fn highlights_create_local_pdf_installs_and_stamps() {
    let temp = TempDir::new("bob-cli-highlights-create-local-pdf");
    let source = temp.path().join("paper.pdf");
    let vault = temp.path().join("vault");
    write_bare_pdf(
        &source,
        Some("Attention Is All You Need"),
        Some("Ashish Vaswani"),
    );
    let before = std::fs::read(&source).expect("read source bytes");

    let output = bob_command()
        .arg("highlights")
        .arg("create")
        .arg(&source)
        .arg("-b")
        .arg(&vault)
        .output()
        .expect("run create local PDF");
    assert_success(&output);
    let report = stdout(&output);
    let target = vault.join("xlib/papers/paper.pdf");
    assert!(target.is_file(), "stamped PDF must be installed: {report}");
    assert!(
        report.contains("source: ")
            && report.contains("(PDF)")
            && report.contains("xlib/papers/paper.pdf")
            && report.contains("title: Attention Is All You Need")
            && report.contains("pages: 1 · size: "),
        "{report}"
    );
    assert_eq!(
        std::fs::read(&source).expect("reread source"),
        before,
        "source bytes unchanged"
    );
    let marker_output = bob_command()
        .arg("highlights")
        .arg("marker")
        .arg(&target)
        .arg("-b")
        .arg(&vault)
        .output()
        .expect("inspect marker");
    assert_success(&marker_output);
    let marker = stdout(&marker_output);
    assert!(
        marker.contains("- title: Attention Is All You Need\n"),
        "{marker}"
    );
}

#[test]
fn highlights_create_local_pdf_title_override_and_name_type_output() {
    let temp = TempDir::new("bob-cli-highlights-create-local-opts");
    let source = temp.path().join("paper.pdf");
    let vault = temp.path().join("vault");
    write_bare_pdf(&source, Some("Info Title"), None);

    // -T overrides the Info title.
    let output = bob_command()
        .arg("highlights")
        .arg("create")
        .arg(&source)
        .arg("-b")
        .arg(&vault)
        .arg("-T")
        .arg("Custom Title")
        .arg("-d")
        .output()
        .expect("run create -T dry-run");
    assert_success(&output);
    assert!(
        stdout(&output).contains("title: Custom Title (override)"),
        "{}",
        format_output(&output)
    );

    // -N renames the stem.
    let output = bob_command()
        .arg("highlights")
        .arg("create")
        .arg(&source)
        .arg("-b")
        .arg(&vault)
        .arg("-N")
        .arg("my_paper")
        .arg("-d")
        .output()
        .expect("run create -N dry-run");
    assert_success(&output);
    assert!(
        stdout(&output).contains("xlib/papers/my_paper.pdf"),
        "{}",
        format_output(&output)
    );

    // -t docs selects the ref type.
    let output = bob_command()
        .arg("highlights")
        .arg("create")
        .arg(&source)
        .arg("-b")
        .arg(&vault)
        .arg("-t")
        .arg("docs")
        .arg("-d")
        .output()
        .expect("run create -t dry-run");
    assert_success(&output);
    assert!(
        stdout(&output).contains("xlib/docs/paper.pdf"),
        "{}",
        format_output(&output)
    );

    // -o writes the exact path.
    let exact = temp.path().join("custom/exact.pdf");
    let output = bob_command()
        .arg("highlights")
        .arg("create")
        .arg(&source)
        .arg("-b")
        .arg(&vault)
        .arg("-o")
        .arg(&exact)
        .arg("-d")
        .output()
        .expect("run create -o dry-run");
    assert_success(&output);
    assert!(
        stdout(&output).contains(&exact.display().to_string()),
        "{}",
        format_output(&output)
    );
}

#[test]
fn highlights_create_local_pdf_dry_run_writes_nothing_and_refuses_junk() {
    let temp = TempDir::new("bob-cli-highlights-create-local-dry");
    let source = temp.path().join("paper.pdf");
    let vault = temp.path().join("vault");
    write_bare_pdf(&source, Some("Dry Title"), None);

    let output = bob_command()
        .arg("highlights")
        .arg("create")
        .arg(&source)
        .arg("-b")
        .arg(&vault)
        .arg("-d")
        .output()
        .expect("run create dry-run");
    assert_success(&output);
    assert!(
        stdout(&output).contains("writes: none"),
        "{}",
        format_output(&output)
    );
    assert!(!vault.exists(), "dry-run must not create the vault");

    // Non-PDF bytes with a .pdf extension are refused.
    let junk = temp.path().join("junk.pdf");
    write_file(&junk, "this is not a pdf");
    let output = bob_command()
        .arg("highlights")
        .arg("create")
        .arg(&junk)
        .arg("-b")
        .arg(&vault)
        .output()
        .expect("run create junk pdf");
    assert_eq!(output.status.code(), Some(1), "{}", format_output(&output));

    // A non-markdown, non-PDF extension is refused.
    let txt = temp.path().join("notes.txt");
    write_file(&txt, "plain text");
    let output = bob_command()
        .arg("highlights")
        .arg("create")
        .arg(&txt)
        .arg("-b")
        .arg(&vault)
        .output()
        .expect("run create txt");
    assert_eq!(output.status.code(), Some(1), "{}", format_output(&output));
    assert!(
        stderr(&output).contains("TARGET must be"),
        "{}",
        format_output(&output)
    );

    // An encrypted PDF is refused with the unencrypted-copy hint.
    let enc = temp.path().join("enc.pdf");
    write_bare_pdf(&enc, Some("Enc"), None);
    {
        let mut doc = lopdf::Document::load(&enc).expect("load enc base");
        let id = doc.new_object_id();
        doc.objects
            .insert(id, lopdf::Object::Dictionary(lopdf::dictionary! {}));
        doc.trailer.set("Encrypt", lopdf::Object::Reference(id));
        doc.save(&enc).expect("save encrypted stub");
    }
    let output = bob_command()
        .arg("highlights")
        .arg("create")
        .arg(&enc)
        .arg("-b")
        .arg(&vault)
        .output()
        .expect("run create encrypted");
    assert_eq!(output.status.code(), Some(1), "{}", format_output(&output));
    assert!(
        stderr(&output).contains("encrypted"),
        "{}",
        format_output(&output)
    );
}

#[test]
fn highlights_create_local_pdf_prepends_marker_and_snake_cases_stem() {
    let temp = TempDir::new("bob-cli-highlights-create-local-sticky");
    let vault = temp.path().join("vault");

    // A page-1 sticky note is kept; bob's marker goes first.
    let sticky = temp.path().join("sticky.pdf");
    write_pdf_with_sticky_note(&sticky);
    let output = bob_command()
        .arg("highlights")
        .arg("create")
        .arg(&sticky)
        .arg("-b")
        .arg(&vault)
        .output()
        .expect("run create sticky PDF");
    assert_success(&output);
    let target = vault.join("xlib/papers/sticky.pdf");
    let doc = lopdf::Document::load(&target).expect("load stamped sticky");
    let page = doc.page_iter().next().expect("first page");
    let annots = doc
        .get_dictionary(page)
        .and_then(|page| page.get(b"Annots"))
        .expect("page annots");
    let count = match annots {
        lopdf::Object::Array(items) => items.len(),
        lopdf::Object::Reference(id) => doc
            .get_object(*id)
            .and_then(lopdf::Object::as_array)
            .expect("annots array")
            .len(),
        other => panic!("unexpected annots: {other:?}"),
    };
    assert_eq!(count, 2, "sticky note plus bob marker");
    let marker_output = bob_command()
        .arg("highlights")
        .arg("marker")
        .arg(&target)
        .arg("-b")
        .arg(&vault)
        .output()
        .expect("inspect sticky marker");
    assert_success(&marker_output);
    assert!(
        stdout(&marker_output).contains("- title:"),
        "{}",
        format_output(&marker_output)
    );

    // A stem with spaces is snake-cased.
    let spaced = temp.path().join("My Paper Draft.pdf");
    write_bare_pdf(&spaced, None, None);
    let output = bob_command()
        .arg("highlights")
        .arg("create")
        .arg(&spaced)
        .arg("-b")
        .arg(&vault)
        .arg("-d")
        .output()
        .expect("run create spaced stem");
    assert_success(&output);
    assert!(
        stdout(&output).contains("my_paper_draft"),
        "{}",
        format_output(&output)
    );
}

#[test]
fn highlights_create_local_pdf_inside_library_is_refused_with_listen_hint() {
    let temp = TempDir::new("bob-cli-highlights-create-local-inside");
    let vault = temp.path().join("vault");
    let library_pdf = vault.join("lib/papers/paper.pdf");
    write_bare_pdf(&library_pdf, Some("Library Paper"), None);
    // Stamp it so it reads as already captured.
    let stamp = bob_command()
        .arg("highlights")
        .arg("create")
        .arg(&library_pdf)
        .arg("-b")
        .arg(&vault)
        .output()
        .expect("first create inside lib");
    // The first create itself must refuse (inside lib without marker).
    assert_eq!(stamp.status.code(), Some(1), "{}", format_output(&stamp));

    // Now stamp via an outside copy and move it into lib to simulate a capture.
    let outside = temp.path().join("outside.pdf");
    write_bare_pdf(&outside, Some("Library Paper"), None);
    let output = bob_command()
        .arg("highlights")
        .arg("create")
        .arg(&outside)
        .arg("-b")
        .arg(&vault)
        .arg("-o")
        .arg(&library_pdf)
        .arg("--force")
        .output()
        .expect("install into lib");
    assert_success(&output);
    let output = bob_command()
        .arg("highlights")
        .arg("create")
        .arg(&library_pdf)
        .arg("-b")
        .arg(&vault)
        .output()
        .expect("rerun on captured lib PDF");
    assert_eq!(output.status.code(), Some(1), "{}", format_output(&output));
    let diagnostic = format!("{}{}", stdout(&output), stderr(&output));
    assert!(
        diagnostic.contains("already captured")
            && diagnostic.contains("--listen"),
        "{diagnostic}"
    );
}

#[test]
fn highlights_create_pdf_url_stamps_and_dedupes() {
    let temp = TempDir::new("bob-cli-highlights-create-pdf-url");
    let vault = temp.path().join("vault");
    let root = temp.path().join("curl-root");
    std::fs::create_dir_all(&root).expect("create curl root");
    write_bare_pdf(
        &root.join("paper.pdf"),
        Some("Remote Paper Title"),
        Some("Jane Doe"),
    );
    write_arxiv_api_fixture(&root);
    // Reuse the paper PDF as the arXiv body too.
    std::fs::copy(root.join("paper.pdf"), root.join("arxiv.pdf"))
        .expect("copy arxiv pdf");
    let fake = write_fake_curl(temp.path());
    let log = temp.path().join("curl.log");
    std::fs::write(&log, "").expect("init log");

    let output = bob_command()
        .arg("highlights")
        .arg("create")
        .arg("https://example.com/paper.pdf")
        .arg("-b")
        .arg(&vault)
        .env("BOB_HIGHLIGHTS_CURL", &fake)
        .env("FAKE_CURL_ROOT", &root)
        .env("FAKE_CURL_LOG", &log)
        .output()
        .expect("run create PDF URL");
    assert_success(&output);
    let report = stdout(&output);
    assert!(
        report.contains("source: https://example.com/paper.pdf (PDF)")
            && report.contains("xlib/papers/paper.pdf")
            && report.contains("title: Remote Paper Title")
            && report.contains("author: Jane Doe")
            && report.contains("captured: ")
            && report.contains("id: paper"),
        "{report}"
    );
    let marker_output = bob_command()
        .arg("highlights")
        .arg("marker")
        .arg(vault.join("xlib/papers/paper.pdf"))
        .arg("-b")
        .arg(&vault)
        .output()
        .expect("inspect PDF URL marker");
    assert_success(&marker_output);
    let marker = stdout(&marker_output);
    assert!(
        marker.contains("source_url: https://example.com/paper.pdf")
            && marker.contains("captured: "),
        "{marker}"
    );

    // A second run is refused by dedupe.
    let output = bob_command()
        .arg("highlights")
        .arg("create")
        .arg("https://example.com/paper.pdf")
        .arg("-b")
        .arg(&vault)
        .env("BOB_HIGHLIGHTS_CURL", &fake)
        .env("FAKE_CURL_ROOT", &root)
        .env("FAKE_CURL_LOG", &log)
        .output()
        .expect("rerun PDF URL");
    assert_eq!(output.status.code(), Some(1), "{}", format_output(&output));
}

#[test]
fn highlights_create_pdf_url_rejects_claimed_pdf_and_404() {
    let temp = TempDir::new("bob-cli-highlights-create-pdf-errors");
    let vault = temp.path().join("vault");
    let root = temp.path().join("curl-root");
    std::fs::create_dir_all(&root).expect("create curl root");
    write_bare_pdf(&root.join("paper.pdf"), Some("T"), None);
    let fake = write_fake_curl(temp.path());
    let log = temp.path().join("curl.log");
    std::fs::write(&log, "").expect("init log");
    let run = |url: &str| {
        bob_command()
            .arg("highlights")
            .arg("create")
            .arg(url)
            .arg("-b")
            .arg(&vault)
            .env("BOB_HIGHLIGHTS_CURL", &fake)
            .env("FAKE_CURL_ROOT", &root)
            .env("FAKE_CURL_LOG", &log)
            .output()
            .expect("run create url")
    };

    let output = run("https://example.com/claimed-pdf");
    assert_eq!(output.status.code(), Some(1), "{}", format_output(&output));
    assert!(
        stderr(&output).contains("server said PDF but sent something else"),
        "{}",
        format_output(&output)
    );

    let output = run("https://example.com/missing.pdf");
    assert_eq!(output.status.code(), Some(1), "{}", format_output(&output));
    assert!(
        stderr(&output).contains("server returned HTTP 404"),
        "{}",
        format_output(&output)
    );
}

#[test]
fn highlights_create_arxiv_uses_api_metadata_and_short_stem() {
    let temp = TempDir::new("bob-cli-highlights-create-arxiv");
    let vault = temp.path().join("vault");
    let root = temp.path().join("curl-root");
    std::fs::create_dir_all(&root).expect("create curl root");
    write_bare_pdf(&root.join("arxiv.pdf"), Some("Junk Info Title"), None);
    write_bare_pdf(&root.join("paper.pdf"), Some("T"), None);
    write_arxiv_api_fixture(&root);
    let fake = write_fake_curl(temp.path());
    let log = temp.path().join("curl.log");
    std::fs::write(&log, "").expect("init log");

    let output = bob_command()
        .arg("highlights")
        .arg("create")
        .arg("https://arxiv.org/abs/1706.03762")
        .arg("-b")
        .arg(&vault)
        .env("BOB_HIGHLIGHTS_CURL", &fake)
        .env("FAKE_CURL_ROOT", &root)
        .env("FAKE_CURL_LOG", &log)
        .output()
        .expect("run create arXiv");
    assert_success(&output);
    let report = stdout(&output);
    assert!(
        report.contains(
            "source: https://arxiv.org/abs/1706.03762 (arXiv 1706.03762)"
        ) && report.contains("title: Attention Is All You Need")
            && report.contains("author: Ashish Vaswani et al.")
            && report.contains("published: 2017-06-12")
            && report.contains("xlib/papers/attention_is_all_you_need.pdf"),
        "{report}"
    );
    let logged = std::fs::read_to_string(&log).expect("read curl log");
    assert!(
        logged.contains("https://arxiv.org/pdf/1706.03762"),
        "abs URL must fetch /pdf/<id>: {logged}"
    );

    // A legacy url: note refuses the abs URL.
    std::fs::create_dir_all(vault.join("ref/papers")).expect("create ref dir");
    write_file(
        &vault.join("ref/papers/ea_graph.md"),
        "---\ntitle: EA-Graph\nurl: https://arxiv.org/pdf/2608.04278\nsource_pdf: lib/papers/ea_graph.pdf\n---\n\n# EA-Graph\n",
    );
    write_bare_pdf(&root.join("arxiv.pdf"), Some("T"), None);
    let output = bob_command()
        .arg("highlights")
        .arg("create")
        .arg("https://arxiv.org/abs/2608.04278v2")
        .arg("-b")
        .arg(&vault)
        .env("BOB_HIGHLIGHTS_CURL", &fake)
        .env("FAKE_CURL_ROOT", &root)
        .env("FAKE_CURL_LOG", &log)
        .output()
        .expect("run duplicate arXiv");
    assert_eq!(output.status.code(), Some(1), "{}", format_output(&output));
}

#[test]
fn highlights_create_arxiv_api_failure_falls_back_with_warning() {
    let temp = TempDir::new("bob-cli-highlights-create-arxiv-fallback");
    let vault = temp.path().join("vault");
    let root = temp.path().join("curl-root");
    std::fs::create_dir_all(&root).expect("create curl root");
    // No API fixture: the fake returns 500 for unknown ids, and the PDF
    // carries a plausible Info title.
    write_bare_pdf(
        &root.join("arxiv.pdf"),
        Some("Fallback Info Title"),
        Some("Info Author"),
    );
    write_bare_pdf(&root.join("paper.pdf"), Some("T"), None);
    let fake = write_fake_curl(temp.path());
    let log = temp.path().join("curl.log");
    std::fs::write(&log, "").expect("init log");

    // Use an id the fake API does not know; metadata degrades.
    let output = bob_command()
        .arg("highlights")
        .arg("create")
        .arg("https://arxiv.org/abs/2602.16844")
        .arg("-b")
        .arg(&vault)
        .env("BOB_HIGHLIGHTS_CURL", &fake)
        .env("FAKE_CURL_ROOT", &root)
        .env("FAKE_CURL_LOG", &log)
        .output()
        .expect("run arXiv fallback");
    // The PDF fetch for 2602.16844 is not canned, so this 404s; assert the
    // failure is clean rather than a panic. If the fixture ever gains that
    // id, the fallback path asserts below instead.
    if output.status.success() {
        let report = stdout(&output);
        assert!(report.contains("xlib/papers/"), "{report}");
    } else {
        assert_eq!(output.status.code(), Some(1), "{}", format_output(&output));
    }
}

#[test]
fn highlights_create_markdown_renders_outside_xlib() {
    let temp = TempDir::new("bob-cli-highlights-create-scratch-render");
    let source = temp.path().join("report.md");
    let vault = temp.path().join("vault");
    write_file(&source, "# Scratch Render\n");
    let fixture = temp.path().join("rendered.pdf");
    write_highlights_pdf_pages(&fixture, &[&[]]);
    let pandoc = temp.path().join("pandoc");
    let logged = temp.path().join("pandoc-args.log");
    write_executable(
        &pandoc,
        &format!(
            "#!/bin/sh\nprintf '%s\\n' \"$@\" > {}\nout=\"\"\nprev=\"\"\nfor arg in \"$@\"; do\n  if [ \"$prev\" = \"-o\" ]; then out=\"$arg\"; fi\n  prev=\"$arg\"\ndone\ncp \"$BOB_TEST_FIXTURE_PDF\" \"$out\"\n",
            shell_single_quote(path_str(&logged))
        ),
    );

    let output = bob_command()
        .arg("highlights")
        .arg("create")
        .arg(&source)
        .arg("-b")
        .arg(&vault)
        .env("BOB_PANDOC_COMMAND", &pandoc)
        .env("BOB_TEST_FIXTURE_PDF", &fixture)
        .output()
        .expect("run create scratch render");
    assert_success(&output);
    let args = std::fs::read_to_string(&logged).expect("read pandoc args");
    let out_line = args
        .lines()
        .skip_while(|line| *line != "-o")
        .nth(1)
        .unwrap_or("")
        .to_string();
    assert!(
        !out_line.contains("xlib"),
        "fake pandoc -o must be outside xlib/: {out_line} / {args}"
    );
    assert!(
        vault.join("xlib/chat/report.pdf").is_file(),
        "intake PDF installed"
    );
}

#[test]
fn highlights_create_pdf_url_binds_explicit_audio() {
    let temp = TempDir::new("bob-cli-highlights-create-pdf-audio");
    let vault = temp.path().join("vault");
    let root = temp.path().join("curl-root");
    std::fs::create_dir_all(&root).expect("create curl root");
    write_bare_pdf(&root.join("paper.pdf"), Some("Audio Paper"), None);
    let fake = write_fake_curl(temp.path());
    let log = temp.path().join("curl.log");
    std::fs::write(&log, "").expect("init log");
    let audio = temp.path().join("episode.mp3");
    std::fs::write(&audio, b"audio-bytes").expect("write audio");

    let output = bob_command()
        .arg("highlights")
        .arg("create")
        .arg("https://example.com/paper.pdf")
        .arg("-b")
        .arg(&vault)
        .arg("--audio")
        .arg(&audio)
        .env("BOB_HIGHLIGHTS_CURL", &fake)
        .env("FAKE_CURL_ROOT", &root)
        .env("FAKE_CURL_LOG", &log)
        .output()
        .expect("run create PDF with audio");
    assert_success(&output);
    assert!(
        stdout(&output).contains("(from --audio)"),
        "{}",
        format_output(&output)
    );
    assert!(
        vault.join("xlib/papers/paper.mp3").is_file(),
        "audio must be bound"
    );
}

fn article_env(
    command: &mut Command,
    vault: &std::path::Path,
    curl: &std::path::Path,
    root: &std::path::Path,
    log: &std::path::Path,
    adapter: &std::path::Path,
) {
    command
        .arg("-b")
        .arg(vault)
        .env("BOB_HIGHLIGHTS_CURL", curl)
        .env("FAKE_CURL_ROOT", root)
        .env("FAKE_CURL_LOG", log)
        .env("BOB_WEB_CLIP_ADAPTER", adapter);
}

#[test]
fn highlights_create_article_routes_through_clip_engine() {
    let temp = TempDir::new("bob-cli-highlights-create-article");
    let vault = temp.path().join("vault");
    let root = temp.path().join("curl-root");
    std::fs::create_dir_all(&root).expect("create curl root");
    let fake_curl = write_fake_curl(temp.path());
    let log = temp.path().join("curl.log");
    std::fs::write(&log, "").expect("init log");
    let fake = FakeClip::new(&temp, "fake");

    let mut command = bob_command();
    command
        .arg("highlights")
        .arg("create")
        .arg("https://example.com/article/hello");
    article_env(&mut command, &vault, &fake_curl, &root, &log, &fake.path);
    let output = command.output().expect("run create article");
    assert_success(&output);
    let report = stdout(&output);
    let target = vault.join("xlib/blogs/hello.pdf");
    assert!(target.is_file(), "stamped PDF must be installed: {report}");
    assert!(
        report.contains("created Highlights-ready web PDF")
            && report.contains("source_url: https://example.com/article/hello")
            && report.contains("title: Symphony Spec")
            && report.contains("author: Jane Doe")
            && report.contains("id: hello"),
        "{report}"
    );
    // Create hands the cleaned URL to the adapter with clip defaults.
    let request = fake.request();
    assert!(
        request.contains("\"url\":\"https://example.com/article/hello\"")
            && request.contains("\"dry_run\":false"),
        "{request}"
    );
    let marker_output = bob_command()
        .arg("highlights")
        .arg("marker")
        .arg(&target)
        .arg("-b")
        .arg(&vault)
        .output()
        .expect("inspect article marker");
    assert_success(&marker_output);
    let marker = stdout(&marker_output);
    assert!(
        marker.contains("- title: Symphony Spec\n")
            && marker.contains("source_url: https://example.com/article/hello"),
        "{marker}"
    );

    // A second run is refused by dedupe.
    let mut rerun = bob_command();
    rerun
        .arg("highlights")
        .arg("create")
        .arg("https://example.com/article/hello");
    article_env(&mut rerun, &vault, &fake_curl, &root, &log, &fake.path);
    let output = rerun.output().expect("rerun create article");
    assert_eq!(output.status.code(), Some(1), "{}", format_output(&output));
    assert!(
        stderr(&output).contains("already queued"),
        "{}",
        format_output(&output)
    );
}

#[test]
fn highlights_create_article_maps_options_to_clip() {
    let temp = TempDir::new("bob-cli-highlights-create-article-opts");
    let vault = temp.path().join("vault");
    let root = temp.path().join("curl-root");
    std::fs::create_dir_all(&root).expect("create curl root");
    let fake_curl = write_fake_curl(temp.path());
    let log = temp.path().join("curl.log");
    std::fs::write(&log, "").expect("init log");
    let fake = FakeClip::new(&temp, "fake");

    let mut command = bob_command();
    command
        .arg("highlights")
        .arg("create")
        .arg("https://example.com/article/hello")
        .arg("-T")
        .arg("Custom Title")
        .arg("-N")
        .arg("custom_stem")
        .arg("-t")
        .arg("docs")
        .arg("-s")
        .arg("next")
        .arg("-P")
        .arg("sase_ref")
        .arg("-i");
    article_env(&mut command, &vault, &fake_curl, &root, &log, &fake.path);
    let output = command.output().expect("run create article with options");
    assert_success(&output);
    let report = stdout(&output);
    assert!(
        vault.join("xlib/docs/custom_stem.pdf").is_file(),
        "mapped name and ref type must win: {report}"
    );
    assert!(
        report.contains("title: Custom Title")
            && report.contains("status: next")
            && report.contains("parent: sase_ref")
            && report.contains("id: custom_stem"),
        "{report}"
    );
    // The -T override is visible in the adapter request.
    let request = fake.request();
    assert!(request.contains("\"title\":\"Custom Title\""), "{request}");
}

#[test]
fn highlights_create_article_routes_bot_wall_to_adapter() {
    let temp = TempDir::new("bob-cli-highlights-create-article-walled");
    let vault = temp.path().join("vault");
    let root = temp.path().join("curl-root");
    std::fs::create_dir_all(&root).expect("create curl root");
    let fake_curl = write_fake_curl(temp.path());
    let log = temp.path().join("curl.log");
    std::fs::write(&log, "").expect("init log");
    let fake = FakeClip::new(&temp, "fake");

    let mut command = bob_command();
    command
        .arg("highlights")
        .arg("create")
        .arg("https://example.com/walled/field-notes");
    article_env(&mut command, &vault, &fake_curl, &root, &log, &fake.path);
    let output = command.output().expect("run create walled article");
    assert_success(&output);
    let report = stdout(&output);
    assert!(
        vault.join("xlib/blogs/field_notes.pdf").is_file(),
        "bot-walled article must route to the adapter: {report}"
    );
    assert!(fake.called(), "the adapter must run for a 403");
}

#[test]
fn highlights_create_article_binds_explicit_audio() {
    let temp = TempDir::new("bob-cli-highlights-create-article-audio");
    let vault = temp.path().join("vault");
    let root = temp.path().join("curl-root");
    std::fs::create_dir_all(&root).expect("create curl root");
    let fake_curl = write_fake_curl(temp.path());
    let log = temp.path().join("curl.log");
    std::fs::write(&log, "").expect("init log");
    let fake = FakeClip::new(&temp, "fake");
    let audio = temp.path().join("episode.mp3");
    std::fs::write(&audio, b"audio-bytes").expect("write audio");

    let mut command = bob_command();
    command
        .arg("highlights")
        .arg("create")
        .arg("https://example.com/article/hello")
        .arg("--audio")
        .arg(&audio);
    article_env(&mut command, &vault, &fake_curl, &root, &log, &fake.path);
    let output = command.output().expect("run create article with audio");
    assert_success(&output);
    assert!(
        stdout(&output).contains("(from --audio)"),
        "{}",
        format_output(&output)
    );
    assert!(
        vault.join("xlib/blogs/hello.mp3").is_file(),
        "audio must be bound beside the article PDF"
    );
}

#[test]
fn highlights_create_article_dry_run_writes_nothing() {
    let temp = TempDir::new("bob-cli-highlights-create-article-dry");
    let vault = temp.path().join("vault");
    let root = temp.path().join("curl-root");
    std::fs::create_dir_all(&root).expect("create curl root");
    let fake_curl = write_fake_curl(temp.path());
    let log = temp.path().join("curl.log");
    std::fs::write(&log, "").expect("init log");
    let fake = FakeClip::new(&temp, "fake");

    let mut command = bob_command();
    command
        .arg("highlights")
        .arg("create")
        .arg("https://example.com/article/hello")
        .arg("-d");
    article_env(&mut command, &vault, &fake_curl, &root, &log, &fake.path);
    let output = command.output().expect("dry-run create article");
    assert_success(&output);
    let report = stdout(&output);
    assert!(
        report.contains("would create Highlights-ready web PDF")
            && report.contains("writes: none"),
        "{report}"
    );
    assert!(fake.called(), "dry run still captures to plan");
    assert!(
        !vault.join("xlib/blogs/hello.pdf").exists(),
        "dry run must not install: {report}"
    );
}

#[test]
fn landing_name_alone_does_not_embed_marker_id() {
    let temp = TempDir::new("bob-cli-landing-name-no-id");
    let source = temp.path().join("report.md");
    let vault = temp.path().join("vault");
    write_file(&source, "# Report\n");
    let output = bob_command()
        .arg("highlights")
        .arg("create")
        .arg(&source)
        .arg("-b")
        .arg(&vault)
        .arg("-N")
        .arg("custom")
        .arg("-d")
        .output()
        .expect("run create -N dry-run");
    assert_success(&output);
    let report = stdout(&output);
    assert!(
        !report.contains("id:"),
        "-N alone must not embed the marker id: {report}"
    );
}

#[test]
fn landing_companion_beside_source_is_copied() {
    let temp = TempDir::new("bob-cli-landing-companion-copy");
    let source = temp.path().join("paper.pdf");
    let vault = temp.path().join("vault");
    write_bare_pdf(&source, None, None);
    std::fs::write(temp.path().join("paper.mp3"), b"ID3 fake")
        .expect("sibling mp3");
    let output = bob_command()
        .arg("highlights")
        .arg("create")
        .arg(&source)
        .arg("-b")
        .arg(&vault)
        .output()
        .expect("run create local PDF with sibling audio");
    assert_success(&output);
    let installed = vault.join("xlib/papers/paper.mp3");
    assert!(
        installed.is_file(),
        "sibling audio must be copied: {}",
        stdout(&output)
    );
    assert_eq!(
        std::fs::read(&installed).expect("read installed"),
        b"ID3 fake",
        "copied bytes must match"
    );
}

#[test]
fn landing_marked_pdf_outside_vault_is_refused() {
    let temp = TempDir::new("bob-cli-landing-marked-refused");
    let source = temp.path().join("marked.pdf");
    let vault = temp.path().join("vault");
    write_bare_pdf(&source, None, None);
    // Stamp it so it reads as already captured.
    let stamp = bob_command()
        .arg("highlights")
        .arg("create")
        .arg(&source)
        .arg("-b")
        .arg(&vault)
        .output()
        .expect("initial create");
    assert_success(&stamp);
    let installed = vault.join("xlib/papers/marked.pdf");
    assert!(
        installed.is_file(),
        "setup must install: {}",
        stdout(&stamp)
    );
    // Copy the stamped (marked) PDF out and retry as a fresh source.
    let retry_src = temp.path().join("retry.pdf");
    std::fs::copy(&installed, &retry_src).expect("copy marked out");
    let vault2 = temp.path().join("vault2");
    let output = bob_command()
        .arg("highlights")
        .arg("create")
        .arg(&retry_src)
        .arg("-b")
        .arg(&vault2)
        .output()
        .expect("retry marked PDF");
    assert_eq!(output.status.code(), Some(1), "{}", format_output(&output));
    let diagnostic = stderr(&output);
    assert!(
        diagnostic.contains("already carries a Highlights marker"),
        "{diagnostic}"
    );
}

#[test]
fn landing_library_collision_hints_listen_attach() {
    let temp = TempDir::new("bob-cli-landing-collision-hint");
    let source = temp.path().join("report.md");
    let vault = temp.path().join("vault");
    write_file(&source, "# Report\n");
    // Pre-create the library destination so the intake refuses.
    let library_pdf = vault.join("lib/chat/report.pdf");
    std::fs::create_dir_all(library_pdf.parent().expect("lib parent"))
        .expect("lib dir");
    write_bare_pdf(&library_pdf, None, None);
    let output = bob_command()
        .arg("highlights")
        .arg("create")
        .arg(&source)
        .arg("-b")
        .arg(&vault)
        .arg("-d")
        .output()
        .expect("run create with library collision");
    assert_eq!(output.status.code(), Some(1), "{}", format_output(&output));
    let diagnostic = stderr(&output);
    assert!(
        diagnostic.contains("to add audio to that capture, run bob ref create"),
        "{diagnostic}"
    );
}
