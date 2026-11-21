//! Spotify link identity in `bob ref create` and `bob ref find`: teaching
//! refusals (exit 2) for anything but one episode and for `-L`/`-H`,
//! canonical dedupe against legacy share-token records, `spotify:` URIs,
//! and short links resolved through a fake-curl redirect. The network
//! fakes fail when invoked unless a test installs the short-link curl,
//! proving every refusal and dedupe hit happens before any capture.

use crate::support::*;
use std::path::{Path, PathBuf};
use std::process::Output;

const ID: &str = "4UgVFWty2I8hGAgSvT7pjC";
const CANONICAL: &str =
    "https://open.spotify.com/episode/4UgVFWty2I8hGAgSvT7pjC";
/// The exact `source_url` one legacy Spotify ref stores.
const LEGACY_SOURCE: &str = "https://open.spotify.com/episode/4UgVFWty2I8hGAgSvT7pjC?si=wMZ6dnubS8ib7JhCQ-6vLQ&sci=spotify%3Acard-config%3A2A4FnHWC7nYa7BcALmtBOK";
const LEGACY_NOTE: &str = "ref/blogs/4ugvfwty2i8hgagsvt7pjc.md";

fn vault_with_area(temp: &TempDir) -> PathBuf {
    let vault = temp.path().join("vault");
    write_file(&vault.join("sase.md"), "---\ntype: [[area]]\n---\n");
    vault
}

fn write_legacy_ref(vault: &Path) {
    write_file(
        &vault.join(LEGACY_NOTE),
        &format!(
            "---\ntitle: Software factories and code reviews with Harjot Gill\nstatus: ready\nsource_url: \"{LEGACY_SOURCE}\"\nsource_pdf: lib/blogs/4ugvfwty2i8hgagsvt7pjc.pdf\n---\n\n- [ ] ^ref\n"
        ),
    );
}

fn create(vault: &Path, args: &[&str], curl: Option<&Path>) -> Output {
    let mut command = bob_command();
    command
        .arg("ref")
        .arg("create")
        .args(args)
        .arg("-b")
        .arg(vault);
    command
        .env(
            "BOB_HIGHLIGHTS_CURL",
            curl.unwrap_or(Path::new("/bin/false")),
        )
        .env("BOB_WEB_CLIP_ADAPTER", "/bin/false");
    command.output().expect("run bob ref create")
}

fn assert_refusal(output: &Output, code: i32, message: &str, hint: &str) {
    assert_eq!(
        output.status.code(),
        Some(code),
        "{}",
        format_output(output)
    );
    let err = stderr(output);
    assert!(
        err.contains(&format!("error: {message}")),
        "expected `error: {message}`:\n{}",
        format_output(output)
    );
    assert!(
        err.contains(&format!("hint: {hint}")),
        "expected `hint: {hint}`:\n{}",
        format_output(output)
    );
}

/// Fake curl: `spotify.link/<slug>` redirects per slug, Spotify pages
/// serve HTML, everything else 404s.
fn write_short_link_curl(dir: &Path) -> PathBuf {
    let path = dir.join("fake-curl.sh");
    write_executable(
        &path,
        &format!(
            r#"#!/bin/sh
dest=""
prev=""
for arg in "$@"; do
  if [ "$prev" = "-o" ]; then dest="$arg"; fi
  prev="$arg"
  url="$arg"
done
: > "$dest"
case "$url" in
  *"spotify.link/episode"*)
    printf '307\ntext/html\nhttps://open.spotify.com/episode/{ID}?si=abc&t=42\n'
    ;;
  *"spotify.link/show"*)
    printf '302\ntext/html\nhttps://open.spotify.com/show/{ID}?si=abc\n'
    ;;
  *"open.spotify.com"*)
    printf '<html>page</html>' > "$dest"
    printf '200\ntext/html; charset=utf-8\n\n'
    ;;
  *)
    printf '404\ntext/html\n\n'
    ;;
esac
"#
        ),
    );
    path
}

#[test]
fn create_refuses_everything_but_one_episode_with_teaching_hints() {
    let temp = TempDir::new("bob-cli-spotify-refusals");
    let vault = vault_with_area(&temp);
    let episode = format!("https://open.spotify.com/episode/{ID}?si=x");
    let show = format!("https://open.spotify.com/show/{ID}");
    let track = format!("https://open.spotify.com/track/{ID}");
    let album = format!("https://open.spotify.com/intl-de/album/{ID}");
    let playlist = format!("https://open.spotify.com/playlist/{ID}");
    let artist = format!("https://open.spotify.com/artist/{ID}");
    let audiobook = format!("https://open.spotify.com/audiobook/{ID}");
    let chapter = format!("https://open.spotify.com/chapter/{ID}");
    let show_uri = format!("spotify:show:{ID}");
    let concert = format!("https://open.spotify.com/concert/{ID}");
    let short_id = format!("https://open.spotify.com/episode/{}", &ID[..21]);
    let episode_uri = format!("spotify:episode:{ID}");
    const SHOW_HINT: &str = "capture one episode (⋯ → Share → Copy link); follow the show as a record in your podcasts album (docs/records.md)";
    const EPISODES_ONLY: &str = "bob files Spotify podcast episodes only";
    const SHARE_EPISODE: &str =
        "share the episode link (open.spotify.com/episode/…)";
    let rows: Vec<(Vec<&str>, String, &str)> = vec![
        (
            vec![&show, "-P", "sase"],
            format!("Spotify show links are not references: {show}"),
            SHOW_HINT,
        ),
        (
            vec![&show_uri, "-P", "sase"],
            format!("Spotify show links are not references: {show_uri}"),
            SHOW_HINT,
        ),
        (
            vec![&track, "-P", "sase"],
            format!("Spotify music links are not references: {track}"),
            EPISODES_ONLY,
        ),
        (
            vec![&album, "-P", "sase"],
            format!("Spotify music links are not references: {album}"),
            EPISODES_ONLY,
        ),
        (
            vec![&playlist, "-P", "sase"],
            format!("Spotify music links are not references: {playlist}"),
            EPISODES_ONLY,
        ),
        (
            vec![&artist, "-P", "sase"],
            format!("Spotify music links are not references: {artist}"),
            EPISODES_ONLY,
        ),
        (
            vec![&audiobook, "-P", "sase"],
            format!("Spotify audiobooks are not references: {audiobook}"),
            EPISODES_ONLY,
        ),
        (
            vec![&chapter, "-P", "sase"],
            format!("Spotify audiobooks are not references: {chapter}"),
            EPISODES_ONLY,
        ),
        (
            vec!["https://open.spotify.com/user/someone", "-P", "sase"],
            "not a Spotify episode link: https://open.spotify.com/user/someone"
                .to_string(),
            SHARE_EPISODE,
        ),
        (
            vec![&concert, "-P", "sase"],
            format!("not a Spotify episode link: {concert}"),
            SHARE_EPISODE,
        ),
        (
            vec!["https://open.spotify.com/search/podcasts", "-P", "sase"],
            "not a Spotify episode link: https://open.spotify.com/search/podcasts"
                .to_string(),
            SHARE_EPISODE,
        ),
        (
            vec![&short_id, "-P", "sase"],
            format!("not a Spotify episode link: {short_id}"),
            SHARE_EPISODE,
        ),
        (
            vec![&episode, "-L", "-P", "sase"],
            "--listen does not apply to Spotify episodes: the episode is already audio"
                .to_string(),
            "drop -L; pass --audio FILE to bind audio you already have",
        ),
        (
            vec![&episode, "-H", "saved.html", "-P", "sase"],
            "--html does not apply to Spotify links: bob reads episodes from Spotify's public episode metadata"
                .to_string(),
            "drop -H",
        ),
        (
            vec![&episode_uri, "-H", "-", "-P", "sase"],
            "--html does not apply to Spotify links: bob reads episodes from Spotify's public episode metadata"
                .to_string(),
            "drop -H",
        ),
        (
            vec!["https://example.com/a", &episode, "-P", "sase"],
            format!("Spotify episodes are captured one at a time: {episode}"),
            "run bob ref create once per episode",
        ),
    ];
    for (args, message, hint) in rows {
        let mut with_dry = args.clone();
        with_dry.push("-d");
        let output = create(&vault, &with_dry, None);
        assert_refusal(&output, 2, &message, hint);
    }
}

#[test]
fn create_dedupes_every_spelling_against_a_legacy_share_token_record() {
    let temp = TempDir::new("bob-cli-spotify-legacy-dedupe");
    let vault = vault_with_area(&temp);
    write_legacy_ref(&vault);
    let curl_dir = temp.path().join("curl");
    std::fs::create_dir_all(&curl_dir).expect("create curl dir");
    let curl = write_short_link_curl(&curl_dir);
    for target in [
        CANONICAL.to_string(),
        format!("https://open.spotify.com/intl-de/embed/episode/{ID}/?t=30"),
        format!("spotify:episode:{ID}"),
        // Resolved through the fake redirect before dedupe.
        "https://spotify.link/episode".to_string(),
    ] {
        let output = create(&vault, &[&target], Some(&curl));
        assert_eq!(output.status.code(), Some(1), "{}", format_output(&output));
        assert!(
            stderr(&output).contains("already captured")
                && stderr(&output).contains(LEGACY_NOTE),
            "{target}: expected the already-captured refusal:\n{}",
            format_output(&output)
        );
    }
    // A different episode is not the same reference: it reaches the
    // missing-parent error instead of a dedupe refusal.
    let output = create(
        &vault,
        &["https://open.spotify.com/episode/5ygCxO9NxH38Ns7ghTBDDm"],
        None,
    );
    assert_eq!(output.status.code(), Some(2), "{}", format_output(&output));
    assert!(
        stderr(&output).contains("--parent is required")
            && !stderr(&output).contains("already captured"),
        "{}",
        format_output(&output)
    );
}

#[test]
fn create_refuses_short_links_by_what_they_name() {
    let temp = TempDir::new("bob-cli-spotify-short-links");
    let vault = vault_with_area(&temp);
    let curl_dir = temp.path().join("curl");
    std::fs::create_dir_all(&curl_dir).expect("create curl dir");
    let curl = write_short_link_curl(&curl_dir);

    let output = create(
        &vault,
        &["https://spotify.link/show", "-P", "sase", "-d"],
        Some(&curl),
    );
    assert_refusal(
        &output,
        2,
        &format!(
            "Spotify show links are not references: https://open.spotify.com/show/{ID}"
        ),
        "capture one episode (⋯ → Share → Copy link); follow the show as a record in your podcasts album (docs/records.md)",
    );

    let output = create(
        &vault,
        &["https://spotify.link/dead", "-P", "sase", "-d"],
        Some(&curl),
    );
    assert_refusal(
        &output,
        1,
        "could not resolve Spotify short link https://spotify.link/dead to an open.spotify.com link",
        "open it in Spotify and share the open.spotify.com link instead",
    );
}

#[test]
fn find_resolves_every_spotify_spelling_to_the_canonical_key() {
    let temp = TempDir::new("bob-cli-spotify-find");
    let vault = temp.path().join("vault");
    write_legacy_ref(&vault);
    for query in [
        format!("spotify:episode:{ID}"),
        CANONICAL.to_string(),
        format!("https://open.spotify.com/intl-de/episode/{ID}?si=zzz&t=5"),
    ] {
        let output = bob_command()
            .arg("ref")
            .arg("find")
            .arg(&query)
            .arg("-f")
            .arg("json")
            .env("BOB_DIR", &vault)
            .output()
            .expect("run bob ref find");
        assert_success(&output);
        let document: serde_json::Value =
            serde_json::from_str(&stdout(&output)).unwrap_or_else(|error| {
                panic!("parse find JSON: {error}\n{}", format_output(&output))
            });
        let result = &document["results"][0];
        assert_eq!(result["query_kind"], "url", "{query}: {result}");
        assert_eq!(result["verdict"], "in_library", "{query}: {result}");
        assert_eq!(result["keys"][0], CANONICAL, "{query}: {result}");
        assert_eq!(
            result["matches"][0]["matched_key"], CANONICAL,
            "{query}: {result}"
        );
    }
}
