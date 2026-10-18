//! Image notes: paste a screenshot path into a highlight comment.

use crate::support::*;
use std::fs;
use std::path::Path;

fn png_bytes() -> Vec<u8> {
    b"image-note-source-bytes".to_vec()
}

fn setup_vault(
    name: &str,
) -> (TempDir, std::path::PathBuf, std::path::PathBuf) {
    let temp = TempDir::new(name);
    let home = temp.path().join("home");
    let vault = temp.path().join("vault");
    let screenshot = home.join("tmp/screenshots/20261018_104022.png");
    fs::create_dir_all(screenshot.parent().expect("screenshot parent"))
        .expect("create screenshot dir");
    fs::write(&screenshot, png_bytes()).expect("write screenshot");
    let pdf = vault.join("lib/papers/example.pdf");
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

> Latency is not throughput.

~/tmp/screenshots/20261018_104022.png
",
    );
    (temp, home, vault)
}

fn scan(vault: &Path, home: &Path, extra: &[&str]) -> std::process::Output {
    let mut command = bob_command();
    command
        .arg("ref")
        .arg("scan")
        .env("BOB_DIR", vault)
        .env("HOME", home)
        .env("BOB_NOW", "2026-10-18 10:40:22");
    for arg in extra {
        command.arg(*arg);
    }
    command.output().expect("run bob ref scan")
}

#[test]
fn image_notes_copy_embed_and_snapshot() {
    let (_temp, home, vault) = setup_vault("bob-cli-image-notes-copy");
    let dest =
        vault.join("ref/papers/example.assets/20261018_104022-07154fc6.png");
    let note = vault.join("ref/papers/example.md");

    let dry = scan(&vault, &home, &["--verbose", "--dry-run"]);
    assert_success(&dry);
    let dry_out = stdout(&dry);
    assert!(
        dry_out.contains("image_notes: 1"),
        "dry-run reports planned copy:\n{}",
        format_output(&dry)
    );
    assert!(!dest.exists(), "dry-run must not copy the image");
    assert!(!note.exists(), "dry-run must not write the note");

    let first = scan(&vault, &home, &["--verbose"]);
    assert_success(&first);
    assert!(dest.exists(), "scan copies the screenshot");
    assert_eq!(
        fs::read(&dest).expect("read dest"),
        png_bytes(),
        "vault copy is byte-identical"
    );
    let contents = fs::read_to_string(&note).expect("read note");
    assert!(
        contents.contains(
            "![[ref/papers/example.assets/20261018_104022-07154fc6.png]]"
        ),
        "{contents}"
    );

    let second = scan(&vault, &home, &["--verbose"]);
    assert_success(&second);
    assert!(
        stdout(&second).contains("writes: none"),
        "second scan is a no-op:\n{}",
        format_output(&second)
    );

    fs::remove_file(home.join("tmp/screenshots/20261018_104022.png"))
        .expect("delete source");
    let after_delete = scan(&vault, &home, &["--verbose"]);
    assert_success(&after_delete);
    assert!(
        stdout(&after_delete).contains("writes: none"),
        "missing source keeps the snapshot:\n{}",
        format_output(&after_delete)
    );
    assert!(dest.exists(), "snapshot remains after source deletion");
    let contents = fs::read_to_string(&note).expect("read note");
    assert!(
        contents.contains(
            "![[ref/papers/example.assets/20261018_104022-07154fc6.png]]"
        ),
        "{contents}"
    );

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
    let images = &document["refs"][0]["annotations"][0]["images"];
    assert_eq!(
        images,
        &serde_json::json!([
            "ref/papers/example.assets/20261018_104022-07154fc6.png"
        ])
    );
}

#[test]
fn image_notes_missing_source_warns_then_heals() {
    let temp = TempDir::new("bob-cli-image-notes-missing");
    let home = temp.path().join("home");
    let vault = temp.path().join("vault");
    fs::create_dir_all(home.join("tmp/screenshots")).expect("screenshots dir");
    let pdf = vault.join("lib/papers/example.pdf");
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

> Latency is not throughput.

~/tmp/screenshots/20261018_104022.png
",
    );

    let first = scan(&vault, &home, &["--verbose"]);
    assert_success(&first);
    let note = fs::read_to_string(vault.join("ref/papers/example.md"))
        .expect("read note");
    assert!(note.contains("[!warning] Image not found"), "{note}");

    git_in(&vault, ["init", "-q"]);
    configure_test_git_identity(&vault);
    git_in(&vault, ["add", "."]);
    git_in(&vault, ["commit", "-q", "-m", "initial"]);

    let doctor = bob_command()
        .arg("ref")
        .arg("doctor")
        .arg("--no-hooks")
        .env("BOB_DIR", &vault)
        .output()
        .expect("bob ref doctor");
    assert_success(&doctor);
    let doctor_out = stdout(&doctor);
    assert!(
        doctor_out.contains("unresolved_image_note"),
        "doctor lists unresolved image notes:\n{}",
        format_output(&doctor)
    );

    fs::write(
        home.join("tmp/screenshots/20261018_104022.png"),
        png_bytes(),
    )
    .expect("create screenshot");
    let healed = scan(&vault, &home, &["--verbose"]);
    assert_success(&healed);
    let note = fs::read_to_string(vault.join("ref/papers/example.md"))
        .expect("read healed note");
    assert!(
        note.contains(
            "![[ref/papers/example.assets/20261018_104022-07154fc6.png]]"
        ),
        "{note}"
    );
    assert!(!note.contains("[!warning] Image not found"), "{note}");
}
