//! `bob gmail sync` live archive apply: one-shot mutation, let-go, and
//! crash reconciliation over the fake transport.
//!
//! Every live run must keep durable filings when archive setup or per-thread
//! work fails, isolate non-auth failures, abort on `invalid_grant`, create
//! no labels for paused batches, and converge crashes without reclaiming.
//!
//! Core conformance audit (C1-C20):
//! C1 `live_archives_a_fresh_close_and_repeats_as_noop`; C2
//! `cancelled_linker_is_positive_archive_evidence`; C3/C4/C5
//! `archive_scan::{open_linkers_veto_including_open_rows_in_done,
//! no_linkers_means_no_evidence,positive_closed_covers_cancelled_uppercase_and_done}`;
//! C6/C7 `archive_preview::{remote_verdict_keeps_unsaved_consent_and_policy,
//! remote_verdict_holds_unresolvable_and_manual_or_reconcile}` plus
//! `boundary_fresh_close_unresolvable_preview_fails_without_writes` and
//! `boundary_unresolvable_journaled_close_fails_live_and_preview`; C8
//! `live_keeps_unsaved_inbox_mail_without_mutation`; C9/C10
//! `new_reply_lets_go_and_preserves_inbox` / `manual_move_to_inbox_lets_go_without_removing_inbox`;
//! C11 `reopening_an_archived_task_reports_a_restore_hint_without_writing`;
//! C12/C13 `archive_preview::gate_requires_backlog_and_burst_confirmation` /
//! `gmail_archive_preview::dry_run_pauses_backlog_and_confirms_with_the_digest`;
//! C14 `live_reconciles_after_remote_success_and_journal_failure_without_remutating` /
//! `boundary_journal_failure_keeps_prior_row_and_recovers_the_later_write`;
//! C15 `archive_preview::remote_verdict_keeps_unsaved_consent_and_policy`;
//! C16 `live_missing_modify_grant_is_setup_with_login_hint` plus
//! `boundary_invalid_grant_keeps_same_run_archive_and_stops_later_writes` and
//! `boundary_invalid_grant_human_report_keeps_same_run_archive`, and the dry-run
//! cases in `gmail_archive_preview`; C17/C18
//! `archive_preview::{local_verdict_holds_merge_and_identity_evidence,
//! local_verdict_suppresses_same_run_incoming_moves}`; C19
//! `gmail_archive_preview::dry_run_pauses_backlog_and_confirms_with_the_digest`
//! and `cli::sync_accepts_confirm_no_archive_and_preserves_pull`; C20
//! `archive_reports_and_journals_never_echo_hostile_task_text` and the
//! body-free terminal tests in `archive_state`.
//! Mutation-boundary faults use actual fake-gog transitions:
//! `boundary_reopened_between_planning_and_mutation_holds_without_writing`,
//! `boundary_new_unreadable_note_between_plan_and_revalidation_holds`, and
//! `boundary_planned_kept_turning_archivable_is_held`.
//!
//! Surfaces (section 4): `report::{live_success_omits_digest_in_json_and_hides_quiet_leak,
//! dry_run_and_paused_runs_keep_digest,
//! paused_rows_show_gate_reason_and_decision_without_dangling_arrow,
//! kept_and_let_go_rows_render_note_without_dangling_arrow,
//! no_archive_skip_serializes_additively}` plus
//! `gmail_archive_preview::first_enablement_dry_run_previews_the_same_confirmable_backlog`
//! (live success omits `archive_digest`) and
//! `gmail_sync::help_lists_sync_and_hides_the_pull_alias`
//! (sync `-c`/`-n` option order).
//!
//! Receipts (bob-cli-kx.3.1) acceptance matrix:
//! - C1/C2/C5 receipt bytes and notification: `live_archives_a_fresh_close_and_repeats_as_noop`,
//!   `receipt_lifecycle_materializes_banner_and_clears_on_let_go`,
//!   `cancelled_linker_is_positive_archive_evidence`,
//!   `archive_scan::positive_closed_covers_cancelled_uppercase_and_done`,
//!   `quiet_archive_notifies_once_then_stays_silent_on_repeat`,
//!   `recovered_archive_notifies_once_without_remutating`.
//! - C9/C10 clearing: `new_reply_lets_go_and_preserves_inbox`,
//!   `manual_move_to_inbox_lets_go_without_removing_inbox`,
//!   `receipt_lifecycle_materializes_banner_and_clears_on_let_go`.
//! - C11 no auto-restore: `reopening_an_archived_task_reports_a_restore_hint_without_writing`,
//!   `reopening_an_archived_task_retains_its_receipt_banner`.
//! - C14 recovered visibility: `live_reconciles_after_remote_success_and_journal_failure_without_remutating`,
//!   `boundary_recovery_after_crash_followed_by_reply_records_once`.
//! - C16/C19 read-only preview: `unarchive_dry_run_needs_no_writer_and_writes_nothing`,
//!   `dry_run_and_skipped_archive_write_no_receipt_bytes`,
//!   `archive_preview::remote_verdict_keeps_unsaved_consent_and_policy`,
//!   `gmail_archive_preview::dry_run_pauses_backlog_and_confirms_with_the_digest`.
//! - C20 inert output, argv, and journals: `archive_reports_and_journals_never_echo_hostile_task_text`,
//!   `quiet_archive_notifies_once_then_stays_silent_on_repeat`,
//!   `cli::unarchive_banner_argv_keeps_json_and_one_exact_operand`.
//! - `recent_archive` timezone/DST and generation: `archive_projection::previous_calendar_day_starts_at_local_midnight_across_dst`,
//!   `archive_state::recent_projection_keeps_each_generation_but_counts_one_thread_once`,
//!   `archive_state::reopen_rearms_a_new_generation_and_stale_open_cannot_rearm_it_again`.
//! - Undo faults: `unarchive_wrong_writer_fails_before_gog_or_vault_writes`,
//!   `unarchive_missing_modify_scope_is_typed_setup_failure_without_writes`,
//!   `unarchive_partial_metadata_failure_settles_only_the_successful_thread`,
//!   `unarchive_retries_after_remote_success_and_journal_append_failure`,
//!   `unarchive_detects_concurrent_release_and_sequential_reply_let_go`,
//!   `unarchive_invalid_grant_reports_remaining_input_without_mutating_it`,
//!   `unarchive_policy_exclusions_do_not_mutate_spam_or_trash`,
//!   `unarchive_ambiguous_stem_fails_without_mutating_notes_or_mail`,
//!   `unarchive_other_account_note_fails_and_unknown_thread_is_unchanged`,
//!   `unarchive_unsafe_journal_fails_each_input_without_mutation`,
//!   `unarchive_restores_only_frozen_ids_and_clears_the_note_receipt` (duplicate inputs).
//! - Complete flows: `archive_restore_sync_flow_shows_historical_restored_state_without_rearchiving`,
//!   `reopen_restore_and_reclose_archives_a_new_generation`,
//!   `receipt_lifecycle_materializes_banner_and_clears_on_let_go`,
//!   `manual_move_to_inbox_lets_go_without_removing_inbox`.
//! - bob-plugins suites: `scripts/test-bob-mail-archive-banner.cjs`,
//!   `scripts/test-ledger-tools-mail-link-marks.cjs`,
//!   `scripts/test-ledger-tools-gcal-events.cjs`.

mod gmail_support;

use std::fs;
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

use gmail_support::{
    json_stdout, run_gmail, run_gmail_with_notify, stderr, stdout, FakeGog,
    FakeNotify, GmailEnv, ACCOUNT, HOST,
};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

const THREAD_A: &str = "0123456789abcdef";
const THREAD_B: &str = "aaaaaaaaaaaaaaaa";
const THREAD_D: &str = "ddddddddddddddd0";
const MESSAGE_A: &str = "00000000000000a0";
const MESSAGE_B: &str = "00000000000000b1";
const MESSAGE_C: &str = "00000000000000c2";
const MESSAGE_D: &str = "00000000000000d3";
const ENROLLMENT_ID: &str = "Label_9";
const RECEIPT_ID: &str = "Label_10";

fn assert_exit(output: &std::process::Output, expected: i32, label: &str) {
    assert_eq!(
        output.status.code(),
        Some(expected),
        "{label} expected exit {expected}:\nstdout:\n{}\nstderr:\n{}",
        stdout(output),
        stderr(output)
    );
}

fn write_archive_config(env: &GmailEnv) {
    env.write_config(&format!(
        "timezone: America/New_York\ngmail:\n  account: {ACCOUNT}\n  writer: {HOST}\n  sync:\n    archive: on\n"
    ));
}

fn write_task_settings(env: &GmailEnv) {
    let path = env
        .vault()
        .join(".obsidian/plugins/obsidian-tasks-plugin/data.json");
    fs::create_dir_all(path.parent().unwrap()).expect("tasks dir");
    fs::write(
        path,
        serde_json::to_vec(&json!({
            "globalFilter": "#task",
            "taskFormat": "dataview",
            "statusSettings": {
                "coreStatuses": [
                    {"symbol": " ", "name": "Ready", "type": "TODO"},
                    {"symbol": "x", "name": "Done", "type": "DONE"}
                ],
                "customStatuses": [
                    {"symbol": "*", "name": "Next", "nextStatusSymbol": "x", "availableAsCommand": true, "type": "ON_HOLD"},
                    {"symbol": "/", "name": "In Progress", "nextStatusSymbol": "x", "availableAsCommand": true, "type": "IN_PROGRESS"},
                    {"symbol": "?", "name": "Blocked", "nextStatusSymbol": " ", "availableAsCommand": true, "type": "ON_HOLD"},
                    {"symbol": "-", "name": "Canceled", "nextStatusSymbol": " ", "availableAsCommand": true, "type": "CANCELLED"}
                ]
            }
        }))
        .expect("serialize settings"),
    )
    .expect("write settings");
}

fn seed_labels(fake: &FakeGog, with_receipt: bool) {
    let mut labels = vec![
        json!({"id": ENROLLMENT_ID, "name": "bob", "type": "user"}),
        json!({"id": "INBOX", "name": "INBOX", "type": "system"}),
    ];
    if with_receipt {
        labels.push(
            json!({"id": RECEIPT_ID, "name": "bob/closed", "type": "user"}),
        );
    }
    fake.set_labels(Value::Array(labels));
}

fn thread_message(id: &str, labels: &[&str]) -> Value {
    json!({
        "id": id,
        "threadId": THREAD_A,
        "labelIds": labels,
        "internalDate": "1793908800000",
        "snippet": "A metadata-only archive fixture.",
        "payload": {
            "mimeType": "text/plain",
            "headers": [
                {"name": "From", "value": "Ada <ada@example.com>"},
                {"name": "To", "value": ACCOUNT},
                {"name": "Subject", "value": "Archive apply probe"},
                {"name": "Message-ID", "value": format!("<{id}@example.test>")}
            ],
            "body": {"size": 5, "data": "SGVsbG8="}
        }
    })
}

fn seed_thread_and_messages(fake: &FakeGog, messages: Vec<Value>) {
    fake.set_threads(json!({
        THREAD_A: {"thread": {"id": THREAD_A, "historyId": "1", "messages": messages.clone()}}
    }));
    let mut map = serde_json::Map::new();
    for message in &messages {
        let id = message["id"].as_str().expect("message id").to_string();
        let label_ids = message["labelIds"].clone();
        map.insert(id, json!({"threadId": THREAD_A, "labelIds": label_ids}));
    }
    fake.set_messages(Value::Object(map));
}

fn journal_text(env: &GmailEnv) -> String {
    let dir = env.vault().join(".bob/gmail");
    let Ok(entries) = fs::read_dir(dir) else {
        return String::new();
    };
    let mut lines: Vec<String> = entries
        .flatten()
        .filter_map(|entry| fs::read_to_string(entry.path()).ok())
        .collect();
    lines.sort();
    lines.join("\n")
}

fn journal_has(env: &GmailEnv, kind: &str) -> bool {
    journal_text(env).contains(kind)
}

fn remote_write_calls(fake: &FakeGog) -> Vec<Vec<String>> {
    fake.calls()
        .into_iter()
        .filter(|call| {
            call.iter().any(|arg| {
                arg == "gmail.batch.modify" || arg == "gmail.labels.create"
            })
        })
        .collect()
}

fn make_recovery_unreadable(env: &GmailEnv) {
    let mut keyed = ACCOUNT.to_ascii_lowercase().into_bytes();
    keyed.push(0);
    keyed.extend_from_slice(
        fs::canonicalize(env.vault())
            .expect("canonical vault path")
            .to_string_lossy()
            .as_bytes(),
    );
    let digest = hex::encode(Sha256::digest(keyed));
    let path = env
        .root()
        .join("state/bob/gmail")
        .join(format!("sync-recovery-{digest}.json"));
    fs::create_dir_all(path).expect("create unreadable recovery directory");
}

fn file_adopt(env: &GmailEnv, fake: &FakeGog) -> (String, String) {
    let saved = run_gmail(env, fake, &["save", THREAD_A, "-f", "json"]);
    assert_eq!(
        saved.status.code(),
        Some(0),
        "save the watched mail note:\n{}\n{}",
        stdout(&saved),
        stderr(&saved)
    );
    let doc = json_stdout(&saved);
    let note = doc["note"].as_str().expect("saved note").to_string();
    let link = doc["link"].as_str().expect("saved link").to_string();
    let inbox = env.vault().join("gmail_inbox.md");
    fs::write(
        &inbox,
        format!(
            "#task archive apply probe {link}\n\n- [ ] #task archive apply probe {link}\n"
        ),
    )
    .expect("write inbox task");
    let adopted =
        run_gmail(env, fake, &["sync", "-l", "0", "-r", "0", "-f", "json"]);
    assert_eq!(
        adopted.status.code(),
        Some(0),
        "adopt the watched thread:\n{}\n{}",
        stdout(&adopted),
        stderr(&adopted)
    );
    (note, link)
}

fn close_inbox_task(env: &GmailEnv) {
    set_inbox_task_status(env, 'x');
}

fn set_inbox_task_status(env: &GmailEnv, status: char) {
    let inbox = env.vault().join("gmail_inbox.md");
    let text = fs::read_to_string(&inbox).expect("read inbox");
    let task = text
        .lines()
        .find(|line| line.starts_with("- [") && line.contains("#task"))
        .expect("inbox task row");
    let replacement = format!("- [{status}]{}", &task[5..]);
    fs::write(&inbox, text.replacen(task, &replacement, 1))
        .expect("update inbox task status");
}

#[test]
fn live_archives_a_fresh_close_and_repeats_as_noop() {
    let env = GmailEnv::new("bob-cli-gmail-archive-apply-fresh");
    env.write_journal_rules();
    write_archive_config(&env);
    write_task_settings(&env);
    let fake = FakeGog::new(&env, "gog").scenario("modify_grant");
    seed_labels(&fake, true);
    seed_thread_and_messages(
        &fake,
        vec![thread_message(MESSAGE_A, &["INBOX", ENROLLMENT_ID])],
    );
    let (note, _) = file_adopt(&env, &fake);
    fake.clear_calls();
    close_inbox_task(&env);
    let archived =
        run_gmail(&env, &fake, &["sync", "-l", "0", "-r", "0", "-f", "json"]);
    assert_exit(&archived, 0, "fresh close archives without confirm");
    let value = json_stdout(&archived);
    let archive = value["archive"].as_array().expect("archive rows");
    assert_eq!(archive.len(), 1, "one live archive row");
    assert_eq!(archive[0]["thread_id"], THREAD_A);
    assert_eq!(archive[0]["action"], "archived");
    assert_eq!(archive[0]["messages"], json!([MESSAGE_A]));
    let recent = &value["recent_archive"];
    assert!(recent["since"].as_str().is_some_and(|since| {
        chrono::DateTime::parse_from_rfc3339(since).is_ok()
    }));
    assert_eq!(recent["count"], 1);
    assert_eq!(recent["items"][0]["thread_id"], THREAD_A);
    assert_eq!(recent["items"][0]["note"], note);
    assert_eq!(recent["items"][0]["message_count"], 1);
    assert_eq!(recent["items"][0]["label"]["name"], "bob/closed");
    assert_eq!(recent["items"][0]["label"]["id"], RECEIPT_ID);
    assert_eq!(recent["items"][0]["state"], "archived");
    assert!(recent["items"][0]["archive_event_id"]
        .as_str()
        .is_some_and(|id| !id.is_empty()));
    assert!(
        journal_has(&env, "gmail.archived"),
        "journal records the archive"
    );
    assert_eq!(
        fake.message_labels(MESSAGE_A),
        vec![RECEIPT_ID.to_string(), ENROLLMENT_ID.to_string()],
        "INBOX removed and receipt added"
    );
    let writes = remote_write_calls(&fake);
    assert!(
        writes
            .iter()
            .any(|call| call.contains(&"gmail.batch.modify".to_string())),
        "one batch modify ran"
    );
    assert!(
        !writes
            .iter()
            .any(|call| call.contains(&"gmail.labels.create".to_string())),
        "existing receipt needs no label creation"
    );
    fake.clear_calls();
    let listed = run_gmail(&env, &fake, &["list", "-a", "-f", "json"]);
    assert_exit(&listed, 0, "offline list exposes active receipts");
    assert!(
        fake.calls().is_empty(),
        "list receipt projection stays offline"
    );
    let list_value = json_stdout(&listed);
    let listed_thread = list_value["threads"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["account"] == ACCOUNT && row["thread_id"] == THREAD_A)
        .expect("exact indexed thread");
    assert_eq!(
        listed_thread["archive_receipts"].as_array().unwrap().len(),
        1
    );
    let listed_receipt = &listed_thread["archive_receipts"][0];
    assert_eq!(listed_receipt["message_count"], 1);
    assert_eq!(listed_receipt["label"]["name"], "bob/closed");
    assert_eq!(listed_receipt["label"]["id"], RECEIPT_ID);
    assert!(listed_receipt["archive_event_id"]
        .as_str()
        .is_some_and(|id| !id.is_empty()));
    assert!(list_value["diagnostics"].as_array().unwrap().is_empty());
    let dry_run = run_gmail(
        &env,
        &fake,
        &["sync", "-d", "-l", "0", "-r", "0", "-f", "json"],
    );
    assert_exit(&dry_run, 0, "dry-run can read prior archive history");
    let dry_value = json_stdout(&dry_run);
    assert_eq!(dry_value["recent_archive"]["count"], 1);
    assert_eq!(dry_value["recent_archive"]["items"][0]["state"], "archived");
    assert!(
        dry_value["archive"].as_array().unwrap().is_empty(),
        "existing history is separate from projected archive actions"
    );
    assert!(remote_write_calls(&fake).is_empty());
    let doctor = run_gmail(&env, &fake, &["doctor", "-f", "json"]);
    assert_exit(&doctor, 0, "doctor after archive");
    let doctor_json = json_stdout(&doctor);
    let archive_check = doctor_json["checks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|check| check["id"] == "archive")
        .expect("archive doctor row");
    assert!(archive_check["summary"]
        .as_str()
        .unwrap()
        .contains("this week 1 archived · 0 kept"));
    fake.clear_calls();
    let again =
        run_gmail(&env, &fake, &["sync", "-l", "0", "-r", "0", "-f", "json"]);
    assert_exit(&again, 0, "repeat sync is a no-op");
    assert!(
        json_stdout(&again)["archive"]
            .as_array()
            .expect("archive")
            .is_empty(),
        "consumed close never re-archives"
    );
    assert!(remote_write_calls(&fake).is_empty(), "no-op writes nothing");
    assert!(
        env.vault().join(&note).is_file(),
        "filing commits precede mutation and remain"
    );
}

#[test]
fn first_use_receipt_id_is_journaled_and_released_by_id() {
    let env = GmailEnv::new("bob-cli-gmail-archive-first-use-label");
    env.write_journal_rules();
    write_archive_config(&env);
    write_task_settings(&env);
    let fake = FakeGog::new(&env, "gog").scenario("modify_grant");
    seed_labels(&fake, false);
    seed_thread_and_messages(
        &fake,
        vec![thread_message(MESSAGE_A, &["INBOX", ENROLLMENT_ID])],
    );
    file_adopt(&env, &fake);
    close_inbox_task(&env);

    let archived =
        run_gmail(&env, &fake, &["sync", "-l", "0", "-r", "0", "-f", "json"]);
    assert_exit(&archived, 0, "first-use archive creates its receipt label");
    assert_eq!(json_stdout(&archived)["archive"][0]["action"], "archived");
    assert_eq!(
        fake.message_labels(MESSAGE_A),
        vec!["Label_1", ENROLLMENT_ID]
    );
    let journal = journal_text(&env);
    assert!(journal.contains("gmail.archived"));
    assert!(journal.contains("\"id\":\"Label_1\""));

    seed_thread_and_messages(
        &fake,
        vec![
            thread_message(MESSAGE_A, &["Label_1", ENROLLMENT_ID]),
            thread_message(MESSAGE_B, &["INBOX"]),
        ],
    );
    fake.clear_calls();
    let returned =
        run_gmail(&env, &fake, &["sync", "-l", "0", "-r", "0", "-f", "json"]);
    assert_exit(&returned, 0, "reply lets go of the created receipt by ID");
    assert_eq!(json_stdout(&returned)["archive"][0]["action"], "let_go");
    assert_eq!(fake.message_labels(MESSAGE_A), vec![ENROLLMENT_ID]);
    assert_eq!(fake.message_labels(MESSAGE_B), vec!["INBOX"]);
    assert!(journal_text(&env).contains("gmail.let_go"));
}

#[test]
fn archive_label_rename_rebaselines_and_keeps_old_receipt_identity() {
    let env = GmailEnv::new("bob-cli-gmail-archive-label-rename");
    env.write_journal_rules();
    write_archive_config(&env);
    write_task_settings(&env);
    let fake = FakeGog::new(&env, "gog").scenario("modify_grant");
    seed_labels(&fake, true);
    seed_thread_and_messages(
        &fake,
        vec![thread_message(MESSAGE_A, &["INBOX", ENROLLMENT_ID])],
    );
    file_adopt(&env, &fake);
    close_inbox_task(&env);
    let archived =
        run_gmail(&env, &fake, &["sync", "-l", "0", "-r", "0", "-f", "json"]);
    assert_exit(&archived, 0, "archive under the original receipt name");

    set_inbox_task_status(&env, ' ');
    seed_thread_and_messages(
        &fake,
        vec![
            thread_message(MESSAGE_A, &[RECEIPT_ID, ENROLLMENT_ID]),
            thread_message(MESSAGE_B, &["INBOX"]),
        ],
    );
    env.write_config(&format!(
        "timezone: America/New_York\ngmail:\n  account: {ACCOUNT}\n  writer: {HOST}\n  sync:\n    archive: on\n    archive_label: bob/finished\n"
    ));
    fake.set_labels(json!([
        {"id": ENROLLMENT_ID, "name": "bob", "type": "user"},
        {"id": "INBOX", "name": "INBOX", "type": "system"},
        {"id": RECEIPT_ID, "name": "bob/closed", "type": "user"},
        {"id": "Label_11", "name": "bob/finished", "type": "user"}
    ]));

    let released =
        run_gmail(&env, &fake, &["sync", "-l", "0", "-r", "0", "-f", "json"]);
    assert_exit(
        &released,
        0,
        "renamed label rebaseline still releases old receipt",
    );
    let value = json_stdout(&released);
    assert_eq!(value["archive"][0]["action"], "let_go");
    assert_eq!(value["archive"][0]["reason"], "replied");
    assert_eq!(fake.message_labels(MESSAGE_A), vec![ENROLLMENT_ID]);
    assert_eq!(fake.message_labels(MESSAGE_B), vec!["INBOX"]);
    let journal = journal_text(&env);
    assert!(journal.contains("bob/finished"));
    assert!(journal.contains("Label_11"));
    assert!(journal.contains("gmail.let_go"));
}

#[test]
fn saved_messages_added_after_close_do_not_widen_the_retry_set() {
    let env = GmailEnv::new("bob-cli-gmail-archive-frozen-saved-ids");
    env.write_journal_rules();
    write_archive_config(&env);
    write_task_settings(&env);
    let fake = FakeGog::new(&env, "gog").scenario("modify_grant");
    seed_labels(&fake, true);
    seed_thread_and_messages(
        &fake,
        vec![thread_message(MESSAGE_A, &["INBOX", ENROLLMENT_ID])],
    );
    file_adopt(&env, &fake);
    close_inbox_task(&env);
    fake.set_transient_failure(
        "batch_modify",
        "temporary archive failure",
        1,
        1,
    );
    let first =
        run_gmail(&env, &fake, &["sync", "-l", "0", "-r", "0", "-f", "json"]);
    assert_exit(&first, 1, "first archive attempt fails transiently");
    assert!(journal_has(&env, "gmail.archive_closed"));
    assert!(!journal_has(&env, "gmail.archived"));

    seed_thread_and_messages(
        &fake,
        vec![
            thread_message(MESSAGE_A, &["INBOX", ENROLLMENT_ID]),
            thread_message(MESSAGE_B, &["INBOX", ENROLLMENT_ID]),
        ],
    );
    let saved = run_gmail(&env, &fake, &["save", THREAD_A, "-f", "json"]);
    assert_exit(&saved, 0, "save the new message after the close");
    fake.clear_calls();

    let retry =
        run_gmail(&env, &fake, &["sync", "-l", "0", "-r", "0", "-f", "json"]);
    assert_exit(
        &retry,
        0,
        "retry keeps the newly saved ID outside the close",
    );
    let value = json_stdout(&retry);
    assert_eq!(value["archive"][0]["action"], "kept");
    assert_eq!(value["archive"][0]["reason"], "unsaved");
    assert!(remote_write_calls(&fake).is_empty());
    assert!(fake
        .message_labels(MESSAGE_A)
        .contains(&"INBOX".to_string()));
    assert!(fake
        .message_labels(MESSAGE_B)
        .contains(&"INBOX".to_string()));
    assert!(journal_has(&env, "gmail.kept"));
    assert!(!journal_has(&env, "gmail.archived"));
}

#[test]
fn question_mark_linker_can_be_observed_open_and_archived_after_closing() {
    let env = GmailEnv::new("bob-cli-gmail-archive-question-status");
    env.write_journal_rules();
    write_archive_config(&env);
    write_task_settings(&env);
    let fake = FakeGog::new(&env, "gog").scenario("modify_grant");
    seed_labels(&fake, true);
    seed_thread_and_messages(
        &fake,
        vec![thread_message(MESSAGE_A, &["INBOX", ENROLLMENT_ID])],
    );
    let saved = run_gmail(&env, &fake, &["save", THREAD_A, "-f", "json"]);
    assert_exit(&saved, 0, "save the mail note");
    let saved_json = json_stdout(&saved);
    let link = saved_json["link"].as_str().unwrap();
    fs::write(
        env.vault().join("gmail_inbox.md"),
        format!("#task question status probe {link}\n\n- [?] #task question status probe {link}\n"),
    )
    .expect("write question-mark task");

    let observed =
        run_gmail(&env, &fake, &["sync", "-l", "0", "-r", "0", "-f", "json"]);
    assert_exit(&observed, 0, "observe the question-mark task as open");
    assert!(json_stdout(&observed)["archive"]
        .as_array()
        .unwrap()
        .is_empty());
    set_inbox_task_status(&env, 'x');
    let archived =
        run_gmail(&env, &fake, &["sync", "-l", "0", "-r", "0", "-f", "json"]);
    assert_exit(&archived, 0, "archive after the question-mark task closes");
    assert_eq!(json_stdout(&archived)["archive"][0]["action"], "archived");
}

#[test]
fn live_pauses_backlog_without_confirm_and_creates_no_labels() {
    let env = GmailEnv::new("bob-cli-gmail-archive-apply-backlog");
    write_archive_config(&env);
    write_task_settings(&env);
    let fake = FakeGog::new(&env, "gog").scenario("modify_grant");
    seed_labels(&fake, false);
    seed_thread_and_messages(
        &fake,
        vec![thread_message(MESSAGE_A, &["INBOX", ENROLLMENT_ID])],
    );
    let journal_dir = env.vault().join(".bob/gmail");
    fs::create_dir_all(&journal_dir).expect("journal dir");
    let label = json!({"name": "bob/closed", "id": Value::Null});
    let epoch = {
        use sha2::{Digest, Sha256};
        format!(
            "sha256:{}",
            hex::encode(Sha256::digest(
                serde_json::to_vec(&json!({
                    "purpose": "gmail.archive.epoch",
                    "account": ACCOUNT,
                    "previous_change_id": Value::Null,
                    "label": label,
                }))
                .unwrap()
            ))
        )
    };
    let change = {
        use sha2::{Digest, Sha256};
        format!(
            "sha256:{}",
            hex::encode(Sha256::digest(
                serde_json::to_vec(&json!({
                    "purpose": "gmail.archive.enablement",
                    "account": ACCOUNT,
                    "enabled": true,
                    "epoch": epoch,
                    "previous_change_id": Value::Null,
                    "label": label,
                }))
                .unwrap()
            ))
        )
    };
    fs::write(
        journal_dir.join("athena.jsonl"),
        serde_json::to_string(&json!({
            "v": 1,
            "ts": "2026-01-05T09:00:00-05:00",
            "host": "athena",
            "kind": "gmail.archive_enabled",
            "value": {
                "account": ACCOUNT,
                "enabled": true,
                "epoch": epoch,
                "change_id": change,
                "previous_change_id": Value::Null,
                "label": label,
            },
        }))
        .unwrap()
            + "\n",
    )
    .expect("seed enablement");
    // Adopt without archive observations so the first observed close is a
    // historical backlog: the initial live sync skips archive with `-n`.
    let saved = run_gmail(&env, &fake, &["save", THREAD_A, "-f", "json"]);
    assert_eq!(
        saved.status.code(),
        Some(0),
        "save the watched mail note:\n{}\n{}",
        stdout(&saved),
        stderr(&saved)
    );
    let link = json_stdout(&saved)["link"]
        .as_str()
        .expect("saved link")
        .to_string();
    let inbox = env.vault().join("gmail_inbox.md");
    fs::write(
        &inbox,
        format!("#task archive apply probe {link}\n\n- [ ] #task archive apply probe {link}\n"),
    )
    .expect("write inbox task");
    let adopted = run_gmail(
        &env,
        &fake,
        &["sync", "-l", "0", "-r", "0", "-n", "-f", "json"],
    );
    assert_eq!(
        adopted.status.code(),
        Some(0),
        "adopt without archive observations:\n{}\n{}",
        stdout(&adopted),
        stderr(&adopted)
    );
    close_inbox_task(&env);
    fake.clear_calls();
    let paused =
        run_gmail(&env, &fake, &["sync", "-l", "0", "-r", "0", "-f", "json"]);
    assert_exit(&paused, 1, "unconfirmed backlog pauses");
    let value = json_stdout(&paused);
    assert_eq!(value["archive"][0]["action"], "paused");
    assert_eq!(value["archive"][0]["reason"], "backlog_unconfirmed");
    let digest = value["archive_digest"]
        .as_str()
        .expect("digest")
        .to_string();
    assert!(
        remote_write_calls(&fake).is_empty(),
        "paused creates no labels"
    );
    assert!(
        !journal_has(&env, "gmail.archived"),
        "paused appends no terminal"
    );
    let confirmed = run_gmail(
        &env,
        &fake,
        &["sync", "-l", "0", "-r", "0", "-c", &digest, "-f", "json"],
    );
    assert_exit(&confirmed, 0, "matching digest confirms the backlog");
    assert_eq!(json_stdout(&confirmed)["archive"][0]["action"], "archived");
    assert!(
        journal_has(&env, "gmail.archived"),
        "confirmed backlog archives"
    );
}

#[test]
fn supplied_confirmation_mismatch_never_writes_even_without_a_gate() {
    let env = GmailEnv::new("bob-cli-gmail-archive-confirm-mismatch");
    env.write_journal_rules();
    write_archive_config(&env);
    write_task_settings(&env);
    let fake = FakeGog::new(&env, "gog").scenario("modify_grant");
    seed_labels(&fake, true);
    seed_thread_and_messages(
        &fake,
        vec![thread_message(MESSAGE_A, &["INBOX", ENROLLMENT_ID])],
    );
    file_adopt(&env, &fake);
    close_inbox_task(&env);
    let stale = "f".repeat(64);

    fake.clear_calls();
    let dry = run_gmail(
        &env,
        &fake,
        &[
            "sync", "-d", "-l", "0", "-r", "0", "-c", &stale, "-f", "json",
        ],
    );
    assert_exit(&dry, 1, "wrong digest pauses the dry-run");
    assert_eq!(
        json_stdout(&dry)["archive"][0]["reason"],
        "confirm_mismatch"
    );
    assert!(remote_write_calls(&fake).is_empty());

    fake.clear_calls();
    let live = run_gmail(
        &env,
        &fake,
        &["sync", "-l", "0", "-r", "0", "-c", &stale, "-f", "json"],
    );
    assert_exit(&live, 1, "wrong digest pauses live apply");
    assert_eq!(
        json_stdout(&live)["archive"][0]["reason"],
        "confirm_mismatch"
    );
    assert!(remote_write_calls(&fake).is_empty());
    assert!(!journal_has(&env, "gmail.archived"));

    set_inbox_task_status(&env, ' ');
    fake.clear_calls();
    let dry_empty = run_gmail(
        &env,
        &fake,
        &[
            "sync", "-d", "-l", "0", "-r", "0", "-c", &stale, "-f", "json",
        ],
    );
    assert_exit(&dry_empty, 1, "wrong digest mismatches an empty dry-run");
    let empty_value = json_stdout(&dry_empty);
    assert!(empty_value["archive"].as_array().unwrap().is_empty());
    assert!(empty_value.get("archive_digest").is_none());
    assert!(empty_value["warnings"]
        .as_array()
        .unwrap()
        .iter()
        .any(|row| {
            row.as_str()
                .is_some_and(|text| text.contains("does not match"))
        }));
    assert!(remote_write_calls(&fake).is_empty());

    fake.clear_calls();
    let live_empty = run_gmail(
        &env,
        &fake,
        &["sync", "-l", "0", "-r", "0", "-c", &stale, "-f", "json"],
    );
    assert_exit(&live_empty, 1, "wrong digest mismatches an empty live plan");
    let empty_value = json_stdout(&live_empty);
    assert!(empty_value["archive"].as_array().unwrap().is_empty());
    assert!(empty_value.get("archive_digest").is_none());
    assert!(remote_write_calls(&fake).is_empty());
}

#[test]
fn incomplete_filing_recovery_blocks_every_archive_write() {
    let env = GmailEnv::new("bob-cli-gmail-archive-filing-gate");
    env.write_journal_rules();
    write_archive_config(&env);
    write_task_settings(&env);
    let fake = FakeGog::new(&env, "gog").scenario("modify_grant");
    seed_labels(&fake, true);
    seed_thread_and_messages(
        &fake,
        vec![thread_message(MESSAGE_A, &["INBOX", ENROLLMENT_ID])],
    );
    file_adopt(&env, &fake);
    close_inbox_task(&env);
    make_recovery_unreadable(&env);
    fake.clear_calls();

    let output =
        run_gmail(&env, &fake, &["sync", "-l", "0", "-r", "0", "-f", "json"]);
    assert_exit(&output, 1, "incomplete sync recovery blocks archive");
    let value = json_stdout(&output);
    assert!(value["archive"].as_array().unwrap().is_empty());
    assert!(
        value["warnings"].as_array().unwrap().iter().any(|warning| {
            warning.as_str().is_some_and(|warning| {
                warning.contains("recovery evidence is incomplete")
            })
        }),
        "the archive skip is explained: {value}"
    );
    assert!(remote_write_calls(&fake).is_empty());
    assert!(!journal_has(&env, "gmail.archive_closed"));
    assert!(!journal_has(&env, "gmail.archived"));
}

#[test]
fn live_keeps_unsaved_inbox_mail_without_mutation() {
    let env = GmailEnv::new("bob-cli-gmail-archive-apply-unsaved");
    write_archive_config(&env);
    write_task_settings(&env);
    let fake = FakeGog::new(&env, "gog").scenario("modify_grant");
    seed_labels(&fake, true);
    seed_thread_and_messages(
        &fake,
        vec![thread_message(MESSAGE_A, &["INBOX", ENROLLMENT_ID])],
    );
    let (_note, _) = file_adopt(&env, &fake);
    close_inbox_task(&env);
    seed_thread_and_messages(
        &fake,
        vec![
            thread_message(MESSAGE_A, &["INBOX", ENROLLMENT_ID]),
            thread_message(MESSAGE_B, &["INBOX", ENROLLMENT_ID]),
        ],
    );
    fake.set_messages(json!({
        MESSAGE_A: {"threadId": THREAD_A, "labelIds": ["INBOX", ENROLLMENT_ID]},
        MESSAGE_B: {"threadId": THREAD_A, "labelIds": ["INBOX", ENROLLMENT_ID]},
    }));
    fake.clear_calls();
    let kept =
        run_gmail(&env, &fake, &["sync", "-l", "0", "-r", "0", "-f", "json"]);
    assert_exit(&kept, 0, "unsaved mail keeps the thread");
    let value = json_stdout(&kept);
    assert_eq!(value["archive"][0]["action"], "kept");
    assert_eq!(value["archive"][0]["reason"], "unsaved");
    assert!(remote_write_calls(&fake).is_empty(), "kept writes nothing");
    assert!(journal_has(&env, "gmail.kept"), "kept consumes the close");
    assert_eq!(
        fake.message_labels(MESSAGE_A),
        vec!["INBOX".to_string(), ENROLLMENT_ID.to_string()],
        "kept leaves Gmail untouched"
    );
}

#[test]
fn live_isolates_one_thread_failure_and_keeps_filings() {
    let env = GmailEnv::new("bob-cli-gmail-archive-apply-isolation");
    write_archive_config(&env);
    write_task_settings(&env);
    let fake = FakeGog::new(&env, "gog").scenario("modify_grant");
    seed_labels(&fake, true);
    seed_thread_and_messages(
        &fake,
        vec![thread_message(MESSAGE_A, &["INBOX", ENROLLMENT_ID])],
    );
    let (_note, _) = file_adopt(&env, &fake);
    close_inbox_task(&env);
    fake.set_failure("batch_modify", "transient: PRIVATE_SENTINEL flap", 1);
    let failed =
        run_gmail(&env, &fake, &["sync", "-l", "0", "-r", "0", "-f", "json"]);
    assert_exit(&failed, 1, "isolated failure fails the run");
    assert!(
        !journal_has(&env, "gmail.archived"),
        "failure consumes nothing"
    );
    assert!(
        fs::read_to_string(env.vault().join("gmail_inbox.md")).is_ok(),
        "successful filings remain after failure"
    );
}

#[test]
fn live_missing_modify_grant_is_setup_with_login_hint() {
    let env = GmailEnv::new("bob-cli-gmail-archive-apply-grant");
    write_archive_config(&env);
    write_task_settings(&env);
    let fake = FakeGog::new(&env, "gog");
    seed_labels(&fake, true);
    seed_thread_and_messages(
        &fake,
        vec![thread_message(MESSAGE_A, &["INBOX", ENROLLMENT_ID])],
    );
    let (_note, _) = file_adopt(&env, &fake);
    close_inbox_task(&env);
    fake.set_search_pages(json!([{
        "threads": [{"id": THREAD_A}],
        "nextPageToken": null,
        "resultSizeEstimate": 1
    }]));
    fake.clear_calls();
    let denied = run_gmail(&env, &fake, &["sync", "-r", "0", "-f", "json"]);
    assert_exit(&denied, 2, "missing modify grant is setup");
    let denied_json = json_stdout(&denied);
    assert_eq!(denied_json["archive_error"]["kind"], "modify_grant");
    assert!(
        denied_json["archive_error"]["hint"]
            .as_str()
            .unwrap_or_default()
            .contains("bob gmail login -s modify"),
        "login hint is loud:\n{denied_json}"
    );
    assert!(denied_json["results"].as_array().is_some_and(|rows| {
        rows.iter().any(|row| row["thread_id"] == THREAD_A)
    }), "the enrolled filing row survives the archive setup error: {denied_json}");
    assert!(denied_json["ok"] == false);
    assert!(
        fs::read_to_string(env.vault().join("gmail_inbox.md")).is_ok(),
        "filings remain after setup failure"
    );
}

#[test]
fn live_reconciles_after_remote_success_and_journal_failure_without_remutating()
{
    let env = GmailEnv::new("bob-cli-gmail-archive-apply-recover");
    write_archive_config(&env);
    write_task_settings(&env);
    let fake = FakeGog::new(&env, "gog").scenario("modify_grant");
    seed_labels(&fake, true);
    seed_thread_and_messages(
        &fake,
        vec![thread_message(MESSAGE_A, &["INBOX", ENROLLMENT_ID])],
    );
    let (_note, _) = file_adopt(&env, &fake);
    close_inbox_task(&env);
    fake.make_archive_journal_readonly_after_batch();
    let interrupted =
        run_gmail(&env, &fake, &["sync", "-l", "0", "-r", "0", "-f", "json"]);
    assert_exit(&interrupted, 1, "journal append fails after remote success");
    assert!(
        !journal_has(&env, "gmail.archived"),
        "the failed local append did not claim ownership"
    );
    assert_eq!(
        fake.message_labels(MESSAGE_A),
        vec![RECEIPT_ID.to_string(), ENROLLMENT_ID.to_string()],
        "the remote archive succeeded before the journal failure"
    );
    let journal = env.vault().join(".bob/gmail/test-host.jsonl");
    let mut permissions = fs::metadata(&journal)
        .expect("journal metadata")
        .permissions();
    permissions.set_readonly(false);
    fs::set_permissions(&journal, permissions).expect("restore journal writes");
    seed_thread_and_messages(
        &fake,
        vec![thread_message(MESSAGE_A, &[RECEIPT_ID, ENROLLMENT_ID])],
    );
    fake.clear_calls();
    let again =
        run_gmail(&env, &fake, &["sync", "-l", "0", "-r", "0", "-f", "json"]);
    assert_exit(&again, 0, "receipt evidence reconciles the archive");
    let value = json_stdout(&again);
    assert_eq!(value["archive"][0]["action"], "archived", "{value}");
    assert_eq!(value["archive"][0]["reason"], "recovered");
    assert!(journal_text(&env).contains("\"recovered\":true"));
    assert!(
        remote_write_calls(&fake).is_empty(),
        "recovery does not mutate"
    );
    let _ = THREAD_B;
}

#[test]
fn cancelled_linker_is_positive_archive_evidence() {
    let env = GmailEnv::new("bob-cli-gmail-archive-cancelled");
    write_archive_config(&env);
    write_task_settings(&env);
    let fake = FakeGog::new(&env, "gog").scenario("modify_grant");
    seed_labels(&fake, true);
    seed_thread_and_messages(
        &fake,
        vec![thread_message(MESSAGE_A, &["INBOX", ENROLLMENT_ID])],
    );
    file_adopt(&env, &fake);
    set_inbox_task_status(&env, '-');
    let archived =
        run_gmail(&env, &fake, &["sync", "-l", "0", "-r", "0", "-f", "json"]);
    assert_exit(&archived, 0, "cancelled task is a positive close");
    assert_eq!(json_stdout(&archived)["archive"][0]["action"], "archived");
    assert_eq!(
        fake.message_labels(MESSAGE_A),
        vec![RECEIPT_ID.to_string(), ENROLLMENT_ID.to_string()]
    );
}

#[test]
fn new_reply_lets_go_and_preserves_inbox() {
    let env = GmailEnv::new("bob-cli-gmail-archive-reply-let-go");
    write_archive_config(&env);
    write_task_settings(&env);
    let fake = FakeGog::new(&env, "gog").scenario("modify_grant");
    seed_labels(&fake, true);
    seed_thread_and_messages(
        &fake,
        vec![thread_message(MESSAGE_A, &["INBOX", ENROLLMENT_ID])],
    );
    file_adopt(&env, &fake);
    close_inbox_task(&env);
    let archived =
        run_gmail(&env, &fake, &["sync", "-l", "0", "-r", "0", "-f", "json"]);
    assert_exit(&archived, 0, "archive the closed thread");
    seed_thread_and_messages(
        &fake,
        vec![
            thread_message(MESSAGE_A, &[RECEIPT_ID, ENROLLMENT_ID]),
            thread_message(MESSAGE_B, &["INBOX"]),
        ],
    );
    fake.clear_calls();
    let returned =
        run_gmail(&env, &fake, &["sync", "-l", "0", "-r", "0", "-f", "json"]);
    assert_exit(&returned, 0, "new reply releases Bob's receipt");
    let value = json_stdout(&returned);
    assert_eq!(value["archive"][0]["action"], "let_go");
    assert_eq!(value["archive"][0]["reason"], "replied");
    assert_eq!(fake.message_labels(MESSAGE_A), vec![ENROLLMENT_ID]);
    assert_eq!(fake.message_labels(MESSAGE_B), vec!["INBOX"]);
    assert!(journal_has(&env, "gmail.let_go"));
    fake.clear_calls();
    let again =
        run_gmail(&env, &fake, &["sync", "-l", "0", "-r", "0", "-f", "json"]);
    assert_exit(&again, 0, "the released close stays consumed");
    assert!(remote_write_calls(&fake).is_empty());
}

#[test]
fn manual_move_to_inbox_lets_go_without_removing_inbox() {
    let env = GmailEnv::new("bob-cli-gmail-archive-manual-let-go");
    write_archive_config(&env);
    write_task_settings(&env);
    let fake = FakeGog::new(&env, "gog").scenario("modify_grant");
    seed_labels(&fake, true);
    seed_thread_and_messages(
        &fake,
        vec![thread_message(MESSAGE_A, &["INBOX", ENROLLMENT_ID])],
    );
    file_adopt(&env, &fake);
    close_inbox_task(&env);
    let archived =
        run_gmail(&env, &fake, &["sync", "-l", "0", "-r", "0", "-f", "json"]);
    assert_exit(&archived, 0, "archive the closed thread");
    seed_thread_and_messages(
        &fake,
        vec![thread_message(
            MESSAGE_A,
            &["INBOX", RECEIPT_ID, ENROLLMENT_ID],
        )],
    );
    fake.clear_calls();
    let returned =
        run_gmail(&env, &fake, &["sync", "-l", "0", "-r", "0", "-f", "json"]);
    assert_exit(&returned, 0, "manual return releases Bob's receipt");
    let value = json_stdout(&returned);
    assert_eq!(value["archive"][0]["action"], "let_go");
    assert_eq!(value["archive"][0]["reason"], "manual_return");
    assert_eq!(
        fake.message_labels(MESSAGE_A),
        vec!["INBOX".to_string(), ENROLLMENT_ID.to_string()]
    );
    assert!(journal_has(&env, "gmail.let_go"));
}

#[test]
fn reopening_an_archived_task_reports_a_restore_hint_without_writing() {
    let env = GmailEnv::new("bob-cli-gmail-archive-reopen-hint");
    write_archive_config(&env);
    write_task_settings(&env);
    let fake = FakeGog::new(&env, "gog").scenario("modify_grant");
    seed_labels(&fake, true);
    seed_thread_and_messages(
        &fake,
        vec![thread_message(MESSAGE_A, &["INBOX", ENROLLMENT_ID])],
    );
    file_adopt(&env, &fake);
    close_inbox_task(&env);
    let archived =
        run_gmail(&env, &fake, &["sync", "-l", "0", "-r", "0", "-f", "json"]);
    assert_exit(&archived, 0, "archive the closed thread");
    seed_thread_and_messages(
        &fake,
        vec![thread_message(MESSAGE_A, &[RECEIPT_ID, ENROLLMENT_ID])],
    );
    set_inbox_task_status(&env, ' ');
    fake.clear_calls();
    let reopened =
        run_gmail(&env, &fake, &["sync", "-l", "0", "-r", "0", "-f", "json"]);
    assert_exit(&reopened, 0, "reopened task receives a hint");
    let value = json_stdout(&reopened);
    assert_eq!(value["archive_hints"][0]["kind"], "restore_to_inbox");
    assert_eq!(value["archive_hints"][0]["thread_id"], THREAD_A);
    assert_eq!(
        value["archive_hints"][0]["message"],
        "Task reopened; archived mail stays in Gmail. Restore with bob gmail unarchive."
    );
    assert!(
        remote_write_calls(&fake).is_empty(),
        "reopen does not mutate Gmail"
    );
    let human = run_gmail(&env, &fake, &["sync", "-l", "0", "-r", "0"]);
    assert_exit(&human, 0, "human reopen hint");
    assert!(
        stdout(&human).contains(&format!("bob gmail unarchive -- {THREAD_A}"))
    );
    assert!(remote_write_calls(&fake).is_empty());
}

#[test]
fn archive_reports_and_journals_never_echo_hostile_task_text() {
    let env = GmailEnv::new("bob-cli-gmail-archive-hostile-text");
    write_archive_config(&env);
    write_task_settings(&env);
    let fake = FakeGog::new(&env, "gog").scenario("modify_grant");
    seed_labels(&fake, true);
    seed_thread_and_messages(
        &fake,
        vec![thread_message(MESSAGE_A, &["INBOX", ENROLLMENT_ID])],
    );
    file_adopt(&env, &fake);
    let inbox = env.vault().join("gmail_inbox.md");
    let text = fs::read_to_string(&inbox).expect("read task note");
    let hostile = "PRIVATE TASK TEXT \u{1b}[31m\u{1b}[2J";
    let replacement = format!("- [ ] #task {hostile}");
    fs::write(
        &inbox,
        text.replacen("- [ ] #task archive apply probe", &replacement, 1),
    )
    .expect("write hostile task text");
    close_inbox_task(&env);

    let preview = run_gmail(&env, &fake, &["sync", "-l", "0", "-r", "0", "-d"]);
    assert_exit(&preview, 0, "human archive preview");
    assert!(!stdout(&preview).contains(hostile));
    assert!(!stdout(&preview).contains('\u{1b}'));

    let archived =
        run_gmail(&env, &fake, &["sync", "-l", "0", "-r", "0", "-f", "json"]);
    assert_exit(&archived, 0, "JSON archive report");
    let report = stdout(&archived);
    assert!(!report.contains(hostile));
    assert!(!report.contains("PRIVATE TASK TEXT"));
    assert!(!journal_text(&env).contains("PRIVATE TASK TEXT"));
}

// --- Section 2 (boundary) fault-isolation tests ---

fn thread_message_in(thread_id: &str, id: &str, labels: &[&str]) -> Value {
    json!({
        "id": id,
        "threadId": thread_id,
        "labelIds": labels,
        "internalDate": "1793908800000",
        "snippet": "A metadata-only archive fixture.",
        "payload": {
            "mimeType": "text/plain",
            "headers": [
                {"name": "From", "value": "Ada <ada@example.com>"},
                {"name": "To", "value": ACCOUNT},
                {"name": "Subject", "value": "Archive apply probe"},
                {"name": "Message-ID", "value": format!("<{id}@example.test>")}
            ],
            "body": {"size": 5, "data": "SGVsbG8="}
        }
    })
}

fn seed_threads(fake: &FakeGog, threads: Vec<(&str, Vec<Value>)>) {
    let mut thread_map = serde_json::Map::new();
    let mut message_map = serde_json::Map::new();
    for (thread_id, messages) in threads {
        thread_map.insert(
            thread_id.to_string(),
            json!({"thread": {"id": thread_id, "historyId": "1", "messages": messages}}),
        );
        for message in &messages {
            let id = message["id"].as_str().expect("message id").to_string();
            let label_ids = message["labelIds"].clone();
            let tid = message["threadId"]
                .as_str()
                .unwrap_or(thread_id)
                .to_string();
            message_map
                .insert(id, json!({"threadId": tid, "labelIds": label_ids}));
        }
    }
    fake.set_threads(Value::Object(thread_map));
    fake.set_messages(Value::Object(message_map));
}

fn file_adopt_thread(
    env: &GmailEnv,
    fake: &FakeGog,
    thread_id: &str,
) -> (String, String) {
    let saved = run_gmail(env, fake, &["save", thread_id, "-f", "json"]);
    assert_eq!(
        saved.status.code(),
        Some(0),
        "save the watched mail note:\n{}\n{}",
        stdout(&saved),
        stderr(&saved)
    );
    let doc = json_stdout(&saved);
    let note = doc["note"].as_str().expect("saved note").to_string();
    let link = doc["link"].as_str().expect("saved link").to_string();
    let inbox = env.vault().join("gmail_inbox.md");
    fs::write(
        &inbox,
        format!("#task archive apply probe {link}\n\n- [ ] #task archive apply probe {link}\n"),
    )
    .expect("write inbox task");
    let adopted =
        run_gmail(env, fake, &["sync", "-l", "0", "-r", "0", "-f", "json"]);
    assert_eq!(
        adopted.status.code(),
        Some(0),
        "adopt the watched thread:\n{}\n{}",
        stdout(&adopted),
        stderr(&adopted)
    );
    (note, link)
}

#[test]
fn boundary_reopened_between_planning_and_mutation_holds_without_writing() {
    // Unique thread so the reopen seam cannot leak into parallel tests.
    const THREAD_R: &str = "cccccccccccccccc";
    const MESSAGE_R: &str = "cccccccccccccccc";
    let env = GmailEnv::new("bob-cli-gmail-archive-boundary-reopen");
    env.write_journal_rules();
    write_archive_config(&env);
    write_task_settings(&env);
    let fake = FakeGog::new(&env, "gog").scenario("modify_grant");
    seed_labels(&fake, true);
    seed_threads(
        &fake,
        vec![(
            THREAD_R,
            vec![thread_message_in(
                THREAD_R,
                MESSAGE_R,
                &["INBOX", ENROLLMENT_ID],
            )],
        )],
    );
    let (_note, _link) = file_adopt_thread(&env, &fake, THREAD_R);
    set_inbox_task_status(&env, 'x');
    fake.clear_calls();
    let inbox = env.vault().join("gmail_inbox.md");
    let reopened = fs::read_to_string(&inbox)
        .expect("read closed task")
        .replacen("- [x]", "- [ ]", 1);
    fake.write_vault_before_metadata_read(
        THREAD_R,
        1,
        &inbox,
        reopened.as_bytes(),
    );
    let held =
        run_gmail(&env, &fake, &["sync", "-l", "0", "-r", "0", "-f", "json"]);
    assert_exit(&held, 1, "reopened evidence holds the run");
    assert!(
        fs::read_to_string(&inbox)
            .expect("read reopened task")
            .contains("- [ ]"),
        "fake transport changed the real synthetic task row"
    );
    let value = json_stdout(&held);
    assert!(
        value["archive"].as_array().expect("archive").is_empty(),
        "no terminal row on hold: {value}"
    );
    assert!(
        !journal_has(&env, "gmail.archived"),
        "no archive without a plannable terminal"
    );
    assert!(
        remote_write_calls(&fake).is_empty(),
        "held evidence writes nothing"
    );
}

#[test]
fn boundary_new_unreadable_note_between_plan_and_revalidation_holds() {
    const THREAD_U: &str = "cccccccccccccccd";
    const MESSAGE_U: &str = "cccccccccccccccd";
    let env = GmailEnv::new("bob-cli-gmail-archive-boundary-incomplete-scan");
    env.write_journal_rules();
    write_archive_config(&env);
    write_task_settings(&env);
    let fake = FakeGog::new(&env, "gog").scenario("modify_grant");
    seed_labels(&fake, true);
    seed_threads(
        &fake,
        vec![(
            THREAD_U,
            vec![thread_message_in(
                THREAD_U,
                MESSAGE_U,
                &["INBOX", ENROLLMENT_ID],
            )],
        )],
    );
    file_adopt_thread(&env, &fake, THREAD_U);
    close_inbox_task(&env);
    let late_note = env.vault().join("late-unreadable-note.md");
    fake.write_vault_before_metadata_read(
        THREAD_U,
        1,
        &late_note,
        &[0xff, 0xfe, b'\n'],
    );

    let held =
        run_gmail(&env, &fake, &["sync", "-l", "0", "-r", "0", "-f", "json"]);
    assert_exit(
        &held,
        1,
        "an incomplete fresh vault scan holds archive mutation",
    );
    let value = json_stdout(&held);
    assert_eq!(
        value["ok"], false,
        "incomplete evidence fails the run: {value}"
    );
    assert!(
        value["warnings"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(Value::as_str)
            .any(|warning| warning.contains("vault scan became incomplete")),
        "the body-free warning identifies the incomplete revalidation: {value}"
    );
    assert!(value["archive"].as_array().unwrap().is_empty());
    assert!(!journal_has(&env, "gmail.archived"));
    assert!(!journal_has(&env, "gmail.kept"));
    assert!(
        remote_write_calls(&fake).is_empty(),
        "the incomplete scan issues no message-label mutation"
    );
    assert!(env.vault().join("gmail_inbox.md").is_file());
}

#[test]
fn boundary_reply_between_read_and_batch_archives_only_frozen_ids() {
    const MESSAGE_REPLY: &str = "00000000000000c2";
    let env = GmailEnv::new("bob-cli-gmail-archive-boundary-race");
    env.write_journal_rules();
    write_archive_config(&env);
    write_task_settings(&env);
    let fake = FakeGog::new(&env, "gog").scenario("modify_grant");
    seed_labels(&fake, true);
    seed_thread_and_messages(
        &fake,
        vec![thread_message(MESSAGE_A, &["INBOX", ENROLLMENT_ID])],
    );
    file_adopt(&env, &fake);
    close_inbox_task(&env);
    // A reply arrives between the last read and the batch. The thread envelope
    // the preview reads holds only the frozen ID; the messages map already
    // holds the reply without INBOX, and the race flips it to INBOX just
    // before the batch modify. The batch must touch only frozen IDs.
    fake.set_threads(json!({
        THREAD_A: {"thread": {"id": THREAD_A, "historyId": "1", "messages": [
            thread_message_in(THREAD_A, MESSAGE_A, &["INBOX", ENROLLMENT_ID]),
        ]}}
    }));
    fake.set_messages(json!({
        MESSAGE_A: {"threadId": THREAD_A, "labelIds": ["INBOX", ENROLLMENT_ID]},
        MESSAGE_REPLY: {"threadId": THREAD_A, "labelIds": [ENROLLMENT_ID]},
    }));
    fake.set_batch_race_before_modify(MESSAGE_REPLY, &["INBOX", ENROLLMENT_ID]);
    fake.clear_calls();
    // Re-arm the race after clearing the call log (clear_calls preserves state).
    fake.set_batch_race_before_modify(MESSAGE_REPLY, &["INBOX", ENROLLMENT_ID]);
    let raced =
        run_gmail(&env, &fake, &["sync", "-l", "0", "-r", "0", "-f", "json"]);
    assert_exit(&raced, 0, "race after read archives frozen only");
    let value = json_stdout(&raced);
    let rows = value["archive"].as_array().expect("archive");
    assert_eq!(rows[0]["action"], "archived");
    assert_eq!(rows[0]["messages"], json!([MESSAGE_A]));
    assert_eq!(
        fake.message_labels(MESSAGE_A),
        vec![RECEIPT_ID.to_string(), ENROLLMENT_ID.to_string()],
        "only frozen IDs change"
    );
    assert_eq!(
        fake.message_labels(MESSAGE_REPLY),
        vec!["INBOX".to_string(), ENROLLMENT_ID.to_string()],
        "the reply stays in INBOX"
    );
    assert!(journal_has(&env, "gmail.archived"));
}

#[test]
fn boundary_crash_between_let_go_write_and_journal_converges() {
    let env = GmailEnv::new("bob-cli-gmail-archive-boundary-letgo-crash");
    env.write_journal_rules();
    write_archive_config(&env);
    write_task_settings(&env);
    let fake = FakeGog::new(&env, "gog").scenario("modify_grant");
    seed_labels(&fake, true);
    seed_thread_and_messages(
        &fake,
        vec![thread_message(MESSAGE_A, &["INBOX", ENROLLMENT_ID])],
    );
    file_adopt(&env, &fake);
    close_inbox_task(&env);
    let archived =
        run_gmail(&env, &fake, &["sync", "-l", "0", "-r", "0", "-f", "json"]);
    assert_exit(&archived, 0, "archive the closed thread");
    // A reply reopens INBOX membership; the next let-go will crash after its
    // remote write but before its journal append.
    seed_thread_and_messages(
        &fake,
        vec![
            thread_message(MESSAGE_A, &[RECEIPT_ID, ENROLLMENT_ID]),
            thread_message(MESSAGE_B, &["INBOX"]),
        ],
    );
    // Messages map needs both IDs with correct threadIds for the race-free path.
    seed_threads(
        &fake,
        vec![(
            THREAD_A,
            vec![
                thread_message_in(
                    THREAD_A,
                    MESSAGE_A,
                    &[RECEIPT_ID, ENROLLMENT_ID],
                ),
                thread_message_in(THREAD_A, MESSAGE_B, &["INBOX"]),
            ],
        )],
    );
    fake.clear_calls();
    fake.make_archive_journal_readonly_after_batch();
    let crashed =
        run_gmail(&env, &fake, &["sync", "-l", "0", "-r", "0", "-f", "json"]);
    assert_exit(&crashed, 1, "let-go journal failure fails the run");
    assert!(
        !journal_has(&env, "gmail.let_go"),
        "the failed local append claims nothing"
    );
    assert_eq!(
        fake.message_labels(MESSAGE_A),
        vec![ENROLLMENT_ID.to_string()],
        "the remote let-go succeeded before the crash"
    );
    // Restore journal writes; convergence records the release without another write.
    let journal = env.vault().join(".bob/gmail/test-host.jsonl");
    let mut permissions = fs::metadata(&journal)
        .expect("journal metadata")
        .permissions();
    permissions.set_readonly(false);
    fs::set_permissions(&journal, permissions).expect("restore journal writes");
    fake.clear_calls();
    let converged =
        run_gmail(&env, &fake, &["sync", "-l", "0", "-r", "0", "-f", "json"]);
    assert_exit(&converged, 0, "crash convergence records let-go");
    assert!(journal_has(&env, "gmail.let_go"));
    assert!(
        remote_write_calls(&fake).is_empty(),
        "convergence does not remutate"
    );
}

#[test]
fn boundary_recovery_after_crash_followed_by_reply_records_once() {
    const MESSAGE_NEW: &str = "00000000000000d3";
    let env = GmailEnv::new("bob-cli-gmail-archive-boundary-recover-reply");
    env.write_journal_rules();
    write_archive_config(&env);
    write_task_settings(&env);
    let fake = FakeGog::new(&env, "gog").scenario("modify_grant");
    seed_labels(&fake, true);
    seed_thread_and_messages(
        &fake,
        vec![thread_message(MESSAGE_A, &["INBOX", ENROLLMENT_ID])],
    );
    file_adopt(&env, &fake);
    close_inbox_task(&env);
    fake.make_archive_journal_readonly_after_batch();
    let crashed =
        run_gmail(&env, &fake, &["sync", "-l", "0", "-r", "0", "-f", "json"]);
    assert_exit(&crashed, 1, "archive write succeeds then journal fails");
    assert!(!journal_has(&env, "gmail.archived"));
    let journal = env.vault().join(".bob/gmail/test-host.jsonl");
    let mut permissions = fs::metadata(&journal)
        .expect("journal metadata")
        .permissions();
    permissions.set_readonly(false);
    fs::set_permissions(&journal, permissions).expect("restore journal writes");
    // A reply (and a removed enrollment label stays) after the crash: the
    // thread now carries the receipt on the frozen ID plus a new unsaved reply.
    seed_threads(
        &fake,
        vec![(
            THREAD_A,
            vec![
                thread_message_in(
                    THREAD_A,
                    MESSAGE_A,
                    &[RECEIPT_ID, ENROLLMENT_ID],
                ),
                thread_message_in(
                    THREAD_A,
                    MESSAGE_NEW,
                    &["INBOX", ENROLLMENT_ID],
                ),
            ],
        )],
    );
    fake.clear_calls();
    let recovered =
        run_gmail(&env, &fake, &["sync", "-l", "0", "-r", "0", "-f", "json"]);
    assert_exit(&recovered, 0, "recovery records the frozen receipt once");
    let value = json_stdout(&recovered);
    assert_eq!(value["archive"][0]["action"], "archived");
    assert_eq!(value["archive"][0]["reason"], "recovered");
    assert_eq!(value["archive"][0]["messages"], json!([MESSAGE_A]));
    assert_eq!(
        fake.message_labels(MESSAGE_NEW),
        vec!["INBOX".to_string(), ENROLLMENT_ID.to_string()],
        "the reply stays in INBOX and is never touched"
    );
    assert!(
        remote_write_calls(&fake).is_empty(),
        "recovery does not remutate"
    );
}

#[test]
fn boundary_planned_kept_turning_archivable_is_held() {
    // A row planned Kept (unsaved reply visible at preview) that becomes
    // archivable before mutation must be held, never widening the batch.
    const THREAD_K: &str = "dddddddddddddddd";
    const MESSAGE_K: &str = "dddddddddddddddd";
    const MESSAGE_U: &str = "ddddddddddddddde";
    let env = GmailEnv::new("bob-cli-gmail-archive-boundary-planned-kept");
    env.write_journal_rules();
    write_archive_config(&env);
    write_task_settings(&env);
    let fake = FakeGog::new(&env, "gog").scenario("modify_grant");
    seed_labels(&fake, true);
    // Save with only the frozen ID; the unsaved reply arrives after the close
    // without a save, so it falls in U.
    seed_threads(
        &fake,
        vec![(
            THREAD_K,
            vec![thread_message_in(
                THREAD_K,
                MESSAGE_K,
                &["INBOX", ENROLLMENT_ID],
            )],
        )],
    );
    let (_note, _link) = file_adopt_thread(&env, &fake, THREAD_K);
    set_inbox_task_status(&env, 'x');
    // The reply arrives after the close (visible in Gmail, never saved).
    seed_threads(
        &fake,
        vec![(
            THREAD_K,
            vec![
                thread_message_in(
                    THREAD_K,
                    MESSAGE_K,
                    &["INBOX", ENROLLMENT_ID],
                ),
                thread_message_in(
                    THREAD_K,
                    MESSAGE_U,
                    &["INBOX", ENROLLMENT_ID],
                ),
            ],
        )],
    );
    // Preview plans Kept for the unsaved reply. The fake Gmail transport then
    // removes that reply before the boundary metadata read, so the real fresh
    // verdict turns archivable and the planned-decision gate must hold.
    fake.replace_thread_before_metadata_read(
        THREAD_K,
        2,
        json!({"thread": {
            "id": THREAD_K,
            "historyId": "2",
            "messages": [thread_message_in(
                THREAD_K,
                MESSAGE_K,
                &["INBOX", ENROLLMENT_ID],
            )],
        }}),
    );
    let held =
        run_gmail(&env, &fake, &["sync", "-l", "0", "-r", "0", "-f", "json"]);
    assert_exit(&held, 1, "upgrade after planning is held");
    let value = json_stdout(&held);
    assert!(
        value["archive"].as_array().expect("archive").is_empty(),
        "held upgrade reports no terminal row: {value}"
    );
    assert!(
        value["warnings"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(Value::as_str)
            .any(|warning| warning.contains("archivable after planning")),
        "the real remote transition reached the planned-decision gate: {value}"
    );
    assert!(remote_write_calls(&fake).is_empty());
    assert!(!journal_has(&env, "gmail.archived"));
    assert!(!journal_has(&env, "gmail.kept"));
}

fn file_adopt_two(
    env: &GmailEnv,
    fake: &FakeGog,
    thread_a: &str,
    message_a: &str,
    thread_b: &str,
    message_b: &str,
) {
    seed_threads(
        fake,
        vec![
            (
                thread_a,
                vec![thread_message_in(
                    thread_a,
                    message_a,
                    &["INBOX", ENROLLMENT_ID],
                )],
            ),
            (
                thread_b,
                vec![thread_message_in(
                    thread_b,
                    message_b,
                    &["INBOX", ENROLLMENT_ID],
                )],
            ),
        ],
    );
    for thread_id in [thread_a, thread_b] {
        let saved = run_gmail(env, fake, &["save", thread_id, "-f", "json"]);
        assert_eq!(
            saved.status.code(),
            Some(0),
            "save {thread_id}:\n{}\n{}",
            stdout(&saved),
            stderr(&saved)
        );
    }
    // Re-read links from the two saves (save twice to capture both links).
    let inbox = env.vault().join("gmail_inbox.md");
    // Collect links by re-saving? Simpler: link tasks via thread IDs directly
    // is not supported; instead write two generic tasks and rely on the mail
    // index (saved notes) plus explicit links discovered from a second save.
    // For determinism, save once more to fetch links is unnecessary: write
    // tasks that reference each thread's note via a second save round-trip.
    let mut links = Vec::new();
    for thread_id in [thread_a, thread_b] {
        let saved = run_gmail(env, fake, &["save", thread_id, "-f", "json"]);
        let doc = json_stdout(&saved);
        links.push(doc["link"].as_str().expect("link").to_string());
    }
    let body = links
        .iter()
        .map(|link| format!("- [ ] #task archive apply probe {link}"))
        .collect::<Vec<_>>()
        .join("\n");
    fs::write(&inbox, format!("#task archive apply probe\n\n{body}\n"))
        .expect("write inbox tasks");
    let adopted =
        run_gmail(env, fake, &["sync", "-l", "0", "-r", "0", "-f", "json"]);
    assert_eq!(
        adopted.status.code(),
        Some(0),
        "adopt both threads:\n{}\n{}",
        stdout(&adopted),
        stderr(&adopted)
    );
}

fn close_all_inbox_tasks(env: &GmailEnv) {
    let inbox = env.vault().join("gmail_inbox.md");
    let text = fs::read_to_string(&inbox).expect("read inbox");
    fs::write(&inbox, text.replace("- [ ]", "- [x]"))
        .expect("close inbox tasks");
}

#[test]
fn boundary_two_threads_where_one_batch_fails_isolates() {
    const THREAD_C: &str = "bbbbbbbbbbbbbbbb";
    const MESSAGE_C: &str = "bbbbbbbbbbbbbbbb";
    let env = GmailEnv::new("bob-cli-gmail-archive-boundary-two-fail");
    env.write_journal_rules();
    write_archive_config(&env);
    write_task_settings(&env);
    let fake = FakeGog::new(&env, "gog").scenario("modify_grant");
    seed_labels(&fake, true);
    file_adopt_two(&env, &fake, THREAD_A, MESSAGE_A, THREAD_C, MESSAGE_C);
    close_all_inbox_tasks(&env);
    // First batch (THREAD_A, sorted first) succeeds; second fails transiently.
    fake.set_fail_after(
        "batch_modify",
        "transient: PRIVATE_SENTINEL flap",
        1,
        1,
    );
    fake.clear_calls();
    // Re-arm after clearing the log (state is preserved).
    fake.set_fail_after(
        "batch_modify",
        "transient: PRIVATE_SENTINEL flap",
        1,
        1,
    );
    let failed =
        run_gmail(&env, &fake, &["sync", "-l", "0", "-r", "0", "-f", "json"]);
    assert_exit(&failed, 1, "one failure fails the run without rollback");
    let value = json_stdout(&failed);
    let rows = value["archive"].as_array().expect("archive");
    assert_eq!(rows.len(), 1, "only the successful thread reports: {value}");
    assert_eq!(rows[0]["thread_id"], THREAD_A);
    assert_eq!(rows[0]["action"], "archived");
    assert!(journal_has(&env, "gmail.archive_closed"));
    // The successful thread's filing and archive are kept; the failed thread
    // consumes nothing.
    assert!(env.vault().join("gmail_inbox.md").is_file());
}

#[test]
fn boundary_journal_failure_keeps_prior_row_and_recovers_the_later_write() {
    const THREAD_C: &str = "bbbbbbbbbbbbbbbb";
    const MESSAGE_C: &str = "bbbbbbbbbbbbbbbb";
    let env = GmailEnv::new("bob-cli-gmail-archive-boundary-journal-partial");
    env.write_journal_rules();
    write_archive_config(&env);
    write_task_settings(&env);
    let fake = FakeGog::new(&env, "gog").scenario("modify_grant");
    seed_labels(&fake, true);
    file_adopt_two(&env, &fake, THREAD_A, MESSAGE_A, THREAD_C, MESSAGE_C);
    close_all_inbox_tasks(&env);
    fake.clear_calls();
    fake.make_archive_journal_readonly_after_batch_number(2);

    let failed =
        run_gmail(&env, &fake, &["sync", "-l", "0", "-r", "0", "-f", "json"]);
    assert_exit(&failed, 1, "second journal append failure fails the run");
    let value = json_stdout(&failed);
    let rows = value["archive"].as_array().expect("partial rows");
    assert_eq!(rows.len(), 1, "earlier journaled archive remains: {value}");
    assert_eq!(rows[0]["thread_id"], THREAD_A);
    assert_eq!(value["summary"]["archived"], 1);
    assert_eq!(value["archive_error"]["kind"], "journal");
    assert_eq!(
        journal_text(&env).matches("gmail.archived").count(),
        1,
        "the unjournaled second write is not reported as a terminal event"
    );
    assert_eq!(fake.message_labels(MESSAGE_A).len(), 2);
    assert_eq!(fake.message_labels(MESSAGE_C).len(), 2);

    // The remote success without its append is reconciled on the next run.
    for entry in fs::read_dir(env.vault().join(".bob/gmail"))
        .expect("list archive journal")
        .flatten()
    {
        let mut permissions = fs::metadata(entry.path())
            .expect("journal metadata")
            .permissions();
        permissions.set_readonly(false);
        fs::set_permissions(entry.path(), permissions)
            .expect("restore journal write access");
    }
    fake.clear_calls();
    let recovered =
        run_gmail(&env, &fake, &["sync", "-l", "0", "-r", "0", "-f", "json"]);
    assert_exit(&recovered, 0, "later run reconciles remote archive success");
    let recovered_value = json_stdout(&recovered);
    assert_eq!(recovered_value["archive"].as_array().unwrap().len(), 1);
    assert_eq!(recovered_value["archive"][0]["thread_id"], THREAD_C);
    assert_eq!(recovered_value["archive"][0]["reason"], "recovered");
    assert!(remote_write_calls(&fake).is_empty());
}

#[test]
fn boundary_fresh_close_unresolvable_preview_fails_without_writes() {
    const WRONG_THREAD: &str = "eeeeeeeeeeeeeeee";
    let env =
        GmailEnv::new("bob-cli-gmail-archive-boundary-unresolvable-fresh");
    env.write_journal_rules();
    write_archive_config(&env);
    write_task_settings(&env);
    let fake = FakeGog::new(&env, "gog").scenario("modify_grant");
    seed_labels(&fake, true);
    seed_thread_and_messages(
        &fake,
        vec![thread_message(MESSAGE_A, &["INBOX", ENROLLMENT_ID])],
    );
    file_adopt(&env, &fake);
    close_inbox_task(&env);
    fake.replace_thread_before_metadata_read(
        THREAD_A,
        1,
        json!({"thread": {
            "id": WRONG_THREAD,
            "historyId": "2",
            "messages": [thread_message_in(
                WRONG_THREAD,
                MESSAGE_A,
                &["INBOX", ENROLLMENT_ID],
            )],
        }}),
    );

    let held =
        run_gmail(&env, &fake, &["sync", "-l", "0", "-r", "0", "-f", "json"]);
    assert_exit(&held, 1, "an unresolvable fresh close fails the preview");
    let value = json_stdout(&held);
    assert_eq!(value["ok"], false);
    assert!(value["archive"].as_array().unwrap().is_empty());
    assert!(value["warnings"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(Value::as_str)
        .any(|warning| warning.contains("unresolvable")));
    assert!(!journal_has(&env, "gmail.archive_closed"));
    assert!(!journal_has(&env, "gmail.archived"));
    assert!(remote_write_calls(&fake).is_empty());
}

#[test]
fn boundary_unresolvable_journaled_close_fails_live_and_preview() {
    const WRONG_THREAD: &str = "eeeeeeeeeeeeeeee";
    let env =
        GmailEnv::new("bob-cli-gmail-archive-boundary-unresolvable-journaled");
    env.write_journal_rules();
    write_archive_config(&env);
    write_task_settings(&env);
    let fake = FakeGog::new(&env, "gog").scenario("modify_grant");
    seed_labels(&fake, true);
    seed_thread_and_messages(
        &fake,
        vec![thread_message(MESSAGE_A, &["INBOX", ENROLLMENT_ID])],
    );
    file_adopt(&env, &fake);
    close_inbox_task(&env);
    fake.replace_thread_before_metadata_read(
        THREAD_A,
        2,
        json!({"thread": {
            "id": WRONG_THREAD,
            "historyId": "2",
            "messages": [thread_message_in(
                WRONG_THREAD,
                MESSAGE_A,
                &["INBOX", ENROLLMENT_ID],
            )],
        }}),
    );

    let boundary =
        run_gmail(&env, &fake, &["sync", "-l", "0", "-r", "0", "-f", "json"]);
    assert_exit(
        &boundary,
        1,
        "unresolvable identity fails the live boundary",
    );
    assert!(journal_has(&env, "gmail.archive_closed"));
    assert!(!journal_has(&env, "gmail.archived"));
    assert!(remote_write_calls(&fake).is_empty());

    fake.clear_calls();
    let preview =
        run_gmail(&env, &fake, &["sync", "-l", "0", "-r", "0", "-f", "json"]);
    assert_exit(&preview, 1, "the persisted close remains failed in preview");
    let value = json_stdout(&preview);
    assert_eq!(value["ok"], false);
    assert!(value["archive"].as_array().unwrap().is_empty());
    assert!(value["warnings"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(Value::as_str)
        .any(|warning| warning.contains("unresolvable")));
    assert!(remote_write_calls(&fake).is_empty());
}

fn partial_archive_auth_fixture(prefix: &str) -> (GmailEnv, FakeGog) {
    let env = GmailEnv::new(prefix);
    env.write_journal_rules();
    write_archive_config(&env);
    write_task_settings(&env);
    let fake = FakeGog::new(&env, "gog").scenario("modify_grant");
    seed_labels(&fake, true);
    seed_thread_and_messages(
        &fake,
        vec![thread_message(MESSAGE_A, &["INBOX", ENROLLMENT_ID])],
    );
    file_adopt(&env, &fake);
    close_inbox_task(&env);
    let first =
        run_gmail(&env, &fake, &["sync", "-l", "0", "-r", "0", "-f", "json"]);
    assert_exit(&first, 0, "seed an already-owned receipt");

    seed_threads(
        &fake,
        vec![
            (
                THREAD_A,
                vec![thread_message_in(
                    THREAD_A,
                    MESSAGE_A,
                    &[RECEIPT_ID, ENROLLMENT_ID],
                )],
            ),
            (
                THREAD_B,
                vec![thread_message_in(
                    THREAD_B,
                    MESSAGE_C,
                    &["INBOX", ENROLLMENT_ID],
                )],
            ),
            (
                THREAD_D,
                vec![thread_message_in(
                    THREAD_D,
                    MESSAGE_D,
                    &["INBOX", ENROLLMENT_ID],
                )],
            ),
        ],
    );
    let mut links = Vec::new();
    for thread_id in [THREAD_B, THREAD_D] {
        let saved = run_gmail(&env, &fake, &["save", thread_id, "-f", "json"]);
        assert_eq!(saved.status.code(), Some(0), "save {thread_id}");
        links.push(
            json_stdout(&saved)["link"]
                .as_str()
                .expect("saved link")
                .to_string(),
        );
    }
    let inbox = env.vault().join("gmail_inbox.md");
    let tasks = links
        .iter()
        .map(|link| format!("- [ ] #task archive apply probe {link}"))
        .collect::<Vec<_>>()
        .join("\n");
    fs::write(&inbox, format!("#task archive apply probe\n\n{tasks}\n"))
        .expect("write two new task linkers");
    let adopted =
        run_gmail(&env, &fake, &["sync", "-l", "0", "-r", "0", "-f", "json"]);
    assert_exit(&adopted, 0, "adopt and observe the two open linkers");

    // Give the previously owned thread a reply only after baseline adoption,
    // so the abort must prevent its later let-go cleanup.
    seed_threads(
        &fake,
        vec![
            (
                THREAD_A,
                vec![
                    thread_message_in(
                        THREAD_A,
                        MESSAGE_A,
                        &[RECEIPT_ID, ENROLLMENT_ID],
                    ),
                    thread_message_in(
                        THREAD_A,
                        MESSAGE_B,
                        &["INBOX", ENROLLMENT_ID],
                    ),
                ],
            ),
            (
                THREAD_B,
                vec![thread_message_in(
                    THREAD_B,
                    MESSAGE_C,
                    &["INBOX", ENROLLMENT_ID],
                )],
            ),
            (
                THREAD_D,
                vec![thread_message_in(
                    THREAD_D,
                    MESSAGE_D,
                    &["INBOX", ENROLLMENT_ID],
                )],
            ),
        ],
    );
    close_all_inbox_tasks(&env);
    fake.clear_calls();
    (env, fake)
}

#[test]
fn boundary_invalid_grant_keeps_same_run_archive_and_stops_later_writes() {
    let (env, fake) = partial_archive_auth_fixture(
        "bob-cli-gmail-archive-boundary-invalid-grant-json",
    );
    fake.fail_metadata_read(THREAD_D, 2, "invalid_grant: expired", 1);

    let aborted =
        run_gmail(&env, &fake, &["sync", "-l", "0", "-r", "0", "-f", "json"]);
    assert_exit(
        &aborted,
        2,
        "archive authentication failure uses setup exit 2",
    );
    let value = json_stdout(&aborted);
    let rows = value["archive"].as_array().expect("partial archive rows");
    assert_eq!(rows.len(), 1, "same-run success remains visible: {value}");
    assert_eq!(rows[0]["thread_id"], THREAD_B);
    assert_eq!(rows[0]["action"], "archived");
    assert_eq!(value["summary"]["archived"], 1);
    assert_eq!(value["archive_error"]["kind"], "invalid_grant");
    assert!(value["archive_error"]["hint"]
        .as_str()
        .unwrap_or_default()
        .contains("bob gmail login -s modify"));
    assert!(journal_has(&env, "gmail.archived"));
    assert!(
        env.vault()
            .join(rows[0]["note"].as_str().unwrap())
            .is_file(),
        "the successful row still points to its durably filed note"
    );
    assert!(!journal_has(&env, "gmail.let_go"));
    assert_eq!(
        fake.message_labels(MESSAGE_C),
        vec![RECEIPT_ID.to_string(), ENROLLMENT_ID.to_string()],
        "the first thread's mutation and journal both remain"
    );
    assert_eq!(
        fake.message_labels(MESSAGE_D),
        vec!["INBOX".to_string(), ENROLLMENT_ID.to_string()],
        "the auth-failed thread is untouched"
    );
    assert_eq!(
        fake.message_labels(MESSAGE_A),
        vec![RECEIPT_ID.to_string(), ENROLLMENT_ID.to_string()],
        "the prior receipt is not released after the abort"
    );
    let writes = remote_write_calls(&fake);
    assert_eq!(writes.len(), 1, "only the earlier thread wrote: {writes:?}");
    assert!(writes[0].contains(&"gmail.batch.modify".to_string()));
    assert!(!writes[0].contains(&"gmail.labels.create".to_string()));
}

#[test]
fn boundary_invalid_grant_human_report_keeps_same_run_archive() {
    let (env, fake) = partial_archive_auth_fixture(
        "bob-cli-gmail-archive-boundary-invalid-grant-human",
    );
    fake.fail_metadata_read(THREAD_D, 2, "invalid_grant: expired", 1);

    let aborted = run_gmail(&env, &fake, &["sync", "-l", "0", "-r", "0"]);
    assert_exit(&aborted, 2, "human archive auth failure uses exit 2");
    let report = stdout(&aborted);
    assert!(
        report.contains(THREAD_B),
        "the successful row remains: {report}"
    );
    assert!(
        report.contains("archived"),
        "the success action remains: {report}"
    );
    let errors = stderr(&aborted);
    assert!(
        errors.contains("invalid_grant"),
        "the auth error is reported: {errors}"
    );
    assert!(
        errors.contains("bob gmail login -s modify"),
        "the modify login hint remains visible: {errors}"
    );
    assert_eq!(fake.message_labels(MESSAGE_C).len(), 2);
    assert!(!journal_has(&env, "gmail.let_go"));
}

#[test]
fn boundary_let_go_runs_while_backlog_gate_is_paused() {
    let env = GmailEnv::new("bob-cli-gmail-archive-boundary-paused-letgo");
    env.write_journal_rules();
    write_archive_config(&env);
    write_task_settings(&env);
    let fake = FakeGog::new(&env, "gog").scenario("modify_grant");
    seed_labels(&fake, true);
    // Run 1: archive THREAD_A to create an owned receipt.
    seed_thread_and_messages(
        &fake,
        vec![thread_message(MESSAGE_A, &["INBOX", ENROLLMENT_ID])],
    );
    file_adopt(&env, &fake);
    close_inbox_task(&env);
    let first =
        run_gmail(&env, &fake, &["sync", "-l", "0", "-r", "0", "-f", "json"]);
    assert_exit(&first, 0, "first archive succeeds");
    // THREAD_A now needs let-go via a reply; THREAD_B becomes a paused backlog
    // (adopted with -n so its first close is historical and needs -c).
    seed_threads(
        &fake,
        vec![
            (
                THREAD_A,
                vec![
                    thread_message_in(
                        THREAD_A,
                        MESSAGE_A,
                        &[RECEIPT_ID, ENROLLMENT_ID],
                    ),
                    thread_message_in(
                        THREAD_A,
                        MESSAGE_B,
                        &["INBOX", ENROLLMENT_ID],
                    ),
                ],
            ),
            (
                THREAD_B,
                vec![thread_message_in(
                    THREAD_B,
                    MESSAGE_C_BLANK,
                    &["INBOX", ENROLLMENT_ID],
                )],
            ),
        ],
    );
    // Adopt THREAD_B without archive observations so its first close is a
    // historical backlog. Use -n to skip archive (and let-go) during adoption.
    let saved_b = run_gmail(&env, &fake, &["save", THREAD_B, "-f", "json"]);
    assert_eq!(saved_b.status.code(), Some(0), "save backlog thread");
    let link_b = json_stdout(&saved_b)["link"]
        .as_str()
        .expect("link")
        .to_string();
    let inbox = env.vault().join("gmail_inbox.md");
    let mut text = fs::read_to_string(&inbox).expect("read inbox");
    text.push_str(&format!("\n- [ ] #task archive apply probe {link_b}\n"));
    fs::write(&inbox, &text).expect("add backlog task");
    let adopted_b = run_gmail(
        &env,
        &fake,
        &["sync", "-l", "0", "-r", "0", "-n", "-f", "json"],
    );
    assert_exit(&adopted_b, 0, "adopt backlog thread without archiving");
    // Close the backlog thread's task (last open row).
    let text = fs::read_to_string(&inbox).expect("read inbox");
    let mut lines: Vec<String> = text.lines().map(str::to_string).collect();
    if let Some(pos) = lines.iter().rposition(|l| l.starts_with("- [ ]")) {
        lines[pos] = lines[pos].replacen("- [ ]", "- [x]", 1);
    }
    fs::write(&inbox, lines.join("\n") + "\n").expect("close backlog task");
    fake.clear_calls();
    let paused =
        run_gmail(&env, &fake, &["sync", "-l", "0", "-r", "0", "-f", "json"]);
    assert_exit(&paused, 1, "backlog gate pauses the run");
    let value = json_stdout(&paused);
    let rows = value["archive"].as_array().expect("archive rows");
    // One paused backlog row and one let-go row.
    assert!(
        rows.iter().any(|row| row["thread_id"] == THREAD_B
            && row["action"] == "paused"
            && row["reason"] == "backlog_unconfirmed"),
        "backlog row is paused: {value}"
    );
    assert!(
        rows.iter().any(
            |row| row["thread_id"] == THREAD_A && row["action"] == "let_go"
        ),
        "let-go still runs while paused: {value}"
    );
    assert!(journal_has(&env, "gmail.let_go"), "let-go journaled");
    assert!(
        !journal_text(&env).contains(
            "\"thread_id\":\"aaaaaaaaaaaaaaaa\",\"action\":\"archived\""
        ) && !value["archive"]
            .as_array()
            .unwrap()
            .iter()
            .any(|row| row["thread_id"] == THREAD_B
                && row["action"] == "archived"),
        "paused backlog never archives"
    );
    let writes = remote_write_calls(&fake);
    assert!(
        writes
            .iter()
            .any(|call| call.contains(&"gmail.batch.modify".to_string())),
        "let-go batch ran while paused: {writes:?}"
    );
    assert!(
        !writes
            .iter()
            .any(|call| call.contains(&"gmail.labels.create".to_string())),
        "paused creates no labels: {writes:?}"
    );
}

const MESSAGE_C_BLANK: &str = "00000000000000e4";

// --- Receipt lifecycle (bob-cli-kx.3.1.2) ---
//
// The folded journal is authoritative: sync converges `gmail_archived` plus
// one banner span per owned note, clears both on let-go, retains both on
// reopen, repairs interrupted local writes without remutating Gmail, and
// never writes receipts on dry-run or skipped-archive runs.

fn note_text(env: &GmailEnv, note: &str) -> String {
    fs::read_to_string(env.vault().join(note)).expect("read mail note")
}

fn archived_stamp(text: &str) -> String {
    text.lines()
        .find_map(|line| line.strip_prefix("gmail_archived: "))
        .expect("gmail_archived field")
        .trim()
        .to_string()
}

fn expected_banner(stamp: &str) -> String {
    let date = stamp.split('T').next().unwrap_or(stamp);
    format!(
        "> \u{1f5c4}\u{fe0f} Archived by Bob on {date} \u{2014} to move it back, use Move to Inbox in Gmail."
    )
}

fn downgrade_hub_to_previous(env: &GmailEnv) -> String {
    let hub_v3 =
        fs::read_to_string(env.vault().join("mail.base")).expect("mail.base");
    assert!(hub_v3.contains("Archived by Bob"), "filing ships the view");
    // The split consumes the newline that ends the previous template, so
    // restore it: the shipped templates end with a trailing newline.
    let hub_v2 = format!(
        "{}\n",
        hub_v3
            .split("\n  - type: table\n    name: Archived by Bob")
            .next()
            .expect("previous template prefix")
    );
    fs::write(env.vault().join("mail.base"), &hub_v2).expect("downgrade hub");
    hub_v3
}

#[test]
fn receipt_lifecycle_materializes_banner_and_clears_on_let_go() {
    let env = GmailEnv::new("bob-cli-gmail-archive-receipt-lifecycle");
    env.write_journal_rules();
    write_archive_config(&env);
    write_task_settings(&env);
    let fake = FakeGog::new(&env, "gog").scenario("modify_grant");
    seed_labels(&fake, true);
    seed_thread_and_messages(
        &fake,
        vec![thread_message(MESSAGE_A, &["INBOX", ENROLLMENT_ID])],
    );
    let (note, _) = file_adopt(&env, &fake);
    // Hostile task prose must never leak into the banner (C20).
    let inbox = env.vault().join("gmail_inbox.md");
    let text = fs::read_to_string(&inbox).expect("read task note");
    fs::write(
        &inbox,
        text.replacen(
            "- [ ] #task archive apply probe",
            "- [ ] #task hostile <script>alert(1)</script>",
            1,
        ),
    )
    .expect("write hostile task text");
    close_inbox_task(&env);
    let hub_v3 = downgrade_hub_to_previous(&env);
    let archived =
        run_gmail(&env, &fake, &["sync", "-l", "0", "-r", "0", "-f", "json"]);
    assert_exit(&archived, 0, "archive the closed thread");
    let after = note_text(&env, &note);
    assert!(after.contains("gmail_archived: "), "field materializes");
    let stamp = archived_stamp(&after);
    assert!(
        chrono::DateTime::parse_from_rfc3339(&stamp).is_ok(),
        "field is a valid RFC3339 timestamp"
    );
    let banner = after
        .lines()
        .find(|line| line.contains("Archived by Bob"))
        .expect("banner line")
        .to_string();
    assert_eq!(
        banner,
        expected_banner(&stamp),
        "banner states only the date"
    );
    assert_eq!(
        after.matches("bob:gmail:receipt:begin").count(),
        1,
        "exactly one banner span"
    );
    assert_eq!(
        fs::read_to_string(env.vault().join("mail.base")).expect("hub"),
        hub_v3,
        "untouched template upgrades on the archive run"
    );
    fake.clear_calls();
    let again =
        run_gmail(&env, &fake, &["sync", "-l", "0", "-r", "0", "-f", "json"]);
    assert_exit(&again, 0, "repeat sync converges");
    assert_eq!(
        note_text(&env, &note),
        after,
        "idempotent projection writes zero bytes"
    );
    assert!(
        remote_write_calls(&fake).is_empty(),
        "converged run writes nothing remote"
    );
    // A customized hub stays byte-identical.
    let custom = hub_v3.replace("Archived by Bob", "Archived by Me");
    fs::write(env.vault().join("mail.base"), &custom).expect("customize hub");
    let customized =
        run_gmail(&env, &fake, &["sync", "-l", "0", "-r", "0", "-f", "json"]);
    assert_exit(&customized, 0, "sync with a customized hub");
    assert_eq!(
        fs::read_to_string(env.vault().join("mail.base")).expect("hub"),
        custom,
        "customized hub is untouched"
    );
    fs::write(env.vault().join("mail.base"), &hub_v3).expect("restore hub");
    // A new reply lets go and clears the projection (C9/C10).
    seed_thread_and_messages(
        &fake,
        vec![
            thread_message(MESSAGE_A, &[RECEIPT_ID, ENROLLMENT_ID]),
            thread_message(MESSAGE_B, &["INBOX"]),
        ],
    );
    fake.clear_calls();
    let returned =
        run_gmail(&env, &fake, &["sync", "-l", "0", "-r", "0", "-f", "json"]);
    assert_exit(&returned, 0, "reply releases the receipt");
    assert_eq!(json_stdout(&returned)["archive"][0]["action"], "let_go");
    let cleared = note_text(&env, &note);
    assert!(
        !cleared.contains("gmail_archived"),
        "field clears on let-go"
    );
    assert!(
        !cleared.contains("bob:gmail:receipt"),
        "banner clears on let-go"
    );
    assert!(cleared.contains("^m-"), "cards survive clearing");
}

#[test]
fn reopening_an_archived_task_retains_its_receipt_banner() {
    let env = GmailEnv::new("bob-cli-gmail-archive-reopen-retains");
    env.write_journal_rules();
    write_archive_config(&env);
    write_task_settings(&env);
    let fake = FakeGog::new(&env, "gog").scenario("modify_grant");
    seed_labels(&fake, true);
    seed_thread_and_messages(
        &fake,
        vec![thread_message(MESSAGE_A, &["INBOX", ENROLLMENT_ID])],
    );
    let (note, _) = file_adopt(&env, &fake);
    close_inbox_task(&env);
    let archived =
        run_gmail(&env, &fake, &["sync", "-l", "0", "-r", "0", "-f", "json"]);
    assert_exit(&archived, 0, "archive the closed thread");
    let after = note_text(&env, &note);
    assert!(after.contains("gmail_archived: "), "receipt materializes");
    seed_thread_and_messages(
        &fake,
        vec![thread_message(MESSAGE_A, &[RECEIPT_ID, ENROLLMENT_ID])],
    );
    set_inbox_task_status(&env, ' ');
    fake.clear_calls();
    let reopened =
        run_gmail(&env, &fake, &["sync", "-l", "0", "-r", "0", "-f", "json"]);
    assert_exit(&reopened, 0, "reopen keeps the receipt");
    assert_eq!(
        note_text(&env, &note),
        after,
        "reopen alone retains the projection"
    );
    assert!(
        remote_write_calls(&fake).is_empty(),
        "reopen does not mutate Gmail"
    );
    assert_eq!(
        json_stdout(&reopened)["archive_hints"][0]["kind"],
        "restore_to_inbox"
    );
}

#[test]
fn interrupted_note_write_repairs_without_remutating() {
    let env = GmailEnv::new("bob-cli-gmail-archive-receipt-repair");
    env.write_journal_rules();
    write_archive_config(&env);
    write_task_settings(&env);
    let fake = FakeGog::new(&env, "gog").scenario("modify_grant");
    seed_labels(&fake, true);
    seed_thread_and_messages(
        &fake,
        vec![thread_message(MESSAGE_A, &["INBOX", ENROLLMENT_ID])],
    );
    let (note, _) = file_adopt(&env, &fake);
    close_inbox_task(&env);
    let archived =
        run_gmail(&env, &fake, &["sync", "-l", "0", "-r", "0", "-f", "json"]);
    assert_exit(&archived, 0, "archive the closed thread");
    let after = note_text(&env, &note);
    assert!(after.contains("gmail_archived: "), "receipt materializes");
    let terminal_count = journal_text(&env).matches("gmail.archived").count();
    assert_eq!(terminal_count, 1, "one durable archive event");
    // Simulate journal-success/note-write-failure: strip the projection while
    // the journal keeps ownership.
    let mut stripped = String::new();
    let mut skipping = false;
    for line in after.lines() {
        if line.trim() == "<!-- bob:gmail:receipt:begin v1 -->" {
            skipping = true;
            continue;
        }
        if line.trim() == "<!-- bob:gmail:receipt:end -->" {
            skipping = false;
            continue;
        }
        if skipping || line.starts_with("gmail_archived:") {
            continue;
        }
        stripped.push_str(line);
        stripped.push('\n');
    }
    fs::write(env.vault().join(&note), &stripped).expect("strip receipt");
    assert!(!note_text(&env, &note).contains("gmail_archived"));
    fake.clear_calls();
    let healed =
        run_gmail(&env, &fake, &["sync", "-l", "0", "-r", "0", "-f", "json"]);
    assert_exit(&healed, 0, "repair run converges");
    assert_eq!(
        note_text(&env, &note),
        after,
        "projection restores byte-identical"
    );
    assert!(
        remote_write_calls(&fake).is_empty(),
        "repair never remutates Gmail"
    );
    assert_eq!(
        journal_text(&env).matches("gmail.archived").count(),
        terminal_count,
        "repair appends no terminal event"
    );
}

#[test]
fn dry_run_and_skipped_archive_write_no_receipt_bytes() {
    let env = GmailEnv::new("bob-cli-gmail-archive-receipt-dry-run");
    env.write_journal_rules();
    write_archive_config(&env);
    write_task_settings(&env);
    let fake = FakeGog::new(&env, "gog").scenario("modify_grant");
    seed_labels(&fake, true);
    seed_thread_and_messages(
        &fake,
        vec![thread_message(MESSAGE_A, &["INBOX", ENROLLMENT_ID])],
    );
    let (note, _) = file_adopt(&env, &fake);
    close_inbox_task(&env);
    downgrade_hub_to_previous(&env);
    let before = note_text(&env, &note);
    assert!(!before.contains("gmail_archived"));
    let dry = run_gmail(&env, &fake, &["sync", "-d", "-l", "0", "-r", "0"]);
    assert_exit(&dry, 0, "dry-run previews without writing");
    assert_eq!(note_text(&env, &note), before, "dry-run writes no receipts");
    assert!(
        !fs::read_to_string(env.vault().join("mail.base"))
            .expect("hub")
            .contains("Archived by Bob"),
        "dry-run upgrades no templates"
    );
    assert!(remote_write_calls(&fake).is_empty());
    let skipped = run_gmail(
        &env,
        &fake,
        &["sync", "-n", "-l", "0", "-r", "0", "-f", "json"],
    );
    assert_exit(&skipped, 0, "skipped archive run stays quiet");
    assert_eq!(
        json_stdout(&skipped)["archive_skipped"],
        serde_json::json!(true)
    );
    assert_eq!(
        note_text(&env, &note),
        before,
        "skipped archive writes no receipts"
    );
}

fn create_owned_receipt(env: &GmailEnv, fake: &FakeGog) -> String {
    env.write_journal_rules();
    write_archive_config(env);
    write_task_settings(env);
    seed_labels(fake, true);
    seed_thread_and_messages(
        fake,
        vec![thread_message(MESSAGE_A, &["INBOX", ENROLLMENT_ID])],
    );
    let (note, _) = file_adopt(env, fake);
    close_inbox_task(env);
    let archived =
        run_gmail(env, fake, &["sync", "-l", "0", "-r", "0", "-f", "json"]);
    assert_exit(&archived, 0, "create receipt for restore test");
    assert_eq!(json_stdout(&archived)["archive"][0]["action"], "archived");
    assert!(note_text(env, &note).contains("gmail_archived:"));
    note
}

fn create_two_owned_receipts(
    env: &GmailEnv,
    fake: &FakeGog,
) -> (String, String) {
    env.write_journal_rules();
    write_archive_config(env);
    write_task_settings(env);
    seed_labels(fake, true);
    seed_threads(
        fake,
        vec![
            (
                THREAD_A,
                vec![thread_message_in(
                    THREAD_A,
                    MESSAGE_A,
                    &["INBOX", ENROLLMENT_ID],
                )],
            ),
            (
                THREAD_B,
                vec![thread_message_in(
                    THREAD_B,
                    MESSAGE_C,
                    &["INBOX", ENROLLMENT_ID],
                )],
            ),
        ],
    );
    let saved_a = run_gmail(env, fake, &["save", THREAD_A, "-f", "json"]);
    assert_exit(&saved_a, 0, "save first restore thread");
    let note_a = json_stdout(&saved_a)["note"]
        .as_str()
        .expect("first note path")
        .to_string();
    let link_a = json_stdout(&saved_a)["link"]
        .as_str()
        .expect("first task link")
        .to_string();
    let saved_b = run_gmail(env, fake, &["save", THREAD_B, "-f", "json"]);
    assert_exit(&saved_b, 0, "save second restore thread");
    let note_b = json_stdout(&saved_b)["note"]
        .as_str()
        .expect("second note path")
        .to_string();
    let link_b = json_stdout(&saved_b)["link"]
        .as_str()
        .expect("second task link")
        .to_string();
    assert_ne!(note_a, note_b, "each thread has its own mail note");

    fs::write(
        env.vault().join("gmail_inbox.md"),
        format!(
            "#task archive apply probe\n\n- [ ] #task archive apply probe {link_a}\n- [ ] #task archive apply probe {link_b}\n"
        ),
    )
    .expect("write two linked tasks");
    let adopted =
        run_gmail(env, fake, &["sync", "-l", "0", "-r", "0", "-f", "json"]);
    assert_exit(&adopted, 0, "observe both open tasks");
    close_all_inbox_tasks(env);
    let archived =
        run_gmail(env, fake, &["sync", "-l", "0", "-r", "0", "-f", "json"]);
    assert_exit(&archived, 0, "archive both closed threads");
    let archived_value = json_stdout(&archived);
    let actions = archived_value["archive"]
        .as_array()
        .expect("archive results");
    assert_eq!(
        actions
            .iter()
            .filter(|row| row["action"] == "archived")
            .count(),
        2,
        "both inputs start with owned receipts"
    );
    (note_a, note_b)
}

fn markdown_snapshot(env: &GmailEnv) -> BTreeMap<String, Vec<u8>> {
    fn collect(
        root: &Path,
        directory: &Path,
        output: &mut BTreeMap<String, Vec<u8>>,
    ) {
        for entry in fs::read_dir(directory).expect("read vault directory") {
            let entry = entry.expect("read vault entry");
            let path = entry.path();
            let file_type = entry.file_type().expect("vault entry type");
            if file_type.is_symlink() {
                continue;
            }
            if file_type.is_dir() {
                collect(root, &path, output);
            } else if file_type.is_file()
                && path.extension().is_some_and(|extension| extension == "md")
            {
                let relative = path
                    .strip_prefix(root)
                    .expect("note inside vault")
                    .to_string_lossy()
                    .replace('\\', "/");
                output
                    .insert(relative, fs::read(path).expect("read note bytes"));
            }
        }
    }

    let mut snapshot = BTreeMap::new();
    collect(env.vault(), env.vault(), &mut snapshot);
    snapshot
}

fn assert_unrelated_markdown_unchanged(
    env: &GmailEnv,
    before: &BTreeMap<String, Vec<u8>>,
    targets: &[&str],
) {
    let after = markdown_snapshot(env);
    for (path, bytes) in before {
        if targets.contains(&path.as_str()) {
            continue;
        }
        assert_eq!(
            after.get(path),
            Some(bytes),
            "unrelated note changed: {path}"
        );
    }
    for path in after.keys() {
        if !targets.contains(&path.as_str()) {
            assert!(
                before.contains_key(path),
                "unexpected note appeared: {path}"
            );
        }
    }
}

fn receipt_neutral(contents: &str) -> (serde_yaml::Value, String) {
    const BEGIN: &str = "<!-- bob:gmail:receipt:begin v1 -->";
    const END: &str = "<!-- bob:gmail:receipt:end -->";
    let contents = contents.strip_prefix("---\n").expect("note frontmatter");
    let (frontmatter, body) = contents
        .split_once("\n---\n")
        .expect("frontmatter terminator");
    let mut frontmatter: serde_yaml::Value =
        serde_yaml::from_str(frontmatter).expect("parse note frontmatter");
    if let Some(mapping) = frontmatter.as_mapping_mut() {
        mapping.remove(serde_yaml::Value::String("gmail_archived".into()));
    }
    let mut body_without_receipt = String::new();
    let mut in_receipt = false;
    for line in body.split_inclusive('\n') {
        match line.trim() {
            BEGIN => {
                assert!(!in_receipt, "receipt span is not nested");
                in_receipt = true;
            }
            END => {
                assert!(in_receipt, "receipt end has a begin");
                in_receipt = false;
            }
            _ if !in_receipt => body_without_receipt.push_str(line),
            _ => {}
        }
    }
    assert!(!in_receipt, "receipt span is closed");
    (frontmatter, body_without_receipt)
}

fn assert_receipt_projection_only(before: &str, after: &str) {
    assert_eq!(
        receipt_neutral(before),
        receipt_neutral(after),
        "unarchive may change only gmail_archived and the managed banner"
    );
}

fn gmail_journal_path(env: &GmailEnv) -> PathBuf {
    fs::read_dir(env.vault().join(".bob/gmail"))
        .expect("Gmail journal directory")
        .flatten()
        .map(|entry| entry.path())
        .find(|path| {
            path.extension()
                .is_some_and(|extension| extension == "jsonl")
        })
        .expect("Gmail journal file")
}

#[test]
fn unarchive_restores_only_frozen_ids_and_clears_the_note_receipt() {
    let env = GmailEnv::new("bob-cli-gmail-unarchive-exact-ids");
    let fake = FakeGog::new(&env, "gog").scenario("modify_grant");
    let note = create_owned_receipt(&env, &fake);
    let mut messages = serde_json::Map::new();
    messages.insert(
        MESSAGE_A.to_string(),
        json!({"threadId": THREAD_A, "labelIds": [RECEIPT_ID, ENROLLMENT_ID]}),
    );
    messages.insert(
        MESSAGE_C.to_string(),
        json!({"threadId": THREAD_A, "labelIds": ["INBOX", ENROLLMENT_ID]}),
    );
    fake.set_messages(Value::Object(messages));
    fake.clear_calls();

    let stem = note
        .rsplit('/')
        .next()
        .unwrap()
        .strip_suffix(".md")
        .unwrap()
        .to_string();
    let restored = run_gmail(
        &env,
        &fake,
        &["unarchive", &note, &stem, &note, "-f", "json"],
    );
    assert_exit(&restored, 0, "explicit restore");
    let value = json_stdout(&restored);
    assert_eq!(value["schema_version"], 1);
    assert_eq!(value["ok"], true);
    assert_eq!(value["results"].as_array().unwrap().len(), 1);
    assert_eq!(value["results"][0]["action"], "restored");
    assert_eq!(value["results"][0]["message_ids"], json!([MESSAGE_A]));
    assert_eq!(value["results"][0]["label"]["name"], "bob/closed");
    assert_eq!(fake.message_labels(MESSAGE_A), vec!["INBOX", ENROLLMENT_ID]);
    assert_eq!(fake.message_labels(MESSAGE_C), vec!["INBOX", ENROLLMENT_ID]);
    assert!(journal_has(&env, "gmail.restored"));
    let note_after = note_text(&env, &note);
    assert!(!note_after.contains("gmail_archived:"));
    assert!(!note_after.contains("bob:gmail:receipt:begin"));

    let writes = remote_write_calls(&fake);
    let restore = writes
        .iter()
        .find(|call| call.iter().any(|arg| arg == "gmail.batch.modify"))
        .expect("one batch restore");
    let modify = restore.iter().position(|arg| arg == "modify").unwrap();
    assert_eq!(&restore[modify + 1..modify + 2], &[MESSAGE_A]);
    assert!(restore.contains(&"--add-label".to_string()));
    assert!(restore.contains(&"INBOX".to_string()));
    assert!(restore.contains(&RECEIPT_ID.to_string()));
    assert!(!restore.iter().any(|arg| arg.contains("thread")));

    fake.clear_calls();
    let repeated = run_gmail(&env, &fake, &["unarchive", &note, "-f", "json"]);
    assert_exit(&repeated, 0, "repeated restore is a typed no-op");
    assert_eq!(json_stdout(&repeated)["results"][0]["action"], "unchanged");
    assert_eq!(
        json_stdout(&repeated)["results"][0]["reason"],
        "no_owned_receipt"
    );
    assert!(remote_write_calls(&fake).is_empty());
}

#[test]
fn unarchive_dry_run_needs_no_writer_and_writes_nothing() {
    let env = GmailEnv::new("bob-cli-gmail-unarchive-dry-run");
    let fake = FakeGog::new(&env, "gog").scenario("modify_grant");
    let note = create_owned_receipt(&env, &fake);
    let before = note_text(&env, &note);
    env.write_config(&format!(
        "gmail:\n  account: {ACCOUNT}\n  writer: another-host\n  sync:\n    archive: on\n"
    ));
    fake.clear_calls();

    let preview =
        run_gmail(&env, &fake, &["unarchive", &note, "-d", "-f", "json"]);
    assert_exit(
        &preview,
        0,
        "read-only restore preview works away from writer",
    );
    let value = json_stdout(&preview);
    assert_eq!(value["ok"], true);
    assert_eq!(value["results"][0]["action"], "would_restore");
    assert_eq!(note_text(&env, &note), before);
    assert!(!journal_has(&env, "gmail.restored"));
    assert!(remote_write_calls(&fake).is_empty());
}

#[test]
fn unarchive_settles_an_already_restored_remote_state_without_mutating_gmail() {
    let env = GmailEnv::new("bob-cli-gmail-unarchive-crash-recovery");
    let fake = FakeGog::new(&env, "gog").scenario("modify_grant");
    let note = create_owned_receipt(&env, &fake);
    fake.set_messages(json!({
        MESSAGE_A: {"threadId": THREAD_A, "labelIds": ["INBOX", ENROLLMENT_ID]}
    }));
    fake.clear_calls();

    let restored = run_gmail(&env, &fake, &["unarchive", &note, "-f", "json"]);
    assert_exit(
        &restored,
        0,
        "journal converges after an earlier remote restore",
    );
    assert_eq!(json_stdout(&restored)["results"][0]["action"], "restored");
    assert_eq!(fake.message_labels(MESSAGE_A), vec!["INBOX", ENROLLMENT_ID]);
    assert!(journal_has(&env, "gmail.restored"));
    assert!(!note_text(&env, &note).contains("gmail_archived:"));
    assert!(remote_write_calls(&fake).is_empty());
}

#[test]
fn unarchive_refuses_missing_remote_metadata_without_releasing_the_receipt() {
    let env = GmailEnv::new("bob-cli-gmail-unarchive-missing-metadata");
    let fake = FakeGog::new(&env, "gog").scenario("modify_grant");
    let note = create_owned_receipt(&env, &fake);
    fake.set_failure("message_get", "notFound: recorded message is gone", 1);
    fake.clear_calls();

    let failed = run_gmail(&env, &fake, &["unarchive", &note, "-f", "json"]);
    assert_exit(&failed, 1, "missing remote evidence fails this input");
    let value = json_stdout(&failed);
    assert_eq!(value["ok"], false);
    assert_eq!(value["results"][0]["action"], "failed");
    assert_eq!(value["results"][0]["error"]["kind"], "not_found");
    assert!(!journal_has(&env, "gmail.restored"));
    assert!(remote_write_calls(&fake).is_empty());
    assert!(note_text(&env, &note).contains("gmail_archived:"));
}

#[test]
fn unarchive_retry_repairs_the_note_after_the_release_was_recorded() {
    let env = GmailEnv::new("bob-cli-gmail-unarchive-note-repair");
    let fake = FakeGog::new(&env, "gog").scenario("modify_grant");
    let note = create_owned_receipt(&env, &fake);
    let note_path = env.vault().join(&note);
    let held_note = env.root().join("held-note.md");
    fs::rename(&note_path, &held_note).expect("temporarily remove the note");

    let first = run_gmail(&env, &fake, &["unarchive", &note, "-f", "json"]);
    assert_exit(
        &first,
        1,
        "receipt release succeeds but note projection reports missing file",
    );
    assert_eq!(json_stdout(&first)["results"][0]["error"]["kind"], "note");
    assert!(journal_has(&env, "gmail.restored"));
    assert_eq!(fake.message_labels(MESSAGE_A), vec!["INBOX", ENROLLMENT_ID]);

    fs::rename(&held_note, &note_path).expect("restore the held note bytes");
    fake.clear_calls();
    let retry = run_gmail(&env, &fake, &["unarchive", &note, "-f", "json"]);
    assert_exit(
        &retry,
        0,
        "repeat call repairs the released note projection",
    );
    assert_eq!(json_stdout(&retry)["results"][0]["action"], "unchanged");
    assert!(!note_text(&env, &note).contains("gmail_archived:"));
    assert!(remote_write_calls(&fake).is_empty());
}

#[test]
fn unarchive_works_with_archive_disabled_and_uses_the_recorded_label_id() {
    let env = GmailEnv::new("bob-cli-gmail-unarchive-archive-disabled");
    let fake = FakeGog::new(&env, "gog").scenario("modify_grant");
    let note = create_owned_receipt(&env, &fake);
    env.write_config(&format!(
        "gmail:\n  account: {ACCOUNT}\n  writer: {HOST}\n  sync:\n    archive: off\n    archive_label: bob/finished\n"
    ));
    fake.clear_calls();

    let restored = run_gmail(&env, &fake, &["unarchive", &note, "-f", "json"]);
    assert_exit(
        &restored,
        0,
        "explicit restore works after archive is disabled",
    );
    assert_eq!(json_stdout(&restored)["results"][0]["action"], "restored");
    assert_eq!(
        json_stdout(&restored)["results"][0]["label"]["name"],
        "bob/closed"
    );
    assert_eq!(
        json_stdout(&restored)["results"][0]["label"]["id"],
        RECEIPT_ID
    );
    assert_eq!(fake.message_labels(MESSAGE_A), vec!["INBOX", ENROLLMENT_ID]);
    assert!(remote_write_calls(&fake).iter().any(|call| {
        call.contains(&RECEIPT_ID.to_string())
            && !call.contains(&"bob/finished".to_string())
    }));
}

#[test]
fn unarchive_wrong_writer_fails_before_gog_or_vault_writes() {
    let env = GmailEnv::new("bob-cli-gmail-unarchive-wrong-writer");
    let fake = FakeGog::new(&env, "gog").scenario("modify_grant");
    let note = create_owned_receipt(&env, &fake);
    let before_note = note_text(&env, &note);
    let before_journal = journal_text(&env);
    let before_markdown = markdown_snapshot(&env);
    env.write_config(&format!(
        "gmail:\n  account: {ACCOUNT}\n  writer: another-host\n  sync:\n    archive: on\n"
    ));
    fake.clear_calls();

    let refused = run_gmail(&env, &fake, &["unarchive", &note, "-f", "json"]);
    assert_exit(&refused, 2, "live restore requires its configured writer");
    let value = json_stdout(&refused);
    assert_eq!(value["error"]["kind"], "writer");
    assert!(value["error"]["hint"]
        .as_str()
        .unwrap_or_default()
        .contains("another-host"));
    assert!(fake.calls().is_empty(), "writer refusal precedes gog calls");
    assert!(remote_write_calls(&fake).is_empty());
    assert_eq!(note_text(&env, &note), before_note);
    assert_eq!(journal_text(&env), before_journal);
    assert_unrelated_markdown_unchanged(&env, &before_markdown, &[]);
}

#[test]
fn unarchive_missing_modify_scope_is_typed_setup_failure_without_writes() {
    let env = GmailEnv::new("bob-cli-gmail-unarchive-missing-modify");
    let fake = FakeGog::new(&env, "gog").scenario("modify_grant");
    let note = create_owned_receipt(&env, &fake);
    let before_note = note_text(&env, &note);
    let before_journal = journal_text(&env);
    let before_markdown = markdown_snapshot(&env);
    fake.set_accounts(json!([{
        "email": ACCOUNT,
        "client": "bob-gmail",
        "services": ["gmail"],
        "scopes": ["https://www.googleapis.com/auth/gmail.readonly"]
    }]));
    fake.clear_calls();

    let refused = run_gmail(&env, &fake, &["unarchive", &note, "-f", "json"]);
    assert_exit(&refused, 2, "missing Gmail modify scope is setup failure");
    let error = &json_stdout(&refused)["error"];
    assert_eq!(error["kind"], "modify_grant");
    assert!(error["hint"]
        .as_str()
        .unwrap_or_default()
        .contains("bob gmail login -s modify"));
    assert!(remote_write_calls(&fake).is_empty());
    assert_eq!(note_text(&env, &note), before_note);
    assert_eq!(journal_text(&env), before_journal);
    assert_unrelated_markdown_unchanged(&env, &before_markdown, &[&note]);
}

#[test]
fn unarchive_partial_metadata_failure_settles_only_the_successful_thread() {
    let env = GmailEnv::new("bob-cli-gmail-unarchive-partial-metadata");
    let fake = FakeGog::new(&env, "gog").scenario("modify_grant");
    let (note_a, note_b) = create_two_owned_receipts(&env, &fake);
    let before_a = note_text(&env, &note_a);
    let before_b = note_text(&env, &note_b);
    let before_markdown = markdown_snapshot(&env);
    fake.set_fail_after(
        "message_get",
        "notFound: synthetic recorded-message metadata failure",
        1,
        1,
    );
    fake.clear_calls();

    let output =
        run_gmail(&env, &fake, &["unarchive", &note_a, &note_b, "-f", "json"]);
    assert_exit(
        &output,
        1,
        "one failed input does not erase the other result",
    );
    let value = json_stdout(&output);
    assert_eq!(value["ok"], false);
    assert_eq!(value["results"].as_array().unwrap().len(), 2);
    assert_eq!(value["results"][0]["action"], "restored");
    assert_eq!(value["results"][1]["action"], "failed");
    assert_eq!(value["results"][1]["error"]["kind"], "not_found");
    assert_eq!(fake.message_labels(MESSAGE_A), vec!["INBOX", ENROLLMENT_ID]);
    assert_eq!(
        fake.message_labels(MESSAGE_C),
        vec![RECEIPT_ID, ENROLLMENT_ID]
    );
    assert!(journal_has(&env, "gmail.restored"));
    assert_eq!(journal_text(&env).matches("gmail.restored").count(), 1);
    assert_receipt_projection_only(&before_a, &note_text(&env, &note_a));
    assert_eq!(note_text(&env, &note_b), before_b);
    assert_unrelated_markdown_unchanged(
        &env,
        &before_markdown,
        &[&note_a, &note_b],
    );
}

#[test]
fn unarchive_retries_after_remote_success_and_journal_append_failure() {
    let env = GmailEnv::new("bob-cli-gmail-unarchive-journal-retry");
    let fake = FakeGog::new(&env, "gog").scenario("modify_grant");
    let note = create_owned_receipt(&env, &fake);
    let before_note = note_text(&env, &note);
    let before_markdown = markdown_snapshot(&env);
    fake.make_archive_journal_readonly_after_batch();
    fake.clear_calls();

    let interrupted =
        run_gmail(&env, &fake, &["unarchive", &note, "-f", "json"]);
    assert_exit(
        &interrupted,
        1,
        "journal failure follows successful Gmail restore",
    );
    let first = json_stdout(&interrupted);
    assert_eq!(first["results"][0]["action"], "failed");
    assert_eq!(first["results"][0]["error"]["kind"], "journal");
    assert_eq!(fake.message_labels(MESSAGE_A), vec!["INBOX", ENROLLMENT_ID]);
    assert!(!journal_has(&env, "gmail.restored"));
    assert_eq!(note_text(&env, &note), before_note);
    assert!(remote_write_calls(&fake)
        .iter()
        .any(|call| call.iter().any(|arg| arg == "gmail.batch.modify")));

    let journal = gmail_journal_path(&env);
    let mut permissions = fs::metadata(&journal)
        .expect("archive journal metadata")
        .permissions();
    permissions.set_readonly(false);
    fs::set_permissions(&journal, permissions)
        .expect("make the journal writable for retry");
    fake.clear_calls();
    let retry = run_gmail(&env, &fake, &["unarchive", &note, "-f", "json"]);
    assert_exit(&retry, 0, "retry settles the already-restored remote state");
    assert_eq!(json_stdout(&retry)["results"][0]["action"], "restored");
    assert_eq!(
        json_stdout(&retry)["results"][0]["reason"],
        "already_in_inbox"
    );
    assert!(
        remote_write_calls(&fake).is_empty(),
        "retry does not modify Gmail twice"
    );
    assert_eq!(journal_text(&env).matches("gmail.restored").count(), 1);
    assert_receipt_projection_only(&before_note, &note_text(&env, &note));
    assert_unrelated_markdown_unchanged(&env, &before_markdown, &[&note]);
}

#[test]
fn unarchive_detects_concurrent_release_and_sequential_reply_let_go() {
    let env = GmailEnv::new("bob-cli-gmail-unarchive-stale-ownership");
    let fake = FakeGog::new(&env, "gog").scenario("modify_grant");
    let note = create_owned_receipt(&env, &fake);
    let journal = gmail_journal_path(&env);
    let owned_journal = fs::read(&journal).expect("owned journal bytes");
    let owned_note = note_text(&env, &note);

    // Capture the exact let-go event a real sync writes after a reply, then
    // restore the owned snapshot so the fake can inject that release between
    // unarchive's journal read and its ownership recheck.
    seed_thread_and_messages(
        &fake,
        vec![
            thread_message(MESSAGE_A, &[RECEIPT_ID, ENROLLMENT_ID]),
            thread_message(MESSAGE_B, &["INBOX", ENROLLMENT_ID]),
        ],
    );
    let released =
        run_gmail(&env, &fake, &["sync", "-l", "0", "-r", "0", "-f", "json"]);
    assert_exit(&released, 0, "sync lets go after a new reply");
    assert_eq!(json_stdout(&released)["archive"][0]["action"], "let_go");
    let released_journal = fs::read(&journal).expect("released journal bytes");
    assert!(String::from_utf8_lossy(&released_journal).contains("gmail.let_go"));

    fs::write(&journal, &owned_journal)
        .expect("restore owned journal snapshot");
    fs::write(env.vault().join(&note), &owned_note)
        .expect("restore owned note projection");
    fake.set_messages(json!({
        MESSAGE_A: {"threadId": THREAD_A, "labelIds": [RECEIPT_ID, ENROLLMENT_ID]}
    }));
    let before_markdown = markdown_snapshot(&env);
    fake.clear_calls();
    fake.write_vault_before_message_metadata_read(
        MESSAGE_A,
        1,
        &journal,
        &released_journal,
    );
    let concurrent =
        run_gmail(&env, &fake, &["unarchive", &note, "-f", "json"]);
    assert_exit(&concurrent, 1, "concurrent let-go makes the receipt stale");
    let row = &json_stdout(&concurrent)["results"][0];
    assert_eq!(row["action"], "failed");
    assert_eq!(row["error"]["kind"], "stale_ownership");
    assert!(remote_write_calls(&fake).is_empty());
    assert!(!journal_has(&env, "gmail.restored"));
    assert_eq!(note_text(&env, &note), owned_note);
    assert_unrelated_markdown_unchanged(&env, &before_markdown, &[&note]);

    // The ordinary sequential reply path also leaves a harmless typed no-op
    // for a later explicit undo.
    let env = GmailEnv::new("bob-cli-gmail-unarchive-after-let-go");
    let fake = FakeGog::new(&env, "gog").scenario("modify_grant");
    let note = create_owned_receipt(&env, &fake);
    seed_thread_and_messages(
        &fake,
        vec![
            thread_message(MESSAGE_A, &[RECEIPT_ID, ENROLLMENT_ID]),
            thread_message(MESSAGE_B, &["INBOX", ENROLLMENT_ID]),
        ],
    );
    let let_go =
        run_gmail(&env, &fake, &["sync", "-l", "0", "-r", "0", "-f", "json"]);
    assert_exit(&let_go, 0, "sync releases after reply");
    let before_note = note_text(&env, &note);
    let before_markdown = markdown_snapshot(&env);
    fake.clear_calls();
    let unchanged = run_gmail(&env, &fake, &["unarchive", &note, "-f", "json"]);
    assert_exit(&unchanged, 0, "released receipt is a harmless no-op");
    assert_eq!(json_stdout(&unchanged)["results"][0]["action"], "unchanged");
    assert_eq!(
        json_stdout(&unchanged)["results"][0]["reason"],
        "no_owned_receipt"
    );
    assert!(remote_write_calls(&fake).is_empty());
    assert!(!journal_has(&env, "gmail.restored"));
    assert_eq!(note_text(&env, &note), before_note);
    assert_unrelated_markdown_unchanged(&env, &before_markdown, &[&note]);
}

#[test]
fn unarchive_invalid_grant_reports_remaining_input_without_mutating_it() {
    let env = GmailEnv::new("bob-cli-gmail-unarchive-invalid-grant");
    let fake = FakeGog::new(&env, "gog").scenario("modify_grant");
    let (note_a, note_b) = create_two_owned_receipts(&env, &fake);
    let before_a = note_text(&env, &note_a);
    let before_b = note_text(&env, &note_b);
    let before_markdown = markdown_snapshot(&env);
    fake.set_failure(
        "batch_modify",
        "invalid_grant: synthetic revoked token",
        1,
    );
    fake.clear_calls();

    let output =
        run_gmail(&env, &fake, &["unarchive", &note_a, &note_b, "-f", "json"]);
    assert_exit(&output, 2, "invalid_grant stops remaining restores");
    let value = json_stdout(&output);
    assert_eq!(value["results"].as_array().unwrap().len(), 2);
    assert_eq!(value["results"][0]["action"], "failed");
    assert_eq!(value["results"][0]["error"]["kind"], "invalid_grant");
    assert!(value["results"][0]["error"]["message"]
        .as_str()
        .unwrap_or_default()
        .contains("sign-in needs renewal"));
    assert_eq!(value["results"][1]["action"], "failed");
    assert_eq!(value["results"][1]["error"]["kind"], "aborted");
    assert_eq!(
        fake.message_labels(MESSAGE_A),
        vec![RECEIPT_ID, ENROLLMENT_ID]
    );
    assert_eq!(
        fake.message_labels(MESSAGE_C),
        vec![RECEIPT_ID, ENROLLMENT_ID]
    );
    assert!(!journal_has(&env, "gmail.restored"));
    assert_eq!(note_text(&env, &note_a), before_a);
    assert_eq!(note_text(&env, &note_b), before_b);
    assert_unrelated_markdown_unchanged(
        &env,
        &before_markdown,
        &[&note_a, &note_b],
    );
    assert_eq!(
        remote_write_calls(&fake)
            .iter()
            .filter(|call| call.iter().any(|arg| arg == "gmail.batch.modify"))
            .count(),
        1,
        "only the first restore attempted a mutation"
    );
}

#[test]
fn unarchive_policy_exclusions_do_not_mutate_spam_or_trash() {
    for policy_label in ["SPAM", "TRASH"] {
        let env =
            GmailEnv::new(&format!("bob-cli-gmail-unarchive-{policy_label}"));
        let fake = FakeGog::new(&env, "gog").scenario("modify_grant");
        let note = create_owned_receipt(&env, &fake);
        fake.set_messages(json!({
            MESSAGE_A: {
                "threadId": THREAD_A,
                "labelIds": [RECEIPT_ID, ENROLLMENT_ID, policy_label]
            }
        }));
        let before_note = note_text(&env, &note);
        let before_markdown = markdown_snapshot(&env);
        fake.clear_calls();

        let output =
            run_gmail(&env, &fake, &["unarchive", &note, "-f", "json"]);
        assert_exit(&output, 1, "policy-excluded message is not restored");
        assert_eq!(
            json_stdout(&output)["results"][0]["error"]["kind"],
            "policy_excluded"
        );
        assert_eq!(note_text(&env, &note), before_note);
        assert_eq!(
            fake.message_labels(MESSAGE_A),
            vec![RECEIPT_ID, ENROLLMENT_ID, policy_label]
        );
        assert!(remote_write_calls(&fake).is_empty());
        assert_unrelated_markdown_unchanged(&env, &before_markdown, &[&note]);
    }
}

#[test]
fn unarchive_ambiguous_stem_fails_without_mutating_notes_or_mail() {
    let env = GmailEnv::new("bob-cli-gmail-unarchive-ambiguous-stem");
    let fake = FakeGog::new(&env, "gog").scenario("modify_grant");
    let (note_a, note_b) = create_two_owned_receipts(&env, &fake);
    let shared_a = env.vault().join("mail/unarchive-a/shared.md");
    let shared_b = env.vault().join("mail/unarchive-b/shared.md");
    fs::create_dir_all(shared_a.parent().unwrap())
        .expect("first alias directory");
    fs::create_dir_all(shared_b.parent().unwrap())
        .expect("second alias directory");
    fs::rename(env.vault().join(&note_a), &shared_a).expect("move first note");
    fs::rename(env.vault().join(&note_b), &shared_b).expect("move second note");
    let before_markdown = markdown_snapshot(&env);
    fake.clear_calls();

    let output = run_gmail(&env, &fake, &["unarchive", "shared", "-f", "json"]);
    assert_exit(&output, 2, "ambiguous note stem is a typed setup error");
    assert_eq!(
        json_stdout(&output)["results"][0]["error"]["kind"],
        "ambiguous_identity"
    );
    assert!(remote_write_calls(&fake).is_empty());
    assert_unrelated_markdown_unchanged(&env, &before_markdown, &[]);
}

#[test]
fn unarchive_other_account_note_fails_and_unknown_thread_is_unchanged() {
    let env = GmailEnv::new("bob-cli-gmail-unarchive-cross-account");
    let fake = FakeGog::new(&env, "gog").scenario("modify_grant");
    let note = create_owned_receipt(&env, &fake);
    let foreign = env.vault().join("mail/foreign/other-account.md");
    fs::create_dir_all(foreign.parent().unwrap())
        .expect("foreign note directory");
    let text = note_text(&env, &note);
    let foreign_text = text.replace(
        &format!("gmail_account: {ACCOUNT}"),
        "gmail_account: other@example.com",
    );
    assert_ne!(foreign_text, text, "the copied note has a foreign account");
    fs::write(&foreign, foreign_text).expect("write foreign-account note");
    let before_markdown = markdown_snapshot(&env);
    fake.clear_calls();

    let foreign_path = "mail/foreign/other-account.md";
    let refused =
        run_gmail(&env, &fake, &["unarchive", foreign_path, "-f", "json"]);
    assert_exit(&refused, 2, "cross-account note is refused");
    assert_eq!(
        json_stdout(&refused)["results"][0]["error"]["kind"],
        "cross_account"
    );
    assert!(remote_write_calls(&fake).is_empty());
    assert_unrelated_markdown_unchanged(&env, &before_markdown, &[]);

    fake.clear_calls();
    let absent = run_gmail(&env, &fake, &["unarchive", THREAD_D, "-f", "json"]);
    assert_exit(&absent, 0, "unknown thread has no restore authority");
    assert_eq!(json_stdout(&absent)["results"][0]["action"], "unchanged");
    assert_eq!(
        json_stdout(&absent)["results"][0]["reason"],
        "no_owned_receipt"
    );
    assert!(remote_write_calls(&fake).is_empty());
    assert_unrelated_markdown_unchanged(&env, &before_markdown, &[]);
}

#[test]
fn unarchive_unsafe_journal_fails_each_input_without_mutation() {
    let env = GmailEnv::new("bob-cli-gmail-unarchive-unsafe-journal");
    let fake = FakeGog::new(&env, "gog").scenario("modify_grant");
    let note = create_owned_receipt(&env, &fake);
    let before_note = note_text(&env, &note);
    let journal = gmail_journal_path(&env);
    use std::io::Write;
    fs::OpenOptions::new()
        .append(true)
        .open(&journal)
        .expect("open journal")
        .write_all(b"not a journal event\n")
        .expect("make journal unsafe");
    let before_markdown = markdown_snapshot(&env);
    fake.clear_calls();

    let output =
        run_gmail(&env, &fake, &["unarchive", &note, THREAD_D, "-f", "json"]);
    assert_exit(&output, 1, "unsafe journal reports every input");
    let output_value = json_stdout(&output);
    let results = output_value["results"].as_array().expect("per-input rows");
    assert_eq!(results.len(), 2);
    assert!(results.iter().all(|row| {
        row["action"] == "failed" && row["error"]["kind"] == "unsafe_journal"
    }));
    assert!(remote_write_calls(&fake).is_empty());
    assert_eq!(note_text(&env, &note), before_note);
    assert_unrelated_markdown_unchanged(&env, &before_markdown, &[&note]);
}

#[test]
fn archive_restore_sync_flow_shows_historical_restored_state_without_rearchiving(
) {
    let env = GmailEnv::new("bob-cli-gmail-archive-restore-flow");
    let fake = FakeGog::new(&env, "gog").scenario("modify_grant");
    let note = create_owned_receipt(&env, &fake);
    let before_note = note_text(&env, &note);

    let archived =
        run_gmail(&env, &fake, &["sync", "-l", "0", "-r", "0", "-f", "json"]);
    assert_exit(&archived, 0, "sync exposes the archived history");
    assert_eq!(json_stdout(&archived)["recent_archive"]["count"], 1);
    assert_eq!(
        json_stdout(&archived)["recent_archive"]["items"][0]["state"],
        "archived"
    );
    let listed = run_gmail(&env, &fake, &["list", "-a", "-f", "json"]);
    assert_exit(&listed, 0, "list exposes the active receipt");
    let listed_value = json_stdout(&listed);
    let listed_thread = listed_value["threads"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["thread_id"] == THREAD_A)
        .expect("listed archived thread");
    assert_eq!(listed_thread["archive_receipts"][0]["message_count"], 1);
    assert_eq!(
        listed_thread["archive_receipts"][0]["label"]["name"],
        "bob/closed"
    );

    let before_markdown = markdown_snapshot(&env);
    let restored = run_gmail(&env, &fake, &["unarchive", &note, "-f", "json"]);
    assert_exit(&restored, 0, "explicit restore clears the active receipt");
    assert_eq!(json_stdout(&restored)["results"][0]["action"], "restored");
    assert_receipt_projection_only(&before_note, &note_text(&env, &note));
    assert!(!note_text(&env, &note).contains("gmail_archived:"));
    assert!(!note_text(&env, &note).contains("bob:gmail:receipt:begin"));
    assert_unrelated_markdown_unchanged(&env, &before_markdown, &[&note]);
    let cleared = run_gmail(&env, &fake, &["list", "-a", "-f", "json"]);
    assert_exit(&cleared, 0, "list confirms that no active receipt remains");
    let cleared_value = json_stdout(&cleared);
    let cleared_thread = cleared_value["threads"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["thread_id"] == THREAD_A)
        .expect("listed restored thread");
    assert_eq!(cleared_thread["archive_receipts"], json!([]));

    fake.clear_calls();
    let after =
        run_gmail(&env, &fake, &["sync", "-l", "0", "-r", "0", "-f", "json"]);
    assert_exit(&after, 0, "later sync reads the restored history");
    let after_value = json_stdout(&after);
    assert_eq!(after_value["recent_archive"]["count"], 1);
    assert_eq!(
        after_value["recent_archive"]["items"][0]["state"],
        "restored"
    );
    assert!(!after_value["archive"]
        .as_array()
        .unwrap()
        .iter()
        .any(|row| row["action"] == "archived"));
    assert!(
        remote_write_calls(&fake).is_empty(),
        "restored mail is not rearchived"
    );
}

#[test]
fn reopen_restore_and_reclose_archives_a_new_generation() {
    let mut env = GmailEnv::new("bob-cli-gmail-unarchive-new-generation");
    let fake = FakeGog::new(&env, "gog").scenario("modify_grant");
    let note = create_owned_receipt(&env, &fake);
    let first_sync =
        run_gmail(&env, &fake, &["sync", "-l", "0", "-r", "0", "-f", "json"]);
    assert_exit(&first_sync, 0, "read initial archive event");
    let first_event = json_stdout(&first_sync)["recent_archive"]["items"][0]
        ["archive_event_id"]
        .as_str()
        .expect("first event id")
        .to_string();
    let first_stamp = json_stdout(&first_sync)["recent_archive"]["items"][0]
        ["archived_at"]
        .as_str()
        .expect("first archive timestamp")
        .to_string();
    let first_note = note_text(&env, &note);

    seed_thread_and_messages(
        &fake,
        vec![thread_message(MESSAGE_A, &[RECEIPT_ID, ENROLLMENT_ID])],
    );
    set_inbox_task_status(&env, ' ');
    fake.clear_calls();
    let reopened =
        run_gmail(&env, &fake, &["sync", "-l", "0", "-r", "0", "-f", "json"]);
    assert_exit(&reopened, 0, "reopening reports the receipt restore hint");
    assert_eq!(
        json_stdout(&reopened)["archive_hints"][0]["thread_id"],
        THREAD_A
    );
    assert_eq!(note_text(&env, &note), first_note);
    assert!(
        remote_write_calls(&fake).is_empty(),
        "reopen never restores mail"
    );

    let before_markdown = markdown_snapshot(&env);
    fake.clear_calls();
    let restored = run_gmail(&env, &fake, &["unarchive", &note, "-f", "json"]);
    assert_exit(&restored, 0, "explicit undo releases the first generation");
    assert_eq!(json_stdout(&restored)["results"][0]["action"], "restored");
    assert_receipt_projection_only(&first_note, &note_text(&env, &note));
    assert_unrelated_markdown_unchanged(&env, &before_markdown, &[&note]);

    env.set_now("2026-11-05 12:01:00");
    set_inbox_task_status(&env, 'x');
    seed_thread_and_messages(
        &fake,
        vec![thread_message(MESSAGE_A, &["INBOX", ENROLLMENT_ID])],
    );
    fake.clear_calls();
    let rearchived =
        run_gmail(&env, &fake, &["sync", "-l", "0", "-r", "0", "-f", "json"]);
    assert_exit(
        &rearchived,
        0,
        "later close creates a fresh archive generation",
    );
    let value = json_stdout(&rearchived);
    assert_eq!(value["archive"][0]["action"], "archived");
    assert_eq!(value["recent_archive"]["count"], 1);
    assert_eq!(value["recent_archive"]["items"][0]["state"], "archived");
    let second_event = value["recent_archive"]["items"][0]["archive_event_id"]
        .as_str()
        .expect("second event id");
    assert_ne!(
        second_event, first_event,
        "new close has a distinct archive event"
    );
    let final_note = note_text(&env, &note);
    assert_ne!(archived_stamp(&final_note), first_stamp);
    assert_eq!(
        archived_stamp(&final_note),
        value["recent_archive"]["items"][0]["archived_at"]
    );
    assert_eq!(final_note.matches("bob:gmail:receipt:begin").count(), 1);
    assert!(final_note.contains("gmail_archived:"));
}

#[test]
fn quiet_archive_notifies_once_then_stays_silent_on_repeat() {
    let env = GmailEnv::new("bob-cli-gmail-archive-notify-once");
    env.write_journal_rules();
    write_archive_config(&env);
    write_task_settings(&env);
    let fake = FakeGog::new(&env, "gog").scenario("modify_grant");
    let notify = FakeNotify::new(&env, "notify");
    seed_labels(&fake, true);
    seed_thread_and_messages(
        &fake,
        vec![thread_message(MESSAGE_A, &["INBOX", ENROLLMENT_ID])],
    );
    let (_note, _) = file_adopt(&env, &fake);
    close_inbox_task(&env);
    let archived = run_gmail_with_notify(
        &env,
        &fake,
        &notify,
        &["sync", "-q", "-l", "0", "-r", "0", "-f", "json"],
    );
    assert_exit(&archived, 0, "quiet archive succeeds");
    let takes = notify.takes();
    assert_eq!(takes.len(), 1, "one notification for the new archive");
    assert!(
        takes[0].contains("bob gmail")
            && takes[0].contains(THREAD_A)
            && takes[0].contains("bob gmail unarchive"),
        "notification names the undo command and thread: {}",
        takes[0]
    );
    assert!(
        !takes[0].contains("Ada") && !takes[0].contains("Archive apply probe"),
        "notification carries no sender or subject text: {}",
        takes[0]
    );
    let again = run_gmail_with_notify(
        &env,
        &fake,
        &notify,
        &["sync", "-q", "-l", "0", "-r", "0", "-f", "json"],
    );
    assert_exit(&again, 0, "repeat sync is a no-op");
    assert_eq!(
        notify.takes().len(),
        1,
        "the consumed receipt never notifies again"
    );
}

#[test]
fn archive_notification_stays_silent_without_durable_outcomes() {
    let env = GmailEnv::new("bob-cli-gmail-archive-notify-silent");
    env.write_journal_rules();
    write_archive_config(&env);
    write_task_settings(&env);
    let fake = FakeGog::new(&env, "gog").scenario("modify_grant");
    let notify = FakeNotify::new(&env, "notify");
    seed_labels(&fake, true);
    seed_thread_and_messages(
        &fake,
        vec![thread_message(MESSAGE_A, &["INBOX", ENROLLMENT_ID])],
    );
    let (_note, _) = file_adopt(&env, &fake);
    close_inbox_task(&env);
    let preview = run_gmail_with_notify(
        &env,
        &fake,
        &notify,
        &["sync", "-d", "-l", "0", "-r", "0", "-f", "json"],
    );
    assert_exit(&preview, 0, "dry-run previews without writing");
    assert!(notify.takes().is_empty(), "a preview never notifies");
    let skipped = run_gmail_with_notify(
        &env,
        &fake,
        &notify,
        &["sync", "-n", "-l", "0", "-r", "0", "-f", "json"],
    );
    assert_exit(&skipped, 0, "skipped archive runs clean");
    assert!(
        notify.takes().is_empty(),
        "a skipped archive phase never notifies"
    );
    fake.set_failure("batch_modify", "transient: PRIVATE_SENTINEL flap", 1);
    let failed = run_gmail_with_notify(
        &env,
        &fake,
        &notify,
        &["sync", "-q", "-l", "0", "-r", "0", "-f", "json"],
    );
    assert_exit(&failed, 1, "isolated failure fails the run");
    assert!(
        !journal_has(&env, "gmail.archived"),
        "failure consumes nothing"
    );
    assert!(
        notify.takes().is_empty(),
        "a failed remote write never notifies"
    );
}

#[test]
fn archive_notification_respects_the_shared_opt_out() {
    let env = GmailEnv::new("bob-cli-gmail-archive-notify-opt-out");
    env.write_journal_rules();
    write_archive_config(&env);
    write_task_settings(&env);
    let fake = FakeGog::new(&env, "gog").scenario("modify_grant");
    let notify = FakeNotify::new(&env, "notify");
    seed_labels(&fake, true);
    seed_thread_and_messages(
        &fake,
        vec![thread_message(MESSAGE_A, &["INBOX", ENROLLMENT_ID])],
    );
    let (_note, _) = file_adopt(&env, &fake);
    close_inbox_task(&env);
    let mut command = env.command();
    fake.install(&mut command);
    notify.install(&mut command);
    command.env("BOB_VAULT_SYNC_NOTIFY", "0");
    command.arg("gmail");
    for arg in ["sync", "-q", "-l", "0", "-r", "0", "-f", "json"] {
        command.arg(arg);
    }
    let muted = command.output().expect("run bob gmail");
    assert_exit(&muted, 0, "opted-out archive succeeds");
    assert_eq!(
        json_stdout(&muted)["archive"][0]["action"],
        "archived",
        "the archive itself still lands"
    );
    assert!(
        notify.takes().is_empty(),
        "the vault-sync opt-out suppresses the gmail notification"
    );
}

#[test]
fn recovered_archive_notifies_once_without_remutating() {
    let env = GmailEnv::new("bob-cli-gmail-archive-notify-recovery");
    env.write_journal_rules();
    write_archive_config(&env);
    write_task_settings(&env);
    let fake = FakeGog::new(&env, "gog").scenario("modify_grant");
    let notify = FakeNotify::new(&env, "notify");
    seed_labels(&fake, true);
    seed_thread_and_messages(
        &fake,
        vec![thread_message(MESSAGE_A, &["INBOX", ENROLLMENT_ID])],
    );
    let (_note, _) = file_adopt(&env, &fake);
    close_inbox_task(&env);
    fake.make_archive_journal_readonly_after_batch();
    let interrupted = run_gmail_with_notify(
        &env,
        &fake,
        &notify,
        &["sync", "-q", "-l", "0", "-r", "0", "-f", "json"],
    );
    assert_exit(&interrupted, 1, "journal failure fails the run");
    assert!(
        notify.takes().is_empty(),
        "the interrupted run notifies nothing"
    );
    let journal = env.vault().join(".bob/gmail/test-host.jsonl");
    let mut permissions = fs::metadata(&journal)
        .expect("journal metadata")
        .permissions();
    permissions.set_readonly(false);
    fs::set_permissions(&journal, permissions).expect("restore journal writes");
    seed_thread_and_messages(
        &fake,
        vec![thread_message(MESSAGE_A, &[RECEIPT_ID, ENROLLMENT_ID])],
    );
    fake.clear_calls();
    let recovered = run_gmail_with_notify(
        &env,
        &fake,
        &notify,
        &["sync", "-q", "-l", "0", "-r", "0", "-f", "json"],
    );
    assert_exit(&recovered, 0, "recovery journals the archive");
    assert_eq!(
        json_stdout(&recovered)["archive"][0]["action"],
        "archived",
        "recovery reports the durable outcome"
    );
    let takes = notify.takes();
    assert_eq!(takes.len(), 1, "recovery notifies exactly once");
    assert!(
        takes[0].contains(THREAD_A) && takes[0].contains("bob gmail unarchive"),
        "recovery names the undo command and thread: {}",
        takes[0]
    );
    assert!(
        remote_write_calls(&fake).is_empty(),
        "recovery does not mutate"
    );
}
