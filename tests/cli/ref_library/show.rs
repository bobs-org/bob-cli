//! `bob ref show` exact resolution: metadata, annotations, notes, and
//! tasks in human, Markdown, and JSON output over the fixture vault.

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

/// A scratch vault with exactly the given `ref/…` notes.
fn inline_vault(prefix: &str, notes: &[(&str, &str)]) -> (TempDir, PathBuf) {
    let temp = TempDir::new(prefix);
    let vault = temp.path().join("vault");
    for (relative, contents) in notes {
        let path = vault.join(relative);
        fs::create_dir_all(path.parent().expect("note parent"))
            .expect("create note parent");
        write_file(&path, contents);
    }
    (temp, vault)
}

fn run_show(vault: &Path, args: &[&str]) -> Output {
    bob_command()
        .arg("ref")
        .arg("show")
        .args(args)
        .env("BOB_DIR", vault)
        .env("BOB_NOW", FIXED_NOW)
        .output()
        .expect("run bob ref show")
}

fn run_show_json(vault: &Path, args: &[&str]) -> serde_json::Value {
    let mut full = args.to_vec();
    full.extend(["-f", "json"]);
    let output = run_show(vault, &full);
    assert_success(&output);
    assert_stdout_has_no_ansi(&output);
    serde_json::from_str(&stdout(&output)).unwrap_or_else(|error| {
        panic!("parse show JSON: {error}\n{}", format_output(&output))
    })
}

fn shown_paths(document: &serde_json::Value) -> Vec<String> {
    document["refs"]
        .as_array()
        .expect("refs array")
        .iter()
        .map(|row| row["path"].as_str().expect("path string").to_string())
        .collect()
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
fn show_resolves_every_exact_kind() {
    let (_temp, vault) = fixture_vault("bob-cli-ref-show-kinds");
    // Note: the 1706.03762 arXiv twins are both PDF-backed, so an
    // arXiv query for them is ambiguous by design (covered separately
    // by the ambiguous-stem test); identity URLs use unique keys here.
    let document = run_show_json(
        &vault,
        &[
            "clipped_blog",
            "ref/blogs/clipped_blog.md",
            "ref/blogs/clipped_blog",
            "https://example.com/papers/synced-paper",
            "Duplicate Stem in Blogs",
        ],
    );
    assert_eq!(
        shown_paths(&document),
        [
            "ref/blogs/clipped_blog.md",
            "ref/blogs/clipped_blog.md",
            "ref/blogs/clipped_blog.md",
            "ref/papers/synced_paper.md",
            "ref/blogs/dup_stem.md",
        ],
    );
    assert_eq!(document["ok"], true);
    assert_eq!(document["schema_version"], 1);
    assert_eq!(document["command"], "ref show");
    assert_eq!(document["generated_at"], "2026-10-06T12:00:00");
}

#[test]
fn show_superseded_url_picks_the_live_note_with_also() {
    let (_temp, vault) = fixture_vault("bob-cli-ref-show-also");
    let document =
        run_show_json(&vault, &["https://example.com/blog/capture-flows"]);
    let row = &document["refs"][0];
    assert_eq!(row["path"], "ref/blogs/clipped_blog.md");
    assert_eq!(row["also"], serde_json::json!(["ref/ai/legacy_unread.md"]));

    let output = run_show(&vault, &["https://example.com/blog/capture-flows"]);
    assert_success(&output);
    let human = stdout(&output);
    assert!(
        human
            .contains("also: ref/ai/legacy_unread.md (superseded legacy note)"),
        "dim also line:\n{human}"
    );
}

#[test]
fn show_ambiguous_stem_never_picks_first() {
    let (_temp, vault) = fixture_vault("bob-cli-ref-show-ambiguous");
    let output = run_show(&vault, &["dup_stem"]);
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(stdout(&output), "", "no output on failure");
    let human = stderr(&output);
    assert!(
        human.contains("bob ref: error: ambiguous reference: dup_stem"),
        "ambiguous message:\n{human}"
    );
    assert!(
        human.contains("hint: ref/blogs/dup_stem.md — Duplicate Stem in Blogs")
            && human.contains(
                "hint: ref/papers/dup_stem.md — Duplicate Stem in Papers"
            ),
        "both candidates as hints:\n{human}"
    );

    let output = run_show(&vault, &["dup_stem", "-f", "json"]);
    assert_eq!(output.status.code(), Some(1));
    let document: serde_json::Value = serde_json::from_str(&stdout(&output))
        .unwrap_or_else(|error| panic!("parse show error JSON: {error}"));
    assert_eq!(document["ok"], false);
    assert_eq!(document["error"]["code"], "ambiguous_reference");
    let candidates =
        document["candidates"].as_array().expect("candidates array");
    let paths: Vec<&str> = candidates
        .iter()
        .map(|candidate| candidate["path"].as_str().expect("path"))
        .collect();
    assert_eq!(paths, ["ref/blogs/dup_stem.md", "ref/papers/dup_stem.md"]);
}

#[test]
fn show_miss_suggests_title_hints() {
    let (_temp, vault) = fixture_vault("bob-cli-ref-show-miss");
    let output = run_show(&vault, &["Capture Flows"]);
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(stdout(&output), "", "no output on failure");
    let human = stderr(&output);
    assert!(
        human.contains(
            "bob ref: error: no reference note matches Capture Flows"
        ),
        "miss message:\n{human}"
    );
    assert!(
        human.contains("hint: ref/blogs/clipped_blog.md"),
        "title hint:\n{human}"
    );

    let output = run_show(&vault, &["Capture Flows", "-f", "json"]);
    assert_eq!(output.status.code(), Some(1));
    let document: serde_json::Value =
        serde_json::from_str(&stdout(&output)).expect("error JSON parses");
    assert_eq!(document["error"]["code"], "unknown_reference");
    assert!(
        !document["candidates"]
            .as_array()
            .expect("candidates")
            .is_empty(),
        "hints travel as candidates"
    );
}

#[test]
fn show_resolves_before_printing_anything() {
    let (_temp, vault) = fixture_vault("bob-cli-ref-show-atomic");
    let output = run_show(&vault, &["clipped_blog", "dup_stem"]);
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(
        stdout(&output),
        "",
        "one failure prints nothing else:\n{}",
        format_output(&output)
    );
}

#[test]
fn show_keeps_quotes_comments_images_and_tombstone_counts() {
    let (_temp, vault) = fixture_vault("bob-cli-ref-show-content");
    let document = run_show_json(&vault, &["clipped_blog"]);
    let row = &document["refs"][0];
    assert_eq!(row["annotations_status"], "parsed");
    assert_eq!(
        row["excluded"],
        serde_json::json!({
            "marker_mirrors": 0,
            "preamble": 0,
            "removed": 1,
        })
    );
    let annotations = row["annotations"].as_array().expect("annotations");
    assert_eq!(annotations.len(), 4);
    let kinds: Vec<&str> = annotations
        .iter()
        .map(|annotation| annotation["kind"].as_str().expect("kind"))
        .collect();
    assert_eq!(kinds, ["highlight", "highlight", "note", "image"]);
    assert_eq!(
        annotations[0]["quote"],
        serde_json::json!("First highlight text.")
    );
    assert_eq!(annotations[0]["comment"], serde_json::json!(null));
    assert_eq!(
        annotations[1]["comment"],
        serde_json::json!("My comment here.")
    );
    assert_eq!(
        annotations[3]["asset"],
        serde_json::json!("ref/blogs/capture-flows.assets/h-666666666666.png")
    );
    assert_eq!(
        annotations[3]["link"],
        serde_json::json!("[[ref/blogs/clipped_blog#^h-666666666666]]")
    );

    let output = run_show(&vault, &["clipped_blog"]);
    assert_success(&output);
    let human = stdout(&output);
    assert!(human.contains("Clipped Blog on Capture Flows"));
    assert!(
        human.contains("✓ FINISHED") && human.contains("read 2026-08-18"),
        "state chip and date:\n{human}"
    );
    assert!(
        human.contains("“Second highlight with a comment.”")
            && human.contains("↳ My comment here."),
        "quote kept apart from its comment:\n{human}"
    );
    assert!(
        human.contains("(1 removed highlight not shown)"),
        "tombstones counted, not shown:\n{human}"
    );
}

#[test]
fn show_excludes_mirrors_with_counts() {
    let (_temp, vault) = fixture_vault("bob-cli-ref-show-mirror");
    let document = run_show_json(&vault, &["ref/papers/synced_paper.md"]);
    let row = &document["refs"][0];
    assert_eq!(row["annotations"].as_array().expect("array").len(), 1);
    assert_eq!(row["excluded"]["marker_mirrors"], 1);
    assert_eq!(
        row["own_notes"],
        serde_json::json!("My own note about this paper.")
    );

    let output = run_show(&vault, &["ref/papers/synced_paper.md"]);
    assert_success(&output);
    let human = stdout(&output);
    assert!(
        human.contains("(1 leaked marker block not shown)"),
        "mirror exclusion line:\n{human}"
    );
    assert!(
        !human.contains("status: ready"),
        "mirror text never renders:\n{human}"
    );
}

#[test]
fn show_legacy_note_reports_absent_annotations() {
    let (_temp, vault) = fixture_vault("bob-cli-ref-show-legacy");
    let document = run_show_json(&vault, &["ref/ai/legacy_unread.md"]);
    let row = &document["refs"][0];
    assert_eq!(row["annotations_status"], "absent");
    assert_eq!(row["annotations"], serde_json::json!([]));
    assert!(
        row["own_notes"]
            .as_str()
            .expect("own_notes string")
            .contains("Some migrated zorg body text without a tracker."),
        "migrated body passes through as notes"
    );
}

#[test]
fn show_comments_only_and_no_annotations() {
    let (_temp, vault) = fixture_vault("bob-cli-ref-show-flags");
    let document = run_show_json(&vault, &["clipped_blog", "-c"]);
    let annotations = document["refs"][0]["annotations"]
        .as_array()
        .expect("array");
    assert_eq!(annotations.len(), 2, "commented highlight plus note");
    assert_eq!(
        annotations[0]["quote"].as_str().expect("quote"),
        "Second highlight with a comment."
    );
    assert_eq!(annotations[1]["kind"], "note");

    let output = run_show(&vault, &["clipped_blog", "-c"]);
    assert_success(&output);
    let human = stdout(&output);
    assert!(
        !human.contains("First highlight text."),
        "uncommented quotes drop out:\n{human}"
    );

    let document = run_show_json(&vault, &["ref/papers/synced_paper.md", "-N"]);
    assert_eq!(document["refs"][0]["annotations"], serde_json::json!([]));
    let output = run_show(&vault, &["ref/papers/synced_paper.md", "-N"]);
    assert_success(&output);
    let human = stdout(&output);
    assert!(
        !human.contains("ANNOTATIONS"),
        "metadata and notes only:\n{human}"
    );
    assert!(human.contains("My own note about this paper."));

    let output = run_show(&vault, &["clipped_blog", "-c", "-N"]);
    assert_eq!(
        output.status.code(),
        Some(2),
        "the content flags conflict:\n{}",
        format_output(&output)
    );
}

#[test]
fn show_comments_only_without_comments_reports_empty() {
    let (_temp, vault) = inline_vault(
        "bob-cli-ref-show-no-comments",
        &[(
            "ref/papers/quiet.md",
            "---\nstatus: read\ntitle: Quiet Paper\n---\n\n# Quiet Paper\n\n- [x] #task #ref [[lib/papers/quiet.pdf]] #hide ^ref\n\n## Highlights\n\n<!-- highlights:begin -->\n\n### Page 1\n\n> [!quote] Quoted text without a comment.\n\n^h-ffffffffffffffff\n\n<!-- highlights:end -->\n",
        )],
    );
    let output = run_show(&vault, &["quiet", "-c"]);
    assert_success(&output);
    let human = stdout(&output);
    assert!(
        human.contains("No comments or standalone notes."),
        "empty comments report:\n{human}"
    );
}

#[test]
fn show_separates_several_refs_in_order() {
    let (_temp, vault) = fixture_vault("bob-cli-ref-show-multi");
    let document =
        run_show_json(&vault, &["chat_read", "ref/papers/synced_paper.md"]);
    assert_eq!(
        shown_paths(&document),
        ["ref/chat/chat_read.md", "ref/papers/synced_paper.md"]
    );
    let output = run_show(&vault, &["chat_read", "ref/papers/synced_paper.md"]);
    assert_success(&output);
    let human = stdout(&output);
    assert_text_order(
        &human,
        &["Chat Read Episode", "Synced Paper on Reference Indexing"],
    );
}

#[test]
fn show_markdown_digest_covers_research_and_sections() {
    let (_temp, vault) = fixture_vault("bob-cli-ref-show-markdown");
    let output = run_show(&vault, &["chat_read", "-f", "markdown"]);
    assert_success(&output);
    let markdown = stdout(&output);
    assert!(markdown.contains("## Chat Read Episode"));
    assert!(markdown.contains("- Note: [[ref/chat/chat_read]]"));
    assert!(markdown.contains("- Reading state: finished (ref_task:[x]"));
    assert!(markdown.contains("- Research report: research:202610/some_topic"));

    let output = run_show(&vault, &["clipped_blog", "-f", "markdown"]);
    assert_success(&output);
    let markdown = stdout(&output);
    assert!(markdown.contains("### Annotations"));
    assert!(markdown.contains("**Page 1**"));
    assert!(markdown.contains("> Second highlight with a comment."));
    assert!(markdown.contains("Comment: My comment here."));
    assert!(markdown.contains("Note: A standalone note of my own."));
    assert!(!markdown.contains("### Notes"), "empty notes stay omitted");
    assert!(!markdown.contains("### Tasks"), "empty tasks stay omitted");
}

#[test]
fn show_missing_ref_dir_fails() {
    let temp = TempDir::new("bob-cli-ref-show-missing");
    let vault = temp.path().join("vault");
    fs::create_dir_all(&vault).expect("create vault");
    let output = run_show(&vault, &["anything"]);
    assert_eq!(output.status.code(), Some(1));
    assert!(
        stderr(&output).contains("reference directory not found"),
        "missing dir message:\n{}",
        format_output(&output)
    );

    let output = run_show(&vault, &["anything", "-f", "json"]);
    assert_eq!(output.status.code(), Some(1));
    let document: serde_json::Value =
        serde_json::from_str(&stdout(&output)).expect("error JSON parses");
    assert_eq!(document["ok"], false);
    assert_eq!(document["command"], "ref show");
}

#[test]
fn show_counts_preamble_blocks_as_excluded() {
    let (_temp, vault) = inline_vault(
        "bob-cli-ref-show-preamble",
        &[(
            "ref/papers/preamble_case.md",
            "---\nstatus: ready\ntitle: Preamble Case\n---\n\n# Preamble Case\n\n- [ ] #task #ref [[lib/papers/pre.pdf]] #hide ^ref\n\n## Highlights\n\n<!-- highlights:begin -->\n\n> [!quote] Early stray block.\n^h-aaaaaaaaaaaaaaaa\n\n### Page 3\n\n> [!quote] Paged quote.\n^h-bbbbbbbbbbbbbbbb\n\n<!-- highlights:end -->\n",
        )],
    );
    let document = run_show_json(&vault, &["preamble_case"]);
    let row = &document["refs"][0];
    assert_eq!(row["annotations_status"], "parsed");
    assert_eq!(row["excluded"]["preamble"], 1);
    assert_eq!(
        row["annotations"].as_array().expect("array").len(),
        1,
        "only the paged block shows"
    );

    let output = run_show(&vault, &["preamble_case"]);
    assert_success(&output);
    let human = stdout(&output);
    assert!(
        human.contains("(1 preamble block not shown)"),
        "preamble exclusion line:\n{human}"
    );
    assert!(
        !human.contains("Early stray block"),
        "preamble text never renders:\n{human}"
    );
}

#[test]
fn show_unparsed_region_reports_raw() {
    let (_temp, vault) = inline_vault(
        "bob-cli-ref-show-unparsed",
        &[(
            "ref/papers/unparsed_case.md",
            "---\nstatus: read\ntitle: Unparsed Case\n---\n\n# Unparsed Case\n\n- [x] #task #ref [[lib/papers/un.pdf]] #hide ^ref\n\n## Highlights\n\n<!-- highlights:begin -->\n\n### Page 1\n\n> [!tip] A callout the parser does not know.\n^h-cccccccccccccccc\n\n> [!quote] A fine quote.\n^h-dddddddddddddddd\n\n<!-- highlights:end -->\n",
        )],
    );
    let document = run_show_json(&vault, &["unparsed_case"]);
    let row = &document["refs"][0];
    assert_eq!(row["annotations_status"], "unparsed");
    assert!(
        row["raw_region"]
            .as_str()
            .expect("raw_region string")
            .contains("A callout the parser does not know."),
        "raw region travels with the row"
    );
    assert_eq!(
        row["annotations"].as_array().expect("array").len(),
        1,
        "parsed blocks still show"
    );
}

#[test]
fn show_tasks_own_notes_and_multiline_comments() {
    let (_temp, vault) = inline_vault(
        "bob-cli-ref-show-tasks",
        &[(
            "ref/papers/tasks_case.md",
            "---\nstatus: wip\ntitle: Tasks Case\n---\n\n# Tasks Case\n\n- [/] #task #ref [[lib/papers/tasks.pdf]] #hide ^ref\n\nSome own notes here.\n\n## Highlights\n\n<!-- highlights:begin -->\n\n### Page 4\n\n> [!quote] Quoted text here.\n>\n> > [!note] Comment First comment line.\n> > Second comment line.\n^h-eeeeeeeeeeeeeeee\n\n<!-- highlights:end -->\n\n## Tasks\n\n- [ ] Compare with the appendix. [[#^h-eeeeeeeeeeeeeeee|🔖]] [created:: 2026-10-01]\n- [x] Done item [[#^h-eeeeeeeeeeeeeeee|🔖]]\n- [-] Partial item [[#^h-eeeeeeeeeeeeeeee|🔖]]\n",
        )],
    );
    let document = run_show_json(&vault, &["tasks_case"]);
    let row = &document["refs"][0];
    assert_eq!(row["own_notes"], serde_json::json!("Some own notes here."));
    assert_eq!(
        row["annotations"][0]["comment"],
        serde_json::json!("First comment line.\nSecond comment line.")
    );
    assert_eq!(
        row["tasks"],
        serde_json::json!([
            {
                "checked": false,
                "mark": " ",
                "text": "Compare with the appendix.",
                "block_id": "h-eeeeeeeeeeeeeeee",
            },
            {
                "checked": true,
                "mark": "x",
                "text": "Done item",
                "block_id": "h-eeeeeeeeeeeeeeee",
            },
            {
                "checked": false,
                "mark": "-",
                "text": "Partial item",
                "block_id": "h-eeeeeeeeeeeeeeee",
            },
        ])
    );

    let output = run_show(&vault, &["tasks_case"]);
    assert_success(&output);
    let human = stdout(&output);
    assert!(human.contains("TASKS"), "tasks section:\n{human}");
    // Each task carries its linked annotation's page label.
    assert!(human.contains("☐ Compare with the appendix.   Page 4"));
    assert!(human.contains("☑ Done item   Page 4"));
    assert!(human.contains("☐ Partial item   Page 4"));

    let output = run_show(&vault, &["tasks_case", "-f", "markdown"]);
    assert_success(&output);
    let markdown = stdout(&output);
    assert!(markdown.contains("### Notes"));
    assert!(markdown.contains("Some own notes here."));
    assert!(markdown.contains("### Tasks"));
    assert!(markdown.contains("- [ ] Compare with the appendix."));
    assert!(markdown.contains("- [x] Done item"));
    assert!(markdown.contains("- [-] Partial item"));
}

#[test]
fn show_resolves_id_doi_arxiv_and_source_pdf() {
    let (_temp, vault) = inline_vault(
        "bob-cli-ref-show-id",
        &[
            (
                "ref/papers/id_note.md",
                "---\nstatus: ready\ntitle: Identity Note\nid: my-note-id\nsource_url: https://doi.org/10.1234/test-doi\nsource_pdf: \"[[lib/papers/id_note.pdf]]\"\n---\n\n# Identity Note\n\n- [ ] #task #ref [[lib/papers/id_note.pdf]] #hide ^ref\n",
            ),
            (
                "ref/papers/arxiv_note.md",
                "---\nstatus: ready\ntitle: Arxiv Note\nsource_url: https://arxiv.org/abs/2601.00001\nsource_pdf: \"[[lib/papers/arxiv_note.pdf]]\"\n---\n\n# Arxiv Note\n\n- [ ] #task #ref [[lib/papers/arxiv_note.pdf]] #hide ^ref\n",
            ),
        ],
    );
    let document = run_show_json(
        &vault,
        &[
            "my-note-id",
            "10.1234/test-doi",
            "lib/papers/id_note.pdf",
            "https://arxiv.org/pdf/2601.00001",
            "arxiv:2601.00001",
        ],
    );
    assert_eq!(
        shown_paths(&document),
        [
            "ref/papers/id_note.md",
            "ref/papers/id_note.md",
            "ref/papers/id_note.md",
            "ref/papers/arxiv_note.md",
            "ref/papers/arxiv_note.md",
        ]
    );
}

#[test]
fn show_is_read_only() {
    let (_temp, vault) = fixture_vault("bob-cli-ref-show-readonly");
    let before = vault_snapshot(&vault);
    for args in [
        vec!["clipped_blog"],
        vec!["clipped_blog", "-c"],
        vec!["clipped_blog", "-N"],
        vec!["clipped_blog", "-f", "markdown"],
        vec!["clipped_blog", "-f", "json"],
        vec!["dup_stem"],
    ] {
        run_show(&vault, &args);
    }
    assert_eq!(
        vault_snapshot(&vault),
        before,
        "show never writes to the vault"
    );
}
