//! Read-only planning and refusal behavior for `bob ref rename`.

use crate::support::*;
use std::{
    fs,
    path::{Path, PathBuf},
    process::Output,
};

const FIXED_NOW: &str = "2026-10-15 12:00:00";

fn synced_vault(
    prefix: &str,
    stem: &str,
    marker: &str,
    companions: bool,
) -> (TempDir, PathBuf) {
    let temp = TempDir::new(prefix);
    let vault = temp.path().join("vault");
    for directory in ["lib/blogs", "ref/blogs", "xlib", "done", "daily"] {
        fs::create_dir_all(vault.join(directory))
            .expect("create vault directory");
    }
    write_area_note(&vault.join("sase.md"), "SASE");
    let pdf = vault.join(format!("lib/blogs/{stem}.pdf"));
    if companions {
        write_file(&vault.join(format!("lib/blogs/{stem}.md")), "");
        write_file(
            &vault.join(format!("lib/blogs/{stem}.textbundle/text.md")),
            "",
        );
        write_file(&vault.join(format!("lib/blogs/{stem}.mp3")), "audio bytes");
    }
    write_highlights_pdf(&pdf, marker);
    let output = bob_command()
        .args(["ref", "sync"])
        .arg(&pdf)
        .env("BOB_DIR", &vault)
        .env("BOB_NOW", FIXED_NOW)
        .output()
        .expect("run initial ref sync");
    assert_success(&output);
    (temp, vault)
}

fn gist_vault(prefix: &str) -> (TempDir, PathBuf) {
    synced_vault(
        prefix,
        "442a6bf555914893e9891c11519de94f",
        concat!(
            "- status: wip\n",
            "- parent: sase\n",
            "- title: karpathy/llm-wiki.md\n",
            "- id: 442a6bf555914893e9891c11519de94f\n",
            "- source_url: https://gist.github.com/karpathy/442a6bf555914893e9891c11519de94f\n",
        ),
        false,
    )
}

fn run_rename(vault: &Path, args: &[&str]) -> Output {
    bob_command()
        .arg("ref")
        .arg("rename")
        .args(args)
        .env("BOB_DIR", vault)
        .env("BOB_NOW", FIXED_NOW)
        .env("NO_COLOR", "1")
        .output()
        .expect("run bob ref rename")
}

fn rename_json(vault: &Path, args: &[&str]) -> serde_json::Value {
    let mut full = args.to_vec();
    full.extend(["-d", "-f", "json"]);
    let output = run_rename(vault, &full);
    assert_success(&output);
    assert_stdout_has_no_ansi(&output);
    serde_json::from_str(&stdout(&output)).unwrap_or_else(|error| {
        panic!("parse rename JSON: {error}\n{}", format_output(&output))
    })
}

fn error_json(vault: &Path, args: &[&str], dry_run: bool) -> serde_json::Value {
    let mut full = args.to_vec();
    if dry_run {
        full.push("-d");
    }
    full.extend(["-f", "json"]);
    let output = run_rename(vault, &full);
    assert_eq!(
        output.status.code(),
        Some(1),
        "expected a structured refusal:\n{}",
        format_output(&output)
    );
    assert_stdout_has_no_ansi(&output);
    serde_json::from_str(&stdout(&output)).unwrap_or_else(|error| {
        panic!(
            "parse rename error JSON: {error}\n{}",
            format_output(&output)
        )
    })
}

fn minimal_modern_note(vault: &Path, stem: &str, source_pdf: &str, body: &str) {
    write_file(
        &vault.join(format!("ref/blogs/{stem}.md")),
        &format!(
            "---\nid: stable-{stem}\ntitle: {stem}\nsource_pdf: {source_pdf}\n---\n{body}"
        ),
    );
}

fn git_commit_all(vault: &Path) {
    git_in(vault, ["init", "-q"]);
    configure_test_git_identity(vault);
    git_in(vault, ["add", "-A"]);
    git_in(vault, ["commit", "-q", "-m", "fixture"]);
}

fn snapshot(root: &Path) -> Vec<(String, Vec<u8>)> {
    fn walk(root: &Path, current: &Path, out: &mut Vec<(String, Vec<u8>)>) {
        for entry in fs::read_dir(current).expect("read snapshot directory") {
            let entry = entry.expect("snapshot entry");
            let path = entry.path();
            if path.file_name().is_some_and(|name| name == ".git") {
                continue;
            }
            if path.is_dir() {
                walk(root, &path, out);
            } else {
                let relative = path
                    .strip_prefix(root)
                    .unwrap()
                    .to_string_lossy()
                    .to_string();
                out.push((
                    relative,
                    fs::read(path).expect("read snapshot file"),
                ));
            }
        }
    }
    let mut result = Vec::new();
    walk(root, root, &mut result);
    result.sort_by(|left, right| left.0.cmp(&right.0));
    result
}

#[test]
fn rename_help_lists_options_alphabetically_with_short_aliases() {
    let temp = TempDir::new("bob-cli-ref-rename-help");
    let vault = temp.path().join("vault");
    fs::create_dir_all(&vault).unwrap();
    let output = run_rename(&vault, &["--help"]);
    assert_success(&output);
    let help = stdout(&output);
    let options = [
        "-b, --bob-dir",
        "-d, --dry-run",
        "-f, --format",
        "-l, --lib-dir",
        "-o, --offline",
        "-r, --ref-dir",
        "-x, --xlib-dir",
        "-h, --help",
    ];
    let positions = options
        .iter()
        .map(|option| {
            help.find(option)
                .unwrap_or_else(|| panic!("missing {option}:\n{help}"))
        })
        .collect::<Vec<_>>();
    assert!(
        positions.windows(2).all(|pair| pair[0] < pair[1]),
        "options are not alphabetical:\n{help}"
    );
    for long_option in [
        "--bob-dir",
        "--dry-run",
        "--format",
        "--lib-dir",
        "--offline",
        "--ref-dir",
        "--xlib-dir",
    ] {
        assert!(help.contains(long_option), "missing {long_option}:\n{help}");
    }
}

#[test]
fn gist_dry_run_reports_moves_links_kept_identity_and_writes_nothing() {
    let (_temp, vault) = gist_vault("bob-cli-ref-rename-gist");
    let before = snapshot(&vault);
    let old = "442a6bf555914893e9891c11519de94f";
    let document = rename_json(&vault, &[old, "karpathy_llm_wiki"]);
    assert_eq!(document["ok"], true);
    assert_eq!(document["command"], "ref rename");
    assert_eq!(document["schema_version"], 1);
    assert_eq!(document["mode"], "dry_run");
    assert_eq!(document["outcome"], "renamed");
    assert_eq!(document["from"]["path"], format!("ref/blogs/{old}.md"));
    assert_eq!(document["to"]["path"], "ref/blogs/karpathy_llm_wiki.md");
    assert_eq!(document["moves"].as_array().unwrap().len(), 2);
    assert_eq!(document["links"]["rewritten"], 1);
    assert_eq!(document["links"]["files"][0], "sase.md");
    assert_eq!(document["frontmatter"], serde_json::json!(["source_pdf"]));
    assert_eq!(document["kept"]["marker_id"], old);
    assert_eq!(document["kept"]["task"]["path"], "sase.md");
    assert!(document["kept"]["task"]["block_id"].as_str().is_some());
    assert_eq!(document["ref"]["path"], format!("ref/blogs/{old}.md"));
    assert_eq!(document["commit"], serde_json::Value::Null);
    assert_eq!(document["sync"]["pre"], "skipped");
    assert_eq!(document["sync"]["post"], "skipped");
    assert_eq!(
        snapshot(&vault),
        before,
        "dry-run must not change vault bytes"
    );
}

#[test]
fn bundle_dry_run_includes_companions_done_daily_and_ambiguous_bare_links() {
    let (_temp, vault) = synced_vault(
        "bob-cli-ref-rename-bundle",
        "old.bundle",
        "- status: wip\n- parent: sase\n- title: Bundle\n- id: bundle-id\n",
        true,
    );
    write_file(
        &vault.join("done/finished.md"),
        "[old](ref/blogs/old.bundle.md#section)\n",
    );
    write_file(
        &vault.join("daily/2026-10-15.md"),
        "[[ref/blogs/old.bundle]]\n",
    );
    let parent = vault.join("sase.md");
    let mut parent_contents =
        fs::read_to_string(&parent).expect("read task note");
    parent_contents.push_str("\nBare: [[old.bundle]]\n");
    write_file(&parent, &parent_contents);
    write_file(&vault.join("archive/old.bundle.md"), "archive\n");
    let document = rename_json(&vault, &["old.bundle", "new.bundle"]);
    let moves = document["moves"].as_array().unwrap();
    for expected in [
        "ref/blogs/new.bundle.md",
        "lib/blogs/new.bundle.pdf",
        "lib/blogs/new.bundle.md",
        "lib/blogs/new.bundle.textbundle",
        "lib/blogs/new.bundle.mp3",
    ] {
        assert!(
            moves.iter().any(|movement| movement["to"] == expected),
            "missing move {expected}: {moves:?}"
        );
    }
    assert!(document["links"]["rewritten"].as_u64().unwrap() >= 3);
    let files = document["links"]["files"].as_array().unwrap();
    for expected in ["sase.md", "done/finished.md", "daily/2026-10-15.md"] {
        assert!(
            files.iter().any(|path| path == expected),
            "missing {expected}: {files:?}"
        );
    }
    let skipped = document["links"]["skipped_ambiguous"].as_array().unwrap();
    assert!(skipped.iter().any(|link| link["target"] == "old.bundle"));
    assert_eq!(
        document["frontmatter"],
        serde_json::json!(["audio", "highlights_sidecar", "source_pdf"])
    );
}

#[test]
fn dotted_stem_dry_run_preserves_the_full_stem() {
    let (_temp, vault) = synced_vault(
        "bob-cli-ref-rename-dotted",
        "source.v1",
        "- status: wip\n- parent: sase\n- title: Dotted\n- id: dotted-id\n",
        false,
    );
    let document = rename_json(&vault, &["source.v1", "next.release.v2"]);
    assert_eq!(document["from"]["pdf"], "lib/blogs/source.v1.pdf");
    assert_eq!(document["to"]["pdf"], "lib/blogs/next.release.v2.pdf");
    assert_eq!(document["moves"][1]["from"], "lib/blogs/source.v1.pdf");
}

#[test]
fn resolver_scope_name_collision_and_highlight_refusals_are_structured() {
    let temp = TempDir::new("bob-cli-ref-rename-refusals");
    let vault = temp.path().join("vault");
    fs::create_dir_all(vault.join("ref/blogs")).unwrap();
    fs::create_dir_all(vault.join("ref/ai")).unwrap();
    fs::create_dir_all(vault.join("lib/blogs")).unwrap();
    fs::create_dir_all(vault.join("xlib/blogs")).unwrap();

    assert_eq!(
        error_json(&vault, &["missing", "new"], true)["error"]["code"],
        "unknown_reference"
    );
    write_file(
        &vault.join("ref/blogs/a.md"),
        "---\nid: duplicate\ntitle: A\n---\n",
    );
    write_file(
        &vault.join("ref/blogs/b.md"),
        "---\nid: duplicate\ntitle: B\n---\n",
    );
    assert_eq!(
        error_json(&vault, &["duplicate", "new"], true)["error"]["code"],
        "ambiguous_reference"
    );
    assert!(
        error_json(&vault, &["duplicate", "new"], true)["error"]["hint"]
            .as_str()
            .unwrap()
            .contains("Candidates:")
    );

    write_file(
        &vault.join("ref/ai/legacy.md"),
        "---\nid: legacy\ntitle: Legacy\n---\n",
    );
    assert_eq!(
        error_json(&vault, &["legacy", "new"], true)["error"]["code"],
        "legacy_reference"
    );
    assert_eq!(
        error_json(&vault, &["xlib/blogs/intake.pdf", "new"], true)["error"]
            ["code"],
        "intake_reference"
    );

    minimal_modern_note(&vault, "old", "lib/blogs/old.pdf", "");
    assert_eq!(
        error_json(&vault, &["old", "bad name"], true)["error"]["code"],
        "invalid_name"
    );
    write_file(&vault.join("lib/blogs/old.pdf"), "pdf");
    write_file(&vault.join("ref/blogs/taken.md"), "target\n");
    assert_eq!(
        error_json(&vault, &["old", "taken"], true)["error"]["code"],
        "name_taken"
    );

    let annotated = concat!(
        "---\nid: annotated\ntitle: Annotated\nsource_pdf: lib/blogs/annotated.pdf\n---\n",
        "<!-- highlights:begin -->\n\n### Page 1\n\n> [!quote] A quote\n^h-0123456789abcdef\n\n<!-- highlights:end -->\n",
    );
    minimal_modern_note(&vault, "annotated", "lib/blogs/annotated.pdf", "");
    write_file(&vault.join("ref/blogs/annotated.md"), annotated);
    write_file(&vault.join("lib/blogs/annotated.pdf"), "pdf");
    assert_eq!(
        error_json(&vault, &["annotated", "next"], true)["error"]["code"],
        "has_highlights"
    );
}

#[test]
fn listen_link_not_in_sync_dirty_and_live_mode_refusals() {
    let temp = TempDir::new("bob-cli-ref-rename-preconditions");
    let vault = temp.path().join("vault");
    fs::create_dir_all(vault.join("lib/blogs")).unwrap();
    fs::create_dir_all(vault.join("ref/blogs")).unwrap();
    write_area_note(&vault.join("sase.md"), "SASE");
    minimal_modern_note(&vault, "old", "lib/blogs/old.pdf", "");
    let pdf = vault.join("lib/blogs/old.pdf");
    write_highlights_pdf(
        &pdf,
        "- status: wip\n- parent: sase\n- title: Old\n- id: old-id\n",
    );
    write_file(&vault.join("lib/blogs/old.mp3"), "audio");
    add_listen_link(&pdf, "obsidian://open?file=lib%2Fblogs%2Fold.mp3");
    assert_eq!(
        error_json(&vault, &["old", "new"], true)["error"]["code"],
        "listen_card_link"
    );

    let (_temp, stale_vault) = synced_vault(
        "bob-cli-ref-rename-stale",
        "old",
        "- status: wip\n- parent: sase\n- title: Old\n- id: old-id\n",
        false,
    );
    let stale_note = stale_vault.join("ref/blogs/old.md");
    let stale = fs::read_to_string(&stale_note).unwrap().replace(
        "source_pdf: lib/blogs/old.pdf",
        "source_pdf: lib/blogs/missing.pdf",
    );
    write_file(&stale_note, &stale);
    assert_eq!(
        error_json(&stale_vault, &["old", "new"], true)["error"]["code"],
        "not_in_sync"
    );

    let (_temp, dirty_vault) = gist_vault("bob-cli-ref-rename-dirty");
    git_commit_all(&dirty_vault);
    let parent = dirty_vault.join("sase.md");
    let mut parent_contents = fs::read_to_string(&parent).unwrap();
    parent_contents
        .push_str("[[ref/blogs/442a6bf555914893e9891c11519de94f]]\n");
    write_file(&parent, &parent_contents);
    assert_eq!(
        error_json(
            &dirty_vault,
            &[
                "442a6bf555914893e9891c11519de94f",
                "karpathy_llm_wiki",
                "-o"
            ],
            true,
        )["error"]["code"],
        "dirty_paths"
    );

    let (_temp, no_git_vault) = gist_vault("bob-cli-ref-rename-no-git");
    assert_eq!(
        error_json(
            &no_git_vault,
            &["442a6bf555914893e9891c11519de94f", "karpathy_llm_wiki"],
            false,
        )["error"]["code"],
        "not_a_worktree"
    );

    let (_temp, live_vault) = gist_vault("bob-cli-ref-rename-live");
    git_commit_all(&live_vault);
    assert_eq!(
        error_json(
            &live_vault,
            &["442a6bf555914893e9891c11519de94f", "karpathy_llm_wiki"],
            false,
        )["error"]["code"],
        "not_implemented"
    );
}

fn add_listen_link(pdf: &Path, uri: &str) {
    use lopdf::{dictionary, Document, Object, StringFormat};

    let mut document =
        Document::load(pdf).expect("load PDF for URI annotation");
    let page_id = document.page_iter().next().expect("first PDF page");
    let action_id = document.add_object(dictionary! {
        "S" => "URI",
        "URI" => Object::String(uri.as_bytes().to_vec(), StringFormat::Literal),
    });
    let link_id = document.add_object(dictionary! {
        "Type" => "Annot",
        "Subtype" => "Link",
        "Rect" => vec![
            Object::Integer(0),
            Object::Integer(0),
            Object::Integer(20),
            Object::Integer(20),
        ],
        "A" => Object::Reference(action_id),
    });
    let page = document
        .get_object_mut(page_id)
        .expect("page object")
        .as_dict_mut()
        .expect("page dictionary");
    let mut annots = page
        .get(b"Annots")
        .ok()
        .and_then(|value| value.as_array().ok())
        .cloned()
        .unwrap_or_default();
    annots.push(Object::Reference(link_id));
    page.set("Annots", Object::Array(annots));
    document.save(pdf).expect("save URI annotation");
}
