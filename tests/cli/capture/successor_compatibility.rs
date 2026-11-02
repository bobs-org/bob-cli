//! The successor-label setting leaves unrelated capture flows byte-identical.

use crate::support::*;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Output;

const NOW: &str = "2026-10-14 10:20:00";
type VaultSnapshot = Vec<(String, Vec<u8>)>;
type CaptureOutput = (Vec<u8>, Vec<u8>, VaultSnapshot);

#[derive(Clone, Copy, Debug)]
enum Flow {
    ManagedDone,
    PomodoroClose,
    CompleteLabelClose,
    CancelRecovery,
}

impl Flow {
    fn name(self) -> &'static str {
        match self {
            Self::ManagedDone => "managed-done",
            Self::PomodoroClose => "pomodoro-close",
            Self::CompleteLabelClose => "complete-label-close",
            Self::CancelRecovery => "cancel-recovery",
        }
    }

    fn draft(self) -> &'static str {
        match self {
            Self::ManagedDone => "!sase:pred",
            Self::PomodoroClose => "x!",
            Self::CompleteLabelClose => {
                "@cash+sell-stocks ~dev:sell-stocks-now|\"implemented by\""
            }
            Self::CancelRecovery => {
                "@cash+sell-stocks ~dev:sell-stocks-now|\"duplicate of\""
            }
        }
    }

    fn picks(self) -> &'static [&'static [&'static str]] {
        match self {
            Self::CancelRecovery => &[&[]],
            _ => &[&["-p", "all"], &["-p", "none"]],
        }
    }
}

struct Fixture {
    _temp: TempDir,
    vault: PathBuf,
    day: PathBuf,
    config: PathBuf,
}

fn setup(flow: Flow) -> Fixture {
    let temp =
        TempDir::new(&format!("bob-cli-successor-compat-{}", flow.name()));
    let vault = temp.path().join("vault");
    let day = vault.join("today.md");
    let config = temp.path().join("config.yml");
    let fixture = Fixture {
        _temp: temp,
        vault,
        day,
        config,
    };
    reset(&fixture, flow, 0);
    fixture
}

fn reset(fixture: &Fixture, flow: Flow, config_kind: usize) {
    if fixture.vault.exists() {
        fs::remove_dir_all(&fixture.vault).expect("reset test vault");
    }
    write_toggle_task_settings(&fixture.vault);
    let successor_config = match config_kind {
        0 => "",
        1 => "  successor_labels: []\n",
        2 => "  successor_labels:\n    - starts after\n",
        _ => unreachable!("three config variants"),
    };
    write_file(
        &fixture.config,
        &format!(
            "relations:\n  cancel_labels:\n    - duplicate of\n  complete_labels:\n    - implemented by\n{successor_config}"
        ),
    );

    let dependent = concat!(
        "- [?] #task Follow up [fresh:: 2026-10-01] [keeps:: 2] ",
        "[unblocked:: 2026-10-02] [dependsOn:: {target}] [id:: follow] ^follow\n",
    );
    match flow {
        Flow::ManagedDone => {
            let body = format!(
                "- [ ] #task Predecessor [id:: pred] ^pred\n{}",
                dependent.replace("{target}", "pred")
            );
            write_file(&fixture.vault.join("sase.md"), &body);
            write_file(
                &fixture.day,
                "## Pomodoros\n- [ ] () — SASE\n  - [[sase#^pred]]\n",
            );
        }
        Flow::PomodoroClose => {
            let body = format!(
                "- [ ] #task Predecessor [id:: pred] ^pred\n{}",
                dependent.replace("{target}", "pred")
            );
            write_file(&fixture.vault.join("sase.md"), &body);
            write_file(
                &fixture.day,
                "## Pomodoros\n- [ ] (**0920-0950**) — SASE\n  - [[sase#^pred]]\n- [ ] () — FIX\n",
            );
        }
        Flow::CompleteLabelClose | Flow::CancelRecovery => {
            write_file(
                &fixture.vault.join("cash.md"),
                "- [ ] #task Sell stocks [id:: cash__sell-stocks] ^sell-stocks\n",
            );
            write_file(
                &fixture.vault.join("dev.md"),
                "- [ ] #task Sell stocks now [id:: sell-stocks-now] ^sell-stocks-now\n",
            );
            write_file(
                &fixture.vault.join("needs.md"),
                &dependent.replace("{target}", "cash__sell-stocks"),
            );
            write_file(
                &fixture.day,
                "## Pomodoros\n- [ ] Morning 0900-1000\n  - [[cash#^sell-stocks]]\n",
            );
        }
    }
}

fn snapshot(vault: &Path) -> Vec<(String, Vec<u8>)> {
    let mut entries = Vec::new();
    let mut stack = vec![vault.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in fs::read_dir(&dir).expect("read vault directory") {
            let path = entry.expect("vault entry").path();
            if path.is_dir() {
                stack.push(path);
            } else {
                let relative = path
                    .strip_prefix(vault)
                    .expect("vault-relative path")
                    .to_string_lossy()
                    .into_owned();
                entries
                    .push((relative, fs::read(path).expect("read vault file")));
            }
        }
    }
    entries.sort_by(|left, right| left.0.cmp(&right.0));
    entries
}

fn run(fixture: &Fixture, flow: Flow, picks: &[&str], human: bool) -> Output {
    let mut command = bob_command();
    command.arg("capture").arg("-b").arg(&fixture.vault);
    if !human {
        command.arg("-f").arg("json");
    } else {
        command.env("NO_COLOR", "1");
    }
    command
        .args(picks)
        .arg("--")
        .arg(flow.draft())
        .env("BOB_CONFIG_FILE", &fixture.config)
        .env("BOB_DAY_FILE", &fixture.day)
        .env("BOB_NOW", NOW)
        .output()
        .expect("run capture CLI")
}

#[test]
fn successor_config_preserves_other_capture_outputs_and_vault_bytes() {
    for flow in [
        Flow::ManagedDone,
        Flow::PomodoroClose,
        Flow::CompleteLabelClose,
        Flow::CancelRecovery,
    ] {
        let fixture = setup(flow);
        for picks in flow.picks() {
            for human in [false, true] {
                let mut baseline: Option<CaptureOutput> = None;
                for config_kind in 0..3 {
                    reset(&fixture, flow, config_kind);
                    let output = run(&fixture, flow, picks, human);
                    assert_success(&output);
                    let actual = (
                        output.stdout.clone(),
                        output.stderr.clone(),
                        snapshot(&fixture.vault),
                    );
                    if let Some(expected) = &baseline {
                        assert_eq!(
                            actual, *expected,
                            "{} picks {picks:?}, human={human}, config={config_kind}",
                            flow.name()
                        );
                    } else {
                        baseline = Some(actual);
                    }
                }
            }
        }
    }
}
