use std::{
    env, fs,
    path::{Path, PathBuf},
    sync::atomic::Ordering,
};

use chrono::NaiveDateTime;
use rusqlite::Connection;

use super::{
    clipboard::{merge_history_candidates, normalize_clipboard_output},
    clipy::read_clipy_history,
    files::{percent_decode, sanitize_file_name, sha256_hex, snippet_slug},
    is_valid_header,
    persist::TEMP_COUNTER,
    plan::{
        is_structural_line, MAX_ATTACHMENT_COUNT, MAX_INLINE_CHARACTERS,
        MAX_LINES,
    },
    render::rendered_lines,
    rendered_header, ClipMode, ClipPlan,
};

fn plan(
    bob_dir: &Path,
    header: Option<&str>,
    clipboard: &str,
    now: NaiveDateTime,
) -> Result<ClipPlan, String> {
    super::plan(bob_dir, header, clipboard, now, "  ")
}

fn plan_history(
    bob_dir: &Path,
    clipboards: &[String],
    now: NaiveDateTime,
) -> Result<ClipPlan, String> {
    super::plan_history(bob_dir, clipboards, now, "  ")
}

fn test_root(label: &str) -> PathBuf {
    let sequence = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    let root = env::temp_dir().join(format!(
        "bob-capture-clip-{label}-{}-{sequence}",
        std::process::id()
    ));
    fs::create_dir_all(&root).expect("create test root");
    root
}

fn test_now() -> NaiveDateTime {
    NaiveDateTime::parse_from_str("2026-07-15 13:14:15", "%Y-%m-%d %H:%M:%S")
        .expect("time")
}

#[test]
fn formats_headers() {
    assert_eq!(rendered_header("clip"), "CLIP");
    assert_eq!(rendered_header("foo_bar_baz"), "FOO BAR BAZ");
    assert_eq!(rendered_header("foo-bar2"), "FOO-BAR2");
    assert!(is_valid_header("A_b-2"));
    assert!(!is_valid_header("bad!"));
}

#[test]
fn normalizes_clipboard_text_and_rejects_binary_or_empty() {
    assert_eq!(
        normalize_clipboard_output(b"one\r\ntwo\rthree\n \n".to_vec())
            .expect("normalize"),
        "one\ntwo\nthree"
    );
    assert!(normalize_clipboard_output(vec![b'a', 0]).is_err());
    assert!(normalize_clipboard_output(vec![0xff]).is_err());
    assert_eq!(
        normalize_clipboard_output(b" \n\n".to_vec()).unwrap_err(),
        "clipboard is empty"
    );
}

#[test]
fn merges_live_clipboard_with_up_to_date_and_lagging_histories() {
    assert_eq!(
        merge_history_candidates(
            "current".to_string(),
            vec![
                "current".to_string(),
                "older".to_string(),
                "oldest".to_string(),
            ],
            3,
        )
        .expect("up-to-date history"),
        ["current", "older", "oldest"]
    );
    assert_eq!(
        merge_history_candidates(
            "current".to_string(),
            vec![
                "older".to_string(),
                "current".to_string(),
                "oldest".to_string(),
            ],
            3,
        )
        .expect("lagging history"),
        ["current", "older", "oldest"]
    );
    assert_eq!(
        merge_history_candidates(
            "same".to_string(),
            vec!["same".to_string(), "same".to_string(), "older".to_string(),],
            3,
        )
        .expect("later duplicate"),
        ["same", "same", "older"]
    );
    let error = merge_history_candidates(
        "current".to_string(),
        vec!["older".to_string()],
        3,
    )
    .expect_err("insufficient history");
    assert!(error.contains("requested 3") && error.contains("only 2"));
}

#[test]
fn reads_clipy_sqlite_assets_in_deterministic_order() {
    let root = test_root("clipy-database");
    let database = root.join("sqlite.db");
    let connection = Connection::open(&database).expect("fixture database");
    create_clipy_fixture_schema(&connection);
    connection
        .execute(
            "INSERT INTO pasteboardHistories (id, updateAt) VALUES (?1, ?2)",
            ("older", 1_i64),
        )
        .expect("older history");
    connection
        .execute(
            "INSERT INTO pasteboardHistories (id, updateAt) VALUES (?1, ?2)",
            ("same-a", 2_i64),
        )
        .expect("tie history a");
    connection
        .execute(
            "INSERT INTO pasteboardHistories (id, updateAt) VALUES (?1, ?2)",
            ("same-b", 2_i64),
        )
        .expect("tie history b");
    insert_clipy_asset(
        &connection,
        "asset-1",
        "same-b",
        0,
        "public.utf8-plain-text",
        b"newest\r\ntext",
    );
    insert_clipy_asset(
        &connection,
        "asset-2",
        "same-a",
        0,
        "public.file-url",
        b"file:///tmp/first.txt",
    );
    insert_clipy_asset(
        &connection,
        "asset-3",
        "same-a",
        1,
        "public.file-url",
        b"file:///tmp/second.txt",
    );
    let mut filenames = Vec::new();
    plist::Value::Array(vec![
        plist::Value::String("/tmp/legacy one.txt".to_string()),
        plist::Value::String("/tmp/legacy two.txt".to_string()),
    ])
    .to_writer_xml(&mut filenames)
    .expect("filenames plist");
    insert_clipy_asset(
        &connection,
        "asset-4",
        "older",
        0,
        "NSFilenamesPboardType",
        &filenames,
    );
    drop(connection);

    let history = read_clipy_history(&database, 4).expect("read Clipy fixture");
    assert_eq!(
        history,
        [
            "newest\r\ntext",
            "file:///tmp/first.txt\nfile:///tmp/second.txt",
            "/tmp/legacy one.txt\n/tmp/legacy two.txt",
        ]
    );
    let normalized = history
        .into_iter()
        .map(|value| normalize_clipboard_output(value.into_bytes()))
        .collect::<Result<Vec<_>, _>>()
        .expect("normalize fixture history");
    let error =
        merge_history_candidates("newest\ntext".to_string(), normalized, 4)
            .expect_err("fixture is insufficient for four entries");
    assert!(error.contains("requested 4") && error.contains("only 3"));
    fs::remove_dir_all(root).expect("cleanup");
}

#[test]
fn rejects_unsupported_and_unmigrated_clipy_databases() {
    let root = test_root("clipy-errors");
    let unsupported = root.join("unsupported.db");
    let connection = Connection::open(&unsupported).expect("fixture");
    create_clipy_fixture_schema(&connection);
    connection
        .execute(
            "INSERT INTO pasteboardHistories (id, updateAt) VALUES ('image', 1)",
            [],
        )
        .expect("history");
    insert_clipy_asset(
        &connection,
        "asset",
        "image",
        0,
        "public.png",
        b"binary",
    );
    drop(connection);
    let error = read_clipy_history(&unsupported, 1)
        .expect_err("binary-only entry must fail");
    assert!(
        error.contains("entry 1") && error.contains("public.png"),
        "{error}"
    );

    let unmigrated = root.join("unmigrated.db");
    let connection = Connection::open(&unmigrated).expect("fixture");
    connection
        .execute("CREATE TABLE pasteboardHistories (id TEXT)", [])
        .expect("partial schema");
    drop(connection);
    let error = read_clipy_history(&unmigrated, 1)
        .expect_err("unmigrated database must fail");
    assert!(
        error.contains("unsupported or unmigrated")
            && error.contains("updateAt"),
        "{error}"
    );
    fs::remove_dir_all(root).expect("cleanup");
}

#[test]
fn detects_structural_lines() {
    for line in [
        "- item",
        "* item",
        "+ item",
        "1. item",
        "# h",
        "> q",
        "```",
        "~~~",
        "  indented",
    ] {
        assert!(is_structural_line(line), "{line}");
    }
    for line in ["plain", "1.item", ">quote"] {
        assert!(!is_structural_line(line), "{line}");
    }
}

#[test]
fn recognizes_and_renders_flat_unordered_lists() {
    let root = Path::new("/tmp/unused-bob-clip-list-test");
    let now = test_now();
    let requested = concat!(
        "- Use `@` symbol instead of `#` for tribe prefix.\n",
        "- Support expansion of families within clan.\n",
        "- Family members must be launched sequentially.",
    );
    let output = plan(root, None, requested, now).expect("dash list").output;
    assert_eq!(output.mode, ClipMode::Lines);
    assert_eq!(
        output.lines,
        [
            "  - Use `@` symbol instead of `#` for tribe prefix.",
            "  - Support expansion of families within clan.",
            "  - Family members must be launched sequentially.",
        ]
    );

    let mixed_markers = plan(
        root,
        None,
        "*\tfirst with **bold**\n+   second with [link](target)\n- [ ] checkbox",
        now,
    )
    .expect("mixed unordered markers")
    .output;
    assert_eq!(mixed_markers.mode, ClipMode::Lines);
    assert_eq!(
        mixed_markers.lines,
        [
            "  - first with **bold**",
            "  - second with [link](target)",
            "  - [ ] checkbox",
        ]
    );

    let single = plan(root, None, "+ one item", now)
        .expect("single item")
        .output;
    assert_eq!(single.mode, ClipMode::Lines);
    assert_eq!(single.lines, ["  - one item"]);

    let headed = plan(root, Some("build_log"), "- first\n* second", now)
        .expect("headed list")
        .output;
    assert_eq!(headed.mode, ClipMode::Lines);
    assert_eq!(
        headed.lines,
        ["  - **BUILD LOG:**", "    - first", "    - second"]
    );
}

#[test]
fn flat_unordered_lists_keep_the_inline_line_boundary() {
    let root = Path::new("/tmp/unused-bob-clip-list-boundary-test");
    let ten_items = (1..=MAX_LINES)
        .map(|index| format!("- item {index}"))
        .collect::<Vec<_>>()
        .join("\n");
    let ten = plan(root, None, &ten_items, test_now())
        .expect("ten-item list")
        .output;
    assert_eq!(ten.mode, ClipMode::Lines);
    assert_eq!(ten.lines.len(), MAX_LINES);
    assert_eq!(ten.lines[0], "  - item 1");
    assert_eq!(ten.lines[MAX_LINES - 1], "  - item 10");

    let eleven_items = format!("{ten_items}\n- item 11");
    assert_eq!(
        plan(root, None, &eleven_items, test_now())
            .expect("over-limit list")
            .output
            .mode,
        ClipMode::Snippet
    );
}

#[test]
fn unsafe_or_incomplete_unordered_lists_remain_snippets() {
    let root = test_root("unsafe-lists");
    let vault = root.join("vault");
    fs::create_dir_all(&vault).expect("vault");
    for text in [
        "- first\n- ",
        "- first\nplain prose",
        "1. first\n2. second",
        "- parent\n  - nested",
        "- wrapped item\ncontinuation",
        "- first\n\n- second",
        "- first\n> quote",
        "- first\n# heading",
        "- first\n```\n- second",
    ] {
        assert_eq!(
            plan(&vault, None, text, test_now())
                .expect("unsafe list snippet")
                .output
                .mode,
            ClipMode::Snippet,
            "{text:?}"
        );
    }
    fs::remove_dir_all(root).expect("cleanup");
}

#[test]
fn sanitizes_attachment_names_and_builds_slugs() {
    assert_eq!(sanitize_file_name(" .a::b[#]文.md. "), "a-b-文.md");
    assert_eq!(sanitize_file_name("..."), "attachment");
    assert_eq!(
        snippet_slug("Hello, Wonderful World!\nnext").as_deref(),
        Some("hello-wonderful-world")
    );
}

#[test]
fn renders_inline_lines_and_long_text_modes() {
    let root = Path::new("/tmp/unused-bob-clip-test");
    let now = test_now();
    let headerless_inline =
        plan(root, None, "hello", now).expect("headerless inline");
    assert_eq!(headerless_inline.output.header, None);
    assert_eq!(headerless_inline.output.mode, ClipMode::Inline);
    assert_eq!(headerless_inline.output.lines, ["  - hello"]);
    let single_json = serde_json::to_value(&headerless_inline.output)
        .expect("single output JSON");
    assert_eq!(single_json["entries"], serde_json::json!([]));

    let inline = plan(root, Some("clip"), "hello", now).expect("headed inline");
    assert_eq!(inline.output.header.as_deref(), Some("CLIP"));
    assert_eq!(inline.output.mode, ClipMode::Inline);
    assert_eq!(inline.output.lines, ["  - **CLIP:** hello"]);

    let headerless_lines =
        plan(root, None, "one\ntwo", now).expect("headerless lines");
    assert_eq!(headerless_lines.output.header, None);
    assert_eq!(headerless_lines.output.mode, ClipMode::Lines);
    assert_eq!(headerless_lines.output.lines, ["  - one", "  - two"]);

    let lines = plan(root, Some("log"), "one\ntwo", now).expect("headed lines");
    assert_eq!(lines.output.mode, ClipMode::Lines);
    assert_eq!(
        lines.output.lines,
        ["  - **LOG:**", "    - one", "    - two"]
    );

    let long = "x".repeat(MAX_INLINE_CHARACTERS + 1);
    let headerless_snippet =
        plan(root, None, &long, now).expect("headerless snippet");
    assert_eq!(headerless_snippet.output.header, None);
    assert_eq!(headerless_snippet.output.mode, ClipMode::Snippet);
    assert_eq!(
        headerless_snippet.output.lines,
        ["  - [[file/clip-20260715-131415-xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx]]"]
    );

    let snippet = plan(root, Some("clip"), &long, now).expect("headed snippet");
    assert_eq!(snippet.output.mode, ClipMode::Snippet);
    assert_eq!(
        snippet.output.lines,
        ["  - **CLIP:** [[file/clip-20260715-131415-xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx]]"]
    );
    assert_eq!(
        snippet.output.snippet.as_deref(),
        Some("file/clip-20260715-131415-xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx.md")
    );
}

#[test]
fn tab_indent_renders_every_clipboard_shape() {
    assert_eq!(
        rendered_lines(None, &["one".to_string(), "two".to_string()], "\t"),
        ["\t- one", "\t- two"]
    );
    assert_eq!(
        rendered_lines(Some("CLIP"), &["one".to_string()], "\t"),
        ["\t- **CLIP:** one"]
    );
    assert_eq!(
        rendered_lines(
            Some("CLIP"),
            &["one".to_string(), "two".to_string()],
            "\t",
        ),
        ["\t- **CLIP:**", "\t\t- one", "\t\t- two"]
    );

    let root = test_root("tab-indent-shapes");
    let vault = root.join("vault");
    let attachment = root.join("image.png");
    fs::create_dir_all(&vault).expect("vault");
    fs::write(&attachment, b"image").expect("attachment");
    let attachment_plan = super::plan(
        &vault,
        Some("photo"),
        attachment.to_str().expect("UTF-8 attachment path"),
        test_now(),
        "\t",
    )
    .expect("tab-indented attachment");
    assert_eq!(
        attachment_plan.output.lines,
        ["\t- **PHOTO:** ![[img/image.png|400]]"]
    );

    let snippet_plan =
        super::plan(&vault, None, "# Structured\n\nbody", test_now(), "\t")
            .expect("tab-indented snippet");
    assert_eq!(
        snippet_plan.output.lines,
        ["\t- [[file/clip-20260715-131415-structured]]"]
    );
    fs::remove_dir_all(root).expect("cleanup");
}

#[test]
fn percent_decodes_file_uris() {
    assert_eq!(
        percent_decode("/tmp/hello%20world.md").expect("decode"),
        "/tmp/hello world.md"
    );
    assert!(percent_decode("/tmp/%xx").is_err());
}

#[test]
fn classifies_paths_structured_text_and_attachment_limits() {
    let root = test_root("classify");
    let vault = root.join("vault");
    let source = root.join("hello world.png");
    fs::create_dir_all(&vault).expect("vault");
    fs::write(&source, b"image").expect("source");

    let uri = format!("file://{}", source.display()).replace(' ', "%20");
    let attachment = plan(&vault, Some("photo"), &uri, test_now())
        .expect("file URI attachment");
    assert_eq!(attachment.output.mode, ClipMode::Attachments);
    assert_eq!(
        attachment.output.lines,
        ["  - **PHOTO:** ![[img/hello world.png|400]]"]
    );
    let quoted = plan(
        &vault,
        Some("clip"),
        &format!("\"{}\"", source.display()),
        test_now(),
    )
    .expect("quoted attachment");
    assert_eq!(quoted.output.mode, ClipMode::Attachments);

    let document = root.join("document.pdf");
    fs::write(&document, b"document").expect("document source");
    let multiple = plan(
        &vault,
        Some("clip"),
        &format!("{}\n{}", source.display(), document.display()),
        test_now(),
    )
    .expect("multiple attachments");
    assert_eq!(
        multiple.output.lines,
        [
            "  - **CLIP:**",
            "    - ![[img/hello world.png|400]]",
            "    - [[file/document.pdf]]",
        ]
    );

    let headerless_attachment =
        plan(&vault, None, &uri, test_now()).expect("headerless attachment");
    assert_eq!(headerless_attachment.output.header, None);
    assert_eq!(
        headerless_attachment.output.lines,
        ["  - ![[img/hello world.png|400]]"]
    );
    let headerless_multiple = plan(
        &vault,
        None,
        &format!("{}\n{}", source.display(), document.display()),
        test_now(),
    )
    .expect("headerless multiple attachments");
    assert_eq!(
        headerless_multiple.output.lines,
        [
            "  - ![[img/hello world.png|400]]",
            "  - [[file/document.pdf]]",
        ]
    );

    let directory = plan(
        &vault,
        Some("clip"),
        root.to_str().expect("utf8 root"),
        test_now(),
    )
    .expect("directories fall through");
    assert_eq!(directory.output.mode, ClipMode::Inline);

    for text in ["one\n\ntwo", " one\ntwo"] {
        assert_eq!(
            plan(&vault, Some("clip"), text, test_now())
                .expect("snippet")
                .output
                .mode,
            ClipMode::Snippet,
            "{text:?}"
        );
    }

    let missing = root.join("missing.txt");
    let error = plan(
        &vault,
        Some("clip"),
        missing.to_str().expect("utf8 path"),
        test_now(),
    )
    .expect_err("missing attachment");
    assert!(error.contains("does not exist"), "{error}");

    let mut attachment_paths = Vec::new();
    for index in 0..=MAX_ATTACHMENT_COUNT {
        let path = root.join(format!("attachment-{index}.txt"));
        fs::write(&path, index.to_string()).expect("attachment source");
        attachment_paths.push(path.display().to_string());
    }
    let error = plan(
        &vault,
        Some("clip"),
        &attachment_paths.join("\n"),
        test_now(),
    )
    .expect_err("too many attachments");
    assert!(error.contains("11 attachments"), "{error}");

    let plain_lines = (0..=MAX_LINES)
        .map(|index| format!("line {index}"))
        .collect::<Vec<_>>()
        .join("\n");
    assert_eq!(
        plan(&vault, Some("clip"), &plain_lines, test_now())
            .expect("long multiline snippet")
            .output
            .mode,
        ClipMode::Snippet
    );
    fs::remove_dir_all(root).expect("cleanup");
}

#[test]
fn saves_reuses_and_hash_suffixes_attachments_atomically() {
    let root = test_root("save");
    let vault = root.join("vault");
    let first_dir = root.join("first");
    let second_dir = root.join("second");
    fs::create_dir_all(&vault).expect("vault");
    fs::create_dir_all(&first_dir).expect("first dir");
    fs::create_dir_all(&second_dir).expect("second dir");
    let first = first_dir.join("report:final.txt");
    let second = second_dir.join("report:final.txt");
    fs::write(&first, b"one").expect("first source");
    fs::write(&second, b"two").expect("second source");

    let initial = plan(
        &vault,
        Some("clip"),
        first.to_str().expect("utf8 path"),
        test_now(),
    )
    .expect("initial plan");
    assert_eq!(initial.output.attachments[0].saved, "file/report-final.txt");
    let created = initial.save().expect("save attachment");
    assert_eq!(created.len(), 1);

    let reused = plan(
        &vault,
        Some("clip"),
        first.to_str().expect("utf8 path"),
        test_now(),
    )
    .expect("reuse plan");
    assert!(reused.output.attachments[0].reused);
    assert!(reused.save().expect("reuse").is_empty());

    let differing = plan(
        &vault,
        Some("clip"),
        second.to_str().expect("utf8 path"),
        test_now(),
    )
    .expect("hash plan");
    let expected_hash = &sha256_hex(b"two")[..8];
    assert_eq!(
        differing.output.attachments[0].saved,
        format!("file/report-final-{expected_hash}.txt")
    );
    differing.save().expect("save hashed attachment");
    fs::remove_dir_all(root).expect("cleanup");
}

#[test]
fn snippet_names_use_deterministic_collision_counters() {
    let root = test_root("snippet");
    let vault = root.join("vault");
    fs::create_dir_all(&vault).expect("vault");
    let text = "# Structured title\nbody";
    let first = plan(&vault, Some("clip"), text, test_now()).expect("first");
    assert_eq!(
        first.output.snippet.as_deref(),
        Some("file/clip-20260715-131415-structured-title.md")
    );
    first.save().expect("save first");
    let second = plan(&vault, Some("clip"), text, test_now()).expect("second");
    assert_eq!(
        second.output.snippet.as_deref(),
        Some("file/clip-20260715-131415-structured-title-2.md")
    );
    fs::remove_dir_all(root).expect("cleanup");
}

#[test]
fn aggregate_planner_flattens_entries_and_reserves_all_paths() {
    let root = test_root("aggregate");
    let vault = root.join("vault");
    let first_dir = root.join("first");
    let second_dir = root.join("second");
    fs::create_dir_all(&vault).expect("vault");
    fs::create_dir_all(&first_dir).expect("first dir");
    fs::create_dir_all(&second_dir).expect("second dir");
    let first = first_dir.join("report.txt");
    let second = second_dir.join("report.txt");
    fs::write(&first, b"same").expect("first source");
    fs::write(&second, b"different").expect("second source");
    let snippet = "# Heading\n\nbody".to_string();
    let clipboards = vec![
        "inline".to_string(),
        "line one\nline two".to_string(),
        first.display().to_string(),
        second.display().to_string(),
        first.display().to_string(),
        snippet.clone(),
        snippet,
    ];
    let plan =
        plan_history(&vault, &clipboards, test_now()).expect("aggregate");

    assert_eq!(plan.output.mode, ClipMode::History);
    assert_eq!(plan.output.header, None);
    assert_eq!(plan.output.entries.len(), clipboards.len());
    assert_eq!(plan.output.lines[0], "  - inline");
    assert_eq!(&plan.output.lines[1..3], ["  - line one", "  - line two"]);
    assert!(plan
        .output
        .lines
        .iter()
        .all(|line| !line.contains("**CLIP:**")));
    let expected_hash = &sha256_hex(b"different")[..8];
    assert_eq!(
        plan.output
            .attachments
            .iter()
            .map(|attachment| attachment.saved.as_str())
            .collect::<Vec<_>>(),
        [
            "file/report.txt",
            &format!("file/report-{expected_hash}.txt"),
            "file/report.txt",
        ]
    );
    assert_eq!(
        plan.output.entries[5].snippet.as_deref(),
        Some("file/clip-20260715-131415-heading.md")
    );
    assert_eq!(
        plan.output.entries[6].snippet.as_deref(),
        Some("file/clip-20260715-131415-heading-2.md")
    );
    assert_eq!(plan.files.len(), 4, "unique files only");
    assert_eq!(plan.output.file_confirmations().len(), 4);
    let aggregate_json =
        serde_json::to_value(&plan.output).expect("aggregate JSON");
    assert_eq!(aggregate_json["mode"], "history");
    assert_eq!(aggregate_json["entries"].as_array().unwrap().len(), 7);
    assert_eq!(aggregate_json["attachments"].as_array().unwrap().len(), 3);
    assert!(aggregate_json.get("snippet").is_none(), "{aggregate_json}");
    let created = plan.save().expect("save aggregate");
    assert_eq!(created.len(), 4);
    fs::remove_dir_all(root).expect("cleanup");
}

#[test]
fn aggregate_save_cleans_up_files_after_a_later_failure() {
    let root = test_root("aggregate-cleanup");
    let vault = root.join("vault");
    fs::create_dir_all(&vault).expect("vault");
    fs::write(vault.join("file"), b"blocks snippet directory")
        .expect("blocking file");
    let image = root.join("image.png");
    fs::write(&image, b"image").expect("image source");
    let clipboards = vec![
        image.display().to_string(),
        "# Structured\n\ncontent".to_string(),
    ];
    let plan = plan_history(&vault, &clipboards, test_now()).expect("plan");
    let error = plan.save().expect_err("second save must fail");
    assert!(error.contains("removed clipboard files"), "{error}");
    assert!(!vault.join("img/image.png").exists());
    fs::remove_dir_all(root).expect("cleanup");
}

#[test]
fn aggregate_planner_does_not_alias_snippets_and_attachments() {
    let root = test_root("aggregate-cross-kind");
    let vault = root.join("vault");
    fs::create_dir_all(&vault).expect("vault");
    let text = "# Heading\n\nbody";
    let attachment = root.join("clip-20260715-131415-heading.md");
    fs::write(&attachment, format!("{text}\n")).expect("attachment source");
    let plan = plan_history(
        &vault,
        &[text.to_string(), attachment.display().to_string()],
        test_now(),
    )
    .expect("cross-kind aggregate");
    let hash = sha256_hex(format!("{text}\n").as_bytes());
    assert_eq!(
        plan.output.entries[0].snippet.as_deref(),
        Some("file/clip-20260715-131415-heading.md")
    );
    assert_eq!(
        plan.output.entries[1].attachments[0].saved,
        format!("file/clip-20260715-131415-heading-{}.md", &hash[..8])
    );
    assert_eq!(plan.files.len(), 2);
    fs::remove_dir_all(root).expect("cleanup");
}

fn create_clipy_fixture_schema(connection: &Connection) {
    connection
        .execute_batch(
            "CREATE TABLE pasteboardHistories (\
               id TEXT PRIMARY KEY NOT NULL, updateAt INTEGER NOT NULL\
             );\
             CREATE TABLE pasteboardHistoryAssets (\
               id TEXT PRIMARY KEY NOT NULL,\
               pasteboardHistoryID TEXT NOT NULL,\
               \"index\" INTEGER NOT NULL,\
               pasteboardType TEXT NOT NULL, data BLOB NOT NULL\
             );",
        )
        .expect("Clipy fixture schema");
}

fn insert_clipy_asset(
    connection: &Connection,
    id: &str,
    history_id: &str,
    index: i64,
    pasteboard_type: &str,
    data: &[u8],
) {
    connection
        .execute(
            "INSERT INTO pasteboardHistoryAssets \
             (id, pasteboardHistoryID, \"index\", pasteboardType, data) \
             VALUES (?1, ?2, ?3, ?4, ?5)",
            (id, history_id, index, pasteboard_type, data),
        )
        .expect("Clipy fixture asset");
}
