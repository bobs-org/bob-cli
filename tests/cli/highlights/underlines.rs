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
