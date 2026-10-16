//! Model-contract and serializer tests using synthetic notes only.

use std::{fs, path::PathBuf};

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

fn contract_view() -> SnapshotView {
    let source = source_identity();
    let requirements = anchor(&source, "requirements", "requirements");
    let detail_heading =
        anchor(&source, "heading-level-three", "heading-level-three");
    let next = anchor(&source, "next-task", "t-next");
    let blocked = anchor(&source, "intro-proposal", "t-intro-proposal");
    let closed = anchor(&source, "closed", "closed");
    let omitted = anchor(&source, "omitted-task", "t-omitted");
    let anchors = vec![
        requirements.clone(),
        detail_heading.clone(),
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
        content: vec![text("Next task")],
        chips: Vec::new(),
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
                        label: vec![text("Task One")],
                        target: LinkTarget::Url(
                            "https://example.test/tasks/one".to_string(),
                        ),
                    }],
                    vec![text("Plan")],
                ],
            },
            Block::List {
                kind: ListKind::Unordered,
                items: vec![ListItem {
                    content: vec![text("Prerequisite notes")],
                    children: Vec::new(),
                }],
            },
        ],
    };

    let blocks = vec![
        Block::Paragraph(vec![
            text("Punctuation: [square], _marks_, $69.00, :sparkles:, and @ROUTE; Unicode: café — naïve. "),
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
        Block::Heading(SnapshotHeading {
            level: HeadingLevel::H2,
            content: vec![text("Next & In Progress")],
            class: HeadingClass::Lane,
            anchor: anchor(&source, "next-lane", "next-lane"),
        }),
        Block::Heading(SnapshotHeading {
            level: HeadingLevel::H3,
            content: vec![text("Heading level three")],
            class: HeadingClass::Normal,
            anchor: detail_heading,
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
            relative_path: PathBuf::from("assets/3.png"),
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
    if let Block::Image { width_percent, .. } = &mut view.sections[0].blocks[12]
    {
        *width_percent = Some(140);
    } else {
        panic!("contract image block moved");
    }
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
    assert!(projected.contains("~~struck **bold **~~[visible link](<#t-next>)"));
    assert!(!projected.contains("~~~~"));
    for struck in projected.split("~~").skip(1).step_by(2) {
        assert!(
            !struck.contains("]("),
            "link remained inside strike: {struck}"
        );
    }
}
