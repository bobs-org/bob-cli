//! `bob ref code` read surface: JSON golden, raw/fenced/page/source
//! selection, all six statuses, error envelopes, human output, and summaries.

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

/// Write a store directory for `source` with the given raw block objects.
/// Returns the vault-relative `source.md` pointer.
fn write_store(
    vault: &Path,
    stem: &str,
    source: &str,
    blocks_json: &[String],
    extra: &str,
) -> String {
    let digest = sha256_hex_str(source.as_bytes());
    let hash12 = digest[..12].to_string();
    let dir = vault.join(format!(".bob/ref-sources/papers/{stem}--{hash12}"));
    fs::create_dir_all(&dir).expect("create store dir");
    write_file(&dir.join("source.md"), source);
    let blocks = blocks_json.join(",");
    write_file(
        &dir.join("code.json"),
        &format!(
            r#"{{"version":1,"captured_at":"2026-10-20T00:00:00Z","route":"markdown","source":{{"sha256":"{digest}","bytes":{},"origin":"/tmp/{stem}.md"}},"pdf":{{"path":"lib/papers/{stem}.pdf","sha256":"{}"}},"renderer":{{"bob":"0","pandoc":"test"}}{extra},"blocks":[{blocks}]}}"#,
            source.len(),
            "0".repeat(64)
        ),
    );
    format!(".bob/ref-sources/papers/{stem}--{hash12}/source.md")
}

fn block_json(
    n: u32,
    kind: &str,
    lang: Option<&str>,
    text: &str,
    section_number: Option<&str>,
    section_title: &str,
    page: u32,
    page_end: u32,
    attrs: &str,
) -> String {
    let lang_json = match lang {
        Some(lang) => format!("\"{lang}\""),
        None => "null".to_string(),
    };
    let section_json = match section_number {
        Some(number) => format!(
            r#","section":{{"number":"{number}","title":"{section_title}"}}"#
        ),
        None => {
            format!(r#","section":{{"number":null,"title":"{section_title}"}}"#)
        }
    };
    let escaped = text
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
        .replace('\t', "\\t");
    format!(
        r#"{{"n":{n},"kind":"{kind}","lang":{lang_json},"text":"{escaped}","classes":[],"attrs":{attrs},"page":{page},"page_end":{page_end},"page_source":"anchor"{section_json}}}"#
    )
}

fn run_code(vault: &Path, args: &[&str]) -> Output {
    bob_command()
        .arg("ref")
        .arg("code")
        .args(args)
        .env("BOB_DIR", vault)
        .env("BOB_NOW", FIXED_NOW)
        .output()
        .expect("run bob ref code")
}

fn run_code_json(vault: &Path, args: &[&str]) -> (Output, Value) {
    let mut full = args.to_vec();
    full.extend(["-f", "json"]);
    let output = run_code(vault, &full);
    let value =
        serde_json::from_str(&stdout(&output)).unwrap_or_else(|error| {
            panic!("parse code JSON: {error}\n{}", format_output(&output))
        });
    (output, value)
}

fn golden_vault(prefix: &str) -> (TempDir, std::path::PathBuf, String) {
    let temp = TempDir::new(prefix);
    let vault = base_vault(&temp);
    let source = "# Demo\n\nBody text.\n";
    let blocks = vec![
        block_json(
            1,
            "code",
            Some("rust"),
            "fn main() {}",
            Some("1"),
            "Setup",
            2,
            2,
            "{}",
        ),
        block_json(
            2,
            "code",
            Some("yaml"),
            "refresh:\n  interval_ms: 250",
            Some("2"),
            "Root cause",
            7,
            7,
            "{}",
        ),
        block_json(
            3,
            "code",
            Some("rust"),
            "fn tick() {}",
            Some("2"),
            "Root cause",
            8,
            9,
            "{}",
        ),
    ];
    let pointer = write_store(&vault, "demo", source, &blocks, "");
    write_file(
        &vault.join("ref/papers/demo.md"),
        &note_with_pointer("Demo", &pointer),
    );
    (temp, vault, pointer)
}

#[test]
fn code_json_golden_pins_envelope_and_block_fields() {
    let (_temp, vault, pointer) = golden_vault("bob-cli-ref-code-golden");
    let (output, mut value) = run_code_json(&vault, &["demo"]);
    assert_success(&output);
    assert_stdout_has_no_ansi(&output);
    // Normalize the timestamp for a stable golden.
    value["generated_at"] = Value::String("NORMALIZED".to_string());

    let digest = sha256_hex_str("# Demo\n\nBody text.\n".as_bytes());
    let hash12 = &digest[..12];
    let source_len = "# Demo\n\nBody text.\n".len();
    assert_eq!(value["ok"], Value::Bool(true));
    assert_eq!(value["schema_version"], Value::from(1));
    assert_eq!(value["command"], Value::from("ref code"));
    assert_eq!(
        value["reference"]["path"],
        Value::from("ref/papers/demo.md")
    );
    assert_eq!(value["reference"]["title"], Value::from("Demo"));
    assert_eq!(value["status"], Value::from("available"));
    assert!(value["detail"].is_null(), "{value}");
    assert_eq!(value["source"]["path"], Value::from(pointer.clone()));
    assert_eq!(value["source"]["sha256"], Value::from(digest.clone()));
    assert_eq!(value["source"]["bytes"], Value::from(source_len as u64));
    assert_eq!(value["source"]["captured"], Value::from("2026-10-20"));
    assert_eq!(value["source"]["route"], Value::from("markdown"));
    assert_eq!(
        value["source"]["text"],
        Value::from("# Demo\n\nBody text.\n")
    );

    let blocks = value["blocks"].as_array().expect("blocks array");
    assert_eq!(blocks.len(), 3);
    assert_eq!(blocks[0]["id"], Value::from(format!("{hash12}/1")));
    assert_eq!(blocks[0]["n"], Value::from(1));
    assert_eq!(blocks[0]["kind"], Value::from("code"));
    assert_eq!(blocks[0]["lang"], Value::from("rust"));
    assert_eq!(blocks[0]["lines"], Value::from(1));
    assert_eq!(blocks[0]["bytes"], Value::from("fn main() {}".len() as u64));
    assert_eq!(blocks[0]["text"], Value::from("fn main() {}"));
    assert_eq!(
        blocks[0]["fenced"],
        Value::from("```rust\nfn main() {}\n```\n")
    );
    assert_eq!(blocks[0]["section"]["number"], Value::from("1"));
    assert_eq!(blocks[0]["section"]["title"], Value::from("Setup"));
    assert_eq!(blocks[0]["page"], Value::from(2));
    assert_eq!(blocks[0]["page_end"], Value::from(2));
    assert_eq!(blocks[0]["page_source"], Value::from("anchor"));

    // Every block carries exactly the documented key set, matching
    // bob-mac-capture's Tests/Fixtures/refs-code.json envelope keys.
    let expected_block_keys = [
        "id",
        "n",
        "kind",
        "lang",
        "title",
        "summary",
        "caption",
        "lines",
        "bytes",
        "text",
        "fenced",
        "section",
        "page",
        "page_end",
        "page_source",
    ];
    for block in blocks {
        let mut keys: Vec<&str> = block
            .as_object()
            .expect("block object")
            .keys()
            .map(String::as_str)
            .collect();
        keys.sort_unstable();
        let mut expected = expected_block_keys.to_vec();
        expected.sort_unstable();
        assert_eq!(keys, expected, "{block}");
    }
    let mut envelope_keys: Vec<&str> = value
        .as_object()
        .expect("envelope object")
        .keys()
        .map(String::as_str)
        .collect();
    envelope_keys.sort_unstable();
    assert_eq!(
        envelope_keys,
        [
            "blocks",
            "command",
            "detail",
            "generated_at",
            "ok",
            "reference",
            "schema_version",
            "source",
            "status"
        ]
    );
    let mut source_keys: Vec<&str> = value["source"]
        .as_object()
        .expect("source object")
        .keys()
        .map(String::as_str)
        .collect();
    source_keys.sort_unstable();
    assert_eq!(
        source_keys,
        ["bytes", "captured", "path", "route", "sha256", "text"]
    );
}

#[test]
fn code_raw_bytes_match_exact_block_text() {
    let temp = TempDir::new("bob-cli-ref-code-raw");
    let vault = base_vault(&temp);
    // Leading and mid-line tabs, trailing spaces, Unicode, no trailing newline.
    let text = "line\tone  \n\tindented\tn\u{00e9}xt \u{1f600}  ";
    let source = "# Raw\n\nBody.\n";
    let blocks = vec![block_json(
        1,
        "code",
        Some("text"),
        text,
        Some("1"),
        "Body",
        3,
        3,
        "{}",
    )];
    let pointer = write_store(&vault, "rawnote", source, &blocks, "");
    write_file(
        &vault.join("ref/papers/rawnote.md"),
        &note_with_pointer("Raw", &pointer),
    );
    let output = run_code(&vault, &["rawnote", "1"]);
    assert_success(&output);
    assert_eq!(
        output.stdout,
        text.as_bytes(),
        "raw stdout must equal text byte for byte"
    );
}

#[test]
fn code_fenced_uses_longer_fence_for_backtick_runs() {
    let temp = TempDir::new("bob-cli-ref-code-fenced");
    let vault = base_vault(&temp);
    let text = "before\n```\ninside\n```\nafter";
    let source = "# Fenced\n\nBody.\n";
    let blocks = vec![block_json(
        1,
        "code",
        Some("rust"),
        text,
        Some("1"),
        "Body",
        4,
        4,
        "{}",
    )];
    let pointer = write_store(&vault, "fenced", source, &blocks, "");
    write_file(
        &vault.join("ref/papers/fenced.md"),
        &note_with_pointer("Fenced", &pointer),
    );
    let output = run_code(&vault, &["fenced", "1", "-f", "markdown"]);
    assert_success(&output);
    let expected = format!("````rust\n{text}\n````\n");
    assert_eq!(stdout(&output), expected);

    let (json_output, value) = run_code_json(&vault, &["fenced", "1"]);
    assert_success(&json_output);
    let blocks = value["blocks"].as_array().expect("blocks");
    assert_eq!(blocks.len(), 1);
    assert_eq!(blocks[0]["n"], Value::from(1));
}

#[test]
fn code_page_filter_keeps_spanning_blocks() {
    let (_temp, vault, _pointer) = golden_vault("bob-cli-ref-code-page");
    // Block 2 is page 7..7, block 3 is 8..9.
    let (_output, value) = run_code_json(&vault, &["demo", "-p", "8"]);
    let blocks = value["blocks"].as_array().expect("blocks");
    assert_eq!(blocks.len(), 1);
    assert_eq!(blocks[0]["n"], Value::from(3));

    let (_output, value) = run_code_json(&vault, &["demo", "-p", "7"]);
    let blocks = value["blocks"].as_array().expect("blocks");
    assert_eq!(blocks.len(), 1);
    assert_eq!(blocks[0]["n"], Value::from(2));

    // A spanning block (8..9) contains page 9 as well.
    let (_output, value) = run_code_json(&vault, &["demo", "-p", "9"]);
    let blocks = value["blocks"].as_array().expect("blocks");
    assert_eq!(blocks.len(), 1);
    assert_eq!(blocks[0]["n"], Value::from(3));
}

#[test]
fn code_source_prints_exact_bytes_and_rejects_combinations() {
    let (_temp, vault, _pointer) = golden_vault("bob-cli-ref-code-source");
    let output = run_code(&vault, &["demo", "-s"]);
    assert_success(&output);
    assert_eq!(stdout(&output), "# Demo\n\nBody text.\n");

    for args in [
        vec!["demo", "-s", "1"],
        vec!["demo", "-s", "-p", "7"],
        vec!["demo", "-s", "-f", "json"],
    ] {
        let output = run_code(&vault, &args);
        assert_eq!(output.status.code(), Some(2), "{args:?}");
    }
}

#[test]
fn code_all_six_statuses_in_json() {
    // available
    let (_temp, vault, _pointer) = golden_vault("bob-cli-ref-code-available");
    let (output, value) = run_code_json(&vault, &["demo"]);
    assert_success(&output);
    assert_eq!(value["status"], Value::from("available"));
    assert!(value["source"]["text"].is_string());

    // no_code: verified, zero blocks.
    {
        let temp = TempDir::new("bob-cli-ref-code-no-code");
        let vault = base_vault(&temp);
        let source = "# Empty\n\nNo blocks.\n";
        let pointer = write_store(&vault, "empty", source, &[], "");
        write_file(
            &vault.join("ref/papers/empty.md"),
            &note_with_pointer("Empty", &pointer),
        );
        let (output, value) = run_code_json(&vault, &["empty"]);
        assert_success(&output);
        assert_eq!(value["status"], Value::from("no_code"));
        assert!(value["source"]["text"].is_string());
    }

    // no_catalog: missing code.json.
    {
        let temp = TempDir::new("bob-cli-ref-code-no-catalog");
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
        let (output, value) = run_code_json(&vault, &["missing"]);
        assert_success(&output);
        assert_eq!(value["status"], Value::from("no_catalog"));
        assert!(value["source"]["text"].is_string());
    }

    // no_catalog: catalog_error recorded.
    {
        let temp = TempDir::new("bob-cli-ref-code-catalog-error");
        let vault = base_vault(&temp);
        let source = "# Error\n\nBody.\n";
        let pointer = write_store(
            &vault,
            "carderr",
            source,
            &[],
            r#","catalog_error":"pandoc failed""#,
        );
        write_file(
            &vault.join("ref/papers/carderr.md"),
            &note_with_pointer("Err", &pointer),
        );
        let (output, value) = run_code_json(&vault, &["carderr"]);
        assert_success(&output);
        assert_eq!(value["status"], Value::from("no_catalog"));
        assert!(value["source"]["text"].is_string());
    }

    // no_source: no pointer.
    {
        let temp = TempDir::new("bob-cli-ref-code-no-source");
        let vault = base_vault(&temp);
        write_file(
            &vault.join("ref/papers/plain.md"),
            &note_without_pointer("Plain"),
        );
        let (output, value) = run_code_json(&vault, &["plain"]);
        assert_success(&output);
        assert_eq!(value["status"], Value::from("no_source"));
        assert!(value["source"].is_null(), "{value}");
    }

    // source_missing: pointer set, source.md absent.
    {
        let temp = TempDir::new("bob-cli-ref-code-source-missing");
        let vault = base_vault(&temp);
        let pointer =
            ".bob/ref-sources/papers/gone--aaaaaaaaaaaa/source.md".to_string();
        write_file(
            &vault.join("ref/papers/gone.md"),
            &note_with_pointer("Gone", &pointer),
        );
        let (output, value) = run_code_json(&vault, &["gone"]);
        assert_success(&output);
        assert_eq!(value["status"], Value::from("source_missing"));
        assert!(value["source"]["text"].is_null(), "{value}");
    }

    // source_invalid: digest mismatch.
    {
        let temp = TempDir::new("bob-cli-ref-code-mismatch");
        let vault = base_vault(&temp);
        let source = "# Mismatch\n\nBody.\n";
        let pointer = write_store(
            &vault,
            "mismatch",
            source,
            &[block_json(
                1,
                "code",
                Some("text"),
                "x",
                Some("1"),
                "B",
                1,
                1,
                "{}",
            )],
            "",
        );
        fs::write(vault.join(&pointer), b"# corrupted\n").expect("corrupt");
        write_file(
            &vault.join("ref/papers/mismatch.md"),
            &note_with_pointer("Mismatch", &pointer),
        );
        let (output, value) = run_code_json(&vault, &["mismatch"]);
        assert_success(&output);
        assert_eq!(value["status"], Value::from("source_invalid"));
        assert!(value["source"]["text"].is_null(), "{value}");
    }

    // source_invalid: unsupported version.
    {
        let temp = TempDir::new("bob-cli-ref-code-bad-version");
        let vault = base_vault(&temp);
        let source = "# Version\n\nBody.\n";
        let digest = sha256_hex_str(source.as_bytes());
        let hash12 = &digest[..12];
        let dir =
            vault.join(format!(".bob/ref-sources/papers/badver--{hash12}"));
        fs::create_dir_all(&dir).expect("store dir");
        write_file(&dir.join("source.md"), source);
        write_file(
            &dir.join("code.json"),
            &format!(
                r#"{{"version":99,"captured_at":"2026-10-20T00:00:00Z","route":"markdown","source":{{"sha256":"{digest}","bytes":{},"origin":"/tmp/x.md"}},"pdf":{{"path":"lib/papers/x.pdf","sha256":"{}"}},"renderer":{{"bob":"0","pandoc":"test"}},"blocks":[]}}"#,
                source.len(),
                "0".repeat(64)
            ),
        );
        let pointer =
            format!(".bob/ref-sources/papers/badver--{hash12}/source.md");
        write_file(
            &vault.join("ref/papers/badver.md"),
            &note_with_pointer("BadVer", &pointer),
        );
        let (output, value) = run_code_json(&vault, &["badver"]);
        assert_success(&output);
        assert_eq!(value["status"], Value::from("source_invalid"));
    }

    // source_invalid: pointer outside .bob/ref-sources.
    {
        let temp = TempDir::new("bob-cli-ref-code-outside-pointer");
        let vault = base_vault(&temp);
        write_file(
            &vault.join("ref/papers/outside.md"),
            &note_with_pointer("Outside", "lib/papers/outside.md"),
        );
        let (output, value) = run_code_json(&vault, &["outside"]);
        assert_success(&output);
        assert_eq!(value["status"], Value::from("source_invalid"));
    }
}

#[test]
fn code_non_json_exits_cover_statuses_and_errors() {
    // A non-available status in human output exits 1 with a reason and hint.
    {
        let temp = TempDir::new("bob-cli-ref-code-human-err");
        let vault = base_vault(&temp);
        write_file(
            &vault.join("ref/papers/plain.md"),
            &note_without_pointer("Plain"),
        );
        let output = run_code(&vault, &["plain"]);
        assert_eq!(output.status.code(), Some(1));
        let error = stderr(&output);
        assert!(error.contains("no saved Markdown source"), "{error}");
        assert!(error.contains("hint: bob ref code"), "{error}");
    }

    // no_code with human output prints `no code blocks` and exits 0.
    {
        let temp = TempDir::new("bob-cli-ref-code-human-nocode");
        let vault = base_vault(&temp);
        let source = "# Empty\n\nNo blocks.\n";
        let pointer = write_store(&vault, "empty", source, &[], "");
        write_file(
            &vault.join("ref/papers/empty.md"),
            &note_with_pointer("Empty", &pointer),
        );
        let output = run_code(&vault, &["empty"]);
        assert_success(&output);
        assert_eq!(stdout(&output), "no code blocks\n");
    }

    // A missing block number reports the count and exits 1.
    {
        let (_temp, vault, _pointer) =
            golden_vault("bob-cli-ref-code-missing-n");
        let output = run_code(&vault, &["demo", "7"]);
        assert_eq!(output.status.code(), Some(1));
        assert!(
            stderr(&output).contains("no block 7 (3 blocks)"),
            "{}",
            format_output(&output)
        );
    }

    // Unknown and ambiguous references exit 1; JSON uses the show envelope.
    {
        let (_temp, vault, _pointer) = golden_vault("bob-cli-ref-code-unknown");
        let output = run_code(&vault, &["does-not-exist"]);
        assert_eq!(output.status.code(), Some(1));

        write_file(
            &vault.join("ref/chat/demo.md"),
            &note_without_pointer("Demo copy"),
        );
        let ambiguous = run_code(&vault, &["demo"]);
        assert_eq!(
            ambiguous.status.code(),
            Some(1),
            "{}",
            format_output(&ambiguous)
        );

        let (json_output, value) = run_code_json(&vault, &["does-not-exist"]);
        assert_eq!(json_output.status.code(), Some(1));
        assert_eq!(value["ok"], Value::Bool(false));
    }
}

#[test]
fn code_human_output_groups_sections_and_hints_shortest_ref() {
    let (_temp, vault, _pointer) = golden_vault("bob-cli-ref-code-human");
    let output = run_code(&vault, &["demo"]);
    assert_success(&output);
    assert_stdout_has_no_ansi(&output);
    let text = stdout(&output);
    assert!(text.starts_with("Demo\n"), "{text}");
    let second_line = text.lines().nth(1).unwrap_or_default();
    assert!(second_line.contains("ref/papers/demo.md"), "{text}");
    assert!(second_line.contains("3 blocks"), "{text}");
    assert!(second_line.contains("captured 2026-10-20"), "{text}");
    assert!(text.contains("Setup"), "{text}");
    assert!(text.contains("Root cause"), "{text}");
    assert!(text.contains("p. 7"), "{text}");
    assert!(
        text.contains("hint: bob ref code demo 2 | pbcopy"),
        "{text}"
    );
}

#[test]
fn code_summaries_skip_noise_and_read_mermaid_frontmatter() {
    let temp = TempDir::new("bob-cli-ref-code-summary");
    let vault = base_vault(&temp);
    let source = "# Summary\n\nBody.\n";
    let mermaid_text = "---\ntitle: Overview\n---\nflowchart LR\n  A --> B";
    let noisy_text = "\n---\n***\n[]\n\u{2500}\u{2500}\u{2500}\nreal summary\n";
    let blocks = vec![
        block_json(
            1,
            "mermaid",
            Some("mermaid"),
            mermaid_text,
            Some("4"),
            "Overview",
            9,
            9,
            "{}",
        ),
        block_json(
            2,
            "code",
            Some("text"),
            noisy_text,
            Some("5"),
            "Noise",
            10,
            10,
            r#"{"title":"  "}"#,
        ),
    ];
    let pointer = write_store(&vault, "summary", source, &blocks, "");
    write_file(
        &vault.join("ref/papers/summary.md"),
        &note_with_pointer("Summary", &pointer),
    );
    let (_output, value) = run_code_json(&vault, &["summary"]);
    let blocks = value["blocks"].as_array().expect("blocks");
    assert_eq!(blocks[0]["kind"], Value::from("mermaid"));
    assert_eq!(blocks[0]["summary"], Value::from("flowchart LR"));
    assert_eq!(blocks[0]["caption"], Value::from("Overview"));
    assert_eq!(blocks[1]["summary"], Value::from("real summary"));
}
