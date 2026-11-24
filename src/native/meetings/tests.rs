use std::{fs, path::PathBuf};

use proptest::prelude::*;
use sha2::Digest;

use super::*;

fn fixture(name: &str) -> String {
    fs::read_to_string(fixture_path(name)).expect("fixture reads")
}

fn fixture_path(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/meetings")
        .join(name)
}

fn update_or_assert_golden(name: &str, actual: &str) {
    let path = fixture_path(&format!("golden/{name}"));
    if std::env::var_os("BOB_UPDATE_MEETING_GOLDENS").is_some() {
        fs::create_dir_all(path.parent().expect("golden parent exists"))
            .expect("golden directory creates");
        fs::write(&path, actual).expect("golden writes");
    } else {
        let expected = fs::read_to_string(&path).unwrap_or_else(|error| {
            panic!("read golden {}: {error}", path.display())
        });
        assert_eq!(actual, expected, "meeting golden {name} changed");
    }
}

fn test_meta(include_transcript: bool) -> DossierMeta {
    DossierMeta {
        event_title: "Cedar Labs and Driftwood Analytics planning".into(),
        date: "2026-11-24".into(),
        span: "14:00–14:30".into(),
        root: "projects/seaglass".into(),
        producer: Some("VoiceQuill".into()),
        filed_date: "2026-11-24".into(),
        include_transcript,
        edited: false,
    }
}

fn test_recap_meta() -> RecapMeta {
    RecapMeta {
        date: "2026-11-24".into(),
        span: "14:00–14:30".into(),
        event_link:
            "[[20261124#^cedar-driftwood|Cedar Labs and Driftwood Analytics]]"
                .into(),
        producer: Some("VoiceQuill".into()),
    }
}

#[test]
fn synthetic_models_match_goldens() {
    for (input_name, golden_name) in [
        ("wispr_plain.txt", "model_wispr.json"),
        ("markdown_paste.md", "model_markdown.json"),
        ("adversarial.txt", "model_adversarial.json"),
        ("unrecognized.txt", "model_unrecognized.json"),
    ] {
        let model = normalize(&fixture(input_name));
        let pretty =
            format!("{}\n", serde_json::to_string_pretty(&model).unwrap());
        update_or_assert_golden(golden_name, &pretty);
    }
}

#[test]
fn dossier_and_recap_match_goldens() {
    let notes = normalize(&fixture("wispr_plain.txt"));
    update_or_assert_golden(
        "dossier_wispr.md",
        &render_dossier(&notes, &test_meta(false)),
    );
    update_or_assert_golden(
        "recap_wispr.md",
        &render_recap(&notes, &test_recap_meta()),
    );
    let adversarial = normalize(&fixture("adversarial.txt"));
    update_or_assert_golden(
        "recap_adversarial.md",
        &render_recap(&adversarial, &test_recap_meta()),
    );
}

#[test]
fn dossier_frontmatter_and_normalization_round_trip_all_shapes() {
    for (name, include_transcript) in [
        ("wispr_plain.txt", false),
        ("markdown_paste.md", true),
        ("adversarial.txt", false),
        ("unrecognized.txt", false),
    ] {
        let notes = normalize(&fixture(name));
        let meta = test_meta(include_transcript);
        let dossier = render_dossier(&notes, &meta);
        let parsed = parse_dossier(&dossier).expect("rendered dossier parses");
        let expected_notes = if include_transcript {
            notes.clone()
        } else {
            let mut copy = notes.clone();
            copy.transcript = None;
            copy
        };
        assert_eq!(parsed.notes, expected_notes, "{name} notes round trip");
        assert_eq!(parsed.meta, meta, "{name} metadata round trip");
        assert_eq!(
            normalize(&dossier),
            expected_notes,
            "{name} normalize round trip"
        );
        if !include_transcript {
            assert!(!dossier.contains("Morgan: The schedule"));
        }
    }
}

#[test]
fn wispr_shape_preserves_required_evidence_and_other_party_owner() {
    let notes = normalize(&fixture("wispr_plain.txt"));
    assert_eq!(notes.shape, NotesShape::WisprPlainText);
    assert!(notes.tldr.contains("CloudCove connector"));
    assert!(notes
        .sections
        .iter()
        .any(|section| section.items.iter().any(|item| {
            item.contains("18–24") && item.contains("18–42")
        })));
    assert!(notes.sections.iter().any(|section| section
        .items
        .iter()
        .any(|item| { item.contains("sounded like 'Mistral'") })));
    assert_eq!(notes.next_steps[1].owner.as_deref(), Some("Morgan Hale"));
    assert!(notes.next_steps[1].action.contains("Confirm whether"));
}

#[test]
fn recap_escapes_adversarial_tokens_and_keeps_region_digest_owned() {
    let notes = normalize(&fixture("adversarial.txt"));
    let recap = render_recap(&notes, &test_recap_meta());
    assert!(!recap.contains("#task"));
    assert!(!recap.contains("- [ ]"));
    assert!(!recap.contains("[[Link]]"));
    assert!(!recap.contains("key:: value"));
    assert!(!recap.contains("^id"));
    assert!(!recap.contains("%%c%%"));
    assert!(!recap.contains("<b>unsafe</b>"));
    assert_eq!(recap.matches("<!-- bob:meeting-recap:end -->").count(), 1);
    assert_eq!(recap_state(&recap), RecapState::Owned);
    assert_eq!(recap, render_recap(&notes, &test_recap_meta()));

    let edited = recap.replace("Payload", "Changed");
    assert_eq!(recap_state(&edited), RecapState::Edited);
    assert!(replace_recap(&edited, &recap).is_err());
}

#[test]
fn recap_inserts_and_replaces_without_touching_surrounding_note_text() {
    let notes = normalize(&fixture("wispr_plain.txt"));
    let recap = render_recap(&notes, &test_recap_meta());
    let note = "# Meeting\n\n![[job#^ref-meeting]]\n\n## Highlights\n\n<!-- highlights:begin -->\n<!-- highlights:end -->\n";
    let inserted = replace_recap(note, &recap).expect("recap inserts");
    assert_eq!(recap_state(&inserted), RecapState::Owned);
    assert!(inserted
        .contains("![[job#^ref-meeting]]\n\n<!-- bob:meeting-recap:begin"));
    assert!(inserted.contains("\n\n## Highlights"));

    let next_recap = render_recap(
        &notes,
        &RecapMeta {
            producer: None,
            ..test_recap_meta()
        },
    );
    let replaced =
        replace_recap(&inserted, &next_recap).expect("owned recap replaces");
    assert!(replaced.contains("## Highlights"));
    assert_eq!(recap_state(&replaced), RecapState::Owned);
}

#[test]
fn intake_normalizes_bom_crlf_and_trailing_spaces_but_hashes_original_bytes() {
    let raw = fs::read(fixture_path("intake_sample.txt"))
        .expect("fixture bytes read");
    let read =
        read_notes(NotesSource::Path(&fixture_path("intake_sample.txt")))
            .expect("crlf fixture reads");
    assert_eq!(read.sha256, hex::encode(sha2::Sha256::digest(&raw)));
    assert_eq!(read.text, "Sample intake text.\nSecond line.");

    let bom = read_notes(NotesSource::Path(&fixture_path("bom.txt")))
        .expect("BOM fixture reads");
    assert_eq!(bom.text, "BOM sample.");
    assert_eq!(
        read_notes(NotesSource::Text(" \n\t ")).unwrap_err().code(),
        "no_notes"
    );
    assert_eq!(
        read_notes(NotesSource::Path(&fixture_path("empty.txt")))
            .unwrap_err()
            .code(),
        "no_notes"
    );
    assert_eq!(
        read_notes(NotesSource::Text("a\0b")).unwrap_err().code(),
        "notes_unreadable"
    );
    let too_large = "a".repeat(MAX_NOTES_BYTES + 1);
    assert_eq!(
        read_notes(NotesSource::Text(&too_large))
            .unwrap_err()
            .code(),
        "notes_unreadable"
    );
}

proptest! {
    #[test]
    fn plain_word_sequence_survives_dossier_and_full_recap(
        words in prop::collection::vec("[A-Za-z]{1,12}", 1..24)
    ) {
        let source = words.join(" ");
        let notes = normalize(&source);
        let dossier = render_dossier(&notes, &test_meta(false));
        let recap = render_recap(&notes, &test_recap_meta());

        prop_assert_eq!(word_tokens(&source), word_tokens(&visible_dossier_body(&dossier)));
        prop_assert_eq!(word_tokens(&source), word_tokens(&visible_recap_content(&recap)));
        prop_assert_eq!(normalize(&dossier), notes);
    }

    #[test]
    fn wispr_word_sequence_survives_structural_dossier_and_full_recap(
        tldr in prop::collection::vec("[A-Za-z]{1,10}", 1..8),
        detail in prop::collection::vec("[A-Za-z]{1,10}", 1..8),
        owner in "[A-Za-z]{1,10}",
        action in prop::collection::vec("[A-Za-z]{1,10}", 1..8),
    ) {
        let tldr = tldr.join(" ");
        let detail = detail.join(" ");
        let action = action.join(" ");
        let source = format!(
            "{tldr}\n\n\nDetails\n\n{detail}\n\n\nNext steps\n\n({owner}) {action}"
        );
        let notes = normalize(&source);
        let dossier = render_dossier(&notes, &test_meta(false));
        let recap = render_recap(&notes, &test_recap_meta());

        prop_assert_eq!(word_tokens(&source), word_tokens(&visible_dossier_body(&dossier)));
        prop_assert_eq!(word_tokens(&source), word_tokens(&visible_recap_content(&recap)));
        prop_assert_eq!(normalize(&dossier), notes);
    }
}

fn word_tokens(text: &str) -> Vec<String> {
    regex::Regex::new(r"[[:alnum:]]+")
        .unwrap()
        .find_iter(text)
        .map(|found| found.as_str().to_ascii_lowercase())
        .collect()
}

fn visible_dossier_body(dossier: &str) -> String {
    let body = dossier.split_once("\n---\n").unwrap().1;
    body.lines()
        .filter(|line| {
            !line.trim().starts_with(":::") && !line.contains("{.meeting-tldr}")
        })
        .collect::<Vec<_>>()
        .join("\n")
        .replace("- ", " ")
}

fn visible_recap_content(recap: &str) -> String {
    let (_, rest) = recap.split_once(" -->\n").unwrap();
    let (inner, _) = rest
        .rsplit_once("\n<!-- bob:meeting-recap:end -->")
        .unwrap();
    let content = inner.split_once('\n').map_or("", |(_, content)| content);
    decode_entities(content)
        .lines()
        .map(|line| {
            line.trim_start_matches('#').trim_start_matches("- ").trim()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn decode_entities(text: &str) -> String {
    [
        ("&#91;", "["),
        ("&#93;", "]"),
        ("&#35;", "#"),
        ("&#58;", ":"),
        ("&#94;", "^"),
        ("&#37;", "%"),
        ("&#36;", "$"),
        ("&#42;", "*"),
        ("&#95;", "_"),
        ("&#96;", "`"),
        ("&#92;", "\\"),
        ("&#126;", "~"),
        ("&#33;", "!"),
        ("&#45;", "-"),
        ("&#43;", "+"),
        ("&#124;", "|"),
        ("&#61;", "="),
        ("&#123;", "{"),
        ("&#125;", "}"),
        ("&lt;", "<"),
        ("&gt;", ">"),
        ("&amp;", "&"),
    ]
    .into_iter()
    .fold(text.to_string(), |text, (entity, character)| {
        text.replace(entity, character)
    })
}
