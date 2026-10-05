//! Highlights create tests.

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
                && report.contains("next: bob highlights sync")
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
            && report.contains("next: bob highlights scan")
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
            && report.contains("next: bob highlights scan"),
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
            && report.contains("next: bob highlights sync")
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
            && report.contains("next: bob highlights scan"),
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
