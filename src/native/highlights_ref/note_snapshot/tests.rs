//! Model-contract and serializer tests using synthetic notes only.

use std::{
    fs,
    io::Write,
    path::PathBuf,
    process::{Command, Stdio},
};

use sha2::Digest;

use super::*;

fn source_identity() -> SourceIdentity {
    SourceIdentity {
        canonical_path: PathBuf::from("/synthetic-vault/Contract.md"),
        vault_relative_path: "Contract.md".to_string(),
        route: "Contract".to_string(),
    }
}

fn anchor(source: &SourceIdentity, fragment: &str, id: &str) -> SnapshotAnchor {
    SnapshotAnchor {
        identity: AnchorIdentity {
            source: source.clone(),
            kind: if id.starts_with("t-") {
                AnchorKind::Block
            } else {
                AnchorKind::Heading
            },
            fragment: fragment.to_string(),
        },
        id: id.to_string(),
    }
}

fn text(value: &str) -> Inline {
    Inline::Text(value.to_string())
}

fn task(
    status: SnapshotTaskStatus,
    content: &str,
    task_anchor: Option<SnapshotAnchor>,
) -> Block {
    Block::Task(SnapshotTask {
        status,
        content: vec![text(content)],
        chips: Vec::new(),
        anchor: task_anchor,
        children: Vec::new(),
    })
}

pub(super) fn contract_view() -> SnapshotView {
    let source = source_identity();
    let requirements = anchor(&source, "requirements", "requirements");
    let fasteners = anchor(&source, "fasteners", "fasteners");
    let next = anchor(&source, "next-task", "t-next");
    let blocked = anchor(&source, "intro-proposal", "t-intro-proposal");
    let closed = anchor(&source, "closed", "closed");
    let omitted = anchor(&source, "omitted-task", "t-omitted");
    let anchors = vec![
        requirements.clone(),
        fasteners.clone(),
        next.clone(),
        blocked.clone(),
        closed.clone(),
    ];

    let open_task = SnapshotTask {
        status: SnapshotTaskStatus::Open,
        content: vec![text("Open task")],
        chips: vec![
            Chip {
                content: vec![text("sched Oct 17")],
                kind: ChipKind::Normal,
                target: None,
            },
            Chip {
                content: vec![text("high")],
                kind: ChipKind::Accent,
                target: None,
            },
            Chip {
                content: vec![text("Keep")],
                kind: ChipKind::Normal,
                target: Some(LinkTarget::Url(
                    "https://example.test/keep?x=1&y=2".to_string(),
                )),
            },
        ],
        anchor: None,
        children: Vec::new(),
    };
    let next_task = SnapshotTask {
        status: SnapshotTaskStatus::Next,
            content: vec![
                text("Brief the "),
                Inline::Link {
                    label: vec![text("night crew")],
                    target: LinkTarget::ObsidianUri(
                        "obsidian://open?vault=bob&file=night_crew.md".to_string(),
                    ),
                },
            ],
            chips: vec![Chip {
                content: vec![text("Keep")],
                kind: ChipKind::Normal,
                target: Some(LinkTarget::Url(
                    "https://example.com/keep/night-crew".to_string(),
                )),
            }],
        anchor: Some(next.clone()),
        children: vec![Block::List {
            kind: ListKind::Unordered,
            items: vec![ListItem {
                content: vec![text("Nested child")],
                children: vec![Block::List {
                    kind: ListKind::Ordered { start: 2 },
                    items: vec![ListItem {
                        content: vec![text("Second level")],
                        children: Vec::new(),
                    }],
                }],
            }],
        }],
    };
    let blocked_task = SnapshotTask {
        status: SnapshotTaskStatus::Blocked,
        content: vec![text("Blocked task")],
        chips: Vec::new(),
        anchor: Some(blocked.clone()),
        children: vec![
            Block::LabeledRow {
                label: vec![text("Waits on")],
                items: vec![
                    vec![Inline::Link {
                        label: vec![text("Tide Office")],
                        target: LinkTarget::ObsidianUri(
                            "obsidian://open?vault=bob&file=tide_office.md".to_string(),
                        ),
                    }],
                    vec![Inline::Link {
                        label: vec![text("Pier Nine")],
                        target: LinkTarget::ObsidianUri(
                            "obsidian://open?vault=bob&file=pier_nine.md".to_string(),
                        ),
                    }],
                ],
            },
            Block::List {
                kind: ListKind::Unordered,
                items: vec![ListItem {
                    content: vec![text("A nested ordinary list under a task:")],
                    children: vec![
                        Block::List {
                            kind: ListKind::Unordered,
                            items: vec![
                                ListItem {
                                    content: vec![
                                        text("first bullet with a "),
                                        Inline::Link {
                                            label: vec![text("child link")],
                                            target: LinkTarget::Url(
                                                "https://example.com/child".to_string(),
                                            ),
                                        },
                                    ],
                                    children: Vec::new(),
                                },
                                ListItem {
                                    content: vec![
                                        text("second bullet with "),
                                        Inline::Code("code_token".to_string()),
                                        text(" and "),
                                        Inline::Emphasis(vec![text("emphasis")]),
                                    ],
                                    children: Vec::new(),
                                },
                            ],
                        },
                        Block::List {
                            kind: ListKind::Ordered { start: 1 },
                            items: vec![
                                ListItem {
                                    content: vec![text("check the flood lamps")],
                                    children: Vec::new(),
                                },
                                ListItem {
                                    content: vec![text("log the gull nests")],
                                    children: Vec::new(),
                                },
                            ],
                        },
                    ],
                }],
            },
        ],
    };

    let blocks = vec![
        Block::Paragraph(vec![
            text("Punctuation: [square], _marks_, $69.00, :sparkles:, and @ROUTE; Unicode: café — naïve. A short prose paragraph with emphasis, inline code, and a "),
            Inline::Link {
                label: vec![text("harbor link")],
                target: LinkTarget::ObsidianUri(
                    "obsidian://open?vault=bob&file=harbor.md".to_string(),
                ),
            },
            text(". See "),
            Inline::Link {
                label: vec![text("Closed tasks")],
                target: LinkTarget::Internal(closed.identity.clone()),
            },
            text(" for history. Literal specials: % & _ # $ { } and a backslash \\ stay visible. A route @BANK and an amount $69,044.29 and a literal :shortcode: never become citations, math, or emoji. Office ligature check: office offers efficient coffee. A long token that must wrap without visible spacers: "),
            Inline::Code("a_very_long_harbor_crane_identifier/with/slashes-and.dots.that.keeps.going/and_going_onward".to_string()),
            text(". "),
            Inline::Emphasis(vec![text("emphasis")]),
            text(" "),
            Inline::Strong(vec![text("strong")]),
            text(" "),
            Inline::Code("a `code` sample".to_string()),
            text(" "),
            Inline::Muted(vec![text("[[due:: soon]]")]),
            text(" "),
            Inline::Chip(Chip {
                content: vec![text("inline tag")],
                kind: ChipKind::Normal,
                target: None,
            }),
            Inline::Break,
            text("after a hard break"),
        ]),
        Block::Paragraph(vec![Inline::Strike(vec![
            text("struck "),
            Inline::Strong(vec![
                text("bold "),
                Inline::Link {
                    label: vec![text("visible link")],
                    target: LinkTarget::Internal(next.identity.clone()),
                },
            ]),
            text(" and "),
            Inline::Link {
                label: vec![text("omitted item")],
                target: LinkTarget::Internal(omitted.identity.clone()),
            },
            text(" tail"),
        ])]),
        Block::Paragraph(vec![text("A long token sample and a link to "), Inline::Link {
            label: vec![text("fasteners")],
            target: LinkTarget::Url("https://example.com/fasteners".to_string()),
        }, text(" plus "), Inline::Code("mono_token".to_string()), text("." )]),
        Block::Heading(SnapshotHeading {
            level: HeadingLevel::H2,
            content: vec![text("Next & In Progress")],
            class: HeadingClass::Lane,
            anchor: anchor(&source, "next-lane", "next-lane"),
        }),
        Block::Heading(SnapshotHeading {
            level: HeadingLevel::H3,
            content: vec![text("Fastener notes")],
            class: HeadingClass::Normal,
            anchor: fasteners,
        }),
        Block::Task(open_task),
        Block::Task(next_task),
        Block::Task(SnapshotTask {
            status: SnapshotTaskStatus::Wip,
            content: vec![text("Work in progress")],
            chips: Vec::new(),
            anchor: None,
            children: Vec::new(),
        }),
        Block::Task(blocked_task),
        Block::Task(SnapshotTask {
            status: SnapshotTaskStatus::Done,
            content: vec![text("Done task")],
            chips: Vec::new(),
            anchor: None,
            children: Vec::new(),
        }),
        Block::Task(SnapshotTask {
            status: SnapshotTaskStatus::Canceled,
            content: vec![text("Canceled task")],
            chips: Vec::new(),
            anchor: None,
            children: Vec::new(),
        }),
        Block::LogRow {
            label: vec![text("Worked")],
            date: Some(vec![text("Oct 15")]),
            content: vec![text("Shipped a fix")],
            older_count: Some(2),
        },
        Block::Code {
            language: Some("rust".to_string()),
            text: "fn main() {\n    println!(\"literal [[link]]\");\n}\n".to_string(),
        },
        Block::Image {
            asset_id: "cover".to_string(),
            alt: "diagram & notes".to_string(),
            width_percent: Some(62),
        },
        Block::MissingImage {
            label: vec![text("Missing image · img/x.png")],
        },
        Block::Embed {
            source: "Snapshot \"quoted\" & scope › ref".to_string(),
            blocks: vec![Block::Paragraph(vec![text("Embedded prose.")])],
        },
        Block::LiveQuery {
            blocks: vec![Block::Paragraph(vec![Inline::Link {
                label: vec![text("Open live query")],
                target: LinkTarget::Url("https://example.test/query".to_string()),
            }])],
        },
        Block::Callout {
            kind: "note".to_string(),
            title: vec![text("A \"note\" {title}")],
            blocks: vec![Block::Paragraph(vec![text("Callout body.")])],
        },
        Block::Compact {
            blocks: vec![task(
                SnapshotTaskStatus::Done,
                "Compact closed row",
                None,
            )],
        },
        Block::EmptyState(vec![text("No open tasks · 8 closed not shown")]),
    ];

    SnapshotView {
        source: source.clone(),
        masthead: SnapshotMasthead {
            title: "Snapshot: \"contract\" {draft}".to_string(),
            kicker: vec![text("NOTE")],
            display_title: vec![text("Snapshot Contract")],
            dek: Some(vec![text("A \"quoted\" dek: {inputs} & checks.")]),
            meta: vec![
                vec![text("Started Oct 15")],
                vec![Inline::Link {
                    label: vec![text("Requirements")],
                    target: LinkTarget::Internal(requirements.identity.clone()),
                }],
            ],
            stats: vec![
                StatusCount {
                    status: SnapshotTaskStatus::Open,
                    count: 2,
                },
                StatusCount {
                    status: SnapshotTaskStatus::Next,
                    count: 1,
                },
                StatusCount {
                    status: SnapshotTaskStatus::Wip,
                    count: 1,
                },
                StatusCount {
                    status: SnapshotTaskStatus::Blocked,
                    count: 1,
                },
                StatusCount {
                    status: SnapshotTaskStatus::Done,
                    count: 1,
                },
                StatusCount {
                    status: SnapshotTaskStatus::Canceled,
                    count: 1,
                },
            ],
            omitted: Some(vec![text("8 closed not shown")]),
            snapshot_line: vec![text("Snapshot Oct 16 · 8 shown")],
            footer: vec![text("Generated for the contract tests.")],
            glyph_table: "glyphs.lua".to_string(),
            return_links: true,
        },
        sections: vec![SnapshotSection {
            source: source.clone(),
            heading: SnapshotHeading {
                level: HeadingLevel::H1,
                content: vec![text("Requirements")],
                class: HeadingClass::Normal,
                anchor: requirements,
            },
            blocks,
        }],
        closed: Some(SnapshotSection {
            source: source.clone(),
            heading: SnapshotHeading {
                level: HeadingLevel::H1,
                content: vec![text("Closed tasks")],
                class: HeadingClass::Appendix,
                anchor: closed.clone(),
            },
            blocks: vec![Block::Compact {
                blocks: vec![task(
                    SnapshotTaskStatus::Canceled,
                    "Closed appendix row",
                    Some(anchor(&source, "closed-row", "t-closed-row")),
                )],
            }],
        }),
        assets: vec![SnapshotAsset {
            id: "cover".to_string(),
            canonical_source_path: PathBuf::from(
                "/synthetic-vault/img/cover.png",
            ),
            relative_path: PathBuf::from("assets/quay_crane.png"),
            media_kind: AssetMediaKind::Png,
            width_px_hint: Some(400),
            width_percent: Some(62),
        }],
        summary: SnapshotSummary {
            sections: 1,
            task_counts: [
                (SnapshotTaskStatus::Open, 2),
                (SnapshotTaskStatus::Next, 1),
                (SnapshotTaskStatus::Wip, 1),
                (SnapshotTaskStatus::Blocked, 1),
                (SnapshotTaskStatus::Done, 1),
                (SnapshotTaskStatus::Canceled, 1),
            ]
            .into_iter()
            .collect(),
            links: SnapshotLinkCounts {
                resolved: 4,
                unresolved: 1,
            },
            assets: 1,
            missing_images: 1,
            transclusions: 2,
            guarded_omissions: 1,
            omitted_closed_tasks: 8,
            omitted_self_snapshot_tasks: 1,
            omitted_sections: 0,
            schedule_logs: 1,
            work_logs: 1,
        },
        warnings: vec![SnapshotWarning {
            kind: SnapshotWarningKind::UnresolvedLink,
            origin: Some(SourceLocation {
                source: source.clone(),
                line: Some(42),
            }),
            message: "no synthetic note named 'Missing Note'".to_string(),
        }],
        anchors,
    }
}

fn generated_view() -> SnapshotView {
    let source = SourceIdentity {
        canonical_path: PathBuf::from("/synthetic-vault/Generated.md"),
        vault_relative_path: "Generated.md".to_string(),
        route: "Generated".to_string(),
    };
    let heading_anchor = anchor(&source, "generated", "generated");
    SnapshotView {
        source: source.clone(),
        masthead: SnapshotMasthead {
            title: "Generated · Snapshot".to_string(),
            kicker: vec![text("NOTE")],
            display_title: vec![text("Generated Snapshot")],
            dek: None,
            meta: Vec::new(),
            stats: Vec::new(),
            omitted: None,
            snapshot_line: vec![text("Synthetic golden")],
            footer: vec![text("Generated fixture")],
            glyph_table: "glyphs.lua".to_string(),
            return_links: false,
        },
        sections: vec![SnapshotSection {
            source,
            heading: SnapshotHeading {
                level: HeadingLevel::H1,
                content: vec![text("Generated")],
                class: HeadingClass::Normal,
                anchor: heading_anchor.clone(),
            },
            blocks: vec![Block::Paragraph(vec![
                text("Quotes \" braces {x} and Unicode: λ."),
                Inline::Link {
                    label: vec![text("self")],
                    target: LinkTarget::Internal(
                        heading_anchor.identity.clone(),
                    ),
                },
            ])],
        }],
        closed: None,
        assets: Vec::new(),
        summary: SnapshotSummary::default(),
        warnings: Vec::new(),
        anchors: vec![heading_anchor],
    }
}

#[test]
fn serializer_matches_the_hand_authored_complete_contract() {
    let actual =
        serialize_projected_markdown(&contract_view()).expect("serialize view");
    let fixture = fs::read_to_string(
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/note_snapshot/contract.projected.md"),
    )
    .expect("read hand-authored contract fixture");
    assert_eq!(actual, fixture);
}

#[test]
fn generated_view_golden_has_an_explicit_update_switch() {
    let actual = serialize_projected_markdown(&generated_view())
        .expect("serialize view");
    assert_projection_golden("model-generated", &actual);
}

fn assert_projection_golden(name: &str, actual: &str) {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/note_snapshot")
        .join(format!("{name}.projected.md"));
    if std::env::var_os("BOB_UPDATE_GOLDENS").as_deref()
        == Some(std::ffi::OsStr::new("1"))
    {
        fs::write(&path, actual).expect("update generated projection golden");
        return;
    }
    let expected = fs::read_to_string(&path).unwrap_or_else(|error| {
        panic!(
            "read generated projection golden {}: {error}",
            path.display()
        )
    });
    assert_eq!(
        actual,
        expected,
        "generated projection golden: {}",
        path.display()
    );
}

#[test]
fn internal_links_without_a_final_visible_anchor_become_plain_labels() {
    let mut view = contract_view();
    view.anchors.retain(|anchor| anchor.id != "t-next");
    let projected =
        serialize_projected_markdown(&view).expect("serialize view");
    assert!(!projected.contains("[visible link](<#t-next>)"));
    assert!(projected.contains("visible link"));
    assert!(projected.contains("omitted item"));
    assert!(!projected.contains("[omitted item](<#t-omitted>)"));
}

#[test]
fn escaping_caps_image_width_and_keeps_authored_code_fences_intact() {
    let mut view = contract_view();
    view.assets[0].relative_path = PathBuf::from("assets/a b>c.png");
    let image = view.sections[0]
        .blocks
        .iter_mut()
        .find_map(|block| match block {
            Block::Image {
                asset_id,
                width_percent,
                ..
            } if asset_id == "cover" => Some(width_percent),
            _ => None,
        })
        .expect("contract image block");
    *image = Some(140);
    view.sections[0].blocks.push(Block::Code {
        language: None,
        text: "```\ninside\n".to_string(),
    });
    let projected =
        serialize_projected_markdown(&view).expect("serialize view");
    assert!(projected
        .contains("![diagram \\& notes](<assets/a%20b%3Ec.png>){width=100%}"));
    assert!(projected.contains("literal [[link]]"));
    assert!(projected.contains("````\n```\ninside\n````"));
}

#[test]
fn raw_source_metadata_and_task_bodies_survive_in_the_separate_source_record() {
    let raw = b"---\ntype: '[[project]]'\ncreated: 2026-10-15\ncustom: {retain: true}\n---\n- [?] blocked [custom:: value] #task ^task-id\n";
    let raw_line = "- [?] blocked [custom:: value] #task ^task-id";
    let line_start = raw
        .windows(raw_line.len())
        .position(|window| window == raw_line.as_bytes())
        .expect("locate raw synthetic task line");
    let task_line = ParsedTaskLine {
        raw_line: raw_line.to_string(),
        body: "blocked [custom:: value] #task ^task-id".to_string(),
        extent: SourceExtent {
            byte_start: line_start,
            byte_end: line_start + raw_line.len(),
            first_line: 6,
            last_line: 6,
        },
        indentation: SourceIndentation {
            prefix: String::new(),
            columns: 0,
        },
        block_id: Some("task-id".to_string()),
    };
    let frontmatter_yaml =
        "type: '[[project]]'\ncreated: 2026-10-15\ncustom: {retain: true}";
    let frontmatter = SourceFrontmatter {
        extent: SourceExtent {
            byte_start: 0,
            byte_end: frontmatter_yaml.len() + 8,
            first_line: 1,
            last_line: 5,
        },
        raw_yaml: frontmatter_yaml.to_string(),
        metadata: serde_yaml::from_str(frontmatter_yaml)
            .expect("parse synthetic metadata"),
    };
    let parsed = ParsedSource::from_bytes(
        source_identity(),
        raw.to_vec(),
        Some(frontmatter),
        vec![task_line.clone()],
    );

    assert_eq!(parsed.raw_bytes.as_ref(), raw);
    assert_eq!(parsed.sha256, hex::encode(sha2::Sha256::digest(raw)));
    assert_eq!(parsed.task_lines, vec![task_line]);
    assert_eq!(
        parsed.frontmatter.as_ref().unwrap().raw_yaml,
        frontmatter_yaml
    );
    assert_eq!(
        parsed.frontmatter.as_ref().unwrap().metadata["custom"]["retain"],
        serde_yaml::Value::Bool(true)
    );
}

#[test]
fn strike_serialization_never_places_links_inside_strike_nodes() {
    let projected =
        serialize_projected_markdown(&contract_view()).expect("serialize view");
    assert!(projected.contains("~~struck **bold**~~ [visible link](<#t-next>)"));
    assert!(projected.contains("~~and~~ omitted item ~~tail~~"));
    assert!(!projected.contains("~~~~"));
    for struck in projected.split("~~").skip(1).step_by(2) {
        assert!(
            !struck.contains("]("),
            "link remained inside strike: {struck}"
        );
    }
    assert!(projected.contains("- [Waits on]{.label}"));
}

#[test]
fn serialized_internal_links_resolve_only_to_final_visible_anchors() {
    let view = contract_view();
    let projected =
        serialize_projected_markdown(&view).expect("serialize contract");
    assert_serialized_internal_targets_exist(&view, &projected);
    assert!(!projected.contains("](<#t-omitted>)"), "{projected}");
}

#[test]
fn pandoc_parses_the_complete_contract_as_markdown_ast_when_available() {
    let projected = serialize_projected_markdown(&contract_view())
        .expect("serialize complete contract");
    let mut child = match Command::new("pandoc")
        .args(["--from=commonmark_x-tex_math_dollars-emoji", "--to=json"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
    {
        Ok(child) => child,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            eprintln!("skipping snapshot AST test: pandoc is required");
            return;
        }
        Err(error) => panic!("start pandoc AST parser: {error}"),
    };
    child
        .stdin
        .take()
        .expect("pandoc stdin")
        .write_all(projected.as_bytes())
        .expect("write projected Markdown");
    let output = child.wait_with_output().expect("wait for pandoc");
    assert!(
        output.status.success(),
        "pandoc could not parse projected Markdown: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let ast: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("pandoc JSON AST");
    for node in ["Header", "Span", "Div", "Link", "Image", "CodeBlock"] {
        assert!(ast_has_node(&ast, node), "missing Pandoc {node} node");
    }
    assert!(ast_has_node(&ast, "Strikeout"), "strike semantics lost");
    assert!(ast_has_node(&ast, "Strong"), "strong semantics lost");
    assert!(
        ast_tag_contains_tag(&ast, "Strikeout", "Strong"),
        "strong text is no longer struck"
    );
    assert!(
        !ast_tag_contains_tag(&ast, "Strikeout", "Link"),
        "a link remained inside strikeout"
    );
    for struck in ["struck", "bold", "and", "tail"] {
        assert!(
            ast_tag_contains_text(&ast, "Strikeout", struck),
            "lost strike semantics for {struck}"
        );
    }
    for delimiter in ["~~", "**"] {
        assert!(
            !ast_has_str_node_containing(&ast, delimiter),
            "unintended delimiter text survived: {delimiter}"
        );
    }
    assert!(ast_has_class(&ast, "label"), "labeled row lost .label span");
    assert!(
        ast_has_attr_id(&ast, "requirements"),
        "heading anchor missing"
    );
    assert!(ast_has_attr_id(&ast, "t-next"), "task anchor missing");
    assert!(
        ast_contains_text(&ast, "println!"),
        "preserved code missing"
    );
}

/// Structural residue invariant for generated Markdown. It skips protected fenced
/// and inline code, recognizes the generated `:::` fenced-div grammar as unrelated
/// block syntax, and checks only actual bracketed `key:: value` field forms.
pub(crate) fn assert_no_obsidian_residue(projected: &str) {
    let mut fence: Option<(u8, usize)> = None;
    for line in projected.lines() {
        if let Some((delimiter, length)) = fence {
            if is_fence_close(line, delimiter, length) {
                fence = None;
            }
            continue;
        }
        if let Some(opening) = fence_open(line) {
            fence = Some(opening);
            continue;
        }
        let visible = without_inline_code(line);
        assert!(!visible.contains("[["), "wikilink residue: {visible}");
        assert!(!visible.contains("]]"), "wikilink residue: {visible}");
        assert!(
            !visible.contains("%%"),
            "Obsidian comment residue: {visible}"
        );
        assert!(
            !visible.contains("<!--") && !visible.contains("-->"),
            "HTML comment residue: {visible}"
        );
        assert!(
            !has_bracketed_field(&visible),
            "bracketed inline field residue: {visible}"
        );
        assert!(
            crate::native::collect_done::trailing_block_id_in_line(&visible)
                .is_none(),
            "trailing block ID residue: {visible}"
        );
    }
}

pub(crate) fn assert_serialized_internal_targets_exist(
    view: &SnapshotView,
    projected: &str,
) {
    let visible_ids = view
        .anchors
        .iter()
        .map(|anchor| anchor.id.as_str())
        .collect::<std::collections::BTreeSet<_>>();
    let mut rest = projected;
    while let Some(start) = rest.find("](<#") {
        rest = &rest[start + 4..];
        let Some(end) = rest.find(">)") else {
            panic!("malformed serialized internal link near: {rest}");
        };
        let target = &rest[..end];
        assert!(
            visible_ids.contains(target),
            "serialized internal target #{target} has no visible anchor"
        );
        rest = &rest[end + 2..];
    }
}

fn fence_open(line: &str) -> Option<(u8, usize)> {
    let leading = line.bytes().take_while(|byte| *byte == b' ').count();
    if leading > 3 {
        return None;
    }
    let rest = &line.as_bytes()[leading..];
    let delimiter = *rest.first()?;
    if delimiter != b'`' && delimiter != b'~' {
        return None;
    }
    let length = rest.iter().take_while(|byte| **byte == delimiter).count();
    (length >= 3).then_some((delimiter, length))
}

fn is_fence_close(line: &str, delimiter: u8, minimum: usize) -> bool {
    let leading = line.bytes().take_while(|byte| *byte == b' ').count();
    if leading > 3 {
        return false;
    }
    let rest = &line.as_bytes()[leading..];
    let length = rest.iter().take_while(|byte| **byte == delimiter).count();
    length >= minimum && rest[length..].iter().all(u8::is_ascii_whitespace)
}

fn without_inline_code(line: &str) -> String {
    let bytes = line.as_bytes();
    let mut output = String::with_capacity(line.len());
    let mut cursor = 0;
    while cursor < bytes.len() {
        if bytes[cursor] != b'`' {
            let next = line[cursor..]
                .chars()
                .next()
                .expect("valid UTF-8")
                .len_utf8();
            output.push_str(&line[cursor..cursor + next]);
            cursor += next;
            continue;
        }
        let run = bytes[cursor..]
            .iter()
            .take_while(|byte| **byte == b'`')
            .count();
        if let Some(close) = matching_backtick_run(bytes, cursor + run, run) {
            cursor = close + run;
        } else {
            output.push_str(&line[cursor..cursor + run]);
            cursor += run;
        }
    }
    output
}

fn matching_backtick_run(
    bytes: &[u8],
    mut cursor: usize,
    length: usize,
) -> Option<usize> {
    while cursor < bytes.len() {
        let Some(relative) =
            bytes[cursor..].iter().position(|byte| *byte == b'`')
        else {
            return None;
        };
        cursor += relative;
        let run = bytes[cursor..]
            .iter()
            .take_while(|byte| **byte == b'`')
            .count();
        if run == length {
            return Some(cursor);
        }
        cursor += run;
    }
    None
}

fn has_bracketed_field(text: &str) -> bool {
    let bytes = text.as_bytes();
    let mut cursor = 0;
    while cursor < bytes.len() {
        if bytes[cursor] != b'[' {
            cursor += 1;
            continue;
        }
        if bytes.get(cursor + 1) == Some(&b'[') {
            cursor += 2;
            continue;
        }
        let Some(relative_end) =
            bytes[cursor + 1..].iter().position(|byte| *byte == b']')
        else {
            return false;
        };
        let end = cursor + 1 + relative_end;
        if let Some((key, _)) = text[cursor + 1..end].split_once("::") {
            let key = key.trim();
            if !key.is_empty()
                && key.bytes().all(|byte| {
                    byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-')
                })
            {
                return true;
            }
        }
        cursor = end + 1;
    }
    false
}

fn ast_has_node(value: &serde_json::Value, tag: &str) -> bool {
    match value {
        serde_json::Value::Array(values) => {
            values.iter().any(|value| ast_has_node(value, tag))
        }
        serde_json::Value::Object(values) => {
            values.get("t").and_then(serde_json::Value::as_str) == Some(tag)
                || values.values().any(|value| ast_has_node(value, tag))
        }
        _ => false,
    }
}

fn ast_tag_contains_tag(
    value: &serde_json::Value,
    parent: &str,
    child: &str,
) -> bool {
    match value {
        serde_json::Value::Array(values) => values
            .iter()
            .any(|value| ast_tag_contains_tag(value, parent, child)),
        serde_json::Value::Object(values) => {
            let is_parent =
                values.get("t").and_then(serde_json::Value::as_str)
                    == Some(parent);
            (is_parent && ast_has_node(value, child))
                || values
                    .values()
                    .any(|value| ast_tag_contains_tag(value, parent, child))
        }
        _ => false,
    }
}

fn ast_tag_contains_text(
    value: &serde_json::Value,
    tag: &str,
    text: &str,
) -> bool {
    match value {
        serde_json::Value::Array(values) => values
            .iter()
            .any(|value| ast_tag_contains_text(value, tag, text)),
        serde_json::Value::Object(values) => {
            (values.get("t").and_then(serde_json::Value::as_str) == Some(tag)
                && ast_contains_text(value, text))
                || values
                    .values()
                    .any(|value| ast_tag_contains_text(value, tag, text))
        }
        _ => false,
    }
}

fn ast_has_str_node_containing(value: &serde_json::Value, text: &str) -> bool {
    match value {
        serde_json::Value::Array(values) => {
            values
                .iter()
                .any(|value| ast_has_str_node_containing(value, text))
        }
        serde_json::Value::Object(values) => {
            (values.get("t").and_then(serde_json::Value::as_str) == Some("Str")
                && values
                    .get("c")
                    .and_then(serde_json::Value::as_str)
                    .is_some_and(|actual| actual.contains(text)))
                || values
                    .values()
                    .any(|value| ast_has_str_node_containing(value, text))
        }
        _ => false,
    }
}

fn ast_has_class(value: &serde_json::Value, class: &str) -> bool {
    match value {
        serde_json::Value::Array(values) => {
            let has_class = values
                .get(1)
                .and_then(serde_json::Value::as_array)
                .is_some_and(|classes| {
                    classes.iter().any(|value| value.as_str() == Some(class))
                });
            has_class || values.iter().any(|value| ast_has_class(value, class))
        }
        serde_json::Value::Object(values) => {
            values.values().any(|value| ast_has_class(value, class))
        }
        _ => false,
    }
}

fn ast_has_attr_id(value: &serde_json::Value, id: &str) -> bool {
    match value {
        serde_json::Value::Array(values) => {
            let has_attr_shape = values.len() >= 3
                && values.first().and_then(serde_json::Value::as_str)
                    == Some(id)
                && values.get(1).is_some_and(serde_json::Value::is_array)
                && values.get(2).is_some_and(serde_json::Value::is_array);
            has_attr_shape
                || values.iter().any(|value| ast_has_attr_id(value, id))
        }
        serde_json::Value::Object(values) => {
            values.values().any(|value| ast_has_attr_id(value, id))
        }
        _ => false,
    }
}

fn ast_contains_text(value: &serde_json::Value, needle: &str) -> bool {
    match value {
        serde_json::Value::String(text) => text.contains(needle),
        serde_json::Value::Array(values) => {
            values.iter().any(|value| ast_contains_text(value, needle))
        }
        serde_json::Value::Object(values) => values
            .values()
            .any(|value| ast_contains_text(value, needle)),
        _ => false,
    }
}
