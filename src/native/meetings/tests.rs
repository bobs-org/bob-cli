//! Vault-level flows: explicit, URL, and query resolution, the
//! auto-match gate, recent meetings, parents, the filed scan, and agenda
//! fields, plus meeting-note normalization, dossier, and recap goldens.
//! Fixtures use invented people and companies, never real meeting content.

use std::{
    ffi::OsStr,
    fs,
    path::{Path, PathBuf},
};

use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use chrono::{NaiveDate, NaiveDateTime};
use proptest::prelude::*;
use sha2::Digest;

use super::*;
use super::{
    agenda::agenda_meetings,
    matching::{auto_match, AutoMatch},
    model::{
        collect_window_rows, event_ref_for, filed_notes_index, MeetingRow,
    },
    parent::{meeting_parent, MeetingParent},
    resolve::{recent_meetings, resolve_event, ResolveError},
    token::{parse_event_token, EventToken},
};
use crate::native::{
    env::TestEnvGuard,
    gcal::journal::{append_row, DescriptionState, NewRow},
};
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
fn overwrite_recap_reseeds_owned_and_keeps_edited_bytes() {
    let notes = normalize(&fixture("wispr_plain.txt"));
    let recap = render_recap(&notes, &test_recap_meta());
    let skeleton = "# Meeting\n\n![[job#^ref-meeting]]\n\n## Highlights\n";
    let live = replace_recap(skeleton, &recap).expect("seed owned recap");
    let next = render_recap(
        &notes,
        &RecapMeta {
            producer: None,
            ..test_recap_meta()
        },
    );
    let applied =
        apply_overwrite_recap(&live, skeleton, &next).expect("owned reseed");
    assert!(applied.warning.is_none());
    assert_eq!(recap_state(&applied.body), RecapState::Owned);
    assert!(applied.body.contains(&next));

    let absent = apply_overwrite_recap(skeleton, skeleton, &next)
        .expect("absent inserts");
    assert!(absent.warning.is_none());
    assert_eq!(recap_state(&absent.body), RecapState::Owned);
    assert!(absent.body.contains(&next));

    let mut edited_live = live.clone();
    if let Some(inner_start) = edited_live.find(" -->\n") {
        edited_live.insert_str(inner_start + 5, "edited ");
    }
    assert_eq!(recap_state(&edited_live), RecapState::Edited);
    let kept = apply_overwrite_recap(&edited_live, skeleton, &next)
        .expect("edited recap stays");
    assert_eq!(kept.warning.as_deref(), Some(EDITED_RECAP_WARNING));
    assert!(
        kept.body.contains("edited "),
        "edited recap bytes stay on the live note:\n{}",
        kept.body
    );
}

#[test]
fn strip_recap_does_not_count_as_authored_prose_gap() {
    let notes = normalize(&fixture("wispr_plain.txt"));
    let recap = render_recap(&notes, &test_recap_meta());
    let skeleton = "# Meeting\n\n![[job#^ref-meeting]]\n\n## Highlights\n";
    let live = replace_recap(skeleton, &recap).expect("seed");
    let stripped = strip_recap(&live);
    assert!(!stripped.contains("bob:meeting-recap:begin"));
    assert_eq!(stripped, skeleton);
}

#[test]
fn malformed_recap_markers_are_not_deleted_on_overwrite() {
    let skeleton = "# Meeting\n\n![[job#^ref-meeting]]\n\n## Highlights\n";
    let live = "# Meeting\n\n![[job#^ref-meeting]]\n\n<!-- bob:meeting-recap:begin v1 broken -->\nuser prose that must stay\n<!-- bob:meeting-recap:end -->\n\n## Highlights\n";
    assert_eq!(recap_state(live), RecapState::Edited);
    let notes = normalize(&fixture("wispr_plain.txt"));
    let next = render_recap(&notes, &test_recap_meta());
    let kept = apply_overwrite_recap(live, skeleton, &next)
        .expect("malformed recap stays");
    assert_eq!(kept.warning.as_deref(), Some(EDITED_RECAP_WARNING));
    assert!(
        kept.body.contains("user prose that must stay"),
        "malformed recap must not authorize deletion:\n{}",
        kept.body
    );
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
    let bom_crlf = b"\xEF\xBB\xBFSample\r\nSecond \t\r\n";
    let from_bytes =
        read_notes(NotesSource::Bytes(bom_crlf)).expect("bytes source");
    assert_eq!(
        from_bytes.sha256,
        hex::encode(sha2::Sha256::digest(bom_crlf))
    );
    assert_eq!(from_bytes.text, "Sample\nSecond");
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
const PINNED_NOW: &str = "2026-11-23 16:00";

fn guard_now() -> TestEnvGuard {
    TestEnvGuard::set(&[("BOB_NOW", Some(OsStr::new(PINNED_NOW)))])
}

fn pinned_now() -> NaiveDateTime {
    NaiveDateTime::parse_from_str(PINNED_NOW, "%Y-%m-%d %H:%M")
        .expect("pinned now")
}

fn pinned_day() -> NaiveDate {
    NaiveDate::from_ymd_opt(2026, 11, 23).expect("pinned day")
}

fn write_file(path: &Path, contents: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("create parent");
    }
    fs::write(path, contents).expect("write file");
}

/// Calendar URL whose `eid=` decodes to `event_id`.
fn calendar_url(event_id: &str) -> String {
    let payload = URL_SAFE_NO_PAD.encode(format!("{event_id} calendar-id"));
    format!("https://calendar.google.com/event?eid={payload}")
}

fn fixture_day(qbr_url: &str) -> String {
    format!(
        concat!(
            "# Day\n",
            "## Events\n",
            "- [*] #task **1500-1530** Acme Corp QBR [📅]({} \"Open in Google Calendar\") [root:: job] ^acme-qbr\n",
            "- [ ] #task **1600-1630** Acme Corp QBR follow-up [root:: job] ^acme-followup\n",
            "- [-] #task **1000-1030** Cancelled standup [root:: job] ^cancelled-standup\n",
            "- [ ] #task **1400-1430** Ad-hoc sync [root:: job]\n",
            "- [ ] #task **1430-1500** Inbox triage [root:: meet_inbox] ^inbox-triage\n",
            "- [ ] #task **1700-1730** Future planning [root:: job] ^future-planning\n",
            "## Pomodoros\n",
        ),
        qbr_url,
    )
}

fn fixture_vault() -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("vault");
    let vault = dir.path();
    write_file(&vault.join("work.md"), "---\ntype: [[area]]\n---\n# Work\n");
    write_file(
        &vault.join("job.md"),
        concat!(
            "---\ntype: [[project]]\nstatus: wip\nparent: [[work]]\n---\n",
            "# Job\n",
            "## Tasks\n",
            "- [*] #task Read Acme QBR notes ^ref-acme-qbr\n",
            "- [ ] #task Read follow-up notes ^ref-acme-followup\n",
        ),
    );
    write_file(
        &vault.join("inbox.md"),
        "---\ntype: [[area]]\n---\n# Inbox\n",
    );
    write_file(
        &vault.join("meet_inbox.md"),
        "---\ntype: [[area]]\nparent: [[inbox]]\n---\n# Meetings\n",
    );
    let qbr_url = calendar_url("e1-google");
    write_file(&vault.join("2026/20261123.md"), &fixture_day(&qbr_url));
    write_file(
        &vault.join("2026/20261124.md"),
        concat!(
            "# Day\n",
            "## Events\n",
            "- [ ] #task **0900-0930** Acme Corp QBR retrospective [root:: job] ^acme-retro\n",
            "## Pomodoros\n",
        ),
    );
    // Journal identity for the QBR row (path, block id) and a URL-less
    // row, proving the row-key path works without a 📅 badge.
    append_row(
        vault,
        "test-host",
        &NewRow {
            calendar_id: "primary".to_string(),
            event_id: "e1-google".to_string(),
            ical_uid: None,
            series_id: Some("series-9".to_string()),
            day: "2026-11-23".to_string(),
            path: "2026/20261123.md".to_string(),
            block_id: "acme-qbr".to_string(),
            row_kind: "task".to_string(),
            spans: std::collections::BTreeMap::new(),
            root: Some("job".to_string()),
            source: None,
            url: Some(qbr_url),
            description: DescriptionState::Absent,
            note: None,
            unacked: false,
        },
    )
    .expect("journal row");
    append_row(
        vault,
        "test-host",
        &NewRow {
            calendar_id: "primary".to_string(),
            event_id: "e2-google".to_string(),
            ical_uid: None,
            series_id: None,
            day: "2026-11-23".to_string(),
            path: "2026/20261123.md".to_string(),
            block_id: "acme-followup".to_string(),
            row_kind: "task".to_string(),
            spans: std::collections::BTreeMap::new(),
            root: Some("job".to_string()),
            source: None,
            url: None,
            description: DescriptionState::Absent,
            note: None,
            unacked: false,
        },
    )
    .expect("journal row");
    dir
}

fn fixture_notes(vault: &Path) {
    write_file(
        &vault.join("ref/meetings/261123_acme_corp_qbr.md"),
        concat!(
            "---\n",
            "title: \"Acme Corp QBR — Meeting Notes\"\n",
            "event: \"[[2026/20261123#^acme-qbr]]\"\n",
            "event_id: \"e1-google\"\n",
            "---\n",
            "# Acme Corp QBR — Meeting Notes\n",
            "\n",
            "![[job#^ref-acme-qbr]]\n",
            "\n",
            "## Notes\n",
            "\n",
            "- discussed the widget launch\n",
        ),
    );
    write_file(
        &vault.join("ref/meetings/261123_acme_followup.md"),
        concat!(
            "---\n",
            "title: \"Acme Corp QBR follow-up — Meeting Notes\"\n",
            "event: \"[[2026/20261123#^acme-followup]]\"\n",
            "---\n",
            "# Acme Corp QBR follow-up — Meeting Notes\n",
            "\n",
            "![[job#^ref-acme-followup]]\n",
            "\n",
            "## Notes\n",
            "\n",
            "- parking lot items\n",
        ),
    );
}

fn test_row(title: &str) -> MeetingRow {
    let date = pinned_day();
    let starts_at = date.and_hms_opt(12, 0, 0).expect("noon");
    MeetingRow {
        path: Path::new("2026/20261123.md").to_path_buf(),
        line: 3,
        block_id: Some("test-row".to_string()),
        date,
        time_range: "1200-1230".to_string(),
        starts_at,
        ends_at: date.and_hms_opt(12, 30, 0).expect("end"),
        title: title.to_string(),
        status: ' ',
        root: None,
        root_is_inbox: false,
        calendar_url: None,
        event_id: None,
        series_id: None,
        calendar_id: None,
    }
}

#[test]
fn every_explicit_spelling_resolves_to_the_same_row() {
    let dir = fixture_vault();
    let vault = dir.path();
    let mut refs = Vec::new();
    for raw in [
        "20261123#^acme-qbr",
        "2026/20261123#^acme-qbr",
        "[[20261123#^acme-qbr|Acme Corp QBR]]",
        "[[2026/20261123#^acme-qbr]]",
        "20261123:acme-qbr",
        "20261123:^acme-qbr",
        "~20261123#^acme-qbr",
        "~20261123:acme-qbr",
    ] {
        let token = parse_event_token(raw);
        assert!(
            matches!(token, EventToken::Explicit { .. }),
            "{raw} parsed as {token:?}"
        );
        let row = resolve_event(vault, &token, 7, None)
            .unwrap_or_else(|error| panic!("{raw}: {}", error.message()));
        assert_eq!(row.title, "Acme Corp QBR", "{raw}");
        assert_eq!(row.event_id.as_deref(), Some("e1-google"), "{raw}");
        assert_eq!(row.calendar_id.as_deref(), Some("primary"), "{raw}");
        assert_eq!(row.series_id.as_deref(), Some("series-9"), "{raw}");
        assert_eq!(row.root.as_deref(), Some("job"), "{raw}");
        assert!(!row.root_is_inbox, "{raw}");
        refs.push(event_ref_for(&row.path, row.block_id.as_deref().unwrap()));
    }
    assert!(refs
        .iter()
        .all(|event_ref| event_ref.as_deref() == Some("20261123#^acme-qbr")));
}

#[test]
fn note_paths_resolve_stems_and_full_paths() {
    let dir = fixture_vault();
    let vault = dir.path();
    let index = crate::native::meetings::model::vault_index(vault);
    use crate::native::meetings::model::resolve_note_path as resolve_meeting_note;
    assert_eq!(
        resolve_meeting_note(&index, "20261123").expect("stem resolves"),
        Path::new("2026/20261123.md")
    );
    assert_eq!(
        resolve_meeting_note(&index, "2026/20261123").expect("path resolves"),
        Path::new("2026/20261123.md")
    );
    assert!(resolve_meeting_note(&index, "20261199").is_err());
}

#[test]
fn explicit_misses_are_typed() {
    let dir = fixture_vault();
    let vault = dir.path();
    let missing_note = resolve_event(
        vault,
        &parse_event_token(
            "20261123#^acme-qbr"
                .replace("20261123", "20261199")
                .as_str(),
        ),
        7,
        None,
    );
    assert!(matches!(missing_note, Err(ResolveError::NoMeeting { .. })));
    let missing_id =
        resolve_event(vault, &parse_event_token("20261123#^nope"), 7, None);
    assert!(matches!(missing_id, Err(ResolveError::NoMeeting { .. })));
    // A task id outside any daily note is not a meeting.
    let not_daily =
        resolve_event(vault, &parse_event_token("job#^ref-acme-qbr"), 7, None);
    assert!(matches!(not_daily, Err(ResolveError::NoMeeting { .. })));
}

#[test]
fn url_resolves_through_the_journal() {
    let dir = fixture_vault();
    let vault = dir.path();
    let url = calendar_url("e1-google");
    let row = resolve_event(vault, &parse_event_token(&url), 7, None)
        .expect("journal url resolves");
    assert_eq!(row.title, "Acme Corp QBR");
    assert_eq!(row.event_id.as_deref(), Some("e1-google"));
    assert_eq!(row.calendar_id.as_deref(), Some("primary"));
    // An unknown event id is a miss, not a guess.
    let unknown = calendar_url("no-such-event");
    assert!(matches!(
        resolve_event(vault, &parse_event_token(&unknown), 7, None),
        Err(ResolveError::NoMeeting { .. })
    ));
}

#[test]
fn journal_identity_applies_without_a_calendar_badge() {
    let dir = fixture_vault();
    let vault = dir.path();
    let row = resolve_event(
        vault,
        &parse_event_token("20261123#^acme-followup"),
        7,
        None,
    )
    .expect("follow-up resolves");
    assert!(row.calendar_url.is_none());
    assert_eq!(row.event_id.as_deref(), Some("e2-google"));
}

#[test]
fn row_badge_decodes_without_a_journal_entry() {
    let dir = tempfile::tempdir().expect("vault");
    let vault = dir.path();
    let url = calendar_url("badge-only");
    write_file(
        &vault.join("2026/20261123.md"),
        &format!(
            "# Day\n## Events\n- [ ] #task **1500-1530** Badge sync [📅]({url} \"Open\") ^badge-sync\n## Pomodoros\n"
        ),
    );
    let row = resolve_event(
        vault,
        &parse_event_token("20261123#^badge-sync"),
        7,
        None,
    )
    .expect("badge row resolves");
    assert_eq!(row.event_id.as_deref(), Some("badge-only"));
    assert!(row.calendar_id.is_none());
    // The same URL resolves through the badge with no journal at all.
    let via_url = resolve_event(vault, &parse_event_token(&url), 7, None)
        .expect("badge url resolves");
    assert_eq!(via_url.title, "Badge sync");
}

#[test]
fn query_words_match_and_ambiguity_carries_candidates() {
    let _now = guard_now();
    let dir = fixture_vault();
    let vault = dir.path();
    let single = resolve_event(vault, &parse_event_token("follow-up"), 7, None)
        .expect("follow-up query resolves");
    assert_eq!(single.block_id.as_deref(), Some("acme-followup"));
    let ambiguous =
        resolve_event(vault, &parse_event_token("Acme Corp"), 7, None);
    match ambiguous {
        Err(ResolveError::Ambiguous { candidates, .. }) => {
            assert_eq!(candidates.len(), 2);
            assert!(candidates.iter().all(|candidate| candidate
                .meeting
                .title
                .to_ascii_lowercase()
                .contains("acme corp")));
            assert!(candidates
                .iter()
                .all(|candidate| !candidate.detail.is_empty()));
        }
        other => panic!("expected ambiguity, got {other:?}"),
    }
    let missing =
        resolve_event(vault, &parse_event_token("企画 zzz-no-match"), 7, None);
    assert!(matches!(missing, Err(ResolveError::NoMeeting { .. })));
    // A single day scopes the scan to that note.
    let retro = resolve_event(
        vault,
        &parse_event_token("retrospective"),
        7,
        NaiveDate::from_ymd_opt(2026, 11, 24),
    )
    .expect("day-scoped query resolves");
    assert_eq!(retro.block_id.as_deref(), Some("acme-retro"));
}

#[test]
fn rows_without_an_id_refuse_with_a_hint() {
    let _now = guard_now();
    let dir = fixture_vault();
    let vault = dir.path();
    match resolve_event(vault, &parse_event_token("Ad-hoc sync"), 7, None) {
        Err(ResolveError::MissingId { hint, .. }) => {
            assert!(hint.contains("^id"), "{hint}");
        }
        other => panic!("expected MissingId, got {other:?}"),
    }
}

#[test]
fn auto_match_selects_one_strong_candidate_with_evidence() {
    let candidates =
        vec![test_row("Acme Corp QBR"), test_row("Weekly standup")];
    let notes =
        "Notes from the Acme Corp QBR: the widget launch stays on track.";
    match auto_match(notes, &candidates) {
        AutoMatch::Selected {
            row_index,
            score,
            evidence,
        } => {
            assert_eq!(row_index, 0);
            assert!(score >= 0.75, "{score}");
            assert!(evidence.contains("acme"), "{evidence}");
        }
        AutoMatch::Unresolved { .. } => panic!("expected a selection"),
    }
}

#[test]
fn recurring_title_tie_never_auto_selects() {
    let candidates = vec![
        test_row("Acme Corp QBR"),
        MeetingRow {
            title: "Acme Corp QBR".to_string(),
            date: NaiveDate::from_ymd_opt(2026, 11, 24).expect("next day"),
            ..test_row("Acme Corp QBR")
        },
    ];
    let notes = "Notes from the Acme Corp QBR.";
    match auto_match(notes, &candidates) {
        AutoMatch::Unresolved { ranked } => {
            assert_eq!(ranked.len(), 2);
            assert!(ranked.iter().all(|candidate| candidate.score >= 0.75));
            assert!(ranked[0].evidence.contains("title words in notes"));
        }
        selected => panic!("a tie must not select: {selected:?}"),
    }
}

#[test]
fn weak_fields_stay_unresolved() {
    let candidates = vec![test_row("Acme Corp QBR")];
    match auto_match("unrelated notes about lunch", &candidates) {
        AutoMatch::Unresolved { ranked } => {
            assert_eq!(ranked.len(), 1);
            assert!(ranked[0].score < 0.75);
        }
        selected => panic!("weak match must not select: {selected:?}"),
    }
}

#[test]
fn recent_meetings_are_newest_first_with_filed_state() {
    let _now = guard_now();
    let dir = fixture_vault();
    let vault = dir.path();
    fixture_notes(vault);
    let meetings = recent_meetings(vault, 7, pinned_now());
    let titles: Vec<&str> = meetings
        .iter()
        .map(|meeting| meeting.meeting.title.as_str())
        .collect();
    // Newest first; cancelled and unstarted rows never walk.
    assert_eq!(
        titles,
        vec![
            "Acme Corp QBR follow-up",
            "Acme Corp QBR",
            "Inbox triage",
            "Ad-hoc sync"
        ]
    );
    for meeting in &meetings {
        assert!(meeting.meeting.status != '-');
        assert!(meeting.meeting.starts_at <= pinned_now());
        assert!(meeting.meeting.ends_at > meeting.meeting.starts_at);
    }
    let by_title = |title: &str| {
        meetings
            .iter()
            .find(|meeting| meeting.meeting.title == title)
            .unwrap_or_else(|| panic!("missing {title}"))
    };
    // Journal event id keys the QBR filing; the follow-up files by link.
    let qbr = by_title("Acme Corp QBR");
    let qbr_notes = qbr.notes.as_ref().expect("qbr filed");
    assert_eq!(qbr_notes.note, "ref/meetings/261123_acme_corp_qbr.md");
    assert_eq!(qbr_notes.task, "job#^ref-acme-qbr");
    assert_eq!(qbr_notes.status, "*");
    assert_eq!(qbr.event_ref.as_deref(), Some("20261123#^acme-qbr"));
    let followup = by_title("Acme Corp QBR follow-up");
    let followup_notes = followup.notes.as_ref().expect("follow-up filed");
    assert_eq!(followup_notes.task, "job#^ref-acme-followup");
    assert_eq!(
        followup.event_ref.as_deref(),
        Some("20261123#^acme-followup")
    );
    // No notes filed yet for these rows.
    assert!(by_title("Inbox triage").notes.is_none());
    let adhoc = by_title("Ad-hoc sync");
    assert!(adhoc.notes.is_none());
    assert!(adhoc.event_ref.is_none());
}

#[test]
fn filed_index_prefers_neither_key_when_both_are_absent() {
    let dir = fixture_vault();
    // No ref/meetings dir yet: the index is empty, not an error.
    let index = filed_notes_index(dir.path());
    assert!(index.by_event_id.is_empty());
    assert!(index.by_target.is_empty());
}

#[test]
fn meeting_parent_resolves_ready_and_needs() {
    let _now = guard_now();
    let dir = fixture_vault();
    let vault = dir.path();
    let qbr =
        resolve_event(vault, &parse_event_token("20261123#^acme-qbr"), 7, None)
            .expect("qbr resolves");
    match meeting_parent(vault, &qbr, None).expect("parent resolves") {
        MeetingParent::Ready(resolved) => assert_eq!(resolved.route, "job"),
        MeetingParent::NeedsParent { .. } => panic!("job is a usable parent"),
    }
    // An explicit parent wins over the row root.
    match meeting_parent(vault, &qbr, Some("work")).expect("explicit resolves")
    {
        MeetingParent::Ready(resolved) => assert_eq!(resolved.route, "work"),
        MeetingParent::NeedsParent { .. } => panic!("work is a usable parent"),
    }
    assert!(meeting_parent(vault, &qbr, Some("no-such-note")).is_err());
    // The meet_inbox root needs a parent, with the root named.
    let triage = resolve_event(
        vault,
        &parse_event_token("20261123#^inbox-triage"),
        7,
        None,
    )
    .expect("triage resolves");
    assert!(triage.root_is_inbox);
    match meeting_parent(vault, &triage, None).expect("inbox needs") {
        MeetingParent::NeedsParent { root, candidates } => {
            assert_eq!(root.as_deref(), Some("meet_inbox"));
            assert!(!candidates.is_empty());
        }
        MeetingParent::Ready(_) => panic!("meet_inbox must need a parent"),
    }
    // A missing root needs a parent with no root named.
    let mut rootless = qbr.clone();
    rootless.root = None;
    match meeting_parent(vault, &rootless, None).expect("missing needs") {
        MeetingParent::NeedsParent { root, .. } => assert!(root.is_none()),
        MeetingParent::Ready(_) => panic!("a missing root must need a parent"),
    }
}

#[test]
fn collect_window_rows_collects_one_day() {
    let dir = fixture_vault();
    let rows = collect_window_rows(dir.path(), 7, Some(pinned_day()));
    // Six task rows: the cancelled row still parses (filtering is the
    // caller's job), context-free.
    assert_eq!(rows.len(), 6);
    assert!(rows.iter().all(|row| row.date == pinned_day()));
}

#[test]
fn agenda_meetings_cover_started_rows_with_ids() {
    let _now = guard_now();
    let dir = fixture_vault();
    let vault = dir.path();
    fixture_notes(vault);
    let relative = Path::new("2026/20261123.md");
    let contents = fs::read_to_string(vault.join(relative)).expect("day");
    let events =
        crate::native::event_task::events_in_note(&contents, pinned_day());
    let fields = agenda_meetings(
        vault,
        relative,
        &contents,
        &events,
        pinned_day(),
        pinned_now(),
    );
    // Started rows with ids: qbr, follow-up, inbox triage. The ad-hoc
    // row has no id and future planning has not started.
    assert_eq!(fields.len(), 3);
    let qbr_line = events
        .iter()
        .find(|event| event.block_id.as_deref() == Some("acme-qbr"))
        .expect("qbr row")
        .line_index;
    let (event_ref, notes) = fields.get(&qbr_line).expect("qbr fields");
    assert_eq!(event_ref, "20261123#^acme-qbr");
    assert_eq!(
        notes.note.as_deref(),
        Some("ref/meetings/261123_acme_corp_qbr.md")
    );
}
