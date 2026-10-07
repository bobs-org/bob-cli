//! Reference items: bare links queue for the reading queue (phase capture).
//!
//! Routing is on by default (`bob_command()` points at a missing config
//! file, which gives the defaults): a lone bare URL becomes `kind: ref`
//! with `placement: queued`, a staged spool job, and no vault write.
//! Every test isolates `XDG_STATE_HOME`, so the spool is inspectable.

use crate::highlights::fake_clip::{failure_response, FakeClip};
use crate::support::*;
use sha2::Digest;
use std::fs;
use std::time::Instant;

const NOW: &str = "2026-10-07 13:40:00";
const ARTICLE_URL: &str = "https://example.com/post";
const TASK_LINE: &str =
    "- [ ] #task https://example.com/post [created::2026-10-07]";

fn state_dir(temp: &TempDir) -> std::path::PathBuf {
    temp.path().join("state")
}

fn pending_dir(temp: &TempDir) -> std::path::PathBuf {
    state_dir(temp).join("bob-cli/ref/jobs/pending")
}

fn pending_files(temp: &TempDir) -> Vec<std::path::PathBuf> {
    fs::read_dir(pending_dir(temp))
        .map(|entries| {
            entries
                .flatten()
                .map(|entry| entry.path())
                .filter(|path| {
                    path.extension().is_some_and(|ext| ext == "json")
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Pending plus running jobs: the kick test's worker may claim the job
/// before the test looks, and the slow adapter holds it in `running/`.
fn queued_files(temp: &TempDir) -> Vec<std::path::PathBuf> {
    let mut files = pending_files(temp);
    if let Ok(entries) =
        fs::read_dir(state_dir(temp).join("bob-cli/ref/jobs/running"))
    {
        files.extend(
            entries.flatten().map(|entry| entry.path()).filter(|path| {
                path.extension().is_some_and(|ext| ext == "json")
            }),
        );
    }
    files
}

fn capture(
    temp: &TempDir,
    vault: &std::path::Path,
    args: &[&str],
) -> std::process::Command {
    let mut command = bob_command();
    command.env("XDG_STATE_HOME", state_dir(temp));
    command.env("BOB_NOW", NOW);
    command.arg("capture").arg("-b").arg(vault);
    for arg in args {
        command.arg(arg);
    }
    command
}

fn parse_json(output: &std::process::Output) -> serde_json::Value {
    serde_json::from_str(stdout(output).trim()).unwrap_or_else(|error| {
        panic!("stdout should be JSON: {error}\n{}", format_output(output))
    })
}

fn write_article_curl(dir: &std::path::Path) -> std::path::PathBuf {
    let path = dir.join("fake-curl.sh");
    let script = "#!/bin/sh\ndest=\"\"\nprev=\"\"\nfor arg in \"$@\"; do\n  if [ \"$prev\" = \"-o\" ]; then dest=\"$arg\"; fi\n  prev=\"$arg\"\n  url=\"$arg\"\ndone\ncase \"$url\" in\n  *\"example.com\"*)\n    printf '<html><body>article</body></html>' > \"$dest\"\n    printf '200\\ntext/html; charset=utf-8\\n\\n'\n    ;;\n  *)\n    printf '404\\ntext/html\\n\\n'\n    ;;\nesac\n";
    write_executable(&path, script);
    path
}

#[test]
fn capture_lone_url_queues_a_ref_job() {
    let temp = TempDir::new("bob-cli-capture-ref-lone");
    let vault = temp.path().join("vault");
    fs::create_dir_all(&vault).expect("create vault");

    let output = capture(&temp, &vault, &["-f", "json", ARTICLE_URL])
        .output()
        .expect("run lone-URL capture");
    assert_success(&output);
    let json = parse_json(&output);
    assert_eq!(json["ok"], true);
    assert_eq!(json["dry_run"], false);
    assert_eq!(json["routed"], false);
    assert!(json["route"].is_null(), "{json}");
    assert_eq!(json["route_label"], "");
    assert_eq!(json["relative_target"], "");
    assert_eq!(json["target"], "");
    assert_eq!(json["text"], ARTICLE_URL);
    assert_eq!(json["task_line"], "");
    assert_eq!(json["kind"], "ref");
    assert_eq!(json["created"], "2026-10-07");
    assert!(json["scheduled"].is_null(), "{json}");
    assert_eq!(json["placement"], "queued");
    assert!(json.get("ref").is_some(), "{json}");
    let reference = &json["ref"];
    assert_eq!(reference["url"], ARTICLE_URL);
    assert_eq!(reference["cleaned_url"], ARTICLE_URL);
    assert_eq!(reference["dedupe_key"], ARTICLE_URL);
    assert_eq!(reference["display"], "example.com/post");
    assert_eq!(reference["route_hint"], "article");
    assert_eq!(reference["library"]["verdict"], "not_found");
    for key in ["path", "title", "reading_state", "message"] {
        assert!(
            reference["library"].get(key).is_some()
                && reference["library"][key].is_null(),
            "not_found library carries {key} as null: {json}"
        );
    }
    assert_eq!(reference["job"]["state"], "pending");
    assert!(!reference["job"]["id"]
        .as_str()
        .unwrap_or_default()
        .is_empty());
    assert_eq!(reference["fallback"]["relative_target"], "mac_inbox.md");
    assert_eq!(reference["fallback"]["task_line"], TASK_LINE);

    // One spool file, named for the reported job id, and no vault write.
    let jobs = pending_files(&temp);
    assert_eq!(jobs.len(), 1, "{json}");
    let name = jobs[0].file_stem().expect("job stem").to_string_lossy();
    assert_eq!(name, reference["job"]["id"].as_str().unwrap_or_default());
    assert!(!vault.join("mac_inbox.md").exists(), "no inbox write");
}

#[test]
fn capture_dry_run_matches_real_apart_from_dry_run_and_job() {
    let temp = TempDir::new("bob-cli-capture-ref-dry");
    let vault = temp.path().join("vault");
    fs::create_dir_all(&vault).expect("create vault");

    let dry = capture(&temp, &vault, &["-d", "-f", "json", ARTICLE_URL])
        .output()
        .expect("run dry-run capture");
    assert_success(&dry);
    let mut dry_json = parse_json(&dry);
    assert_eq!(dry_json["dry_run"], true);
    assert!(dry_json["ref"].get("job").is_none(), "{dry_json}");
    // The dry run staged nothing: the state tree stays absent.
    assert!(!state_dir(&temp).join("bob-cli").exists());

    let real = capture(&temp, &vault, &["-f", "json", ARTICLE_URL])
        .output()
        .expect("run real capture");
    assert_success(&real);
    let mut real_json = parse_json(&real);

    // A real run's output equals the dry run's apart from `dry_run` and `job`.
    dry_json["dry_run"] = serde_json::json!(false);
    dry_json["ref"]["job"] = real_json["ref"]["job"].clone();
    real_json["ref"]["job"]["id"] = dry_json["ref"]["job"]["id"].clone();
    assert_eq!(dry_json, real_json);
}

#[test]
fn capture_dry_run_and_parse_never_touch_the_network_or_spool() {
    let temp = TempDir::new("bob-cli-capture-ref-offline");
    let vault = temp.path().join("vault");
    fs::create_dir_all(&vault).expect("create vault");
    let marker = temp.path().join("network-called");
    let sentinel = temp.path().join("sentinel.sh");
    write_executable(
        &sentinel,
        &format!(
            "#!/bin/sh\necho called >> {}\nexit 1\n",
            shell_single_quote(path_str(&marker))
        ),
    );

    let dry = capture(&temp, &vault, &["-d", "-f", "json", ARTICLE_URL])
        .env("BOB_HIGHLIGHTS_CURL", &sentinel)
        .env("BOB_WEB_CLIP_ADAPTER", &sentinel)
        .output()
        .expect("run dry-run capture");
    assert_success(&dry);
    assert!(!marker.exists(), "dry run must stay offline");

    let parsed = bob_command()
        .arg("capture-parse")
        .arg("-f")
        .arg("json")
        .arg("--")
        .arg(ARTICLE_URL)
        .env("BOB_HIGHLIGHTS_CURL", &sentinel)
        .env("BOB_WEB_CLIP_ADAPTER", &sentinel)
        .output()
        .expect("run capture-parse");
    assert_success(&parsed);
    assert!(!marker.exists(), "capture-parse must stay offline");
    assert!(!state_dir(&temp).join("bob-cli").exists(), "no spool touch");
}

#[test]
fn capture_bracketed_url_queues_without_brackets() {
    let temp = TempDir::new("bob-cli-capture-ref-brackets");
    let vault = temp.path().join("vault");
    fs::create_dir_all(&vault).expect("create vault");

    let output = capture(
        &temp,
        &vault,
        &["-d", "-f", "json", "<https://example.com/post>"],
    )
    .output()
    .expect("run bracketed-URL capture");
    assert_success(&output);
    let json = parse_json(&output);
    assert_eq!(json["kind"], "ref");
    assert_eq!(json["text"], ARTICLE_URL);
    assert_eq!(json["ref"]["url"], ARTICLE_URL);
    // The fallback is exactly what routing-off capture would write.
    assert_eq!(
        json["ref"]["fallback"]["task_line"],
        "- [ ] #task <https://example.com/post> [created::2026-10-07]"
    );
}

#[test]
fn capture_url_with_markers_or_flags_stays_a_task() {
    let temp = TempDir::new("bob-cli-capture-ref-optouts");
    let vault = temp.path().join("vault");
    fs::create_dir_all(&vault).expect("create vault");

    // Each natural opt-out keeps the item a task, exactly as before.
    let drafts = [
        vec!["https://example.com/post read later"],
        vec!["https://example.com/post @cash"],
        vec!["https://example.com/post s:1"],
        vec!["@@work", "https://example.com/post"],
    ];
    for draft in &drafts {
        let mut args = vec!["-d", "-f", "json"];
        args.extend(draft.iter());
        let output = capture(&temp, &vault, &args)
            .output()
            .expect("run opt-out capture");
        assert_success(&output);
        let json = parse_json(&output);
        assert_eq!(json["kind"], "task", "draft {draft:?}:\n{json}");
        assert!(json.get("ref").is_none(), "draft {draft:?}:\n{json}");
    }
    let child = "https://example.com/post\n- a detail";
    let output = capture(&temp, &vault, &["-d", "-f", "json", child])
        .output()
        .expect("run child-line capture");
    assert_success(&output);
    assert_eq!(parse_json(&output)["kind"], "task");

    // Forced flags are explicit choices that keep the item a task.
    for flag in [&["-r", "notes"][..], &["-c"][..]] {
        let mut args = vec!["-d", "-f", "json"];
        args.extend(flag.iter());
        args.push(ARTICLE_URL);
        let output = capture(&temp, &vault, &args)
            .output()
            .expect("run forced-flag capture");
        assert_success(&output);
        assert_eq!(parse_json(&output)["kind"], "task", "flags {flag:?}");
    }
    write_file(
        &vault.join("notes.md"),
        "# Notes\n- [ ] #task existing ^test-id\n  - SEC\n\n## Sec\n",
    );
    let task_line = "- [ ] #task existing ^test-id";
    let task_ref = format!(
        "2:{}",
        &hex::encode(sha2::Sha256::digest(task_line.trim_end().as_bytes()))
            [..8]
    );
    // `-s` forces a section bullet, the others force task placement;
    // all opt out of ref claiming (no `ref` object, never `ref` kind).
    let flag_sets: Vec<Vec<String>> = vec![
        vec!["-r", "notes", "-s", "Sec"],
        vec!["-r", "notes", "-t", "test-id"],
        vec!["-r", "notes", "-t", "test-id", "-S", "SEC"],
        vec!["-r", "notes", "--task-ref", &task_ref],
    ]
    .into_iter()
    .map(|flags| flags.into_iter().map(str::to_string).collect())
    .collect();
    for flag in &flag_sets {
        let mut args = vec!["-d", "-f", "json"];
        args.extend(flag.iter().map(String::as_str));
        args.push(ARTICLE_URL);
        let output = capture(&temp, &vault, &args)
            .output()
            .expect("run forced-flag capture");
        assert_success(&output);
        let json = parse_json(&output);
        assert_ne!(json["kind"], "ref", "flags {flag:?}:\n{json}");
        assert!(
            json.get("ref").is_none() || json["ref"].is_null(),
            "flags {flag:?}:\n{json}"
        );
    }
}

#[test]
fn capture_inline_global_blocks_ref_claim() {
    let temp = TempDir::new("bob-cli-capture-ref-inline-global");
    let vault = temp.path().join("vault");
    fs::create_dir_all(&vault).expect("create vault");

    let mut command = capture(&temp, &vault, &["-d", "-f", "json"]);
    let output = run_with_stdin(
        &mut command,
        "Buy milk @@groceries\n\nhttps://example.com/post",
    );
    assert_success(&output);
    let json = parse_json(&output);
    let captures = json["captures"].as_array().expect("captures array");
    assert_eq!(captures.len(), 2);
    for capture in captures {
        assert_eq!(capture["kind"], "task", "{json}");
        assert!(
            capture.get("ref").is_none() || capture["ref"].is_null(),
            "{json}"
        );
    }
    let second = &captures[1];
    let routed = second["route"]
        .as_str()
        .unwrap_or_default()
        .contains("groceries")
        || second["relative_target"]
            .as_str()
            .unwrap_or_default()
            .contains("groceries");
    assert!(routed, "URL item routes to @@ target: {json}");

    let output = bob_command()
        .arg("capture-parse")
        .arg("-f")
        .arg("json")
        .arg("--")
        .arg("Buy milk @@groceries\n\nhttps://example.com/post")
        .output()
        .expect("run inline-global capture-parse");
    assert_success(&output);
    let json: serde_json::Value =
        serde_json::from_str(stdout(&output).trim()).expect("parse JSON");
    let items = json["items"].as_array().expect("items array");
    assert_eq!(items.len(), 2);
    assert_eq!(items[1]["mode"], "task", "{json}");
    assert_ne!(items[1]["mode"], "ref");
}

#[test]
fn capture_excluded_and_corporate_hosts_stay_tasks() {
    let temp = TempDir::new("bob-cli-capture-ref-hosts");
    let vault = temp.path().join("vault");
    fs::create_dir_all(&vault).expect("create vault");

    for url in ["https://github.com/org/repo", "http://go/x"] {
        let output = capture(&temp, &vault, &["-d", "-f", "json", url])
            .output()
            .expect("run host capture");
        assert_success(&output);
        let json = parse_json(&output);
        assert_eq!(json["kind"], "task", "url {url}:\n{json}");
    }
}

#[test]
fn capture_config_off_and_invalid_config_keep_tasks() {
    let temp = TempDir::new("bob-cli-capture-ref-config");
    let vault = temp.path().join("vault");
    fs::create_dir_all(&vault).expect("create vault");

    let off = temp.path().join("off.yml");
    write_file(&off, "highlights:\n  url_routing:\n    capture: false\n");
    let output = capture(&temp, &vault, &["-d", "-f", "json", ARTICLE_URL])
        .env("BOB_CONFIG_FILE", &off)
        .output()
        .expect("run config-off capture");
    assert_success(&output);
    assert_eq!(parse_json(&output)["kind"], "task");

    let bad = temp.path().join("bad.yml");
    write_file(&bad, "highlights:\n  url_routing: [broken\n");
    let output = capture(&temp, &vault, &["-d", "-f", "json", ARTICLE_URL])
        .env("BOB_CONFIG_FILE", &bad)
        .output()
        .expect("run invalid-config capture");
    assert_success(&output);
    assert_eq!(parse_json(&output)["kind"], "task");
    let diagnostic = stderr(&output);
    let expected = format!(
        "bob capture: warning: URL routing is off: parse {}:",
        bad.display()
    );
    assert!(
        diagnostic.contains(&expected),
        "expected full warning line {expected:?}:\n{}",
        format_output(&output)
    );
    assert!(
        !diagnostic.contains("Invalid(") && !diagnostic.contains("Read("),
        "warning must use Display, not Debug:\n{}",
        format_output(&output)
    );
}

#[test]
fn capture_url_list_queues_one_item_per_line() {
    let temp = TempDir::new("bob-cli-capture-ref-list");
    let vault = temp.path().join("vault");
    fs::create_dir_all(&vault).expect("create vault");

    let mut command = capture(&temp, &vault, &["-f", "json"]);
    let output = run_with_stdin(
        &mut command,
        "https://example.com/1\nhttps://example.com/2",
    );
    assert_success(&output);
    let json = parse_json(&output);
    let items = [&json, &json["captures"][0]];
    assert_eq!(json["captures"].as_array().expect("array").len(), 2);
    for item in items {
        assert_eq!(item["kind"], "ref");
        assert_eq!(item["placement"], "queued");
    }
    assert_eq!(pending_files(&temp).len(), 2);
}

#[test]
fn capture_mixed_list_and_draft_plan_each_item() {
    let temp = TempDir::new("bob-cli-capture-ref-mixed");
    let vault = temp.path().join("vault");
    fs::create_dir_all(&vault).expect("create vault");

    // A list with an excluded host mixes a task with a reference item.
    let mut command = capture(&temp, &vault, &["-d", "-f", "json"]);
    let output = run_with_stdin(
        &mut command,
        "https://github.com/org/repo\nhttps://example.com/1",
    );
    assert_success(&output);
    let json = parse_json(&output);
    let captures = json["captures"].as_array().expect("captures array");
    assert_eq!(captures.len(), 2);
    assert_eq!(captures[0]["kind"], "task");
    assert_eq!(captures[1]["kind"], "ref");

    // A mixed task-and-URL draft writes the task and queues the link.
    let mut command = capture(&temp, &vault, &["-f", "json"]);
    let output =
        run_with_stdin(&mut command, "buy milk\n\nhttps://example.com/2");
    assert_success(&output);
    let json = parse_json(&output);
    let captures = json["captures"].as_array().expect("captures array");
    assert_eq!(captures.len(), 2);
    assert_eq!(captures[0]["kind"], "task");
    assert_eq!(captures[1]["kind"], "ref");
    assert_eq!(pending_files(&temp).len(), 1);
    assert_eq!(
        fs::read_to_string(vault.join("mac_inbox.md")).expect("read inbox"),
        "- [ ] #task buy milk [created::2026-10-07]\n"
    );
}

#[test]
fn capture_library_hits_are_unchanged() {
    let temp = TempDir::new("bob-cli-capture-ref-library");
    let vault = temp.path().join("vault");
    write_file(
        &vault.join("ref/papers/captured.md"),
        "---\ntitle: Captured Post\nstatus: ready\nsource_url: https://example.com/captured\nsource_pdf: lib/papers/captured.pdf\n---\n\n- [ ] ^ref\n",
    );
    write_highlights_pdf(
        &vault.join("xlib/blogs/queued.pdf"),
        "- status: ready\n- parent: obsidian_ref\n- title: Queued Post\n- source_url: https://example.com/queued\n",
    );
    write_file(
        &vault.join("ref/blogs/legacy.md"),
        "---\ntitle: Legacy Post\nstatus: ready\nsource_url: https://example.com/legacy\n---\n\n- [ ] ^ref\n",
    );

    // Already in the library: success, no job, target names the note.
    let output = capture(
        &temp,
        &vault,
        &["-f", "json", "https://example.com/captured"],
    )
    .output()
    .expect("run in-library capture");
    assert_success(&output);
    let json = parse_json(&output);
    assert_eq!(json["placement"], "unchanged");
    assert_eq!(json["relative_target"], "ref/papers/captured.md");
    assert!(json["target"]
        .as_str()
        .unwrap_or_default()
        .ends_with("ref/papers/captured.md"));
    assert_eq!(json["ref"]["library"]["verdict"], "in_library");
    assert_eq!(json["ref"]["library"]["title"], "Captured Post");
    assert!(json["ref"].get("job").is_none(), "{json}");
    assert!(json["ref"].get("fallback").is_none(), "{json}");
    assert!(pending_files(&temp).is_empty());

    // Already queued for scan: the intake PDF is the target.
    let output =
        capture(&temp, &vault, &["-f", "json", "https://example.com/queued"])
            .output()
            .expect("run in-intake capture");
    assert_success(&output);
    let json = parse_json(&output);
    assert_eq!(json["placement"], "unchanged");
    assert_eq!(json["ref"]["library"]["verdict"], "in_intake");
    assert_eq!(json["relative_target"], "xlib/blogs/queued.pdf");

    // Legacy-only: a fresh copy is queued.
    let output =
        capture(&temp, &vault, &["-f", "json", "https://example.com/legacy"])
            .output()
            .expect("run legacy capture");
    assert_success(&output);
    let json = parse_json(&output);
    assert_eq!(json["placement"], "queued");
    assert_eq!(json["ref"]["library"]["verdict"], "legacy");
    assert_eq!(json["ref"]["library"]["path"], "ref/blogs/legacy.md");
    assert_eq!(pending_files(&temp).len(), 1);
}

#[test]
fn capture_spool_hit_is_clipping() {
    let temp = TempDir::new("bob-cli-capture-ref-clipping");
    let vault = temp.path().join("vault");
    fs::create_dir_all(&vault).expect("create vault");
    let pending = pending_dir(&temp);
    fs::create_dir_all(&pending).expect("create pending dir");
    fs::write(
        pending.join("seed.json"),
        format!(
            r#"{{"schema_version":1,"id":"seed","created_at":"2026-10-07T14:30:12-04:00","source":"capture","bob_dir":{},"url":"{}","cleaned_url":"{}","dedupe_key":"{}","display":"example.com/post","route_hint":"article","attempts":0,"fallback":{{"relative_target":"mac_inbox.md","task_line":{}}}}}"#,
            serde_json::json!(vault.to_string_lossy()),
            ARTICLE_URL,
            ARTICLE_URL,
            ARTICLE_URL,
            serde_json::json!(TASK_LINE),
        ),
    )
    .expect("seed pending job");

    let output = capture(&temp, &vault, &["-f", "json", ARTICLE_URL])
        .output()
        .expect("run clipping capture");
    assert_success(&output);
    let json = parse_json(&output);
    assert_eq!(json["placement"], "unchanged");
    assert_eq!(json["ref"]["library"]["verdict"], "clipping");
    assert_eq!(pending_files(&temp).len(), 1, "no duplicate job");
}

#[test]
fn capture_repeat_in_draft_is_duplicate() {
    let temp = TempDir::new("bob-cli-capture-ref-duplicate");
    let vault = temp.path().join("vault");
    fs::create_dir_all(&vault).expect("create vault");

    let mut command = capture(&temp, &vault, &["-f", "json"]);
    let output = run_with_stdin(
        &mut command,
        "https://example.com/a\n\nhttps://example.com/a",
    );
    assert_success(&output);
    let json = parse_json(&output);
    let captures = json["captures"].as_array().expect("captures array");
    assert_eq!(captures.len(), 2);
    assert_eq!(captures[0]["placement"], "queued");
    assert_eq!(captures[0]["ref"]["library"]["verdict"], "not_found");
    assert_eq!(captures[1]["placement"], "unchanged");
    assert_eq!(captures[1]["ref"]["library"]["verdict"], "duplicate");
    assert_eq!(
        captures[1]["ref"]["library"]["message"],
        "same link as item 1"
    );
    // One job for the pair.
    assert_eq!(pending_files(&temp).len(), 1);
}

#[test]
fn capture_human_wording_for_every_case() {
    let temp = TempDir::new("bob-cli-capture-ref-human");
    let vault = temp.path().join("vault");
    write_file(
        &vault.join("ref/papers/captured.md"),
        "---\ntitle: Captured Post\nstatus: ready\nsource_url: https://example.com/captured\nsource_pdf: lib/papers/captured.pdf\n---\n\n- [ ] ^ref\n",
    );
    write_highlights_pdf(
        &vault.join("xlib/blogs/queued.pdf"),
        "- status: ready\n- parent: obsidian_ref\n- title: Queued Post\n- source_url: https://example.com/queued\n",
    );
    write_file(
        &vault.join("ref/blogs/legacy.md"),
        "---\ntitle: Legacy Post\nstatus: ready\nsource_url: https://example.com/legacy\n---\n\n- [ ] ^ref\n",
    );
    let clipping_pending = state_dir(&temp).join("bob-cli/ref/jobs/pending");
    fs::create_dir_all(&clipping_pending).expect("create pending dir");
    fs::write(
        clipping_pending.join("seed.json"),
        format!(
            r#"{{"schema_version":1,"id":"seed","created_at":"2026-10-07T14:30:12-04:00","source":"capture","bob_dir":{},"url":"https://example.com/clipping","cleaned_url":"https://example.com/clipping","dedupe_key":"https://example.com/clipping","display":"example.com/clipping","route_hint":"article","attempts":0,"fallback":{{"relative_target":"mac_inbox.md","task_line":"- [ ] #task https://example.com/clipping [created::2026-10-07]"}}}}"#,
            serde_json::json!(vault.to_string_lossy()),
        ),
    )
    .expect("seed clipping job");

    let human = |args: &[&str]| -> String {
        let output = capture(&temp, &vault, args)
            .output()
            .expect("run human capture");
        assert_success(&output);
        stdout(&output)
    };

    assert_eq!(
        human(&["-d", ARTICLE_URL]),
        "[dry-run] ok would queue  example.com/post → reading queue\n  new to your library · clips in the background\n"
    );
    assert_eq!(
        human(&[ARTICLE_URL]),
        "✓ queued  example.com/post → reading queue\n  clipping in the background · bob ref jobs\n"
    );
    assert_eq!(
        human(&["-d", "https://example.com/captured"]),
        "[dry-run] ok already in library  ref/papers/captured.md\n  Captured Post · queued\n"
    );
    assert_eq!(
        human(&["-d", "https://example.com/queued"]),
        "[dry-run] ok already queued  xlib/blogs/queued.pdf\n  waiting for bob ref scan\n"
    );
    assert_eq!(
        human(&["-d", "https://example.com/legacy"]),
        "[dry-run] ok would queue  example.com/legacy → reading queue\n  in your library as a legacy note (ref/blogs/legacy.md) · a fresh copy will be clipped\n"
    );
    assert_eq!(
        human(&["https://example.com/legacy"]),
        "✓ queued  example.com/legacy → reading queue\n  in your library as a legacy note (ref/blogs/legacy.md) · a fresh copy will be clipped\n"
    );
    assert_eq!(
        human(&["-d", "https://example.com/clipping"]),
        "[dry-run] ok already clipping  example.com/clipping\n  a pending ref job has this link · bob ref jobs\n"
    );
    assert_eq!(
        human(&["https://example.com/clipping"]),
        "✓ already clipping  example.com/clipping\n  a pending ref job has this link · bob ref jobs\n"
    );

    for dry in [true, false] {
        let mut args = vec!["-f", "human"];
        if dry {
            args.push("-d");
        }
        let mut command = capture(&temp, &vault, &args);
        let output = run_with_stdin(
            &mut command,
            "https://example.com/dup\n\nhttps://example.com/dup",
        );
        assert_success(&output);
        let report = stdout(&output);
        assert!(
            report.contains("duplicate") && report.contains("example.com/dup"),
            "duplicate headline:\n{report}"
        );
        assert!(
            report.contains("same link as item 1"),
            "duplicate detail:\n{report}"
        );
    }
    let mut command = capture(&temp, &vault, &["-f", "json"]);
    let output = run_with_stdin(
        &mut command,
        "https://example.com/dup\n\nhttps://example.com/dup",
    );
    assert_success(&output);
    let json = parse_json(&output);
    let captures = json["captures"].as_array().expect("captures array");
    assert_eq!(captures[1]["ref"]["library"]["verdict"], "duplicate");
    assert_eq!(
        captures[1]["ref"]["library"]["message"],
        "same link as item 1"
    );
}

#[test]
fn capture_unknown_library_queues_with_message() {
    let temp = TempDir::new("bob-cli-capture-ref-unknown");
    let vault = temp.path().join("vault");
    fs::create_dir_all(vault.join("ref")).expect("create ref dir");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(
            vault.join("ref"),
            fs::Permissions::from_mode(0o000),
        )
        .expect("make ref unreadable");
    }

    // A real run queues a pending job, which would read back as
    // `clipping` (not `unknown`) for the same URL — so every run uses a
    // distinct URL.
    let cases = [
        (true, "json", "https://example.com/post-dry-json"),
        (true, "human", "https://example.com/post-dry-human"),
        (false, "json", "https://example.com/post-real-json"),
        (false, "human", "https://example.com/post-real-human"),
    ];
    let mut outputs = Vec::new();
    for (dry, mode, url) in cases {
        if mode == "human" {
            let mut human_args = vec![];
            if dry {
                human_args.push("-d");
            }
            let human_out = capture(&temp, &vault, &human_args)
                .arg(url)
                .output()
                .expect("run unknown human capture");
            outputs.push((dry, mode, url, None, Some(human_out)));
        } else {
            let mut args = vec!["-f", "json"];
            if dry {
                args.push("-d");
            }
            let json_out = capture(&temp, &vault, &args)
                .arg(url)
                .output()
                .expect("run unknown capture");
            outputs.push((dry, mode, url, Some(json_out), None));
        }
    }

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(
            vault.join("ref"),
            fs::Permissions::from_mode(0o755),
        )
        .expect("restore ref");
    }

    for (dry, mode, url, json_out, human_out) in &outputs {
        if let Some(json_out) = json_out {
            assert_success(json_out);
            let json = parse_json(json_out);
            assert_eq!(json["ref"]["library"]["verdict"], "unknown", "{json}");
            let message = json["ref"]["library"]["message"]
                .as_str()
                .unwrap_or_default();
            assert!(!message.is_empty(), "unknown carries a reason: {json}");
        }
        if let Some(human_out) = human_out {
            assert_success(human_out);
            let report = stdout(human_out);
            assert!(
                report.contains("library check unavailable:")
                    && report.contains("the clip still dedupes"),
                "unknown human wording (dry={dry}, mode={mode}, url={url}):\n{report}"
            );
        }
    }
}

#[test]
fn capture_staged_write_failure_removes_jobs() {
    let temp = TempDir::new("bob-cli-capture-ref-rollback");
    let vault = temp.path().join("vault");
    fs::create_dir_all(&vault).expect("create vault");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&vault, fs::Permissions::from_mode(0o555))
            .expect("make vault read-only");
    }

    let mut command = capture(&temp, &vault, &["-f", "json"]);
    let output =
        run_with_stdin(&mut command, "buy milk\n\nhttps://example.com/post");
    assert_eq!(output.status.code(), Some(1));
    let json = parse_json(&output);
    assert!(
        json["error"]
            .as_str()
            .unwrap_or_default()
            .contains("removed ref jobs created by this capture"),
        "{json}"
    );
    assert!(pending_files(&temp).is_empty(), "job was rolled back");

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&vault, fs::Permissions::from_mode(0o755))
            .expect("restore vault permissions");
    }
}

#[test]
fn capture_end_to_end_clips_through_the_worker() {
    let temp = TempDir::new("bob-cli-capture-ref-e2e");
    let vault = temp.path().join("vault");
    fs::create_dir_all(&vault).expect("create vault");
    let fake = FakeClip::new(&temp, "fake");
    let curl = write_article_curl(temp.path());

    let output = capture(&temp, &vault, &[ARTICLE_URL])
        .env("BOB_HIGHLIGHTS_CURL", &curl)
        .env("BOB_WEB_CLIP_ADAPTER", &fake.path)
        .output()
        .expect("run real capture");
    assert_success(&output);
    assert_eq!(pending_files(&temp).len(), 1);

    // With the kick off, the worker drains the job on demand.
    let output = bob_command()
        .arg("ref")
        .arg("jobs")
        .arg("run")
        .env("XDG_STATE_HOME", state_dir(&temp))
        .env("BOB_DIR", &vault)
        .env("BOB_HIGHLIGHTS_CURL", &curl)
        .env("BOB_WEB_CLIP_ADAPTER", &fake.path)
        .output()
        .expect("run ref jobs");
    assert_success(&output);
    assert!(fake.called(), "the adapter must clip the article");
    assert!(
        vault.join("xlib/blogs/symphony_spec.pdf").is_file(),
        "intake PDF was clipped"
    );
    assert!(pending_files(&temp).is_empty());
    assert!(!vault.join("mac_inbox.md").exists(), "no fallback task");
}

#[test]
fn capture_end_to_end_falls_back_on_blocked() {
    let temp = TempDir::new("bob-cli-capture-ref-fallback");
    let vault = temp.path().join("vault");
    fs::create_dir_all(&vault).expect("create vault");
    let fake = FakeClip::new(&temp, "fake");
    fake.respond(&failure_response("blocked", "Bot wall", "try later"));
    let curl = write_article_curl(temp.path());

    let output = capture(&temp, &vault, &[ARTICLE_URL])
        .env("BOB_HIGHLIGHTS_CURL", &curl)
        .env("BOB_WEB_CLIP_ADAPTER", &fake.path)
        .output()
        .expect("run real capture");
    assert_success(&output);

    let output = bob_command()
        .arg("ref")
        .arg("jobs")
        .arg("run")
        .env("XDG_STATE_HOME", state_dir(&temp))
        .env("BOB_DIR", &vault)
        .env("BOB_HIGHLIGHTS_CURL", &curl)
        .env("BOB_WEB_CLIP_ADAPTER", &fake.path)
        .output()
        .expect("run ref jobs");
    assert_success(&output);
    // The fallback is exactly the routing-off task line plus the ⚠️ child.
    assert_eq!(
        fs::read_to_string(vault.join("mac_inbox.md")).expect("read inbox"),
        format!("{TASK_LINE}\n\t- ⚠️ Clip failed (blocked): blocked: Bot wall · retry: bob ref create {ARTICLE_URL}\n")
    );
}

#[test]
fn capture_with_kick_returns_before_the_clip_finishes() {
    let temp = TempDir::new("bob-cli-capture-ref-kick");
    let vault = temp.path().join("vault");
    fs::create_dir_all(&vault).expect("create vault");
    // A slow adapter: if the kick held the call's pipes open, this
    // capture would take the whole sleep to return.
    let slow = temp.path().join("slow-clip.sh");
    write_executable(&slow, "#!/bin/sh\nsleep 20\n");
    let curl = write_article_curl(temp.path());

    let started = Instant::now();
    let output = capture(&temp, &vault, &["-f", "json", ARTICLE_URL])
        .env_remove("BOB_REF_JOBS_KICK")
        .env("BOB_HIGHLIGHTS_CURL", &curl)
        .env("BOB_WEB_CLIP_ADAPTER", &slow)
        .output()
        .expect("run kicking capture");
    let elapsed = started.elapsed();
    assert_success(&output);
    assert_eq!(parse_json(&output)["placement"], "queued");
    assert!(
        elapsed.as_secs() < 10,
        "capture must return before the 20s clip finishes (took {elapsed:?})"
    );
    assert_eq!(queued_files(&temp).len(), 1);
}

#[test]
fn capture_parse_reports_ref_mode_and_span() {
    let temp = TempDir::new("bob-cli-capture-ref-parse");
    let vault = temp.path().join("vault");
    fs::create_dir_all(&vault).expect("create vault");

    let output = bob_command()
        .arg("capture-parse")
        .arg("-f")
        .arg("json")
        .arg("--")
        .arg(ARTICLE_URL)
        .output()
        .expect("run capture-parse");
    assert_success(&output);
    let json: serde_json::Value =
        serde_json::from_str(stdout(&output).trim()).expect("parse JSON");
    assert_eq!(json["mode"], "ref");
    assert_eq!(
        json["spans"],
        serde_json::json!([{"start": 0, "end": 24, "kind": "ref_url"}])
    );

    // `-R` keeps parse and capture in agreement: the URL stays a task.
    let output = bob_command()
        .arg("capture-parse")
        .arg("-f")
        .arg("json")
        .arg("-R")
        .arg("--")
        .arg(ARTICLE_URL)
        .output()
        .expect("run capture-parse with -R");
    assert_success(&output);
    let json: serde_json::Value =
        serde_json::from_str(stdout(&output).trim()).expect("parse JSON");
    assert_eq!(json["mode"], "task");

    // A URL-list block reports one item per line with exact ranges.
    let output = bob_command()
        .arg("capture-parse")
        .arg("-f")
        .arg("json")
        .arg("--")
        .arg("https://example.com/1\nhttps://example.com/2")
        .output()
        .expect("run capture-parse list");
    assert_success(&output);
    let json: serde_json::Value =
        serde_json::from_str(stdout(&output).trim()).expect("parse JSON");
    let items = json["items"].as_array().expect("items array");
    assert_eq!(items.len(), 2);
    assert!(items.iter().all(|item| item["mode"] == "ref"));
}
