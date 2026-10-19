//! Continuation highlights: a `<` note folds into the previous quote.

use crate::support::*;
use std::fs;
use std::path::Path;

fn scan(vault: &Path, extra: &[&str]) -> std::process::Output {
    let mut command = bob_command();
    command.arg("ref").arg("scan").env("BOB_DIR", vault);
    for arg in extra {
        command.arg(*arg);
    }
    command.output().expect("run bob ref scan")
}

#[test]
fn continuation_scan_merges_two_pages_and_is_idempotent() {
    let temp = TempDir::new("bob-cli-highlights-continuations");
    let vault = temp.path().join("vault");
    let pdf = vault.join("lib/blogs/example.pdf");
    write_highlights_pdf(
        &pdf,
        "- status: wip\n- parent: obsidian\n- title: Example\n",
    );
    write_file(
        &pdf.with_extension("md"),
        "\
## Page 1

- status: wip
- parent: obsidian

---

#### [Page 11](highlights://example#page=11)

> PR Sheriff Standing Orders. On every

***

#### [Page 12](highlights://example#page=12)

> session startup, the sheriff checks open PRs

< PR Merge Master?!
",
    );

    let first = scan(&vault, &[]);
    assert_success(&first);
    let note = vault.join("ref/blogs/example.md");
    let contents = fs::read_to_string(&note).expect("read note");
    assert!(contents.contains("### Pages 11–12"), "{contents}");
    assert!(contents.contains("On every session startup,"), "{contents}");
    assert!(contents.contains("PR Merge Master?!"), "{contents}");
    assert!(!contents.contains("< PR Merge Master"), "{contents}");

    let second = scan(&vault, &[]);
    assert_success(&second);
    let again = fs::read_to_string(&note).expect("read note again");
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
    let highlights: Vec<_> = annotations
        .iter()
        .filter(|ann| ann["kind"] == "highlight")
        .collect();
    assert_eq!(highlights.len(), 1, "{annotations:?}");
    assert_eq!(
        highlights[0]["page_label"],
        serde_json::json!("Pages 11–12")
    );
}
