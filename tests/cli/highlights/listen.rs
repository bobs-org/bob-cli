//! Highlights `--listen` tests: the fake listen command, attach mode,
//! dry runs, failure paths, and doctor rows.

use super::fake_clip::*;
use crate::support::*;

/// Fake `BOB_HIGHLIGHTS_LISTEN_COMMAND`: logs argv to `$FAKE_LISTEN_LOG`,
/// logs the environment to `$FAKE_LISTEN_ENV_LOG` when set, prints canned
/// lines, and writes an ID3 stub to the `.mp3` argument.
///
/// - `FAKE_LISTEN_EXIT`: exit code instead of 0.
/// - `FAKE_LISTEN_NO_AUDIO=1`: write no audio (the NoAudio path).
/// - `FAKE_LISTEN_TOUCH`: an extra file to create before exiting (the
///   post-listen collision path).
/// - `FAKE_LISTEN_PROBE_TARGET`: when set, the fake records whether that
///   path exists (`target-exists:yes/no`) to prove `{target}` is staged
///   while the command runs.
fn write_fake_listen(dir: &std::path::Path) -> std::path::PathBuf {
    let path = dir.join("fake-listen.sh");
    let script = r#"#!/bin/sh
echo "$@" >> "$FAKE_LISTEN_LOG"
if [ -n "$FAKE_LISTEN_ENV_LOG" ]; then env >> "$FAKE_LISTEN_ENV_LOG"; fi
# Prove `{target}` ($1) exists while the command runs.
if [ -f "$1" ]; then
  echo "target-exists:yes" >> "$FAKE_LISTEN_LOG"
else
  echo "target-exists:no" >> "$FAKE_LISTEN_LOG"
fi
if [ -n "$FAKE_LISTEN_PROBE_TARGET" ]; then
  if [ -f "$FAKE_LISTEN_PROBE_TARGET" ]; then
    echo "probe-exists:yes" >> "$FAKE_LISTEN_LOG"
  else
    echo "probe-exists:no" >> "$FAKE_LISTEN_LOG"
  fi
fi
echo "FAKE-LISTEN-STDOUT"
echo "FAKE-LISTEN-STDERR" >&2
if [ -n "$FAKE_LISTEN_TOUCH" ]; then mkdir -p "$(dirname "$FAKE_LISTEN_TOUCH")"; printf 'fake' > "$FAKE_LISTEN_TOUCH"; fi
for arg in "$@"; do
  case "$arg" in
    *.mp3)
      if [ -z "$FAKE_LISTEN_NO_AUDIO" ]; then
        printf 'ID3 fake episode' > "$arg"
      fi
      ;;
  esac
done
exit "${FAKE_LISTEN_EXIT:-0}"
"#;
    write_executable(&path, script);
    path
}

/// Minimal fake curl: serves `$FAKE_CURL_ROOT/paper.pdf` as a PDF for any
/// URL and logs the request URL to `$FAKE_CURL_LOG`.
fn write_fake_pdf_curl(dir: &std::path::Path) -> std::path::PathBuf {
    let path = dir.join("fake-curl.sh");
    let script = r#"#!/bin/sh
dest=""
url=""
prev=""
for arg in "$@"; do
  if [ "$prev" = "-o" ]; then dest="$arg"; fi
  prev="$arg"
  url="$arg"
done
echo "$url" >> "$FAKE_CURL_LOG"
cp "$FAKE_CURL_ROOT/paper.pdf" "$dest"
printf '200\napplication/pdf\n\n'
"#;
    write_executable(&path, script);
    path
}

/// Minimal fake curl: serves an HTML article for any URL.
fn write_fake_html_curl(dir: &std::path::Path) -> std::path::PathBuf {
    let path = dir.join("fake-html-curl.sh");
    let script = r#"#!/bin/sh
dest=""
url=""
prev=""
for arg in "$@"; do
  if [ "$prev" = "-o" ]; then dest="$arg"; fi
  prev="$arg"
  url="$arg"
done
echo "$url" >> "$FAKE_CURL_LOG"
printf '<html><body>article</body></html>' > "$dest"
printf '200\ntext/html; charset=utf-8\n\n'
"#;
    write_executable(&path, script);
    path
}

fn write_bare_pdf(path: &std::path::Path) {
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
    doc.save(path).expect("write bare PDF");
}

/// Fake pandoc: copies `$FAKE_PANDOC_FIXTURE` to the `-o` output.
fn write_fake_pandoc(dir: &std::path::Path) -> std::path::PathBuf {
    let path = dir.join("fake-pandoc.sh");
    let script = r#"#!/bin/sh
out=""
prev=""
for arg in "$@"; do
  if [ "$prev" = "-o" ]; then out="$arg"; fi
  prev="$arg"
done
cp "$FAKE_PANDOC_FIXTURE" "$out"
"#;
    write_executable(&path, script);
    path
}

/// Recursively collect files under `dir` (empty when missing).
fn collect_files(dir: &std::path::Path) -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(current) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&current) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if path.is_file() {
                out.push(path);
            }
        }
    }
    out
}

/// `BOB_HIGHLIGHTS_LISTEN_COMMAND` value invoking the fake.
fn listen_command(fake: &std::path::Path) -> String {
    format!(
        "{} {{target}} -e full -o {{audio}} --title {{title}}",
        path_str(fake)
    )
}

#[test]
fn listen_markdown_binds_episode_and_play_uri() {
    let temp = TempDir::new("bob-cli-highlights-listen-markdown");
    let source = temp.path().join("report.md");
    let vault = temp.path().join("vault");
    write_file(
        &source,
        "# Report\n\n<div class=\"listen\">\n\n♫ **Brief audio edition**\n\n</div>\n",
    );
    write_bare_pdf(&temp.path().join("render-fixture.pdf"));
    let pandoc = write_fake_pandoc(temp.path());
    let fake = write_fake_listen(temp.path());
    let log = temp.path().join("listen.log");

    let output = bob_command()
        .arg("highlights")
        .arg("create")
        .arg(&source)
        .arg("-b")
        .arg(&vault)
        .arg("-L")
        .env("BOB_PANDOC_COMMAND", &pandoc)
        .env(
            "FAKE_PANDOC_FIXTURE",
            temp.path().join("render-fixture.pdf"),
        )
        .env("BOB_HIGHLIGHTS_LISTEN_COMMAND", listen_command(&fake))
        .env("FAKE_LISTEN_LOG", &log)
        .output()
        .expect("run create markdown --listen");
    assert_success(&output);
    let report = stdout(&output);
    assert!(
        vault.join("xlib/chat/report.mp3").is_file(),
        "episode must be bound beside the PDF: {report}"
    );
    assert!(
        vault.join("xlib/chat/report.pdf").is_file(),
        "PDF must be installed: {report}"
    );
    assert!(
        report.contains("(from --listen)")
            && report.contains("audio_link:")
            && report.contains("lib%2Fchat%2Freport.mp3"),
        "report binds the episode and its Play URI: {report}"
    );
    // `{target}` exists during the run and lives outside xlib/.
    let logged = std::fs::read_to_string(&log).expect("read listen log");
    assert!(
        logged.contains("target-exists:yes"),
        "the fake proves {{target}} exists while it runs: {logged}"
    );
    assert!(
        logged.contains("bob-create-")
            && logged.contains("report.pdf")
            && !logged.contains("xlib"),
        "narration source is staged scratch outside xlib: {logged}"
    );
}

#[test]
fn listen_pdf_url_streams_output_and_quotes_title() {
    let temp = TempDir::new("bob-cli-highlights-listen-pdf-url");
    let vault = temp.path().join("vault");
    let root = temp.path().join("curl-root");
    std::fs::create_dir_all(&root).expect("create curl root");
    write_bare_pdf(&root.join("paper.pdf"));
    let fake_curl = write_fake_pdf_curl(temp.path());
    let curl_log = temp.path().join("curl.log");
    std::fs::write(&curl_log, "").expect("init curl log");
    let fake = write_fake_listen(temp.path());
    let log = temp.path().join("listen.log");

    let output = bob_command()
        .arg("highlights")
        .arg("create")
        .arg("https://example.com/paper.pdf")
        .arg("-b")
        .arg(&vault)
        .arg("-L")
        .arg("-T")
        .arg("Bob's Paper")
        .env("BOB_HIGHLIGHTS_CURL", &fake_curl)
        .env("FAKE_CURL_ROOT", &root)
        .env("FAKE_CURL_LOG", &curl_log)
        .env("BOB_HIGHLIGHTS_LISTEN_COMMAND", listen_command(&fake))
        .env("FAKE_LISTEN_LOG", &log)
        .output()
        .expect("run create PDF URL --listen");
    assert_success(&output);
    let report = stdout(&output);
    let run_at = report.find("listen: run").expect("listen run line");
    let fake_at = report
        .find("FAKE-LISTEN-STDOUT")
        .expect("fake listen output");
    assert!(
        run_at < fake_at,
        "`listen: run` precedes the command output: {report}"
    );
    assert!(
        report.contains("'Bob'\\''s Paper'"),
        "the title is shell-quoted: {report}"
    );
    // `{target}` and `{title}` come from the fake's argv log, not bob's
    // stdout: the URL also appears on the `source:` line, so a stdout
    // check proves nothing.
    let logged = std::fs::read_to_string(&log).expect("read listen log");
    assert!(
        logged.contains("https://example.com/paper.pdf"),
        "the fake saw {{target}} as the cleaned URL: {logged}"
    );
    assert!(
        logged.contains("Bob's Paper"),
        "the fake saw {{title}} with the apostrophe intact: {logged}"
    );
    assert!(
        stderr(&output).contains("FAKE-LISTEN-STDERR"),
        "stderr streams unchanged: {}",
        format_output(&output)
    );
    assert!(
        vault.join("xlib/papers/paper.mp3").is_file(),
        "episode must be bound: {report}"
    );
    assert!(report.contains("(from --listen)"), "{report}");
}

#[test]
fn listen_failure_writes_nothing() {
    let temp = TempDir::new("bob-cli-highlights-listen-failure");
    let vault = temp.path().join("vault");
    let root = temp.path().join("curl-root");
    std::fs::create_dir_all(&root).expect("create curl root");
    write_bare_pdf(&root.join("paper.pdf"));
    let fake_curl = write_fake_pdf_curl(temp.path());
    let curl_log = temp.path().join("curl.log");
    std::fs::write(&curl_log, "").expect("init curl log");
    let fake = write_fake_listen(temp.path());
    let log = temp.path().join("listen.log");

    let output = bob_command()
        .arg("highlights")
        .arg("create")
        .arg("https://example.com/paper.pdf")
        .arg("-b")
        .arg(&vault)
        .arg("--listen")
        .env("BOB_HIGHLIGHTS_CURL", &fake_curl)
        .env("FAKE_CURL_ROOT", &root)
        .env("FAKE_CURL_LOG", &curl_log)
        .env("BOB_HIGHLIGHTS_LISTEN_COMMAND", listen_command(&fake))
        .env("FAKE_LISTEN_LOG", &log)
        .env("FAKE_LISTEN_EXIT", "4")
        .output()
        .expect("run create with failing listen");
    assert_eq!(output.status.code(), Some(1), "{}", format_output(&output));
    let diagnostic = stderr(&output);
    assert!(
        diagnostic.contains("listen command failed with exit 4")
            && diagnostic.contains("nothing was written"),
        "{diagnostic}"
    );
    assert!(
        !vault.join("xlib/papers/paper.pdf").exists()
            && !vault.join("xlib/papers/paper.mp3").exists(),
        "a failed listen writes nothing"
    );
    // No file at all may remain under xlib/.
    let leftovers = collect_files(&vault.join("xlib"));
    assert!(
        leftovers.is_empty(),
        "xlib/ must contain no files after exit 4: {leftovers:?}"
    );
}

#[test]
fn listen_interrupted_exits_130() {
    let temp = TempDir::new("bob-cli-highlights-listen-interrupt");
    let vault = temp.path().join("vault");
    let root = temp.path().join("curl-root");
    std::fs::create_dir_all(&root).expect("create curl root");
    write_bare_pdf(&root.join("paper.pdf"));
    let fake_curl = write_fake_pdf_curl(temp.path());
    let curl_log = temp.path().join("curl.log");
    std::fs::write(&curl_log, "").expect("init curl log");
    let fake = write_fake_listen(temp.path());
    let log = temp.path().join("listen.log");

    let output = bob_command()
        .arg("highlights")
        .arg("create")
        .arg("https://example.com/paper.pdf")
        .arg("-b")
        .arg(&vault)
        .arg("-L")
        .env("BOB_HIGHLIGHTS_CURL", &fake_curl)
        .env("FAKE_CURL_ROOT", &root)
        .env("FAKE_CURL_LOG", &curl_log)
        .env("BOB_HIGHLIGHTS_LISTEN_COMMAND", listen_command(&fake))
        .env("FAKE_LISTEN_LOG", &log)
        .env("FAKE_LISTEN_EXIT", "130")
        .output()
        .expect("run create with interrupted listen");
    assert_eq!(
        output.status.code(),
        Some(130),
        "{}",
        format_output(&output)
    );
    assert!(
        !vault.join("xlib/papers/paper.pdf").exists(),
        "an interrupted listen writes nothing"
    );
}

#[test]
fn listen_without_audio_is_an_error() {
    let temp = TempDir::new("bob-cli-highlights-listen-no-audio");
    let vault = temp.path().join("vault");
    let root = temp.path().join("curl-root");
    std::fs::create_dir_all(&root).expect("create curl root");
    write_bare_pdf(&root.join("paper.pdf"));
    let fake_curl = write_fake_pdf_curl(temp.path());
    let curl_log = temp.path().join("curl.log");
    std::fs::write(&curl_log, "").expect("init curl log");
    let fake = write_fake_listen(temp.path());
    let log = temp.path().join("listen.log");

    let output = bob_command()
        .arg("highlights")
        .arg("create")
        .arg("https://example.com/paper.pdf")
        .arg("-b")
        .arg(&vault)
        .arg("-L")
        .env("BOB_HIGHLIGHTS_CURL", &fake_curl)
        .env("FAKE_CURL_ROOT", &root)
        .env("FAKE_CURL_LOG", &curl_log)
        .env("BOB_HIGHLIGHTS_LISTEN_COMMAND", listen_command(&fake))
        .env("FAKE_LISTEN_LOG", &log)
        .env("FAKE_LISTEN_NO_AUDIO", "1")
        .output()
        .expect("run create with silent listen");
    assert_eq!(output.status.code(), Some(1), "{}", format_output(&output));
    assert!(
        stderr(&output).contains("wrote no MP3 audio"),
        "{}",
        format_output(&output)
    );
    assert!(
        !vault.join("xlib/papers/paper.mp3").exists(),
        "missing audio is never installed"
    );
    assert!(
        !vault.join("xlib/papers/paper.pdf").exists(),
        "exit-0-with-no-audio also leaves no PDF"
    );
    let leftovers = collect_files(&vault.join("xlib"));
    assert!(
        leftovers.is_empty(),
        "xlib/ must contain no files after NoAudio: {leftovers:?}"
    );
}

#[test]
fn listen_dry_run_prints_would_run_without_invoking() {
    let temp = TempDir::new("bob-cli-highlights-listen-dry-run");
    let vault = temp.path().join("vault");
    let root = temp.path().join("curl-root");
    std::fs::create_dir_all(&root).expect("create curl root");
    write_bare_pdf(&root.join("paper.pdf"));
    let fake_curl = write_fake_pdf_curl(temp.path());
    let curl_log = temp.path().join("curl.log");
    std::fs::write(&curl_log, "").expect("init curl log");
    let fake = write_fake_listen(temp.path());
    let log = temp.path().join("listen.log");

    let output = bob_command()
        .arg("highlights")
        .arg("create")
        .arg("https://example.com/paper.pdf")
        .arg("-b")
        .arg(&vault)
        .arg("-L")
        .arg("-d")
        .env("BOB_HIGHLIGHTS_CURL", &fake_curl)
        .env("FAKE_CURL_ROOT", &root)
        .env("FAKE_CURL_LOG", &curl_log)
        .env("BOB_HIGHLIGHTS_LISTEN_COMMAND", listen_command(&fake))
        .env("FAKE_LISTEN_LOG", &log)
        .output()
        .expect("dry-run create --listen");
    assert_success(&output);
    let report = stdout(&output);
    assert!(
        report.contains("listen: would-run")
            && report.contains("writes: none")
            && report.contains("(from --listen)"),
        "{report}"
    );
    assert!(!log.exists(), "dry run never invokes the listen command");
    assert!(!vault.exists(), "dry run writes nothing");
}

#[test]
fn listen_unconfigured_fails_before_fetch() {
    let temp = TempDir::new("bob-cli-highlights-listen-unconfigured");
    let vault = temp.path().join("vault");
    let root = temp.path().join("curl-root");
    std::fs::create_dir_all(&root).expect("create curl root");
    write_bare_pdf(&root.join("paper.pdf"));
    let fake_curl = write_fake_pdf_curl(temp.path());
    let curl_log = temp.path().join("curl.log");

    let output = bob_command()
        .arg("highlights")
        .arg("create")
        .arg("https://example.com/paper.pdf")
        .arg("-b")
        .arg(&vault)
        .arg("-L")
        .env("BOB_HIGHLIGHTS_CURL", &fake_curl)
        .env("FAKE_CURL_ROOT", &root)
        .env("FAKE_CURL_LOG", &curl_log)
        .env_remove("BOB_HIGHLIGHTS_LISTEN_COMMAND")
        .output()
        .expect("run create --listen unconfigured");
    assert_eq!(output.status.code(), Some(1), "{}", format_output(&output));
    let diagnostic = stderr(&output);
    assert!(
        diagnostic.contains("--listen needs highlights.listen_command")
            && diagnostic.contains(
                "listen_command: sase-listen render {target} -e full -o {audio}"
            ),
        "{diagnostic}"
    );
    assert!(
        !curl_log.exists(),
        "an unconfigured --listen fails pre-fetch"
    );
}

#[test]
fn listen_invalid_template_is_an_error() {
    let temp = TempDir::new("bob-cli-highlights-listen-invalid");
    let vault = temp.path().join("vault");
    let source = temp.path().join("report.md");
    write_file(&source, "# Report\n");

    let output = bob_command()
        .arg("highlights")
        .arg("create")
        .arg(&source)
        .arg("-b")
        .arg(&vault)
        .arg("-L")
        .env("BOB_HIGHLIGHTS_LISTEN_COMMAND", "render {target}")
        .output()
        .expect("run create --listen with invalid template");
    assert_eq!(output.status.code(), Some(1), "{}", format_output(&output));
    assert!(
        stderr(&output).contains("must write MP3 audio to {audio}"),
        "{}",
        format_output(&output)
    );
    assert!(!vault.exists(), "an invalid template fails pre-render");
}

#[test]
fn listen_conflicts_with_audio_and_no_audio() {
    let temp = TempDir::new("bob-cli-highlights-listen-conflicts");
    let source = temp.path().join("report.md");
    write_file(&source, "# Report\n");
    let audio = temp.path().join("episode.mp3");
    std::fs::write(&audio, b"audio-bytes").expect("write audio");

    for args in [vec!["-L", "--audio", path_str(&audio)], vec!["-L", "-n"]] {
        let output = bob_command()
            .arg("highlights")
            .arg("create")
            .arg(&source)
            .args(&args)
            .output()
            .unwrap_or_else(|error| panic!("run create {args:?}: {error}"));
        assert_eq!(
            output.status.code(),
            Some(2),
            "args {args:?}: {}",
            format_output(&output)
        );
        let diagnostic = format!("{}{}", stdout(&output), stderr(&output));
        assert!(
            diagnostic.contains("cannot be used with"),
            "args {args:?}: {diagnostic}"
        );
    }
}

/// A library capture plus its ref note, for attach tests.
fn write_library_capture(
    vault: &std::path::Path,
    title: &str,
    url_key: &str,
    url_value: &str,
    audio_field: Option<&str>,
) -> Vec<u8> {
    let pdf = vault.join("lib/papers/ea_graph.pdf");
    write_bare_pdf(&pdf);
    let before = std::fs::read(&pdf).expect("read library PDF bytes");
    let mut note = format!(
        "---\ntitle: {title}\nsource_pdf: lib/papers/ea_graph.pdf\n{url_key}: {url_value}\n"
    );
    if let Some(audio) = audio_field {
        note.push_str(&format!("audio: {audio}\n"));
    }
    note.push_str("---\n\n# EA-Graph\n");
    write_file(&vault.join("ref/papers/ea_graph.md"), &note);
    before
}

fn listen_for(temp: &TempDir) -> (std::path::PathBuf, std::path::PathBuf) {
    let fake = write_fake_listen(temp.path());
    let log = temp.path().join("listen.log");
    (fake, log)
}

#[test]
fn listen_attaches_to_ref_note_capture() {
    let temp = TempDir::new("bob-cli-highlights-listen-attach-note");
    let vault = temp.path().join("vault");
    let before = write_library_capture(
        &vault,
        "EA-Graph Paper",
        "source_url",
        "https://arxiv.org/abs/2608.04278",
        None,
    );
    let fake_curl = write_fake_pdf_curl(temp.path());
    let curl_log = temp.path().join("curl.log");
    let (fake, log) = listen_for(&temp);

    // A different spelling of the same paper hits the same dedupe key.
    let output = bob_command()
        .arg("highlights")
        .arg("create")
        .arg("https://arxiv.org/abs/2608.04278v2")
        .arg("-b")
        .arg(&vault)
        .arg("-L")
        .env("BOB_HIGHLIGHTS_CURL", &fake_curl)
        .env("FAKE_CURL_ROOT", temp.path())
        .env("FAKE_CURL_LOG", &curl_log)
        .env("BOB_HIGHLIGHTS_LISTEN_COMMAND", listen_command(&fake))
        .env("FAKE_LISTEN_LOG", &log)
        .output()
        .expect("run create --listen attach");
    assert_success(&output);
    let report = stdout(&output);
    assert!(
        report.contains("ok attached listen episode to existing capture")
            && report.contains("(unchanged)")
            && report.contains("title: EA-Graph Paper")
            && report.contains("xlib/papers/ea_graph.mp3 (from --listen)")
            && report.contains("next: bob ref scan"),
        "{report}"
    );
    assert!(
        report.contains(
            &vault.join("ref/papers/ea_graph.md").display().to_string()
        ),
        "the ref note is named: {report}"
    );
    assert!(
        vault.join("xlib/papers/ea_graph.mp3").is_file(),
        "episode lands beside the queued path"
    );
    assert_eq!(
        std::fs::read(vault.join("lib/papers/ea_graph.pdf"))
            .expect("reread library PDF"),
        before,
        "the library PDF is untouched"
    );
    assert!(
        !curl_log.exists(),
        "attach never fetches: {}",
        format_output(&output)
    );
    let logged = std::fs::read_to_string(&log).expect("read listen log");
    assert!(
        logged.contains("https://arxiv.org/abs/2608.04278v2"),
        "the cleaned URL is the narration source: {logged}"
    );
}

#[test]
fn listen_attaches_for_legacy_url_arxiv_note() {
    let temp = TempDir::new("bob-cli-highlights-listen-attach-legacy");
    let vault = temp.path().join("vault");
    write_library_capture(
        &vault,
        "EA-Graph Paper",
        "url",
        "https://arxiv.org/pdf/2608.04278",
        None,
    );
    let (fake, log) = listen_for(&temp);

    let output = bob_command()
        .arg("highlights")
        .arg("create")
        .arg("https://arxiv.org/abs/2608.04278")
        .arg("-b")
        .arg(&vault)
        .arg("-L")
        .env("BOB_HIGHLIGHTS_CURL", temp.path().join("unused-curl"))
        .env("BOB_HIGHLIGHTS_LISTEN_COMMAND", listen_command(&fake))
        .env("FAKE_LISTEN_LOG", &log)
        .output()
        .expect("run create --listen legacy attach");
    assert_success(&output);
    let report = stdout(&output);
    assert!(
        report.contains("ok attached listen episode to existing capture")
            && report.contains("(arXiv 2608.04278)"),
        "{report}"
    );
    assert!(
        vault.join("xlib/papers/ea_graph.mp3").is_file(),
        "episode lands for the legacy-url note"
    );
}

#[test]
fn listen_attach_refuses_when_audio_exists() {
    // A ref note that already carries an audio field refuses.
    let temp = TempDir::new("bob-cli-highlights-listen-attach-audio-field");
    let vault = temp.path().join("vault");
    write_library_capture(
        &vault,
        "EA-Graph Paper",
        "source_url",
        "https://arxiv.org/abs/2608.04278",
        Some("\"[[lib/papers/ea_graph.mp3]]\""),
    );
    let (fake, log) = listen_for(&temp);

    let output = bob_command()
        .arg("highlights")
        .arg("create")
        .arg("https://arxiv.org/abs/2608.04278")
        .arg("-b")
        .arg(&vault)
        .arg("-L")
        .env("BOB_HIGHLIGHTS_LISTEN_COMMAND", listen_command(&fake))
        .env("FAKE_LISTEN_LOG", &log)
        .output()
        .expect("run attach with note audio");
    assert_eq!(output.status.code(), Some(1), "{}", format_output(&output));
    assert!(
        stderr(&output).contains("already has companion audio"),
        "{}",
        format_output(&output)
    );
    assert!(
        !vault.join("xlib/papers/ea_graph.mp3").exists(),
        "a refused attach writes nothing"
    );

    // A companion beside the library PDF refuses too.
    let temp = TempDir::new("bob-cli-highlights-listen-attach-companion");
    let vault = temp.path().join("vault");
    write_library_capture(
        &vault,
        "EA-Graph Paper",
        "source_url",
        "https://arxiv.org/abs/2608.04278",
        None,
    );
    std::fs::write(vault.join("lib/papers/ea_graph.mp3"), b"old-episode")
        .expect("write library companion");
    let (fake, log) = listen_for(&temp);

    let output = bob_command()
        .arg("highlights")
        .arg("create")
        .arg("https://arxiv.org/abs/2608.04278")
        .arg("-b")
        .arg(&vault)
        .arg("-L")
        .env("BOB_HIGHLIGHTS_LISTEN_COMMAND", listen_command(&fake))
        .env("FAKE_LISTEN_LOG", &log)
        .output()
        .expect("run attach with library companion");
    assert_eq!(output.status.code(), Some(1), "{}", format_output(&output));
    let diagnostic = stderr(&output);
    assert!(
        diagnostic.contains("already has companion audio")
            && diagnostic.contains("ea_graph.mp3"),
        "{diagnostic}"
    );
}

#[test]
fn listen_attaches_to_queued_intake_pdf() {
    let temp = TempDir::new("bob-cli-highlights-listen-attach-intake");
    let vault = temp.path().join("vault");
    let root = temp.path().join("curl-root");
    std::fs::create_dir_all(&root).expect("create curl root");
    write_bare_pdf(&root.join("paper.pdf"));
    let fake_curl = write_fake_pdf_curl(temp.path());
    let curl_log = temp.path().join("curl.log");
    std::fs::write(&curl_log, "").expect("init curl log");
    let (fake, log) = listen_for(&temp);

    // Capture the PDF URL without --listen first: it queues the intake.
    let output = bob_command()
        .arg("highlights")
        .arg("create")
        .arg("https://example.com/paper.pdf")
        .arg("-b")
        .arg(&vault)
        .env("BOB_HIGHLIGHTS_CURL", &fake_curl)
        .env("FAKE_CURL_ROOT", &root)
        .env("FAKE_CURL_LOG", &curl_log)
        .output()
        .expect("queue the intake PDF");
    assert_success(&output);
    assert!(vault.join("xlib/papers/paper.pdf").is_file());

    // The same URL with --listen attaches to the queued intake.
    let output = bob_command()
        .arg("highlights")
        .arg("create")
        .arg("https://example.com/paper.pdf")
        .arg("-b")
        .arg(&vault)
        .arg("-L")
        .env("BOB_HIGHLIGHTS_CURL", &fake_curl)
        .env("FAKE_CURL_ROOT", &root)
        .env("FAKE_CURL_LOG", &curl_log)
        .env("BOB_HIGHLIGHTS_LISTEN_COMMAND", listen_command(&fake))
        .env("FAKE_LISTEN_LOG", &log)
        .output()
        .expect("run create --listen intake attach");
    assert_success(&output);
    let report = stdout(&output);
    assert!(
        report.contains("ok attached listen episode to existing capture")
            && report.contains("xlib/papers/paper.mp3 (from --listen)")
            && !report.contains("ref: "),
        "{report}"
    );
    assert!(vault.join("xlib/papers/paper.mp3").is_file());
}

#[test]
fn listen_attaches_when_target_is_the_library_pdf() {
    let temp = TempDir::new("bob-cli-highlights-listen-attach-local");
    let vault = temp.path().join("vault");
    let source = temp.path().join("paper.pdf");
    write_bare_pdf(&source);

    // Capture a local PDF, then move it into the library by hand: the
    // marker (and its bytes) travel with it.
    let output = bob_command()
        .arg("highlights")
        .arg("create")
        .arg(&source)
        .arg("-b")
        .arg(&vault)
        .output()
        .expect("capture the local PDF");
    assert_success(&output);
    let queued = vault.join("xlib/papers/paper.pdf");
    let library = vault.join("lib/papers/paper.pdf");
    std::fs::create_dir_all(library.parent().expect("lib parent"))
        .expect("create lib dir");
    std::fs::rename(&queued, &library).expect("archive the PDF");
    let before = std::fs::read(&library).expect("read library bytes");
    let (fake, log) = listen_for(&temp);

    let output = bob_command()
        .arg("highlights")
        .arg("create")
        .arg(&library)
        .arg("-b")
        .arg(&vault)
        .arg("-L")
        .env("BOB_HIGHLIGHTS_LISTEN_COMMAND", listen_command(&fake))
        .env("FAKE_LISTEN_LOG", &log)
        .output()
        .expect("run create library PDF --listen");
    assert_success(&output);
    let report = stdout(&output);
    assert!(
        report.contains("ok attached listen episode to existing capture")
            && report.contains("(unchanged)"),
        "{report}"
    );
    assert!(vault.join("xlib/papers/paper.mp3").is_file());
    assert_eq!(
        std::fs::read(&library).expect("reread library PDF"),
        before,
        "the library PDF is untouched"
    );
    let logged = std::fs::read_to_string(&log).expect("read listen log");
    assert!(
        logged.contains(&library.display().to_string()),
        "the PDF path is the narration source: {logged}"
    );
}

#[test]
fn listen_refusal_without_flag_points_at_listen() {
    let temp = TempDir::new("bob-cli-highlights-listen-hint");
    let vault = temp.path().join("vault");
    let root = temp.path().join("curl-root");
    std::fs::create_dir_all(&root).expect("create curl root");
    write_bare_pdf(&root.join("paper.pdf"));
    let fake_curl = write_fake_pdf_curl(temp.path());
    let curl_log = temp.path().join("curl.log");
    std::fs::write(&curl_log, "").expect("init curl log");

    let run = || {
        let mut command = bob_command();
        command
            .arg("highlights")
            .arg("create")
            .arg("https://example.com/paper.pdf")
            .arg("-b")
            .arg(&vault)
            .env("BOB_HIGHLIGHTS_CURL", &fake_curl)
            .env("FAKE_CURL_ROOT", &root)
            .env("FAKE_CURL_LOG", &curl_log);
        command.output().expect("run create PDF URL")
    };
    assert_success(&run());
    let rerun = run();
    assert_eq!(rerun.status.code(), Some(1), "{}", format_output(&rerun));
    assert!(
        stderr(&rerun).contains("add --listen"),
        "{}",
        format_output(&rerun)
    );
}

#[test]
fn listen_post_listen_collision_keeps_scratch() {
    let temp = TempDir::new("bob-cli-highlights-listen-collision");
    let source = temp.path().join("report.md");
    let vault = temp.path().join("vault");
    write_file(&source, "# Report\n");
    write_bare_pdf(&temp.path().join("render-fixture.pdf"));
    let pandoc = write_fake_pandoc(temp.path());
    let (fake, log) = listen_for(&temp);
    // The fake creates the target PDF mid-listen, so the post-listen
    // collision check fires.
    let target = vault.join("xlib/chat/report.pdf");

    let output = bob_command()
        .arg("highlights")
        .arg("create")
        .arg(&source)
        .arg("-b")
        .arg(&vault)
        .arg("-L")
        .env("BOB_PANDOC_COMMAND", &pandoc)
        .env(
            "FAKE_PANDOC_FIXTURE",
            temp.path().join("render-fixture.pdf"),
        )
        .env("BOB_HIGHLIGHTS_LISTEN_COMMAND", listen_command(&fake))
        .env("FAKE_LISTEN_LOG", &log)
        .env("FAKE_LISTEN_TOUCH", &target)
        .output()
        .expect("run create with colliding listen");
    assert_eq!(output.status.code(), Some(1), "{}", format_output(&output));
    let report = stdout(&output);
    assert!(
        report.contains("kept:") && report.contains("report.mp3"),
        "the kept scratch audio is named: {report}"
    );
    let diagnostic = stderr(&output);
    assert!(
        diagnostic.contains("bind it with") && diagnostic.contains("--audio"),
        "the recovery hint rebinds the kept audio: {diagnostic}"
    );
    assert_eq!(
        std::fs::read(&target).expect("read collided target"),
        b"fake",
        "the colliding target is left as the listen run left it"
    );
    // The scratch directory is kept, with the episode inside.
    let workdir = diagnostic
        .lines()
        .find_map(|line| line.strip_prefix("workdir: "))
        .expect("kept workdir line");
    assert!(
        std::path::Path::new(workdir).join("report.mp3").is_file(),
        "the kept scratch holds the episode: {diagnostic}"
    );
    std::fs::remove_dir_all(workdir).ok();
}

#[test]
fn clip_listen_captures_and_binds_episode() {
    let temp = TempDir::new("bob-cli-highlights-clip-listen");
    let vault = temp.path().join("vault");
    let fake = FakeClip::new(&temp, "fake");
    let (listen, log) = listen_for(&temp);

    let output = bob_command()
        .arg("highlights")
        .arg("clip")
        .arg("https://example.com/article/hello")
        .arg("-b")
        .arg(&vault)
        .arg("-L")
        .env("BOB_WEB_CLIP_ADAPTER", &fake.path)
        .env("BOB_HIGHLIGHTS_LISTEN_COMMAND", listen_command(&listen))
        .env("FAKE_LISTEN_LOG", &log)
        .output()
        .expect("run clip --listen");
    assert_success(&output);
    let report = stdout(&output);
    assert!(
        vault.join("xlib/blogs/hello.pdf").is_file(),
        "the article PDF is installed: {report}"
    );
    assert!(
        vault.join("xlib/blogs/hello.mp3").is_file(),
        "the episode is bound: {report}"
    );
    assert!(report.contains("(from --listen)"), "{report}");
    let logged = std::fs::read_to_string(&log).expect("read listen log");
    assert!(
        logged.contains("https://example.com/article/hello"),
        "the cleaned URL is the narration source: {logged}"
    );
}

#[test]
fn create_article_listen_uses_adapter_render() {
    let temp = TempDir::new("bob-cli-highlights-create-article-listen");
    let vault = temp.path().join("vault");
    let fake = FakeClip::new(&temp, "fake");
    let fake_curl = write_fake_html_curl(temp.path());
    let curl_log = temp.path().join("curl.log");
    std::fs::write(&curl_log, "").expect("init curl log");
    let (listen, log) = listen_for(&temp);

    let output = bob_command()
        .arg("highlights")
        .arg("create")
        .arg("https://example.com/article/hello")
        .arg("-b")
        .arg(&vault)
        .arg("-L")
        .env("BOB_HIGHLIGHTS_CURL", &fake_curl)
        .env("FAKE_CURL_ROOT", temp.path())
        .env("FAKE_CURL_LOG", &curl_log)
        .env("BOB_WEB_CLIP_ADAPTER", &fake.path)
        .env("BOB_HIGHLIGHTS_LISTEN_COMMAND", listen_command(&listen))
        .env("FAKE_LISTEN_LOG", &log)
        .output()
        .expect("run create article --listen");
    assert_success(&output);
    let report = stdout(&output);
    assert!(
        vault.join("xlib/blogs/hello.mp3").is_file(),
        "the episode is bound on the article route: {report}"
    );
    assert!(report.contains("(from --listen)"), "{report}");
    let logged = std::fs::read_to_string(&log).expect("read listen log");
    assert!(
        logged.contains("https://example.com/article/hello"),
        "the cleaned URL is the narration source: {logged}"
    );
}

#[test]
fn doctor_listen_rows_cover_unset_ok_and_invalid() {
    let temp = TempDir::new("bob-cli-highlights-doctor-listen");
    let vault = temp.path().join("vault");
    std::fs::create_dir_all(vault.join("lib")).expect("create lib");
    std::fs::create_dir_all(vault.join("ref")).expect("create ref");
    std::fs::create_dir_all(vault.join("xlib")).expect("create xlib");
    git_in(&vault, ["init", "-q"]);
    configure_test_git_identity(&vault);

    let doctor = |listen: Option<String>| {
        let mut command = bob_command();
        command
            .arg("highlights")
            .arg("doctor")
            .arg("--no-hooks")
            .arg("-b")
            .arg(&vault);
        if let Some(template) = listen {
            command.env("BOB_HIGHLIGHTS_LISTEN_COMMAND", template);
        } else {
            command.env_remove("BOB_HIGHLIGHTS_LISTEN_COMMAND");
        }
        command.output().expect("run highlights doctor")
    };

    let output = doctor(None);
    assert_success(&output);
    assert!(
        stdout(&output).contains("listen_command: none"),
        "{}",
        format_output(&output)
    );

    let fake = write_fake_listen(temp.path());
    let output =
        doctor(Some(format!("{} {{target}} -o {{audio}}", path_str(&fake))));
    assert_success(&output);
    assert!(
        stdout(&output).contains("listen_command: ok"),
        "{}",
        format_output(&output)
    );

    let output = doctor(Some("render {target}".to_string()));
    assert_success(&output);
    assert!(
        stdout(&output).contains("listen_command: warn"),
        "{}",
        format_output(&output)
    );
}
