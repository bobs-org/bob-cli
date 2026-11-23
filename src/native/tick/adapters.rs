//! Per-step JSON adapters: parse each child's fresh stdout JSON.
use super::{catalog::Adapted, receipt::Outcome};
use crate::native::subprocess_runner::RawRunOutput;
use serde_json::Value;

fn stdout_value(output: &RawRunOutput) -> Option<Value> {
    let text = String::from_utf8_lossy(&output.stdout);
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return None;
    }
    serde_json::from_str(trimmed).ok()
}

fn child_message(value: &Value) -> Option<String> {
    if let Some(s) = value.get("message").and_then(Value::as_str)
        && !s.trim().is_empty()
    {
        return Some(s.trim().to_string());
    }
    if let Some(err) = value.get("error")
        && let Some(msg) = err.get("message").and_then(Value::as_str)
        && !msg.trim().is_empty()
    {
        return Some(msg.trim().to_string());
    }
    None
}

fn child_hint(value: &Value) -> Option<String> {
    if let Some(s) = value.get("hint").and_then(Value::as_str)
        && !s.trim().is_empty()
    {
        return Some(s.trim().to_string());
    }
    if let Some(err) = value.get("error")
        && let Some(hint) = err.get("hint").and_then(Value::as_str)
        && !hint.trim().is_empty()
    {
        return Some(hint.trim().to_string());
    }
    if let Some(waiting) = value.get("waiting")
        && let Some(detail) = waiting.get("detail").and_then(Value::as_str)
    {
        return Some(detail.to_string());
    }
    None
}

/// Combined message + hint detail for failed steps: the child's message on
/// the first line, followed by its hint on the second line when they differ.
/// For Gmail's `ok: false` document (no `error`, no top-level message), uses
/// the first `failure_kinds` entry plus the first warnings line. `run.rs`
/// already shows the first two detail lines under `│`.
fn child_detail(value: &Value) -> String {
    if let Some(message) = child_message(value)
        && let Some(hint) = child_hint(value)
        && hint != message
        && !hint.is_empty()
    {
        return format!("{message}\n{hint}");
    }
    if let Some(message) = child_message(value) {
        return message;
    }
    if let Some(hint) = child_hint(value) {
        return hint;
    }
    // Gmail's `ok: false` shape: no `error`, no top-level message/hint.
    let kind = value
        .get("failure_kinds")
        .and_then(Value::as_array)
        .and_then(|kinds| kinds.first())
        .and_then(Value::as_str)
        .unwrap_or_default();
    let warning = value
        .get("warnings")
        .and_then(Value::as_array)
        .and_then(|warnings| warnings.first())
        .and_then(Value::as_str)
        .unwrap_or_default();
    match (kind.is_empty(), warning.is_empty()) {
        (true, true) => String::new(),
        (false, true) => kind.to_string(),
        (true, false) => warning.to_string(),
        (false, false) => format!("{kind}\n{warning}"),
    }
}

fn child_code(value: &Value) -> Option<String> {
    if let Some(code) = value.get("code").and_then(Value::as_str) {
        return Some(code.to_string());
    }
    if let Some(err) = value.get("error")
        && let Some(code) = err.get("code").and_then(Value::as_str)
    {
        return Some(code.to_string());
    }
    if let Some(waiting) = value.get("waiting")
        && let Some(reason) = waiting.get("reason").and_then(Value::as_str)
    {
        return Some(reason.to_string());
    }
    None
}

/// All machine-readable failure kinds on a child document: `error.kind`,
/// top-level `kind`, and every `failure_kinds` entry.
fn transient_kinds(value: &Value) -> std::collections::BTreeSet<String> {
    let mut kinds = std::collections::BTreeSet::new();
    if let Some(kind) = value.get("kind").and_then(Value::as_str)
        && !kind.trim().is_empty()
    {
        kinds.insert(kind.trim().to_string());
    }
    if let Some(err) = value.get("error")
        && let Some(kind) = err.get("kind").and_then(Value::as_str)
        && !kind.trim().is_empty()
    {
        kinds.insert(kind.trim().to_string());
    }
    if let Some(list) = value.get("failure_kinds").and_then(Value::as_array) {
        for kind in list.iter().filter_map(Value::as_str) {
            if !kind.trim().is_empty() {
                kinds.insert(kind.trim().to_string());
            }
        }
    }
    kinds
}

/// Transient network kinds that map to an `offline` wait.
const TRANSIENT_KINDS: &[&str] =
    &["network", "timeout", "rate_limited", "transient"];

/// Shared contention mapping every adapter's nonzero path consults before
/// falling back to `Failed`: vault/scan lock timeouts report `vault_busy`
/// without consuming the interval, and a refs `inputs_changed` reports a
/// wait that consumes the refs interval (bounding CPU by one refs cadence).
/// Interrupts and timeouts are never contention and still fail.
fn contention_wait(output: &RawRunOutput) -> Option<Adapted> {
    if output.interrupted || output.timed_out {
        return None;
    }
    if output.status == Some(0) {
        return None;
    }
    let value = stdout_value(output)?;
    let code = child_code(&value)?;
    match code.as_str() {
        "lock_busy" | "scan_busy" => Some(Adapted {
            outcome: Outcome::Waiting,
            reason: "vault_busy".into(),
            summary: "vault busy · retrying next tick".into(),
            detail: "vault busy · retrying next tick".into(),
            consumes_interval: false,
        }),
        "inputs_changed" => Some(Adapted {
            outcome: Outcome::Waiting,
            reason: "inputs_changed".into(),
            summary: "notes changed during scan · retrying".into(),
            detail: "notes changed during scan · retrying".into(),
            consumes_interval: true,
        }),
        _ => None,
    }
}

/// Shared offline mapping every adapter's nonzero path consults after
/// `contention_wait`: when the child's typed kinds are non-empty and every
/// member is transient (`network`, `timeout`, `rate_limited`, `transient`),
/// the step waits as `offline` without consuming its interval (the
/// `offline_retry = next_tick` branch). `lock_busy` keeps precedence via
/// `contention_wait`; interrupts, tick's own child timeouts, and any
/// non-transient member still fail.
fn offline_wait(output: &RawRunOutput) -> Option<Adapted> {
    if output.interrupted || output.timed_out {
        return None;
    }
    if output.status == Some(0) {
        return None;
    }
    let value = stdout_value(output)?;
    let kinds = transient_kinds(&value);
    if kinds.is_empty() {
        return None;
    }
    if kinds
        .iter()
        .all(|kind| TRANSIENT_KINDS.contains(&kind.as_str()))
    {
        return Some(Adapted {
            outcome: Outcome::Waiting,
            reason: "offline".into(),
            summary: "offline · retrying next tick".into(),
            detail: "offline · retrying next tick".into(),
            consumes_interval: false,
        });
    }
    None
}

fn generic_ok(output: &RawRunOutput, reason: &str) -> Adapted {
    if output.interrupted {
        return Adapted {
            outcome: Outcome::Failed,
            reason: "interrupted".into(),
            summary: "interrupted".into(),
            detail: String::new(),
            consumes_interval: true,
        };
    }
    if output.timed_out {
        return Adapted {
            outcome: Outcome::Failed,
            reason: "timeout".into(),
            summary: "time limit exceeded".into(),
            detail: String::new(),
            consumes_interval: true,
        };
    }
    if output.status != Some(0) {
        if let Some(wait) = contention_wait(output) {
            return wait;
        }
        if let Some(wait) = offline_wait(output) {
            return wait;
        }
        let summary = format!(
            "exit {}",
            output
                .status
                .map_or_else(|| "signal".into(), |n| n.to_string())
        );
        if let Some(value) = stdout_value(output) {
            let code = child_code(&value).unwrap_or_default();
            let mut summary = summary;
            if !code.is_empty() {
                summary = format!("{summary} · {code}");
            }
            let detail = child_detail(&value);
            if !detail.is_empty() {
                for line in detail.lines().take(2) {
                    eprintln!("bob tick: warning: {line}");
                }
            }
            return Adapted {
                outcome: Outcome::Failed,
                reason: "exit_nonzero".into(),
                summary,
                detail,
                consumes_interval: true,
            };
        }
        return Adapted {
            outcome: Outcome::Failed,
            reason: "exit_nonzero".into(),
            summary,
            detail: String::new(),
            consumes_interval: true,
        };
    }
    match stdout_value(output) {
        None => {
            // Truly malformed (or empty) JSON with exit 0.
            let text = String::from_utf8_lossy(&output.stdout);
            if text.trim().is_empty() || text.trim() == "{}" {
                return Adapted {
                    outcome: Outcome::Ok,
                    reason: reason.into(),
                    summary: "done".into(),
                    detail: String::new(),
                    consumes_interval: true,
                };
            }
            eprintln!("bob tick: warning: output was not JSON; treating as ok");
            Adapted {
                outcome: Outcome::Ok,
                reason: "unreadable_json".into(),
                summary: "done (unreadable JSON)".into(),
                detail: "stdout was not JSON".into(),
                consumes_interval: true,
            }
        }
        Some(_) => Adapted {
            outcome: Outcome::Ok,
            reason: reason.into(),
            summary: "done".into(),
            detail: String::new(),
            consumes_interval: true,
        },
    }
}

fn counts(parts: Vec<(usize, &str, &str)>) -> String {
    let mut out: Vec<String> = Vec::new();
    for (n, singular, plural) in parts {
        if n == 0 {
            continue;
        }
        if n == 1 {
            out.push(format!("1 {singular}"));
        } else {
            out.push(format!("{n} {plural}"));
        }
    }
    if out.is_empty() {
        "no changes".into()
    } else {
        out.join(" · ")
    }
}

fn get_u64(value: &Value, path: &[&str]) -> usize {
    let mut cur = value;
    for key in path {
        match cur.get(*key) {
            Some(next) => cur = next,
            None => return 0,
        }
    }
    cur.as_u64().unwrap_or(0) as usize
}

pub(super) fn gcal(output: &RawRunOutput, reason: &str) -> Adapted {
    if output.interrupted || output.timed_out || output.status != Some(0) {
        if let Some(wait) = contention_wait(output) {
            return wait;
        }
        if let Some(wait) = offline_wait(output) {
            return wait;
        }
        return generic_ok(output, reason);
    }
    let Some(value) = stdout_value(output) else {
        let text = String::from_utf8_lossy(&output.stdout);
        if text.trim().is_empty() || text.trim() == "{}" {
            return Adapted {
                outcome: Outcome::Ok,
                reason: reason.into(),
                summary: "done".into(),
                detail: String::new(),
                consumes_interval: true,
            };
        }
        eprintln!("bob tick: warning: gcal output was not JSON");
        return Adapted {
            outcome: Outcome::Ok,
            reason: "unreadable_json".into(),
            summary: "done (unreadable JSON)".into(),
            detail: "stdout was not JSON".into(),
            consumes_interval: true,
        };
    };
    // Multi-day envelope: {"days": [...]}.
    if let Some(days) = value.get("days").and_then(Value::as_array) {
        let mut waiting_detail: Option<String> = None;
        let mut added = 0;
        let mut refiled = 0;
        let mut closed = 0;
        for day in days {
            if day.get("status").and_then(Value::as_str) == Some("waiting") {
                let detail = day
                    .get("waiting")
                    .and_then(|w| w.get("detail"))
                    .and_then(Value::as_str)
                    .unwrap_or("waiting")
                    .to_string();
                waiting_detail = Some(detail);
            }
            if let Some(pom) = day.get("pomodoro") {
                added +=
                    pom.get("links_added").and_then(Value::as_u64).unwrap_or(0)
                        as usize;
                refiled += pom
                    .get("links_refiled")
                    .and_then(Value::as_u64)
                    .unwrap_or(0) as usize;
                closed += pom
                    .get("links_closed")
                    .and_then(Value::as_u64)
                    .unwrap_or(0) as usize;
            }
        }
        if let Some(detail) = waiting_detail {
            return Adapted {
                outcome: Outcome::Waiting,
                reason: "waiting".into(),
                summary: detail.clone(),
                detail,
                consumes_interval: true,
            };
        }
        let mut summary = counts(vec![
            (added, "added", "added"),
            (refiled, "refiled", "refiled"),
            (closed, "closed", "closed"),
        ]);
        if days.len() == 2 {
            summary = format!("yesterday + today · {summary}");
        } else {
            summary = format!("today · {summary}");
        }
        return Adapted {
            outcome: Outcome::Ok,
            reason: reason.into(),
            summary,
            detail: String::new(),
            consumes_interval: true,
        };
    }
    // Single-day shape.
    if value.get("status").and_then(Value::as_str) == Some("waiting") {
        let detail = value
            .get("waiting")
            .and_then(|w| w.get("detail"))
            .and_then(Value::as_str)
            .unwrap_or("waiting")
            .to_string();
        return Adapted {
            outcome: Outcome::Waiting,
            reason: value
                .get("waiting")
                .and_then(|w| w.get("reason"))
                .and_then(Value::as_str)
                .unwrap_or("waiting")
                .to_string(),
            summary: detail.clone(),
            detail,
            consumes_interval: true,
        };
    }
    let pom = value.get("pomodoro");
    let added = pom
        .and_then(|p| p.get("links_added"))
        .and_then(Value::as_u64)
        .unwrap_or(0) as usize;
    let refiled = pom
        .and_then(|p| p.get("links_refiled"))
        .and_then(Value::as_u64)
        .unwrap_or(0) as usize;
    let closed = pom
        .and_then(|p| p.get("links_closed"))
        .and_then(Value::as_u64)
        .unwrap_or(0) as usize;
    if value.get("day").is_none() && pom.is_none() {
        return Adapted {
            outcome: Outcome::Ok,
            reason: reason.into(),
            summary: "done".into(),
            detail: String::new(),
            consumes_interval: true,
        };
    }
    Adapted {
        outcome: Outcome::Ok,
        reason: reason.into(),
        summary: format!(
            "today · {}",
            counts(vec![
                (added, "added", "added"),
                (refiled, "refiled", "refiled"),
                (closed, "closed", "closed"),
            ])
        ),
        detail: String::new(),
        consumes_interval: true,
    }
}

pub(super) fn gmail(output: &RawRunOutput, reason: &str) -> Adapted {
    if output.interrupted || output.timed_out {
        return generic_ok(output, reason);
    }
    if output.status != Some(0) {
        if let Some(wait) = contention_wait(output) {
            return wait;
        }
        if let Some(wait) = offline_wait(output) {
            return wait;
        }
        return generic_ok(output, reason);
    }
    let Some(value) = stdout_value(output) else {
        let text = String::from_utf8_lossy(&output.stdout);
        if text.trim().is_empty() || text.trim() == "{}" {
            return Adapted {
                outcome: Outcome::Ok,
                reason: reason.into(),
                summary: "done".into(),
                detail: String::new(),
                consumes_interval: true,
            };
        }
        eprintln!("bob tick: warning: gmail output was not JSON");
        return Adapted {
            outcome: Outcome::Ok,
            reason: "unreadable_json".into(),
            summary: "done (unreadable JSON)".into(),
            detail: "stdout was not JSON".into(),
            consumes_interval: true,
        };
    };
    if value.get("pulled").is_none() && value.get("busy").is_none() {
        return Adapted {
            outcome: Outcome::Ok,
            reason: reason.into(),
            summary: "done".into(),
            detail: String::new(),
            consumes_interval: true,
        };
    }
    let busy = value.get("busy").and_then(Value::as_bool).unwrap_or(false);
    let ok = value.get("ok").and_then(Value::as_bool).unwrap_or(true);
    if busy && ok {
        return Adapted {
            outcome: Outcome::Waiting,
            reason: "vault_busy".into(),
            summary: "vault busy · retrying next tick".into(),
            detail: "vault busy · retrying next tick".into(),
            consumes_interval: false,
        };
    }
    let filed = get_u64(&value, &["pulled"]);
    let watching = get_u64(&value, &["summary", "watching"]);
    let awaiting = get_u64(&value, &["summary", "awaiting"]);
    let archived = get_u64(&value, &["summary", "archived"]);
    Adapted {
        outcome: Outcome::Ok,
        reason: reason.into(),
        summary: counts(vec![
            (filed, "filed", "filed"),
            (watching, "watching", "watching"),
            (awaiting, "awaiting", "awaiting"),
            (archived, "archived", "archived"),
        ]),
        detail: String::new(),
        consumes_interval: true,
    }
}

pub(super) fn gkeep(output: &RawRunOutput, reason: &str) -> Adapted {
    if output.interrupted || output.timed_out || output.status != Some(0) {
        if let Some(wait) = contention_wait(output) {
            return wait;
        }
        if let Some(wait) = offline_wait(output) {
            return wait;
        }
        return generic_ok(output, reason);
    }
    let Some(value) = stdout_value(output) else {
        let text = String::from_utf8_lossy(&output.stdout);
        if text.trim().is_empty() || text.trim() == "{}" {
            return Adapted {
                outcome: Outcome::Ok,
                reason: reason.into(),
                summary: "done".into(),
                detail: String::new(),
                consumes_interval: true,
            };
        }
        eprintln!("bob tick: warning: gkeep output was not JSON");
        return Adapted {
            outcome: Outcome::Ok,
            reason: "unreadable_json".into(),
            summary: "done (unreadable JSON)".into(),
            detail: "stdout was not JSON".into(),
            consumes_interval: true,
        };
    };
    let written = get_u64(&value, &["summary", "written"]);
    if value.get("summary").is_none() {
        return Adapted {
            outcome: Outcome::Ok,
            reason: reason.into(),
            summary: "done".into(),
            detail: String::new(),
            consumes_interval: true,
        };
    }
    Adapted {
        outcome: Outcome::Ok,
        reason: reason.into(),
        summary: counts(vec![(written, "note pulled", "notes pulled")]),
        detail: String::new(),
        consumes_interval: true,
    }
}

pub(super) fn refs(output: &RawRunOutput, reason: &str) -> Adapted {
    if output.interrupted || output.timed_out || output.status != Some(0) {
        if let Some(wait) = contention_wait(output) {
            return wait;
        }
        if let Some(wait) = offline_wait(output) {
            return wait;
        }
        return generic_ok(output, reason);
    }
    let Some(value) = stdout_value(output) else {
        let text = String::from_utf8_lossy(&output.stdout);
        if text.trim().is_empty() || text.trim() == "{}" {
            return Adapted {
                outcome: Outcome::Ok,
                reason: reason.into(),
                summary: "done".into(),
                detail: String::new(),
                consumes_interval: true,
            };
        }
        eprintln!("bob tick: warning: refs output was not JSON");
        return Adapted {
            outcome: Outcome::Ok,
            reason: "unreadable_json".into(),
            summary: "done (unreadable JSON)".into(),
            detail: "stdout was not JSON".into(),
            consumes_interval: true,
        };
    };
    let summary_obj = value.get("summary");
    if summary_obj.is_none() {
        return Adapted {
            outcome: Outcome::Ok,
            reason: reason.into(),
            summary: "done".into(),
            detail: String::new(),
            consumes_interval: true,
        };
    }
    // `summary.pdfs` counts every PDF the full scan visited, so a changed PDF
    // is one whose note was not `unchanged` and that neither failed nor was held.
    let pdfs = get_u64(&value, &["summary", "pdfs"]);
    let created = get_u64(&value, &["summary", "created"]);
    let updated = get_u64(&value, &["summary", "updated"]);
    let unchanged = get_u64(&value, &["summary", "unchanged"]);
    let markers = get_u64(&value, &["summary", "markers"]);
    let tasks = get_u64(&value, &["summary", "tasks"]);
    let failures = get_u64(&value, &["summary", "failures"]);
    let held = get_u64(&value, &["summary", "held"]);
    let changed = pdfs.saturating_sub(unchanged + failures + held);
    let mut summary = counts(vec![
        (changed, "PDF changed", "PDFs changed"),
        (created + updated, "note updated", "notes updated"),
        (markers, "marker", "markers"),
        (tasks, "task", "tasks"),
        (failures, "failure", "failures"),
    ]);
    if held > 0 {
        if summary == "no changes" {
            summary = format!("{held} held");
        } else {
            summary = format!("{summary} · {held} held");
        }
    }
    Adapted {
        outcome: Outcome::Ok,
        reason: reason.into(),
        summary,
        detail: String::new(),
        consumes_interval: true,
    }
}

pub(super) fn projects(output: &RawRunOutput, reason: &str) -> Adapted {
    if output.interrupted || output.timed_out || output.status != Some(0) {
        if let Some(wait) = contention_wait(output) {
            return wait;
        }
        if let Some(wait) = offline_wait(output) {
            return wait;
        }
        return generic_ok(output, reason);
    }
    let Some(value) = stdout_value(output) else {
        let text = String::from_utf8_lossy(&output.stdout);
        if text.trim().is_empty() || text.trim() == "{}" {
            return Adapted {
                outcome: Outcome::Ok,
                reason: reason.into(),
                summary: "done".into(),
                detail: String::new(),
                consumes_interval: true,
            };
        }
        eprintln!("bob tick: warning: projects output was not JSON");
        return Adapted {
            outcome: Outcome::Ok,
            reason: "unreadable_json".into(),
            summary: "done (unreadable JSON)".into(),
            detail: "stdout was not JSON".into(),
            consumes_interval: true,
        };
    };
    let changed = value
        .get("changed")
        .and_then(Value::as_array)
        .map(|a| a.len())
        .unwrap_or(0);
    if value.get("changed").is_none() {
        return Adapted {
            outcome: Outcome::Ok,
            reason: reason.into(),
            summary: "done".into(),
            detail: String::new(),
            consumes_interval: true,
        };
    }
    Adapted {
        outcome: Outcome::Ok,
        reason: reason.into(),
        summary: counts(vec![(changed, "note changed", "notes changed")]),
        detail: String::new(),
        consumes_interval: true,
    }
}

pub(super) fn tasks(output: &RawRunOutput, reason: &str) -> Adapted {
    if output.interrupted || output.timed_out || output.status != Some(0) {
        if let Some(wait) = contention_wait(output) {
            return wait;
        }
        if let Some(wait) = offline_wait(output) {
            return wait;
        }
        return generic_ok(output, reason);
    }
    let Some(value) = stdout_value(output) else {
        let text = String::from_utf8_lossy(&output.stdout);
        if text.trim().is_empty() || text.trim() == "{}" {
            return Adapted {
                outcome: Outcome::Ok,
                reason: reason.into(),
                summary: "done".into(),
                detail: String::new(),
                consumes_interval: true,
            };
        }
        eprintln!("bob tick: warning: tasks output was not JSON");
        return Adapted {
            outcome: Outcome::Ok,
            reason: "unreadable_json".into(),
            summary: "done (unreadable JSON)".into(),
            detail: "stdout was not JSON".into(),
            consumes_interval: true,
        };
    };
    if value.get("marked_next").is_none() && value.get("ok").is_none() {
        return Adapted {
            outcome: Outcome::Ok,
            reason: reason.into(),
            summary: "done".into(),
            detail: String::new(),
            consumes_interval: true,
        };
    }
    let mut total = 0;
    for key in [
        "marked_next",
        "marked_in_progress",
        "cleared",
        "marked_blocked",
        "unblocked",
        "struck_completed_references",
        "moved_completed_references",
        "removed_canceled_references",
        "consumed_blocked_references",
        "removed_duplicate_lines",
        "dependency_projection_updates",
        "adopted_dependency_lines",
        "healed_dependency_links",
        "canonicalized_dependency_lines",
        "blocks_line_updates",
        "settled",
        "coalesced_pomodoros",
    ] {
        total += value
            .get(key)
            .and_then(Value::as_array)
            .map(|a| a.len())
            .unwrap_or(0);
    }
    Adapted {
        outcome: Outcome::Ok,
        reason: reason.into(),
        summary: counts(vec![(total, "status change", "status changes")]),
        detail: String::new(),
        consumes_interval: true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::native::subprocess_runner::RawRunOutput;

    fn failed_with(code: &str) -> RawRunOutput {
        RawRunOutput {
            status: Some(1),
            stdout: serde_json::json!({
                "ok": false,
                "error": {
                    "code": code,
                    "message": format!("child failed with {code}"),
                },
            })
            .to_string()
            .into_bytes(),
            stderr: Vec::new(),
            timed_out: false,
            interrupted: false,
            stdout_truncated: false,
            output_incomplete: false,
        }
    }

    fn failed_with_top_code(code: &str) -> RawRunOutput {
        RawRunOutput {
            status: Some(1),
            stdout: serde_json::json!({
                "ok": false,
                "code": code,
                "message": format!("child failed with {code}"),
            })
            .to_string()
            .into_bytes(),
            stderr: Vec::new(),
            timed_out: false,
            interrupted: false,
            stdout_truncated: false,
            output_incomplete: false,
        }
    }

    #[test]
    fn lock_busy_maps_to_vault_busy_wait_without_consuming_interval() {
        for code in ["lock_busy", "scan_busy"] {
            for output in [failed_with(code), failed_with_top_code(code)] {
                for adapted in [
                    generic_ok(&output, "due"),
                    gcal(&output, "due"),
                    gmail(&output, "due"),
                    gkeep(&output, "due"),
                    refs(&output, "due"),
                    projects(&output, "due"),
                    tasks(&output, "due"),
                ] {
                    assert_eq!(adapted.outcome, Outcome::Waiting, "{code}");
                    assert_eq!(adapted.reason, "vault_busy", "{code}");
                    assert_eq!(
                        adapted.summary, "vault busy · retrying next tick",
                        "{code}"
                    );
                    assert!(!adapted.consumes_interval, "{code}");
                }
            }
        }
    }

    #[test]
    fn inputs_changed_maps_to_wait_that_consumes_interval() {
        for output in [
            failed_with("inputs_changed"),
            failed_with_top_code("inputs_changed"),
        ] {
            for adapted in [
                generic_ok(&output, "due"),
                gcal(&output, "due"),
                refs(&output, "due"),
                projects(&output, "due"),
                tasks(&output, "due"),
                gmail(&output, "due"),
                gkeep(&output, "due"),
            ] {
                assert_eq!(adapted.outcome, Outcome::Waiting);
                assert_eq!(adapted.reason, "inputs_changed");
                assert_eq!(
                    adapted.summary,
                    "notes changed during scan · retrying"
                );
                assert!(adapted.consumes_interval);
            }
        }
    }

    #[test]
    fn interrupts_and_timeouts_still_fail() {
        let interrupted = RawRunOutput {
            status: Some(1),
            stdout: serde_json::json!({
                "ok": false,
                "error": {"code": "lock_busy", "message": "busy"},
            })
            .to_string()
            .into_bytes(),
            stderr: Vec::new(),
            timed_out: false,
            interrupted: true,
            stdout_truncated: false,
            output_incomplete: false,
        };
        let timed_out = RawRunOutput {
            status: Some(1),
            stdout: serde_json::json!({
                "ok": false,
                "error": {"code": "inputs_changed", "message": "busy"},
            })
            .to_string()
            .into_bytes(),
            stderr: Vec::new(),
            timed_out: true,
            interrupted: false,
            stdout_truncated: false,
            output_incomplete: false,
        };
        for output in [&interrupted, &timed_out] {
            for adapted in [
                generic_ok(output, "due"),
                gcal(output, "due"),
                refs(output, "due"),
                projects(output, "due"),
            ] {
                assert_eq!(adapted.outcome, Outcome::Failed);
            }
        }
        assert_eq!(generic_ok(&interrupted, "due").reason, "interrupted");
        assert_eq!(generic_ok(&timed_out, "due").reason, "timeout");
    }

    #[test]
    fn other_codes_still_fail() {
        let output = failed_with("sync_failed");
        let adapted = generic_ok(&output, "due");
        assert_eq!(adapted.outcome, Outcome::Failed);
        assert_eq!(adapted.reason, "exit_nonzero");
    }

    fn failed_with_kind(kind: &str) -> RawRunOutput {
        RawRunOutput {
            status: Some(1),
            stdout: serde_json::json!({
                "ok": false,
                "error": {
                    "code": "sync_failed",
                    "kind": kind,
                    "message": format!("remote unreachable: {kind}"),
                    "hint": "the vault remote is unreachable; the next run retries",
                },
            })
            .to_string()
            .into_bytes(),
            stderr: Vec::new(),
            timed_out: false,
            interrupted: false,
            stdout_truncated: false,
            output_incomplete: false,
        }
    }

    #[test]
    fn each_transient_kind_maps_to_offline_without_consuming_interval() {
        for kind in ["network", "timeout", "rate_limited", "transient"] {
            let output = failed_with_kind(kind);
            for adapted in [
                generic_ok(&output, "due"),
                gcal(&output, "due"),
                gmail(&output, "due"),
                gkeep(&output, "due"),
                refs(&output, "due"),
                projects(&output, "due"),
                tasks(&output, "due"),
            ] {
                assert_eq!(adapted.outcome, Outcome::Waiting, "{kind}");
                assert_eq!(adapted.reason, "offline", "{kind}");
                assert_eq!(
                    adapted.summary, "offline · retrying next tick",
                    "{kind}"
                );
                assert!(!adapted.consumes_interval, "{kind}");
            }
        }
    }

    #[test]
    fn mixed_and_untyped_kinds_still_fail() {
        let mixed = RawRunOutput {
            status: Some(1),
            stdout: serde_json::json!({
                "ok": false,
                "failure_kinds": ["network", "route_problem"],
                "warnings": ["skipped Gmail thread aa01: network: down"],
            })
            .to_string()
            .into_bytes(),
            stderr: Vec::new(),
            timed_out: false,
            interrupted: false,
            stdout_truncated: false,
            output_incomplete: false,
        };
        assert_eq!(generic_ok(&mixed, "due").outcome, Outcome::Failed);
        let untyped = failed_with("sync_failed");
        assert_eq!(generic_ok(&untyped, "due").outcome, Outcome::Failed);
    }

    #[test]
    fn offline_yields_to_interrupts_timeouts_and_lock_busy() {
        let with_timeout = RawRunOutput {
            status: Some(1),
            stdout: serde_json::json!({
                "ok": false,
                "error": {"code": "sync_failed", "kind": "network", "message": "down"},
            })
            .to_string()
            .into_bytes(),
            stderr: Vec::new(),
            timed_out: true,
            interrupted: false,
            stdout_truncated: false,
            output_incomplete: false,
        };
        assert_eq!(generic_ok(&with_timeout, "due").outcome, Outcome::Failed);
        let with_interrupt = RawRunOutput {
            status: Some(1),
            stdout: serde_json::json!({
                "ok": false,
                "error": {"code": "sync_failed", "kind": "network", "message": "down"},
            })
            .to_string()
            .into_bytes(),
            stderr: Vec::new(),
            timed_out: false,
            interrupted: true,
            stdout_truncated: false,
            output_incomplete: false,
        };
        assert_eq!(generic_ok(&with_interrupt, "due").outcome, Outcome::Failed);
        let both = RawRunOutput {
            status: Some(1),
            stdout: serde_json::json!({
                "ok": false,
                "code": "lock_busy",
                "error": {"kind": "network", "message": "down"},
            })
            .to_string()
            .into_bytes(),
            stderr: Vec::new(),
            timed_out: false,
            interrupted: false,
            stdout_truncated: false,
            output_incomplete: false,
        };
        let adapted = generic_ok(&both, "due");
        assert_eq!(adapted.outcome, Outcome::Waiting);
        assert_eq!(adapted.reason, "vault_busy");
    }

    #[test]
    fn failure_detail_carries_message_and_hint() {
        let output = RawRunOutput {
            status: Some(1),
            stdout: serde_json::json!({
                "ok": false,
                "error": {
                    "code": "sync_failed",
                    "kind": "auth",
                    "message": "remote unreachable: auth",
                    "hint": "run bob login again",
                },
            })
            .to_string()
            .into_bytes(),
            stderr: Vec::new(),
            timed_out: false,
            interrupted: false,
            stdout_truncated: false,
            output_incomplete: false,
        };
        let adapted = generic_ok(&output, "due");
        assert_eq!(adapted.outcome, Outcome::Failed);
        assert!(
            adapted.detail.contains("remote unreachable"),
            "{}",
            adapted.detail
        );
        assert!(
            adapted.detail.contains("run bob login again"),
            "{}",
            adapted.detail
        );
        let gmail_like = RawRunOutput {
            status: Some(1),
            stdout: serde_json::json!({
                "ok": false,
                "failure_kinds": ["network"],
                "warnings": ["skipped Gmail thread aa01: network: gog could not reach Google"],
            })
            .to_string()
            .into_bytes(),
            stderr: Vec::new(),
            timed_out: false,
            interrupted: false,
            stdout_truncated: false,
            output_incomplete: false,
        };
        // Gmail network documents wait, but a mixed Gmail failure names its
        // cause through the same detail path when it fails.
        let mixed = RawRunOutput {
            status: Some(1),
            stdout: serde_json::json!({
                "ok": false,
                "failure_kinds": ["network", "route_problem"],
                "warnings": ["skipped Gmail thread aa01: network: gog could not reach Google"],
            })
            .to_string()
            .into_bytes(),
            stderr: Vec::new(),
            timed_out: false,
            interrupted: false,
            stdout_truncated: false,
            output_incomplete: false,
        };
        let failed = generic_ok(&mixed, "due");
        assert_eq!(failed.outcome, Outcome::Failed);
        assert!(failed.detail.contains("network"), "{}", failed.detail);
        let _ = generic_ok(&gmail_like, "due");
    }
}
