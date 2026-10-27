//! Configuration and machine signals for coordinated PDF marker writes.
//!
//! This module only describes policy. Callers decide when and how to consume
//! it; the reading probe is lazy so non-writing commands do not spawn tools.

use std::{sync::OnceLock, time::Duration};

#[cfg(target_os = "macos")]
use std::{
    process::{Command, Stdio},
    thread,
    time::Instant,
};

use crate::native::{config::HighlightsConfig, env as bob_env};

pub(crate) const PDF_WRITE_BUDGET: Duration = Duration::from_secs(10);
pub(crate) const PDF_SCAN_BUDGET: Duration = Duration::from_secs(30);
pub(crate) const READING_WINDOW: Duration = Duration::from_secs(30);
#[cfg(target_os = "macos")]
const READING_PROBE_TIMEOUT: Duration = Duration::from_secs(2);
const ENV_PDF_WRITE_BUDGET: &str = "BOB_REF_PDF_WRITE_BUDGET_SECONDS";
const ENV_PDF_SCAN_BUDGET: &str = "BOB_REF_PDF_SCAN_BUDGET_SECONDS";
const ENV_READING_WINDOW: &str = "BOB_REF_READING_WINDOW_SECONDS";
const ENV_READING_PROBE: &str = "BOB_REF_READING_PROBE";
const ENV_FRONTMOST_BUNDLE_ID: &str = "BOB_REF_FRONTMOST_BUNDLE_ID";
const ENV_HID_IDLE_SECONDS: &str = "BOB_REF_HID_IDLE_SECONDS";
const HIGHLIGHTS_BUNDLE_PREFIX: &str = "net.highlightsapp.";

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ReadingActivity {
    Reading {
        idle: Duration,
    },
    Away {
        idle: Duration,
        front: Option<String>,
    },
    Unknown(String),
}

/// Whether this command may hold marker writes because Highlights is in use.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ReadingHoldMode {
    Evaluate,
    Exempt,
}

/// Immutable per-command write settings. The potentially slow machine probe
/// runs at most once, on first access.
pub(crate) struct PdfWritePolicy {
    pub(crate) coordinate_pdf_writes: bool,
    pub(crate) per_pdf_budget: Duration,
    pub(crate) scan_budget: Duration,
    pub(crate) reading_window: Duration,
    pub(crate) reading_hold: ReadingHoldMode,
    reading_activity: OnceLock<ReadingActivity>,
}

impl PdfWritePolicy {
    pub(crate) fn from_config(config: &HighlightsConfig) -> Self {
        Self {
            coordinate_pdf_writes: config.coordinate_pdf_writes(),
            per_pdf_budget: duration_override(
                ENV_PDF_WRITE_BUDGET,
                PDF_WRITE_BUDGET,
            ),
            scan_budget: duration_override(
                ENV_PDF_SCAN_BUDGET,
                PDF_SCAN_BUDGET,
            ),
            reading_window: duration_override(
                ENV_READING_WINDOW,
                READING_WINDOW,
            ),
            reading_hold: ReadingHoldMode::Evaluate,
            reading_activity: OnceLock::new(),
        }
    }

    pub(crate) fn with_reading_hold(mut self, mode: ReadingHoldMode) -> Self {
        self.reading_hold = mode;
        self
    }

    pub(crate) fn reading_activity(&self) -> &ReadingActivity {
        self.reading_activity.get_or_init(probe_reading_activity)
    }

    /// True when an unattended scan should skip this PDF because you are
    /// reading in Highlights. Unknown never holds.
    pub(crate) fn should_hold_for_reading(&self) -> bool {
        if !self.coordinate_pdf_writes
            || self.reading_hold != ReadingHoldMode::Evaluate
        {
            return false;
        }
        matches!(
            self.reading_activity(),
            ReadingActivity::Reading { idle } if *idle < self.reading_window
        )
    }
}

fn duration_override(name: &str, default: Duration) -> Duration {
    bob_env::var(name)
        .ok()
        .and_then(|value| value.trim().parse::<u64>().ok())
        .map(Duration::from_secs)
        .unwrap_or(default)
}

/// Extract the first bundle identifier emitted by `lsappinfo front`.
#[cfg_attr(not(any(test, target_os = "macos")), allow(dead_code))]
pub(crate) fn parse_frontmost_bundle_id(output: &str) -> Option<String> {
    let marker = "bundleID=";
    let start = output.find(marker)? + marker.len();
    let value = output[start..].trim_start();
    let bundle_id = if let Some(quoted) = value.strip_prefix('"') {
        let end = quoted.find('"')?;
        &quoted[..end]
    } else {
        let end = value
            .find(|character: char| {
                character.is_whitespace() || character == ','
            })
            .unwrap_or(value.len());
        &value[..end]
    };
    (!bundle_id.is_empty()).then(|| bundle_id.to_string())
}

/// Parse IOHIDSystem's nanosecond `HIDIdleTime` property into elapsed time.
#[cfg_attr(not(any(test, target_os = "macos")), allow(dead_code))]
pub(crate) fn parse_hid_idle_time(output: &str) -> Option<Duration> {
    let marker = "HIDIdleTime";
    let start = output.find(marker)? + marker.len();
    let equals = output[start..].find('=')? + start + 1;
    let value = output[equals..].trim_start();
    let end = value
        .find(|character: char| !character.is_ascii_digit())
        .unwrap_or(value.len());
    let nanos = value[..end].parse::<u128>().ok()?;
    Some(Duration::from_nanos(u64::try_from(nanos).ok()?))
}

fn probe_reading_activity() -> ReadingActivity {
    if let Ok(forced) = bob_env::var(ENV_READING_PROBE) {
        return match forced.trim().to_ascii_lowercase().as_str() {
            "reading" => ReadingActivity::Reading {
                idle: duration_override(ENV_HID_IDLE_SECONDS, Duration::ZERO),
            },
            "away" => ReadingActivity::Away {
                idle: duration_override(ENV_HID_IDLE_SECONDS, Duration::ZERO),
                front: None,
            },
            value => ReadingActivity::Unknown(format!(
                "invalid {ENV_READING_PROBE} override: {value}"
            )),
        };
    }

    if let (Ok(front), Ok(idle)) = (
        bob_env::var(ENV_FRONTMOST_BUNDLE_ID),
        bob_env::var(ENV_HID_IDLE_SECONDS),
    ) {
        let Some(idle) = idle.trim().parse::<u64>().ok() else {
            return ReadingActivity::Unknown(format!(
                "invalid {ENV_HID_IDLE_SECONDS}"
            ));
        };
        return activity_for(
            front.trim().to_string(),
            Duration::from_secs(idle),
        );
    }

    #[cfg(target_os = "macos")]
    {
        let front = run_bounded("/usr/bin/lsappinfo", &["front"])
            .and_then(|output| {
                parse_frontmost_bundle_id(&String::from_utf8_lossy(
                    &output.stdout,
                ))
            })
            .ok_or_else(|| {
                "could not determine frontmost application".to_string()
            });
        let idle =
            run_bounded("/usr/sbin/ioreg", &["-c", "IOHIDSystem", "-d", "4"])
                .and_then(|output| {
                    parse_hid_idle_time(&String::from_utf8_lossy(
                        &output.stdout,
                    ))
                })
                .ok_or_else(|| "could not determine HID idle time".to_string());
        match (front, idle) {
            (Ok(front), Ok(idle)) => activity_for(front, idle),
            (Err(error), _) | (_, Err(error)) => {
                ReadingActivity::Unknown(error)
            }
        }
    }

    #[cfg(not(target_os = "macos"))]
    {
        ReadingActivity::Away {
            idle: Duration::ZERO,
            front: None,
        }
    }
}

fn activity_for(front: String, idle: Duration) -> ReadingActivity {
    if front.starts_with(HIGHLIGHTS_BUNDLE_PREFIX) {
        ReadingActivity::Reading { idle }
    } else {
        ReadingActivity::Away {
            idle,
            front: Some(front),
        }
    }
}

#[cfg(target_os = "macos")]
fn run_bounded(program: &str, args: &[&str]) -> Option<std::process::Output> {
    let mut child = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let deadline = Instant::now() + READING_PROBE_TIMEOUT;
    loop {
        match child.try_wait().ok()? {
            Some(status) => {
                if !status.success() {
                    return None;
                }
                return child.wait_with_output().ok();
            }
            None if Instant::now() < deadline => {
                thread::sleep(Duration::from_millis(20));
            }
            None => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{ffi::OsStr, path::Path};

    use super::*;
    use crate::native::config::parse_highlights_config;

    #[test]
    fn parses_lsappinfo_front_output() {
        assert_eq!(
            parse_frontmost_bundle_id(
                "ASN:0x0-0x12345: PID:1234 bundleID=\"net.highlightsapp.Highlights\""
            ),
            Some("net.highlightsapp.Highlights".to_string())
        );
        assert_eq!(parse_frontmost_bundle_id("PID:1234 name=Terminal"), None);
    }

    #[test]
    fn parses_ioreg_idle_time_nanoseconds() {
        assert_eq!(
            parse_hid_idle_time("| |   \"HIDIdleTime\" = 1234567890\n"),
            Some(Duration::from_nanos(1_234_567_890))
        );
        assert_eq!(parse_hid_idle_time("HIDIdleTime = nope"), None);
    }

    #[test]
    fn probe_overrides_classify_frontmost_app_and_failures() {
        let _guard = crate::native::env::TestEnvGuard::set(&[
            (ENV_READING_PROBE, Some(OsStr::new("reading"))),
            (ENV_HID_IDLE_SECONDS, Some(OsStr::new("12"))),
        ]);
        assert_eq!(
            probe_reading_activity(),
            ReadingActivity::Reading {
                idle: Duration::from_secs(12)
            }
        );

        let _guard = crate::native::env::TestEnvGuard::set(&[(
            ENV_READING_PROBE,
            Some(OsStr::new("away")),
        )]);
        assert!(matches!(
            probe_reading_activity(),
            ReadingActivity::Away { .. }
        ));
    }

    #[test]
    fn policy_applies_budget_overrides_once_from_config() {
        let config = parse_highlights_config(
            "highlights:\n  coordinate_pdf_writes: true\n",
            Path::new("/config.yml"),
        )
        .expect("config parses");
        let _guard = crate::native::env::TestEnvGuard::set(&[
            (ENV_PDF_WRITE_BUDGET, Some(OsStr::new("7"))),
            (ENV_PDF_SCAN_BUDGET, Some(OsStr::new("8"))),
            (ENV_READING_WINDOW, Some(OsStr::new("9"))),
        ]);
        let policy = PdfWritePolicy::from_config(&config);
        assert!(policy.coordinate_pdf_writes);
        assert_eq!(policy.per_pdf_budget, Duration::from_secs(7));
        assert_eq!(policy.scan_budget, Duration::from_secs(8));
        assert_eq!(policy.reading_window, Duration::from_secs(9));
    }
}
