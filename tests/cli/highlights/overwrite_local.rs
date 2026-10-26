//! Local Markdown, original local PDF, and same-day vault-note overwrite.

use crate::support::*;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

pub(crate) const FIXED_NOW: &str = "2026-10-16 12:00:00";
pub(crate) const STEM: &str = "report";
pub(crate) const TITLE: &str = "Report";

pub(crate) fn write_bare_pdf(path: &Path, title: Option<&str>) {
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
    if let Some(title) = title {
        let info_id = doc.add_object(dictionary! {
            "Title" => Object::string_literal(title),
        });
        doc.trailer.set("Info", info_id);
    }
    doc.save(path).expect("write bare PDF");
}

fn write_fake_pandoc(dir: &Path) -> PathBuf {
    let path = dir.join("fake-pandoc.sh");
    write_executable(
        &path,
        r#"#!/bin/sh
out=""
prev=""
for arg in "$@"; do
  if [ "$prev" = "-o" ]; then out="$arg"; fi
  prev="$arg"
done
cp "$FAKE_PANDOC_FIXTURE" "$out"
"#,
    );
    path
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
if [ -n "$FAKE_LISTEN_NO_AUDIO" ]; then
  exit "${FAKE_LISTEN_EXIT:-1}"
fi
if [ -n "$audio_path" ]; then
  printf 'ID3 fake episode' > "$audio_path"
fi
exit "${FAKE_LISTEN_EXIT:-0}"
"#,
    );
    bin
}

fn marker(status: &str) -> String {
    format!(
        concat!(
            "- status: {status}\n",
            "- parent: sase\n",
            "- title: {title}\n",
            "- id: {stem}\n",
            "- captured: 2026-01-01\n",
        ),
        status = status,
        title = TITLE,
        stem = STEM,
    )
}

pub(crate) struct OverwriteVault {
    pub(crate) temp: TempDir,
    pub(crate) vault: PathBuf,
    pub(crate) pdf: PathBuf,
}

pub(crate) fn filed_vault(
    prefix: &str,
    status: &str,
    audio: bool,
    prose: bool,
) -> OverwriteVault {
    let temp = TempDir::new(prefix);
    let (vault, _remote, _peer) = init_vault_sync_pair(&temp);
    write_area_note(&vault.join("sase.md"), "SASE");
    git_in(&vault, ["add", "sase.md"]);
    git_in(&vault, ["commit", "-q", "-m", "seed parent"]);
    git_in(&vault, ["push", "-q", "origin", "master"]);

    let pdf = vault.join(format!("lib/papers/{STEM}.pdf"));
    write_highlights_pdf(&pdf, &marker(status));
    if audio {
        write_file(&vault.join(format!("lib/papers/{STEM}.mp3")), "old audio");
    }
    let output = bob_command()
        .args(["ref", "sync"])
        .arg(&pdf)
        .env("BOB_DIR", &vault)
        .env("BOB_NOW", FIXED_NOW)
        .output()
        .expect("file the reference");
    assert_success(&output);
    if prose {
        let note = vault.join(format!("ref/papers/{STEM}.md"));
        let mut body = fs::read_to_string(&note).expect("read filed note");
        body.push_str("\nAuthored paragraph that must freeze.\n");
        fs::write(&note, body).expect("write authored prose");
    }
    git_in(&vault, ["add", "-A"]);
    git_in(&vault, ["commit", "-q", "-m", "file reference"]);
    git_in(&vault, ["push", "-q", "origin", "master"]);
    OverwriteVault { temp, vault, pdf }
}

pub(crate) fn commit_vault(vault: &Path, message: &str) {
    git_in(vault, ["add", "-A"]);
    git_in(vault, ["commit", "-q", "-m", message]);
    git_in(vault, ["push", "-q", "origin", "master"]);
}

/// Intake files its own commit, so a post-create stabilization must not
/// fail when the tree is already clean.
pub(crate) fn commit_if_dirty(vault: &Path, message: &str) {
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

pub(crate) fn reread_command(
    fixture: &OverwriteVault,
    source: &Path,
) -> Command {
    let mut command = bob_command();
    command
        .arg("ref")
        .arg("create")
        .arg(source)
        .arg("-P")
        .arg("sase")
        .arg("-f")
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

pub(crate) fn overwrite_command(
    fixture: &OverwriteVault,
    source: &Path,
) -> Command {
    let mut command = bob_command();
    command
        .arg("ref")
        .arg("create")
        .arg(source)
        .arg("-P")
        .arg("sase")
        .arg("-f")
        .arg("overwrite")
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

pub(crate) fn source_pdf(fixture: &OverwriteVault, title: &str) -> PathBuf {
    let path = fixture.temp.path().join(format!("{STEM}.pdf"));
    write_bare_pdf(&path, Some(title));
    path
}

fn source_markdown(
    fixture: &OverwriteVault,
    title: &str,
    body: &str,
) -> PathBuf {
    let path = fixture.temp.path().join(format!("{STEM}.md"));
    write_file(&path, &format!("# {title}\n\n{body}\n"));
    path
}

pub(crate) fn note_body(fixture: &OverwriteVault) -> String {
    fs::read_to_string(fixture.vault.join(format!("ref/papers/{STEM}.md")))
        .expect("read reference note")
}

fn pdf_bytes(fixture: &OverwriteVault) -> Vec<u8> {
    fs::read(&fixture.pdf).expect("read library PDF")
}

#[test]
fn local_pdf_overwrite_dry_run_writes_nothing() {
    let fixture =
        filed_vault("bob-cli-overwrite-pdf-dry", "ready", false, false);
    let before = snapshot_tree(&fixture.vault);
    let source = source_pdf(&fixture, TITLE);
    let output = overwrite_command(&fixture, &source)
        .arg("-d")
        .env("BOB_PANDOC_COMMAND", "/definitely/missing/pandoc")
        .output()
        .expect("dry-run local PDF overwrite");
    assert_success(&output);
    let report = stdout(&output);
    assert!(
        report.contains("ok would replace Highlights-ready PDF")
            && report.contains(&format!("overwrite: papers/{STEM}"))
            && report.contains("writes: none"),
        "dry-run must plan the overwrite:\n{report}"
    );
    assert_eq!(
        snapshot_tree(&fixture.vault),
        before,
        "dry-run overwrite must not write"
    );
}

#[test]
fn local_pdf_overwrite_replaces_in_place_without_archive() {
    let fixture =
        filed_vault("bob-cli-overwrite-pdf-happy", "ready", false, false);
    let before_pdf = pdf_bytes(&fixture);
    let source = source_pdf(&fixture, TITLE);
    let output = overwrite_command(&fixture, &source)
        .output()
        .expect("overwrite local PDF");
    assert_success(&output);
    let report = stdout(&output);
    assert!(
        report.contains("ok replaced Highlights-ready PDF")
            && report.contains(&format!("overwrite: papers/{STEM}"))
            && report.contains("earlier: nothing to archive")
            && report.contains("task: kept"),
        "success must report the in-place overwrite:\n{report}"
    );
    assert!(
        !fixture.vault.join("xlib/papers/report.pdf").exists(),
        "overwrite must not queue intake"
    );
    assert!(
        !fixture.vault.join("xlib/papers/report__v2.pdf").exists(),
        "overwrite must not allocate a reread"
    );
    let after_pdf = pdf_bytes(&fixture);
    assert_ne!(after_pdf, before_pdf, "library PDF bytes must change");
    let marker = bob_command()
        .args(["highlights", "marker"])
        .arg(&fixture.pdf)
        .env("BOB_DIR", &fixture.vault)
        .output()
        .expect("read marker");
    assert_success(&marker);
    let marker = stdout(&marker);
    assert!(
        marker.contains(&format!("- id: {STEM}\n"))
            && marker.contains("- captured: 2026-10-16\n")
            && marker.contains("- status: ready\n"),
        "replacement keeps the stem id and refreshes captured:\n{marker}"
    );
    let note = note_body(&fixture);
    assert!(
        note.contains("recaptured:"),
        "live note records recaptured:\n{note}"
    );
}

#[test]
fn local_pdf_overwrite_archives_authored_prose() {
    let fixture =
        filed_vault("bob-cli-overwrite-pdf-prose", "ready", false, true);
    let source = source_pdf(&fixture, TITLE);
    let output = overwrite_command(&fixture, &source)
        .output()
        .expect("overwrite with prose");
    assert_success(&output);
    let report = stdout(&output);
    assert!(
        report.contains("earlier: ref_history/papers/report__2026-10-16.md"),
        "prose must freeze into ref_history:\n{report}"
    );
    let archive = fs::read_to_string(
        fixture
            .vault
            .join("ref_history/papers/report__2026-10-16.md"),
    )
    .expect("read archive");
    assert!(
        archive.contains("Authored paragraph that must freeze."),
        "archive keeps the authored prose:\n{archive}"
    );
}

#[test]
fn local_pdf_overwrite_preserves_wip_and_explicit_ready() {
    let fixture = filed_vault("bob-cli-overwrite-pdf-wip", "wip", false, false);
    let source = source_pdf(&fixture, TITLE);
    let output = overwrite_command(&fixture, &source)
        .output()
        .expect("overwrite wip");
    assert_success(&output);
    let marker = stdout(
        &bob_command()
            .args(["highlights", "marker"])
            .arg(&fixture.pdf)
            .env("BOB_DIR", &fixture.vault)
            .output()
            .expect("read marker"),
    );
    assert!(
        marker.contains("- status: wip\n"),
        "no -s preserves wip:\n{marker}"
    );

    let fixture =
        filed_vault("bob-cli-overwrite-pdf-s-ready", "read", false, false);
    let source = source_pdf(&fixture, TITLE);
    let output = overwrite_command(&fixture, &source)
        .arg("-s")
        .arg("ready")
        .output()
        .expect("overwrite with -s ready");
    assert_success(&output);
    let report = stdout(&output);
    assert!(
        !report.contains("pass -s ready to reopen"),
        "explicit -s ready reopens:\n{report}"
    );
    let marker = stdout(
        &bob_command()
            .args(["highlights", "marker"])
            .arg(&fixture.pdf)
            .env("BOB_DIR", &fixture.vault)
            .output()
            .expect("read marker"),
    );
    assert!(
        marker.contains("- status: ready\n"),
        "explicit -s ready stamps ready:\n{marker}"
    );
}

#[test]
fn local_pdf_overwrite_preserves_blocked_overlay() {
    let fixture =
        filed_vault("bob-cli-overwrite-pdf-blocked", "ready", false, false);
    let parent = fixture.vault.join("sase.md");
    let body = fs::read_to_string(&parent).expect("parent");
    fs::write(&parent, body.replace("[ ]", "[?]")).expect("block overlay");
    commit_vault(&fixture.vault, "block overlay");
    let source = source_pdf(&fixture, TITLE);
    let output = overwrite_command(&fixture, &source)
        .output()
        .expect("overwrite blocked");
    assert_success(&output);
    let marker = stdout(
        &bob_command()
            .args(["highlights", "marker"])
            .arg(&fixture.pdf)
            .env("BOB_DIR", &fixture.vault)
            .output()
            .expect("read marker"),
    );
    assert!(
        marker.contains("- status: ready\n"),
        "blocked overlay keeps the live marker class:\n{marker}"
    );
    let parent =
        fs::read_to_string(fixture.vault.join("sase.md")).expect("parent");
    assert!(
        parent.contains("[?]"),
        "blocked checkbox overlay is preserved:\n{parent}"
    );
}

#[test]
fn local_pdf_overwrite_keeps_archived_closed_task() {
    let fixture =
        filed_vault("bob-cli-overwrite-pdf-archived", "read", false, false);
    let parent_path = fixture.vault.join("sase.md");
    let parent = fs::read_to_string(&parent_path).expect("parent");
    let mut kept = String::new();
    let mut moved = String::new();
    for line in parent.split_inclusive('\n') {
        if line.contains("^ref-report")
            || (!moved.is_empty() && line.starts_with([' ', '\t']))
        {
            moved.push_str(line);
        } else {
            kept.push_str(line);
        }
    }
    assert!(
        !moved.is_empty(),
        "expected a reading task to archive:\n{parent}"
    );
    fs::write(&parent_path, kept).expect("strip parent task");
    let done = fixture.vault.join("done/sase_done.md");
    fs::create_dir_all(done.parent().expect("done dir")).expect("done dir");
    write_file(
        &done,
        &format!(
            "---\nparent: \"[[sase]]\"\ntype: \"[[done]]\"\n---\n\n# Done\n\n{moved}"
        ),
    );
    commit_vault(&fixture.vault, "archive reading task");
    let restabilize = bob_command()
        .args(["ref", "sync"])
        .arg(&fixture.pdf)
        .env("BOB_DIR", &fixture.vault)
        .env("BOB_NOW", FIXED_NOW)
        .output()
        .expect("restabilize archived task");
    assert_success(&restabilize);
    commit_vault(&fixture.vault, "sync archived embed");
    let source = source_pdf(&fixture, TITLE);
    let output = overwrite_command(&fixture, &source)
        .output()
        .expect("overwrite archived");
    assert_success(&output);
    let report = stdout(&output);
    assert!(
        report.contains("pass -s ready to reopen"),
        "archived closed task stays closed:\n{report}"
    );
    let done = fs::read_to_string(fixture.vault.join("done/sase_done.md"))
        .expect("done note");
    assert!(
        done.contains("^ref-report"),
        "archived task stays in done/:\n{done}"
    );
}

#[test]
fn local_pdf_overwrite_selected_older_reading() {
    let fixture =
        filed_vault("bob-cli-overwrite-pdf-older", "ready", false, false);
    let reread_source = source_pdf(&fixture, TITLE);
    let reread = reread_command(&fixture, &reread_source)
        .output()
        .expect("reread to v2");
    assert_success(&reread);
    let v2 = fixture.vault.join("lib/papers/report__v2.pdf");
    assert!(
        v2.is_file(),
        "reread files the next reading:\n{}",
        stdout(&reread)
    );
    let v2_before = fs::read(&v2).expect("v2 pdf");
    let v1_before = pdf_bytes(&fixture);
    let overwrite_source = fixture.temp.path().join("older.pdf");
    write_bare_pdf(&overwrite_source, Some(TITLE));
    let output = overwrite_command(&fixture, &overwrite_source)
        .arg("-N")
        .arg(STEM)
        .output()
        .expect("overwrite older reading");
    assert_success(&output);
    let report = stdout(&output);
    assert!(
        report.contains(&format!("overwrite: papers/{STEM}"))
            && !report.contains("report__v2"),
        "explicit -N overwrites the older reading:\n{report}"
    );
    assert_ne!(
        pdf_bytes(&fixture),
        v1_before,
        "selected older PDF bytes must change"
    );
    assert_eq!(
        fs::read(&v2).expect("v2 after"),
        v2_before,
        "newer reading stays untouched"
    );
}

#[test]
fn local_pdf_overwrite_keeps_and_replaces_audio() {
    let fixture =
        filed_vault("bob-cli-overwrite-pdf-keep-audio", "ready", true, false);
    let source = source_pdf(&fixture, TITLE);
    let output = overwrite_command(&fixture, &source)
        .output()
        .expect("overwrite keeping audio");
    assert_success(&output);
    let report = stdout(&output);
    assert!(
        report.contains("audio: kept lib/papers/report.mp3"),
        "implicit discovery must not replace audio:\n{report}"
    );
    assert_eq!(
        fs::read(fixture.vault.join("lib/papers/report.mp3"))
            .expect("read audio"),
        b"old audio",
        "kept audio bytes"
    );

    let fixture =
        filed_vault("bob-cli-overwrite-pdf-no-audio", "ready", true, false);
    let source = source_pdf(&fixture, TITLE);
    let output = overwrite_command(&fixture, &source)
        .arg("--no-audio")
        .output()
        .expect("overwrite --no-audio");
    assert_success(&output);
    assert_eq!(
        fs::read(fixture.vault.join("lib/papers/report.mp3"))
            .expect("read audio"),
        b"old audio",
        "--no-audio preserves the companion"
    );

    let fixture = filed_vault(
        "bob-cli-overwrite-pdf-replace-audio",
        "ready",
        true,
        false,
    );
    let source = source_pdf(&fixture, TITLE);
    let replacement = fixture.temp.path().join("new.mp3");
    write_file(&replacement, "new audio");
    let output = overwrite_command(&fixture, &source)
        .arg("-a")
        .arg(&replacement)
        .output()
        .expect("overwrite with -a");
    assert_success(&output);
    let report = stdout(&output);
    assert!(
        report.contains("audio: replaced lib/papers/report.mp3"),
        "explicit -a replaces audio:\n{report}"
    );
    assert_eq!(
        fs::read(fixture.vault.join("lib/papers/report.mp3"))
            .expect("read audio"),
        b"new audio",
        "replaced audio bytes"
    );
}

#[test]
fn local_pdf_overwrite_listen_failure_leaves_bytes_unchanged() {
    let fixture =
        filed_vault("bob-cli-overwrite-pdf-listen-fail", "ready", true, false);
    let before = pdf_bytes(&fixture);
    let before_audio = fs::read(fixture.vault.join("lib/papers/report.mp3"))
        .expect("old audio");
    let source = source_pdf(&fixture, TITLE);
    let bin = write_fake_sase(fixture.temp.path());
    let log = fixture.temp.path().join("listen.log");
    let output = overwrite_command(&fixture, &source)
        .arg("-L")
        .env("PATH", path_with_prefix(&bin))
        .env(
            "BOB_HIGHLIGHTS_LISTEN_COMMAND",
            "sase listen render {target} -o {audio}",
        )
        .env("FAKE_LISTEN_LOG", &log)
        .env("FAKE_LISTEN_EXIT", "1")
        .output()
        .expect("overwrite listen failure");
    assert_ne!(output.status.code(), Some(0), "{}", format_output(&output));
    assert_eq!(pdf_bytes(&fixture), before, "failed listen leaves the PDF");
    assert_eq!(
        fs::read(fixture.vault.join("lib/papers/report.mp3")).expect("audio"),
        before_audio,
        "failed listen leaves the companion"
    );
}

#[test]
fn local_pdf_overwrite_listen_replaces_companion() {
    let fixture =
        filed_vault("bob-cli-overwrite-pdf-listen", "ready", true, false);
    let source = source_pdf(&fixture, TITLE);
    let bin = write_fake_sase(fixture.temp.path());
    let log = fixture.temp.path().join("listen.log");
    let output = overwrite_command(&fixture, &source)
        .arg("-L")
        .env("PATH", path_with_prefix(&bin))
        .env(
            "BOB_HIGHLIGHTS_LISTEN_COMMAND",
            "sase listen render {target} -o {audio}",
        )
        .env("FAKE_LISTEN_LOG", &log)
        .output()
        .expect("overwrite listen");
    assert_success(&output);
    let report = stdout(&output);
    assert!(
        report.contains("audio: replaced lib/papers/report.mp3"),
        "private -L replaces companion audio:\n{report}"
    );
    let audio = fs::read(fixture.vault.join("lib/papers/report.mp3"))
        .expect("replaced audio");
    assert_eq!(
        audio, b"ID3 fake episode",
        "listen writes the replacement episode"
    );
    let log = fs::read_to_string(&log).expect("listen log");
    assert!(
        log.contains("listen render"),
        "overwrite -L must invoke listen:\n{log}"
    );
}

#[test]
fn local_pdf_overwrite_changes_title() {
    let fixture =
        filed_vault("bob-cli-overwrite-pdf-title", "ready", false, false);
    let source = source_pdf(&fixture, TITLE);
    let output = overwrite_command(&fixture, &source)
        .arg("-T")
        .arg("A Better Report")
        .output()
        .expect("overwrite title");
    assert_success(&output);
    let report = stdout(&output);
    assert!(
        report.contains("title: A Better Report"),
        "success names the new title:\n{report}"
    );
    let parent =
        fs::read_to_string(fixture.vault.join("sase.md")).expect("parent");
    assert!(
        parent.contains("A Better Report"),
        "the reading-task alias updates:\n{parent}"
    );
}

#[test]
fn markdown_overwrite_uses_fake_pandoc_and_skips_it_on_dry_run() {
    let fixture = filed_vault("bob-cli-overwrite-md", "ready", false, false);
    let source = source_markdown(&fixture, TITLE, "Replacement body.");
    let fixture_pdf = fixture.temp.path().join("render-fixture.pdf");
    write_bare_pdf(&fixture_pdf, Some(TITLE));
    let pandoc = write_fake_pandoc(fixture.temp.path());
    let dry = overwrite_command(&fixture, &source)
        .arg("-d")
        .env("BOB_PANDOC_COMMAND", "/definitely/missing/pandoc")
        .output()
        .expect("markdown overwrite dry-run");
    assert_success(&dry);
    assert!(
        stdout(&dry).contains("writes: none"),
        "markdown dry-run skips pandoc:\n{}",
        stdout(&dry)
    );

    let output = overwrite_command(&fixture, &source)
        .env("BOB_PANDOC_COMMAND", &pandoc)
        .env("FAKE_PANDOC_FIXTURE", &fixture_pdf)
        .output()
        .expect("markdown overwrite");
    assert_success(&output);
    let report = stdout(&output);
    assert!(
        report.contains("ok replaced Highlights-ready PDF")
            && report.contains(&format!("overwrite: papers/{STEM}")),
        "markdown overwrite replaces in place:\n{report}"
    );
}

#[test]
fn vault_note_same_day_overwrite_plain_markdown() {
    let fixture = filed_vault("bob-cli-overwrite-note", "ready", false, false);
    write_file(
        &fixture.vault.join("zk/harbor.md"),
        "---\nparent: [[sase]]\n---\n# Harbor\n\nA source note.\n",
    );
    git_in(&fixture.vault, ["add", "zk/harbor.md"]);
    git_in(&fixture.vault, ["commit", "-q", "-m", "source note"]);
    git_in(&fixture.vault, ["push", "-q", "origin", "master"]);
    let pdf = fixture.vault.join("lib/notes/harbor_261016.pdf");
    write_highlights_pdf(
        &pdf,
        concat!(
            "- status: ready\n",
            "- parent: sase\n",
            "- title: Harbor\n",
            "- id: harbor_261016\n",
            "- source_note: zk/harbor\n",
            "- captured: 2026-10-16\n",
        ),
    );
    let sync = bob_command()
        .args(["ref", "sync"])
        .arg(&pdf)
        .env("BOB_DIR", &fixture.vault)
        .env("BOB_NOW", FIXED_NOW)
        .output()
        .expect("sync vault snapshot");
    assert_success(&sync);
    git_in(&fixture.vault, ["add", "-A"]);
    git_in(&fixture.vault, ["commit", "-q", "-m", "file snapshot"]);
    git_in(&fixture.vault, ["push", "-q", "origin", "master"]);

    let render = fixture.temp.path().join("render-fixture.pdf");
    write_bare_pdf(&render, Some("Harbor"));
    let pandoc = write_fake_pandoc(fixture.temp.path());
    let dry = bob_command()
        .args([
            "ref",
            "create",
            "[[zk/harbor]]",
            "-M",
            "-f",
            "overwrite",
            "-d",
        ])
        .arg("-b")
        .arg(&fixture.vault)
        .env("BOB_DIR", &fixture.vault)
        .env("BOB_NOW", FIXED_NOW)
        .env("NO_COLOR", "1")
        .env("HOME", fixture.temp.path().join("home"))
        .env("XDG_STATE_HOME", fixture.temp.path().join("state"))
        .env(
            "BOB_VAULT_SYNC_LOCK_FILE",
            fixture.temp.path().join("bob_sync.lock"),
        )
        .env(
            "BOB_VAULT_SYNC_STATE_FILE",
            fixture.temp.path().join("vault-sync.json"),
        )
        .env("BOB_PANDOC_COMMAND", "/definitely/missing/pandoc")
        .output()
        .expect("vault-note overwrite dry-run");
    assert_success(&dry);
    assert!(
        stdout(&dry).contains("overwrite: notes/harbor_261016")
            && stdout(&dry).contains("writes: none"),
        "same-day vault overwrite dry-run:\n{}",
        stdout(&dry)
    );

    let output = bob_command()
        .args(["ref", "create", "[[zk/harbor]]", "-M", "-f", "overwrite"])
        .arg("-b")
        .arg(&fixture.vault)
        .env("BOB_DIR", &fixture.vault)
        .env("BOB_NOW", FIXED_NOW)
        .env("NO_COLOR", "1")
        .env("HOME", fixture.temp.path().join("home"))
        .env("XDG_STATE_HOME", fixture.temp.path().join("state"))
        .env(
            "BOB_VAULT_SYNC_LOCK_FILE",
            fixture.temp.path().join("bob_sync.lock"),
        )
        .env(
            "BOB_VAULT_SYNC_STATE_FILE",
            fixture.temp.path().join("vault-sync.json"),
        )
        .env("BOB_PANDOC_COMMAND", &pandoc)
        .env("FAKE_PANDOC_FIXTURE", &render)
        .output()
        .expect("vault-note overwrite");
    assert_success(&output);
    assert!(
        stdout(&output).contains("ok replaced Highlights-ready PDF")
            && stdout(&output).contains("overwrite: notes/harbor_261016"),
        "same-day vault overwrite:\n{}",
        stdout(&output)
    );
}

#[test]
fn library_pdf_target_still_refuses_overwrite() {
    let fixture =
        filed_vault("bob-cli-overwrite-lib-target", "ready", false, false);
    let output = overwrite_command(&fixture, &fixture.pdf)
        .output()
        .expect("overwrite library PDF target");
    assert_eq!(
        output.status.code(),
        Some(1),
        "library PDF as TARGET must refuse:\n{}",
        format_output(&output)
    );
    let diagnostic = format!("{}{}", stdout(&output), stderr(&output));
    assert!(
        diagnostic.contains("already captured")
            || diagnostic.contains("original file"),
        "refusal must point at the original source:\n{diagnostic}"
    );
}

pub(crate) fn snapshot_tree(root: &Path) -> Vec<(String, Vec<u8>)> {
    fn walk(root: &Path, current: &Path, out: &mut Vec<(String, Vec<u8>)>) {
        for entry in fs::read_dir(current).expect("read snapshot") {
            let path = entry.expect("entry").path();
            if path.file_name().is_some_and(|name| name == ".git") {
                continue;
            }
            if path.is_dir() {
                walk(root, &path, out);
            } else {
                out.push((
                    path.strip_prefix(root)
                        .expect("relative")
                        .to_string_lossy()
                        .into_owned(),
                    fs::read(&path).expect("read file"),
                ));
            }
        }
    }
    let mut files = Vec::new();
    walk(root, root, &mut files);
    files.sort_by(|left, right| left.0.cmp(&right.0));
    files
}
