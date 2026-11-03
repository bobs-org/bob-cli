//! Unit coverage for the attachment origin journal and resolution
//! (`docs/attachments.md`).

use std::{ffi::OsStr, fs, path::PathBuf};

use chrono::DateTime;

use crate::native::{capture_clip::plan_explicit_paths, env::TestEnvGuard};

use super::{
    journal::{self, display_for_source},
    origin::{resolve_origin, OriginError, OriginState},
};

fn test_root(label: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!(
        "bob-attachments-{label}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .as_nanos()
    ));
    fs::create_dir_all(&root).expect("create test root");
    root
}

fn write_source(root: &std::path::Path, name: &str, bytes: &[u8]) -> String {
    let path = root.join(name);
    fs::write(&path, bytes).expect("write source");
    path.display().to_string()
}

fn rfc3339(value: &str) -> chrono::DateTime<chrono::FixedOffset> {
    DateTime::parse_from_rfc3339(value).expect("parse ts")
}

fn record(
    vault: &std::path::Path,
    host: &str,
    ts: &str,
    saved: &str,
    source: &str,
    display: &str,
    sha256: &str,
    via: &str,
) {
    journal::append_event(
        vault,
        host,
        saved,
        source,
        display,
        sha256,
        via,
        rfc3339(ts),
    )
    .expect("append event");
}

fn event_line(
    host: &str,
    ts: &str,
    saved: &str,
    source: &str,
    sha256: &str,
    via: &str,
) -> String {
    journal::serialize_event(
        rfc3339(ts),
        host,
        saved,
        source,
        &format!("~/{source}"),
        sha256,
        via,
    )
}

#[test]
fn display_rewrites_only_the_writer_home_prefix() {
    let _guard =
        TestEnvGuard::set(&[("HOME", Some(OsStr::new("/home/tester")))]);
    assert_eq!(display_for_source("/home/tester/tmp/x.png"), "~/tmp/x.png");
    assert_eq!(display_for_source("/home/tester"), "~");
    assert_eq!(display_for_source("/elsewhere/x.png"), "/elsewhere/x.png");
    // A longer stem that merely starts with the home text is not home.
    assert_eq!(
        display_for_source("/home/tester2/x.png"),
        "/home/tester2/x.png"
    );
}

#[test]
fn journal_round_trip_preserves_every_field() {
    let root = test_root("round-trip");
    let vault = root.join("vault");
    fs::create_dir_all(&vault).expect("vault");
    record(
        &vault,
        "apollo",
        "2026-11-03T14:28:40-05:00",
        "img/x.png",
        "/tmp/shots/x.png",
        "~/shots/x.png",
        &"a".repeat(64),
        "capture",
    );
    let (events, warnings) =
        journal::read_journal(&vault).expect("read journal");
    assert!(warnings.is_empty(), "{warnings:?}");
    assert_eq!(events.len(), 1);
    let event = &events[0];
    assert_eq!(event.host, "apollo");
    assert_eq!(event.saved, "img/x.png");
    assert_eq!(event.source, "/tmp/shots/x.png");
    assert_eq!(event.display, "~/shots/x.png");
    assert_eq!(event.sha256, "a".repeat(64));
    assert_eq!(event.via, "capture");
    assert_eq!(event.ts, rfc3339("2026-11-03T14:28:40-05:00"));
}

#[test]
fn journal_skips_malformed_lines_with_a_bounded_warning() {
    let root = test_root("malformed");
    let vault = root.join("vault");
    let host_file = vault.join(".bob/attachments/apollo.jsonl");
    fs::create_dir_all(host_file.parent().expect("parent"))
        .expect("journal dir");
    let good = event_line(
        "apollo",
        "2026-11-03T14:28:40-05:00",
        "img/x.png",
        "s",
        &"b".repeat(64),
        "cli",
    );
    // Nine payload-bad lines past the good one: a wrong kind, empty saved,
    // empty source, empty display, short sha, non-hex sha, missing sha,
    // missing source, and missing via. The ninth trips the bounded
    // summary. Envelope-level junk (bad JSON, wrong `v`) is covered by
    // the shared `pref_journal` scan.
    let bad = [
        ("theme.hide", "img/y.png", "/y", "~/y", "b".repeat(64), true),
        ("attachment.import", "", "/z", "~/z", "c".repeat(64), true),
        (
            "attachment.import",
            "img/z.png",
            "",
            "~/z",
            "c".repeat(64),
            true,
        ),
        (
            "attachment.import",
            "img/z.png",
            "/z",
            "",
            "c".repeat(64),
            true,
        ),
        (
            "attachment.import",
            "img/z.png",
            "/z",
            "~/z",
            "abc".to_string(),
            true,
        ),
        (
            "attachment.import",
            "img/z.png",
            "/z",
            "~/z",
            "z".repeat(64),
            true,
        ),
        (
            "attachment.import",
            "img/z.png",
            "/z",
            "~/z",
            String::new(),
            true,
        ),
        (
            "attachment.import",
            "img/z.png",
            "",
            "~/z",
            "d".repeat(64),
            false,
        ),
        (
            "attachment.import",
            "img/z.png",
            "/z",
            "~/z",
            "d".repeat(64),
            false,
        ),
    ];
    let mut body = good;
    for (kind, saved, source, display, sha, with_via) in bad {
        let via = with_via.then_some(",\"via\":\"cli\"").unwrap_or("");
        body.push_str(&format!(
            "{{\"v\":1,\"ts\":\"2026-11-03T14:28:40-05:00\",\"host\":\"apollo\",\"kind\":\"{kind}\",\"saved\":\"{saved}\",\"source\":\"{source}\",\"display\":\"{display}\",\"sha256\":\"{sha}\"{via}}}\n",
        ));
    }
    fs::write(&host_file, body).expect("write journal");
    let (events, warnings) =
        journal::read_journal(&vault).expect("read journal");
    assert_eq!(events.len(), 1, "{events:?}");
    assert_eq!(events[0].saved, "img/x.png");
    assert_eq!(warnings.len(), 9, "{warnings:?}");
    assert!(
        warnings
            .last()
            .expect("summary")
            .contains("skipped 9 malformed attachment journal lines"),
        "{warnings:?}"
    );
}

#[test]
fn resolve_reports_present_when_source_matches() {
    let root = test_root("present");
    let vault = root.join("vault");
    fs::create_dir_all(&vault).expect("vault");
    let source = write_source(&root, "x.png", b"bytes");
    let digest = crate::native::capture_clip::sha256_hex(b"bytes");
    fs::create_dir_all(vault.join("img")).expect("img");
    fs::write(vault.join("img/x.png"), b"bytes").expect("vault copy");
    record(
        &vault,
        "apollo",
        "2026-11-03T14:28:40-05:00",
        "img/x.png",
        &source,
        "~/x.png",
        &digest,
        "capture",
    );
    let (report, warnings) =
        resolve_origin(&vault, "img/x.png", "apollo").expect("resolve");
    assert!(warnings.is_empty(), "{warnings:?}");
    let report = report.expect("origin");
    assert_eq!(report.source, source);
    assert_eq!(report.display, "~/x.png");
    assert_eq!(report.host, "apollo");
    assert_eq!(report.via, "capture");
    assert_eq!(report.state, OriginState::Present);
}

#[test]
fn resolve_reports_missing_changed_and_elsewhere() {
    let root = test_root("states");
    let vault = root.join("vault");
    fs::create_dir_all(vault.join("img")).expect("img");
    let gone = write_source(&root, "gone.png", b"gone");
    let changed = write_source(&root, "changed.png", b"before");
    let far = write_source(&root, "far.png", b"far");
    fs::write(vault.join("img/gone.png"), b"gone").expect("copy");
    fs::write(vault.join("img/changed.png"), b"before").expect("copy");
    fs::write(vault.join("img/far.png"), b"far").expect("copy");
    let hex = crate::native::capture_clip::sha256_hex;
    record(
        &vault,
        "apollo",
        "2026-11-03T14:28:40-05:00",
        "img/gone.png",
        &gone,
        "~/gone.png",
        &hex(b"gone"),
        "capture",
    );
    record(
        &vault,
        "apollo",
        "2026-11-03T14:29:40-05:00",
        "img/changed.png",
        &changed,
        "~/changed.png",
        &hex(b"before"),
        "cli",
    );
    record(
        &vault,
        "mac",
        "2026-11-03T14:30:40-05:00",
        "img/far.png",
        &far,
        "~/far.png",
        &hex(b"far"),
        "zoom",
    );
    fs::remove_file(&gone).expect("delete source");
    fs::write(&changed, b"after").expect("edit source");

    let (report, _) =
        resolve_origin(&vault, "img/gone.png", "apollo").expect("gone");
    assert_eq!(report.expect("origin").state, OriginState::Missing);
    let (report, _) =
        resolve_origin(&vault, "img/changed.png", "apollo").expect("changed");
    assert_eq!(report.expect("origin").state, OriginState::Changed);
    // Another host's event is never probed: the source exists, but the
    // state is elsewhere rather than present.
    let (report, _) =
        resolve_origin(&vault, "img/far.png", "apollo").expect("far");
    let report = report.expect("origin");
    assert_eq!(report.state, OriginState::Elsewhere);
    assert_eq!(report.host, "mac");
}

#[test]
fn resolve_survives_rename_through_content_hash() {
    let root = test_root("rename");
    let vault = root.join("vault");
    fs::create_dir_all(vault.join("img")).expect("img");
    let source = write_source(&root, "shot.png", b"shot");
    let digest = crate::native::capture_clip::sha256_hex(b"shot");
    // The vault copy was renamed after import; the saved path no longer
    // matches, but the bytes do.
    fs::write(vault.join("img/renamed.png"), b"shot").expect("copy");
    record(
        &vault,
        "apollo",
        "2026-11-03T14:28:40-05:00",
        "img/shot.png",
        &source,
        "~/shot.png",
        &digest,
        "capture",
    );
    let (report, _) =
        resolve_origin(&vault, "img/renamed.png", "apollo").expect("resolve");
    let report = report.expect("origin");
    assert_eq!(report.source, source);
    assert_eq!(report.state, OriginState::Present);
}

#[test]
fn resolve_falls_back_to_saved_for_an_edited_copy() {
    let root = test_root("edited");
    let vault = root.join("vault");
    fs::create_dir_all(vault.join("img")).expect("img");
    let source = write_source(&root, "edit.png", b"v1");
    let digest = crate::native::capture_clip::sha256_hex(b"v1");
    // The vault copy was edited after import; the hash no longer matches,
    // but the saved path does.
    fs::write(vault.join("img/edit.png"), b"v2-edited").expect("copy");
    record(
        &vault,
        "apollo",
        "2026-11-03T14:28:40-05:00",
        "img/edit.png",
        &source,
        "~/edit.png",
        &digest,
        "capture",
    );
    let (report, _) =
        resolve_origin(&vault, "img/edit.png", "apollo").expect("resolve");
    let report = report.expect("origin");
    assert_eq!(report.source, source);
    assert_eq!(report.state, OriginState::Present);
}

#[test]
fn resolve_prefers_this_host_newest_present_then_newest() {
    let root = test_root("pick");
    let vault = root.join("vault");
    fs::create_dir_all(vault.join("img")).expect("img");
    let old_home = write_source(&root, "old.png", b"same");
    let gone_home = write_source(&root, "vanished.png", b"same");
    let far = write_source(&root, "far2.png", b"same");
    fs::remove_file(&gone_home).expect("delete source");
    fs::write(vault.join("img/pick.png"), b"same").expect("copy");
    let digest = crate::native::capture_clip::sha256_hex(b"same");
    // Newest on this host is gone; the older present one wins over it
    // and over the newest elsewhere event.
    record(
        &vault,
        "apollo",
        "2026-11-03T14:28:40-05:00",
        "img/pick.png",
        &old_home,
        "~/old.png",
        &digest,
        "capture",
    );
    record(
        &vault,
        "apollo",
        "2026-11-03T14:30:40-05:00",
        "img/pick.png",
        &gone_home,
        "~/vanished.png",
        &digest,
        "capture",
    );
    record(
        &vault,
        "mac",
        "2026-11-03T14:31:40-05:00",
        "img/pick.png",
        &far,
        "~/far2.png",
        &digest,
        "zoom",
    );
    let (report, _) =
        resolve_origin(&vault, "img/pick.png", "apollo").expect("resolve");
    let report = report.expect("origin");
    assert_eq!(report.source, old_home);
    assert_eq!(report.state, OriginState::Present);

    // With every this-host source gone, the newest candidate wins even
    // when it lives elsewhere.
    fs::remove_file(&old_home).expect("delete older source");
    let (report, _) =
        resolve_origin(&vault, "img/pick.png", "apollo").expect("resolve");
    let report = report.expect("origin");
    assert_eq!(report.source, far);
    assert_eq!(report.state, OriginState::Elsewhere);
}

#[test]
fn resolve_returns_null_without_events_and_not_found_without_copy() {
    let root = test_root("null");
    let vault = root.join("vault");
    fs::create_dir_all(vault.join("img")).expect("img");
    fs::write(vault.join("img/lonely.png"), b"lonely").expect("copy");
    let (report, warnings) =
        resolve_origin(&vault, "img/lonely.png", "apollo").expect("resolve");
    assert!(report.is_none());
    assert!(warnings.is_empty(), "{warnings:?}");
    let error = resolve_origin(&vault, "img/ghost.png", "apollo")
        .expect_err("missing copy fails");
    assert_eq!(
        error,
        OriginError::NotFound("no such vault file: img/ghost.png".to_string())
    );
    assert_eq!(error.code(), "not_found");
}

#[test]
fn plan_explicit_paths_classifies_each_path_like_clipboard_lines() {
    let root = test_root("explicit");
    let vault = root.join("vault");
    fs::create_dir_all(&vault).expect("vault");
    let first = write_source(&root, "a.png", b"a");
    let sub = root.join("sub");
    fs::create_dir_all(&sub).expect("sub");
    let same_name = sub.join("a.png");
    fs::write(&same_name, b"different").expect("same name");
    let missing = root.join("missing.png").display().to_string();

    let plan = plan_explicit_paths(
        &vault,
        &[first.clone(), same_name.display().to_string()],
    )
    .expect("plan");
    assert_eq!(plan.output.attachments.len(), 2);
    assert_eq!(plan.output.attachments[0].saved, "img/a.png");
    assert!(!plan.output.attachments[0].reused);
    // Same sanitized name with different bytes gains a hash suffix.
    assert!(
        plan.output.attachments[1].saved.starts_with("img/a-"),
        "{}",
        plan.output.attachments[1].saved
    );
    assert!(!plan.output.attachments[1].reused);

    // Reused destinations still carry their source and hash.
    plan_explicit_paths(&vault, std::slice::from_ref(&first))
        .expect("first plans")
        .save()
        .expect("save");
    let reuse = plan_explicit_paths(&vault, std::slice::from_ref(&first))
        .expect("reuse plans");
    assert!(reuse.output.attachments[0].reused);
    assert_eq!(reuse.output.attachments[0].saved, "img/a.png");

    // Quoted paths and file:// URIs classify exactly as `%c` lines do.
    let quoted = plan_explicit_paths(&vault, &[format!("'{first}'")])
        .expect("quoted plans");
    assert_eq!(quoted.output.attachments[0].saved, "img/a.png");
    let uri = plan_explicit_paths(&vault, &[format!("file://{first}")])
        .expect("uri plans");
    assert_eq!(uri.output.attachments[0].saved, "img/a.png");

    let error =
        plan_explicit_paths(&vault, &[missing]).expect_err("missing fails");
    assert_eq!(error.code, "missing");
    let error = plan_explicit_paths(&vault, &[root.display().to_string()])
        .expect_err("directory fails");
    assert_eq!(error.code, "not_a_file");
    let error = plan_explicit_paths(&vault, &["relative/x.png".to_string()])
        .expect_err("relative fails");
    assert_eq!(error.code, "not_absolute");
    let many = vec!["/x.png".to_string(); 11];
    let error = plan_explicit_paths(&vault, &many).expect_err("too many fails");
    assert_eq!(error.code, "too_many");
}
