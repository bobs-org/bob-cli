//! Pinned past-Pomodoro segments (`x=` with `~`/`~~`, grammar 14).

use crate::support::*;
use std::fs;

const NOW: &str = "2026-11-04 09:13:00";

fn write_day(vault: &std::path::Path, body: &str) -> std::path::PathBuf {
    let day = vault.join("2026").join("20261104.md");
    write_file(&day, body);
    day
}

fn segment_vault(
    name: &str,
) -> (TempDir, std::path::PathBuf, std::path::PathBuf) {
    let temp = TempDir::new(name);
    let vault = temp.path().join("vault");
    fs::create_dir_all(vault.join("2026")).expect("create vault");
    write_file(
        &vault.join("bob.md"),
        "## Tasks\n\n- [ ] #task First [created::2026-11-01] ^t1\n- [ ] #task Second [created::2026-11-01] ^t2\n",
    );
    let day = write_day(
        &vault,
        concat!(
            "## Pomodoros\n",
            "- [x] (**0730-0800** [t:: 30m]) — PLAN\n",
            "- [x] (**0830-0900** [t:: 30m]) — BUILD\n",
            "- [ ] () — CHORE\n",
            "\t- [[bob#^t1]]\n",
            "- [ ] () — READ\n",
            "\t- [[bob#^t1]]\n",
        ),
    );
    (temp, vault, day)
}

fn run_json(
    vault: &std::path::Path,
    day: &std::path::Path,
    args: &[&str],
) -> serde_json::Value {
    let output = bob_command()
        .env("BOB_NOW", NOW)
        .env("BOB_DAY_FILE", day)
        .arg("capture")
        .arg("-b")
        .arg(vault)
        .arg("-f")
        .arg("json")
        .arg("--")
        .args(args)
        .output()
        .expect("run bob capture");
    assert_success(&output);
    serde_json::from_str::<serde_json::Value>(stdout(&output).trim())
        .expect("capture JSON")
}

fn run_dry_json(
    vault: &std::path::Path,
    day: &std::path::Path,
    args: &[&str],
) -> serde_json::Value {
    let output = bob_command()
        .env("BOB_NOW", NOW)
        .env("BOB_DAY_FILE", day)
        .arg("capture")
        .arg("-b")
        .arg(vault)
        .arg("-f")
        .arg("json")
        .arg("--dry-run")
        .arg("--")
        .args(args)
        .output()
        .expect("run bob capture dry-run");
    assert_success(&output);
    serde_json::from_str::<serde_json::Value>(stdout(&output).trim())
        .expect("capture JSON")
}

fn run_human(
    vault: &std::path::Path,
    day: &std::path::Path,
    args: &[&str],
) -> String {
    let output = bob_command()
        .env("BOB_NOW", NOW)
        .env("BOB_DAY_FILE", day)
        .env("NO_COLOR", "1")
        .arg("capture")
        .arg("-b")
        .arg(vault)
        .arg("--")
        .args(args)
        .output()
        .expect("run bob capture human");
    assert_success(&output);
    stdout(&output).trim().to_string()
}

fn run_expect_error(
    vault: &std::path::Path,
    day: &std::path::Path,
    args: &[&str],
) -> String {
    let output = bob_command()
        .env("BOB_NOW", NOW)
        .env("BOB_DAY_FILE", day)
        .arg("capture")
        .arg("-b")
        .arg(vault)
        .arg("-f")
        .arg("json")
        .arg("--")
        .args(args)
        .output()
        .expect("run bob capture");
    assert!(
        !output.status.success(),
        "expected failure: {}",
        format_output(&output)
    );
    format!("{}{}", stdout(&output), stderr(&output))
}

fn past_of(value: &serde_json::Value) -> &serde_json::Value {
    value.get("pomodoro_past").expect("pomodoro_past")
}

#[test]
fn record_tail_untyped_cut_at_now() {
    let (_t, vault, day) = segment_vault("bob-cli-seg-tail");
    let dry = run_dry_json(&vault, &day, &["x=~=!1"]);
    let value = run_json(&vault, &day, &["x=~=!1"]);
    assert_eq!(
        past_of(&dry)["window"],
        past_of(&value)["window"],
        "dry run reports the same windows as the real run"
    );
    let past = past_of(&value);
    assert_eq!(
        past["window"],
        serde_json::json!({
            "start": "0900", "end": "0915", "minutes": 15,
            "requested_minutes": 25, "cut": "now",
        }),
        "{past}"
    );
    assert_eq!(past["closed_at"], "0913");
    assert_eq!(past["anchor"], "0915");
    assert_eq!(past["segment"]["raw"], "~");
    assert_eq!(past["segment"]["kind"], "tail");
    assert_eq!(past["segment"]["edge"], "start");
    assert_eq!(past["stack"]["pin"], "start");
    let after = fs::read_to_string(&day).expect("read day");
    assert!(
        after.contains("(**0900-0915** [t:: 15m]) — CHORE"),
        "{after}"
    );
    let (_h, vh, dh) = segment_vault("bob-cli-seg-tail-human");
    let human = run_human(&vh, &dh, &["x=~=!1"]);
    assert!(human.contains("recorded"), "{human}");
    assert!(human.contains("0900-0915"), "{human}");
    assert!(human.contains("25m cut at now"), "{human}");
    assert!(human.contains("since BUILD"), "{human}");
}

#[test]
fn record_tail_typed_read() {
    let (_t, vault, day) = segment_vault("bob-cli-seg-read-tail");
    let dry = run_dry_json(&vault, &day, &["x=read2~=!1"]);
    let value = run_json(&vault, &day, &["x=read2~=!1"]);
    assert_eq!(past_of(&dry)["window"], past_of(&value)["window"]);
    let past = past_of(&value);
    assert_eq!(
        past["window"],
        serde_json::json!({"start": "0900", "end": "0910", "minutes": 10}),
        "{past}"
    );
    assert_eq!(past["closed_at"], "0910");
    assert_eq!(past["name"], "READ");
    assert_eq!(past["segment"]["kind"], "tail");
    assert_eq!(past["stack"]["pin"], "start");
    let after = fs::read_to_string(&day).expect("read day");
    assert!(
        after.contains("(**0900-0910** [t:: 10m]) — READ"),
        "{after}"
    );
}

#[test]
fn record_start_pin_trims_build() {
    let (_t, vault, day) = segment_vault("bob-cli-seg-start-trim");
    let dry = run_dry_json(&vault, &day, &["x=2~1!=!1"]);
    let value = run_json(&vault, &day, &["x=2~1!=!1"]);
    assert_eq!(past_of(&dry)["window"], past_of(&value)["window"]);
    let past = past_of(&value);
    assert_eq!(
        past["window"],
        serde_json::json!({"start": "0830", "end": "0840", "minutes": 10}),
        "{past}"
    );
    assert_eq!(past["closed_at"], "0840");
    assert_eq!(past["segment"]["raw"], "~1");
    assert_eq!(past["segment"]["kind"], "pomodoro");
    assert_eq!(past["segment"]["index"], 1);
    assert_eq!(past["segment"]["edge"], "start");
    assert_eq!(past["segment"]["name"], "BUILD");
    assert_eq!(past["stack"]["pin"], "start");
    let after = fs::read_to_string(&day).expect("read day");
    assert!(
        after.contains("(**0830-0840** [t:: 10m]) — CHORE"),
        "{after}"
    );
    assert!(
        after.contains("(**0840-0900** [t:: 20m]) — BUILD"),
        "{after}"
    );
    let (_h, vh, dh) = segment_vault("bob-cli-seg-start-human");
    let human = run_human(&vh, &dh, &["x=2~1!=!1"]);
    assert!(human.contains("0830-0840"), "{human}");
    assert!(
        human.contains("at the start of ~1 BUILD 0830-0900"),
        "{human}"
    );
}

#[test]
fn record_end_pin_trims_build() {
    let (_t, vault, day) = segment_vault("bob-cli-seg-end-trim");
    let dry = run_dry_json(&vault, &day, &["x=2~1+!=!1"]);
    let value = run_json(&vault, &day, &["x=2~1+!=!1"]);
    assert_eq!(past_of(&dry)["window"], past_of(&value)["window"]);
    let past = past_of(&value);
    assert_eq!(
        past["window"],
        serde_json::json!({"start": "0850", "end": "0900", "minutes": 10}),
        "{past}"
    );
    assert_eq!(past["closed_at"], "0900");
    assert_eq!(past["segment"]["raw"], "~1+");
    assert_eq!(past["segment"]["edge"], "end");
    assert_eq!(past["stack"]["pin"], "end");
    let after = fs::read_to_string(&day).expect("read day");
    assert!(
        after.contains("(**0850-0900** [t:: 10m]) — CHORE"),
        "{after}"
    );
    assert!(
        after.contains("(**0830-0850** [t:: 20m]) — BUILD"),
        "{after}"
    );
    let (_h, vh, dh) = segment_vault("bob-cli-seg-end-human");
    let human = run_human(&vh, &dh, &["x=2~1+!=!1"]);
    assert!(
        human.contains("at the end of ~1 BUILD 0830-0900"),
        "{human}"
    );
}

#[test]
fn record_gap_start_and_end() {
    let (_t, vault, day) = segment_vault("bob-cli-seg-gap-start");
    let dry = run_dry_json(&vault, &day, &["x=2~2=!1"]);
    let value = run_json(&vault, &day, &["x=2~2=!1"]);
    assert_eq!(past_of(&dry)["window"], past_of(&value)["window"]);
    let past = past_of(&value);
    assert_eq!(
        past["window"],
        serde_json::json!({"start": "0800", "end": "0810", "minutes": 10}),
        "{past}"
    );
    assert_eq!(past["closed_at"], "0810");
    assert_eq!(past["segment"]["kind"], "gap");
    assert_eq!(past["segment"]["index"], 2);
    assert_eq!(past["segment"]["edge"], "start");
    let after = fs::read_to_string(&day).expect("read day");
    assert!(
        after.contains("(**0800-0810** [t:: 10m]) — CHORE"),
        "{after}"
    );

    let (_t2, v2, d2) = segment_vault("bob-cli-seg-gap-end");
    let dry = run_dry_json(&v2, &d2, &["x=2~2+=!1"]);
    let value = run_json(&v2, &d2, &["x=2~2+=!1"]);
    assert_eq!(past_of(&dry)["window"], past_of(&value)["window"]);
    let past = past_of(&value);
    assert_eq!(
        past["window"],
        serde_json::json!({"start": "0820", "end": "0830", "minutes": 10}),
        "{past}"
    );
    assert_eq!(past["closed_at"], "0830");
    assert_eq!(past["segment"]["edge"], "end");
    assert_eq!(past["stack"]["pin"], "end");
    let after = fs::read_to_string(&d2).expect("read day");
    assert!(
        after.contains("(**0820-0830** [t:: 10m]) — CHORE"),
        "{after}"
    );
    let (_h, vh, dh) = segment_vault("bob-cli-seg-gap-human");
    let human = run_human(&vh, &dh, &["x=2~2=!1"]);
    assert!(
        human.contains("at the start of ~2 gap 0800-0830"),
        "{human}"
    );
}

#[test]
fn open_cases_head_gap_fill_and_hints() {
    // x=~~ head: 0705-0730.
    let (_t, vault, day) = segment_vault("bob-cli-seg-head");
    let value = run_json(&vault, &day, &["x=~~=!1"]);
    let past = past_of(&value);
    assert_eq!(
        past["window"],
        serde_json::json!({"start": "0705", "end": "0730", "minutes": 25}),
        "{past}"
    );
    assert_eq!(past["segment"]["kind"], "head");
    assert_eq!(past["stack"]["pin"], "end");
    let after = fs::read_to_string(&day).expect("read day");
    assert!(
        after.contains("(**0705-0730** [t:: 25m]) — CHORE"),
        "{after}"
    );
    let (_h, vh, dh) = segment_vault("bob-cli-seg-head-human");
    let human = run_human(&vh, &dh, &["x=~~=!1"]);
    assert!(human.contains("before PLAN"), "{human}");

    // x=~2 untyped fits the gap; x=6~2 fills it.
    let (_a, va, da) = segment_vault("bob-cli-seg-gap-fit");
    let value = run_json(&va, &da, &["x=~2=!1"]);
    assert_eq!(
        past_of(&value)["window"],
        serde_json::json!({"start": "0800", "end": "0825", "minutes": 25}),
        "{value}"
    );
    let (_b, vb, db) = segment_vault("bob-cli-seg-gap-fill");
    let value = run_json(&vb, &db, &["x=6~2=!1"]);
    assert_eq!(
        past_of(&value)["window"],
        serde_json::json!({"start": "0800", "end": "0830", "minutes": 30}),
        "{value}"
    );

    // x=8~2 refuses with both hints; x=8~2! trims.
    let (_c, vc, dc) = segment_vault("bob-cli-seg-overflow");
    let err = run_expect_error(&vc, &dc, &["x=8~2"]);
    assert!(err.contains("add ! to take it: x=8~2!"), "{err}");
    assert!(err.contains("or fit the gap: x=6~2"), "{err}");
    let value = run_json(&vc, &dc, &["x=8~2!"]);
    assert_eq!(
        past_of(&value)["window"],
        serde_json::json!({"start": "0800", "end": "0840", "minutes": 40}),
        "{value}"
    );
    let after = fs::read_to_string(&dc).expect("read day");
    assert!(
        after.contains("(**0840-0900** [t:: 20m]) — BUILD"),
        "{after}"
    );

    // x=~1 needs !; x=~1! carves; x=6~1! suggests carve.
    let (_d, vd, dd) = segment_vault("bob-cli-seg-carve");
    let err = run_expect_error(&vd, &dd, &["x=~1"]);
    assert!(err.contains("add !"), "{err}");
    let value = run_json(&vd, &dd, &["x=~1!"]);
    assert_eq!(
        past_of(&value)["window"],
        serde_json::json!({"start": "0830", "end": "0855", "minutes": 25}),
        "{value}"
    );
    let (_e, ve, de) = segment_vault("bob-cli-seg-erase");
    let err = run_expect_error(&ve, &de, &["x=6~1!"]);
    assert!(err.contains("would take all of BUILD"), "{err}");
    assert!(err.contains("carve at most 5 blocks: x=5~1!"), "{err}");

    // x=2~2! needs no repair.
    let (_f, vf, df) = segment_vault("bob-cli-seg-needless");
    let err = run_expect_error(&vf, &df, &["x=2~2!"]);
    assert!(err.contains("takes no time from another session"), "{err}");

    // x=read4~ cuts at now.
    let (_g2, vg2, dg2) = segment_vault("bob-cli-seg-read-tail-cut");
    let value = run_json(&vg2, &dg2, &["x=read4~=!1"]);
    assert_eq!(
        past_of(&value)["window"],
        serde_json::json!({
            "start": "0900", "end": "0915", "minutes": 15,
            "requested_minutes": 20, "cut": "now",
        }),
        "{value}"
    );
}

#[test]
fn timeline_teaching_rows() {
    // No closed rows.
    let temp = TempDir::new("bob-cli-seg-noclosed");
    let vault = temp.path().join("vault");
    fs::create_dir_all(vault.join("2026")).expect("create vault");
    write_file(&vault.join("bob.md"), "## Tasks\n");
    let day = write_day(&vault, "## Pomodoros\n- [ ] () — CHORE\n");
    let err = run_expect_error(&vault, &day, &["x=2~1"]);
    assert!(err.contains("nothing is closed today yet"), "{err}");
    assert!(err.contains("write x=2 to end now"), "{err}");

    // Empty tail with a running wall.
    let temp = TempDir::new("bob-cli-seg-emptytail");
    let vault = temp.path().join("vault");
    fs::create_dir_all(vault.join("2026")).expect("create vault");
    write_file(&vault.join("bob.md"), "## Tasks\n");
    let day = write_day(
        &vault,
        "## Pomodoros\n- [x] (**0830-0900** [t:: 30m]) — BUILD\n- [ ] (0900-0925) — WORK\n",
    );
    let err = run_expect_error(&vault, &day, &["x=~"]);
    assert!(
        err.contains("nothing to record between BUILD (ended 0900)"),
        "{err}"
    );
    assert!(err.contains("WORK (running since 0900)"), "{err}");

    // No head (first row at midnight).
    let temp = TempDir::new("bob-cli-seg-nohead");
    let vault = temp.path().join("vault");
    fs::create_dir_all(vault.join("2026")).expect("create vault");
    write_file(&vault.join("bob.md"), "## Tasks\n");
    let day = write_day(
        &vault,
        "## Pomodoros\n- [x] (**0000-0030** [t:: 30m]) — FIRST\n",
    );
    let err = run_expect_error(&vault, &day, &["x=~~"]);
    assert!(err.contains("no time before your first Pomodoro"), "{err}");

    // No rows.
    let temp = TempDir::new("bob-cli-seg-norows");
    let vault = temp.path().join("vault");
    fs::create_dir_all(vault.join("2026")).expect("create vault");
    write_file(&vault.join("bob.md"), "## Tasks\n");
    let day = write_day(&vault, "## Pomodoros\n");
    let err = run_expect_error(&vault, &day, &["x=~~"]);
    assert!(err.contains("nothing is on today's timeline yet"), "{err}");

    // Out of range lists segments.
    let (_t, vault, day) = segment_vault("bob-cli-seg-range");
    let err = run_expect_error(&vault, &day, &["x=~4"]);
    assert!(err.contains("today has 3 segments"), "{err}");
    assert!(err.contains("~1 BUILD 0830-0900"), "{err}");
    assert!(err.contains("~2 gap 0800-0830"), "{err}");
    assert!(err.contains("~3 PLAN 0730-0800"), "{err}");

    // First row at 0020: x=~~ cuts to fit, x=5~~ refuses at midnight gate.
    let temp = TempDir::new("bob-cli-seg-midhead");
    let vault = temp.path().join("vault");
    fs::create_dir_all(vault.join("2026")).expect("create vault");
    write_file(&vault.join("bob.md"), "## Tasks\n");
    let day = write_day(
        &vault,
        "## Pomodoros\n- [x] (**0020-0040** [t:: 20m]) — FIRST\n- [ ] () — CHORE\n",
    );
    let value = run_json(&vault, &day, &["x=~~"]);
    assert_eq!(
        past_of(&value)["window"],
        serde_json::json!({
            "start": "0000", "end": "0020", "minutes": 20,
            "requested_minutes": 25, "cut": "segment",
        }),
        "{value}"
    );
    let temp = TempDir::new("bob-cli-seg-midgate");
    let vault = temp.path().join("vault");
    fs::create_dir_all(vault.join("2026")).expect("create vault");
    write_file(&vault.join("bob.md"), "## Tasks\n");
    let day = write_day(
        &vault,
        "## Pomodoros\n- [x] (**0020-0040** [t:: 20m]) — FIRST\n- [ ] () — CHORE\n",
    );
    let err = run_expect_error(&vault, &day, &["x=5~~"]);
    assert!(err.contains("before midnight"), "{err}");
    assert!(err.contains("at most 4 blocks"), "{err}");
}

#[test]
fn wall_cut_and_runs_and_snapshot() {
    // x=~ with WORK running since 0905 cuts at the wall.
    let temp = TempDir::new("bob-cli-seg-wall");
    let vault = temp.path().join("vault");
    fs::create_dir_all(vault.join("2026")).expect("create vault");
    write_file(
        &vault.join("bob.md"),
        "## Tasks\n\n- [ ] #task First [created::2026-11-01] ^t1\n",
    );
    let day = write_day(
        &vault,
        concat!(
            "## Pomodoros\n",
            "- [x] (**0730-0800** [t:: 30m]) — PLAN\n",
            "- [x] (**0830-0900** [t:: 30m]) — BUILD\n",
            "- [ ] (0905-0925) — WORK\n",
            "- [ ] () — CHORE\n",
            "\t- [[bob#^t1]]\n",
        ),
    );
    let value = run_json(&vault, &day, &["x=~=!1"]);
    let past = past_of(&value);
    assert_eq!(
        past["window"],
        serde_json::json!({
            "start": "0900", "end": "0905", "minutes": 5,
            "requested_minutes": 25, "cut": "wall", "wall": "WORK",
        }),
        "{past}"
    );
    let temp_h = TempDir::new("bob-cli-seg-wall-h");
    let vault_h = temp_h.path().join("vault");
    fs::create_dir_all(vault_h.join("2026")).expect("create vault");
    write_file(
        &vault_h.join("bob.md"),
        "## Tasks\n\n- [ ] #task First [created::2026-11-01] ^t1\n",
    );
    let day_h = write_day(
        &vault_h,
        concat!(
            "## Pomodoros\n",
            "- [x] (**0730-0800** [t:: 30m]) — PLAN\n",
            "- [x] (**0830-0900** [t:: 30m]) — BUILD\n",
            "- [ ] (0905-0925) — WORK\n",
            "- [ ] () — CHORE\n",
            "\t- [[bob#^t1]]\n",
        ),
    );
    let human = run_human(&vault_h, &day_h, &["x=~=!1"]);
    assert!(human.contains("cut at WORK"), "{human}");
    assert!(human.contains("0900-0905"), "{human}");

    // Start-pinned two-member run flows forward with a segment cut.
    let (_t, vault, day) = segment_vault("bob-cli-seg-run-start");
    let value = run_json(&vault, &day, &["x=read2~2 x=fix"]);
    assert_eq!(value["captures"].as_array().map(Vec::len), Some(2));
    let items = value["captures"].as_array().cloned().unwrap_or_default();
    assert_eq!(
        items[0]["pomodoro_past"]["window"],
        serde_json::json!({"start": "0800", "end": "0810", "minutes": 10}),
        "{items:?}"
    );
    assert_eq!(
        items[1]["pomodoro_past"]["window"],
        serde_json::json!({
            "start": "0810", "end": "0830", "minutes": 20,
            "requested_minutes": 25, "cut": "segment", "fit": "~2",
        }),
        "{items:?}"
    );

    // End-pinned two-member run flows backward.
    let (_t, vault, day) = segment_vault("bob-cli-seg-run-end");
    let value = run_json(&vault, &day, &["x=email1 x=read2~2+"]);
    let items = value["captures"].as_array().cloned().unwrap_or_default();
    assert_eq!(items.len(), 2);
    assert_eq!(
        items[0]["pomodoro_past"]["window"],
        serde_json::json!({"start": "0815", "end": "0820", "minutes": 5}),
        "{items:?}"
    );
    assert_eq!(
        items[1]["pomodoro_past"]["window"],
        serde_json::json!({"start": "0820", "end": "0830", "minutes": 10}),
        "{items:?}"
    );

    // Snapshot: x closes the running session, then the pinned run numbers
    // against the staged close.
    let temp = TempDir::new("bob-cli-seg-snap");
    let vault = temp.path().join("vault");
    fs::create_dir_all(vault.join("2026")).expect("create vault");
    write_file(
        &vault.join("bob.md"),
        "## Tasks\n\n- [ ] #task First [created::2026-11-01] ^t1\n",
    );
    let day = write_day(
        &vault,
        concat!(
            "## Pomodoros\n",
            "- [x] (**0730-0800** [t:: 30m]) — PLAN\n",
            "- [ ] (0830-0925) — BUILD\n",
            "- [ ] () — CHORE\n",
            "\t- [[bob#^t1]]\n",
        ),
    );
    let value = run_json(&vault, &day, &["x x=2~1!"]);
    let items = value["captures"].as_array().cloned().unwrap_or_default();
    assert_eq!(items.len(), 2);
    assert_eq!(
        items[1]["pomodoro_past"]["window"],
        serde_json::json!({"start": "0830", "end": "0840", "minutes": 10}),
        "{items:?}"
    );
    let after = fs::read_to_string(&day).expect("read day");
    assert!(
        after.contains("(**0830-0840** [t:: 10m]) — CHORE"),
        "{after}"
    );
}

fn run_complete_json(
    vault: &std::path::Path,
    day: &std::path::Path,
    cursor: usize,
    text: &str,
) -> serde_json::Value {
    let output = bob_command()
        .env("BOB_NOW", NOW)
        .env("BOB_DAY_FILE", day)
        .arg("capture-complete")
        .arg("-b")
        .arg(vault)
        .arg("-c")
        .arg(cursor.to_string())
        .arg("-f")
        .arg("json")
        .arg("--")
        .arg(text)
        .output()
        .expect("run bob capture-complete");
    assert_success(&output);
    serde_json::from_str::<serde_json::Value>(stdout(&output).trim())
        .expect("completion JSON")
}

fn run_rewrite_json(text: &str, cursor: usize) -> serde_json::Value {
    let output = bob_command()
        .arg("capture-rewrite")
        .arg("-c")
        .arg(cursor.to_string())
        .arg("-f")
        .arg("json")
        .arg("--")
        .arg(text)
        .output()
        .expect("run bob capture-rewrite");
    assert_success(&output);
    serde_json::from_str::<serde_json::Value>(stdout(&output).trim())
        .expect("rewrite JSON")
}

fn run_capture_raw(
    vault: &std::path::Path,
    day: &std::path::Path,
    extra: &[&str],
    text: &[&str],
) -> (std::process::ExitStatus, serde_json::Value) {
    let mut command = bob_command();
    command
        .env("BOB_NOW", NOW)
        .env("BOB_DAY_FILE", day)
        .arg("capture")
        .arg("-b")
        .arg(vault)
        .arg("-f")
        .arg("json");
    for flag in extra {
        command.arg(flag);
    }
    command.arg("--");
    for arg in text {
        command.arg(arg);
    }
    let output = command.output().expect("run bob capture");
    let value =
        serde_json::from_str::<serde_json::Value>(stdout(&output).trim())
            .expect("capture JSON envelope");
    (output.status, value)
}

#[test]
fn assists_segment_picker_lists_tail_index_and_head() {
    let (_t, vault, day) = segment_vault("bob-cli-seg-picker");
    // Empty query lists every row: tail, ~1..~3, head.
    let value = run_complete_json(&vault, &day, 3, "x=~");
    assert_eq!(value["context"], "pomodoro_past_segment");
    let replacements: Vec<&str> = value["candidates"]
        .as_array()
        .expect("candidates")
        .iter()
        .map(|row| row["replacement"].as_str().expect("replacement"))
        .collect();
    assert_eq!(replacements, vec!["~", "~1", "~2", "~3", "~~"]);
    let first = &value["candidates"][0];
    assert_eq!(first["label"], "~ since BUILD 0900-0915");
    assert_eq!(first["kind"], "tail");
    assert_eq!(first["edge"], "start");
    assert_eq!(first["needs_repair"], false);
    let pomodoro = &value["candidates"][1];
    assert_eq!(pomodoro["replacement"], "~1");
    assert_eq!(pomodoro["label"], "~1 BUILD 0830-0900");
    assert_eq!(pomodoro["needs_repair"], true);
    assert_eq!(pomodoro["index"], 1);

    // `~` query lists only the head.
    let head = run_complete_json(&vault, &day, 4, "x=~~");
    let head_replacements: Vec<&str> = head["candidates"]
        .as_array()
        .expect("candidates")
        .iter()
        .map(|row| row["replacement"].as_str().expect("replacement"))
        .collect();
    assert_eq!(head_replacements, vec!["~~"]);
    assert_eq!(head["candidates"][0]["kind"], "head");

    // Digits prefix-match the index.
    let digits = run_complete_json(&vault, &day, 4, "x=~2");
    let digit_replacements: Vec<&str> = digits["candidates"]
        .as_array()
        .expect("candidates")
        .iter()
        .map(|row| row["replacement"].as_str().expect("replacement"))
        .collect();
    assert_eq!(digit_replacements, vec!["~2"]);

    // Digits with `+` keep the exact index as an end-edge row.
    let plus = run_complete_json(&vault, &day, 5, "x=~2+");
    let plus_rows = plus["candidates"].as_array().expect("candidates");
    assert_eq!(plus_rows.len(), 1);
    assert_eq!(plus_rows[0]["replacement"], "~2+");
    assert_eq!(plus_rows[0]["edge"], "end");

    // Empty days list no rows but stay successful.
    let temp = TempDir::new("bob-cli-seg-picker-empty");
    let empty_vault = temp.path().join("vault");
    std::fs::create_dir_all(empty_vault.join("2026")).expect("vault");
    let empty_day = empty_vault.join("2026").join("20261104.md");
    write_file(&empty_day, "## Pomodoros\n");
    let empty = run_complete_json(&empty_vault, &empty_day, 3, "x=~");
    assert_eq!(empty["candidates"].as_array().expect("array").len(), 0);
}

#[test]
fn assists_dash_rewrite_and_stale_guard_and_shell_tilde() {
    // `bob capture-rewrite --cursor` turns `x=2-` into `x=2~`.
    let rewrite = run_rewrite_json("x=2-", 4);
    assert_eq!(rewrite["text"], "x=2~");
    assert_eq!(rewrite["rule"], "expand_sigil_alias");

    // Matching preview windows succeed.
    let (_t0, v0, d0) = segment_vault("bob-cli-seg-expect-ok");
    let (status, value) = run_capture_raw(
        &v0,
        &d0,
        &["--expect-past-windows", "0900-0915"],
        &["x=~=!1"],
    );
    assert!(status.success(), "{value}");
    assert_eq!(value["ok"], true);

    // Mismatched windows refuse with the stale-preview code and message.
    let (_t1, v1, d1) = segment_vault("bob-cli-seg-expect-mismatch");
    let (status, value) = run_capture_raw(
        &v1,
        &d1,
        &["--expect-past-windows", "0900-0910"],
        &["x=~=!1"],
    );
    assert!(!status.success(), "expected stale refusal: {value}");
    assert_eq!(value["code"], "pomodoro_past_windows_changed");
    assert!(value["error"]
        .as_str()
        .unwrap_or_default()
        .contains("your timeline changed since the preview"));
    assert!(value["error"]
        .as_str()
        .unwrap_or_default()
        .contains("x=~=!1 now means 0900-0915"));

    // Count mismatch and malformed lists also refuse with the same code.
    let (_t2, v2, d2) = segment_vault("bob-cli-seg-expect-count");
    let (status, value) = run_capture_raw(
        &v2,
        &d2,
        &["--expect-past-windows", "0900-0915,0800-0810"],
        &["x=~=!1"],
    );
    assert!(!status.success(), "{value}");
    assert_eq!(value["code"], "pomodoro_past_windows_changed");
    let (_t3, v3, d3) = segment_vault("bob-cli-seg-expect-malformed");
    let (status, value) = run_capture_raw(
        &v3,
        &d3,
        &["--expect-past-windows", "bogus"],
        &["x=~=!1"],
    );
    assert!(!status.success(), "{value}");
    assert_eq!(value["code"], "pomodoro_past_windows_changed");

    // Omitted flag behaves identically (success).
    let (_t4, v4, d4) = segment_vault("bob-cli-seg-expect-omitted");
    let (status, value) = run_capture_raw(&v4, &d4, &[], &["x=~=!1"]);
    assert!(status.success(), "{value}");
    assert_eq!(value["ok"], true);

    // Shell-expanded tilde refuses with its own code (argv only).
    let (_t5, v5, d5) = segment_vault("bob-cli-seg-shell-tilde");
    let home = std::env::var("HOME").unwrap_or_default();
    if !home.is_empty() {
        let token = format!("x={home}");
        let (status, value) = run_capture_raw(&v5, &d5, &[], &[&token]);
        assert!(!status.success(), "{value}");
        assert_eq!(value["code"], "shell_expanded_tilde");
    }
}

#[test]
fn same_capture_refusal_and_rollback() {
    // A later run that would erase a row this capture just recorded refuses.
    // Separate runs need an ordinary item between them; otherwise the two
    // suffixed items form one run and fail with the double-pin teaching.
    let (_t, vault, day) = segment_vault("bob-cli-seg-samecap");
    let before = fs::read_to_string(&day).unwrap();
    let err = run_expect_error(
        &vault,
        &day,
        &["x=2~2=!1\n\n- [ ] #task Note\n\nx=2~3!"],
    );
    assert!(err.contains("which this capture just recorded"), "{err}");
    assert!(err.contains("capture them separately"), "{err}");
    let after = fs::read_to_string(&day).unwrap();
    assert_eq!(after, before, "refusal preserves bytes");

    // Whole-batch rollback: a later run failure leaves bytes identical.
    let (_t, vault, day) = segment_vault("bob-cli-seg-rollback");
    let before = fs::read_to_string(&day).unwrap();
    let err = run_expect_error(
        &vault,
        &day,
        &["x=2~2=!1\n\n- [ ] #task Note\n\nx=6~1!"],
    );
    assert!(!err.is_empty(), "expected a refusal");
    let after = fs::read_to_string(&day).unwrap();
    assert_eq!(after, before, "failed batch rolls back");
}
