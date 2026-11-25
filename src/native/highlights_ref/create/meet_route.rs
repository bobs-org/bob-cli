//! First filing and in-place replacement for `bob ref meet`.
//!
//! Fresh filings ingest a prepared meeting dossier and its event re-root
//! in one locked intake commit. Replacements render and stage the new
//! PDF/source privately, then reuse the overwrite executor so recap
//! ownership, task identity, LINKS, and recovery stay inside that
//! transaction.

use super::super::intake::{
    EventRootEdit, IntakeOutcome, IntakeRequest, MeetingOccurrenceGuard,
};
use super::super::meet::{GuardedRootEdit, PreparedFiling};
use super::super::overwrite::execute::{OverwriteResult, StagedOverwrite};
use super::super::overwrite::OverwriteRequest;
use super::*;

/// Route origin recorded in the write-once source store.
const MEETING_STORE_ROUTE: &str = "meeting_notes";
/// Intake producer for every meeting filing.
const MEETING_PRODUCER: &str = "bob ref meet";

/// Structured result for a first filing, preserving the facts a human
/// receipt and the JSON contract need.
#[derive(Debug)]
pub(in crate::native::highlights_ref) struct MeetFileResult {
    pub(in crate::native::highlights_ref) outcome: IntakeOutcome,
    /// Human warnings: preparation, render, source-store, and skipped
    /// re-root notes.
    pub(in crate::native::highlights_ref) warnings: Vec<String>,
    /// Applied re-root `(from, to)` on successful ingest.
    pub(in crate::native::highlights_ref) reroot_applied:
        Option<(Option<String>, String)>,
    /// True when a prepared re-root was skipped because ingest queued.
    pub(in crate::native::highlights_ref) skipped_reroot: bool,
    /// Installed PDF digest's source pointer, when the store was written.
    pub(in crate::native::highlights_ref) source_pointer: Option<String>,
    /// Rendered PDF page count.
    pub(in crate::native::highlights_ref) pages: usize,
}

/// Structured result for an in-place meeting replacement.
#[derive(Debug)]
pub(in crate::native::highlights_ref) struct MeetReplaceResult {
    pub(in crate::native::highlights_ref) overwrite: OverwriteResult,
    pub(in crate::native::highlights_ref) warnings: Vec<String>,
    pub(in crate::native::highlights_ref) source_pointer: Option<String>,
    pub(in crate::native::highlights_ref) pages: usize,
}

/// Fresh ingest or in-place replacement of a prepared meeting dossier.
#[derive(Debug)]
pub(in crate::native::highlights_ref) enum MeetWriteResult {
    Filed(MeetFileResult),
    Replaced(MeetReplaceResult),
}

/// File one prepared meeting dossier.
///
/// The caller already ran `prepare_filing`; this route never re-resolves
/// the meeting, never runs notes through `prepare_target`, and never
/// prints. It returns the structured intake outcome so the CLI can render
/// human and JSON receipts.
pub(in crate::native::highlights_ref) fn file_meeting(
    config: &Config,
    prepared: &PreparedFiling,
    scratch: &mut super::super::ScratchDir,
) -> Result<MeetFileResult> {
    if let Some(existing) = prepared.existing.as_ref() {
        return Err(CommandError::new(format!(
            "meeting notes are already filed as {}; pass --overwrite to replace them",
            existing.filed.note
        )));
    }
    let mut warnings = prepared.warnings.clone();

    let dossier_meta = dossier_meta_for(prepared);
    let dossier_md =
        crate::native::meetings::render_dossier(&prepared.notes, &dossier_meta);

    let (layout, prose_words) = table_layout_settings()?;
    let filters = write_render_filters(scratch.path(), &layout, prose_words)?;
    let block_id = prepared.meeting.block_id.clone().ok_or_else(|| {
        CommandError::new(
            "selected meeting has no `^id`; add one to file notes",
        )
    })?;
    let event_ref = crate::native::meetings::event_ref_for(
        &prepared.meeting.path,
        &block_id,
    )
    .ok_or_else(|| {
        CommandError::new("selected meeting has no EVENT token; add a `^id`")
    })?;
    let meeting_meta = crate::native::meetings::MeetingMeta {
        title: prepared.title.clone(),
        event_ref: event_ref.clone(),
        accent: None,
    };
    let rendered = crate::native::meetings::render_meeting_pdf(
        &dossier_md,
        scratch.path(),
        &filters,
        &meeting_meta,
    )?;
    for warning in &rendered.warnings {
        warnings.push(warning.message());
    }

    let marker = compose_meeting_marker(prepared, &event_ref)?;
    let info = PdfInfo {
        title: Some(prepared.title.clone()),
        author: prepared.effective_config.producer.clone(),
    };
    let (stamped, page_count) = super::super::pdf_target::stamp_pdf_to_scratch(
        &rendered.pdf,
        scratch,
        &marker,
        &info,
    )?;
    let _installed = super::super::pdf_target::install_stamped_pdf(
        &stamped,
        &prepared.target.target,
        None,
        page_count,
    )?;

    // Persist the exact rendered Markdown and its render-time catalog.
    // Store failures warn but never fail the filing; birth then files an
    // empty recap with a warning.
    let store_warnings = std::cell::RefCell::new(Vec::new());
    let report = |line: &str| {
        store_warnings.borrow_mut().push(line.to_string());
    };
    let source_pointer =
        super::super::source_store::write_for_create_reporting(
            config,
            &prepared.target,
            &prepared.target.target,
            dossier_md.as_bytes(),
            MEETING_STORE_ROUTE,
            event_ref.clone(),
            None,
            None,
            &rendered.code_catalog,
            &report,
        )
        .map(|outcome| outcome.pointer);
    warnings.extend(store_warnings.into_inner());

    let unit = match &prepared.target.workflow {
        TargetWorkflow::Intake {
            library_destination,
        } => super::super::intake::IntakeUnit::Pdf {
            source: prepared.target.target.clone(),
            destination: library_destination.clone(),
        },
        TargetWorkflow::Library => {
            super::super::intake::IntakeUnit::LibraryPdf {
                pdf: prepared.target.target.clone(),
            }
        }
        TargetWorkflow::External => {
            return Err(CommandError::new(
                "meeting target is outside the library; refusing to file",
            ));
        }
    };
    let root_edit = prepared
        .guarded_root_edit
        .as_ref()
        .map(|edit| root_edit_for(prepared, edit));
    let occurrence_guard = prepared.meeting.block_id.as_ref().map(|block_id| {
        MeetingOccurrenceGuard {
            event_id: prepared.meeting.event_id.clone(),
            path: prepared.meeting.path.clone(),
            block_id: block_id.clone(),
        }
    });
    let progress_warnings = std::cell::RefCell::new(Vec::new());
    let progress = |line: &str| {
        progress_warnings.borrow_mut().push(line.to_string());
    };
    let outcome = super::super::intake::finish_intake(
        config,
        unit,
        IntakeRequest {
            producer: MEETING_PRODUCER,
            keep_reason: None,
            expected_source_pointer: source_pointer.as_deref(),
            dry_run: false,
            progress: Some(&progress),
            event_root_edit: root_edit.clone(),
            occurrence_guard,
        },
    );
    warnings.extend(progress_warnings.into_inner());
    let (reroot_applied, skipped_reroot) = match &outcome {
        IntakeOutcome::Ingested { .. } => (
            root_edit
                .as_ref()
                .map(|edit| (edit.from.clone(), edit.to.clone())),
            false,
        ),
        IntakeOutcome::Queued { .. } => {
            if root_edit.is_some() {
                warnings.push(
                    "skipped the event re-root because intake queued; `bob ref scan` will file the notes without re-rooting"
                        .to_string(),
                );
            }
            (None, root_edit.is_some())
        }
        _ => (None, false),
    };
    Ok(MeetFileResult {
        outcome,
        warnings,
        reroot_applied,
        skipped_reroot,
        source_pointer,
        pages: page_count,
    })
}

/// File a new meeting or replace the selected occurrence.
///
/// `--overwrite` with no existing occurrence follows ordinary create-force
/// semantics and files a fresh reference instead of targeting an unrelated
/// note. Dedupe without overwrite is refused by preparation before this
/// route renders.
pub(in crate::native::highlights_ref) fn file_or_replace_meeting(
    config: &Config,
    prepared: &PreparedFiling,
    scratch: &mut super::super::ScratchDir,
) -> Result<MeetWriteResult> {
    if prepared.existing.is_some() {
        Ok(MeetWriteResult::Replaced(replace_meeting(
            config, prepared, scratch,
        )?))
    } else {
        Ok(MeetWriteResult::Filed(file_meeting(
            config, prepared, scratch,
        )?))
    }
}

/// Replace a filed meeting's notes in place, preserving identity, LINKS,
/// and recap ownership inside the overwrite transaction.
pub(in crate::native::highlights_ref) fn replace_meeting(
    config: &Config,
    prepared: &PreparedFiling,
    scratch: &mut super::super::ScratchDir,
) -> Result<MeetReplaceResult> {
    let existing = prepared.existing.as_ref().ok_or_else(|| {
        CommandError::new(
            "overwrite requires a filed meeting occurrence; file it first",
        )
    })?;
    let mut warnings = prepared.warnings.clone();
    let dossier_meta = dossier_meta_for(prepared);
    let dossier_md =
        crate::native::meetings::render_dossier(&prepared.notes, &dossier_meta);
    let recap = crate::native::meetings::render_birth_recap(
        &prepared.notes,
        &recap_meta_for(prepared),
    );
    let member = existing
        .overwrite
        .selection
        .family
        .selected_member()
        .clone();
    let date = super::super::note::capture_clock().date;
    let request = OverwriteRequest {
        explicit_status: prepared
            .status_explicit
            .then(|| prepared.status.clone()),
        explicit_title: prepared.title_explicit.then(|| prepared.title.clone()),
        resolved_parent: prepared
            .parent
            .explicit
            .then(|| prepared.parent.route.clone()),
        producer: Some(MEETING_PRODUCER.to_string()),
        meeting_recap: Some(recap),
        planned_source_store: Some(
            super::super::source_store::PlannedSourceStore::for_bytes(
                "meetings",
                &prepared.stem,
                dossier_md.as_bytes(),
            ),
        ),
        ..OverwriteRequest::default()
    };
    let plan =
        super::super::overwrite::prepare(config, &member, &request, &date)
            .map_err(|refusal| refusal.command_error())?;

    let (layout, prose_words) = table_layout_settings()?;
    let filters = write_render_filters(scratch.path(), &layout, prose_words)?;
    let block_id = prepared.meeting.block_id.clone().ok_or_else(|| {
        CommandError::new(
            "selected meeting has no `^id`; add one to file notes",
        )
    })?;
    let event_ref = crate::native::meetings::event_ref_for(
        &prepared.meeting.path,
        &block_id,
    )
    .ok_or_else(|| {
        CommandError::new("selected meeting has no EVENT token; add a `^id`")
    })?;
    let meeting_meta = crate::native::meetings::MeetingMeta {
        title: prepared.title.clone(),
        event_ref: event_ref.clone(),
        accent: None,
    };
    let rendered = crate::native::meetings::render_meeting_pdf(
        &dossier_md,
        scratch.path(),
        &filters,
        &meeting_meta,
    )?;
    for warning in &rendered.warnings {
        warnings.push(warning.message());
    }

    let marker = compose_meeting_marker(prepared, &event_ref)?;
    let info = PdfInfo {
        title: Some(prepared.title.clone()),
        author: prepared.effective_config.producer.clone(),
    };
    let (stamped, page_count) = super::super::pdf_target::stamp_pdf_to_scratch(
        &rendered.pdf,
        scratch,
        &marker,
        &info,
    )?;
    let source_store = super::stage_overwrite_source_store(
        config,
        &plan,
        scratch.path(),
        dossier_md.as_bytes(),
        MEETING_STORE_ROUTE,
        event_ref,
        None,
        None,
        &stamped,
        &rendered.code_catalog,
    )?;
    let source_pointer = Some(source_store.planned.pointer.clone());
    let marker = super::super::overwrite::local::marker_from_pdf(&stamped)?;
    let overwrite = super::super::overwrite::execute::execute(
        config,
        &plan,
        &request,
        StagedOverwrite {
            private_pdf: stamped,
            audio: None,
            marker,
            source_store: Some(source_store),
        },
    )?;
    warnings.extend(overwrite.warnings.iter().cloned());
    Ok(MeetReplaceResult {
        overwrite,
        warnings,
        source_pointer,
        pages: page_count,
    })
}

fn recap_meta_for(
    prepared: &PreparedFiling,
) -> crate::native::meetings::RecapMeta {
    crate::native::meetings::RecapMeta {
        date: prepared.meeting.date.format("%Y-%m-%d").to_string(),
        span: format!(
            "{}–{}",
            prepared.meeting.starts_at.format("%H:%M"),
            prepared.meeting.ends_at.format("%H:%M")
        ),
        event_link: prepared.relation_target.clone(),
        producer: prepared.effective_config.producer.clone(),
    }
}

fn dossier_meta_for(
    prepared: &PreparedFiling,
) -> crate::native::meetings::DossierMeta {
    let date = prepared.meeting.date.format("%Y-%m-%d").to_string();
    let span = format!(
        "{}–{}",
        prepared.meeting.starts_at.format("%H:%M"),
        prepared.meeting.ends_at.format("%H:%M")
    );
    let filed_date = crate::native::env::current_datetime()
        .date()
        .format("%Y-%m-%d")
        .to_string();
    crate::native::meetings::DossierMeta {
        event_title: prepared.meeting.title.clone(),
        date,
        span,
        root: prepared.parent.route.clone(),
        producer: prepared.effective_config.producer.clone(),
        filed_date,
        include_transcript: prepared.include_transcript,
        edited: prepared.edited,
    }
}

fn compose_meeting_marker(
    prepared: &PreparedFiling,
    _event_ref: &str,
) -> Result<String> {
    let mut extras: Vec<(&str, MarkerValue)> = Vec::new();
    if let Some(url) = prepared.meeting.calendar_url.as_deref() {
        extras.push(("source_url", MarkerValue::String(url.to_string())));
    }
    extras.push((
        "event",
        MarkerValue::String(prepared.relation_target.clone()),
    ));
    if let Some(event_id) = prepared.meeting.event_id.as_deref() {
        extras.push(("event_id", MarkerValue::String(event_id.to_string())));
    }
    extras.push((
        "meeting_start",
        MarkerValue::String(
            prepared
                .meeting
                .starts_at
                .format("%Y-%m-%dT%H:%M")
                .to_string(),
        ),
    ));
    extras.push((
        "meeting_end",
        MarkerValue::String(
            prepared
                .meeting
                .ends_at
                .format("%Y-%m-%dT%H:%M")
                .to_string(),
        ),
    ));
    extras.push((
        "captured",
        MarkerValue::String(super::super::note::capture_clock().stamp),
    ));
    if let Some(producer) = prepared.effective_config.producer.as_deref() {
        extras.push((
            "notes_producer",
            MarkerValue::String(producer.to_string()),
        ));
    }
    compose_marker(
        &prepared.status,
        &prepared.parent.route,
        &prepared.title,
        Some(&prepared.stem),
        &extras,
    )
}

fn root_edit_for(
    prepared: &PreparedFiling,
    edit: &GuardedRootEdit,
) -> EventRootEdit {
    EventRootEdit {
        path: edit.path.clone(),
        line: edit.line,
        block_id: edit.block_id.clone(),
        event_id: edit.event_id.clone(),
        from: edit.from.clone(),
        // Preparation only creates this edit for a missing or inbox root;
        // missing counts as inbox so the locked recheck keeps the same
        // allowance.
        from_is_inbox: prepared.meeting.root.is_none()
            || prepared.meeting.root_is_inbox,
        to: edit.to.clone(),
        expected_line: edit.expected_line.clone(),
        expected_file_sha256: edit.expected_file_sha256.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{ffi::OsStr, fs, path::PathBuf, process::Command};

    use crate::native::{
        env::TestEnvGuard,
        meetings::{
            read_notes, recap_state, NotesSource, RecapState,
            EDITED_RECAP_WARNING,
        },
    };

    use super::super::super::meet::{
        prepare_filing, MeetingPrepareOptions, MeetingSelection,
    };

    const NOW: &str = "2026-11-24 15:00";

    struct RouteFixture {
        _temp: tempfile::TempDir,
        vault: PathBuf,
        config: Config,
        _env: TestEnvGuard,
    }

    impl RouteFixture {
        fn new(config_text: &str) -> Self {
            let temp = tempfile::tempdir().expect("temp vault");
            let vault = temp.path().join("vault");
            std::fs::create_dir_all(&vault).expect("vault dir");
            write_file(&vault.join("config.yml"), config_text);
            write_file(
                &vault.join("work.md"),
                "---\ntype: [[area]]\n---\n# Work\n",
            );
            write_file(
                &vault.join("job.md"),
                "---\ntype: [[project]]\nstatus: wip\nparent: [[work]]\n---\n# Job\n## Tasks\n",
            );
            write_file(
                &vault.join("meet_inbox.md"),
                "---\ntype: [[area]]\nparent: [[work]]\n---\n# Meeting Inbox\n",
            );
            // A vault-sync pair: bare remote + worktree, so the locked
            // intake can pre-sync and commit.
            let remote = temp.path().join("remote.git");
            git(&vault, &["init", "-q"]);
            git(&vault, &["config", "user.name", "Test User"]);
            git(&vault, &["config", "user.email", "test@example.com"]);
            git(&vault, &["config", "commit.gpgsign", "false"]);
            Command::new("git")
                .args(["init", "-q", "--bare"])
                .arg(&remote)
                .current_dir(temp.path())
                .output()
                .expect("init bare remote");
            git(
                &vault,
                &["remote", "add", "origin", &remote.to_string_lossy()],
            );
            let config_path = vault.join("config.yml");
            let sync_lock = temp.path().join("bob_sync.lock");
            let sync_state = temp.path().join("vault-sync.json");
            let state_home = temp.path().join("state");
            fs::create_dir_all(&state_home).expect("state home");
            let _env = TestEnvGuard::set(&[
                ("BOB_DIR", Some(vault.as_os_str())),
                ("BOB_CONFIG_FILE", Some(config_path.as_os_str())),
                ("BOB_NOW", Some(OsStr::new(NOW))),
                ("BOB_REF_INTAKE_LOCK_WAIT_SECONDS", Some(OsStr::new("30"))),
                ("BOB_REF_SCAN_LOCK_WAIT_SECONDS", Some(OsStr::new("30"))),
                ("BOB_VAULT_SYNC_LOCK_FILE", Some(sync_lock.as_os_str())),
                ("BOB_VAULT_SYNC_STATE_FILE", Some(sync_state.as_os_str())),
                ("XDG_STATE_HOME", Some(state_home.as_os_str())),
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
                config,
                _env,
            }
        }

        fn commit_seed(&self, _files: &[&str], message: &str) {
            let output = Command::new("git")
                .args(["add", "."])
                .current_dir(&self.vault)
                .output()
                .expect("git add");
            assert!(output.status.success(), "git add failed");
            let output = Command::new("git")
                .args(["commit", "-q", "-m", message])
                .current_dir(&self.vault)
                .output()
                .expect("git commit");
            assert!(output.status.success(), "git commit failed: {output:?}");
            let output = Command::new("git")
                .args(["push", "-q", "-u", "origin", "HEAD:master"])
                .current_dir(&self.vault)
                .output()
                .expect("git push");
            // A fresh `git init` defaults to master or main; push the
            // current HEAD explicitly so the test does not depend on it.
            assert!(output.status.success(), "git push failed: {output:?}");
        }

        fn day(&self, rows: &str) {
            write_file(
                &self.vault.join("2026/20261124.md"),
                &format!("# Day\n## Events\n{rows}\n## Pomodoros\n"),
            );
        }

        fn notes_input(
            &self,
            text: &str,
        ) -> crate::native::meetings::NotesInput {
            read_notes(NotesSource::Text(text)).expect("notes read")
        }

        fn prepare(
            &self,
            notes_text: &str,
            options: MeetingPrepareOptions,
        ) -> super::super::super::meet::PreparedFiling {
            let input = self.notes_input(notes_text);
            prepare_filing(&self.config, &input, &options)
                .expect("prepare filing")
        }

        fn file(
            &self,
            prepared: &super::super::super::meet::PreparedFiling,
        ) -> MeetFileResult {
            let mut scratch =
                super::super::super::ScratchDir::create("meet-test")
                    .expect("scratch");
            file_meeting(&self.config, prepared, &mut scratch)
                .expect("file meeting")
        }

        fn write(
            &self,
            prepared: &super::super::super::meet::PreparedFiling,
        ) -> MeetWriteResult {
            let mut scratch =
                super::super::super::ScratchDir::create("meet-test")
                    .expect("scratch");
            file_or_replace_meeting(&self.config, prepared, &mut scratch)
                .expect("write meeting")
        }

        fn replace(
            &self,
            prepared: &super::super::super::meet::PreparedFiling,
        ) -> MeetReplaceResult {
            match self.write(prepared) {
                MeetWriteResult::Replaced(result) => result,
                other => panic!("expected replacement, got {other:?}"),
            }
        }
    }

    fn git(vault: &std::path::Path, args: &[&str]) {
        let output = Command::new("git")
            .args(args)
            .current_dir(vault)
            .output()
            .expect("run git");
        assert!(output.status.success(), "git {args:?} failed: {output:?}");
    }

    fn write_file(path: &std::path::Path, contents: &str) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("create parent");
        }
        fs::write(path, contents).expect("write file");
    }

    fn default_config() -> &'static str {
        "meetings:\n  producer: VoiceQuill\n  relation_label: notes from\n  status: next\n  window_days: 7\n"
    }

    fn cedar_row(root: &str, url: &str) -> String {
        format!(
            "- [*] #task **1400-1430** Cedar Labs quarterly review [📅]({url} \"Open in Google Calendar\") {root} ^cedar-review"
        )
    }

    fn commit_subjects(vault: &std::path::Path) -> Vec<String> {
        let output = Command::new("git")
            .args(["log", "--format=%s"])
            .current_dir(vault)
            .output()
            .expect("git log");
        assert!(output.status.success());
        String::from_utf8_lossy(&output.stdout)
            .lines()
            .map(str::to_string)
            .collect()
    }

    #[test]
    fn fresh_filing_files_note_task_pdf_source_and_single_commit() {
        let fixture = RouteFixture::new(default_config());
        let url = "https://calendar.google.com/event?eid=cedar-event";
        fixture.day(&cedar_row("[root:: job]", url));
        fixture.commit_seed(&[], "seed");
        let notes_text =
            "Cedar Labs quarterly review\n\n## Next Steps\n- (Ari) Ship it";
        let prepared = fixture.prepare(
            notes_text,
            MeetingPrepareOptions {
                selection: MeetingSelection::Token(
                    "20261124#^cedar-review".to_string(),
                ),
                ..MeetingPrepareOptions::default()
            },
        );
        assert!(prepared.guarded_root_edit.is_none(), "ordinary root keeps");
        let result = fixture.file(&prepared);
        match &result.outcome {
            IntakeOutcome::Ingested {
                commit_sha,
                commit_warning,
                sync_status,
                ..
            } => {
                assert!(commit_sha.is_some(), "one commit: {result:?}");
                assert!(commit_warning.is_none(), "no commit warning");
                assert!(!sync_status.is_empty(), "sync status kept");
            }
            other => panic!("fresh filing must ingest, got {other:?}"),
        }
        // One ingest commit with the required subject.
        let subjects = commit_subjects(&fixture.vault);
        assert!(
            subjects.iter().any(|subject| subject
                == &format!("bob ref meet: ingest meetings/{}", prepared.stem)),
            "scoped ingest subject: {subjects:?}"
        );
        // Note, task, PDF, and write-once source all exist.
        assert!(prepared.note_path.is_file(), "reference note filed");
        let lib_pdf = fixture
            .vault
            .join(format!("lib/meetings/{}.pdf", prepared.stem));
        assert!(lib_pdf.is_file(), "library PDF filed");
        let pointer = result.source_pointer.expect("source pointer");
        let stored = crate::native::highlights_ref::source_store::read_store(
            &fixture.vault,
            &pointer,
        );
        assert!(stored.bytes.is_some(), "exact rendered source preserved");
        // Marker carries the meeting identity.
        let marker =
            crate::native::highlights_ref::marker::read_pdf_marker(&lib_pdf)
                .expect("read marker");
        let projection = crate::native::highlights_ref::marker::parse_marker(
            &marker.contents,
        )
        .expect("parse marker");
        assert_eq!(
            projection
                .get("event")
                .and_then(crate::native::highlights_ref::model::MarkerValue::as_string)
                .map(str::to_string)
                .as_deref(),
            Some(prepared.relation_target.as_str()),
            "canonical event link stamped"
        );
        // Reading task and recap exist.
        let body = fs::read_to_string(&prepared.note_path).expect("read note");
        assert!(body.contains("<!-- bob:meeting-recap:begin v1 sha256:"));
        assert!(body.contains("Ship it"));
        // No snapshot routing: the xlib source is gone, lib holds the PDF.
        assert!(
            !fixture
                .vault
                .join(format!("xlib/meetings/{}.pdf", prepared.stem))
                .exists(),
            "queued source drained"
        );
        assert!(result.reroot_applied.is_none());
        assert!(!result.skipped_reroot);
        assert!(result.pages > 0, "rendered pages");
        assert!(
            result.warnings.iter().all(|warning| !warning.is_empty()),
            "warnings kept"
        );
    }

    #[test]
    fn inbox_reroot_edits_daily_note_without_touching_badges() {
        let fixture = RouteFixture::new(default_config());
        // Missing root plus a buffer and clock tag: the re-root must keep
        // the Google-owned badge span byte-for-byte except `[root::]`.
        let before_row = "- [*] #task **1400-1430** Cedar Labs quarterly review [📅](https://calendar.google.com/event?eid=x \"Open\") #on [buffer:: 15m] ^cedar-review";
        fixture.day(before_row);
        fixture.commit_seed(&[], "seed");
        let before_bytes =
            fs::read(fixture.vault.join("2026/20261124.md")).expect("day");
        let prepared = fixture.prepare(
            "Cedar Labs quarterly review\n\n## Next Steps\n- (Ari) Ship it",
            MeetingPrepareOptions {
                selection: MeetingSelection::Token(
                    "20261124#^cedar-review".to_string(),
                ),
                parent: Some("job".to_string()),
                ..MeetingPrepareOptions::default()
            },
        );
        let edit = prepared
            .guarded_root_edit
            .as_ref()
            .expect("missing root prepares an edit");
        assert_eq!(edit.to, "job");
        let result = fixture.file(&prepared);
        assert!(
            matches!(&result.outcome, IntakeOutcome::Ingested { .. }),
            "reroot files: {:?}",
            result.outcome
        );
        assert_eq!(
            result.reroot_applied,
            Some((None, "job".to_string())),
            "reroot reported"
        );
        let after = fs::read_to_string(fixture.vault.join("2026/20261124.md"))
            .expect("day after");
        assert!(after.contains("[root:: job]"), "new root written");
        assert!(after.contains("[📅]("), "calendar badge kept");
        assert!(after.contains("#on"), "clock tag kept");
        assert!(after.contains("[buffer:: 15m]"), "buffer kept");
        assert_ne!(
            before_bytes,
            fs::read(fixture.vault.join("2026/20261124.md")).expect("bytes")
        );
    }

    #[test]
    fn ordinary_root_override_files_without_reroot() {
        let fixture = RouteFixture::new(default_config());
        fixture.day(&cedar_row("[root:: job]", "https://example.com/e"));
        fixture.commit_seed(&[], "seed");
        let before =
            fs::read(fixture.vault.join("2026/20261124.md")).expect("day");
        let prepared = fixture.prepare(
            "Cedar Labs quarterly review\n\n- note",
            MeetingPrepareOptions {
                selection: MeetingSelection::Token(
                    "20261124#^cedar-review".to_string(),
                ),
                parent: Some("work".to_string()),
                ..MeetingPrepareOptions::default()
            },
        );
        assert!(
            prepared.guarded_root_edit.is_none(),
            "ordinary root never re-roots"
        );
        let result = fixture.file(&prepared);
        assert!(matches!(&result.outcome, IntakeOutcome::Ingested { .. }));
        let after =
            fs::read(fixture.vault.join("2026/20261124.md")).expect("day");
        assert_eq!(before, after, "daily note untouched");
        assert!(result.reroot_applied.is_none());
    }

    #[test]
    fn stale_event_queues_without_stray_root_edit() {
        let fixture = RouteFixture::new(default_config());
        fixture.day("- [*] #task **1400-1430** Cedar Labs quarterly review ^cedar-review");
        fixture.commit_seed(&[], "seed");
        let prepared = fixture.prepare(
            "Cedar Labs quarterly review\n\n- note",
            MeetingPrepareOptions {
                selection: MeetingSelection::Token(
                    "20261124#^cedar-review".to_string(),
                ),
                parent: Some("job".to_string()),
                ..MeetingPrepareOptions::default()
            },
        );
        // Simulate a concurrent edit: the daily note changed after
        // preparation, so the locked recheck must refuse.
        write_file(
            &fixture.vault.join("2026/20261124.md"),
            "# Day\n## Events\n- [*] #task **1400-1430** Cedar Labs quarterly review CHANGED ^cedar-review\n## Pomodoros\n",
        );
        // Keep the stale expectation to prove the guard fires.
        assert_ne!(
            fs::read_to_string(fixture.vault.join("2026/20261124.md"))
                .expect("day")
                .lines()
                .nth(
                    prepared.guarded_root_edit.as_ref().expect("edit").line - 1
                )
                .unwrap_or(""),
            prepared
                .guarded_root_edit
                .as_ref()
                .expect("edit")
                .expected_line
                .as_str()
        );
        let result = fixture.file(&prepared);
        match &result.outcome {
            IntakeOutcome::Queued { reason, .. } => {
                assert_eq!(
                    *reason,
                    super::super::super::intake::QueueReason::Refused
                );
            }
            other => panic!("stale event must queue, got {other:?}"),
        }
        assert!(result.skipped_reroot, "skipped re-root reported");
        let after = fs::read_to_string(fixture.vault.join("2026/20261124.md"))
            .expect("day");
        assert!(
            after.contains("CHANGED"),
            "stale edit preserved, no overwrite"
        );
        // No reference was filed.
        assert!(!prepared.note_path.exists());
    }

    #[test]
    fn transcript_and_status_variants_file() {
        for status in ["next", "ready", "wip", "read"] {
            let fixture = RouteFixture::new(default_config());
            fixture.day(&cedar_row("[root:: job]", "https://example.com/e"));
            fixture.commit_seed(&[], "seed");
            let prepared = fixture.prepare(
                "Cedar Labs quarterly review\n\n## Next Steps\n- (Ari) Ship it\n\n## Transcript\nnotes transcript words",
                MeetingPrepareOptions {
                    selection: MeetingSelection::Token(
                        "20261124#^cedar-review".to_string(),
                    ),
                    status: Some(status.to_string()),
                    include_transcript: true,
                    ..MeetingPrepareOptions::default()
                },
            );
            assert_eq!(prepared.status, status);
            let result = fixture.file(&prepared);
            assert!(
                matches!(&result.outcome, IntakeOutcome::Ingested { .. }),
                "status {status} files"
            );
            let pointer = result.source_pointer.expect("pointer");
            let stored =
                crate::native::highlights_ref::source_store::read_store(
                    &fixture.vault,
                    &pointer,
                );
            let bytes = stored.bytes.expect("stored bytes");
            let text = String::from_utf8_lossy(&bytes);
            assert!(
                text.contains("Transcript"),
                "transcript kept when requested"
            );
        }
        // Without the flag the transcript stays out of the PDF input and
        // the saved source.
        let fixture = RouteFixture::new(default_config());
        fixture.day(&cedar_row("[root:: job]", "https://example.com/e"));
        fixture.commit_seed(&[], "seed");
        let prepared = fixture.prepare(
            "Cedar Labs quarterly review\n\n## Transcript\nsecret words",
            MeetingPrepareOptions {
                selection: MeetingSelection::Token(
                    "20261124#^cedar-review".to_string(),
                ),
                ..MeetingPrepareOptions::default()
            },
        );
        let result = fixture.file(&prepared);
        let pointer = result.source_pointer.expect("pointer");
        let stored = crate::native::highlights_ref::source_store::read_store(
            &fixture.vault,
            &pointer,
        );
        let text =
            String::from_utf8_lossy(&stored.bytes.expect("bytes")).into_owned();
        assert!(!text.contains("secret words"), "transcript omitted");
    }

    #[test]
    fn ingest_commit_holds_scoped_paths_and_reverts_cleanly() {
        let fixture = RouteFixture::new(default_config());
        fixture.day(&cedar_row(
            "[root:: job]",
            "https://calendar.google.com/event?eid=q",
        ));
        fixture.commit_seed(&[], "seed");
        let prepared = fixture.prepare(
            "Cedar Labs quarterly review\n\n- note",
            MeetingPrepareOptions {
                selection: MeetingSelection::Token(
                    "20261124#^cedar-review".to_string(),
                ),
                ..MeetingPrepareOptions::default()
            },
        );
        let result = fixture.file(&prepared);
        assert!(matches!(&result.outcome, IntakeOutcome::Ingested { .. }));
        // Scoped commit: exactly one ingest commit with the required
        // subject; vault-sync may add its own auto-commits around it, so
        // the count is not fixed. A clean tree proves every filed
        // artifact landed in commits (ingest plus vault-sync auto-commits
        // for the pre-sync untracked files).
        let log = Command::new("git")
            .args(["log", "--format=%s"])
            .current_dir(&fixture.vault)
            .output()
            .expect("git log");
        assert!(log.status.success());
        let subjects: Vec<_> = String::from_utf8_lossy(&log.stdout)
            .lines()
            .map(str::to_string)
            .collect();
        let ingest_subject =
            format!("bob ref meet: ingest meetings/{}", prepared.stem);
        assert_eq!(
            subjects.iter().filter(|s| *s == &ingest_subject).count(),
            1,
            "one scoped ingest commit: {subjects:?}"
        );
        let status = Command::new("git")
            .args(["status", "--porcelain=v1", "--untracked-files=all"])
            .current_dir(&fixture.vault)
            .output()
            .expect("git status");
        assert!(status.status.success());
        let porcelain = String::from_utf8_lossy(&status.stdout).into_owned();
        assert!(
            porcelain.trim().is_empty(),
            "clean tree after scoped commit: {porcelain:?}"
        );
        // The exact saved source round-trips through the dossier parser
        // and matches the prepared notes.
        let pointer = result.source_pointer.expect("pointer");
        let stored = crate::native::highlights_ref::source_store::read_store(
            &fixture.vault,
            &pointer,
        );
        let bytes = stored.bytes.expect("stored bytes");
        let document = crate::native::meetings::parse_dossier(
            &String::from_utf8_lossy(&bytes),
        )
        .expect("dossier round-trips");
        assert_eq!(document.notes.tldr, prepared.notes.tldr);
        // The persisted reference note carries the same pointer.
        let note_text =
            fs::read_to_string(&prepared.note_path).expect("read note");
        assert!(
            note_text.contains(&pointer),
            "frontmatter keeps the expected pointer"
        );
        // One-commit revert of the ingest commit removes every filed artifact.
        let ingest_sha = match &result.outcome {
            IntakeOutcome::Ingested {
                commit_sha: Some(sha),
                ..
            } => sha.clone(),
            other => panic!("ingest must commit, got {other:?}"),
        };
        let output = Command::new("git")
            .args(["revert", "--no-commit", &ingest_sha])
            .current_dir(&fixture.vault)
            .output()
            .expect("git revert");
        assert!(
            output.status.success(),
            "one-commit revert: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let status = Command::new("git")
            .args(["status", "--porcelain=v1", "--untracked-files=all"])
            .current_dir(&fixture.vault)
            .output()
            .expect("git status");
        assert!(status.status.success());
        // Abort the revert so the fixture stays clean for drop.
        let _ = Command::new("git")
            .args(["revert", "--abort"])
            .current_dir(&fixture.vault)
            .output();
    }

    #[test]
    fn dirty_daily_note_is_preserved_across_reroot() {
        let fixture = RouteFixture::new(default_config());
        fixture.day("- [*] #task **1400-1430** Cedar Labs quarterly review ^cedar-review");
        fixture.commit_seed(&[], "seed");
        // Dirty the daily note before filing: pre-sync auto-commits the
        // extra line, then the locked re-root applies on top without
        // losing it.
        fs::write(
            fixture.vault.join("2026/20261124.md"),
            "# Day\n## Events\n- [*] #task **1400-1430** Cedar Labs quarterly review ^cedar-review\nuncommitted change\n## Pomodoros\n",
        )
        .expect("dirty day");
        let prepared = fixture.prepare(
            "Cedar Labs quarterly review\n\n- note",
            MeetingPrepareOptions {
                selection: MeetingSelection::Token(
                    "20261124#^cedar-review".to_string(),
                ),
                parent: Some("job".to_string()),
                ..MeetingPrepareOptions::default()
            },
        );
        assert!(prepared.guarded_root_edit.is_some());
        let result = fixture.file(&prepared);
        assert!(
            matches!(&result.outcome, IntakeOutcome::Ingested { .. }),
            "pre-sync auto-commit lets the filing proceed: {:?}",
            result.outcome
        );
        assert_eq!(result.reroot_applied, Some((None, "job".to_string())));
        let after = fs::read_to_string(fixture.vault.join("2026/20261124.md"))
            .expect("day");
        assert!(
            after.contains("uncommitted change"),
            "authored dirty line preserved"
        );
        assert!(after.contains("[root:: job]"), "re-root applied");
    }

    #[test]
    fn queued_intake_skips_reroot_and_drains_without_it() {
        let fixture = RouteFixture::new(default_config());
        fixture.day("- [*] #task **1400-1430** Cedar Labs quarterly review ^cedar-review");
        fixture.commit_seed(&[], "seed");
        let prepared = fixture.prepare(
            "Cedar Labs quarterly review\n\n- note",
            MeetingPrepareOptions {
                selection: MeetingSelection::Token(
                    "20261124#^cedar-review".to_string(),
                ),
                parent: Some("job".to_string()),
                ..MeetingPrepareOptions::default()
            },
        );
        // Hold the scan writer lock so intake queues; the daily note
        // must stay untouched and the queued drain must not replay the
        // abandoned re-root intent.
        let _scan_guard = super::super::super::acquire_scan_writer_lock(false)
            .expect("hold scan lock");
        let result = fixture.file(&prepared);
        match &result.outcome {
            IntakeOutcome::Queued { .. } => {}
            other => panic!("lock contention must queue, got {other:?}"),
        }
        assert!(result.skipped_reroot, "skipped re-root reported");
        let day_before =
            fs::read_to_string(fixture.vault.join("2026/20261124.md"))
                .expect("day");
        assert!(
            !day_before.contains("[root::"),
            "queued ingest leaves the daily note untouched"
        );
        drop(_scan_guard);
        // Drain the queue: the meeting files, but the daily note still
        // has no root because the re-root intent was abandoned.
        let mut progress = |_line: &str| {};
        let mut seen = 0;
        let outcome = super::super::super::intake::run_intake_only_tracked(
            &fixture.config,
            false,
            &mut progress,
            &mut |_item| {
                seen += 1;
            },
        );
        assert!(outcome.lock_held, "drain took the lock");
        let run = outcome.run.expect("drain runs");
        match run {
            super::super::super::intake::IntakeOnlyRun::Drained(report) => {
                assert_eq!(report.filed_count(), 1, "queued PDF drains");
            }
            _ => panic!("drain must file"),
        }
        assert_eq!(seen, 1);
        let day_after =
            fs::read_to_string(fixture.vault.join("2026/20261124.md"))
                .expect("day");
        assert!(
            !day_after.contains("[root::"),
            "drain births the marker without replaying the re-root"
        );
        assert!(
            prepared.note_path.is_file(),
            "drained filing writes the note"
        );
    }

    #[test]
    fn competing_filing_with_a_different_stem_refuses_after_pre_sync() {
        let fixture = RouteFixture::new(default_config());
        let url = "https://calendar.google.com/event?eid=cedar-event";
        fixture.day(&cedar_row("[root:: job]", url));
        fixture.commit_seed(&[], "seed");
        let prepared = fixture.prepare(
            "Cedar Labs quarterly review\n\n## Next Steps\n- (Ari) Ship it",
            MeetingPrepareOptions {
                selection: MeetingSelection::Token(
                    "20261124#^cedar-review".to_string(),
                ),
                ..MeetingPrepareOptions::default()
            },
        );
        assert_ne!(
            prepared.stem, "other_host_stem",
            "planned stem must differ from the competing live stem"
        );
        write_file(
            &fixture.vault.join("ref/meetings/other_host_stem.md"),
            concat!(
                "---\ntype: [[ref]]\nstatus: next\n",
                "event_id: cedar-event\n",
                "event: \"[[20261124#^cedar-review]]\"\n---\n",
                "# Other Host Filing\n",
            ),
        );
        fixture.commit_seed(&[], "competing host filing");
        let result = fixture.file(&prepared);
        match &result.outcome {
            IntakeOutcome::Queued { reason, detail, .. } => {
                assert_eq!(
                    *reason,
                    super::super::super::intake::QueueReason::Refused
                );
                assert!(
                    detail.contains("already filed"),
                    "occurrence identity, not filename: {detail}"
                );
            }
            other => panic!("competing occurrence must queue, got {other:?}"),
        }
        assert!(
            !prepared.note_path.exists(),
            "planned stem must stay unfiled when a different stem occupies the event"
        );
    }

    #[test]
    fn competing_stem_with_missing_root_leaves_the_daily_note_untouched() {
        let fixture = RouteFixture::new(default_config());
        fixture.day(
            "- [*] #task **1400-1430** Cedar Labs quarterly review [📅](https://calendar.google.com/event?eid=cedar-event \"Open\") ^cedar-review",
        );
        fixture.commit_seed(&[], "seed");
        let before =
            fs::read(fixture.vault.join("2026/20261124.md")).expect("day");
        let prepared = fixture.prepare(
            "Cedar Labs quarterly review\n\n- note",
            MeetingPrepareOptions {
                selection: MeetingSelection::Token(
                    "20261124#^cedar-review".to_string(),
                ),
                parent: Some("job".to_string()),
                ..MeetingPrepareOptions::default()
            },
        );
        assert!(prepared.guarded_root_edit.is_some());
        write_file(
            &fixture.vault.join("ref/meetings/other_host_stem.md"),
            concat!(
                "---\ntype: [[ref]]\nstatus: next\n",
                "event_id: cedar-event\n",
                "event: \"[[20261124#^cedar-review]]\"\n---\n",
                "# Other Host Filing\n",
            ),
        );
        fixture.commit_seed(&[], "competing host filing");
        let result = fixture.file(&prepared);
        assert!(
            matches!(&result.outcome, IntakeOutcome::Queued { .. }),
            "competing occurrence queues: {:?}",
            result.outcome
        );
        assert_eq!(
            before,
            fs::read(fixture.vault.join("2026/20261124.md"))
                .expect("day after"),
            "incomplete/queued occurrence must not apply the prepared re-root"
        );
    }

    fn live_note_body(path: &std::path::Path) -> String {
        let contents = fs::read_to_string(path).expect("note");
        crate::native::highlights_ref::split_frontmatter(&contents)
            .map(|(_, body)| body)
            .unwrap_or(contents)
    }

    fn parent_task_block(
        vault: &std::path::Path,
        parent: &str,
        block_id: &str,
    ) -> String {
        let body = fs::read_to_string(vault.join(format!("{parent}.md")))
            .expect("parent note");
        let lines: Vec<&str> = body.lines().collect();
        let start = lines
            .iter()
            .position(|line| line.contains(&format!("^{block_id}")))
            .unwrap_or_else(|| panic!("missing task ^{block_id} in {body}"));
        let indent = lines[start]
            .chars()
            .take_while(|ch| matches!(ch, ' ' | '\t'))
            .count();
        let mut end = start + 1;
        while end < lines.len() {
            let line = lines[end];
            if line.trim().is_empty() {
                end += 1;
                continue;
            }
            let child = line
                .chars()
                .take_while(|ch| matches!(ch, ' ' | '\t'))
                .count();
            if child <= indent {
                break;
            }
            end += 1;
        }
        lines[start..end].join("\n")
    }

    fn cedar_options(overwrite: bool) -> MeetingPrepareOptions {
        MeetingPrepareOptions {
            selection: MeetingSelection::Token(
                "20261124#^cedar-review".to_string(),
            ),
            overwrite,
            ..MeetingPrepareOptions::default()
        }
    }

    #[test]
    fn overwrite_reseeds_owned_recap_and_preserves_identity() {
        let fixture = RouteFixture::new(default_config());
        let url = "https://calendar.google.com/event?eid=cedar-event";
        fixture.day(&cedar_row("[root:: job]", url));
        fixture.commit_seed(&[], "seed");
        let first = fixture.prepare(
            "Cedar Labs quarterly review\n\n## Next Steps\n- (Ari) Ship it",
            cedar_options(false),
        );
        let filed = fixture.file(&first);
        assert!(matches!(&filed.outcome, IntakeOutcome::Ingested { .. }));
        let old_pointer = filed.source_pointer.clone().expect("old pointer");
        let old_links =
            parent_task_block(&fixture.vault, "job", &first.task_block_id);
        assert!(
            old_links.contains("LINKS"),
            "birth writes a LINKS child:\n{old_links}"
        );
        let already = prepare_filing(
            &fixture.config,
            &fixture.notes_input(
                "Cedar Labs quarterly review\n\n## Next Steps\n- (Ari) Ship v2",
            ),
            &cedar_options(false),
        )
        .expect_err("google identity refuses without overwrite");
        assert_eq!(already.code, "already_filed");

        let prepared = fixture.prepare(
            "Cedar Labs quarterly review\n\n## Next Steps\n- (Ari) Ship v2",
            cedar_options(true),
        );
        assert_eq!(prepared.stem, first.stem);
        assert_eq!(prepared.task_block_id, first.task_block_id);
        assert_eq!(prepared.status, first.status);
        assert!(!prepared.status_explicit);
        let replaced = fixture.replace(&prepared);
        assert!(replaced.pages > 0, "replacement still renders a PDF");
        assert_eq!(
            replaced.overwrite.task_block_id,
            Some(first.task_block_id.clone())
        );
        assert_eq!(replaced.overwrite.task_status, first.status);
        assert!(
            replaced.overwrite.warnings.is_empty(),
            "{:?}",
            replaced.warnings
        );
        let subjects = commit_subjects(&fixture.vault);
        let overwrite_subject =
            format!("bob ref meet: overwrite meetings/{}", prepared.stem);
        assert!(
            subjects.iter().any(|subject| subject == &overwrite_subject),
            "meeting overwrite subject: {subjects:?}"
        );
        let body = live_note_body(&prepared.note_path);
        assert_eq!(recap_state(&body), RecapState::Owned);
        assert!(body.contains("Ship v2"), "owned recap reseeds:\n{body}");
        assert!(
            !body.contains("Ship it"),
            "old owned recap leaves the live note:\n{body}"
        );
        let history = fixture.vault.join("ref_history");
        if history.exists() {
            let frozen = fs::read_to_string(
                fs::read_dir(&history)
                    .expect("history")
                    .flatten()
                    .map(|entry| entry.path())
                    .find(|path| {
                        path.extension().is_some_and(|ext| ext == "md")
                    })
                    .unwrap_or_else(|| history.join("missing.md")),
            )
            .unwrap_or_default();
            assert!(
                !frozen.contains("bob:meeting-recap:begin"),
                "recap must stay off ref_history:\n{frozen}"
            );
        }
        let new_links =
            parent_task_block(&fixture.vault, "job", &first.task_block_id);
        assert_eq!(new_links, old_links, "LINKS and task identity stay");
        let new_pointer = replaced.source_pointer.expect("new pointer");
        assert_ne!(new_pointer, old_pointer, "source store grows a new object");
        let old_store = crate::native::highlights_ref::source_store::read_store(
            &fixture.vault,
            &old_pointer,
        );
        assert!(old_store.bytes.is_some(), "old source remains as history");
        let new_store = crate::native::highlights_ref::source_store::read_store(
            &fixture.vault,
            &new_pointer,
        );
        let new_bytes = new_store.bytes.expect("new bytes");
        let new_text = String::from_utf8_lossy(&new_bytes);
        assert!(new_text.contains("Ship v2"), "replacement source saved");
    }

    #[test]
    fn overwrite_keeps_edited_recap_and_explicit_status() {
        let fixture = RouteFixture::new(default_config());
        fixture.day(&cedar_row("[root:: job]", "https://example.com/e"));
        fixture.commit_seed(&[], "seed");
        let first = fixture.prepare(
            "Cedar Labs quarterly review\n\n## Next Steps\n- (Ari) Ship it",
            cedar_options(false),
        );
        fixture.file(&first);
        let mut note = fs::read_to_string(&first.note_path).expect("note");
        if let Some(inner) = note.find(" -->\n") {
            note.insert_str(inner + 5, "edited ");
        } else {
            panic!("filed note must carry an owned recap:\n{note}");
        }
        fs::write(&first.note_path, note).expect("edit recap");
        fixture.commit_seed(&[], "edit recap");
        let prepared = fixture.prepare(
            "Cedar Labs quarterly review\n\n## Next Steps\n- (Ari) Ship v2",
            MeetingPrepareOptions {
                selection: MeetingSelection::Token(
                    "20261124#^cedar-review".to_string(),
                ),
                overwrite: true,
                status: Some("read".into()),
                ..MeetingPrepareOptions::default()
            },
        );
        assert!(prepared.status_explicit);
        assert_eq!(prepared.status, "read");
        let replaced = fixture.replace(&prepared);
        assert_eq!(replaced.overwrite.task_status, "read");
        assert!(
            replaced
                .warnings
                .iter()
                .any(|warning| warning == EDITED_RECAP_WARNING),
            "edited recap warns: {:?}",
            replaced.warnings
        );
        let body = live_note_body(&prepared.note_path);
        assert!(body.contains("edited "), "edited recap bytes stay:\n{body}");
        assert!(
            body.contains("Ship it"),
            "edited recap keeps the old inner text:\n{body}"
        );
        assert!(
            !body.contains("Ship v2"),
            "replacement must not clobber an edited recap:\n{body}"
        );
    }

    #[test]
    fn overwrite_without_existing_files_as_new() {
        let fixture = RouteFixture::new(default_config());
        fixture.day(&cedar_row("[root:: job]", "https://example.com/e"));
        fixture.commit_seed(&[], "seed");
        let prepared = fixture.prepare(
            "Cedar Labs quarterly review\n\n- note",
            cedar_options(true),
        );
        assert!(
            prepared.existing.is_none(),
            "force with no occurrence is a first filing"
        );
        match fixture.write(&prepared) {
            MeetWriteResult::Filed(result) => {
                assert!(matches!(
                    result.outcome,
                    IntakeOutcome::Ingested { .. }
                ));
            }
            MeetWriteResult::Replaced(_) => {
                panic!("-O with no existing files as new, not as an overwrite")
            }
        }
        assert!(prepared.note_path.is_file());
    }

    #[test]
    fn overwrite_prepare_follows_a_renamed_reference() {
        let fixture = RouteFixture::new(default_config());
        fixture.day(&cedar_row("[root:: job]", "https://example.com/e"));
        fixture.commit_seed(&[], "seed");
        let first = fixture.prepare(
            "Cedar Labs quarterly review\n\n## Next Steps\n- (Ari) Ship it",
            cedar_options(false),
        );
        fixture.file(&first);
        let new_stem = format!("{}_renamed", first.stem);
        let old_rel = format!("ref/meetings/{}.md", first.stem);
        let new_rel = format!("ref/meetings/{new_stem}.md");
        git(&fixture.vault, &["mv", &old_rel, &new_rel]);
        fixture.commit_seed(&[], "rename meeting note");
        let prepared = fixture.prepare(
            "Cedar Labs quarterly review\n\n## Next Steps\n- (Ari) Ship v2",
            cedar_options(true),
        );
        assert_eq!(prepared.stem, new_stem);
        assert_eq!(prepared.task_block_id, first.task_block_id);
        assert_eq!(
            prepared
                .existing
                .as_ref()
                .map(|existing| existing.filed.note.as_str()),
            Some(new_rel.as_str()),
            "overwrite keeps the live renamed path"
        );
        assert!(
            !first.note_path.exists(),
            "the old note path is no longer the live member"
        );
    }

    #[test]
    fn manual_event_dedupes_by_target_then_overwrites() {
        let fixture = RouteFixture::new(default_config());
        fixture.day(
            "- [*] #task **1400-1430** Cedar Labs quarterly review [root:: job] ^cedar-review",
        );
        fixture.commit_seed(&[], "seed");
        let first = fixture.prepare(
            "Cedar Labs quarterly review\n\n- first notes",
            cedar_options(false),
        );
        assert!(
            first.meeting.event_id.is_none(),
            "manual event has no Google id"
        );
        fixture.file(&first);
        let already = prepare_filing(
            &fixture.config,
            &fixture
                .notes_input("Cedar Labs quarterly review\n\n- second notes"),
            &cedar_options(false),
        )
        .expect_err("manual identity refuses without overwrite");
        assert_eq!(already.code, "already_filed");
        let prepared = fixture.prepare(
            "Cedar Labs quarterly review\n\n- second notes",
            cedar_options(true),
        );
        assert_eq!(prepared.stem, first.stem);
        let replaced = fixture.replace(&prepared);
        assert_eq!(replaced.overwrite.task_block_id, Some(first.task_block_id));
        let body = live_note_body(&prepared.note_path);
        assert!(
            body.contains("second notes"),
            "manual overwrite reseeds:\n{body}"
        );
    }
}
