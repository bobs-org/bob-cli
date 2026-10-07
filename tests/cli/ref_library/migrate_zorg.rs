//! `bob ref migrate-zorg` dry-run planner and report over fixture vaults.

use crate::support::*;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Output;

const FIXED_NOW: &str = "2026-10-06 12:00:00";

fn vault_with(prefix: &str, files: &[(&str, &str)]) -> (TempDir, PathBuf) {
    let temp = TempDir::new(prefix);
    let vault = temp.path().join("vault");
    for dir in ["lib", "ref", "xlib"] {
        fs::create_dir_all(vault.join(dir)).expect("create dir");
    }
    for (name, contents) in files {
        write_file(&vault.join(name), contents);
    }
    (temp, vault)
}

fn run_migrate(vault: &Path, args: &[&str]) -> Output {
    let mut full = vec!["ref", "migrate-zorg"];
    full.extend(args);
    bob_command()
        .args(&full)
        .env("BOB_DIR", vault)
        .env("BOB_NOW", FIXED_NOW)
        .output()
        .expect("run bob ref migrate-zorg")
}

fn run_json(vault: &Path, args: &[&str]) -> serde_json::Value {
    let mut full = args.to_vec();
    full.extend(["-f", "json"]);
    let output = run_migrate(vault, &full);
    assert_success(&output);
    assert_stdout_has_no_ansi(&output);
    serde_json::from_str(&stdout(&output)).unwrap_or_else(|error| {
        panic!(
            "parse migrate-zorg JSON: {error}\n{}",
            format_output(&output)
        )
    })
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
            continue;
        }
        let name = child
            .strip_prefix(root)
            .expect("under root")
            .to_string_lossy()
            .replace('\\', "/");
        let bytes = fs::read(&child).expect("read file");
        files.push((name, bytes));
    }
}

fn note_by_id(document: &serde_json::Value, id: &str) -> serde_json::Value {
    document["notes"]
        .as_array()
        .expect("notes array")
        .iter()
        .find(|note| note["source_id"] == id)
        .unwrap_or_else(|| panic!("missing note for {id}"))
        .clone()
}

const PLAIN_HUB: &str = "- 250411 250411#0p [[read]] #mcp ID::awesome_mcp_servers ^z-250411-0p\n  | LINKS: [[mcp_ref#^z-250411-0p|model_context_protocol]]\n  * file:: [[lib/docs/awesome_mcp_servers.pdf]]\n  * status:: READ\n  * url:: https://example.com/mcp\n";

#[test]
fn dry_run_plans_one_plain_record() {
    let (_temp, vault) =
        vault_with("bob-cli-migrate-zorg-plain", &[("work_ref.md", PLAIN_HUB)]);
    let before = vault_snapshot(&vault);
    let document = run_json(&vault, &[]);
    assert_eq!(document["ok"], true);
    assert_eq!(document["command"], "ref migrate-zorg");
    assert_eq!(document["mode"], "dry_run");
    assert_eq!(document["commit"], serde_json::Value::Null);
    let summary = &document["summary"];
    assert_eq!(summary["records"], 1);
    assert_eq!(summary["notes"], 1);
    assert_eq!(summary["chapters"], 0);
    assert_eq!(summary["skipped"], 0);
    assert_eq!(summary["renamed"], 0);
    assert_eq!(summary["no_url"], 0);
    assert_eq!(summary["already_migrated"], 0);
    let note = note_by_id(&document, "awesome_mcp_servers");
    assert_eq!(note["path"], "ref/zorg/work_ref/awesome_mcp_servers.md");
    assert_eq!(note["title"], "Awesome MCP Servers");
    assert_eq!(note["ref_type"], "docs");
    assert_eq!(note["legacy_status"], "read");
    assert_eq!(note["reading_state"], "finished");
    assert_eq!(note["urls"], serde_json::json!(["https://example.com/mcp"]));
    assert_eq!(note["renamed"], serde_json::Value::Null);
    // The dry run leaves the vault byte-identical and takes no lock.
    assert_eq!(vault_snapshot(&vault), before);
    assert!(!vault.join("ref/zorg").exists());

    let human = run_migrate(&vault, &[]);
    assert_success(&human);
    let report = stdout(&human);
    assert!(
        report.contains(
            "bob ref migrate-zorg · dry run · 1 records in 1 files → 1 notes under ref/zorg/"
        ),
        "{report}"
    );
    assert!(
        report.contains("coverage after --write: 0 unindexed (0 = skipped)"),
        "{report}"
    );
}

#[test]
fn multi_url_and_unusable_url_records() {
    let hub = "- 250411#0a [[read]] ID::multi_url ^z-250411-0a\n  * status:: READ\n  * url::\n    - https://a.example/one\n    - https://b.example/two\n\n- 250411#0b [[read]] ID::no_url ^z-250411-0b\n  * status:: UNREAD\n  * url:: NONE\n\n- 250411#0c [[read]] ID::wiki_url ^z-250411-0c\n  * status:: READ\n  * url:: [[some-note]]\n";
    let (_temp, vault) =
        vault_with("bob-cli-migrate-zorg-urls", &[("work_ref.md", hub)]);
    let document = run_json(&vault, &[]);
    assert_eq!(document["summary"]["records"], 3);
    assert_eq!(document["summary"]["notes"], 3);
    assert_eq!(document["summary"]["no_url"], 2);
    let multi = note_by_id(&document, "multi_url");
    assert_eq!(
        multi["urls"],
        serde_json::json!(["https://a.example/one", "https://b.example/two"])
    );
    let none = note_by_id(&document, "no_url");
    assert_eq!(none["urls"], serde_json::json!([]));
    let report = stdout(&run_migrate(&vault, &[]));
    assert!(report.contains("notes without a URL (2):"), "{report}");
    assert!(report.contains("ref/zorg/work_ref/no_url.md"), "{report}");
}

#[test]
fn stem_collisions_rename_against_vault_and_same_run() {
    let (_temp, vault) = vault_with(
        "bob-cli-migrate-zorg-stems",
        &[
            ("lit/practical_vim.md", "# Practical\n"),
            (
                "work_ref.md",
                "- 250115#0j [[read]] ID::practical_vim ^z-250115-0j\n  * status:: READ\n",
            ),
            (
                "dev_ref.md",
                "- 250115#0k [[read]] ID::practical_vim ^z-250115-0k\n  * status:: READ\n",
            ),
            ("_generated/tag_pages/ilar.md", "# Ilar\n"),
            (
                "prj_ilar.md",
                "- 250910#0w [[read]] ID::ilar ^z-250910-0w\n  * status:: READ\n",
            ),
        ],
    );
    let document = run_json(&vault, &[]);
    assert_eq!(document["summary"]["notes"], 3);
    assert_eq!(document["summary"]["renamed"], 3);
    let ilar = note_by_id(&document, "ilar");
    assert_eq!(ilar["path"], "ref/zorg/prj_ilar/ilar_ref.md");
    assert_eq!(
        ilar["renamed"],
        serde_json::json!({
            "from": "ilar",
            "avoids": "_generated/tag_pages/ilar.md",
        })
    );
    let mut paths: Vec<String> = document["notes"]
        .as_array()
        .expect("notes")
        .iter()
        .map(|note| note["path"].as_str().expect("path").to_string())
        .collect();
    paths.sort();
    assert!(
        paths.contains(&"ref/zorg/dev_ref/practical_vim_ref.md".to_string())
    );
    assert!(
        paths.contains(&"ref/zorg/work_ref/practical_vim_ref_2.md".to_string())
    );
    assert!(paths.contains(&"ref/zorg/prj_ilar/ilar_ref.md".to_string()));
    // Plan order is source path then owner line: dev_ref plans first and
    // renames against the vault note, work_ref collides with both.
    let first = note_by_id(&document, "practical_vim");
    assert_eq!(first["path"], "ref/zorg/dev_ref/practical_vim_ref.md");
    assert_eq!(
        first["renamed"],
        serde_json::json!({
            "from": "practical_vim",
            "avoids": "lit/practical_vim.md",
        })
    );
}

#[test]
fn shared_block_pair_stays_distinct_under_provenance() {
    let hub = "- 250425#0f [[read]] ID::bidder_declarations_prd ^z-250425-0f\n  * status:: READ\n  * url:: https://example.com/prd\n\n- 250425#0g [[read]] ID::cs_bidder_dec ^z-250425-0f\n  * status:: READ\n  * url:: https://example.com/dec\n";
    let mirrored = "---\nstatus: legacy\nlegacy_status: read\ntitle: Prd\nref_type: docs\nsource_note: \"[[prj_gbd]]\"\nsource_block: \"^z-250425-0f\"\nsource_id: bidder_declarations_prd\nsource_path: prj_gbd.md\nsource_line_start: 1\nsource_line_end: 3\n---\n\n# Prd\n";
    let (_temp, vault) = vault_with(
        "bob-cli-migrate-zorg-shared",
        &[("prj_gbd.md", hub), ("ref/docs/mirrored.md", mirrored)],
    );
    let document = run_json(&vault, &[]);
    assert_eq!(document["summary"]["records"], 1);
    assert_eq!(document["summary"]["already_migrated"], 1);
    assert_eq!(document["summary"]["notes"], 1);
    let note = &document["notes"][0];
    assert_eq!(note["source_id"], "cs_bidder_dec");
}

#[test]
fn book_folds_lid_and_book_tied_chapters() {
    let hub = "- 250920#01 [[read]] ID::ad_tech_book ^z-250920-01\n  * file:: [[lib/books/ad_tech_book.epub]]\n  * status:: BOOK\n  * url:: https://example.com/book\n\n- 250920#10 [[read]] LID::chapter_1 (a) ^z-250920-10\n  * status:: COLLECT_FLEETING_NOTES\n  * title:: Introduction\n\n- 250921#08 [[read]] LID::chapter_2 (b) ^z-250921-08\n  * status:: UNREAD\n\n- 250323#0b [[read]] ID::chapter_9 Appendix ^z-250323-0b\n  | BOOK: [[ad_tech_book#^z-250920-01|ad_tech_book]]\n  * status:: READ\n";
    let (_temp, vault) =
        vault_with("bob-cli-migrate-zorg-book", &[("ad_tech_book.md", hub)]);
    let document = run_json(&vault, &[]);
    assert_eq!(document["summary"]["records"], 4);
    assert_eq!(document["summary"]["notes"], 1);
    assert_eq!(document["summary"]["chapters"], 3);
    let note = note_by_id(&document, "ad_tech_book");
    assert_eq!(note["chapters"], 3);
    assert_eq!(note["legacy_status"], "book");
    // started + queued chapters alongside a finished one stay started.
    assert_eq!(note["reading_state"], "started");
    assert_eq!(note["ref_type"], "books");
}

#[test]
fn book_without_chapters_stays_unknown_and_strays_skip() {
    let hub = "- 241223#0t [[read]] ID::work_clean ^z-241223-0t\n  * status:: BOOK\n  * url:: https://example.com/work-clean\n";
    let strays = "- 250921#10 [[read]] LID::orphan_chapter ^z-250921-10\n  * status:: READ\n\n- status:: READ\n\n- 250921#11 [[read]] ^z-250921-11\n  * status:: READ\n";
    let (_temp, vault) = vault_with(
        "bob-cli-migrate-zorg-strays",
        &[("books.md", hub), ("notes.md", strays)],
    );
    let document = run_json(&vault, &[]);
    // books.md holds the BOOK (a note); notes.md holds an LID without a
    // book, a status line without an owner, and an owner without an ID.
    assert_eq!(document["summary"]["records"], 4);
    assert_eq!(document["summary"]["notes"], 1);
    assert_eq!(document["summary"]["chapters"], 0);
    assert_eq!(document["summary"]["skipped"], 3);
    let note = note_by_id(&document, "work_clean");
    assert_eq!(note["reading_state"], "unknown");
    let mut reasons: Vec<String> = document["skipped"]
        .as_array()
        .expect("skipped")
        .iter()
        .map(|row| row["reason"].as_str().expect("reason").to_string())
        .collect();
    reasons.sort();
    assert_eq!(reasons, vec!["no_id", "no_owner", "unassigned_chapter"]);
}

#[test]
fn invariant_note_chapter_skipped_equals_doctor_count() {
    let hub = "- 250920#01 [[read]] ID::ad_tech_book ^z-250920-01\n  * status:: BOOK\n\n- 250920#10 [[read]] LID::chapter_1 (a) ^z-250920-10\n  * status:: READ\n\n- 250411#0p [[read]] ID::plain ^z-250411-0p\n  * status:: READ\n  * url:: https://example.com/plain\n\n- 250921#11 [[read]] ^z-250921-11\n  * status:: READ\n";
    let (_temp, vault) =
        vault_with("bob-cli-migrate-zorg-invariant", &[("hub.md", hub)]);
    let document = run_json(&vault, &[]);
    let summary = &document["summary"];
    let total = summary["notes"].as_u64().expect("notes")
        + summary["chapters"].as_u64().expect("chapters")
        + summary["skipped"].as_u64().expect("skipped");
    assert_eq!(total, summary["records"].as_u64().expect("records"));
    assert_eq!(summary["records"], 4);
}
