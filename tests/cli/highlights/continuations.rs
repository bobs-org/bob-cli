//! Continuation highlights: a `<` note folds into the previous quote.

use crate::support::*;
use std::fs;
use std::path::Path;

#[test]
fn show_exposes_group_pieces_with_seams_and_omits_them_for_singles() {
    let temp = TempDir::new("bob-cli-ref-show-pieces");
    let vault = temp.path().join("vault");
    let note = vault.join("ref/blogs/pieces_case.md");
    fs::create_dir_all(note.parent().expect("note parent"))
        .expect("create note parent");
    write_file(
        &note,
        "---\nstatus: read\ntitle: Pieces Case\n---\n\n# Pieces Case\n\n- [x] #task #ref [[lib/papers/pieces.pdf]] #hide ^ref\n\n## Highlights\n\n<!-- highlights:begin -->\n\n### Pages 11–12\n\n> [!quote|Page 11] Head words on eleven\n>\n> > [!quote|Page 12] Tail words on twelve\n> >\n> > > [!note] Comment Follower note\n\n^h-aaaaaaaaaaaaaaaa\n\n### Page 3\n\n> [!quote] A lonely single.\n\n^h-bbbbbbbbbbbbbbbb\n\n<!-- highlights:end -->\n",
    );

    let output = bob_command()
        .arg("ref")
        .arg("show")
        .arg("pieces_case")
        .args(["-f", "json"])
        .env("BOB_DIR", &vault)
        .env("BOB_NOW", "2026-10-06 12:00:00")
        .output()
        .expect("run bob ref show");
    assert_success(&output);
    let document: serde_json::Value =
        serde_json::from_str(&stdout(&output)).expect("show json parses");
    let annotations = document["refs"][0]["annotations"]
        .as_array()
        .expect("annotations");
    assert_eq!(annotations.len(), 2, "{annotations:?}");
    let group = &annotations[0];
    assert_eq!(group["kind"], serde_json::json!("highlight"));
    assert_eq!(
        group["quote"],
        serde_json::json!("Head words on eleven\n\nTail words on twelve")
    );
    let pieces = group["pieces"].as_array().expect("group pieces");
    assert_eq!(pieces.len(), 2, "{pieces:?}");
    assert_eq!(pieces[0]["page_label"], serde_json::json!("Page 11"));
    assert_eq!(
        pieces[0]["quote"],
        serde_json::json!("Head words on eleven")
    );
    assert!(pieces[0].get("comment").is_none(), "{pieces:?}");
    assert_eq!(pieces[1]["page_label"], serde_json::json!("Page 12"));
    assert_eq!(
        pieces[1]["quote"],
        serde_json::json!("Tail words on twelve")
    );
    assert_eq!(pieces[1]["comment"], serde_json::json!("Follower note"));
    assert!(
        annotations[1].get("pieces").is_none(),
        "singles omit pieces: {}",
        annotations[1]
    );

    let output = bob_command()
        .arg("ref")
        .arg("show")
        .arg("pieces_case")
        .env("BOB_DIR", &vault)
        .env("BOB_NOW", "2026-10-06 12:00:00")
        .output()
        .expect("run bob ref show human");
    assert_success(&output);
    let human = stdout(&output);
    assert!(human.contains("Head words on eleven"), "{human}");
    assert!(human.contains("Tail words on twelve"), "{human}");
    assert!(human.contains("┄ Page 12"), "follower seam:\n{human}");
    assert_eq!(
        human.matches('┄').count(),
        1,
        "no seam on the single:\n{human}"
    );
    assert!(human.contains("↳ Follower note"), "{human}");

    let output = bob_command()
        .arg("ref")
        .arg("show")
        .arg("pieces_case")
        .args(["-f", "markdown"])
        .env("BOB_DIR", &vault)
        .env("BOB_NOW", "2026-10-06 12:00:00")
        .output()
        .expect("run bob ref show markdown");
    assert_success(&output);
    let markdown = stdout(&output);
    assert!(markdown.contains("> Head words on eleven"), "{markdown}");
    assert!(markdown.contains("> Tail words on twelve"), "{markdown}");
    assert!(markdown.contains("*┄ Page 12*"), "seam line:\n{markdown}");
    assert!(markdown.contains("Comment: Follower note"), "{markdown}");

    let output = bob_command()
        .arg("ref")
        .arg("show")
        .arg("pieces_case")
        .args(["-c", "-f", "json"])
        .env("BOB_DIR", &vault)
        .env("BOB_NOW", "2026-10-06 12:00:00")
        .output()
        .expect("run bob ref show comments-only");
    assert_success(&output);
    let document: serde_json::Value =
        serde_json::from_str(&stdout(&output)).expect("show json parses");
    let annotations = document["refs"][0]["annotations"]
        .as_array()
        .expect("annotations");
    assert_eq!(
        annotations.len(),
        1,
        "group commented only on its follower is kept: {annotations:?}"
    );
    assert_eq!(
        annotations[0]["pieces"].as_array().expect("pieces").len(),
        2,
        "all pieces shown: {:?}",
        annotations[0]["pieces"]
    );
}

fn scan(vault: &Path, extra: &[&str]) -> std::process::Output {
    let mut command = bob_command();
    command.arg("ref").arg("scan").env("BOB_DIR", vault);
    for arg in extra {
        command.arg(*arg);
    }
    command.output().expect("run bob ref scan")
}

#[test]
fn continuation_scan_tucks_two_pages_and_is_idempotent() {
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

    let dry = bob_command()
        .arg("ref")
        .arg("sync")
        .arg(&pdf)
        .arg("--dry-run")
        .env("BOB_DIR", &vault)
        .output()
        .expect("bob ref sync --dry-run");
    assert_success(&dry);
    let dry_out = stdout(&dry);
    assert!(
        dry_out.contains("continuations: 1"),
        "dry-run reports the absorbed follower:\n{}",
        format_output(&dry)
    );
    assert!(
        !dry_out.contains("continuation_dangling:"),
        "merged marker is not dangling:\n{}",
        format_output(&dry)
    );
    assert!(
        dry_out.contains("highlights_count: 1"),
        "a continuation group counts once:\n{}",
        format_output(&dry)
    );

    let json = scan(&vault, &["-f", "json", "-d"]);
    assert_success(&json);
    let scan_document: serde_json::Value =
        serde_json::from_str(&stdout(&json)).expect("scan json");
    let notes = scan_document["notes"].as_array().expect("notes");
    assert_eq!(notes.len(), 1, "{notes:?}");
    assert_eq!(notes[0]["continuations"], 1);
    assert!(
        notes[0].get("continuations_dangling").is_none(),
        "zero dangling count is omitted: {}",
        notes[0]
    );

    let concise = scan(&vault, &["-d"]);
    assert_success(&concise);
    assert!(
        stdout(&concise).contains("1 continuation"),
        "concise scan names the merge:\n{}",
        format_output(&concise)
    );

    let first = scan(&vault, &[]);
    assert_success(&first);
    let note = vault.join("ref/blogs/example.md");
    let contents = fs::read_to_string(&note).expect("read note");
    assert!(contents.contains("### Pages 11–12"), "{contents}");
    assert!(
        contents.contains(
            "> [!quote|Page 11] PR Sheriff Standing Orders. On every"
        ),
        "head piece keeps its page:\n{contents}"
    );
    assert!(
        contents.contains(
            "> > [!quote|Page 12] session startup, the sheriff checks open PRs"
        ),
        "follower stays in its own nested quote:\n{contents}"
    );
    assert!(contents.contains("PR Merge Master?!"), "{contents}");
    assert!(!contents.contains("< PR Merge Master"), "{contents}");
    assert!(contents.contains("highlights_count: 1\n"), "{contents}");

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
    assert_eq!(
        highlights[0]["quote"],
        serde_json::json!(
            "PR Sheriff Standing Orders. On every\n\nsession startup, the sheriff checks open PRs"
        )
    );
}

#[test]
fn continuation_scan_reports_dangling_marker() {
    let temp = TempDir::new("bob-cli-highlights-continuations-dangling");
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

> PR Sheriff Standing Orders.

< this dangles
",
    );

    let dry = bob_command()
        .arg("ref")
        .arg("sync")
        .arg(&pdf)
        .arg("--dry-run")
        .env("BOB_DIR", &vault)
        .output()
        .expect("bob ref sync --dry-run");
    assert_success(&dry);
    let dry_out = stdout(&dry);
    assert!(
        dry_out.contains("continuations: 0"),
        "dangling still prints the counter:\n{}",
        format_output(&dry)
    );
    assert!(
        dry_out.contains(
            "continuation_dangling: Page 11 (no earlier text highlight)"
        ),
        "dangling line names the page:\n{}",
        format_output(&dry)
    );

    let json = scan(&vault, &["-f", "json", "-d"]);
    assert_success(&json);
    let document: serde_json::Value =
        serde_json::from_str(&stdout(&json)).expect("scan json");
    let notes = document["notes"].as_array().expect("notes");
    assert_eq!(notes.len(), 1, "{notes:?}");
    assert!(
        notes[0].get("continuations").is_none(),
        "zero merged count is omitted: {}",
        notes[0]
    );
    assert_eq!(notes[0]["continuations_dangling"], 1);

    let concise = scan(&vault, &["-d"]);
    assert_success(&concise);
    let concise_out = stdout(&concise);
    assert!(
        concise_out.contains("1 dangling continuation"),
        "concise scan names the dangling marker:\n{}",
        format_output(&concise)
    );
}
