//! `bob ref links` read surface: JSON golden, raw/markdown/page
//! selection, all seven statuses, error envelopes, and human output.

use crate::support::*;
use serde_json::Value;
use std::fs;
use std::path::Path;
use std::process::Output;

const FIXED_NOW: &str = "2026-10-06 12:00:00";

fn sha256_hex_str(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    hex::encode(Sha256::digest(bytes))
}

fn base_vault(temp: &TempDir) -> std::path::PathBuf {
    let vault = temp.path().join("vault");
    fs::create_dir_all(vault.join("ref/papers")).expect("create ref/papers");
    fs::create_dir_all(vault.join("lib/papers")).expect("create lib/papers");
    vault
}

fn note_with_pointer(title: &str, pointer: &str) -> String {
    format!(
        "---\nstatus: ready\ntitle: {title}\nsource_markdown: {pointer}\n---\n\n# {title}\n\n- [ ] #task #ref #hide ^ref\n"
    )
}

fn note_without_pointer(title: &str) -> String {
    format!(
        "---\nstatus: ready\ntitle: {title}\n---\n\n# {title}\n\n- [ ] #task #ref #hide ^ref\n"
    )
}

fn json_escape(text: &str) -> String {
    text.replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
        .replace('\t', "\\t")
}

fn write_store(vault: &Path, stem: &str, source: &str, extra: &str) -> String {
    let digest = sha256_hex_str(source.as_bytes());
    let hash12 = digest[..12].to_string();
    let dir = vault.join(format!(".bob/ref-sources/papers/{stem}--{hash12}"));
    fs::create_dir_all(&dir).expect("create store dir");
    write_file(&dir.join("source.md"), source);
    write_file(
        &dir.join("code.json"),
        &format!(
            r#"{{"version":1,"captured_at":"2026-10-20T00:00:00Z","route":"markdown","source":{{"sha256":"{digest}","bytes":{},"origin":"/tmp/{stem}.md"}},"pdf":{{"path":"lib/papers/{stem}.pdf","sha256":"{}"}},"renderer":{{"bob":"0","pandoc":"test"}}{extra},"blocks":[]}}"#,
            source.len(),
            "0".repeat(64)
        ),
    );
    format!(".bob/ref-sources/papers/{stem}--{hash12}/source.md")
}

fn link_json(
    n: u32,
    url: &str,
    text: &str,
    title: Option<&str>,
    autolink: bool,
    context: Option<&str>,
    section_number: Option<&str>,
    section_title: &str,
    page: Option<u32>,
) -> String {
    let title_json = match title {
        Some(title) => format!("\"{}\"", json_escape(title)),
        None => "null".to_string(),
    };
    let context_json = match context {
        Some(context) => format!("\"{}\"", json_escape(context)),
        None => "null".to_string(),
    };
    let section_json = match section_number {
        Some(number) => format!(
            r#""section":{{"number":"{number}","title":"{}"}}"#,
            json_escape(section_title)
        ),
        None => format!(
            r#""section":{{"number":null,"title":"{}"}}"#,
            json_escape(section_title)
        ),
    };
    let page_json = match page {
        Some(page) => format!(r#""page":{page},"page_source":"anchor""#),
        None => r#""page":null,"page_source":null"#.to_string(),
    };
    format!(
        r#"{{"n":{n},"url":"{}","text":"{}","title":{title_json},"autolink":{autolink},"context":{context_json},{section_json},{page_json}}}"#,
        json_escape(url),
        json_escape(text)
    )
}

fn run_links(vault: &Path, args: &[&str]) -> Output {
    bob_command()
        .arg("ref")
        .arg("links")
        .args(args)
        .env("BOB_DIR", vault)
        .env("BOB_NOW", FIXED_NOW)
        .output()
        .expect("run bob ref links")
}

fn run_links_json(vault: &Path, args: &[&str]) -> (Output, Value) {
    let mut full = args.to_vec();
    full.extend(["-f", "json"]);
    let output = run_links(vault, &full);
    let value =
        serde_json::from_str(&stdout(&output)).unwrap_or_else(|error| {
            panic!("parse links JSON: {error}\n{}", format_output(&output))
        });
    (output, value)
}

fn golden_links_json() -> Vec<String> {
    vec![
        link_json(
            1,
            "https://developers.openai.com/docs/image-generation",
            "OpenAI image guide",
            None,
            false,
            Some("See also"),
            Some("3"),
            "Prior art",
            Some(6),
        ),
        link_json(
            2,
            "https://ai.google.dev/gemini-api/docs/imagen",
            "Imagen 4 model card",
            None,
            false,
            Some("Google's current image model is documented in the"),
            Some("4"),
            "Image models",
            Some(7),
        ),
        link_json(
            3,
            "https://www.github.com/black-forest-labs/flux",
            "https://www.github.com/black-forest-labs/flux",
            None,
            true,
            Some("Sources:"),
            Some("4"),
            "Image models",
            Some(7),
        ),
        link_json(
            4,
            "https://example.com/bare",
            "https://example.com/bare",
            None,
            true,
            None,
            Some("4"),
            "Image models",
            Some(7),
        ),
        link_json(
            5,
            "mailto:me@example.com",
            "me@example.com",
            None,
            true,
            None,
            Some("5"),
            "Contact",
            Some(8),
        ),
        link_json(
            6,
            "obsidian://open?vault=bob",
            "open vault",
            None,
            false,
            None,
            Some("5"),
            "Contact",
            None,
        ),
        link_json(
            7,
            "https://example.com/a b",
            "see [this] path",
            Some("ignored title"),
            false,
            None,
            Some("5"),
            "Contact",
            Some(8),
        ),
        link_json(
            8,
            "https://example.com/foo(bar",
            r"slash \ brackets",
            None,
            false,
            None,
            Some("5"),
            "Contact",
            Some(8),
        ),
        link_json(
            9,
            "https://ai.google.dev/gemini-api/docs/imagen",
            "Imagen again",
            None,
            false,
            None,
            Some("5"),
            "Contact",
            Some(9),
        ),
    ]
}

fn golden_vault(prefix: &str) -> (TempDir, std::path::PathBuf, String) {
    let temp = TempDir::new(prefix);
    let vault = base_vault(&temp);
    let source = "# Demo\n\nBody text.\n";
    let extra = format!(",\"links\":[{}]", golden_links_json().join(","));
    let pointer = write_store(&vault, "demo", source, &extra);
    write_file(
        &vault.join("ref/papers/demo.md"),
        &note_with_pointer("Demo", &pointer),
    );
    (temp, vault, pointer)
}

#[test]
fn links_json_golden_pins_envelope_and_computed_fields() {
    let (_temp, vault, pointer) = golden_vault("bob-cli-ref-links-golden");
    let (output, mut value) = run_links_json(&vault, &["demo"]);
    assert_success(&output);
    assert_stdout_has_no_ansi(&output);
    value["generated_at"] = Value::String("NORMALIZED".to_string());

    let digest = sha256_hex_str("# Demo\n\nBody text.\n".as_bytes());
    let hash12 = &digest[..12];
    assert_eq!(value["ok"], Value::Bool(true));
    assert_eq!(value["schema_version"], Value::from(1));
    assert_eq!(value["command"], Value::from("ref links"));
    assert_eq!(
        value["reference"]["path"],
        Value::from("ref/papers/demo.md")
    );
    assert_eq!(value["reference"]["title"], Value::from("Demo"));
    assert_eq!(value["status"], Value::from("available"));
    assert!(value["detail"].is_null(), "{value}");
    assert_eq!(value["source"]["path"], Value::from(pointer));

    let links = value["links"].as_array().expect("links array");
    assert_eq!(links.len(), 9);
    assert_eq!(links[1]["id"], Value::from(format!("{hash12}/link-2")));
    assert_eq!(links[1]["n"], Value::from(2));
    assert_eq!(
        links[1]["url"],
        Value::from("https://ai.google.dev/gemini-api/docs/imagen")
    );
    assert_eq!(links[1]["text"], Value::from("Imagen 4 model card"));
    assert!(links[1]["title"].is_null(), "{}", links[1]);
    assert_eq!(links[1]["autolink"], Value::Bool(false));
    assert_eq!(links[1]["label"], Value::from("Imagen 4 model card"));
    assert_eq!(links[1]["label_kind"], Value::from("text"));
    assert_eq!(links[1]["site"], Value::from("ai.google.dev"));
    assert_eq!(
        links[1]["markdown"],
        Value::from(
            "[Imagen 4 model card](https://ai.google.dev/gemini-api/docs/imagen)"
        )
    );
    assert_eq!(links[1]["section"]["number"], Value::from("4"));
    assert_eq!(links[1]["section"]["title"], Value::from("Image models"));
    assert_eq!(links[1]["page"], Value::from(7));
    assert_eq!(links[1]["page_source"], Value::from("anchor"));

    assert_eq!(links[2]["label"], Value::from("Sources"));
    assert_eq!(links[2]["label_kind"], Value::from("context"));
    assert_eq!(links[2]["site"], Value::from("github.com"));
    assert_eq!(
        links[2]["markdown"],
        Value::from("<https://www.github.com/black-forest-labs/flux>")
    );

    assert_eq!(links[3]["label"], Value::from("example.com/bare"));
    assert_eq!(links[3]["label_kind"], Value::from("url"));

    assert_eq!(links[4]["label"], Value::from("me@example.com"));
    assert_eq!(links[4]["label_kind"], Value::from("url"));
    assert_eq!(links[4]["site"], Value::from("email"));
    assert_eq!(links[4]["markdown"], Value::from("<me@example.com>"));

    assert_eq!(links[5]["site"], Value::from("obsidian"));
    assert!(links[5]["page"].is_null(), "{}", links[5]);

    assert_eq!(
        links[6]["markdown"],
        Value::from("[see \\[this\\] path](<https://example.com/a b>)")
    );
    assert_eq!(
        links[7]["markdown"],
        Value::from("[slash \\\\ brackets](<https://example.com/foo(bar>)")
    );

    let mut envelope_keys: Vec<&str> = value
        .as_object()
        .expect("object")
        .keys()
        .map(String::as_str)
        .collect();
    envelope_keys.sort_unstable();
    assert_eq!(
        envelope_keys,
        [
            "command",
            "detail",
            "generated_at",
            "links",
            "ok",
            "reference",
            "schema_version",
            "source",
            "status"
        ]
    );
    let mut link_keys: Vec<&str> = links[1]
        .as_object()
        .expect("link object")
        .keys()
        .map(String::as_str)
        .collect();
    link_keys.sort_unstable();
    assert_eq!(
        link_keys,
        [
            "autolink",
            "context",
            "id",
            "label",
            "label_kind",
            "markdown",
            "n",
            "page",
            "page_source",
            "section",
            "site",
            "text",
            "title",
            "url"
        ]
    );
    let mut source_keys: Vec<&str> = value["source"]
        .as_object()
        .expect("source object")
        .keys()
        .map(String::as_str)
        .collect();
    source_keys.sort_unstable();
    assert_eq!(source_keys, ["path"]);
}

#[test]
fn links_raw_bytes_match_exact_url_without_newline() {
    let (_temp, vault, _pointer) = golden_vault("bob-cli-ref-links-raw");
    let output = run_links(&vault, &["demo", "2"]);
    assert_success(&output);
    assert_eq!(
        output.stdout,
        b"https://ai.google.dev/gemini-api/docs/imagen"
    );
}

#[test]
fn links_markdown_n_and_list_with_escaping() {
    let (_temp, vault, _pointer) = golden_vault("bob-cli-ref-links-md");
    let one = run_links(&vault, &["demo", "7", "-f", "markdown"]);
    assert_success(&one);
    assert_eq!(
        stdout(&one),
        "[see \\[this\\] path](<https://example.com/a b>)"
    );

    let list = run_links(&vault, &["demo", "-f", "markdown"]);
    assert_success(&list);
    let text = stdout(&list);
    assert!(text.starts_with("# Demo\n"), "{text}");
    assert!(text.contains("## 4 Image models\n"), "{text}");
    assert!(
        text.contains(
            "- [Imagen 4 model card](https://ai.google.dev/gemini-api/docs/imagen) · p. 7\n"
        ),
        "{text}"
    );
    assert!(
        text.contains(
            "- <https://www.github.com/black-forest-labs/flux> · p. 7\n"
        ),
        "{text}"
    );
    assert!(
        text.contains("- [open vault](obsidian://open?vault=bob)\n"),
        "{text}"
    );
}

#[test]
fn links_page_filter_keeps_exact_page() {
    let (_temp, vault, _pointer) = golden_vault("bob-cli-ref-links-page");
    let (_output, value) = run_links_json(&vault, &["demo", "-p", "7"]);
    let links = value["links"].as_array().expect("links");
    let numbers: Vec<u64> = links
        .iter()
        .map(|link| link["n"].as_u64().expect("n"))
        .collect();
    assert_eq!(numbers, vec![2, 3, 4]);

    let human = run_links(&vault, &["demo", "-p", "99"]);
    assert_success(&human);
    assert_eq!(stdout(&human), "no links on page 99\n");
}

#[test]
fn links_all_seven_statuses_in_json() {
    let (_temp, vault, _pointer) = golden_vault("bob-cli-ref-links-available");
    let (output, value) = run_links_json(&vault, &["demo"]);
    assert_success(&output);
    assert_eq!(value["status"], Value::from("available"));

    {
        let temp = TempDir::new("bob-cli-ref-links-no-links");
        let vault = base_vault(&temp);
        let source = "# Empty\n\nNo links.\n";
        let pointer = write_store(&vault, "empty", source, r#","links":[]"#);
        write_file(
            &vault.join("ref/papers/empty.md"),
            &note_with_pointer("Empty", &pointer),
        );
        let (output, value) = run_links_json(&vault, &["empty"]);
        assert_success(&output);
        assert_eq!(value["status"], Value::from("no_links"));
        assert_eq!(value["detail"], Value::from("no links in this reference"));
        assert_eq!(value["links"].as_array().map(Vec::len), Some(0));
    }

    {
        let temp = TempDir::new("bob-cli-ref-links-not-recorded");
        let vault = base_vault(&temp);
        let source = "# Old\n\nBody.\n";
        let pointer = write_store(&vault, "old", source, "");
        write_file(
            &vault.join("ref/papers/old.md"),
            &note_with_pointer("Old", &pointer),
        );
        let (output, value) = run_links_json(&vault, &["old"]);
        assert_success(&output);
        assert_eq!(value["status"], Value::from("not_recorded"));
        assert_eq!(
            value["detail"],
            Value::from(
                "links were not recorded for this reference; run bob ref recatalog"
            )
        );
    }

    {
        let temp = TempDir::new("bob-cli-ref-links-no-catalog");
        let vault = base_vault(&temp);
        let source = "# Missing\n\nBody.\n";
        let digest = sha256_hex_str(source.as_bytes());
        let hash12 = &digest[..12];
        let dir =
            vault.join(format!(".bob/ref-sources/papers/missing--{hash12}"));
        fs::create_dir_all(&dir).expect("store dir");
        write_file(&dir.join("source.md"), source);
        let pointer =
            format!(".bob/ref-sources/papers/missing--{hash12}/source.md");
        write_file(
            &vault.join("ref/papers/missing.md"),
            &note_with_pointer("Missing", &pointer),
        );
        let (output, value) = run_links_json(&vault, &["missing"]);
        assert_success(&output);
        assert_eq!(value["status"], Value::from("no_catalog"));
    }

    {
        let temp = TempDir::new("bob-cli-ref-links-no-source");
        let vault = base_vault(&temp);
        write_file(
            &vault.join("ref/papers/plain.md"),
            &note_without_pointer("Plain"),
        );
        let (output, value) = run_links_json(&vault, &["plain"]);
        assert_success(&output);
        assert_eq!(value["status"], Value::from("no_source"));
        assert!(value["source"].is_null(), "{value}");
    }

    {
        let temp = TempDir::new("bob-cli-ref-links-source-missing");
        let vault = base_vault(&temp);
        let pointer =
            ".bob/ref-sources/papers/gone--aaaaaaaaaaaa/source.md".to_string();
        write_file(
            &vault.join("ref/papers/gone.md"),
            &note_with_pointer("Gone", &pointer),
        );
        let (output, value) = run_links_json(&vault, &["gone"]);
        assert_success(&output);
        assert_eq!(value["status"], Value::from("source_missing"));
    }

    {
        let temp = TempDir::new("bob-cli-ref-links-mismatch");
        let vault = base_vault(&temp);
        let source = "# Mismatch\n\nBody.\n";
        let pointer = write_store(&vault, "mismatch", source, r#","links":[]"#);
        fs::write(vault.join(&pointer), b"# corrupted\n").expect("corrupt");
        write_file(
            &vault.join("ref/papers/mismatch.md"),
            &note_with_pointer("Mismatch", &pointer),
        );
        let (output, value) = run_links_json(&vault, &["mismatch"]);
        assert_success(&output);
        assert_eq!(value["status"], Value::from("source_invalid"));
    }
}

#[test]
fn links_non_json_exits_cover_statuses_and_errors() {
    {
        let temp = TempDir::new("bob-cli-ref-links-human-err");
        let vault = base_vault(&temp);
        write_file(
            &vault.join("ref/papers/plain.md"),
            &note_without_pointer("Plain"),
        );
        let output = run_links(&vault, &["plain"]);
        assert_eq!(output.status.code(), Some(1));
        let error = stderr(&output);
        assert!(error.contains("no saved Markdown source"), "{error}");
        assert!(error.contains("hint: bob ref links"), "{error}");
    }

    {
        let temp = TempDir::new("bob-cli-ref-links-not-recorded-human");
        let vault = base_vault(&temp);
        let source = "# Old\n\nBody.\n";
        let pointer = write_store(&vault, "old", source, "");
        write_file(
            &vault.join("ref/papers/old.md"),
            &note_with_pointer("Old", &pointer),
        );
        let output = run_links(&vault, &["old"]);
        assert_eq!(output.status.code(), Some(1));
        let error = stderr(&output);
        assert!(
            error.contains("links were not recorded for this reference"),
            "{error}"
        );
        assert!(error.contains("bob ref recatalog"), "{error}");
    }

    {
        let temp = TempDir::new("bob-cli-ref-links-human-nolinks");
        let vault = base_vault(&temp);
        let source = "# Empty\n\nNo links.\n";
        let pointer = write_store(&vault, "empty", source, r#","links":[]"#);
        write_file(
            &vault.join("ref/papers/empty.md"),
            &note_with_pointer("Empty", &pointer),
        );
        let output = run_links(&vault, &["empty"]);
        assert_success(&output);
        assert_eq!(stdout(&output), "no links\n");
    }

    {
        let (_temp, vault, _pointer) =
            golden_vault("bob-cli-ref-links-missing-n");
        let output = run_links(&vault, &["demo", "99"]);
        assert_eq!(output.status.code(), Some(1));
        assert!(
            stderr(&output).contains("no link 99 (9 links)"),
            "{}",
            format_output(&output)
        );
    }

    {
        let (_temp, vault, _pointer) =
            golden_vault("bob-cli-ref-links-unknown");
        let output = run_links(&vault, &["does-not-exist"]);
        assert_eq!(output.status.code(), Some(1));

        write_file(
            &vault.join("ref/chat/demo.md"),
            &note_without_pointer("Demo copy"),
        );
        let ambiguous = run_links(&vault, &["demo"]);
        assert_eq!(
            ambiguous.status.code(),
            Some(1),
            "{}",
            format_output(&ambiguous)
        );

        let (json_output, value) = run_links_json(&vault, &["does-not-exist"]);
        assert_eq!(json_output.status.code(), Some(1));
        assert_eq!(value["ok"], Value::Bool(false));
    }

    {
        let (_temp, vault, _pointer) =
            golden_vault("bob-cli-ref-links-raw-usage");
        let output = run_links(&vault, &["demo", "-f", "raw"]);
        assert_eq!(output.status.code(), Some(2));
        assert!(
            stderr(&output).contains("--format raw requires a link number N"),
            "{}",
            format_output(&output)
        );
    }
}

#[test]
fn links_human_output_groups_sections_and_hints_shortest_ref() {
    let (_temp, vault, _pointer) = golden_vault("bob-cli-ref-links-human");
    let output = run_links(&vault, &["demo"]);
    assert_success(&output);
    assert_stdout_has_no_ansi(&output);
    let text = stdout(&output);
    assert!(text.starts_with("Demo\n"), "{text}");
    let second_line = text.lines().nth(1).unwrap_or_default();
    assert!(second_line.contains("ref/papers/demo.md"), "{text}");
    assert!(second_line.contains("9 links"), "{text}");
    assert!(second_line.contains("sites"), "{text}");
    assert!(text.contains("Image models"), "{text}");
    assert!(text.contains("Imagen 4 model card"), "{text}");
    assert!(text.contains("p. 7"), "{text}");
    assert!(text.contains("ai.google.dev"), "{text}");
    assert!(
        text.contains("hint: bob ref links demo 1 | pbcopy"),
        "{text}"
    );
}
