//! Agreement between the offline URL-routing verdicts and
//! `bob ref create` dedupe: every `in_library`, `in_intake`, or
//! `legacy` fixture reaches create's matching refusal or legacy
//! warning. The network fakes fail when invoked, proving dedupe runs
//! before any fetch.

use crate::support::*;

fn dead_network(command: &mut std::process::Command) {
    // If dedupe misses, create would invoke curl or the adapter and
    // fail with a different error than the expected refusal.
    command
        .env("BOB_HIGHLIGHTS_CURL", "/bin/false")
        .env("BOB_WEB_CLIP_ADAPTER", "/bin/false");
}

#[test]
fn url_routing_in_library_matches_create_refusal() {
    let temp = TempDir::new("bob-cli-url-routing-in-library");
    let vault = temp.path().join("vault");
    write_file(&vault.join("sase.md"), "---\ntype: [[area]]\n---\n");
    write_file(
        &vault.join("ref/papers/captured.md"),
        "---\ntitle: Captured Post\nstatus: ready\nsource_url: https://example.com/captured\nsource_pdf: lib/papers/captured.pdf\n---\n\n- [ ] ^ref\n",
    );

    let mut command = bob_command();
    command
        .arg("highlights")
        .arg("create")
        .arg("-P")
        .arg("sase")
        .arg("https://example.com/captured")
        .arg("-b")
        .arg(&vault);
    dead_network(&mut command);
    let output = command.output().expect("run create over captured URL");
    assert_eq!(output.status.code(), Some(1), "{}", format_output(&output));
    assert!(
        stderr(&output).contains("already captured as")
            && stderr(&output).contains("ref/papers/captured.md"),
        "expected the already-captured refusal:\n{}",
        format_output(&output)
    );
}

#[test]
fn url_routing_in_intake_matches_create_refusal() {
    let temp = TempDir::new("bob-cli-url-routing-in-intake");
    let vault = temp.path().join("vault");
    write_file(&vault.join("sase.md"), "---\ntype: [[area]]\n---\n");
    write_highlights_pdf(
        &vault.join("xlib/blogs/queued.pdf"),
        "- status: ready\n- parent: obsidian_ref\n- title: Queued Post\n- source_url: https://example.com/queued\n",
    );

    let mut command = bob_command();
    command
        .arg("highlights")
        .arg("create")
        .arg("-P")
        .arg("sase")
        .arg("https://example.com/queued")
        .arg("-b")
        .arg(&vault);
    dead_network(&mut command);
    let output = command.output().expect("run create over queued URL");
    assert_eq!(output.status.code(), Some(1), "{}", format_output(&output));
    assert!(
        stderr(&output).contains("already queued in")
            && stderr(&output).contains("xlib/blogs/queued.pdf"),
        "expected the already-queued refusal:\n{}",
        format_output(&output)
    );
}

#[test]
fn url_routing_legacy_matches_create_warning() {
    let temp = TempDir::new("bob-cli-url-routing-legacy");
    let vault = temp.path().join("vault");
    write_file(&vault.join("sase.md"), "---\ntype: [[area]]\n---\n");
    write_file(
        &vault.join("ref/ai/old.md"),
        "---\ntitle: Old\nsource_url: https://example.com/legacy-note\n---\n\n# Old\n",
    );

    // A dry run over a legacy-only note: no vault write happens, and
    // the warning proves dedupe ran before the fresh-copy fetch.
    let mut command = bob_command();
    command
        .arg("highlights")
        .arg("create")
        .arg("-P")
        .arg("sase")
        .arg("https://example.com/legacy-note")
        .arg("-b")
        .arg(&vault)
        .arg("--dry-run");
    dead_network(&mut command);
    let output = command
        .output()
        .expect("run create dry-run over legacy URL");
    // A legacy-only hit warns and captures a fresh copy, so the fetch
    // that follows is expected: the agreement point is that create
    // reaches the legacy warning for the legacy fixture.
    let diagnostic = stderr(&output);
    assert!(
        diagnostic.contains("already in the library as")
            && diagnostic.contains("ref/ai/old.md")
            && diagnostic.contains("capturing a fresh copy"),
        "expected the legacy warning:\n{}",
        format_output(&output)
    );
}

#[test]
fn url_routing_doctor_prints_routing_row() {
    let temp = TempDir::new("bob-cli-url-routing-doctor");
    let vault = temp.path().join("vault");
    std::fs::create_dir_all(vault.join("lib")).expect("create lib dir");
    std::fs::create_dir_all(vault.join("ref")).expect("create ref dir");

    let output = bob_command()
        .arg("ref")
        .arg("doctor")
        .arg("-b")
        .arg(&vault)
        .output()
        .expect("run ref doctor");
    let report = stdout(&output);
    assert!(
        report.contains("url routing: capture on")
            && report.contains("gkeep on")
            && report.contains("excludes google.com"),
        "expected the routing row:\n{}",
        format_output(&output)
    );
}
