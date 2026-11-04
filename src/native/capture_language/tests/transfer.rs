//! Transfer lexer table tests: span-aware recognition, grammar vectors,
//! and refusal codes. The scanner stays test-gated until transfer-resolvers
//! wires both consumers.

use super::super::model::RawLine;
use super::super::transfer::*;

fn scan(text: &str) -> TransferScan {
    scan_with_base(text, 0, text, false)
}

fn scan_with_base(
    text: &str,
    base: usize,
    source: &str,
    has_children: bool,
) -> TransferScan {
    let parent = RawLine {
        text,
        start: base,
        end: base + text.len(),
    };
    scan_transfer_line(&parent, source, has_children)
}

fn valid_sources(scan: &TransferScan) -> Vec<(String, String)> {
    match scan {
        TransferScan::Valid { sources, .. } => sources
            .iter()
            .map(|s| (s.route.clone(), s.block_id.clone()))
            .collect(),
        other => panic!("expected Valid, got {other:?}"),
    }
}

#[test]
fn single_source_note_destination_is_valid() {
    let parsed = scan("@inbox+bench > @db");
    let TransferScan::Valid {
        sources,
        arrow,
        destination,
    } = &parsed
    else {
        panic!("expected Valid, got {parsed:?}");
    };
    assert_eq!(sources.len(), 1);
    assert_eq!(sources[0].route, "inbox");
    assert_eq!(sources[0].block_id, "bench");
    assert_eq!((sources[0].start, sources[0].end), (0, 12));
    assert_eq!((arrow.start, arrow.end), (13, 14));
    assert_eq!(destination.kind, TransferDestinationKind::Note);
    assert_eq!(destination.route, "db");
    assert_eq!((destination.start, destination.end), (15, 18));
}

#[test]
fn many_sources_keep_typed_order_and_spans() {
    let text = "@inbox+bench @work+pgdump > @db";
    let parsed = scan(text);
    let TransferScan::Valid {
        sources,
        arrow,
        destination,
    } = &parsed
    else {
        panic!("expected Valid, got {parsed:?}");
    };
    assert_eq!(
        valid_sources(&parsed),
        vec![
            ("inbox".to_string(), "bench".to_string()),
            ("work".to_string(), "pgdump".to_string()),
        ]
    );
    assert_eq!((sources[0].start, sources[0].end), (0, 12));
    assert_eq!((sources[1].start, sources[1].end), (13, 25));
    assert_eq!((arrow.start, arrow.end), (26, 27));
    assert_eq!((destination.start, destination.end), (28, 31));
}

#[test]
fn all_four_destinations_parse() {
    let note = scan("@a+x > @c");
    assert!(matches!(
        note,
        TransferScan::Valid {
            destination: TransferDestination {
                kind: TransferDestinationKind::Note,
                ..
            },
            ..
        }
    ));
    let task = scan("@a+x > @c+tid");
    let TransferScan::Valid { destination, .. } = &task else {
        panic!("expected Valid task, got {task:?}");
    };
    assert_eq!(destination.kind, TransferDestinationKind::Task);
    assert_eq!(destination.block_id.as_deref(), Some("tid"));
    let drawer = scan("@a+x > @c+tid#future-work");
    let TransferScan::Valid { destination, .. } = &drawer else {
        panic!("expected Valid drawer, got {drawer:?}");
    };
    assert_eq!(destination.kind, TransferDestinationKind::Drawer);
    assert_eq!(destination.drawer.as_deref(), Some("future-work"));
    let section = scan("@a+x > @c#future-work");
    let TransferScan::Valid { destination, .. } = &section else {
        panic!("expected Valid section, got {section:?}");
    };
    assert_eq!(destination.kind, TransferDestinationKind::Section);
    assert_eq!(destination.section.as_deref(), Some("future-work"));
}

#[test]
fn quoted_selectors_preserve_intent() {
    let drawer = scan("@a+x > @db+upgrade#\"Future Work\"");
    let TransferScan::Valid { destination, .. } = &drawer else {
        panic!("expected Valid quoted drawer, got {drawer:?}");
    };
    assert_eq!(destination.kind, TransferDestinationKind::Drawer);
    assert!(destination.drawer_exact);
    let section = scan("@mac_inbox+ws-proto > @dev#“Future Work”");
    let TransferScan::Valid { destination, .. } = &section else {
        panic!("expected Valid quoted section, got {section:?}");
    };
    assert_eq!(destination.kind, TransferDestinationKind::Section);
    assert!(destination.section_exact);
}

#[test]
fn partial_right_hand_sides_report_needs() {
    let bare = scan("@a+x >");
    let TransferScan::Incomplete { need, .. } = &bare else {
        panic!("expected Incomplete, got {bare:?}");
    };
    assert_eq!(*need, TransferNeed::Destination);
    for (text, need) in [
        ("@a+x > @", TransferNeed::Destination),
        ("@a+x > @c+", TransferNeed::BlockId),
        ("@a+x > @c+#d", TransferNeed::BlockId),
        ("@a+x > @c+tid#", TransferNeed::Drawer),
        ("@a+x > @c#", TransferNeed::Section),
        ("@a+x > @c+tid#\"Future", TransferNeed::Drawer),
        ("@a+x > @c#\"Future", TransferNeed::Section),
    ] {
        let parsed = scan(text);
        let TransferScan::Incomplete { need: hit_need, .. } = &parsed else {
            panic!("expected Incomplete for {text:?}, got {parsed:?}");
        };
        assert_eq!(*hit_need, need, "{text:?}");
    }
}

#[test]
fn escaped_arrow_keeps_literal_and_drops_backslash() {
    let parsed = scan("@a+x \\> @c");
    let TransferScan::Escaped {
        sources,
        backslash_span,
        arrow_span,
    } = &parsed
    else {
        panic!("expected Escaped, got {parsed:?}");
    };
    assert_eq!(sources.len(), 1);
    assert_eq!(*backslash_span, (5, 6));
    assert_eq!(*arrow_span, (6, 7));
    let elsewhere = scan("called Bob \\> 3 times");
    assert_eq!(elsewhere, TransferScan::Unclaimed);
}

#[test]
fn glued_arrows_need_spaces() {
    for (text, span) in [
        ("@a+x>@c", (4, 5)),
        ("@a+x >@c", (5, 6)),
        ("@a+x> @c", (4, 5)),
    ] {
        let parsed = scan(text);
        let TransferScan::Invalid {
            code, span: hit, ..
        } = &parsed
        else {
            panic!("expected Invalid for {text:?}, got {parsed:?}");
        };
        assert_eq!(*code, "transfer_arrow_spacing", "{text:?}");
        assert_eq!(*hit, span, "{text:?}");
    }
}

#[test]
fn extra_text_after_destination_is_refused() {
    for text in [
        "@a+x > @c extra",
        "@a+x > @c s:2",
        "@a+x > @c $1",
        "@a+x > @c %c",
        "@a+x > @c &note:id",
        "@a+x > @c *",
        "@a+x > @c ~",
        "@a+x > @c #type",
        "@a+x > @db+upgrade extra",
    ] {
        let parsed = scan(text);
        let TransferScan::Invalid { code, .. } = &parsed else {
            panic!("expected Invalid extra_text for {text:?}, got {parsed:?}");
        };
        assert_eq!(*code, "transfer_extra_text", "{text:?}");
    }
}

#[test]
fn children_make_a_transfer_invalid() {
    let parsed = scan_with_base("@a+x > @c", 0, "@a+x > @c", true);
    let TransferScan::Invalid { code, .. } = &parsed else {
        panic!("expected Invalid children, got {parsed:?}");
    };
    assert_eq!(*code, "transfer_children");
}

#[test]
fn source_suffixes_name_the_drawer_or_toggle() {
    for text in ["@a+x#sec > @c", "@a+x! > @c"] {
        let parsed = scan(text);
        let TransferScan::Invalid { code, .. } = &parsed else {
            panic!("expected Invalid suffix for {text:?}, got {parsed:?}");
        };
        assert_eq!(*code, "transfer_source_suffix", "{text:?}");
    }
}

#[test]
fn source_markers_teach_plus_ids() {
    for text in ["@a:x > @c", "@a^x > @c", "^a:x > @c"] {
        let parsed = scan(text);
        let TransferScan::Invalid { code, message, .. } = &parsed else {
            panic!("expected Invalid marker for {text:?}, got {parsed:?}");
        };
        assert_eq!(*code, "transfer_source_marker", "{text:?}");
        assert!(message.contains("@route+id"), "{text:?}: {message}");
    }
}

#[test]
fn bad_destinations_name_the_four_shapes() {
    for text in [
        "@a+x > s:2",
        "@a+x > prose",
        "@a+x > @c+tid!",
        "@a+x > @@c",
        "@a+x > @c:xyz",
    ] {
        let parsed = scan(text);
        let TransferScan::Invalid { code, .. } = &parsed else {
            panic!("expected Invalid destination for {text:?}, got {parsed:?}");
        };
        assert_eq!(*code, "transfer_destination_marker", "{text:?}");
    }
}

#[test]
fn lexical_duplicates_use_parent_identity() {
    let dup = scan("@A+x @a+x > @c");
    let TransferScan::Invalid { code, .. } = &dup else {
        panic!("expected duplicate, got {dup:?}");
    };
    assert_eq!(*code, "duplicate_capture_target");
    let distinct = scan("@a+X @a+x > @c");
    assert!(
        matches!(distinct, TransferScan::Valid { .. }),
        "block IDs stay case-sensitive: {distinct:?}"
    );
}

#[test]
fn destination_is_source_when_it_names_a_source_task() {
    let parsed = scan("@a+x > @a+x");
    let TransferScan::Invalid { code, .. } = &parsed else {
        panic!("expected destination_is_source, got {parsed:?}");
    };
    assert_eq!(*code, "transfer_destination_is_source");
    let note_ok = scan("@a+x > @c");
    assert!(matches!(note_ok, TransferScan::Valid { .. }));
}

#[test]
fn prose_code_and_blockquotes_stay_unclaimed() {
    assert_eq!(scan("called Bob > 3 times"), TransferScan::Unclaimed);
    assert_eq!(scan("> @a+x > @c"), TransferScan::Unclaimed);
    assert_eq!(scan(""), TransferScan::Unclaimed);
    assert_eq!(scan("@@inbox"), TransferScan::Unclaimed);
    let code_draft = "`@a+x > @c`";
    assert_eq!(
        scan_with_base("@a+x > @c", 1, code_draft, false),
        TransferScan::Unclaimed
    );
    let wikilink_draft = "[[@a+x > @c]]";
    assert_eq!(
        scan_with_base("@a+x > @c", 2, wikilink_draft, false),
        TransferScan::Unclaimed
    );
}

#[test]
fn transfer_needs_use_editor_labels() {
    assert_eq!(TransferNeed::Destination.label(), "transfer_destination");
    assert_eq!(TransferNeed::BlockId.label(), "block_id");
    assert_eq!(TransferNeed::Drawer.label(), "drawer");
    assert_eq!(TransferNeed::Section.label(), "section");
}

#[test]
fn utf8_and_crlf_offsets_stay_on_char_boundaries() {
    let draft = "🎉\n@inbox+bench @work+pgdump > @db+upgrade#future-work";
    let line = "@inbox+bench @work+pgdump > @db+upgrade#future-work";
    let base = "🎉\n".len();
    let parsed = scan_with_base(line, base, draft, false);
    let TransferScan::Valid {
        sources,
        arrow,
        destination,
    } = &parsed
    else {
        panic!("expected Valid UTF-8, got {parsed:?}");
    };
    assert_eq!((sources[0].start, sources[0].end), (base, base + 12));
    assert_eq!((arrow.start, arrow.end), (base + 26, base + 27));
    assert_eq!(
        (destination.start, destination.end),
        (base + 28, base + line.len())
    );
    let crlf_draft = "line1\r\n@a+x > @c";
    let crlf_line = "@a+x > @c";
    let crlf_base = "line1\r\n".len();
    let crlf = scan_with_base(crlf_line, crlf_base, crlf_draft, false);
    let TransferScan::Valid {
        sources,
        arrow,
        destination,
    } = &crlf
    else {
        panic!("expected Valid CRLF, got {crlf:?}");
    };
    assert_eq!(
        (sources[0].start, sources[0].end),
        (crlf_base, crlf_base + 4)
    );
    assert_eq!((arrow.start, arrow.end), (crlf_base + 5, crlf_base + 6));
    assert_eq!(
        (destination.start, destination.end),
        (crlf_base + 7, crlf_base + 9)
    );
}
