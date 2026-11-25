use std::{
    collections::BTreeMap,
    ffi::OsStr,
    fs,
    path::{Path, PathBuf},
};

use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use chrono::NaiveDate;

use super::model::MatchMethod;
use super::*;
use crate::native::{
    env::TestEnvGuard,
    highlights_ref::model::Config,
    meetings::{read_notes, NotesSource},
};

const NOW: &str = "2026-11-24 15:00";

struct Fixture {
    _temp: tempfile::TempDir,
    vault: PathBuf,
    config_path: PathBuf,
    config: Config,
    _env: TestEnvGuard,
}

impl Fixture {
    fn new(config: &str) -> Self {
        let temp = tempfile::tempdir().expect("temporary vault");
        let vault = temp.path().to_path_buf();
        write(&vault.join("config.yml"), config);
        write(&vault.join("work.md"), "---\ntype: [[area]]\n---\n# Work\n");
        write(
            &vault.join("job.md"),
            "---\ntype: [[project]]\nstatus: wip\nparent: [[work]]\n---\n# Job\n## Tasks\n",
        );
        write(
            &vault.join("meet_inbox.md"),
            "---\ntype: [[area]]\nparent: [[work]]\n---\n# Meeting Inbox\n",
        );
        let config_path = vault.join("config.yml");
        let _env = TestEnvGuard::set(&[
            ("BOB_CONFIG_FILE", Some(config_path.as_os_str())),
            ("BOB_NOW", Some(OsStr::new(NOW))),
        ]);
        let config = Config {
            bob_dir: vault.clone(),
            lib_dir: vault.join("lib"),
            ref_dir: vault.join("ref"),
            xlib_dir: vault.join("xlib"),
        };
        Self {
            _temp: temp,
            vault,
            config_path,
            config,
            _env,
        }
    }

    fn event_url(event_id: &str) -> String {
        let payload = URL_SAFE_NO_PAD.encode(format!("{event_id} calendar-id"));
        format!("https://calendar.google.com/event?eid={payload}")
    }

    fn day(&self, rows: &str) {
        write(
            &self.vault.join("2026/20261124.md"),
            &format!("# Day\n## Events\n{rows}\n## Pomodoros\n"),
        );
    }

    fn old_day(&self, rows: &str) {
        write(
            &self.vault.join("2026/20261120.md"),
            &format!("# Day\n## Events\n{rows}\n## Pomodoros\n"),
        );
    }

    fn notes(&self, text: &str) -> crate::native::meetings::NotesInput {
        read_notes(NotesSource::Text(text)).expect("fixture notes read")
    }
}

fn write(path: &Path, text: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("create fixture parent");
    }
    fs::write(path, text).expect("write fixture file");
}

fn cedar_row(root: &str, url: &str) -> String {
    format!(
        "- [*] #task **1400-1430** Cedar Labs quarterly review [📅]({url} \"Open in Google Calendar\") {root} ^cedar-review"
    )
}

fn default_config() -> &'static str {
    "meetings:\n  producer: Config Producer\n  relation_label: notes from\n  status: ready\n  window_days: 7\n"
}

fn explicit_options() -> MeetingPrepareOptions {
    MeetingPrepareOptions {
        selection: MeetingSelection::Token("20261124#^cedar-review".into()),
        ..MeetingPrepareOptions::default()
    }
}

fn snapshot_files(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    fn walk(root: &Path, dir: &Path, files: &mut BTreeMap<PathBuf, Vec<u8>>) {
        let entries = fs::read_dir(dir).expect("read directory");
        for entry in entries {
            let entry = entry.expect("directory entry");
            let path = entry.path();
            if path.is_dir() {
                walk(root, &path, files);
            } else {
                files.insert(
                    path.strip_prefix(root)
                        .expect("relative path")
                        .to_path_buf(),
                    fs::read(&path).expect("read fixture file"),
                );
            }
        }
    }
    let mut files = BTreeMap::new();
    walk(root, root, &mut files);
    files
}

#[test]
fn resolves_explicit_query_url_sharing_variants_auto_and_picked_evidence() {
    let fixture = Fixture::new(default_config());
    let url = Fixture::event_url("cedar-event");
    fixture.day(&cedar_row("[root:: job]", &url));
    let notes = fixture.notes(
        "Cedar Labs quarterly review\n\n## Next Steps\n- Ari: send summary",
    );

    let explicit = prepare_filing_from_source(
        &fixture.config,
        NotesSource::Text(&notes.text),
        &explicit_options(),
    )
    .expect("explicit ref resolves from one source read");
    assert_eq!(explicit.match_evidence.method, MatchMethod::Explicit);
    assert_eq!(explicit.meeting.event_id.as_deref(), Some("cedar-event"));
    assert_eq!(explicit.input_sha256, notes.sha256);

    let query = prepare_filing(
        &fixture.config,
        &notes,
        &MeetingPrepareOptions {
            selection: MeetingSelection::Token("Cedar Labs quarterly".into()),
            ..MeetingPrepareOptions::default()
        },
    )
    .expect("title query resolves");
    assert_eq!(query.match_evidence.method, MatchMethod::Query);

    let url_with_sharing_params = format!("{url}&authuser=0&source=calendar");
    let by_url = prepare_filing(
        &fixture.config,
        &notes,
        &MeetingPrepareOptions {
            selection: MeetingSelection::Token(url_with_sharing_params),
            ..MeetingPrepareOptions::default()
        },
    )
    .expect("decoded eid resolves despite sharing parameters");
    assert_eq!(by_url.meeting.event_id.as_deref(), Some("cedar-event"));
    assert_eq!(by_url.match_evidence.method, MatchMethod::Explicit);

    let automatic = prepare_filing(
        &fixture.config,
        &notes,
        &MeetingPrepareOptions::default(),
    )
    .expect("strong title evidence auto-selects");
    assert_eq!(automatic.match_evidence.method, MatchMethod::Auto);

    let picked_evidence =
        vec!["title words in notes (3/4): cedar, labs, review".into()];
    let picked = prepare_filing(
        &fixture.config,
        &notes,
        &MeetingPrepareOptions {
            selection: MeetingSelection::Picked {
                token: "20261124#^cedar-review".into(),
                evidence: picked_evidence.clone(),
            },
            ..MeetingPrepareOptions::default()
        },
    )
    .expect("picked candidate revalidates");
    assert_eq!(picked.match_evidence.method, MatchMethod::Picked);
    assert_eq!(picked.match_evidence.evidence, picked_evidence);
}

#[test]
fn reports_ambiguity_and_missing_block_id_as_structured_refusals() {
    let fixture = Fixture::new(default_config());
    fixture.day(concat!(
        "- [*] #task **1400-1430** Cedar Labs review [root:: job] ^cedar-one\n",
        "- [ ] #task **1430-1500** Cedar Labs review [root:: job] ^cedar-two\n",
        "- [ ] #task **1500-1530** Cedar Labs review without id [root:: job]",
    ));
    let notes = fixture.notes("Cedar Labs review");

    let ambiguous = prepare_filing(
        &fixture.config,
        &notes,
        &MeetingPrepareOptions {
            selection: MeetingSelection::Token("Cedar Labs review".into()),
            ..MeetingPrepareOptions::default()
        },
    )
    .expect_err("query needs a choice");
    assert_eq!(ambiguous.code, "ambiguous_meeting");
    assert_eq!(ambiguous.exit_code, 2);
    assert_eq!(ambiguous.candidates.len(), 3);
    assert!(error_human(&ambiguous).contains("Cedar Labs review"));
    let json: serde_json::Value =
        serde_json::from_str(&error_json(&ambiguous)).expect("error JSON");
    assert_eq!(json["error"]["code"], "ambiguous_meeting");
    assert_eq!(json["candidates"].as_array().unwrap().len(), 3);

    let missing_id = prepare_filing(
        &fixture.config,
        &notes,
        &MeetingPrepareOptions {
            selection: MeetingSelection::Token(
                "Cedar Labs review without id".into(),
            ),
            ..MeetingPrepareOptions::default()
        },
    )
    .expect_err("a meeting without ^id refuses");
    assert_eq!(missing_id.code, "no_meeting");
    assert!(missing_id
        .hint
        .as_deref()
        .unwrap_or_default()
        .contains("^id"));
}

#[test]
fn day_and_window_bound_list_and_resolution() {
    let fixture = Fixture::new(default_config());
    fixture.day(&cedar_row("[root:: job]", ""));
    fixture.old_day(
        "- [*] #task **1400-1430** Old Cedar Labs quarterly review [root:: job] ^old-review",
    );
    let notes = fixture.notes("Old Cedar Labs quarterly review");

    let too_narrow = prepare_filing(
        &fixture.config,
        &notes,
        &MeetingPrepareOptions {
            selection: MeetingSelection::Token(
                "Old Cedar Labs quarterly".into(),
            ),
            window_days: Some(3),
            ..MeetingPrepareOptions::default()
        },
    )
    .expect_err("old day is outside the window");
    assert_eq!(too_narrow.code, "no_meeting");

    let list = prepare_list(
        &fixture.config,
        None,
        Some(NaiveDate::from_ymd_opt(2026, 11, 20).unwrap()),
    )
    .expect("explicit day listing");
    assert_eq!(list.meetings.len(), 1);
    assert_eq!(list.meetings[0].date, "2026-11-20");
}

#[test]
fn config_precedence_effect_labels_parent_choices_and_root_guards() {
    let fixture = Fixture::new(default_config());
    fixture.day(
        "- [*] #task **1400-1430** Cedar Labs quarterly review ^cedar-review",
    );
    let notes = fixture.notes(
        "Cedar Labs quarterly review\n\n## Next Steps\n- Ari: send report",
    );
    let options = MeetingPrepareOptions {
        parent: Some("job".into()),
        producer: Some("User Producer".into()),
        status: Some("wip".into()),
        window_days: Some(2),
        ..explicit_options()
    };
    let prepared = prepare_filing(&fixture.config, &notes, &options)
        .expect("explicit parent makes the missing-root route ready");
    assert_eq!(
        prepared.effective_config.producer.as_deref(),
        Some("User Producer")
    );
    assert_eq!(prepared.status, "wip");
    assert!(prepared.status_explicit);
    assert_eq!(prepared.effective_config.window_days, 2);
    let root_edit =
        prepared.guarded_root_edit.expect("missing root is guarded");
    assert_eq!(root_edit.to, "job");
    assert_eq!(root_edit.block_id, "cedar-review");
    assert!(!root_edit.expected_file_sha256.is_empty());

    let missing_parent =
        prepare_filing(&fixture.config, &notes, &explicit_options())
            .expect_err("missing event root asks for a parent");
    assert_eq!(missing_parent.code, "needs_parent");
    assert!(missing_parent
        .parent_candidates
        .iter()
        .any(|choice| choice.route == "job"));

    write(
        &fixture.config_path,
        "meetings:\n  relation_label: superseded by\nrelations:\n  cancel_labels: [superseded by]\n",
    );
    let effect_label = prepare_filing(
        &fixture.config,
        &notes,
        &MeetingPrepareOptions {
            parent: Some("job".into()),
            ..explicit_options()
        },
    )
    .expect_err("effect-bearing relation label refuses before writing");
    assert_eq!(effect_label.code, "invalid_config");
}

#[test]
fn ordinary_parent_override_does_not_rewrite_event_root() {
    let fixture = Fixture::new(default_config());
    fixture.day(&cedar_row("[root:: job]", ""));
    let notes = fixture.notes("Cedar Labs quarterly review");
    let prepared = prepare_filing(
        &fixture.config,
        &notes,
        &MeetingPrepareOptions {
            parent: Some("work".into()),
            ..explicit_options()
        },
    )
    .expect("ordinary event root may have a different filing parent");
    assert_eq!(prepared.parent.route, "work");
    assert!(prepared.guarded_root_edit.is_none());
}

#[test]
fn status_defaults_and_overwrite_preserve_task_identity() {
    let fixture = Fixture::new(default_config());
    let url = Fixture::event_url("cedar-event");
    fixture.day(&cedar_row("[root:: job]", &url));
    write(
        &fixture.vault.join("job.md"),
        concat!(
            "---\ntype: [[project]]\nstatus: wip\nparent: [[work]]\n---\n",
            "# Job\n## Tasks\n",
            "- [*] #task #ref Read Cedar meeting notes ^ref-261124-cedar-labs-quarterly-review\n",
        ),
    );
    write(
        &fixture.vault.join("ref/meetings/261124_cedar_labs_quarterly_review.md"),
        concat!(
            "---\ntype: [[ref]]\nstatus: next\nevent_id: cedar-event\nevent: [[20261124#^cedar-review]]\n---\n",
            "# Cedar Meeting\n\n![[job#^ref-261124-cedar-labs-quarterly-review]]\n\n## Highlights\n",
        ),
    );
    let notes = fixture.notes(
        "Cedar Labs quarterly review\n\n## Next Steps\n- Ari: send report",
    );
    let already = prepare_filing(
        &fixture.config,
        &notes,
        &MeetingPrepareOptions {
            selection: MeetingSelection::Token("20261124#^cedar-review".into()),
            ..MeetingPrepareOptions::default()
        },
    )
    .expect_err("filed event refuses without overwrite");
    assert_eq!(already.code, "already_filed");
    assert!(already
        .existing
        .as_ref()
        .unwrap()
        .task
        .ends_with("#^ref-261124-cedar-labs-quarterly-review"));

    let replaced = prepare_filing(
        &fixture.config,
        &notes,
        &MeetingPrepareOptions {
            selection: MeetingSelection::Token("20261124#^cedar-review".into()),
            overwrite: true,
            ..MeetingPrepareOptions::default()
        },
    )
    .expect("overwrite selects the exact filed reference");
    assert_eq!(replaced.stem, "261124_cedar_labs_quarterly_review");
    assert_eq!(
        replaced.task_block_id,
        "ref-261124-cedar-labs-quarterly-review"
    );
    assert_eq!(replaced.parent.route, "job");
    assert_eq!(replaced.status, "next");
    assert!(!replaced.status_explicit);
    assert!(!replaced.title_explicit);
    assert_eq!(replaced.title, "Cedar Meeting");

    let explicit_status = prepare_filing(
        &fixture.config,
        &notes,
        &MeetingPrepareOptions {
            selection: MeetingSelection::Token("20261124#^cedar-review".into()),
            overwrite: true,
            status: Some("read".into()),
            ..MeetingPrepareOptions::default()
        },
    )
    .expect("explicit status overrides the old task status");
    assert_eq!(explicit_status.status, "read");
    assert!(explicit_status.status_explicit);
}

#[test]
fn names_collisions_and_reports_transcript_outline_without_writes() {
    let fixture = Fixture::new(default_config());
    fixture.day(&cedar_row("[root:: job]", ""));
    write(
        &fixture
            .vault
            .join("ref/meetings/261124_cedar_labs_quarterly_review.md"),
        "---\ntype: [[ref]]\n---\n# Unrelated occupant\n",
    );
    let input = fixture.notes(concat!(
        "# Recap title\n\n",
        "The Cedar Labs quarterly review settled the rollout.\n\n",
        "## Next Steps\n- (Ari) send the schedule\n\n",
        "## Risks\n- Delivery may slip\n- Capacity is tight\n\n",
        "## Transcript\nAri: The calendar remains unchanged.\n",
    ));
    let before = snapshot_files(&fixture.vault);
    let editor_marker = fixture.vault.join("editor-was-launched");
    let _editor = TestEnvGuard::set(&[(
        "VISUAL",
        Some(OsStr::new(&format!(
            "sh -c 'touch {}'",
            editor_marker.display()
        ))),
    )]);
    let prepared = prepare_filing(
        &fixture.config,
        &input,
        &MeetingPrepareOptions {
            selection: MeetingSelection::Auto,
            ..MeetingPrepareOptions::default()
        },
    )
    .expect("read-only preparation succeeds");
    assert_eq!(prepared.stem, "261124_cedar_labs_quarterly_review_2");
    assert_eq!(
        prepared.transcript,
        super::model::TranscriptDisposition::Omitted
    );
    assert!(prepared.notes.transcript.is_none());
    assert_eq!(prepared.notes.sections[0].items.len(), 2);
    assert_eq!(prepared.notes.next_steps[0].owner.as_deref(), Some("Ari"));
    assert!(!prepared.notes.tldr.contains("calendar remains unchanged"));
    assert_eq!(snapshot_files(&fixture.vault), before);
    assert!(!fixture.vault.join(".bob/locks").exists());
    assert!(!editor_marker.exists());

    let json: serde_json::Value =
        serde_json::from_str(&prepare_json(&prepared)).expect("JSON report");
    assert_eq!(json["ok"], true);
    assert_eq!(json["schema_version"], 1);
    assert_eq!(json["command"], "ref meet");
    assert_eq!(json["outcome"], "would_file");
    assert_eq!(json["notes"]["transcript"], "omitted");
    assert_eq!(json["notes"]["outline"]["sections"][0]["items"], 2);
    assert_eq!(
        json["filing"]["stem"],
        "261124_cedar_labs_quarterly_review_2"
    );
    assert!(prepare_human(&prepared).contains("Would file meeting notes"));

    let list = prepare_list(&fixture.config, None, None).expect("list report");
    let list_json: serde_json::Value =
        serde_json::from_str(&list_json(&list)).expect("list JSON");
    assert_eq!(list_json["meetings"].as_array().unwrap().len(), 1);
    assert!(list_human(&list).contains("need notes"));
}

#[test]
fn expected_hash_mismatch_fails_before_resolution_or_target_planning() {
    let fixture = Fixture::new(default_config());
    fixture.day(&cedar_row("[root:: job]", ""));
    let notes = fixture.notes("Cedar Labs quarterly review");
    let error = prepare_filing(
        &fixture.config,
        &notes,
        &MeetingPrepareOptions {
            expected_sha256: Some("0".repeat(64)),
            ..explicit_options()
        },
    )
    .expect_err("stale notes hash refuses");
    assert_eq!(error.code, "notes_changed");
}
