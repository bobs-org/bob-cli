//! Copy builder for the Mail Close Card.
//!
//! Every surface renders the same strings. The table in
//! `docs/mail-close-card.md` is authoritative; this module is its Rust
//! spelling. Field rules:
//!
//! - `title`: one target → `Complete/Cancel this email task?`; batch →
//!   `Complete/Cancel N tasks?`, or `Close N tasks?` when verbs mix.
//! - `subtitle`: one target → `{path} · {Before} → {After}` with status
//!   names; batch → `N tasks · K wired to email` where `K` is the distinct
//!   mail-note count.
//! - Row `label`: `✉️ {mail title} · {who}`; the task text when the mail
//!   title is missing. `who` is the mail note's `who` (first line,
//!   `::` folded to `:`); the `· {who}` tail is omitted when `who` is
//!   empty.
//! - `turn_line`: yours → `↩ {who} · {age} — waiting on your reply`
//!   (`who` omitted when unknown, exactly like the reply badge); theirs →
//!   `✈ You wrote {Sat, Nov 7} — waiting on {who}` (date from `turn_since`
//!   via `%a, %b %-d`; the `— waiting on {who}` tail is omitted when `who`
//!   is empty; when `turn_since` is missing the line reads
//!   `✈ Waiting on {who}`, or is omitted when `who` is also missing).
//!   Unknown turn → no line. Ages are `now` (<60s), `Nm` (<60m), `Nh`
//!   (<24h), `Nd` (whole days below), exactly the reply badge buckets.
//!   When a task links several mails, the first `yours` mail wins,
//!   otherwise the first mail.
//! - `tone`: `warning` when any orphaned row's turn is `yours`;
//!   otherwise `calm`.
//! - `consequence`: no orphaned rows → `Another open task still watches
//!   this thread.` (`these threads` when several distinct threads are
//!   involved). Orphaned with a `yours` turn → `{who} is waiting on your
//!   reply, and nothing reopens this task until they write again.`
//!   (empty `who` reads `They are …`; several orphaned threads read
//!   `{K} threads are waiting on your reply, and nothing reopens these
//!   tasks until they write again.`). Orphaned with no `yours` but a
//!   `theirs` turn → `A reply from {who} reopens it.` (empty `who` reads
//!   `A reply reopens it.`; several orphaned threads read `A reply
//!   reopens them.`). Orphaned and otherwise unknown → `Bob stops
//!   watching this thread.` (`these threads` for several). When
//!   `gmail.sync.archive` is on and any row is orphaned, append
//!   ` Bob's archive-on-close can then clear it from your Gmail inbox.`
//!   (`them … inboxes` is `them from your Gmail inbox` for several).

use super::predicate::MailNoteFacts;
use chrono::{DateTime, NaiveDateTime, Utc};

/// Complete vs cancel, derived from the closing checkbox.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CloseVerb {
    Complete,
    Cancel,
}

/// One task the gesture would close, with its linked mail notes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CloseTarget {
    pub(crate) path: String,
    pub(crate) line: usize,
    pub(crate) before: char,
    pub(crate) after: char,
    /// Raw task row text, used when the mail title is missing.
    pub(crate) task_text: String,
    pub(crate) mails: Vec<MailNoteFacts>,
}

/// One rendered email row (at most five; the rest fold into `more`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CopyRow {
    pub(crate) path: String,
    pub(crate) line: usize,
    pub(crate) label: String,
    pub(crate) turn_line: Option<String>,
    pub(crate) note: String,
}

/// The shared copy plus tone, rows, and consequence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct MailCloseCopy {
    pub(crate) title: String,
    pub(crate) subtitle: String,
    pub(crate) tone: String,
    pub(crate) verb: String,
    pub(crate) rows: Vec<CopyRow>,
    pub(crate) more: usize,
    pub(crate) consequence: String,
}

/// The closing verb for `after`: `x`/`X` complete, `-` cancel.
pub(crate) fn close_verb(after: char) -> Option<CloseVerb> {
    match after {
        'x' | 'X' => Some(CloseVerb::Complete),
        '-' => Some(CloseVerb::Cancel),
        _ => None,
    }
}

/// Display name for a checkbox status.
pub(crate) fn status_name(status: char) -> &'static str {
    match status {
        ' ' => "Ready",
        '*' => "Next",
        '/' => "In Progress",
        '?' => "Blocked",
        'x' | 'X' => "Done",
        '-' => "Cancelled",
        _ => "Task",
    }
}

/// First line, trimmed, with `::` folded to `:` (the reply-badge rule).
pub(crate) fn normalize_who(who: &str) -> String {
    who.lines()
        .next()
        .unwrap_or("")
        .trim()
        .replace("::", ":")
        .trim()
        .to_string()
}

/// Task text for the label fallback: strip the `- [s]` prefix and every
/// `#task` token, then trim. Block ids stay; they disambiguate duplicates.
pub(crate) fn task_text(line: &str) -> String {
    let after_box = line
        .find(']')
        .map(|close| line[close + 1..].trim_start().to_string())
        .unwrap_or_else(|| line.trim().to_string());
    after_box
        .split_whitespace()
        .filter(|token| *token != "#task")
        .collect::<Vec<_>>()
        .join(" ")
}

fn chosen_mail(mails: &[MailNoteFacts]) -> Option<&MailNoteFacts> {
    mails
        .iter()
        .find(|mail| mail.turn.as_deref() == Some("yours"))
        .or_else(|| mails.first())
}

pub(crate) fn row_label(
    mail: &MailNoteFacts,
    fallback_task_text: &str,
) -> String {
    let title = mail.title.trim();
    let who = normalize_who(&mail.who);
    let base = if title.is_empty() {
        fallback_task_text.trim().to_string()
    } else {
        format!("✉️ {title}")
    };
    if base.is_empty() {
        return if who.is_empty() {
            "✉️ Email task".to_string()
        } else {
            format!("✉️ {who}")
        };
    }
    if who.is_empty() {
        base
    } else {
        format!("{base} · {who}")
    }
}

fn parse_moment(raw: &str) -> Option<DateTime<Utc>> {
    let text = raw.trim();
    if text.is_empty() {
        return None;
    }
    if let Ok(moment) = text.parse::<DateTime<Utc>>() {
        return Some(moment);
    }
    if let Ok(naive) = text.parse::<NaiveDateTime>() {
        return Some(naive.and_utc());
    }
    if let Ok(date) = chrono::NaiveDate::parse_from_str(text, "%Y-%m-%d") {
        return Some(date.and_hms_opt(0, 0, 0)?.and_utc());
    }
    None
}

/// Reply-badge age: `now`, `Nm`, `Nh`, `Nd`. Unknown or unparseable
/// moments read as `now` is never guessed: callers omit the turn line.
fn format_age_between(since: DateTime<Utc>, now: DateTime<Utc>) -> String {
    let seconds = now.signed_duration_since(since).num_seconds().max(0);
    if seconds < 60 {
        "now".to_string()
    } else if seconds < 60 * 60 {
        format!("{}m", seconds / 60)
    } else if seconds < 24 * 60 * 60 {
        format!("{}h", seconds / (60 * 60))
    } else {
        format!("{}d", seconds / (24 * 60 * 60))
    }
}

pub(crate) fn format_age(
    turn_since: Option<&str>,
    now: &str,
) -> Option<String> {
    let since = parse_moment(turn_since?)?;
    let at = parse_moment(now)?;
    Some(format_age_between(since, at))
}

pub(crate) fn format_short_date(turn_since: Option<&str>) -> Option<String> {
    let moment = parse_moment(turn_since?)?;
    Some(moment.format("%a, %b %-d").to_string())
}

/// The turn line for one mail, or `None` for unknown turn.
pub(crate) fn turn_line(mail: &MailNoteFacts, now: &str) -> Option<String> {
    match mail.turn.as_deref() {
        Some("yours") => {
            let age = format_age(mail.turn_since.as_deref(), now)?;
            let who = normalize_who(&mail.who);
            if who.is_empty() {
                Some(format!("↩ {age} — waiting on your reply"))
            } else {
                Some(format!("↩ {who} · {age} — waiting on your reply"))
            }
        }
        Some("theirs") => {
            let who = normalize_who(&mail.who);
            match (
                format_short_date(mail.turn_since.as_deref()),
                who.is_empty(),
            ) {
                (Some(date), false) => Some(format!(
                    "✈ You wrote {date} — waiting on {}",
                    normalize_who(&mail.who)
                )),
                (Some(date), true) => Some(format!("✈ You wrote {date}")),
                (None, false) => {
                    Some(format!("✈ Waiting on {}", normalize_who(&mail.who)))
                }
                (None, true) => None,
            }
        }
        _ => None,
    }
}

/// Build the shared copy. `other_open_linkers` maps a mail-note path to
/// the count of other open tasks linking it (excluding the targets being
/// closed); zero means the row is orphaned.
pub(crate) fn build_copy(
    targets: &[CloseTarget],
    now: &str,
    other_open_linkers: &dyn Fn(&str) -> usize,
    archive_on: bool,
) -> MailCloseCopy {
    let verbs: Vec<CloseVerb> = targets
        .iter()
        .filter_map(|target| close_verb(target.after))
        .collect();
    let complete = verbs
        .iter()
        .filter(|verb| **verb == CloseVerb::Complete)
        .count();
    let cancel = verbs.len().saturating_sub(complete);
    let mixed = complete > 0 && cancel > 0;
    let verb = if mixed {
        "close"
    } else if cancel > 0 {
        "cancel"
    } else {
        "complete"
    };
    let title = if targets.len() == 1 {
        if verb == "cancel" {
            "Cancel this email task?".to_string()
        } else {
            "Complete this email task?".to_string()
        }
    } else if mixed {
        format!("Close {} tasks?", targets.len())
    } else if verb == "cancel" {
        format!("Cancel {} tasks?", targets.len())
    } else {
        format!("Complete {} tasks?", targets.len())
    };
    let subtitle = if targets.len() == 1 {
        let target = &targets[0];
        format!(
            "{} · {} → {}",
            target.path,
            status_name(target.before),
            status_name(target.after)
        )
    } else {
        let mut distinct: Vec<&str> = targets
            .iter()
            .flat_map(|target| {
                target.mails.iter().map(|mail| mail.note.as_str())
            })
            .collect();
        distinct.sort();
        distinct.dedup();
        format!(
            "{} tasks · {} wired to email",
            targets.len(),
            distinct.len()
        )
    };
    let mut rows = Vec::new();
    for target in targets.iter().take(5) {
        let fallback = if target.task_text.trim().is_empty() {
            task_text("(task)")
        } else {
            target.task_text.clone()
        };
        let (label, line, note) = match chosen_mail(&target.mails) {
            Some(mail) => (
                row_label(mail, &fallback),
                turn_line(mail, now),
                mail.note.clone(),
            ),
            None => (fallback, None, String::new()),
        };
        rows.push(CopyRow {
            path: target.path.clone(),
            line: target.line,
            label,
            turn_line: line,
            note,
        });
    }
    let more = targets.len().saturating_sub(rows.len());
    let mut distinct_notes: Vec<&str> = targets
        .iter()
        .flat_map(|target| target.mails.iter().map(|mail| mail.note.as_str()))
        .collect();
    distinct_notes.sort();
    distinct_notes.dedup();
    let orphaned: Vec<&str> = distinct_notes
        .iter()
        .copied()
        .filter(|note| other_open_linkers(note) == 0)
        .collect();
    let tone = if orphaned.iter().any(|note| {
        targets.iter().flat_map(|target| &target.mails).any(|mail| {
            &mail.note == note && mail.turn.as_deref() == Some("yours")
        })
    }) {
        "warning"
    } else {
        "calm"
    };
    let mut consequence = if orphaned.is_empty() {
        if distinct_notes.len() > 1 {
            "Another open task still watches these threads.".to_string()
        } else {
            "Another open task still watches this thread.".to_string()
        }
    } else {
        let yours: Vec<&MailNoteFacts> = orphaned
            .iter()
            .flat_map(|note| {
                targets.iter().flat_map(|target| &target.mails).filter(
                    move |mail| {
                        &mail.note == note
                            && mail.turn.as_deref() == Some("yours")
                    },
                )
            })
            .collect();
        let theirs: Vec<&MailNoteFacts> = orphaned
            .iter()
            .flat_map(|note| {
                targets.iter().flat_map(|target| &target.mails).filter(
                    move |mail| {
                        &mail.note == note
                            && mail.turn.as_deref() == Some("theirs")
                    },
                )
            })
            .collect();
        if !yours.is_empty() {
            if orphaned.len() == 1 {
                let who = yours
                    .first()
                    .map(|mail| normalize_who(&mail.who))
                    .unwrap_or_default();
                if who.is_empty() {
                    "They are waiting on your reply, and nothing reopens this task until they write again.".to_string()
                } else {
                    format!(
                        "{who} is waiting on your reply, and nothing reopens this task until they write again."
                    )
                }
            } else {
                format!(
                    "{} threads are waiting on your reply, and nothing reopens these tasks until they write again.",
                    orphaned.len()
                )
            }
        } else if !theirs.is_empty() {
            if orphaned.len() == 1 {
                let who = theirs
                    .first()
                    .map(|mail| normalize_who(&mail.who))
                    .unwrap_or_default();
                if who.is_empty() {
                    "A reply reopens it.".to_string()
                } else {
                    format!("A reply from {who} reopens it.")
                }
            } else {
                "A reply reopens them.".to_string()
            }
        } else if orphaned.len() == 1 {
            "Bob stops watching this thread.".to_string()
        } else {
            "Bob stops watching these threads.".to_string()
        }
    };
    if archive_on && !orphaned.is_empty() {
        if orphaned.len() == 1 {
            consequence.push_str(
                " Bob's archive-on-close can then clear it from your Gmail inbox.",
            );
        } else {
            consequence.push_str(
                " Bob's archive-on-close can then clear them from your Gmail inbox.",
            );
        }
    }
    MailCloseCopy {
        title,
        subtitle,
        tone: tone.to_string(),
        verb: verb.to_string(),
        rows,
        more,
        consequence,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mail(
        note: &str,
        title: &str,
        who: &str,
        turn: Option<&str>,
        since: Option<&str>,
    ) -> MailNoteFacts {
        MailNoteFacts {
            note: note.to_string(),
            title: title.to_string(),
            who: who.to_string(),
            turn: turn.map(str::to_string),
            turn_since: since.map(str::to_string),
            gmail_url: String::new(),
        }
    }

    #[test]
    fn age_buckets_match_reply_badge() {
        let now = "2026-11-07T16:00:00";
        assert_eq!(
            format_age(Some("2026-11-07T15:59:30"), now).as_deref(),
            Some("now")
        );
        assert_eq!(
            format_age(Some("2026-11-07T14:00:00"), now).as_deref(),
            Some("2h")
        );
        assert_eq!(
            format_age(Some("2026-11-07T15:30:00"), now).as_deref(),
            Some("30m")
        );
        assert_eq!(
            format_age(Some("2026-11-04T16:00:00"), now).as_deref(),
            Some("3d")
        );
    }

    #[test]
    fn short_date_matches_no_reply_pill() {
        assert_eq!(
            format_short_date(Some("2026-11-07T14:00:00")).as_deref(),
            Some("Sat, Nov 7")
        );
        assert_eq!(
            format_short_date(Some("2026-11-04T09:00:00")).as_deref(),
            Some("Wed, Nov 4")
        );
    }

    #[test]
    fn yours_line_omits_unknown_who() {
        let yours =
            mail("m.md", "T", "", Some("yours"), Some("2026-11-07T14:00:00"));
        assert_eq!(
            turn_line(&yours, "2026-11-07T16:00:00").as_deref(),
            Some("↩ 2h — waiting on your reply")
        );
    }
}
