//! `bob ref attach-source`: verified backfill of saved Markdown sources.

use crate::support::*;
use std::{fs, path::Path, process::Output};

const REPORT: &str = r#"# Harbor Cranes

Intro paragraph about harbor cranes and shipping logistics in the bay
area harbor district, with observers watching every container move.

See [harbor details](https://example.test/harbor).

## Retrieval

```yaml
refresh:
  interval_ms: 250
  jitter_ms: 40
```

Some connecting prose about polling loops and backoff strategies for
crane scheduling systems across the terminal yards.

```rust
fn refresh_loop(state: &mut AppState) {
    state.tick();
}
```

Closing paragraph about deployment and monitoring of the harbor crane
scheduler service through the night shift rotations.
"#;

fn skip_without_render_tools(case: &str) -> bool {
    for tool in ["pandoc", "pdftotext", "xelatex"] {
        if std::process::Command::new(tool)
            .arg("--version")
            .output()
            .is_err()
        {
            eprintln!("skipping {case}: {tool} is required");
            return true;
        }
    }
    false
}

fn git_vault(temp: &TempDir) -> std::path::PathBuf {
    let vault = temp.path().join("vault");
    fs::create_dir_all(&vault).expect("create vault");
    git_in(&vault, ["init", "-q"]);
    configure_test_git_identity(&vault);
    vault
}

fn commit_all(vault: &Path, message: &str) {
    git_in(&vault, ["add", "-A"]);
    git_in(&vault, ["commit", "-q", "-m", message]);
}

fn render_library_pdf(report: &Path, pdf: &Path) {
    let output = std::process::Command::new("pandoc")
        .arg(report)
        .arg("-o")
        .arg(pdf)
        .arg("--pdf-engine=xelatex")
        .output()
        .expect("run pandoc render");
    assert!(
        output.status.success(),
        "pandoc render failed:\n{}",
        format_output(&output)
    );
}

/// A git vault with one filed PDF-backed note, its rendered library PDF,
/// and a same-stem candidate under `sidecar/`.
fn attached_fixture(
    prefix: &str,
) -> (TempDir, std::path::PathBuf, std::path::PathBuf) {
    let temp = TempDir::new(prefix);
    let vault = git_vault(&temp);
    let sidecar = temp.path().join("sidecar");
    fs::create_dir_all(sidecar.join("nested")).expect("sidecar dirs");
    fs::create_dir_all(vault.join("lib/chat")).expect("lib dir");
    fs::create_dir_all(vault.join("ref/chat")).expect("ref dir");
    write_file(&sidecar.join("nested/report.md"), REPORT);
    write_file(&sidecar.join("report.md"), REPORT);
    // Two same-stem candidates would read as ambiguous, so the nested
    // copy is removed; it only proves recursion is not required.
    fs::remove_file(sidecar.join("nested/report.md")).expect("dedup");
    render_library_pdf(
        &sidecar.join("report.md"),
        &vault.join("lib/chat/report.pdf"),
    );
    write_file(
        &vault.join("ref/chat/report.md"),
        "---\nid: harbor-cranes\ntitle: Harbor Cranes\nsource_pdf: \"[[lib/chat/report.pdf]]\"\n---\n\n# Harbor Cranes\n\n- [ ] Read [[lib/chat/report.pdf]] #task ^ref-report\n",
    );
    commit_all(&vault, "seed attach fixture");
    let search = sidecar;
    (temp, vault, search)
}

fn sha256(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    hex::encode(Sha256::digest(bytes))
}

/// Add an orphan render-time store for the fixture PDF and commit it as
/// pre-existing vault data. The sentinel page metadata makes any catalog
/// rewrite visible to the adoption test.
fn write_orphan_store(
    vault: &Path,
    store_stem: &str,
    source: &str,
    pdf_path: &str,
) -> (String, std::path::PathBuf, Vec<u8>) {
    let digest = sha256(source.as_bytes());
    let pointer = format!(
        ".bob/ref-sources/chat/{store_stem}--{}/source.md",
        &digest[..12]
    );
    let dir = vault
        .join(&pointer)
        .parent()
        .expect("store directory")
        .to_path_buf();
    fs::create_dir_all(&dir).expect("create store directory");
    write_file(&vault.join(&pointer), source);
    let pdf_file = vault.join(pdf_path);
    let pdf_bytes = if pdf_file.is_file() {
        fs::read(pdf_file).expect("read fixture PDF")
    } else {
        fs::read(vault.join("lib/chat/report.pdf")).expect("read fixture PDF")
    };
    let pdf_sha = sha256(&pdf_bytes);
    let code = serde_json::json!({
        "version": 1,
        "captured_at": "2026-10-26T12:00:00Z",
        "route": "markdown",
        "source": {
            "sha256": digest,
            "bytes": source.len(),
            "origin": "/tmp/report.md"
        },
        "pdf": { "path": pdf_path, "sha256": pdf_sha },
        "renderer": { "bob": "0.3.1", "pandoc": "test" },
        "blocks": [
            {
                "n": 1,
                "kind": "code",
                "lang": "yaml",
                "text": "refresh:\n  interval_ms: 250\n  jitter_ms: 40",
                "classes": ["yaml"],
                "attrs": {},
                "page": 7,
                "page_end": 7,
                "page_source": "anchor"
            },
            {
                "n": 2,
                "kind": "code",
                "lang": "rust",
                "text": "fn refresh_loop(state: &mut AppState) {\n    state.tick();\n}",
                "classes": ["rust"],
                "attrs": {},
                "page": 8,
                "page_end": 8,
                "page_source": "anchor"
            }
        ],
        "links": []
    });
    let code_path = dir.join("code.json");
    write_file(
        &code_path,
        &serde_json::to_string_pretty(&code).expect("serialize catalog"),
    );
    let bytes = fs::read(&code_path).expect("read seeded catalog");
    (pointer, code_path, bytes)
}

fn orphan_fixture(
    prefix: &str,
) -> (
    TempDir,
    std::path::PathBuf,
    std::path::PathBuf,
    String,
    std::path::PathBuf,
    Vec<u8>,
) {
    let (temp, vault, search) = attached_fixture(prefix);
    let (pointer, code_path, code_bytes) =
        write_orphan_store(&vault, "report", REPORT, "lib/chat/report.pdf");
    commit_all(&vault, "seed orphan saved source");
    (temp, vault, search, pointer, code_path, code_bytes)
}

/// Render and queue a real Markdown reference with `bob ref create`, then
/// seed a pointerless filed note against that exact PDF. This keeps the
/// render-time catalog and saved bytes produced by bob's normal toolchain.
fn exact_render_orphan_fixture(
    prefix: &str,
) -> (
    TempDir,
    std::path::PathBuf,
    String,
    std::path::PathBuf,
    Vec<u8>,
) {
    let temp = TempDir::new(prefix);
    let vault = git_vault(&temp);
    write_file(&vault.join(".gitignore"), "/xlib/\n");
    write_file(
        &vault.join("sase.md"),
        "---\ntype: \"[[area]]\"\n---\n\n# SASE\n",
    );
    commit_all(&vault, "seed real-render attach fixture");
    let stem = "machine_load_accounting_transparency";
    let source = temp.path().join(format!("{stem}.md"));
    let markdown = format!(
        "# Machine Load Accounting Transparency\n\n```text\nstale:    ⌂athena cap 5/8 ⟳41s              snapshot older than ~2 refresh cycles\n```\n\n```text\nrunner slot accounting preserves stable claims\n```\n\n```text\ncapacity displays explain live reservations\n```\n"
    );
    write_file(&source, &markdown);
    let output = bob_command()
        .arg("ref")
        .arg("create")
        .arg(&source)
        .arg("-P")
        .arg("sase")
        .arg("-b")
        .arg(&vault)
        .arg("-N")
        .arg(stem)
        .arg("-k")
        .env("BOB_DIR", &vault)
        .env("BOB_REF_INTAKE_LOCK_WAIT_SECONDS", "0")
        .env("BOB_REF_SCAN_LOCK_WAIT_SECONDS", "0")
        .env(
            "BOB_VAULT_SYNC_LOCK_FILE",
            temp.path().join("bob_sync.lock"),
        )
        .env(
            "BOB_VAULT_SYNC_STATE_FILE",
            temp.path().join("vault-sync.json"),
        )
        .env("XDG_STATE_HOME", temp.path().join("state"))
        .output()
        .expect("run bob ref create with real renderer");
    assert_success(&output);
    let create_report = stdout(&output);
    let pointer = create_report
        .lines()
        .find_map(|line| {
            line.trim()
                .strip_prefix("source_markdown:")
                .map(|value| value.trim().trim_matches('"').to_string())
        })
        .unwrap_or_else(|| {
            panic!("create did not report source pointer:\n{create_report}")
        });
    let queued_pdf = vault.join(format!("xlib/chat/{stem}.pdf"));
    assert!(queued_pdf.is_file(), "-k should leave a real PDF queued");
    let pdf_rel = format!("lib/chat/{stem}.pdf");
    let pdf_path = vault.join(&pdf_rel);
    fs::create_dir_all(pdf_path.parent().expect("PDF parent"))
        .expect("create library directory");
    fs::rename(&queued_pdf, &pdf_path).expect("move rendered PDF into library");

    let code_path = vault
        .join(&pointer)
        .parent()
        .expect("source store parent")
        .join("code.json");
    let mut code: serde_json::Value = serde_json::from_slice(
        &fs::read(&code_path).expect("read real render catalog"),
    )
    .expect("real render catalog parses");
    code["pdf"]["path"] = serde_json::Value::String(pdf_rel.clone());
    write_file(
        &code_path,
        serde_json::to_string_pretty(&code)
            .expect("serialize catalog with filed PDF path")
            .as_str(),
    );
    let code_before = fs::read(&code_path).expect("save catalog bytes");
    write_file(
        &vault.join(format!("ref/chat/{stem}.md")),
        &format!(
            "---\nid: {stem}\ntitle: Machine Load Accounting Transparency\nsource_pdf: \"[[{pdf_rel}]]\"\n---\n\n# Machine Load Accounting Transparency\n\n- [ ] Read [[{pdf_rel}]] #task ^ref-{stem}\n"
        ),
    );
    commit_all(&vault, "seed pointerless real-render reference");
    (temp, vault, pointer, code_path, code_before)
}

fn attach_command(
    temp: &TempDir,
    vault: &Path,
    args: &[&str],
) -> std::process::Command {
    let mut command = bob_command();
    command
        .arg("ref")
        .arg("attach-source")
        .args(args)
        .env("BOB_DIR", vault)
        .env("NO_COLOR", "1")
        .env(
            "BOB_VAULT_SYNC_LOCK_FILE",
            temp.path().join("bob_sync.lock"),
        )
        .env(
            "BOB_VAULT_SYNC_STATE_FILE",
            temp.path().join("vault-sync.json"),
        )
        .env("XDG_STATE_HOME", temp.path().join("state"));
    command
}

fn run_attach(temp: &TempDir, vault: &Path, args: &[&str]) -> Output {
    attach_command(temp, vault, args)
        .output()
        .expect("run attach-source")
}

fn head_subject(vault: &Path) -> String {
    let log = git_in(&vault, ["log", "--format=%s", "-1"]);
    String::from_utf8(log.stdout)
        .expect("log utf8")
        .trim()
        .to_string()
}

#[test]
fn attach_verifies_and_records_a_fixture_source() {
    if skip_without_render_tools("attach verifies a fixture source") {
        return;
    }
    let (temp, vault, search) = attached_fixture("bob-cli-attach-happy");
    let output =
        run_attach(&temp, &vault, &["-s", &search.to_string_lossy(), "report"]);
    assert_success(&output);
    let report = stdout(&output);
    assert!(
        report.contains("ref/chat/report.md: attached"),
        "attach line:\n{report}"
    );
    assert!(
        report.contains("summary: 1 attached, 0 skipped"),
        "summary:\n{report}"
    );
    let note = fs::read_to_string(vault.join("ref/chat/report.md"))
        .expect("read note");
    let pointer = note
        .lines()
        .find_map(|line| line.strip_prefix("source_markdown: "))
        .expect("pointer line");
    assert!(
        pointer.starts_with(".bob/ref-sources/chat/report--"),
        "pointer layout: {pointer}"
    );
    let store = vault.join(pointer);
    assert!(store.is_file(), "source.md stored");
    let code_path = store.parent().expect("parent").join("code.json");
    let code: serde_json::Value = serde_json::from_str(
        &fs::read_to_string(&code_path).expect("code.json"),
    )
    .expect("code.json parses");
    assert_eq!(code["route"], "attached");
    assert_eq!(code["version"], 1);
    let links = code["links"].as_array().expect("attached link records");
    assert_eq!(links.len(), 1);
    assert_eq!(links[0]["url"], "https://example.test/harbor");
    assert_eq!(links[0]["page"], 1);
    assert_eq!(links[0]["page_source"], "annotation");
    assert!(
        code["verified"]["matched"].as_u64().unwrap_or(0) >= 1,
        "verified counts: {code}"
    );
    for block in code["blocks"].as_array().expect("blocks array") {
        if block["kind"] == "code" {
            assert_eq!(block["page_source"], "text_match");
            assert!(block["page"].as_u64().unwrap_or(0) >= 1);
        }
    }
    assert_eq!(head_subject(&vault), "bob ref attach-source: 1 references");
    assert!(
        report.contains("committed "),
        "human commit diagnostic:\n{report}"
    );
    // The saved source feeds the read surface byte for byte.
    let code_out = bob_command()
        .arg("ref")
        .arg("code")
        .arg("report")
        .arg("-f")
        .arg("json")
        .env("BOB_DIR", &vault)
        .env("NO_COLOR", "1")
        .output()
        .expect("run ref code");
    assert_success(&code_out);
    let envelope: serde_json::Value =
        serde_json::from_str(&stdout(&code_out)).expect("code JSON parses");
    assert_eq!(envelope["status"], "available");
    assert_eq!(envelope["source"]["route"], "attached");
}

#[test]
fn attach_dry_run_verifies_but_writes_nothing() {
    if skip_without_render_tools("attach dry run") {
        return;
    }
    let (temp, vault, search) = attached_fixture("bob-cli-attach-dry");
    let before = head_subject(&vault);
    let output = run_attach(
        &temp,
        &vault,
        &["-s", &search.to_string_lossy(), "-d", "report"],
    );
    assert_success(&output);
    let report = stdout(&output);
    assert!(
        report.contains("ref/chat/report.md: attached"),
        "dry-run line:\n{report}"
    );
    assert!(report.contains("writes: none"), "dry-run writes:\n{report}");
    assert!(
        !vault.join(".bob/ref-sources").exists(),
        "dry run writes no store"
    );
    let note = fs::read_to_string(vault.join("ref/chat/report.md"))
        .expect("read note");
    assert!(!note.contains("source_markdown"), "note untouched");
    assert_eq!(head_subject(&vault), before, "no commit on dry run");
}

#[test]
fn attach_adopts_an_orphan_without_search_and_preserves_catalog() {
    if skip_without_render_tools("adopt orphan saved source") {
        return;
    }
    let (temp, vault, _search, pointer, code_path, code_before) =
        orphan_fixture("bob-cli-attach-adopt");
    let output = run_attach(&temp, &vault, &[]);
    assert_success(&output);
    let report = stdout(&output);
    assert!(
        report.contains("ref/chat/report.md: adopted (saved at create)"),
        "adoption output:\n{report}"
    );
    let note = fs::read_to_string(vault.join("ref/chat/report.md"))
        .expect("read adopted note");
    assert!(
        note.contains(&format!("source_markdown: {pointer}")),
        "{note}"
    );
    assert_eq!(
        fs::read(&code_path).expect("read code catalog after adoption"),
        code_before,
        "adoption must leave render-time code.json byte-identical"
    );
    assert_eq!(head_subject(&vault), "bob ref attach-source: 1 references");
    let changed = git_in(&vault, ["show", "--format=", "--name-only", "HEAD"]);
    let changed = String::from_utf8(changed.stdout).expect("commit paths utf8");
    assert!(changed.contains("ref/chat/report.md"), "{changed}");
    assert!(
        !changed.contains("code.json"),
        "catalog was rewritten: {changed}"
    );
    assert!(
        !changed.contains("source.md"),
        "source was rewritten: {changed}"
    );

    let code = bob_command()
        .arg("ref")
        .arg("code")
        .arg("report")
        .arg("-f")
        .arg("json")
        .env("BOB_DIR", &vault)
        .output()
        .expect("read adopted code catalog");
    assert_success(&code);
    let envelope: serde_json::Value =
        serde_json::from_str(&stdout(&code)).expect("code JSON parses");
    assert_eq!(envelope["status"], "available");
    assert_eq!(envelope["source"]["route"], "markdown");
    assert_eq!(envelope["blocks"][0]["page"], 7);
    assert_eq!(envelope["blocks"][0]["page_source"], "anchor");
}

#[test]
fn attach_adopts_an_exact_render_with_one_lost_pdf_glyph() {
    if skip_without_render_tools("real-render lost-glyph adoption") {
        return;
    }
    let (temp, vault, pointer, code_path, code_before) =
        exact_render_orphan_fixture("bob-cli-attach-real-render-glyph");
    let output = run_attach(&temp, &vault, &[]);
    assert_success(&output);
    assert!(
        stdout(&output).contains(
            "ref/chat/machine_load_accounting_transparency.md: adopted (saved at create)"
        ),
        "real-render adoption should verify every block:\n{}",
        stdout(&output)
    );
    let note = fs::read_to_string(
        vault.join("ref/chat/machine_load_accounting_transparency.md"),
    )
    .expect("read adopted real-render note");
    assert!(
        note.contains(&format!("source_markdown: {pointer}")),
        "{note}"
    );
    assert_eq!(
        fs::read(&code_path).expect("read render catalog after adoption"),
        code_before,
        "adoption preserves bob's actual render-time catalog bytes"
    );
    let code: serde_json::Value =
        serde_json::from_slice(&code_before).expect("real catalog JSON");
    assert_eq!(code["route"], "markdown");
    assert!(code["blocks"]
        .as_array()
        .is_some_and(|blocks| blocks.len() == 3));
}

#[test]
fn attach_adoption_dry_run_writes_nothing() {
    if skip_without_render_tools("adopt orphan dry run") {
        return;
    }
    let (temp, vault, _search, _pointer, code_path, code_before) =
        orphan_fixture("bob-cli-attach-adopt-dry");
    let before = head_subject(&vault);
    let output = run_attach(&temp, &vault, &["-d"]);
    assert_success(&output);
    let report = stdout(&output);
    assert!(
        report.contains("ref/chat/report.md: adopted (saved at create)"),
        "dry-run adoption:\n{report}"
    );
    assert!(report.contains("writes: none"), "{report}");
    let note = fs::read_to_string(vault.join("ref/chat/report.md"))
        .expect("read note");
    assert!(!note.contains("source_markdown"), "dry run changed note");
    assert_eq!(fs::read(&code_path).expect("read catalog"), code_before);
    assert_eq!(head_subject(&vault), before, "dry run must not commit");
}

#[test]
fn attach_named_reference_without_search_reports_no_saved_source() {
    if skip_without_render_tools("named reference without saved source") {
        return;
    }
    let (temp, vault, _search) = attached_fixture("bob-cli-attach-no-saved");
    let output = run_attach(&temp, &vault, &["report"]);
    assert_success(&output);
    assert!(
        stdout(&output)
            .contains("ref/chat/report.md: skipped (no saved source; pass -s)"),
        "named no-source output:\n{}",
        stdout(&output)
    );
}

#[test]
fn attach_without_refs_and_search_stays_silent_without_orphans() {
    if skip_without_render_tools("search-free run without orphan stores") {
        return;
    }
    let (temp, vault, _search) = attached_fixture("bob-cli-attach-adopt-empty");
    let output = run_attach(&temp, &vault, &[]);
    assert_success(&output);
    let report = stdout(&output);
    assert!(!report.contains("ref/chat/report.md"), "{report}");
    assert!(
        report.contains("summary: 0 attached, 0 skipped"),
        "{report}"
    );
}

#[test]
fn attach_keeps_missing_search_path_as_a_usage_error() {
    let temp = TempDir::new("bob-cli-attach-missing-search");
    let vault = git_vault(&temp);
    let missing = temp.path().join("does-not-exist");
    let output = run_attach(
        &temp,
        &vault,
        &["-s", &missing.to_string_lossy(), "report"],
    );
    assert_eq!(output.status.code(), Some(2), "{}", format_output(&output));
    assert!(
        stderr(&output).contains("search path not found"),
        "{}",
        format_output(&output)
    );
}

#[test]
fn attach_ambiguous_saved_sources_never_fall_through_to_search() {
    if skip_without_render_tools("ambiguous orphan saved sources") {
        return;
    }
    let (temp, vault, search) = attached_fixture("bob-cli-attach-adopt-amb");
    write_orphan_store(&vault, "report", REPORT, "lib/chat/report.pdf");
    write_orphan_store(
        &vault,
        "report--alternate",
        &format!("{REPORT}\nA second saved version.\n"),
        "lib/chat/report.pdf",
    );
    commit_all(&vault, "seed ambiguous orphan stores");
    let output =
        run_attach(&temp, &vault, &["-s", &search.to_string_lossy(), "report"]);
    assert_success(&output);
    let report = stdout(&output);
    assert!(
        report
            .contains("ref/chat/report.md: skipped (ambiguous saved sources)"),
        "ambiguous adoption:\n{report}"
    );
    let note = fs::read_to_string(vault.join("ref/chat/report.md"))
        .expect("read note");
    assert!(
        !note.contains("source_markdown"),
        "ambiguous source adopted"
    );
    assert_eq!(head_subject(&vault), "seed ambiguous orphan stores");
}

#[test]
fn attach_rejects_a_saved_store_claimed_by_another_note() {
    if skip_without_render_tools("saved store referenced by another note") {
        return;
    }
    let (temp, vault, _search, pointer, _code_path, _code_before) =
        orphan_fixture("bob-cli-attach-adopt-live-pointer");
    write_file(
        &vault.join("ref/chat/other.md"),
        &format!(
            "---\ntitle: Other\nsource_pdf: \"[[lib/chat/report.pdf]]\"\nsource_markdown: {pointer}\n---\n\n# Other\n"
        ),
    );
    commit_all(&vault, "point another note at saved source");
    let output = run_attach(&temp, &vault, &["report"]);
    assert_success(&output);
    assert!(
        stdout(&output)
            .contains("ref/chat/report.md: skipped (no saved source; pass -s)"),
        "live-pointer store must not be adopted:\n{}",
        stdout(&output)
    );
}

#[test]
fn attach_saved_source_with_wrong_pdf_path_falls_back_to_search() {
    if skip_without_render_tools("saved source with wrong PDF path") {
        return;
    }
    let (temp, vault, search) =
        attached_fixture("bob-cli-attach-adopt-pdf-path");
    write_orphan_store(&vault, "report", REPORT, "lib/chat/other.pdf");
    // A distinct source digest gives the verified candidate a new
    // content-addressed destination. The same-byte collision is refused in
    // its own regression below instead of refreshing this wrong-PDF catalog.
    write_file(
        &search.join("report.md"),
        format!("<!-- distinct source bytes -->\n{REPORT}").as_str(),
    );
    commit_all(&vault, "seed wrong-pdf orphan store");
    let output =
        run_attach(&temp, &vault, &["-s", &search.to_string_lossy(), "report"]);
    assert_success(&output);
    let report = stdout(&output);
    assert!(report.contains("ref/chat/report.md: attached"), "{report}");
    assert!(!report.contains("adopted (saved at create)"), "{report}");
    let note = fs::read_to_string(vault.join("ref/chat/report.md"))
        .expect("read attached note");
    let pointer = note
        .lines()
        .find_map(|line| line.strip_prefix("source_markdown: "))
        .expect("search fallback pointer");
    assert!(
        fs::read_to_string(vault.join(pointer))
            .expect("read selected source")
            .contains("fn refresh_loop"),
        "search fallback stores the candidate source"
    );
}

#[test]
fn attach_unverified_saved_source_reports_failure_then_uses_search() {
    if skip_without_render_tools("unverified saved source fallback") {
        return;
    }
    let (temp, vault, search) =
        attached_fixture("bob-cli-attach-adopt-mismatch");
    write_orphan_store(&vault, "report", DECOY, "lib/chat/report.pdf");
    commit_all(&vault, "seed mismatched orphan source");
    let output =
        run_attach(&temp, &vault, &["-s", &search.to_string_lossy(), "report"]);
    assert_success(&output);
    let report = stdout(&output);
    assert!(
        report.contains(
            "ref/chat/report.md: skipped (saved source does not match: low match"
        ),
        "saved-source verification failure:\n{report}"
    );
    assert!(report.contains("ref/chat/report.md: attached"), "{report}");
    let note = fs::read_to_string(vault.join("ref/chat/report.md"))
        .expect("read fallback note");
    let pointer = note
        .lines()
        .find_map(|line| line.strip_prefix("source_markdown: "))
        .expect("fallback pointer");
    assert_eq!(
        fs::read_to_string(vault.join(pointer)).expect("read attached source"),
        REPORT
    );
}

#[test]
fn attach_json_reports_failed_saved_source_before_search_fallback() {
    if skip_without_render_tools("JSON saved-source fallback") {
        return;
    }
    let (temp, vault, search) =
        attached_fixture("bob-cli-attach-adopt-json-fallback");
    write_orphan_store(&vault, "report", DECOY, "lib/chat/report.pdf");
    commit_all(&vault, "seed JSON mismatched orphan source");
    let output = run_attach(
        &temp,
        &vault,
        &[
            "-s",
            &search.to_string_lossy(),
            "-f",
            "json",
            "-d",
            "report",
        ],
    );
    assert_success(&output);
    let envelope: serde_json::Value =
        serde_json::from_str(&stdout(&output)).expect("attach JSON parses");
    assert_eq!(envelope["attached"], 1);
    let results = envelope["results"].as_array().expect("results array");
    assert_eq!(results.len(), 1);
    assert_eq!(results[0]["status"], "attached");
    let candidates = results[0]["candidates"]
        .as_array()
        .expect("candidate verdicts");
    assert_eq!(candidates.len(), 2);
    assert!(
        candidates[0]["verdict"]
            .as_str()
            .is_some_and(|verdict| verdict.starts_with("saved source does not match: low match")),
        "saved-source failure must be reported before the fallback: {candidates:?}"
    );
    assert_eq!(candidates[1]["verdict"], "verified");
}

#[test]
fn attach_skips_an_edited_candidate_below_threshold() {
    if skip_without_render_tools("attach low-match skip") {
        return;
    }
    let (temp, vault, search) = attached_fixture("bob-cli-attach-low");
    write_file(
        &search.join("report.md"),
        "# Harbor Cranes\n\nEntirely rewritten prose with none of the original code blocks remaining at all.\n",
    );
    git_in(&vault, ["status", "--short"]);
    let output =
        run_attach(&temp, &vault, &["-s", &search.to_string_lossy(), "report"]);
    assert_success(&output);
    let report = stdout(&output);
    assert!(
        report.contains("skipped (title mismatch)")
            || report.contains("skipped (low match"),
        "edited candidate skips:\n{report}"
    );
    assert!(
        !vault.join(".bob/ref-sources").exists(),
        "skipped attach writes no store"
    );
}

#[test]
fn attach_skips_ambiguous_and_title_mismatch_candidates() {
    if skip_without_render_tools("attach ambiguous skip") {
        return;
    }
    let (temp, vault, search) = attached_fixture("bob-cli-attach-amb");
    fs::create_dir_all(search.join("other")).expect("other dir");
    fs::copy(search.join("report.md"), search.join("other/report.md"))
        .expect("copy candidate");
    let output =
        run_attach(&temp, &vault, &["-s", &search.to_string_lossy(), "report"]);
    assert_success(&output);
    assert!(
        stdout(&output).contains(
            "ref/chat/report.md: skipped (ambiguous: 2 of 2 verified)"
        ),
        "ambiguous skip:\n{}",
        stdout(&output)
    );
    fs::remove_file(search.join("other/report.md")).expect("dedup");
    write_file(
        &search.join("report.md"),
        REPORT.replace("Harbor Cranes", "Desert Owls").as_str(),
    );
    let output =
        run_attach(&temp, &vault, &["-s", &search.to_string_lossy(), "report"]);
    assert_success(&output);
    assert!(
        stdout(&output)
            .contains("ref/chat/report.md: skipped (title mismatch)"),
        "title skip:\n{}",
        stdout(&output)
    );
}

#[test]
fn attach_reports_already_attached_and_snapshot_rows() {
    if skip_without_render_tools("attach eligibility skips") {
        return;
    }
    let (temp, vault, search) = attached_fixture("bob-cli-attach-elig");
    // A pointer note is out of scope even when named directly.
    let attached_note = vault.join("ref/chat/report.md");
    let mut contents = fs::read_to_string(&attached_note).expect("read note");
    contents = contents.replace(
        "source_pdf:",
        "source_markdown: .bob/ref-sources/chat/report--0123456789ab/source.md\nsource_pdf:",
    );
    write_file(&attached_note, &contents);
    // A live snapshot note is never attached from its current text.
    write_file(
        &vault.join("ref/chat/live.md"),
        "---\nid: live\ntitle: Harbor Cranes\nsource_pdf: \"[[lib/chat/report.pdf]]\"\nsource_note: zk/harbor\n---\n\n# Live\n",
    );
    commit_all(&vault, "seed eligibility notes");
    let output = run_attach(
        &temp,
        &vault,
        &["-s", &search.to_string_lossy(), "report", "live"],
    );
    assert_success(&output);
    let report = stdout(&output);
    assert!(
        report.contains("ref/chat/report.md: skipped (already attached)"),
        "attached skip:\n{report}"
    );
    assert!(
        report.contains("ref/chat/live.md: skipped (snapshot)"),
        "snapshot skip:\n{report}"
    );
}

/// Same title as the fixture note, but rewritten code: the title check
/// passes while block-match verification fails.
const DECOY: &str = r#"# Harbor Cranes

Intro paragraph about harbor cranes and shipping logistics in the bay
area harbor district, with observers watching every container move.

## Retrieval

```yaml
totally:
  rewritten_config: true
```

Some connecting prose about crane scheduling systems across the terminal yards.

```rust
fn unrelated_decoy() {
    println!("nothing like the source");
}
```

Closing paragraph about deployment and monitoring of the harbor crane
scheduler service through the night shift rotations.
"#;

#[test]
fn attach_attaches_single_passing_ambiguous_candidate() {
    if skip_without_render_tools("attach one-pass ambiguous stem") {
        return;
    }
    let (temp, vault, search) = attached_fixture("bob-cli-attach-onepass");
    fs::create_dir_all(search.join("other")).expect("other dir");
    write_file(&search.join("other/report.md"), DECOY);
    let output =
        run_attach(&temp, &vault, &["-s", &search.to_string_lossy(), "report"]);
    assert_success(&output);
    let report = stdout(&output);
    assert!(
        report.contains("ref/chat/report.md: attached (1 of 2 verified:"),
        "disambiguated attach:\n{report}"
    );
    assert!(
        report.contains(": verified") && report.contains("low match"),
        "per-candidate verdicts:\n{report}"
    );
    let note = fs::read_to_string(vault.join("ref/chat/report.md"))
        .expect("read note");
    let pointer = note
        .lines()
        .find_map(|line| line.strip_prefix("source_markdown: "))
        .expect("pointer line");
    let stored = fs::read_to_string(vault.join(pointer)).expect("source.md");
    assert_eq!(stored, REPORT, "attaches the passing candidate's bytes");
    assert_eq!(head_subject(&vault), "bob ref attach-source: 1 references");
}

#[test]
fn attach_skips_ambiguous_stem_when_none_verify() {
    if skip_without_render_tools("attach zero-pass ambiguous stem") {
        return;
    }
    let (temp, vault, search) = attached_fixture("bob-cli-attach-zeropass");
    fs::create_dir_all(search.join("other")).expect("other dir");
    write_file(&search.join("report.md"), DECOY);
    write_file(&search.join("other/report.md"), DECOY);
    let before = head_subject(&vault);
    let output = run_attach(
        &temp,
        &vault,
        &["-s", &search.to_string_lossy(), "-d", "report"],
    );
    assert_success(&output);
    let report = stdout(&output);
    assert!(
        report.contains(
            "ref/chat/report.md: skipped (ambiguous: 0 of 2 verified)"
        ),
        "zero-pass skip:\n{report}"
    );
    assert!(
        report.contains("low match"),
        "per-candidate verdicts:\n{report}"
    );
    assert!(report.contains("writes: none"), "dry-run writes:\n{report}");
    assert_eq!(head_subject(&vault), before, "no commit on dry run");
}

#[test]
fn attach_explicit_pair_attaches_despite_title_mismatch() {
    if skip_without_render_tools("attach explicit pair") {
        return;
    }
    let (temp, vault, search) = attached_fixture("bob-cli-attach-explicit");
    let (pointer, code_path, code_before) =
        write_orphan_store(&vault, "report", REPORT, "lib/chat/report.pdf");
    let note_path = vault.join("ref/chat/report.md");
    let note = fs::read_to_string(&note_path).expect("read fixture note");
    write_file(
        &note_path,
        note.replace("Harbor Cranes", "Renamed Fixture").as_str(),
    );
    let renamed = search.join("renamed.md");
    write_file(&renamed, REPORT);
    commit_all(&vault, "seed explicit-pair orphan catalog");
    // The renamed file alone is not a same-stem candidate, so the stem
    // search finds nothing; the explicit pair still attaches it.
    let output = run_attach(
        &temp,
        &vault,
        &["report", "-s", &renamed.to_string_lossy()],
    );
    assert_success(&output);
    let report = stdout(&output);
    assert!(
        report.contains(
            "ref/chat/report.md: skipped (saved source does not match: title mismatch)"
        ),
        "ordinary adoption should retain the note-title check:\n{report}"
    );
    assert!(
        report.contains("ref/chat/report.md: adopted (saved at create)"),
        "the explicit pair should verify and reuse the render-time store:\n{report}"
    );
    assert!(
        report.contains("summary: 1 attached, 0 skipped"),
        "explicit pair should complete the attachment:\n{report}"
    );
    let note = fs::read_to_string(&note_path).expect("read note");
    assert!(
        note.lines()
            .any(|line| line == format!("source_markdown: {pointer}")),
        "note gains a pointer"
    );
    assert_eq!(
        fs::read(&code_path).expect("catalog remains render-time bytes"),
        code_before,
        "explicit same-byte fallback adopts without rewriting code.json"
    );
    let code = serde_json::from_slice::<serde_json::Value>(&code_before)
        .expect("seed catalog JSON parses");
    assert_eq!(code["route"], "markdown");
    assert_eq!(code["blocks"][0]["page_source"], "anchor");
    let changed = git_in(&vault, ["show", "--format=", "--name-only", "HEAD"]);
    let changed = String::from_utf8(changed.stdout).expect("commit paths utf8");
    assert!(changed.contains("ref/chat/report.md"), "{changed}");
    assert!(!changed.contains("source.md"), "{changed}");
    assert!(!changed.contains("code.json"), "{changed}");
}

#[test]
fn attach_refuses_same_byte_candidate_when_existing_store_is_claimed() {
    if skip_without_render_tools("claimed store destination collision") {
        return;
    }
    let (temp, vault, _search) =
        attached_fixture("bob-cli-attach-claimed-collision");
    let (pointer, code_path, code_before) =
        write_orphan_store(&vault, "report", REPORT, "lib/chat/report.pdf");
    let note_path = vault.join("ref/chat/report.md");
    let note = fs::read_to_string(&note_path).expect("read fixture note");
    write_file(
        &note_path,
        note.replace("Harbor Cranes", "Renamed Fixture").as_str(),
    );
    write_file(
        &vault.join("ref/chat/other.md"),
        &format!(
            "---\ntitle: Other\nsource_pdf: \"[[lib/chat/report.pdf]]\"\nsource_markdown: {pointer}\n---\n\n# Other\n"
        ),
    );
    let candidate = temp.path().join("renamed.md");
    write_file(&candidate, REPORT);
    commit_all(&vault, "seed claimed destination collision");
    let before = head_subject(&vault);
    let output = run_attach(
        &temp,
        &vault,
        &["report", "-s", &candidate.to_string_lossy()],
    );
    assert_success(&output);
    assert!(
        stdout(&output).contains(
            "skipped (store destination collision: destination is claimed"
        ),
        "collision report:\n{}",
        stdout(&output)
    );
    assert_eq!(head_subject(&vault), before);
    assert_eq!(
        fs::read(&code_path).expect("catalog unchanged"),
        code_before
    );
    assert!(
        !fs::read_to_string(&note_path)
            .expect("read pointerless note")
            .contains("source_markdown:"),
        "refusal must not write the note"
    );
}

#[test]
fn attach_refuses_same_byte_candidate_when_existing_store_has_wrong_pdf() {
    if skip_without_render_tools("wrong-PDF store destination collision") {
        return;
    }
    let (temp, vault, _search) =
        attached_fixture("bob-cli-attach-wrong-pdf-collision");
    let (_pointer, code_path, code_before) =
        write_orphan_store(&vault, "report", REPORT, "lib/chat/other.pdf");
    let note_path = vault.join("ref/chat/report.md");
    let note = fs::read_to_string(&note_path).expect("read fixture note");
    write_file(
        &note_path,
        note.replace("Harbor Cranes", "Renamed Fixture").as_str(),
    );
    let candidate = temp.path().join("renamed.md");
    write_file(&candidate, REPORT);
    commit_all(&vault, "seed wrong-PDF destination collision");
    let before = head_subject(&vault);
    let output = run_attach(
        &temp,
        &vault,
        &["report", "-s", &candidate.to_string_lossy()],
    );
    assert_success(&output);
    assert!(
        stdout(&output).contains("skipped (store destination collision: destination belongs to a different PDF)"),
        "collision report:\n{}",
        stdout(&output)
    );
    assert_eq!(head_subject(&vault), before);
    assert_eq!(
        fs::read(&code_path).expect("catalog unchanged"),
        code_before
    );
    assert!(
        !fs::read_to_string(&note_path)
            .expect("read pointerless note")
            .contains("source_markdown:"),
        "refusal must not write the note"
    );
}

#[test]
fn attach_refuses_same_byte_candidate_when_existing_catalog_is_invalid() {
    if skip_without_render_tools("invalid store destination collision") {
        return;
    }
    let (temp, vault, _search) =
        attached_fixture("bob-cli-attach-invalid-collision");
    let (_pointer, code_path, _code_before) =
        write_orphan_store(&vault, "report", REPORT, "lib/chat/report.pdf");
    write_file(&code_path, "{ invalid catalog");
    let note_path = vault.join("ref/chat/report.md");
    let note = fs::read_to_string(&note_path).expect("read fixture note");
    write_file(
        &note_path,
        note.replace("Harbor Cranes", "Renamed Fixture").as_str(),
    );
    let candidate = temp.path().join("renamed.md");
    write_file(&candidate, REPORT);
    commit_all(&vault, "seed invalid destination collision");
    let before = head_subject(&vault);
    let code_before = fs::read(&code_path).expect("read invalid catalog");
    let output = run_attach(
        &temp,
        &vault,
        &["report", "-s", &candidate.to_string_lossy()],
    );
    assert_success(&output);
    assert!(
        stdout(&output).contains("skipped (store destination collision:"),
        "collision report:\n{}",
        stdout(&output)
    );
    assert_eq!(head_subject(&vault), before);
    assert_eq!(
        fs::read(&code_path).expect("catalog unchanged"),
        code_before
    );
    assert!(
        !fs::read_to_string(&note_path)
            .expect("read pointerless note")
            .contains("source_markdown:"),
        "refusal must not write the note"
    );
}

#[test]
fn attach_json_write_mode_keeps_adoption_stdout_parseable() {
    if skip_without_render_tools("JSON adoption write") {
        return;
    }
    let (temp, vault, _search, _pointer, _code_path, _code_before) =
        orphan_fixture("bob-cli-attach-json-adopt-write");
    let output = run_attach(&temp, &vault, &["-f", "json"]);
    assert_success(&output);
    let envelope: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("entire stdout is JSON");
    assert_eq!(envelope["attached"], 1);
    assert_eq!(envelope["results"][0]["status"], "adopted");
    assert!(stderr(&output).contains("committed "));
}

#[test]
fn attach_json_write_mode_keeps_search_attachment_stdout_parseable() {
    if skip_without_render_tools("JSON attachment write") {
        return;
    }
    let (temp, vault, search) =
        attached_fixture("bob-cli-attach-json-search-write");
    let output = run_attach(
        &temp,
        &vault,
        &["-s", &search.to_string_lossy(), "-f", "json", "report"],
    );
    assert_success(&output);
    let envelope: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("entire stdout is JSON");
    assert_eq!(envelope["attached"], 1);
    assert_eq!(envelope["results"][0]["status"], "attached");
    assert!(stderr(&output).contains("committed "));
}

#[test]
fn attach_json_names_every_candidate_verdict() {
    if skip_without_render_tools("attach JSON verdicts") {
        return;
    }
    let (temp, vault, search) = attached_fixture("bob-cli-attach-json");
    fs::create_dir_all(search.join("other")).expect("other dir");
    fs::copy(search.join("report.md"), search.join("other/report.md"))
        .expect("copy candidate");
    let output = run_attach(
        &temp,
        &vault,
        &[
            "-s",
            &search.to_string_lossy(),
            "-d",
            "-f",
            "json",
            "report",
        ],
    );
    assert_success(&output);
    let envelope: serde_json::Value =
        serde_json::from_str(&stdout(&output)).expect("attach JSON parses");
    assert_eq!(envelope["command"], "ref attach-source");
    assert_eq!(envelope["dry_run"], true);
    assert_eq!(envelope["attached"], 0);
    let results = envelope["results"].as_array().expect("results array");
    assert_eq!(results.len(), 1);
    assert_eq!(results[0]["status"], "skipped");
    let candidates = results[0]["candidates"]
        .as_array()
        .expect("candidates array");
    assert_eq!(candidates.len(), 2, "every candidate named");
    for candidate in candidates {
        assert_eq!(candidate["verdict"], "verified");
        assert!(candidate["matched"].as_u64().unwrap_or(0) >= 1);
    }
}
