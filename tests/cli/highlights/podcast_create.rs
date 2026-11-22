//! The podcast route in `bob ref create`: a Spotify episode becomes a
//! `podcasts` reference printed as an episode dossier.
//!
//! A fake curl serves the trimmed Spotify fixtures by URL shape (episode
//! page, embed, oEmbed, cover), and the fake adapter answers the offline
//! `render` op, keeping the `article.json` it was asked to print. No test
//! touches the network or a real browser.

use super::fake_clip::FakeClip;
use super::ingest::seed_synced_vault;
use crate::support::*;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const ID: &str = "4UgVFWty2I8hGAgSvT7pjC";
const CANONICAL: &str =
    "https://open.spotify.com/episode/4UgVFWty2I8hGAgSvT7pjC";
const TITLE: &str = "Software factories and code reviews with Harjot Gill";
const STEM: &str = "software_factories_and_code_reviews_with_harjot_gill";
const SHOW: &str = "The Pragmatic Engineer";
const PUBLISHED: &str = "2026-11-18";
/// The exact `source_url` one legacy Spotify ref stores.
const LEGACY_SOURCE: &str = "https://open.spotify.com/episode/4UgVFWty2I8hGAgSvT7pjC?si=wMZ6dnubS8ib7JhCQ-6vLQ&sci=spotify%3Acard-config%3A2A4FnHWC7nYa7BcALmtBOK";

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/spotify")
        .join(name)
}

/// A fake curl serving `$FAKE_CURL_ROOT/<surface>.{status,type,body}` by
/// URL shape (404 when unset) and logging every URL.
struct FakeSpotify {
    root: PathBuf,
    curl: PathBuf,
}

impl FakeSpotify {
    fn new(temp: &TempDir) -> Self {
        let root = temp.path().join("spotify");
        fs::create_dir_all(&root).expect("create fake spotify root");
        let curl = root.join("fake-curl.sh");
        write_executable(
            &curl,
            r#"#!/bin/sh
ROOT="$FAKE_CURL_ROOT"
dest=""
url=""
prev=""
for arg in "$@"; do
  if [ "$prev" = "-o" ]; then dest="$arg"; fi
  prev="$arg"
  url="$arg"
done
printf '%s\n' "$url" >> "$ROOT/requests.log"
case "$url" in
  *"/oembed?"*) surface=oembed ;;
  *"/embed/episode/"*) surface=embed ;;
  *"open.spotify.com/episode/"*) surface=page ;;
  *) surface=cover ;;
esac
status=404
type="text/html"
if [ -f "$ROOT/$surface.status" ]; then status="$(cat "$ROOT/$surface.status")"; fi
if [ -f "$ROOT/$surface.type" ]; then type="$(cat "$ROOT/$surface.type")"; fi
if [ -f "$ROOT/$surface.body" ]; then cp "$ROOT/$surface.body" "$dest"; else : > "$dest"; fi
printf '%s\n%s\n\n' "$status" "$type"
"#,
        );
        let fake = Self { root, curl };
        fake.serve_file(
            "page",
            200,
            "text/html",
            &fixture("episode_page.html"),
        );
        fake.serve_file(
            "embed",
            200,
            "text/html",
            &fixture("episode_embed.html"),
        );
        fake.serve_file(
            "oembed",
            200,
            "application/json",
            &fixture("episode_oembed.json"),
        );
        let mut jpeg = vec![0xFF, 0xD8, 0xFF, 0xE0, 0, 16, b'J', b'F'];
        jpeg.extend_from_slice(&[0; 24]);
        fake.serve("cover", 200, "image/jpeg", &jpeg);
        fake
    }

    fn serve(
        &self,
        surface: &str,
        status: u16,
        content_type: &str,
        body: &[u8],
    ) {
        fs::write(
            self.root.join(format!("{surface}.status")),
            status.to_string(),
        )
        .expect("write fake status");
        fs::write(self.root.join(format!("{surface}.type")), content_type)
            .expect("write fake type");
        fs::write(self.root.join(format!("{surface}.body")), body)
            .expect("write fake body");
    }

    fn serve_file(
        &self,
        surface: &str,
        status: u16,
        content_type: &str,
        path: &Path,
    ) {
        self.serve(
            surface,
            status,
            content_type,
            &fs::read(path).expect("read fixture"),
        );
    }

    fn requests(&self) -> Vec<String> {
        fs::read_to_string(self.root.join("requests.log"))
            .unwrap_or_default()
            .lines()
            .map(str::to_string)
            .collect()
    }
}

struct Episode {
    temp: TempDir,
    vault: PathBuf,
    spotify: FakeSpotify,
    adapter: FakeClip,
}

/// A plain vault (dry runs and refusals) or a synced one (real creates
/// that file through intake).
fn episode_fixture(prefix: &str, synced: bool) -> Episode {
    let temp = TempDir::new(prefix);
    let vault = if synced {
        seed_synced_vault(&temp).0
    } else {
        let vault = temp.path().join("vault");
        write_area_note(&vault.join("sase.md"), "SASE");
        vault
    };
    let spotify = FakeSpotify::new(&temp);
    let adapter = FakeClip::new(&temp, "adapter");
    Episode {
        temp,
        vault,
        spotify,
        adapter,
    }
}

impl Episode {
    fn command(&self, args: &[&str]) -> Command {
        let mut command = bob_command();
        command
            .arg("ref")
            .arg("create")
            .args(args)
            .arg("-b")
            .arg(&self.vault)
            .env("BOB_DIR", &self.vault)
            .env("NO_COLOR", "1")
            .env("BOB_HIGHLIGHTS_INGEST_ON_CREATE", "1")
            .env("BOB_REF_INTAKE_LOCK_WAIT_SECONDS", "0")
            .env(
                "BOB_VAULT_SYNC_LOCK_FILE",
                self.temp.path().join("bob_sync.lock"),
            )
            .env(
                "BOB_VAULT_SYNC_STATE_FILE",
                self.temp.path().join("vault-sync.json"),
            )
            .env("XDG_STATE_HOME", self.temp.path().join("state"))
            .env("HOME", self.temp.path().join("home"))
            .env("BOB_HIGHLIGHTS_CURL", &self.spotify.curl)
            .env("FAKE_CURL_ROOT", &self.spotify.root)
            .env("BOB_WEB_CLIP_ADAPTER", &self.adapter.path);
        command
    }

    fn run(&self, args: &[&str]) -> Output {
        self.command(args).output().expect("run bob ref create")
    }

    fn note(&self, route: &str) -> String {
        fs::read_to_string(self.vault.join(route))
            .unwrap_or_else(|error| panic!("read {route}: {error}"))
    }

    /// Point the fake render at a source view so the store is written.
    fn stage_source_view(&self) {
        let render = self.temp.path().join("render");
        fs::create_dir_all(&render).expect("create render dir");
        let source = render.join("source.html");
        fs::write(
            &source,
            format!(
                "<!DOCTYPE html><html><body><article><h1>{TITLE}</h1>\
                 <p>{SHOW}</p><h2>Show notes</h2>\
                 <p>Read the <a href=\"https://newsletter.pragmaticengineer.com/\">deepdives</a>.</p>\
                 <ul><li>3:17 How Harjot shipped</li></ul>\
                 <h2>Sponsors</h2><p>turbopuffer</p></article></body></html>"
            ),
        )
        .expect("write source view");
        let response = serde_json::json!({
            "protocol": 1,
            "ok": true,
            "op": "render",
            "pdf_bytes": 2400,
            "source_html": source.to_string_lossy(),
            "images": {"total": 1, "kept": 1, "skipped_small": 0, "failed": 0},
            "warnings": [],
        });
        self.adapter
            .respond_render(&serde_json::to_string(&response).expect("JSON"));
    }
}

fn warning_lines(output: &Output) -> Vec<String> {
    stderr(output)
        .lines()
        .filter(|line| line.starts_with("warning:"))
        .map(str::to_string)
        .collect()
}

fn pandoc_available() -> bool {
    Command::new("pandoc")
        .arg("--version")
        .output()
        .is_ok_and(|output| output.status.success())
}

#[test]
fn dry_run_reports_the_episode_without_rendering_or_writing() {
    let episode = episode_fixture("bob-cli-podcast-dry-run", false);
    let shared = format!("{CANONICAL}?si=abc&t=1235");
    let output = episode.run(&[&shared, "-P", "sase", "-d"]);
    assert_success(&output);
    let report = stdout(&output);
    for line in [
        "[dry-run] ok would create Highlights-ready podcast PDF".to_string(),
        format!("source: {CANONICAL} (Spotify episode)"),
        format!("title: {TITLE} (spotify page)"),
        format!(
            "author: {SHOW} (spotify page) · published: {PUBLISHED} (spotify page)"
        ),
        "duration: 1 h 26 min · chapters: 16 · cover: yes".to_string(),
        "audio: none".to_string(),
        format!("id: {STEM}"),
        format!("- source_url: {CANONICAL}"),
        "writes: none".to_string(),
    ] {
        assert!(
            report.lines().any(|candidate| candidate.trim() == line.trim()),
            "missing `{line}`:\n{}",
            format_output(&output)
        );
    }
    assert!(
        report.contains(&format!("xlib/podcasts/{STEM}.pdf")),
        "{report}"
    );
    assert!(
        warning_lines(&output).is_empty(),
        "{}",
        format_output(&output)
    );
    assert!(!episode.adapter.render_called(), "a dry run never renders");
    assert!(
        !episode.vault.join("xlib").exists(),
        "a dry run writes nothing"
    );
    assert!(
        !episode.vault.join("ref").exists(),
        "a dry run writes nothing"
    );
    // The page alone held every field: no embed or oEmbed fallback.
    let requests = episode.spotify.requests();
    assert_eq!(requests[0], CANONICAL, "{requests:?}");
    assert!(
        requests
            .iter()
            .all(|url| !url.contains("/embed/") && !url.contains("/oembed")),
        "{requests:?}"
    );
}

#[test]
fn create_files_a_podcasts_reference_from_the_dossier() {
    let episode = episode_fixture("bob-cli-podcast-create", true);
    episode.stage_source_view();
    let shared =
        format!("https://open.spotify.com/intl-de/episode/{ID}?si=x&t=1235");
    let output = episode.run(&[&shared, "-P", "sase"]);
    assert_success(&output);
    let report = stdout(&output);
    for line in [
        "ok created Highlights-ready podcast PDF".to_string(),
        format!("source: {CANONICAL} (Spotify episode)"),
        "audio: none".to_string(),
        format!("title: {TITLE}"),
        format!("author: {SHOW}"),
        format!("published: {PUBLISHED}"),
        "duration: 1 h 26 min".to_string(),
        format!("id: {STEM}"),
    ] {
        assert!(
            report.lines().any(|candidate| candidate.trim() == line),
            "missing `{line}`:\n{}",
            format_output(&output)
        );
    }
    assert!(
        report.lines().any(|line| line.starts_with(
            "capture: episode dossier · 16 chapters · show notes "
        ) && line.ends_with(" words")),
        "{report}"
    );
    assert!(
        report.lines().any(|line| line.starts_with("pages: ")),
        "{report}"
    );
    assert!(!report.contains("transcript"), "{report}");

    // The note is filed under podcasts/ with the canonical identity.
    let note = episode.note(&format!("ref/podcasts/{STEM}.md"));
    assert!(
        note.contains(&format!("source_url: \"{CANONICAL}\""))
            && note.contains(&format!("author: \"{SHOW}\""))
            && note.contains(&format!("published: {PUBLISHED}"))
            && note.contains(&format!("id: {STEM}")),
        "{note}"
    );
    assert!(
        episode
            .vault
            .join(format!("lib/podcasts/{STEM}.pdf"))
            .is_file(),
        "the dossier PDF lands in the library"
    );
    if pandoc_available() {
        assert!(
            note.lines().any(|line| line
                .starts_with("source_markdown: .bob/ref-sources/podcasts/")),
            "the source store pointer rides intake:\n{note}"
        );
    }

    // The adapter printed the Bob-built payload.
    let payload = episode.adapter.rendered_article();
    assert_eq!(payload["kind"], "podcast_episode", "{payload}");
    assert_eq!(payload["source_url"], CANONICAL, "{payload}");
    assert_eq!(payload["title"], TITLE, "{payload}");
    assert_eq!(payload["published"], PUBLISHED, "{payload}");
    let detail = &payload["episode"];
    assert_eq!(detail["show"], SHOW, "{payload}");
    assert_eq!(detail["shared_at_secs"], 1235, "{payload}");
    assert_eq!(detail["listen_url"], CANONICAL, "{payload}");
    assert_eq!(detail["cover"], "assets/cover.jpg", "{payload}");
    let chapters: usize = detail["blocks"]
        .as_array()
        .expect("blocks")
        .iter()
        .filter(|block| block["type"] == "chapters")
        .map(|block| block["chapters"].as_array().expect("chapters").len())
        .sum();
    assert_eq!(chapters, 16, "{payload}");
    assert!(
        detail["sponsors_html"]
            .as_str()
            .is_some_and(|html| html.contains("turbopuffer")),
        "{payload}"
    );
}

#[test]
fn a_legacy_share_token_ref_refuses_before_any_fetch() {
    let episode = episode_fixture("bob-cli-podcast-legacy", false);
    write_file(
        &episode.vault.join("ref/blogs/4ugvfwty2i8hgagsvt7pjc.md"),
        &format!(
            "---\ntitle: {TITLE}\nstatus: ready\nsource_url: \"{LEGACY_SOURCE}\"\nsource_pdf: lib/blogs/4ugvfwty2i8hgagsvt7pjc.pdf\n---\n\n- [ ] ^ref\n"
        ),
    );
    let output = episode.run(&[CANONICAL, "-P", "sase"]);
    assert_eq!(output.status.code(), Some(1), "{}", format_output(&output));
    assert!(
        stderr(&output).contains("already captured"),
        "{}",
        format_output(&output)
    );
    assert!(
        episode.spotify.requests().is_empty(),
        "no fetch before dedupe"
    );
    assert!(!episode.adapter.called(), "no render before dedupe");
}

#[test]
fn reread_files_a_second_reading_and_overwrite_replaces_in_place() {
    let episode = episode_fixture("bob-cli-podcast-recapture", true);
    let output = episode.run(&[CANONICAL, "-P", "sase"]);
    assert_success(&output);
    let first = format!("ref/podcasts/{STEM}.md");
    assert!(episode.vault.join(&first).is_file());

    // A bare -f reads the episode again as `<stem>__v2`.
    let output = episode.run(&[&format!("spotify:episode:{ID}"), "-f"]);
    assert_success(&output);
    let report = stdout(&output);
    assert!(report.contains("reread:"), "{}", format_output(&output));
    let second = format!("ref/podcasts/{STEM}__v2.md");
    let note = episode.note(&second);
    assert!(
        note.contains(&format!("source_url: \"{CANONICAL}\""))
            && note.contains(&format!("id: {STEM}__v2")),
        "{note}"
    );

    // -f overwrite replaces the selected reading in place: same note,
    // stem, id, task, and type; the new title is the override.
    let output = episode.run(&[
        CANONICAL,
        "-f",
        "overwrite",
        "-N",
        STEM,
        "-T",
        "Harjot Gill on software factories",
    ]);
    assert_success(&output);
    let report = stdout(&output);
    assert!(
        report.contains("replaced Highlights-ready PDF")
            && report.contains(&format!("overwrite: podcasts/{STEM}")),
        "{}",
        format_output(&output)
    );
    let note = episode.note(&first);
    assert!(
        note.contains("Harjot Gill on software factories")
            && note.contains(&format!("id: {STEM}\n"))
            && note.contains(&format!("source_url: \"{CANONICAL}\"")),
        "{note}"
    );
    assert!(!episode
        .vault
        .join(format!("ref/podcasts/{STEM}__v3.md"))
        .exists());
}

#[test]
fn overrides_win_over_spotify_metadata() {
    let episode = episode_fixture("bob-cli-podcast-overrides", false);
    let output = episode.run(&[
        CANONICAL,
        "-P",
        "sase",
        "-d",
        "-N",
        "harjot_factories",
        "-t",
        "talks",
        "-T",
        "Harjot on factories",
        "-A",
        "Harjot Gill",
        "-p",
        "2026-01-02",
    ]);
    assert_success(&output);
    let report = stdout(&output);
    assert!(
        report.contains("xlib/talks/harjot_factories.pdf")
            && report.contains("title: Harjot on factories (override)")
            && report.contains(
                "author: Harjot Gill (override) · published: 2026-01-02 (override)"
            )
            && report.contains("id: harjot_factories"),
        "{}",
        format_output(&output)
    );
}

#[test]
fn a_title_without_ascii_falls_back_to_the_episode_id_stem() {
    let episode = episode_fixture("bob-cli-podcast-non-ascii", false);
    let output =
        episode.run(&[CANONICAL, "-P", "sase", "-d", "-T", "ソフトウェア工場"]);
    assert_success(&output);
    let report = stdout(&output);
    assert!(
        report.contains(
            "xlib/podcasts/spotify_episode_4ugvfwty2i8hgagsvt7pjc.pdf"
        ) && !report.contains(&format!("/{ID}.pdf")),
        "{}",
        format_output(&output)
    );
}

#[test]
fn a_metadata_failure_writes_nothing() {
    let episode = episode_fixture("bob-cli-podcast-404", false);
    episode.spotify.serve("page", 404, "text/html", b"");
    let output = episode.run(&[CANONICAL, "-P", "sase"]);
    assert_eq!(output.status.code(), Some(1), "{}", format_output(&output));
    assert!(
        stderr(&output).contains(&format!(
            "error: Spotify has no public episode {ID} (HTTP 404)"
        )),
        "{}",
        format_output(&output)
    );
    assert!(!episode.adapter.called(), "nothing renders");
    assert!(!episode.vault.join("xlib").exists(), "nothing is written");
    assert!(!episode.vault.join("lib").exists(), "nothing is written");
    assert!(!episode.vault.join("ref").exists(), "nothing is written");

    // A page without a title anywhere fails closed with the update hint.
    let episode = episode_fixture("bob-cli-podcast-no-title", false);
    episode.spotify.serve_file(
        "page",
        200,
        "text/html",
        &fixture("drift_page_shell.html"),
    );
    episode
        .spotify
        .serve("embed", 200, "text/html", b"<html></html>");
    episode.spotify.serve("oembed", 404, "text/html", b"");
    let output = episode.run(&[CANONICAL, "-P", "sase"]);
    assert_eq!(output.status.code(), Some(1), "{}", format_output(&output));
    let err = stderr(&output);
    assert!(
        err.contains(&format!("error: Spotify returned no episode metadata for {ID}"))
            && err.contains("hint: retry later; if it persists, Spotify changed its page and bob needs an update"),
        "{}",
        format_output(&output)
    );
    assert!(!episode.vault.join("xlib").exists(), "nothing is written");
}

#[test]
fn a_cover_failure_prints_exactly_one_warning() {
    let episode = episode_fixture("bob-cli-podcast-no-cover", true);
    episode.stage_source_view();
    episode.spotify.serve("cover", 404, "text/html", b"");
    let output = episode.run(&[CANONICAL, "-P", "sase"]);
    assert_success(&output);
    // Without pandoc the source store is skipped with its own warning;
    // that is not a capture degradation.
    let warnings: Vec<String> = warning_lines(&output)
        .into_iter()
        .filter(|line| !line.contains("source store"))
        .collect();
    assert_eq!(warnings.len(), 1, "{}", format_output(&output));
    assert!(
        warnings[0].starts_with("warning: cover art missing: HTTP 404"),
        "{}",
        format_output(&output)
    );
    assert_eq!(
        episode.adapter.rendered_article()["episode"]["cover"],
        serde_json::Value::Null
    );
    assert!(episode
        .vault
        .join(format!("ref/podcasts/{STEM}.md"))
        .is_file());
}
