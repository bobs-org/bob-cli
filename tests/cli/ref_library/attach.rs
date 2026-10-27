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
    let renamed = search.join("renamed.md");
    write_file(
        &renamed,
        REPORT.replace("Harbor Cranes", "Desert Owls").as_str(),
    );
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
        report.contains("ref/chat/report.md: attached"),
        "explicit attach:\n{report}"
    );
    let note = fs::read_to_string(vault.join("ref/chat/report.md"))
        .expect("read note");
    assert!(
        note.lines()
            .any(|line| line.starts_with("source_markdown: ")),
        "note gains a pointer"
    );
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
