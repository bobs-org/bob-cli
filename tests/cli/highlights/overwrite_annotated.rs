//! Linux CLI refusals and dry-run preview for annotated overwrite.
//!
//! Real commands must refuse annotated targets before adapters or fetch and
//! leave vault bytes unchanged. Dry-run may preview the archive on any host.

use super::fake_clip::FakeClip;
use super::overwrite_local::{
    commit_vault, filed_vault, overwrite_command, snapshot_tree, source_pdf,
    OverwriteVault, FIXED_NOW, STEM, TITLE,
};
use super::overwrite_recovery;
use crate::support::*;
use std::fs;
use std::path::PathBuf;
use std::process::Output;

fn host_refusal_needle(highlights: u64) -> String {
    let count = if highlights == 1 {
        "1 highlight".to_string()
    } else {
        format!("{highlights} highlights")
    };
    format!(
        "this reference has {count}, which Highlights holds on the Mac; run the overwrite there"
    )
}

fn intent_path(fixture: &OverwriteVault) -> PathBuf {
    overwrite_recovery::intent_path(
        &fixture.temp.path().join("state"),
        &fixture.vault,
    )
}

type VaultSnapshot = (Vec<(String, Vec<u8>)>, String, String, Option<Vec<u8>>);

fn snapshot_state(fixture: &OverwriteVault) -> VaultSnapshot {
    (
        snapshot_tree(&fixture.vault),
        stdout(&git_in(&fixture.vault, ["rev-parse", "HEAD"]))
            .trim()
            .to_string(),
        stdout(&git_in(
            &fixture.vault,
            ["status", "--porcelain=v1", "--untracked-files=all"],
        )),
        fs::read(intent_path(fixture)).ok(),
    )
}

fn assert_unchanged(fixture: &OverwriteVault, before: &VaultSnapshot) {
    assert_eq!(snapshot_tree(&fixture.vault), before.0);
    assert_eq!(
        stdout(&git_in(&fixture.vault, ["rev-parse", "HEAD"])).trim(),
        before.1
    );
    assert_eq!(
        stdout(&git_in(
            &fixture.vault,
            ["status", "--porcelain=v1", "--untracked-files=all"]
        )),
        before.2
    );
    assert_eq!(fs::read(intent_path(fixture)).ok(), before.3);
}

fn missing_render_env(command: &mut std::process::Command) {
    command
        .env("BOB_PANDOC_COMMAND", "/definitely/missing/pandoc")
        .env("BOB_WEB_CLIP_ADAPTER", "/definitely/missing/adapter");
}

fn sync_pdf(fixture: &OverwriteVault) -> Output {
    bob_command()
        .args(["ref", "sync"])
        .arg(&fixture.pdf)
        .env("BOB_DIR", &fixture.vault)
        .env("BOB_NOW", FIXED_NOW)
        .env("NO_COLOR", "1")
        .env("HOME", fixture.temp.path().join("home"))
        .env("XDG_STATE_HOME", fixture.temp.path().join("state"))
        .env("XDG_CACHE_HOME", fixture.temp.path().join("cache"))
        .output()
        .expect("sync planted annotation")
}

fn plant_text_highlight(fixture: &OverwriteVault) {
    write_file(
        &fixture.vault.join(format!("lib/papers/{STEM}.md")),
        "## Page 1\n\n> Stable quote for identity.\n",
    );
    assert_success(&sync_pdf(fixture));
    commit_vault(&fixture.vault, "plant text highlight");
}

fn highlights_on_note(fixture: &OverwriteVault) -> u64 {
    fs::read_to_string(fixture.vault.join(format!("ref/papers/{STEM}.md")))
        .ok()
        .and_then(|contents| {
            contents.lines().find_map(|line| {
                line.strip_prefix("highlights_count:")
                    .and_then(|value| value.trim().parse().ok())
            })
        })
        .unwrap_or(0)
}

fn first_highlight_id(fixture: &OverwriteVault) -> String {
    fs::read_to_string(fixture.vault.join(format!("ref/papers/{STEM}.md")))
        .expect("annotated note")
        .lines()
        .flat_map(str::split_whitespace)
        .find_map(|token| token.strip_prefix('^'))
        .filter(|block_id| block_id.starts_with("h-"))
        .expect("highlight anchor")
        .to_string()
}

fn expected_annotated_message(output: &Output) -> String {
    let diagnostic = stderr(output);
    #[cfg(target_os = "macos")]
    {
        if diagnostic.contains(
            "annotated overwrite requires highlights.coordinate_pdf_writes: true",
        ) {
            return diagnostic;
        }
    }
    diagnostic
}

pub(super) fn assert_annotated_host_refusal(
    output: &Output,
    highlights: u64,
    label: &str,
) {
    assert_eq!(
        output.status.code(),
        Some(2),
        "{label} must refuse:\n{}",
        format_output(output)
    );
    let diagnostic = expected_annotated_message(output);
    assert!(
        diagnostic.contains("annotated_reference"),
        "{label} must expose annotated_reference:\n{diagnostic}"
    );
    #[cfg(target_os = "macos")]
    {
        assert!(
            diagnostic.contains(
                "annotated overwrite requires highlights.coordinate_pdf_writes: true"
            ) || diagnostic.contains(&host_refusal_needle(highlights)),
            "Mac refusal must name the key or the host gate:\n{diagnostic}"
        );
    }
    #[cfg(not(target_os = "macos"))]
    {
        assert!(
            diagnostic.contains(&host_refusal_needle(highlights))
                && diagnostic.contains("-f starts a new reading from any host"),
            "{label} Linux refusal must name the Mac host gate:\n{diagnostic}"
        );
    }
}

fn refuse_overwrite(fixture: &OverwriteVault) -> Output {
    let mut command = overwrite_command(fixture, &source_pdf(fixture, TITLE));
    missing_render_env(&mut command);
    command.output().expect("run annotated overwrite")
}

#[test]
fn linux_refuses_each_annotation_kind_before_render_and_leaves_vault() {
    for kind in [
        "text-highlight",
        "image-sidecar",
        "assets-only",
        "inbound-link",
    ] {
        let fixture = filed_vault(
            &format!("bob-cli-overwrite-ann-{kind}"),
            "ready",
            false,
            false,
        );
        match kind {
            "text-highlight" => plant_text_highlight(&fixture),
            "image-sidecar" => {
                let bundle =
                    fixture.vault.join(format!("lib/papers/{STEM}.textbundle"));
                fs::create_dir_all(bundle.join("assets"))
                    .expect("textbundle assets");
                write_file(
                    &bundle.join("text.md"),
                    "## Page 1\n\n![Figure](assets/figure.png)\n",
                );
                fs::write(bundle.join("assets/figure.png"), b"png-bytes")
                    .expect("image bytes");
                assert_success(&sync_pdf(&fixture));
                commit_vault(&fixture.vault, "plant image sidecar");
            }
            "assets-only" => {
                let assets =
                    fixture.vault.join(format!("ref/papers/{STEM}.assets"));
                fs::create_dir_all(&assets).expect("assets dir");
                fs::write(assets.join("clip.png"), b"asset-bytes")
                    .expect("asset");
                commit_vault(&fixture.vault, "plant assets");
            }
            "inbound-link" => {
                write_file(
                    &fixture.vault.join("zk/pointer.md"),
                    &format!(
                        "# Pointer\n\nSee [[ref/papers/{STEM}#^h-test]].\n"
                    ),
                );
                commit_vault(&fixture.vault, "plant inbound highlight link");
            }
            _ => unreachable!(),
        }
        let before = snapshot_state(&fixture);
        let output = refuse_overwrite(&fixture);
        assert_annotated_host_refusal(
            &output,
            highlights_on_note(&fixture),
            kind,
        );
        assert_unchanged(&fixture, &before);
    }
}

#[test]
#[cfg(not(target_os = "macos"))]
fn coordination_key_on_still_refuses_annotated_overwrite_off_macos() {
    let fixture =
        filed_vault("bob-cli-overwrite-ann-key-on", "ready", false, false);
    plant_text_highlight(&fixture);
    let config = fixture.temp.path().join("config-on.yml");
    write_file(&config, "highlights:\n  coordinate_pdf_writes: true\n");
    let before = snapshot_state(&fixture);
    let mut command = overwrite_command(&fixture, &source_pdf(&fixture, TITLE));
    missing_render_env(&mut command);
    command.env("BOB_CONFIG_FILE", &config);
    let output = command.output().expect("key-on annotated overwrite");
    assert_annotated_host_refusal(&output, 1, "key-on");
    assert_unchanged(&fixture, &before);
}

#[test]
fn dry_run_previews_annotated_archive_and_writes_nothing() {
    let fixture =
        filed_vault("bob-cli-overwrite-ann-dry", "ready", false, false);
    plant_text_highlight(&fixture);
    let highlight_id = first_highlight_id(&fixture);
    write_file(
        &fixture.vault.join("zk/report.md"),
        "# Unrelated report note\n",
    );
    write_file(
        &fixture.vault.join("zk/pointer.md"),
        &format!(
            "# Pointer\n\nSee [[ref/papers/{STEM}#^{highlight_id}]].\n\nSee [[ref/papers/{STEM}#^h-missing]].\n\nSee [[report#^{highlight_id}]].\n"
        ),
    );
    commit_vault(&fixture.vault, "inbound highlight link");
    let before = snapshot_state(&fixture);
    let mut command = overwrite_command(&fixture, &source_pdf(&fixture, TITLE));
    missing_render_env(&mut command);
    command.arg("-d");
    let output = command.output().expect("annotated dry-run");
    assert_success(&output);
    let report = stdout(&output);
    assert!(
        report.contains("ok would replace Highlights-ready PDF")
            && report.contains("writes: none")
            && report.contains(&format!(
                "earlier: ref_history/papers/{STEM}__2026-10-16.md"
            ))
            && report.contains("1 highlight")
            && report.contains("1 highlight link")
            && report.contains("annotated PDF retained: ref_history/papers/")
            && report.contains(&format!("{STEM}__2026-10-16__annotated.pdf")),
        "dry-run must preview the annotated archive:\n{report}"
    );
    assert!(
        report.contains(&format!(
            "warning: dangling highlight link left unchanged in zk/pointer.md: ref/papers/{STEM}#^h-missing"
        )),
        "dangling links are named and left unchanged:\n{report}"
    );
    assert!(
        report.contains(&format!(
            "warning: ambiguous highlight link left unresolved in zk/pointer.md: report#^{highlight_id}"
        )),
        "ambiguous bare links are named and left unresolved:\n{report}"
    );
    assert_unchanged(&fixture, &before);
}

#[test]
fn dirty_inbound_highlight_file_wins_over_annotated_gate() {
    let fixture =
        filed_vault("bob-cli-overwrite-ann-dirty-in", "ready", false, false);
    plant_text_highlight(&fixture);
    let highlight_id = first_highlight_id(&fixture);
    write_file(
        &fixture.vault.join("zk/pointer.md"),
        &format!("# Pointer\n\nSee [[ref/papers/{STEM}#^{highlight_id}]].\n"),
    );
    commit_vault(&fixture.vault, "inbound highlight link");
    let pointer = fixture.vault.join("zk/pointer.md");
    let mut body = fs::read_to_string(&pointer).expect("pointer");
    body.push_str("dirty inbound edit\n");
    fs::write(&pointer, body).expect("dirty inbound");
    let before = snapshot_state(&fixture);
    let output = refuse_overwrite(&fixture);
    assert_eq!(
        output.status.code(),
        Some(2),
        "dirty inbound must refuse:\n{}",
        format_output(&output)
    );
    assert!(
        stderr(&output).contains("dirty_paths")
            && stderr(&output).contains("zk/pointer.md"),
        "dirty inbound must win over annotated_reference:\n{}",
        format_output(&output)
    );
    assert_unchanged(&fixture, &before);
}

#[test]
fn open_annotation_task_in_highlights_region_refuses_before_host_gate() {
    let fixture =
        filed_vault("bob-cli-overwrite-ann-open-h", "ready", false, false);
    let note = fixture.vault.join(format!("ref/papers/{STEM}.md"));
    let mut body = fs::read_to_string(&note).expect("note");
    body.push_str(
        "\n<!-- highlights:begin -->\n- [ ] leftover open [h:: abc] #task\n<!-- highlights:end -->\n",
    );
    fs::write(&note, body).expect("open annotation task");
    commit_vault(&fixture.vault, "open annotation task in highlights");
    let before = snapshot_state(&fixture);
    let output = refuse_overwrite(&fixture);
    assert_eq!(
        output.status.code(),
        Some(2),
        "open annotation task must refuse:\n{}",
        format_output(&output)
    );
    assert!(
        stderr(&output).contains("open_note_tasks"),
        "open annotation tasks must win over annotated_reference:\n{}",
        format_output(&output)
    );
    assert_unchanged(&fixture, &before);
}

#[test]
fn explicit_output_flag_still_wins_on_annotated_targets() {
    let fixture =
        filed_vault("bob-cli-overwrite-ann-dash-o", "ready", false, false);
    plant_text_highlight(&fixture);
    let before = snapshot_state(&fixture);
    let mut command = overwrite_command(&fixture, &source_pdf(&fixture, TITLE));
    missing_render_env(&mut command);
    command.arg("-o").arg(fixture.temp.path().join("out.pdf"));
    let output = command.output().expect("annotated -o");
    assert_eq!(
        output.status.code(),
        Some(2),
        "-o must refuse:\n{}",
        format_output(&output)
    );
    assert!(
        stderr(&output)
            .contains("-o cannot select a path for an in-place overwrite")
            && stderr(&output).contains("drop -o"),
        "-o must stay the more-specific refusal:\n{}",
        format_output(&output)
    );
    assert_unchanged(&fixture, &before);
}

#[test]
fn exact_set_collection_refuses_annotated_target_before_adapter() {
    let fixture =
        filed_vault("bob-cli-overwrite-ann-coll", "ready", false, false);
    let pdf = &fixture.pdf;
    // Re-stamp the filed PDF as a two-URL collection so exact-set
    // overwrite selects this member without a second capture.
    let marker = format!(
        concat!(
            "- status: ready\n",
            "- parent: sase\n",
            "- title: {title}\n",
            "- id: {stem}\n",
            "- source_url: [https://example.com/a, https://example.com/b]\n",
            "- captured: 2026-01-01\n",
        ),
        title = TITLE,
        stem = STEM,
    );
    write_highlights_pdf(pdf, &marker);
    assert_success(&sync_pdf(&fixture));
    let assets = fixture.vault.join(format!("ref/papers/{STEM}.assets"));
    fs::create_dir_all(&assets).expect("assets");
    fs::write(assets.join("clip.png"), b"collection-asset").expect("asset");
    commit_vault(&fixture.vault, "file annotated collection");

    let fake = FakeClip::new(&fixture.temp, "adapter");
    let curl_log = fixture.temp.path().join("curl.log");
    fs::write(&curl_log, "").expect("curl log");
    let before = snapshot_state(&fixture);
    let output = bob_command()
        .args([
            "ref",
            "create",
            "https://example.com/b",
            "https://example.com/a",
            "-P",
            "sase",
            "-f",
            "overwrite",
            "-b",
        ])
        .arg(&fixture.vault)
        .env("BOB_DIR", &fixture.vault)
        .env("BOB_NOW", FIXED_NOW)
        .env("NO_COLOR", "1")
        .env("HOME", fixture.temp.path().join("home"))
        .env("XDG_STATE_HOME", fixture.temp.path().join("state"))
        .env("XDG_CACHE_HOME", fixture.temp.path().join("cache"))
        .env("BOB_WEB_CLIP_ADAPTER", &fake.path)
        .env("BOB_HIGHLIGHTS_CURL", "/definitely/missing/curl")
        .env("FAKE_CURL_LOG", &curl_log)
        .env("BOB_HIGHLIGHTS_INGEST_ON_CREATE", "1")
        .output()
        .expect("annotated collection overwrite");
    assert_annotated_host_refusal(&output, 0, "collection");
    assert!(!fake.called(), "collection refusal must precede adapters");
    assert!(
        fs::read_to_string(&curl_log).expect("curl log").is_empty(),
        "collection refusal must precede fetch"
    );
    assert_unchanged(&fixture, &before);
}
