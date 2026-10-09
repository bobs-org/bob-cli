//! `bob ref migrate-tasks` over temporary fixture vaults.

use crate::support::*;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Output;

const FIXED_NOW: &str = "2026-10-09 12:00:00";

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
    let mut full = vec!["ref", "migrate-tasks"];
    full.extend(args);
    bob_command()
        .args(&full)
        .env("BOB_DIR", vault)
        .env("BOB_NOW", FIXED_NOW)
        .env("NO_COLOR", "1")
        .output()
        .expect("run bob ref migrate-tasks")
}

fn run_json(vault: &Path, args: &[&str]) -> serde_json::Value {
    let mut full = args.to_vec();
    full.extend(["-f", "json"]);
    let output = run_migrate(vault, &full);
    assert_success(&output);
    assert_stdout_has_no_ansi(&output);
    serde_json::from_str(&stdout(&output)).unwrap_or_else(|error| {
        panic!(
            "parse migrate-tasks JSON: {error}\n{}",
            format_output(&output)
        )
    })
}

fn git_vault(prefix: &str, files: &[(&str, &str)]) -> (TempDir, PathBuf) {
    let (temp, vault) = vault_with(prefix, files);
    git_in(&vault, ["init", "-q"]);
    git_in(&vault, ["symbolic-ref", "HEAD", "refs/heads/master"]);
    configure_test_git_identity(&vault);
    git_in(&vault, ["add", "-A"]);
    git_in(&vault, ["commit", "-q", "-m", "initial vault"]);
    (temp, vault)
}

fn run_write(vault: &Path, args: &[&str]) -> Output {
    let mut full = vec!["ref", "migrate-tasks", "--write", "--offline"];
    full.extend(args);
    bob_command()
        .args(&full)
        .env("BOB_DIR", vault)
        .env("BOB_NOW", FIXED_NOW)
        .env("NO_COLOR", "1")
        .output()
        .expect("run bob ref migrate-tasks --write")
}

fn run_write_json(vault: &Path, args: &[&str]) -> serde_json::Value {
    let mut full = args.to_vec();
    full.extend(["-f", "json"]);
    let output = run_write(vault, &full);
    assert_success(&output);
    assert_stdout_has_no_ansi(&output);
    serde_json::from_str(&stdout(&output)).unwrap_or_else(|error| {
        panic!(
            "parse migrate-tasks --write JSON: {error}\n{}",
            format_output(&output)
        )
    })
}

fn commit_subjects(vault: &Path) -> Vec<String> {
    let output = git_in(vault, &["log", "--format=%s"]);
    stdout(&output).lines().map(str::to_string).collect()
}

fn vault_snapshot(vault: &Path) -> Vec<(String, Vec<u8>)> {
    let mut files = Vec::new();
    let mut stack = vec![vault.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in fs::read_dir(&dir).expect("read dir") {
            let entry = entry.expect("entry");
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
                continue;
            }
            let rel = path
                .strip_prefix(vault)
                .expect("under vault")
                .to_string_lossy()
                .replace('\\', "/");
            files.push((rel, fs::read(&path).expect("read")));
        }
    }
    files.sort_by(|a, b| a.0.cmp(&b.0));
    files
}

fn worktree_snapshot(vault: &Path) -> Vec<(String, Vec<u8>)> {
    vault_snapshot(vault)
        .into_iter()
        .filter(|(name, _)| name != ".git" && !name.starts_with(".git/"))
        .collect()
}

const AREA: &str = "---\ntype: \"[[area]]\"\n---\n\n# Sase\n\n## Tasks\n";

fn ref_note(title: &str, parent: &str, created: &str, tracker: &str) -> String {
    format!(
        "---\nparent: \"[[{parent}]]\"\ncreated: {created}\n---\n\n# {title}\n\n{tracker}\n"
    )
}

#[test]
fn marks_preserve_fields_and_links() {
    let (_temp, vault) = git_vault(
        "bob-cli-migrate-tasks-marks",
        &[
            ("sase.md", AREA),
            (
                "ref/papers/a.md",
                &ref_note(
                    "A Title",
                    "sase",
                    "2026-10-01",
                    "- [ ] #task #ref [[lib/papers/a.pdf]] #hide ^ref",
                ),
            ),
            (
                "ref/papers/b.md",
                &ref_note(
                    "B Title",
                    "sase",
                    "2026-10-02",
                    "- [*] #task #ref [[lib/papers/b.pdf]] #hide [fresh:: 2026-10-02] [id:: custom-a] ^ref",
                ),
            ),
            (
                "ref/papers/c.md",
                &ref_note(
                    "C Title",
                    "sase",
                    "2026-10-03",
                    "- [/] #task #ref [[lib/papers/c.pdf]] #hide [dependsOn:: x] ^ref",
                ),
            ),
            (
                "ref/papers/d.md",
                &ref_note(
                    "D Title",
                    "sase",
                    "2026-10-04",
                    "- [?] #task #ref [[lib/papers/d.pdf]] #hide ^ref",
                ),
            ),
        ],
    );
    let document = run_write_json(&vault, &[]);
    assert_eq!(document["ok"], true);
    assert_eq!(document["mode"], "write");
    let contents = fs::read_to_string(vault.join("sase.md")).expect("read");
    for (mark, stem) in [(" ", "a"), ("*", "b"), ("/", "c"), ("?", "d")] {
        let needle = format!("- [{mark}] #task #ref [[ref/papers/{stem}|");
        assert!(contents.contains(&needle), "missing {needle}:\n{contents}");
        assert!(
            contents.contains(&format!("^ref-{stem}")),
            "missing block id for {stem}:\n{contents}"
        );
    }
    assert!(!contents.contains("#hide"), "{contents}");
    assert!(!contents.contains("lib/papers"), "{contents}");
    // Existing fresh stays, no new fresh invented on lines without one.
    assert!(contents.contains("[fresh:: 2026-10-02]"), "{contents}");
    let fresh_count = contents.matches("[fresh::").count();
    assert_eq!(fresh_count, 1, "expected one fresh:\n{contents}");
    // Custom id stays as written.
    assert!(contents.contains("[id:: custom-a]"), "{contents}");
}

#[test]
fn depends_on_and_pomodoro_links_rewritten() {
    let (_temp, vault) = git_vault(
        "bob-cli-migrate-tasks-deps",
        &[
            ("sase.md", AREA),
            (
                "ref/papers/a.md",
                &ref_note(
                    "A Title",
                    "sase",
                    "2026-10-01",
                    "- [ ] #task #ref [[lib/papers/a.pdf]] #hide ^ref\n  - Depends-On [[#^x]] • [[#^y]]",
                ),
            ),
            (
                "2026/20261009.md",
                "# 2026-10-09\n\n## Pomodoros\n\n- [ ] #task [[ref/papers/a#^ref]] pomodoro work\n",
            ),
            (
                "proj.md",
                "---\ntype: \"[[project]]\"\nstatus: active\n---\n\n# Proj\n\n## Tasks\n\n- [ ] #task parent [[ref/papers/a#^ref]] open\n",
            ),
        ],
    );
    // Dependent task with path-derived dependsOn.
    let old_dep = "ref__papers__a__ref";
    let dep_note =
        format!("# Dep\n\n- [ ] #task work [dependsOn:: {old_dep}]\n");
    fs::write(vault.join("dep.md"), &dep_note).expect("write dep");
    git_in(&vault, &["add", "-A"]);
    git_in(&vault, &["commit", "-q", "-m", "dep"]);
    let document = run_write_json(&vault, &[]);
    assert_eq!(document["ok"], true);
    // Pomodoro link rewritten but absent from wrappers.
    let daily =
        fs::read_to_string(vault.join("2026/20261009.md")).expect("read");
    assert!(!daily.contains("#^ref]]"), "{daily}");
    assert!(daily.contains("#^ref-a"), "{daily}");
    let wrappers = document["possible_wrappers"].as_array().expect("wrappers");
    for w in wrappers {
        assert_ne!(w["file"], "2026/20261009.md", "pomodoro must not wrap");
    }
    // Project embed rewritten and reported as wrapper when open.
    let proj = fs::read_to_string(vault.join("proj.md")).expect("read");
    assert!(proj.contains("sase#^ref-a"), "{proj}");
    assert!(
        wrappers.iter().any(|w| w["file"] == "proj.md"),
        "project wrapper missing: {document}"
    );
    // Dependency ids rewritten.
    let dep = fs::read_to_string(vault.join("dep.md")).expect("read");
    assert!(!dep.contains(old_dep), "{dep}");
    assert!(dep.contains("sase__ref-a"), "{dep}");
}

#[test]
fn ambiguous_bare_stem_left_unchanged() {
    let (_temp, vault) = git_vault(
        "bob-cli-migrate-tasks-ambiguous",
        &[
            ("sase.md", AREA),
            (
                "ref/papers/dup.md",
                &ref_note(
                    "Dup Papers",
                    "sase",
                    "2026-10-01",
                    "- [ ] #task #ref [[lib/papers/dup.pdf]] #hide ^ref",
                ),
            ),
            (
                "ref/blogs/dup.md",
                &ref_note(
                    "Dup Blogs",
                    "sase",
                    "2026-10-02",
                    "- [ ] #task #ref [[lib/blogs/dup.pdf]] #hide ^ref",
                ),
            ),
            ("note.md", "# Note\n\n- [ ] #task see [[dup#^ref]] here\n"),
        ],
    );
    let document = run_json(&vault, &[]);
    assert_eq!(document["ok"], true);
    let ambiguous = document["ambiguous_links"].as_array().expect("ambiguous");
    assert!(
        !ambiguous.is_empty(),
        "expected ambiguous bare stem: {document}"
    );
    let note = fs::read_to_string(vault.join("note.md")).expect("read");
    assert!(note.contains("[[dup#^ref]]"), "{note}");
}

#[test]
fn closed_and_legacy_notes_unchanged() {
    let closed = ref_note(
        "Closed",
        "sase",
        "2026-10-01",
        "- [x] #task #ref [[lib/papers/closed.pdf]] #hide [completion:: 2026-10-02] ^ref",
    );
    let legacy =
        "---\nparent: \"[[sase]]\"\n---\n\n# Legacy\n\nNo tracker here.\n";
    let (_temp, vault) = git_vault(
        "bob-cli-migrate-tasks-closed",
        &[
            ("sase.md", AREA),
            ("ref/papers/closed.md", &closed),
            ("ref/papers/legacy.md", legacy),
            (
                "ref/papers/open.md",
                &ref_note(
                    "Open",
                    "sase",
                    "2026-10-03",
                    "- [ ] #task #ref [[lib/papers/open.pdf]] #hide ^ref",
                ),
            ),
        ],
    );
    let before_closed =
        fs::read(vault.join("ref/papers/closed.md")).expect("read");
    let before_legacy =
        fs::read(vault.join("ref/papers/legacy.md")).expect("read");
    let document = run_write_json(&vault, &[]);
    assert_eq!(document["ok"], true);
    assert_eq!(
        fs::read(vault.join("ref/papers/closed.md")).expect("read"),
        before_closed
    );
    assert_eq!(
        fs::read(vault.join("ref/papers/legacy.md")).expect("read"),
        before_legacy
    );
}

#[test]
fn unmapped_refuses_write_and_map_wins() {
    let (_temp, vault) = git_vault(
        "bob-cli-migrate-tasks-unmapped",
        &[
            ("sase.md", AREA),
            (
                "bob.md",
                "---\ntype: \"[[area]]\"\n---\n\n# Bob\n\n## Tasks\n",
            ),
            (
                "ref/papers/u.md",
                &ref_note(
                    "U Title",
                    "sase",
                    "2026-10-01",
                    "- [ ] #task #ref [[lib/papers/u.pdf]] #hide ^ref",
                ),
            ),
        ],
    );
    // Remove the frontmatter parent so the ref is unmapped (no PDF marker).
    let unmapped_contents = "---\ncreated: 2026-10-01\n---\n\n# U Title\n\n- [ ] #task #ref [[lib/papers/u.pdf]] #hide ^ref\n";
    fs::write(vault.join("ref/papers/u.md"), unmapped_contents).expect("write");
    git_in(&vault, &["add", "-A"]);
    git_in(&vault, &["commit", "-q", "-m", "unmapped"]);
    let before = worktree_snapshot(&vault);
    let dry = run_migrate(&vault, &[]);
    assert!(dry.status.success(), "dry run exits 0");
    assert!(
        String::from_utf8_lossy(&dry.stdout).contains("unmapped")
            || String::from_utf8_lossy(&dry.stdout).contains("U Title")
            || String::from_utf8_lossy(&dry.stdout).contains("u.md"),
        "dry run lists unmapped:\n{}",
        format_output(&dry)
    );
    let write = run_write(&vault, &[]);
    assert!(!write.status.success(), "write refuses unmapped");
    assert_eq!(worktree_snapshot(&vault), before, "writes nothing");

    // A map row whose parent resolves wins over frontmatter.
    let mapped_contents = ref_note(
        "U Title",
        "sase",
        "2026-10-01",
        "- [ ] #task #ref [[lib/papers/u.pdf]] #hide ^ref",
    );
    fs::write(vault.join("ref/papers/u.md"), &mapped_contents).expect("write");
    git_in(&vault, &["add", "-A"]);
    git_in(&vault, &["commit", "-q", "-m", "mapped"]);
    let map_path = vault.join("map.tsv");
    fs::write(&map_path, "ref/papers/u.md\tbob\n").expect("map");
    let output = run_write(&vault, &["-m", map_path.to_str().unwrap()]);
    assert_success(&output);
    let sase = fs::read_to_string(vault.join("sase.md")).expect("read");
    let bob = fs::read_to_string(vault.join("bob.md")).expect("read");
    assert!(
        !sase.contains("ref-u"),
        "map wins over frontmatter:\n{sase}"
    );
    assert!(bob.contains("ref-u"), "map parent used:\n{bob}");
}

#[test]
fn tsv_round_trip_with_comments_and_extra_columns() {
    let (_temp, vault) = vault_with(
        "bob-cli-migrate-tasks-tsv",
        &[
            ("sase.md", AREA),
            (
                "ref/papers/t.md",
                &ref_note(
                    "T Title",
                    "sase",
                    "2026-10-01",
                    "- [ ] #task #ref [[lib/papers/t.pdf]] #hide ^ref",
                ),
            ),
        ],
    );
    let tsv_out = run_migrate(&vault, &["-f", "tsv"]);
    assert_success(&tsv_out);
    let tsv = stdout(&tsv_out);
    assert!(tsv.contains("ref/papers/t.md\tsase"), "{tsv}");
    let map_path = vault.join("round.tsv");
    fs::write(&map_path, format!("# comment\nref_note\tparent\n{tsv}"))
        .expect("map");
    // Extra columns are ignored.
    let extra = fs::read_to_string(&map_path).expect("read");
    fs::write(
        &map_path,
        extra.replace("ref/papers/t.md\tsase", "ref/papers/t.md\tsase\textra"),
    )
    .expect("extra");
    let check =
        run_migrate(&vault, &["-f", "tsv", "-m", map_path.to_str().unwrap()]);
    assert_success(&check);
    assert!(
        stdout(&check).contains("ref/papers/t.md\tsase"),
        "{}",
        format_output(&check)
    );
}

#[test]
fn crash_adoption_writes_embed_without_second_task() {
    let (_temp, vault) = git_vault(
        "bob-cli-migrate-tasks-adopt",
        &[
            ("sase.md", AREA),
            (
                "ref/papers/c.md",
                &ref_note(
                    "C Title",
                    "sase",
                    "2026-10-01",
                    "- [ ] #task #ref [[lib/papers/c.pdf]] #hide ^ref",
                ),
            ),
        ],
    );
    // Parent already contains the v2 line; ref note still has v1.
    let v2 = "- [ ] #task #ref [[ref/papers/c|C Title]] [created::2026-10-01] ^ref-c\n";
    let sase = format!("{AREA}\n{v2}");
    fs::write(vault.join("sase.md"), &sase).expect("write");
    git_in(&vault, &["add", "-A"]);
    git_in(&vault, &["commit", "-q", "-m", "crash"]);
    let document = run_write_json(&vault, &[]);
    assert_eq!(document["ok"], true);
    let after = fs::read_to_string(vault.join("sase.md")).expect("read");
    assert_eq!(after.matches("^ref-c").count(), 1, "{after}");
    let ref_note_contents =
        fs::read_to_string(vault.join("ref/papers/c.md")).expect("read");
    assert!(
        ref_note_contents.contains("![[sase#^ref-c]]"),
        "{ref_note_contents}"
    );
    assert!(!ref_note_contents.contains("^ref\n"), "{ref_note_contents}");
}

#[test]
fn second_write_is_noop_and_non_git_fails() {
    let (_temp, vault) = git_vault(
        "bob-cli-migrate-tasks-noop",
        &[
            ("sase.md", AREA),
            (
                "ref/papers/n.md",
                &ref_note(
                    "N Title",
                    "sase",
                    "2026-10-01",
                    "- [ ] #task #ref [[lib/papers/n.pdf]] #hide ^ref",
                ),
            ),
        ],
    );
    let first = run_write(&vault, &[]);
    assert_success(&first);
    assert!(commit_subjects(&vault).len() == 2);
    let second = run_write(&vault, &[]);
    assert_success(&second);
    assert!(
        stdout(&second).contains("nothing to migrate"),
        "{}",
        format_output(&second)
    );
    assert_eq!(commit_subjects(&vault).len(), 2, "no second commit");

    // Non-git vault.
    let (temp2, vault2) = vault_with(
        "bob-cli-migrate-tasks-nogit",
        &[
            ("sase.md", AREA),
            (
                "ref/papers/z.md",
                &ref_note(
                    "Z Title",
                    "sase",
                    "2026-10-01",
                    "- [ ] #task #ref [[lib/papers/z.pdf]] #hide ^ref",
                ),
            ),
        ],
    );
    let _ = temp2;
    let before = worktree_snapshot(&vault2);
    let output = run_write(&vault2, &[]);
    assert!(!output.status.success());
    assert_eq!(worktree_snapshot(&vault2), before);
}

#[test]
fn json_modes_and_commit_subject() {
    let (_temp, vault) = git_vault(
        "bob-cli-migrate-tasks-json",
        &[
            ("sase.md", AREA),
            (
                "ref/papers/j.md",
                &ref_note(
                    "J Title",
                    "sase",
                    "2026-10-01",
                    "- [ ] #task #ref [[lib/papers/j.pdf]] #hide ^ref",
                ),
            ),
            (
                "ref/papers/k.md",
                &ref_note(
                    "K Title",
                    "sase",
                    "2026-10-02",
                    "- [*] #task #ref [[lib/papers/k.pdf]] #hide ^ref",
                ),
            ),
        ],
    );
    let dry = run_json(&vault, &[]);
    assert_eq!(dry["mode"], "dry_run");
    assert!(dry["commit"].is_null());
    let wrote = run_write_json(&vault, &[]);
    assert_eq!(wrote["mode"], "write");
    assert_eq!(
        wrote["commit"]["subject"],
        "bob ref migrate-tasks: 2 ref tasks into 1 notes"
    );
    assert_eq!(wrote["command"], "ref migrate-tasks");
    assert_eq!(wrote["schema_version"], 1);
}

#[test]
fn help_lists_options_alphabetically() {
    let output = bob_command()
        .args(["ref", "migrate-tasks", "--help"])
        .env("NO_COLOR", "1")
        .output()
        .expect("help");
    assert_success(&output);
    let help = stdout(&output);
    for opt in ["-b", "-f", "-m", "-o", "-r", "-w"] {
        assert!(help.contains(opt), "missing {opt}:\n{help}");
    }
    let b = help.find("-b, --bob-dir").expect("bob-dir");
    let f = help.find("-f, --format").expect("format");
    let m = help.find("-m, --map").expect("map");
    let o = help.find("-o, --offline").expect("offline");
    let r = help.find("-r, --ref-dir").expect("ref-dir");
    let w = help.find("-w, --write").expect("write");
    assert!(b < f && f < m && m < o && o < r && r < w, "{help}");
}
