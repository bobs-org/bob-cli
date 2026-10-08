//! `bob ref find` batch identity lookup: verdicts, match kinds, intake,
//! formats, errors, and side-effect freedom over the fixture vault.

use crate::support::*;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Output;

const FIXED_NOW: &str = "2026-10-06 12:00:00";

fn fixture_vault(prefix: &str) -> (TempDir, PathBuf) {
    let temp = TempDir::new(prefix);
    let vault = temp.path().join("vault");
    copy_dir(&fixture("ref_library/vault"), &vault);
    (temp, vault)
}

fn copy_dir(source: &Path, target: &Path) {
    fs::create_dir_all(target)
        .unwrap_or_else(|error| panic!("create {}: {error}", target.display()));
    let entries = fs::read_dir(source)
        .unwrap_or_else(|error| panic!("read {}: {error}", source.display()));
    for entry in entries {
        let entry = entry.expect("dir entry");
        let child_target = target.join(entry.file_name());
        if entry.path().is_dir() {
            copy_dir(&entry.path(), &child_target);
        } else {
            fs::copy(entry.path(), &child_target).unwrap_or_else(|error| {
                panic!("copy {}: {error}", entry.path().display())
            });
        }
    }
}

fn run_find(vault: &Path, args: &[&str]) -> Output {
    bob_command()
        .arg("ref")
        .arg("find")
        .args(args)
        .env("BOB_DIR", vault)
        .env("BOB_NOW", FIXED_NOW)
        .output()
        .expect("run bob ref find")
}

fn run_find_json(vault: &Path, args: &[&str]) -> serde_json::Value {
    let mut full = args.to_vec();
    full.extend(["-f", "json"]);
    let output = run_find(vault, &full);
    assert_success(&output);
    assert_stdout_has_no_ansi(&output);
    serde_json::from_str(&stdout(&output)).unwrap_or_else(|error| {
        panic!("parse find JSON: {error}\n{}", format_output(&output))
    })
}

fn result_for(document: &serde_json::Value, query: &str) -> serde_json::Value {
    document["results"]
        .as_array()
        .expect("results array")
        .iter()
        .find(|result| result["query"] == query)
        .unwrap_or_else(|| panic!("missing result for {query}"))
        .clone()
}

fn vault_snapshot(vault: &Path) -> Vec<(String, Vec<u8>)> {
    let mut files = Vec::new();
    walk_snapshot(vault, vault, &mut files);
    files.sort_by(|left, right| left.0.cmp(&right.0));
    files
}

fn walk_snapshot(root: &Path, path: &Path, files: &mut Vec<(String, Vec<u8>)>) {
    let entries = fs::read_dir(path)
        .unwrap_or_else(|error| panic!("read {}: {error}", path.display()));
    for entry in entries {
        let entry = entry.expect("dir entry");
        let child = entry.path();
        if child.is_dir() {
            walk_snapshot(root, &child, files);
        } else {
            let relative = child
                .strip_prefix(root)
                .expect("child under root")
                .to_string_lossy()
                .into_owned();
            let contents = fs::read(&child).expect("read file");
            files.push((relative, contents));
        }
    }
}

#[test]
fn find_url_variants_share_one_identity() {
    let (_temp, vault) = fixture_vault("bob-cli-ref-find-url");
    let document = run_find_json(
        &vault,
        &[
            "https://arxiv.org/abs/1706.03762",
            "https://arxiv.org/pdf/1706.03762",
            "https://arxiv.org/html/1706.03762v1",
            "1706.03762",
            "arxiv:1706.03762v2",
        ],
    );
    for query in [
        "https://arxiv.org/abs/1706.03762",
        "https://arxiv.org/pdf/1706.03762",
        "https://arxiv.org/html/1706.03762v1",
        "1706.03762",
        "arxiv:1706.03762v2",
    ] {
        let result = result_for(&document, query);
        assert_eq!(result["verdict"], "in_library", "query {query}");
        assert_eq!(result["reading_state"], "finished", "query {query}");
        assert_eq!(
            result["matches"][0]["ref"]["path"], "ref/papers/arxiv_pdf.md",
            "query {query}"
        );
    }
    let kinds: Vec<&str> = document["results"]
        .as_array()
        .expect("results array")
        .iter()
        .map(|result| result["query_kind"].as_str().expect("query_kind string"))
        .collect();
    assert_eq!(kinds, ["url", "url", "url", "arxiv", "arxiv"]);
}

#[test]
fn find_www_and_slash_variants_with_superseded_secondary() {
    let (_temp, vault) = fixture_vault("bob-cli-ref-find-www");
    let document =
        run_find_json(&vault, &["https://example.com/blog/capture-flows"]);
    let result =
        result_for(&document, "https://example.com/blog/capture-flows");
    assert_eq!(result["verdict"], "in_library");
    assert_eq!(result["reading_state"], "finished");
    // The legacy `www.` + trailing-slash twin is superseded, not hidden.
    let paths: Vec<&str> = result["matches"]
        .as_array()
        .expect("matches array")
        .iter()
        .map(|hit| hit["ref"]["path"].as_str().expect("path string"))
        .collect();
    assert_eq!(
        paths,
        ["ref/blogs/clipped_blog.md", "ref/ai/legacy_unread.md"],
    );
    assert_eq!(
        result["matches"][1]["ref"]["superseded_by"],
        "ref/blogs/clipped_blog.md",
    );
    assert_eq!(
        result["matches"][0]["matched_key"],
        "https://example.com/blog/capture-flows",
    );
}

#[test]
fn find_doi_and_arxiv_doi_queries() {
    let (_temp, vault) = fixture_vault("bob-cli-ref-find-doi");
    let document = run_find_json(
        &vault,
        &[
            "10.48550/arXiv.1706.03762",
            "doi:10.48550/arXiv.1706.03762",
            "https://doi.org/10.48550/arXiv.1706.03762",
        ],
    );
    for query in [
        "10.48550/arXiv.1706.03762",
        "doi:10.48550/arXiv.1706.03762",
        "https://doi.org/10.48550/arXiv.1706.03762",
    ] {
        let result = result_for(&document, query);
        assert_eq!(result["verdict"], "in_library", "query {query}");
        // Both twin notes match through the shared `arxiv:` key, in
        // primary-match order: finished first, then queued.
        let paths = result["matches"]
            .as_array()
            .expect("matches array")
            .iter()
            .map(|hit| hit["ref"]["path"].as_str().expect("path"))
            .collect::<Vec<_>>();
        assert_eq!(
            paths,
            vec!["ref/papers/arxiv_pdf.md", "ref/papers/arxiv_doi.md"],
            "query {query}",
        );
        assert_eq!(result["matches"][0]["match_kind"], "identity");
    }
    assert_eq!(
        document["results"].as_array().expect("results")[0]["query_kind"],
        "doi",
    );
    assert_eq!(
        document["results"].as_array().expect("results")[2]["query_kind"],
        "url",
    );
}

#[test]
fn find_path_source_pdf_stem_and_id_queries() {
    let (_temp, vault) = fixture_vault("bob-cli-ref-find-path");
    fs::write(
        vault.join("ref/papers/noted_id.md"),
        "---\nstatus: ready\ntitle: Identified Note\nid: identified-note-7\n---\n\n# Identified Note\n\n- [ ] #task #ref [[lib/papers/identified.pdf]] #hide ^ref\n",
    )
    .expect("write id fixture note");
    let document = run_find_json(
        &vault,
        &[
            "ref/papers/synced_paper.md",
            "lib/blogs/capture-flows.pdf",
            "synced_paper",
            "dup_stem",
            "identified-note-7",
        ],
    );
    let path = result_for(&document, "ref/papers/synced_paper.md");
    assert_eq!(path["verdict"], "in_library");
    assert_eq!(path["matches"][0]["match_kind"], "path");
    assert_eq!(
        path["matches"][0]["matched_key"],
        "ref/papers/synced_paper.md",
    );
    let pdf = result_for(&document, "lib/blogs/capture-flows.pdf");
    assert_eq!(pdf["verdict"], "in_library");
    assert_eq!(pdf["matches"][0]["match_kind"], "source_pdf");
    assert_eq!(
        pdf["matches"][0]["ref"]["path"],
        "ref/blogs/clipped_blog.md",
    );
    let stem = result_for(&document, "synced_paper");
    assert_eq!(stem["verdict"], "in_library");
    assert_eq!(stem["matches"][0]["match_kind"], "stem");
    // Duplicate stems in two directories both match; find never picks one.
    let dup = result_for(&document, "dup_stem");
    assert_eq!(dup["verdict"], "in_library");
    let dup_paths: Vec<&str> = dup["matches"]
        .as_array()
        .expect("matches array")
        .iter()
        .map(|hit| hit["ref"]["path"].as_str().expect("path string"))
        .collect();
    assert_eq!(
        dup_paths,
        ["ref/blogs/dup_stem.md", "ref/papers/dup_stem.md"],
    );
    let id = result_for(&document, "identified-note-7");
    assert_eq!(id["verdict"], "in_library");
    assert_eq!(id["matches"][0]["match_kind"], "id");
    assert_eq!(id["matches"][0]["matched_key"], "identified-note-7");
}

#[test]
fn find_go_link_keys_stay_opaque() {
    let (_temp, vault) = fixture_vault("bob-cli-ref-find-go");
    let document = run_find_json(&vault, &["go_link"]);
    let result = result_for(&document, "go_link");
    assert_eq!(result["verdict"], "in_library");
    let keys = result["matches"][0]["ref"]["identity"]["keys"]
        .as_array()
        .expect("keys array");
    assert!(
        keys.iter().any(|key| key == "raw:go/capture-thing"),
        "go/ value keeps its raw: key: {keys:?}",
    );
}

#[test]
fn find_title_exact_and_title_candidates() {
    let (_temp, vault) = fixture_vault("bob-cli-ref-find-title");
    let document = run_find_json(
        &vault,
        &["attention is all you need", "attention you need extra"],
    );
    let exact = result_for(&document, "attention is all you need");
    assert_eq!(exact["verdict"], "possible");
    assert!(exact["reading_state"].is_null());
    assert_eq!(exact["candidates"][0]["match_kind"], "title_exact");
    assert_eq!(exact["candidates"][0]["score"], 100);
    assert_eq!(exact["query_kind"], "title");
    // A title match is a candidate, never proof of not having read.
    assert!(exact["matches"].as_array().expect("matches").is_empty());
    let partial = result_for(&document, "attention you need extra");
    assert_eq!(partial["verdict"], "possible");
    assert_eq!(partial["candidates"][0]["match_kind"], "title");
}

#[test]
fn find_slug_fallback_for_url_miss() {
    let (_temp, vault) = fixture_vault("bob-cli-ref-find-slug");
    let document = run_find_json(
        &vault,
        &["https://example.com/x/capture-flows-guide", "-m", "50"],
    );
    let result =
        result_for(&document, "https://example.com/x/capture-flows-guide");
    assert_eq!(result["verdict"], "possible");
    assert!(result["matches"].as_array().expect("matches").is_empty());
    assert_eq!(result["candidates"][0]["match_kind"], "slug_title");
    let strict =
        run_find_json(&vault, &["https://example.com/x/capture-flows-guide"]);
    assert_eq!(
        result_for(&strict, "https://example.com/x/capture-flows-guide")
            ["verdict"],
        "not_found",
        "default --min-score filters the slug candidate",
    );
}

#[test]
fn find_min_score_filters_title_candidates() {
    let (_temp, vault) = fixture_vault("bob-cli-ref-find-min");
    let loose = run_find_json(&vault, &["attention need"]);
    assert_eq!(result_for(&loose, "attention need")["verdict"], "possible",);
    let strict = run_find_json(&vault, &["attention need", "-m", "80"]);
    assert_eq!(
        result_for(&strict, "attention need")["verdict"],
        "not_found",
        "candidates below --min-score behave as no match",
    );
    let invalid = run_find(&vault, &["x", "-m", "0"]);
    assert_eq!(
        invalid.status.code(),
        Some(2),
        "--min-score 0 is a usage error"
    );
    let invalid_high = run_find(&vault, &["x", "-m", "101"]);
    assert_eq!(
        invalid_high.status.code(),
        Some(2),
        "--min-score 101 is a usage error",
    );
}

#[test]
fn find_batch_from_stdin_keeps_order_and_duplicates() {
    let (_temp, vault) = fixture_vault("bob-cli-ref-find-stdin");
    let mut command = bob_command();
    command
        .arg("ref")
        .arg("find")
        .arg("-")
        .arg("-f")
        .arg("json")
        .env("BOB_DIR", &vault)
        .env("BOB_NOW", FIXED_NOW);
    let output = run_with_stdin(
        &mut command,
        "https://arxiv.org/abs/1706.03762\nattention is all you need\nhttps://example.com/nope\nhttps://arxiv.org/abs/1706.03762\n",
    );
    assert_success(&output);
    let document: serde_json::Value =
        serde_json::from_str(&stdout(&output)).expect("parse stdin batch JSON");
    let queries: Vec<&str> = document["results"]
        .as_array()
        .expect("results array")
        .iter()
        .map(|result| result["query"].as_str().expect("query string"))
        .collect();
    assert_eq!(
        queries,
        [
            "https://arxiv.org/abs/1706.03762",
            "attention is all you need",
            "https://example.com/nope",
            "https://arxiv.org/abs/1706.03762",
        ],
    );
    assert_eq!(document["summary"]["queries"], 4);
    assert_eq!(document["summary"]["in_library"], 2);
    assert_eq!(document["summary"]["possible"], 1);
    assert_eq!(document["summary"]["not_found"], 1);
    assert_eq!(document["summary"]["finished"], 2);
}

#[test]
fn find_intake_hit_with_marker_pdf() {
    let (_temp, vault) = fixture_vault("bob-cli-ref-find-intake");
    write_highlights_pdf(
        &vault.join("xlib/blogs/intake-only.pdf"),
        "- status: ready\n- parent: tech_blogs\n- title: Intake Only Article\n- source_url: https://example.com/intake/only-article\n",
    );
    let without =
        run_find_json(&vault, &["https://example.com/intake/only-article"]);
    assert_eq!(
        result_for(&without, "https://example.com/intake/only-article")
            ["verdict"],
        "not_found",
        "intake is invisible without -i",
    );
    assert_eq!(without["coverage"]["intake"], "not_checked");
    let with = run_find_json(
        &vault,
        &["https://example.com/intake/only-article", "-i"],
    );
    let result = result_for(&with, "https://example.com/intake/only-article");
    assert_eq!(result["verdict"], "in_intake");
    assert!(result["reading_state"].is_null());
    assert_eq!(with["coverage"]["intake"], "checked");
    assert_eq!(with["summary"]["in_intake"], 1);
    assert_eq!(result["intake"][0]["path"], "xlib/blogs/intake-only.pdf");
    assert_eq!(
        result["intake"][0]["source_url"],
        "https://example.com/intake/only-article",
    );
    assert_eq!(result["intake"][0]["title"], "Intake Only Article");
    assert_eq!(result["intake"][0]["status"], "ready");
}

#[test]
fn find_intake_records_both_marker_url_fields() {
    let (_temp, vault) = fixture_vault("bob-cli-ref-find-intake-both-urls");
    write_highlights_pdf(
        &vault.join("xlib/blogs/both-urls.pdf"),
        "- status: ready\n- parent: tech_blogs\n- title: Both Urls Article\n- source_url: https://example.com/intake/primary-article\n- url: https://example.com/intake/legacy-alias\n",
    );
    // Each marker URL field resolves through `find -i`, so a marker
    // carrying both is found on either value.
    for query in [
        "https://example.com/intake/primary-article",
        "https://example.com/intake/legacy-alias",
    ] {
        let document = run_find_json(&vault, &[query, "-i"]);
        let result = result_for(&document, query);
        assert_eq!(result["verdict"], "in_intake", "query {query}");
        assert_eq!(
            result["intake"][0]["path"], "xlib/blogs/both-urls.pdf",
            "query {query}",
        );
    }
}

#[test]
fn find_intake_unavailable_without_intake_dir() {
    let (_temp, vault) = fixture_vault("bob-cli-ref-find-nointake");
    let document = run_find_json(
        &vault,
        &["https://example.com/nope", "-i", "-x", "nope-xlib"],
    );
    assert_eq!(
        result_for(&document, "https://example.com/nope")["verdict"],
        "not_found",
    );
    assert_eq!(document["coverage"]["intake"], "unavailable");
}

#[test]
fn find_not_found_still_exits_zero() {
    let (_temp, vault) = fixture_vault("bob-cli-ref-find-zero");
    let output = run_find(&vault, &["https://example.com/definitely-absent"]);
    assert_success(&output);
    let human = stdout(&output);
    assert!(human.contains("· NOT FOUND"), "missing chip:\n{human}");
    assert!(
        human.contains("0 of 1 in library"),
        "missing footer:\n{human}"
    );
}

#[test]
fn find_missing_ref_dir_fails_in_both_formats() {
    let (_temp, vault) = fixture_vault("bob-cli-ref-find-nodir");
    let human = run_find(&vault, &["x", "-r", "nope-ref"]);
    assert_eq!(human.status.code(), Some(1));
    let human_stderr = stderr(&human);
    assert!(
        human_stderr
            .contains("bob ref: error: reference directory not found:",),
        "missing human error:\n{}",
        format_output(&human),
    );
    assert!(
        human_stderr.contains(
            "hint: pass -b/--bob-dir or -r/--ref-dir, or set BOB_DIR",
        ),
        "missing hint:\n{}",
        format_output(&human),
    );
    let json = run_find(&vault, &["x", "-r", "nope-ref", "-f", "json"]);
    assert_eq!(json.status.code(), Some(1));
    let document: serde_json::Value =
        serde_json::from_str(&stdout(&json)).expect("parse error JSON");
    assert_eq!(document["ok"], false);
    assert_eq!(document["schema_version"], 1);
    assert_eq!(document["command"], "ref find");
    assert_eq!(document["error"]["code"], "missing_ref_dir");
}

#[test]
fn find_json_envelope_is_stable_and_honest() {
    let (_temp, vault) = fixture_vault("bob-cli-ref-find-shape");
    let first = run_find(&vault, &["go_link", "-f", "json"]);
    let second = run_find(&vault, &["go_link", "-f", "json"]);
    assert_success(&first);
    assert_eq!(
        stdout(&first),
        stdout(&second),
        "JSON must be byte-stable under BOB_NOW",
    );
    let document: serde_json::Value =
        serde_json::from_str(&stdout(&first)).expect("parse find JSON");
    assert_eq!(document["ok"], true);
    assert_eq!(document["schema_version"], 1);
    assert_eq!(document["command"], "ref find");
    assert_eq!(document["generated_at"], "2026-10-06T12:00:00");
    assert_eq!(document["coverage"]["ref_dir"], "ref");
    assert_eq!(document["coverage"]["notes"], 33);
    assert_eq!(document["coverage"]["skipped"], 3);
    assert!(document["coverage"]["scope"]
        .as_str()
        .expect("scope string")
        .contains("never proof"));
    // Library counts exclude the superseded legacy twin.
    assert_eq!(document["library"]["notes"], 32);
    // The vault-root hub is membership, never an index row.
    let hub = run_find_json(&vault, &["hub_ref", "Reference Hub"]);
    for query in ["hub_ref", "Reference Hub"] {
        let result = result_for(&hub, query);
        for hit in result["matches"]
            .as_array()
            .expect("matches")
            .iter()
            .chain(result["candidates"].as_array().expect("candidates"))
        {
            let path = hit
                .get("ref")
                .and_then(|row| row.get("path"))
                .and_then(serde_json::Value::as_str)
                .unwrap_or("");
            assert!(
                !path.contains("hub_ref"),
                "vault-root hub must never be indexed: {path}",
            );
        }
    }
    // Rows carry machine-stable diagnostics in JSON.
    let bad = run_find_json(&vault, &["bad_yaml"]);
    let bad_result = result_for(&bad, "bad_yaml");
    assert_eq!(bad_result["verdict"], "in_library");
    let codes: Vec<&str> = bad_result["matches"][0]["ref"]["diagnostics"]
        .as_array()
        .expect("diagnostics array")
        .iter()
        .map(|diagnostic| diagnostic["code"].as_str().expect("code string"))
        .collect();
    assert!(
        codes.contains(&"invalid_yaml"),
        "expected invalid_yaml: {codes:?}",
    );
}

#[test]
fn find_human_blocks_carry_chips_and_footnotes() {
    let (_temp, vault) = fixture_vault("bob-cli-ref-find-human");
    let output = run_find(
        &vault,
        &[
            "https://arxiv.org/pdf/1706.03762",
            "go_link",
            "attention is all you need",
            "https://example.com/definitely-absent",
        ],
    );
    assert_success(&output);
    assert_stdout_has_no_ansi(&output);
    let human = stdout(&output);
    assert!(human.starts_with("bob ref find · 4 queries · ref/ (33 notes)\n"));
    assert!(human.contains("✓ FINISHED"), "missing chip:\n{human}");
    assert!(human.contains("○ QUEUED"), "missing chip:\n{human}");
    assert!(human.contains("≈ POSSIBLE"), "missing chip:\n{human}");
    assert!(human.contains("· NOT FOUND"), "missing chip:\n{human}");
    assert!(
        human.contains("also ref/papers/arxiv_doi.md (modern, ready)"),
        "missing secondary line:\n{human}",
    );
    assert!(
        human.contains("read 2026-09-02"),
        "missing dated suffix:\n{human}",
    );
    assert!(
        human.contains("2 of 4 in library (1 finished · 1 queued)"),
        "missing footer:\n{human}",
    );
}

#[test]
fn find_pending_sync_marks_status_with_footnote() {
    let (_temp, vault) = fixture_vault("bob-cli-ref-find-pending");
    let output = run_find(&vault, &["pending_note"]);
    assert_success(&output);
    let human = stdout(&output);
    assert!(human.contains("READ*"), "missing pending star:\n{human}");
    assert!(
        human.contains("* status changed since the last scan"),
        "missing footnote:\n{human}",
    );
}

#[test]
fn find_conflict_and_unknown_states() {
    let (_temp, vault) = fixture_vault("bob-cli-ref-find-states");
    let output = run_find(&vault, &["conflict_note", "bad_yaml"]);
    assert_success(&output);
    let human = stdout(&output);
    assert!(human.contains("? UNKNOWN"), "missing chip:\n{human}");
    assert!(human.contains("CONFLICT"), "missing status:\n{human}");
    let document = run_find_json(&vault, &["conflict_note", "bad_yaml"]);
    assert_eq!(
        result_for(&document, "conflict_note")["reading_state"],
        "unknown",
    );
}

#[test]
fn find_markdown_table_and_coverage_line() {
    let (_temp, vault) = fixture_vault("bob-cli-ref-find-md");
    let output = run_find(
        &vault,
        &[
            "https://arxiv.org/pdf/1706.03762",
            "https://example.com/nope",
            "-f",
            "markdown",
        ],
    );
    assert_success(&output);
    assert_stdout_has_no_ansi(&output);
    let markdown = stdout(&output);
    assert!(
        markdown
            .starts_with("| Query | Verdict | Reading state | Reference |\n",),
        "missing table header:\n{markdown}",
    );
    assert!(
        markdown.contains(
            "| https://arxiv.org/pdf/1706.03762 | In library | finished | [[ref/papers/arxiv_pdf]] Attention Is All You Need |",
        ),
        "missing library row:\n{markdown}",
    );
    assert!(
        markdown.contains("| https://example.com/nope | Not found | — |  |",),
        "missing not-found row:\n{markdown}",
    );
    assert!(
        markdown.contains(
            "\n\nLibrary check: 1 of 2 in library (1 finished) · coverage: ref/ only\n",
        ),
        "blank line before the coverage line:\n{markdown}",
    );
}

#[test]
fn find_alias_matches_canonical() {
    let (_temp, vault) = fixture_vault("bob-cli-ref-find-alias");
    for args in [
        vec!["find", "--help"],
        vec!["find", "https://arxiv.org/pdf/1706.03762"],
        vec!["find", "https://arxiv.org/pdf/1706.03762", "-f", "json"],
        vec!["find", "--min-score", "0", "x"],
    ] {
        let canonical = bob_command()
            .arg("ref")
            .args(&args)
            .env("BOB_DIR", &vault)
            .env("BOB_NOW", FIXED_NOW)
            .output()
            .expect("run canonical find");
        let aliased = bob_command()
            .arg("highlights")
            .args(&args)
            .env("BOB_DIR", &vault)
            .env("BOB_NOW", FIXED_NOW)
            .output()
            .expect("run alias find");
        assert_eq!(
            canonical.status.code(),
            aliased.status.code(),
            "exit code for {args:?}",
        );
        assert_eq!(stdout(&canonical), stdout(&aliased), "stdout for {args:?}",);
        assert_eq!(stderr(&canonical), stderr(&aliased), "stderr for {args:?}",);
        let combined =
            format!("{}\n{}", stdout(&canonical), stderr(&canonical));
        assert!(
            !combined.to_lowercase().contains("deprecat"),
            "no deprecation text for {args:?}",
        );
    }
}

#[test]
fn find_leaves_the_vault_untouched() {
    let (_temp, vault) = fixture_vault("bob-cli-ref-find-clean");
    let before = vault_snapshot(&vault);
    for args in [
        vec!["https://arxiv.org/pdf/1706.03762"],
        vec!["attention", "-f", "json"],
        vec!["x", "-f", "markdown"],
        vec!["https://example.com/nope", "-i"],
    ] {
        let output = run_find(&vault, &args);
        assert_success(&output);
    }
    assert_eq!(
        vault_snapshot(&vault),
        before,
        "find must never write to the vault",
    );
}
