//! Underlines inside a highlight render as bold runs in the host quote.

use crate::support::*;
use std::{fs, path::Path};

fn scan(vault: &Path) -> std::process::Output {
    bob_command()
        .args(["ref", "scan", "--no-hooks"])
        .env("BOB_DIR", vault)
        .output()
        .expect("run bob ref scan")
}

#[test]
fn underline_scan_merges_comment_and_stays_idempotent() {
    let temp = TempDir::new("bob-cli-highlights-underlines");
    let vault = temp.path().join("vault");
    let pdf = vault.join("lib/blogs/example.pdf");
    write_highlights_pdf(
        &pdf,
        "- status: wip\n- parent: obsidian\n- title: Example\n",
    );
    write_file(
        &pdf.with_extension("md"),
        "\
#### [Page 1](highlights://example#page=1)

> The important phrase keeps the work moving.

***

> **important phrase**

Comment: Carry this thought forward.
",
    );

    let dry_run = bob_command()
        .args(["ref", "sync"])
        .arg(&pdf)
        .arg("--dry-run")
        .env("BOB_DIR", &vault)
        .output()
        .expect("run bob ref sync dry-run");
    assert_success(&dry_run);
    let report = stdout(&dry_run);
    assert!(report.contains("underlines_merged: 1"), "{report}");

    let json_scan = bob_command()
        .args(["ref", "scan", "--dry-run", "--no-hooks", "-f", "json"])
        .env("BOB_DIR", &vault)
        .output()
        .expect("run bob ref scan json dry-run");
    assert_success(&json_scan);
    let json: serde_json::Value =
        serde_json::from_str(&stdout(&json_scan)).expect("scan json");
    let entry = &json["notes"][0];
    assert_eq!(entry["underlines_merged"], 1);
    assert!(entry.get("underlines_standalone").is_none(), "{entry}");

    let first = scan(&vault);
    assert_success(&first);
    let note_path = vault.join("ref/blogs/example.md");
    let contents = fs::read_to_string(&note_path).expect("read rendered note");
    assert_eq!(contents.matches("> [!quote]").count(), 1, "{contents}");
    assert!(
        contents.contains(
            "> [!quote] The **important phrase** keeps the work moving."
        ),
        "{contents}"
    );
    assert!(
        contents.contains("Carry this thought forward."),
        "{contents}"
    );
    assert!(
        !contents.contains("> [!quote] **important phrase**"),
        "{contents}"
    );

    let second = scan(&vault);
    assert_success(&second);
    let again = fs::read_to_string(&note_path).expect("read note after rescan");
    assert_eq!(contents, again, "second scan is byte-identical");

    let show = bob_command()
        .arg("ref")
        .arg("show")
        .arg("example")
        .args(["-f", "json"])
        .env("BOB_DIR", &vault)
        .output()
        .expect("bob ref show");
    assert_success(&show);
    let document: serde_json::Value =
        serde_json::from_str(&stdout(&show)).expect("show json");
    let annotations = document["refs"][0]["annotations"]
        .as_array()
        .expect("annotations");
    let highlights = annotations
        .iter()
        .filter(|annotation| annotation["kind"] == "highlight")
        .collect::<Vec<_>>();
    assert_eq!(highlights.len(), 1, "{annotations:?}");
    assert!(
        highlights[0]["quote"]
            .as_str()
            .is_some_and(|quote| quote.contains("**important phrase**")),
        "{highlights:?}"
    );
}

#[test]
fn standalone_underline_reason_appears_in_sync_and_scan_json() {
    let temp = TempDir::new("bob-cli-highlights-standalone-underline");
    let vault = temp.path().join("vault");
    let pdf = vault.join("lib/blogs/standalone.pdf");
    write_highlights_pdf(
        &pdf,
        "- status: wip\n- parent: obsidian\n- title: Standalone\n",
    );
    write_file(
        &pdf.with_extension("md"),
        "\
#### [Page 2](highlights://standalone#page=2)

> **a standalone phrase**
",
    );

    let dry_run = bob_command()
        .args(["ref", "sync"])
        .arg(&pdf)
        .arg("--dry-run")
        .env("BOB_DIR", &vault)
        .output()
        .expect("run bob ref sync dry-run");
    assert_success(&dry_run);
    let report = stdout(&dry_run);
    assert!(report.contains("underlines_merged: 0"), "{report}");
    assert!(
        report
            .contains("underline_standalone: Page 2 (no highlight around it)"),
        "{report}"
    );

    let scan = bob_command()
        .args(["ref", "scan", "--dry-run", "--no-hooks", "-f", "json"])
        .env("BOB_DIR", &vault)
        .output()
        .expect("run bob ref scan json dry-run");
    assert_success(&scan);
    let document: serde_json::Value =
        serde_json::from_str(&stdout(&scan)).expect("scan json");
    let entry = &document["notes"][0];
    assert!(entry.get("underlines_merged").is_none(), "{entry}");
    assert_eq!(entry["underlines_standalone"], 1);
}
