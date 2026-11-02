//! `bob stats -k worked|off-clock|total`: selection, aliases, human and CSV
//! output, JSON clock contract, and diagnostics through the real CLI.

use crate::support::*;
use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};
use tempfile::TempDir;

const NOW: &str = "2026-10-11 12:00:00";
const RANGE: &str = "2026-10-08..2026-10-11";

fn fixture_vault() -> PathBuf {
    fixture("stats/vault")
}

fn copy_dir(source: &Path, target: &Path) {
    fs::create_dir_all(target).expect("create target");
    for entry in fs::read_dir(source).expect("read source") {
        let entry = entry.expect("dir entry");
        let child = target.join(entry.file_name());
        if entry.path().is_dir() {
            copy_dir(&entry.path(), &child);
        } else {
            fs::copy(entry.path(), &child).expect("copy file");
        }
    }
}

/// Every file under `root` with its bytes, for before/after comparisons.
fn snapshot(root: &Path) -> Vec<(PathBuf, Vec<u8>)> {
    let mut files = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in fs::read_dir(&dir).expect("read dir") {
            let path = entry.expect("dir entry").path();
            if path.is_dir() {
                stack.push(path);
            } else {
                let bytes = fs::read(&path).expect("read file");
                files.push((path.strip_prefix(root).unwrap().into(), bytes));
            }
        }
    }
    files.sort();
    files
}

fn stats_in(vault: &Path) -> std::process::Command {
    let mut command = bob_command();
    command
        .env("BOB_DIR", vault)
        .env("BOB_NOW", NOW)
        .env("NO_COLOR", "1");
    command
}

fn run(vault: &Path, args: &[&str]) -> String {
    let output = stats_in(vault).args(args).output().expect("run bob stats");
    assert_success(&output);
    assert_stdout_has_no_ansi(&output);
    stdout(&output)
}

fn run_json(vault: &Path, args: &[&str]) -> Value {
    let mut full = args.to_vec();
    full.extend(["-f", "json"]);
    serde_json::from_str(run(vault, &full).trim()).expect("stats JSON")
}

fn metric<'a>(report: &'a Value, id: &str) -> &'a Value {
    report["metrics"]
        .as_array()
        .expect("metrics")
        .iter()
        .find(|metric| metric["id"] == id)
        .unwrap_or_else(|| panic!("missing metric {id}"))
}

/// The pristine fixture minus its only off-clock row: no off-clock time
/// anywhere, so the Worked report must read exactly as it did before clocks.
fn plain_vault() -> (TempDir, PathBuf) {
    let temp = TempDir::new().expect("tempdir");
    let vault = temp.path().join("vault");
    copy_dir(&fixture_vault(), &vault);
    let daily = vault.join("2026/20261009.md");
    let text = fs::read_to_string(&daily).expect("daily note");
    let kept: Vec<&str> = text
        .lines()
        .filter(|line| !line.contains("MEET") && !line.contains("[[#^gtd]]"))
        .collect();
    fs::write(&daily, format!("{}\n", kept.join("\n"))).expect("write daily");
    (temp, vault)
}

/// The fixture plus one busy day: worked, stamped off-clock, bare, conflict,
/// drift in both directions, and an overlap, with resolvable tagged tasks.
fn clock_vault() -> (TempDir, PathBuf) {
    let temp = TempDir::new().expect("tempdir");
    let vault = temp.path().join("vault");
    copy_dir(&fixture_vault(), &vault);
    let tasks = vault.join("tasks.md");
    let mut text = fs::read_to_string(&tasks).expect("tasks note");
    text.push_str(
        "- [ ] #task Off-clock errand #offclock [created:: 2026-10-09] ^off1\n",
    );
    text.push_str(
        "- [x] #task On-clock chore [created:: 2026-10-09] [completion:: 2026-10-09] ^on1\n",
    );
    fs::write(&tasks, text).expect("write tasks");
    let daily = vault.join("2026/20261009.md");
    let mut text = fs::read_to_string(&daily).expect("daily note");
    for row in [
        "- [x] (**10:00 - 10:30** [t:: 20m] [off:: 45m]) — CONFLICT\n",
        "- [x] (**08:10 - 08:40** [t:: 20m]) — OVER\n",
        "- [x] (**14:00 - 14:25** [t:: 25m]) — LATE\n  - [[TASKS#^off1]]\n",
        "- [x] (**15:00 - 15:20** [off:: 20m]) — EARLY\n  - [[tasks#^on1]]\n",
        "- [x] (**18:00 - 18:30**) — SOLO\n",
    ] {
        text.push_str(row);
    }
    fs::write(&daily, text).expect("write daily");
    (temp, vault)
}

fn config_with_pins(temp: &TempDir) -> PathBuf {
    let path = temp.path().join("config.yml");
    fs::write(
        &path,
        "\
stats:
  hide: [hidden_worked]
  metrics:
    family_time:
      label: Family time
      kind: pomodoro
      clock: off-clock
    hidden_worked:
      label: Hidden worked
      kind: pomodoro
    inner:
      label: Inner
      kind: formula
      expr: worked
      unit: minutes
    outer:
      label: Outer
      kind: formula
      expr: inner + hidden_worked
      unit: minutes
",
    )
    .expect("write config");
    path
}

#[test]
fn worked_overview_names_hidden_off_clock_time_once() {
    let text = run(&fixture_vault(), &["stats", "-r", RANGE]);
    let note = "◌ 1h 00m off-clock - bob stats -k off-clock";
    assert_eq!(text.matches(note).count(), 1, "{text}");
    assert!(!text.contains("off the clock"), "{text}");
    assert!(!text.contains("total -"), "{text}");
    // The default view and an explicit -k worked are the same report.
    assert_eq!(
        text,
        run(&fixture_vault(), &["stats", "-r", RANGE, "-k", "worked"])
    );
}

#[test]
fn off_clock_overview_header_label_and_empty_window() {
    let text =
        run(&fixture_vault(), &["stats", "-r", RANGE, "-k", "off-clock"]);
    let header = text.lines().next().expect("header");
    assert!(
        header.ends_with("vs Oct 5 – Oct 7 - off the clock"),
        "{header}"
    );
    assert!(text.contains("\nOff-clock "), "{text}");
    assert!(!text.contains("No off-clock time"), "{text}");
    assert!(!text.contains("bob stats -k"), "{text}");
    // Pinning the window to the days after the row leaves nothing to count.
    let empty = run(
        &fixture_vault(),
        &["stats", "-r", "2026-10-10..2026-10-11", "-k", "off-clock"],
    );
    assert_eq!(
        empty.matches("No off-clock time in this window").count(),
        1,
        "{empty}"
    );
}

#[test]
fn total_overview_shows_the_breakdown_in_window_units() {
    let text = run(&fixture_vault(), &["stats", "-r", RANGE, "-k", "total"]);
    assert!(
        text.contains("◌ 1h 42m total - 42m worked + 1h 00m off-clock\n"),
        "{text}"
    );
    assert!(text.contains("\nTotal "), "{text}");
    assert_eq!(text.matches("1h 42m total").count(), 1, "{text}");
}

#[test]
fn detail_views_follow_the_clock_with_one_note() {
    let worked = run(
        &fixture_vault(),
        &["stats", "show", "worked", "sessions", "-r", RANGE],
    );
    assert_eq!(
        worked
            .matches("◌ 1h 00m off-clock - bob stats show worked sessions -k off-clock")
            .count(),
        1,
        "{worked}"
    );

    let off = run(
        &fixture_vault(),
        &[
            "stats",
            "show",
            "worked",
            "sessions",
            "-r",
            RANGE,
            "-k",
            "off-clock",
        ],
    );
    assert!(off.starts_with("Off-clock - minutes per day"), "{off}");
    assert!(
        off.contains("\nOff-clock sessions - count per day"),
        "{off}"
    );
    assert!(off.contains("total 1h 00m over 3 days"), "{off}");
    assert!(!off.contains("worked +"), "{off}");

    let total = run(
        &fixture_vault(),
        &[
            "stats", "show", "worked", "sessions", "-r", RANGE, "-k", "total",
        ],
    );
    assert!(total.starts_with("Total - minutes per day"), "{total}");
    assert!(
        total.contains(
            "total 1h 32m over 3 days = 32m worked + 1h 00m off-clock"
        ),
        "{total}"
    );
    assert!(
        total.contains("total 3 over 3 days = 2 worked + 1 off-clock"),
        "{total}"
    );
    assert!(total.contains("MEET 1h 00m 65% (◌ 1h 00m)"), "{total}");
    assert!(
        total.contains("◌ 1h 42m total - 42m worked + 1h 00m off-clock\n"),
        "{total}"
    );
}

#[test]
fn csv_ends_with_the_effective_clock_of_pomodoro_rows_only() {
    for (flag, clock) in [
        ("worked", "worked"),
        ("off-clock", "off-clock"),
        ("total", "total"),
    ] {
        let text = run(
            &fixture_vault(),
            &["stats", "-r", RANGE, "-k", flag, "-f", "csv"],
        );
        let mut lines = text.lines();
        assert_eq!(
            lines.next(),
            Some("date,end,metric,value,display,quality,provisional,num,den,clock")
        );
        for line in lines {
            let fields: Vec<&str> = line.split(',').collect();
            let last = *fields.last().expect("fields");
            if fields[2] == "worked" {
                assert_eq!(last, clock, "{line}");
            } else {
                assert_eq!(last, "", "{line}");
            }
        }
    }
}

#[test]
fn aliases_are_hidden_and_select_the_same_report() {
    let vault = fixture_vault();
    for (canonical, aliases) in [
        ("worked", &["on"][..]),
        ("off-clock", &["off", "untracked"][..]),
        ("total", &["all"][..]),
    ] {
        let expected = run(
            &vault,
            &["stats", "-r", RANGE, "-k", canonical, "-f", "json"],
        );
        let long = run(
            &vault,
            &["stats", "-r", RANGE, "--clock", canonical, "-f", "json"],
        );
        assert_eq!(expected, long);
        for alias in aliases {
            let got =
                run(&vault, &["stats", "-r", RANGE, "-k", alias, "-f", "json"]);
            assert_eq!(expected, got, "-k {alias}");
            let shown = run(
                &vault,
                &["stats", "show", "worked", "-r", RANGE, "-k", alias],
            );
            let shown_canonical = run(
                &vault,
                &["stats", "show", "worked", "-r", RANGE, "-k", canonical],
            );
            assert_eq!(shown, shown_canonical, "show -k {alias}");
        }
    }
    let default = run(&vault, &["stats", "-r", RANGE, "-f", "json"]);
    assert_eq!(
        default,
        run(&vault, &["stats", "-r", RANGE, "-k", "on", "-f", "json"])
    );

    let output = stats_in(&vault)
        .args(["stats", "-k", "bogus"])
        .output()
        .expect("run bob stats -k bogus");
    assert_eq!(output.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("[possible values: worked, off-clock, total]"),
        "{stderr}"
    );
    assert!(!stderr.contains("untracked"), "{stderr}");
}

#[test]
fn completion_lists_only_canonical_clock_values_with_descriptions() {
    let output = bob_command()
        .args([
            "__complete",
            "zsh",
            "--protocol",
            "1",
            "--",
            "bob",
            "stats",
            "-k",
            "",
        ])
        .output()
        .expect("complete bob stats -k");
    assert_success(&output);
    let text = stdout(&output);
    for value in ["worked", "off-clock", "total"] {
        assert!(
            text.lines()
                .any(|line| line.starts_with(&format!("{value}\t"))
                    && line.len() > value.len() + 1),
            "missing described {value}:\n{text}"
        );
    }
    for hidden in ["on\t", "off\t", "untracked", "all\t"] {
        assert!(!text.contains(hidden), "alias {hidden} leaked:\n{text}");
    }

    let show = bob_command()
        .args([
            "__complete",
            "zsh",
            "--protocol",
            "1",
            "--",
            "bob",
            "stats",
            "show",
            "worked",
            "--clock",
            "o",
        ])
        .output()
        .expect("complete bob stats show --clock");
    assert_success(&show);
    assert!(stdout(&show).contains("off-clock\t"), "{}", stdout(&show));
}

#[test]
fn json_clock_object_and_session_filters_follow_the_lens() {
    let (_temp, vault) = clock_vault();
    let mut seen = Vec::new();
    for (flag, minutes, sessions, label) in [
        ("worked", 97 + 10, 6, "Worked"),
        ("off-clock", 110, 3, "Off-clock"),
        ("total", 97 + 10 + 110, 9, "Total"),
    ] {
        let report = run_json(&vault, &["stats", "-r", RANGE, "-k", flag]);
        assert_eq!(report["clock"]["value"], flag);
        assert_eq!(report["clock"]["label"], label);
        assert_eq!(report["clock"]["since"], "2026-10-09");
        let options = report["clock"]["options"].as_array().expect("options");
        let values: Vec<&str> = options
            .iter()
            .map(|o| o["value"].as_str().unwrap())
            .collect();
        assert_eq!(values, ["worked", "off-clock", "total"]);
        // Window totals are independent of the lens.
        let totals: Vec<i64> = options
            .iter()
            .map(|o| o["minutes"].as_i64().unwrap())
            .collect();
        assert_eq!(totals, [107, 110, 217], "{flag}");
        assert_eq!(options[2]["display"], "3h 37m");

        let rows = report["sessions"].as_array().expect("sessions");
        assert_eq!(rows.len(), sessions, "{flag} sessions");
        let row_minutes: i64 =
            rows.iter().map(|r| r["minutes"].as_i64().unwrap()).sum();
        assert_eq!(row_minutes, minutes, "{flag} session minutes");
        for row in rows {
            let off = row["off_clock"] == true;
            match flag {
                "worked" => assert!(!off, "{row}"),
                "off-clock" => assert!(off, "{row}"),
                _ => {}
            }
            assert_eq!(row["homes"].get("untracked").is_some(), off, "{row}");
        }
        // Diagnostics describe the window, not the lens.
        let homes = &report["health"]["homes"];
        assert_eq!(homes["off_clock_sessions"], 3, "{flag}");
        assert_eq!(homes["untracked_sessions"], 3, "{flag}");
        assert_eq!(homes["unstamped_sessions"], 2, "{flag}");
        assert_eq!(homes["clock_conflicts"], 1, "{flag}");
        assert_eq!(homes["clock_drift"], 2, "{flag}");
        assert_eq!(homes["overlap_minutes"], 15, "{flag}");
        assert_eq!(homes["overlaps"][0]["minutes"], 15, "{flag}");
        seen.push(report);
    }

    // The default metric follows the lens, and its sub-share is exact.
    let worked = metric(&seen[0], "worked");
    assert_eq!(worked["points"][1]["value"], 97);
    assert_eq!(worked["points"][1]["off_clock"], 110);
    let off = metric(&seen[1], "worked");
    assert_eq!(off["label"], "Off-clock");
    assert_eq!(off["points"][1]["value"], 110);
    assert!(off["points"][1].get("off_clock").is_none());
    let total = metric(&seen[2], "worked");
    assert_eq!(total["label"], "Total");
    assert_eq!(total["direction"], "neutral");
    assert_eq!(total["points"][1]["value"], 207);
    assert_eq!(total["points"][1]["off_clock"], 110);
    // Per-part sub-shares sum back to the clock's off-clock minutes.
    for dimension in ["area", "root_parent", "project", "theme"] {
        let parts =
            total["parts_summary"][dimension].as_array().expect("parts");
        let value: i64 =
            parts.iter().map(|p| p["value"].as_i64().unwrap()).sum();
        let off: i64 = parts
            .iter()
            .map(|p| p.get("off_clock").and_then(Value::as_i64).unwrap_or(0))
            .sum();
        assert_eq!((value, off), (207, 110), "{dimension}");
    }
}

#[test]
fn human_output_surfaces_each_diagnostic_once_in_every_lens() {
    let (_temp, vault) = clock_vault();
    for flag in ["worked", "off-clock", "total"] {
        let text = run(&vault, &["stats", "-r", RANGE, "-k", flag]);
        for (code, message) in [
            (
                "clock_conflict",
                "1 closed session has more than one duration key",
            ),
            (
                "clock_drift",
                "2 closed sessions disagree with the linked tasks",
            ),
            ("clock_unstamped", "2 closed sessions have no duration key"),
            ("clock_overlap", "closed sessions overlap by 15m"),
        ] {
            let lines: Vec<&str> =
                text.lines().filter(|line| line.contains(code)).collect();
            assert_eq!(lines.len(), 1, "{flag} {code}:\n{text}");
            assert!(lines[0].contains(message), "{flag}: {}", lines[0]);
        }
    }
}

#[test]
fn explicit_ranges_presets_offsets_groups_and_compare_keep_clocks_additive() {
    let (_temp, vault) = clock_vault();
    let windows: &[&[&str]] = &[
        &["-r", RANGE],
        &["-r", RANGE, "-c"],
        &["-r", RANGE, "-g", "week"],
        &["-r", RANGE, "-g", "month"],
        &["-r", "7d"],
        &["-r", "7d", "-o", "1"],
        &["-r", "4w"],
        &["-r", "all"],
        &["-r", "all", "-c"],
    ];
    for window in windows {
        let mut args = vec!["stats", "show", "worked", "sessions"];
        args.extend(*window);
        let mut worked_args = args.clone();
        worked_args.extend(["-k", "worked"]);
        let mut off_args = args.clone();
        off_args.extend(["-k", "off-clock"]);
        let mut total_args = args.clone();
        total_args.extend(["-k", "total"]);
        let on = run_json(&vault, &worked_args);
        let off = run_json(&vault, &off_args);
        let both = run_json(&vault, &total_args);
        for id in ["worked", "sessions"] {
            for field in ["points", "prior_points"] {
                let w = metric(&on, id)[field].as_array().unwrap().clone();
                let o = metric(&off, id)[field].as_array().unwrap().clone();
                let t = metric(&both, id)[field].as_array().unwrap().clone();
                assert_eq!(w.len(), t.len(), "{window:?} {id} {field}");
                assert_eq!(o.len(), t.len(), "{window:?} {id} {field}");
                for ((w, o), t) in w.iter().zip(&o).zip(&t) {
                    // Before adoption Off-clock and Total are unavailable
                    // rather than zero; everywhere else Total is the sum.
                    if t["value"].is_null() {
                        assert_eq!(
                            t["quality"], "not_tracked",
                            "{window:?} {t}"
                        );
                        assert!(o["value"].is_null(), "{window:?} {o}");
                        continue;
                    }
                    assert_eq!(
                        t["value"].as_i64().unwrap(),
                        w["value"].as_i64().unwrap()
                            + o["value"].as_i64().unwrap(),
                        "{window:?} {id} {field}: {w} {o} {t}"
                    );
                    assert_eq!(
                        t.get("off_clock").and_then(Value::as_i64).unwrap_or(0),
                        o["value"].as_i64().unwrap(),
                        "{window:?} {id} {field}: {o} {t}"
                    );
                }
            }
        }
        // Reports differ only by the lens; the window totals never move.
        assert_eq!(
            on["clock"]["options"], both["clock"]["options"],
            "{window:?}"
        );
        assert_eq!(on["range"], both["range"], "{window:?}");
    }
}

#[test]
fn pinned_metrics_keep_their_clock_and_formulas_ignore_the_lens() {
    let (temp, vault) = clock_vault();
    let config = config_with_pins(&temp);
    let ids = ["worked", "family_time", "inner", "outer", "done"];
    let mut args = vec!["stats", "show"];
    args.extend(ids);
    args.extend(["-r", RANGE]);
    let mut reports = Vec::new();
    for flag in ["worked", "off-clock", "total"] {
        let mut full = args.clone();
        full.extend(["-k", flag, "-f", "json"]);
        let output = stats_in(&vault)
            .env("BOB_CONFIG_FILE", &config)
            .args(&full)
            .output()
            .expect("run bob stats show");
        assert_success(&output);
        reports.push(
            serde_json::from_str::<Value>(stdout(&output).trim())
                .expect("JSON"),
        );
    }
    let labels: Vec<(String, String)> = reports
        .iter()
        .map(|r| {
            (
                metric(r, "worked")["label"].as_str().unwrap().to_string(),
                metric(r, "family_time")["label"]
                    .as_str()
                    .unwrap()
                    .to_string(),
            )
        })
        .collect();
    assert_eq!(
        labels,
        [
            ("Worked".to_string(), "Family time · off-clock".to_string()),
            (
                "Off-clock".to_string(),
                "Family time · off-clock".to_string()
            ),
            ("Total".to_string(), "Family time · off-clock".to_string()),
        ]
    );
    for report in &reports {
        let family = metric(report, "family_time");
        assert_eq!(family["parameters"]["clock"], "off-clock");
        assert_eq!(family["direction"], "neutral");
        assert_eq!(family["points"][1]["value"], 110);
        // Formulas read their inputs on the Worked clock whatever the lens.
        assert_eq!(metric(report, "inner")["points"][1]["value"], 97);
        assert_eq!(metric(report, "outer")["points"][1]["value"], 194);
        assert!(metric(report, "inner")["parameters"].get("clock").is_none());
        // The overlay completes one extra task on the 9th.
        assert_eq!(metric(report, "done")["points"][1]["value"], 6);
        assert_eq!(metric(report, "done")["label"], "Done");
    }

    // The pin shows in the human report too, and never reads as worked time.
    let human = stats_in(&vault)
        .env("BOB_CONFIG_FILE", &config)
        .args(["stats", "show", "family_time", "-r", RANGE])
        .output()
        .expect("run bob stats show family_time");
    assert_success(&human);
    assert!(
        stdout(&human).starts_with("Family time · off-clock - minutes per day")
    );
}

#[test]
fn clean_vault_keeps_default_human_output_and_numbers() {
    let (_temp, vault) = plain_vault();
    let text = run(&vault, &["stats", "-r", RANGE]);
    let expected = "\
Stats - Sun Oct 11 - Oct 8 – Oct 10 - vs Oct 5 – Oct 7

               Oct 8 ─ Oct 10    yesterday   summary   vs prior
Worked         ·█▁▃          0m      16m/day  –
Reviewed       ▁·█▅           2        1/day  –
Done           ·█▇▂           4        5/day  –
Created        ▄██▂           5        5/day  ▲ 5×
Net flow       ··▁█          −1     −1 total  shrinking
Review closed  ·●●◌         met  2 of 2 days  –

today so far - Worked 10m - Reviewed 1 - Done 1 - Created 1 - Net flow 0 - Review closed not yet
journal - apollo 09:00 ✓ - hermes 1d ago
warning Review closed on Fri, Oct 9 but no answers were recorded: the review plugin may be outdated or a write failed  journal_gap
warning Review closed on Fri, Oct 9 but no answers were recorded: the review plugin may be outdated or a write failed  journal_gap
";
    assert_eq!(text, expected);

    // No adoption evidence: a null date and nothing invented, in every lens.
    for flag in ["worked", "off-clock", "total"] {
        let report = run_json(&vault, &["stats", "-r", RANGE, "-k", flag]);
        assert!(report["clock"]["since"].is_null(), "{flag}");
        let homes = &report["health"]["homes"];
        for key in [
            "off_clock_sessions",
            "untracked_sessions",
            "clock_conflicts",
            "clock_drift",
            "overlap_minutes",
            "overlaps",
        ] {
            assert!(homes.get(key).is_none(), "{flag}: {key} in {homes}");
        }
        let worked = metric(&report, "worked");
        for point in worked["points"].as_array().unwrap() {
            assert!(point.get("off_clock").is_none(), "{flag}: {point}");
        }
        assert!(report["warnings"].as_array().unwrap().iter().all(|w| !w
            ["code"]
            .as_str()
            .unwrap()
            .starts_with("clock_")));
    }
    let off = report_values(&vault, "off-clock");
    assert!(
        off.iter().all(|(_, quality)| quality == "not_tracked"),
        "{off:?}"
    );
    let empty = run(&vault, &["stats", "-r", RANGE, "-k", "off-clock"]);
    assert_eq!(
        empty.matches("No off-clock time in this window").count(),
        1,
        "{empty}"
    );
}

fn report_values(vault: &Path, flag: &str) -> Vec<(Option<i64>, String)> {
    let report = run_json(vault, &["stats", "-r", RANGE, "-k", flag]);
    metric(&report, "worked")["points"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|p| p["provisional"] == false)
        .map(|p| {
            (
                p["value"].as_i64(),
                p["quality"].as_str().unwrap().to_string(),
            )
        })
        .collect()
}

#[test]
fn non_pomodoro_metrics_ignore_the_lens() {
    let vault = fixture_vault();
    let reports: Vec<Value> = ["worked", "off-clock", "total"]
        .iter()
        .map(|flag| run_json(&vault, &["stats", "-r", RANGE, "-k", flag]))
        .collect();
    for id in ["reviewed", "done", "created", "net", "review_closed"] {
        for report in &reports[1..] {
            assert_eq!(
                metric(&reports[0], id),
                metric(report, id),
                "{id} must not follow the lens"
            );
        }
    }
}

#[test]
fn clock_reports_never_write_the_vault() {
    let (_temp, vault) = clock_vault();
    let before = snapshot(&vault);
    for flag in ["worked", "off-clock", "total"] {
        for extra in [&[][..], &["-f", "json"][..], &["-f", "csv"][..]] {
            let mut args = vec!["stats", "-r", RANGE, "-k", flag];
            args.extend(extra);
            run(&vault, &args);
            let mut show =
                vec!["stats", "show", "-a", "-r", "all", "-c", "-k", flag];
            show.extend(extra);
            run(&vault, &show);
        }
    }
    assert_eq!(before, snapshot(&vault));
}
