//! Whose turn a Gmail thread is in, from message metadata only.
//!
//! This module is pure: no network or vault I/O. [`classify`] sorts one
//! message into a move class; [`fold`] orders a thread's messages and reduces
//! them to whose turn it is and to the net effect of everything newer than a
//! previously applied marker. See `docs/gmail.md` (Save) for the mail-note
//! fields this feeds.

use chrono::{DateTime, Utc};

use super::model::GmailMessage;
use super::note::mailbox_addresses;

/// The class of one Gmail message, in table order: the first rule that
/// matches wins.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MessageClass {
    /// A `DRAFT`, or a note to self. Never counted.
    Ignored,
    /// Bryan wrote to at least one other recipient.
    YourMove,
    /// Someone else wrote.
    TheirMove,
    /// An RFC 3834 auto-reply or other automated message. Seen, never counted.
    Automated,
    /// A delivery-failure report. Counted like their move.
    Bounce,
    /// No `SENT` label and no sender address: the direction can't be proven.
    Unknown,
}

impl MessageClass {
    fn is_counted(self) -> bool {
        matches!(self, Self::YourMove | Self::TheirMove | Self::Bounce)
    }
}

/// Whose turn the thread is, written to a mail note as `turn`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Turn {
    /// The newest counted message is theirs or a bounce: the ball is in
    /// Bryan's court.
    Yours,
    /// Bryan wrote last: the thread is waiting on them.
    Theirs,
}

impl Turn {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Yours => "yours",
            Self::Theirs => "theirs",
        }
    }
}

/// The net effect of the counted messages newer than the applied marker.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Net {
    /// No counted message is newer than the marker.
    Nothing,
    /// Your move: hold the watching tasks.
    Hold,
    /// Their move: lift (or resurface) the watching tasks.
    Lift,
    /// A bounce: lift with reason `bounced`.
    Bounced,
    /// Bob can't establish the order or the direction, so it changes nothing.
    Unknown(UnknownReason),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum UnknownReason {
    /// A message that may count has no usable `internalDate`.
    MissingInternalDate,
    /// The newest message that may count has no provable direction.
    UnprovenDirection,
}

impl UnknownReason {
    #[cfg_attr(not(test), allow(dead_code))] // Reported by the gmail sync watch phase.
    pub(crate) fn message(self) -> &'static str {
        match self {
            Self::MissingInternalDate => {
                "a message has no Gmail internalDate, so the thread can't be ordered"
            }
            Self::UnprovenDirection => {
                "the newest message has no SENT label and no sender address"
            }
        }
    }
}

/// The last applied turn: the counted message whose effect is already on the
/// watching tasks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Marker<'a> {
    pub(crate) message_id: &'a str,
    pub(crate) internal_date: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ThreadTurn {
    /// The effect of the counted messages after the marker (the whole thread
    /// when there is no marker).
    pub(crate) net: Net,
    /// Whose turn the whole thread is in. `None` when nobody has made a move
    /// or the verdict is unknown.
    pub(crate) turn: Option<Turn>,
    /// When the message that set `turn` arrived.
    pub(crate) turn_since: Option<DateTime<Utc>>,
    /// The newest counted message in the whole thread. Whenever `net` is a
    /// move, this is the message that made it.
    pub(crate) newest_counted_id: Option<String>,
    /// That message's Gmail `internalDate`, in milliseconds.
    pub(crate) newest_internal_date: Option<i64>,
    /// When Bryan's newest counted send arrived.
    pub(crate) last_send_at: Option<DateTime<Utc>>,
}

impl ThreadTurn {
    fn unknown(reason: UnknownReason) -> Self {
        Self {
            net: Net::Unknown(reason),
            turn: None,
            turn_since: None,
            newest_counted_id: None,
            newest_internal_date: None,
            last_send_at: None,
        }
    }
}

/// Classify one message for `account`, using metadata only: labels, the
/// sender and recipient headers, the RFC 3834 auto-reply headers, and the
/// payload `Content-Type`.
pub(crate) fn classify(message: &GmailMessage, account: &str) -> MessageClass {
    if has_label(message, "DRAFT") {
        return MessageClass::Ignored;
    }
    let account = account.trim();
    let from = first_address(message, "From");
    let from_is_account = from
        .as_deref()
        .is_some_and(|address| address.eq_ignore_ascii_case(account));
    if has_label(message, "SENT") || from_is_account {
        let to_someone_else = ["To", "Cc", "Bcc"]
            .into_iter()
            .flat_map(|name| addresses(message, name))
            .any(|address| !address.eq_ignore_ascii_case(account));
        return if to_someone_else {
            MessageClass::YourMove
        } else {
            MessageClass::Ignored
        };
    }
    if from.as_deref().is_some_and(is_mailer_daemon)
        || is_delivery_report(message)
    {
        return MessageClass::Bounce;
    }
    if is_automated(message) {
        return MessageClass::Automated;
    }
    if from.is_none() {
        return MessageClass::Unknown;
    }
    MessageClass::TheirMove
}

/// Order a thread's messages by `internalDate` (Gmail's thread order breaks
/// ties) and fold them. `turn` and `turn_since` describe the whole thread;
/// `net` describes only the messages after `after_marker`.
///
/// Automated and ignored messages count only as seen, so a send followed by
/// their reply between two folds is one lift, never a hold and then a lift.
pub(crate) fn fold(
    messages: &[GmailMessage],
    account: &str,
    after_marker: Option<Marker<'_>>,
) -> ThreadTurn {
    let mut entries = Vec::with_capacity(messages.len());
    for (index, message) in messages.iter().enumerate() {
        let class = classify(message, account);
        if class == MessageClass::Ignored {
            continue;
        }
        let Some((millis, time)) = internal_date(message) else {
            return ThreadTurn::unknown(UnknownReason::MissingInternalDate);
        };
        entries.push(Entry {
            millis,
            time,
            index,
            class,
            id: message.id.as_str(),
        });
    }
    entries.sort_by_key(|entry| (entry.millis, entry.index));

    let start = match after_marker {
        None => 0,
        Some(marker) => entries
            .iter()
            .position(|entry| entry.id.eq_ignore_ascii_case(marker.message_id))
            .map(|position| position + 1)
            .unwrap_or_else(|| {
                entries.partition_point(|entry| {
                    entry.millis <= marker.internal_date
                })
            }),
    };
    let net = match newest_decisive(&entries[start..]) {
        None => Net::Nothing,
        Some(entry) => match entry.class {
            MessageClass::YourMove => Net::Hold,
            MessageClass::TheirMove => Net::Lift,
            MessageClass::Bounce => Net::Bounced,
            _ => Net::Unknown(UnknownReason::UnprovenDirection),
        },
    };

    let (turn, turn_since) = match newest_decisive(&entries) {
        Some(entry) if entry.class == MessageClass::YourMove => {
            (Some(Turn::Theirs), Some(entry.time))
        }
        Some(entry) if entry.class.is_counted() => {
            (Some(Turn::Yours), Some(entry.time))
        }
        _ => (None, None),
    };
    let newest_counted =
        entries.iter().rev().find(|entry| entry.class.is_counted());
    let last_send_at = entries
        .iter()
        .rev()
        .find(|entry| entry.class == MessageClass::YourMove)
        .map(|entry| entry.time);
    ThreadTurn {
        net,
        turn,
        turn_since,
        newest_counted_id: newest_counted.map(|entry| entry.id.to_string()),
        newest_internal_date: newest_counted.map(|entry| entry.millis),
        last_send_at,
    }
}

struct Entry<'a> {
    millis: i64,
    time: DateTime<Utc>,
    index: usize,
    class: MessageClass,
    id: &'a str,
}

/// The newest message that is counted or whose direction is unknown.
/// Automated messages are skipped: they never decide a turn.
fn newest_decisive<'a>(entries: &'a [Entry<'a>]) -> Option<&'a Entry<'a>> {
    entries
        .iter()
        .rev()
        .find(|entry| entry.class != MessageClass::Automated)
}

fn internal_date(message: &GmailMessage) -> Option<(i64, DateTime<Utc>)> {
    let millis = message.internal_date.as_deref()?.parse::<i64>().ok()?;
    Some((millis, DateTime::from_timestamp_millis(millis)?))
}

fn has_label(message: &GmailMessage, label: &str) -> bool {
    message
        .label_ids
        .iter()
        .any(|id| id.eq_ignore_ascii_case(label))
}

fn addresses(message: &GmailMessage, header: &str) -> Vec<String> {
    message
        .header_values(header)
        .flat_map(mailbox_addresses)
        .collect()
}

fn first_address(message: &GmailMessage, header: &str) -> Option<String> {
    addresses(message, header).into_iter().next()
}

fn is_mailer_daemon(address: &str) -> bool {
    address.split_once('@').is_some_and(|(local, _)| {
        local.eq_ignore_ascii_case("mailer-daemon")
            || local.eq_ignore_ascii_case("postmaster")
    })
}

/// `Content-Type: multipart/report; report-type=delivery-status` (RFC 3464).
fn is_delivery_report(message: &GmailMessage) -> bool {
    message.header_values("Content-Type").any(|value| {
        let mut parts = value.split(';');
        let media_type = parts.next().unwrap_or_default().trim();
        media_type.eq_ignore_ascii_case("multipart/report")
            && parts.any(|parameter| {
                parameter.split_once('=').is_some_and(|(name, value)| {
                    name.trim().eq_ignore_ascii_case("report-type")
                        && value
                            .trim()
                            .trim_matches('"')
                            .eq_ignore_ascii_case("delivery-status")
                })
            })
    })
}

/// RFC 3834 `Auto-Submitted` other than `no`, the common `X-Autoreply` and
/// `X-Autorespond` markers, or `Precedence: auto_reply`.
fn is_automated(message: &GmailMessage) -> bool {
    message.header_values("Auto-Submitted").any(|value| {
        let keyword = value.split(';').next().unwrap_or_default().trim();
        !keyword.is_empty() && !keyword.eq_ignore_ascii_case("no")
    }) || message.header_values("X-Autoreply").next().is_some()
        || message.header_values("X-Autorespond").next().is_some()
        || message
            .header_values("Precedence")
            .any(|value| value.trim().eq_ignore_ascii_case("auto_reply"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::native::gmail::model::Header;

    const ME: &str = "me@example.com";
    const DANA: &str = "Dana <dana@example.com>";

    /// One message at `minute` past a fixed instant, with `labels` and
    /// `headers` (name, value).
    fn message(
        id: &str,
        minute: i64,
        labels: &[&str],
        headers: &[(&str, &str)],
    ) -> GmailMessage {
        GmailMessage {
            id: id.to_string(),
            thread_id: "aabbccddeeff0011".to_string(),
            label_ids: labels.iter().map(|label| label.to_string()).collect(),
            internal_date: Some(
                (1_799_847_960_000 + minute * 60_000).to_string(),
            ),
            snippet: None,
            headers: headers
                .iter()
                .map(|(name, value)| Header {
                    name: name.to_string(),
                    value: value.to_string(),
                })
                .collect(),
            payload: None,
        }
    }

    fn send(id: &str, minute: i64) -> GmailMessage {
        message(
            id,
            minute,
            &["SENT"],
            &[("From", "Me <me@example.com>"), ("To", DANA)],
        )
    }

    fn reply(id: &str, minute: i64) -> GmailMessage {
        message(id, minute, &["INBOX"], &[("From", DANA), ("To", ME)])
    }

    fn at(minute: i64) -> DateTime<Utc> {
        DateTime::from_timestamp_millis(1_799_847_960_000 + minute * 60_000)
            .unwrap()
    }

    // T1
    #[test]
    fn your_send_is_your_move_and_holds() {
        let thread = [send("a1", 0)];
        assert_eq!(classify(&thread[0], ME), MessageClass::YourMove);
        let folded = fold(&thread, ME, None);
        assert_eq!(folded.net, Net::Hold);
        assert_eq!(folded.turn, Some(Turn::Theirs));
        assert_eq!(folded.turn_since, Some(at(0)));
        assert_eq!(folded.last_send_at, Some(at(0)));
        assert_eq!(folded.newest_counted_id.as_deref(), Some("a1"));
    }

    // T2
    #[test]
    fn their_reply_is_their_move_and_lifts() {
        let thread = [reply("b1", 0)];
        assert_eq!(classify(&thread[0], ME), MessageClass::TheirMove);
        let folded = fold(&thread, ME, None);
        assert_eq!(folded.net, Net::Lift);
        assert_eq!(folded.turn, Some(Turn::Yours));
        assert_eq!(folded.last_send_at, None);
    }

    // T3
    #[test]
    fn auto_submitted_auto_replied_is_automated_and_keeps_the_hold() {
        let auto = message(
            "c2",
            5,
            &["INBOX"],
            &[
                ("From", DANA),
                ("To", ME),
                (
                    "Auto-Submitted",
                    "auto-replied; owner-email=dana@example.com",
                ),
            ],
        );
        assert_eq!(classify(&auto, ME), MessageClass::Automated);
        let folded = fold(&[send("c1", 0), auto], ME, None);
        assert_eq!(folded.net, Net::Hold);
        assert_eq!(folded.turn, Some(Turn::Theirs));
        assert_eq!(folded.turn_since, Some(at(0)));
        assert_eq!(folded.newest_counted_id.as_deref(), Some("c1"));
    }

    // T4
    #[test]
    fn x_autoreply_x_autorespond_and_precedence_auto_reply_are_automated() {
        for header in [
            ("X-Autoreply", "yes"),
            ("X-Autorespond", "Out of office"),
            ("Precedence", " Auto_Reply "),
        ] {
            let auto = message(
                "d1",
                0,
                &["INBOX"],
                &[("From", DANA), ("To", ME), header],
            );
            assert_eq!(
                classify(&auto, ME),
                MessageClass::Automated,
                "{header:?}"
            );
        }
        let bulk = message(
            "d2",
            0,
            &["INBOX"],
            &[("From", DANA), ("To", ME), ("Precedence", "bulk")],
        );
        assert_eq!(classify(&bulk, ME), MessageClass::TheirMove);
    }

    // T5
    #[test]
    fn mailer_daemon_delivery_reports_are_bounces_that_lift() {
        let daemon = message(
            "e2",
            3,
            &["INBOX"],
            &[
                (
                    "From",
                    "Mail Delivery Subsystem <MAILER-DAEMON@googlemail.com>",
                ),
                ("To", ME),
                ("Auto-Submitted", "auto-replied"),
            ],
        );
        assert_eq!(classify(&daemon, ME), MessageClass::Bounce);
        let postmaster = message(
            "e3",
            3,
            &["INBOX"],
            &[("From", "postmaster@example.net"), ("To", ME)],
        );
        assert_eq!(classify(&postmaster, ME), MessageClass::Bounce);
        let report = message(
            "e4",
            3,
            &["INBOX"],
            &[
                ("From", "Relay <relay@example.net>"),
                ("To", ME),
                (
                    "Content-Type",
                    "multipart/report; report-type=\"Delivery-Status\"; boundary=\"b\"",
                ),
            ],
        );
        assert_eq!(classify(&report, ME), MessageClass::Bounce);
        let other_report = message(
            "e5",
            3,
            &["INBOX"],
            &[
                ("From", DANA),
                ("To", ME),
                (
                    "Content-Type",
                    "multipart/report; report-type=disposition-notification",
                ),
            ],
        );
        assert_eq!(classify(&other_report, ME), MessageClass::TheirMove);

        let folded = fold(&[send("e1", 0), daemon], ME, None);
        assert_eq!(folded.net, Net::Bounced);
        assert_eq!(folded.turn, Some(Turn::Yours));
        assert_eq!(folded.turn_since, Some(at(3)));
        assert_eq!(folded.last_send_at, Some(at(0)));
    }

    // T6
    #[test]
    fn a_draft_is_ignored() {
        let draft =
            message("f2", 5, &["DRAFT", "SENT"], &[("From", ME), ("To", DANA)]);
        assert_eq!(classify(&draft, ME), MessageClass::Ignored);
        let folded = fold(&[reply("f1", 0), draft], ME, None);
        assert_eq!(folded.net, Net::Lift);
        assert_eq!(folded.turn, Some(Turn::Yours));
        assert_eq!(folded.last_send_at, None);
    }

    // T7
    #[test]
    fn a_note_to_self_is_ignored() {
        let note = message(
            "g2",
            5,
            &["SENT", "INBOX"],
            &[("From", ME), ("To", "Me <ME@example.com>"), ("Bcc", ME)],
        );
        assert_eq!(classify(&note, ME), MessageClass::Ignored);
        let folded = fold(&[reply("g1", 0), note], ME, None);
        assert_eq!(folded.turn, Some(Turn::Yours));
        assert_eq!(folded.turn_since, Some(at(0)));
    }

    // T8
    #[test]
    fn a_from_match_without_sent_is_your_move() {
        let imported = message(
            "h1",
            0,
            &["INBOX"],
            &[("From", "Me <ME@Example.com>"), ("Cc", DANA)],
        );
        assert_eq!(classify(&imported, ME), MessageClass::YourMove);
        let bcc_only = message(
            "h2",
            0,
            &["SENT"],
            &[("From", "alias@example.org"), ("Bcc", DANA)],
        );
        assert_eq!(classify(&bcc_only, ME), MessageClass::YourMove);
    }

    // T9
    #[test]
    fn send_then_reply_between_folds_is_one_lift() {
        let folded = fold(&[send("i1", 0), reply("i2", 5)], ME, None);
        assert_eq!(folded.net, Net::Lift);
        assert_eq!(folded.turn, Some(Turn::Yours));
        assert_eq!(folded.turn_since, Some(at(5)));
        assert_eq!(folded.last_send_at, Some(at(0)));
        assert_eq!(folded.newest_counted_id.as_deref(), Some("i2"));
    }

    // T10
    #[test]
    fn reply_then_send_between_folds_is_one_hold() {
        let folded = fold(&[reply("j1", 0), send("j2", 5)], ME, None);
        assert_eq!(folded.net, Net::Hold);
        assert_eq!(folded.turn, Some(Turn::Theirs));
        assert_eq!(folded.turn_since, Some(at(5)));
        assert_eq!(folded.last_send_at, Some(at(5)));
    }

    // T11
    #[test]
    fn auto_submitted_no_is_a_human_message() {
        let human = message(
            "k1",
            0,
            &["INBOX"],
            &[("From", DANA), ("To", ME), ("Auto-Submitted", " No ")],
        );
        assert_eq!(classify(&human, ME), MessageClass::TheirMove);
    }

    // T12
    #[test]
    fn a_missing_internal_date_is_unknown() {
        let mut undated = reply("l2", 5);
        undated.internal_date = None;
        let folded = fold(&[send("l1", 0), undated], ME, None);
        assert_eq!(
            folded.net,
            Net::Unknown(UnknownReason::MissingInternalDate)
        );
        assert_eq!(folded.turn, None);
        assert_eq!(folded.turn_since, None);
        assert_eq!(folded.newest_counted_id, None);
        assert!(UnknownReason::MissingInternalDate
            .message()
            .contains("internalDate"));

        let mut out_of_range = reply("l3", 5);
        out_of_range.internal_date = Some("99999999999999999999".to_string());
        assert_eq!(
            fold(&[out_of_range], ME, None).net,
            Net::Unknown(UnknownReason::MissingInternalDate)
        );

        let mut undated_draft = send("l4", 9);
        undated_draft.label_ids.push("DRAFT".to_string());
        undated_draft.internal_date = None;
        let folded = fold(&[reply("l5", 0), undated_draft], ME, None);
        assert_eq!(folded.turn, Some(Turn::Yours));
    }

    #[test]
    fn nobody_has_moved_when_only_automated_or_ignored_messages_exist() {
        let auto = message(
            "m1",
            0,
            &["INBOX"],
            &[("From", DANA), ("To", ME), ("X-Autoreply", "yes")],
        );
        let folded = fold(&[auto], ME, None);
        assert_eq!(folded.net, Net::Nothing);
        assert_eq!(folded.turn, None);
        assert_eq!(folded.turn_since, None);
        assert_eq!(fold(&[], ME, None).net, Net::Nothing);
    }

    #[test]
    fn an_unproven_newest_direction_is_unknown_but_older_ones_do_not_matter() {
        let anonymous = message("n2", 5, &["INBOX"], &[("To", ME)]);
        assert_eq!(classify(&anonymous, ME), MessageClass::Unknown);
        let folded = fold(&[send("n1", 0), anonymous.clone()], ME, None);
        assert_eq!(folded.net, Net::Unknown(UnknownReason::UnprovenDirection));
        assert_eq!(folded.turn, None);
        assert_eq!(folded.newest_counted_id.as_deref(), Some("n1"));

        let mut older = anonymous;
        older.internal_date = Some(at(-5).timestamp_millis().to_string());
        let folded = fold(&[older, send("n1", 0)], ME, None);
        assert_eq!(folded.net, Net::Hold);
        assert_eq!(folded.turn, Some(Turn::Theirs));
    }

    #[test]
    fn messages_order_by_internal_date_with_thread_order_breaking_ties() {
        // Gmail lists the reply first, but it arrived later.
        let folded = fold(&[reply("o2", 5), send("o1", 0)], ME, None);
        assert_eq!(folded.net, Net::Lift);
        assert_eq!(folded.turn_since, Some(at(5)));

        // Same millisecond: the later thread position is newer.
        let folded = fold(&[reply("o3", 5), send("o4", 5)], ME, None);
        assert_eq!(folded.net, Net::Hold);
        assert_eq!(folded.turn, Some(Turn::Theirs));
    }

    #[test]
    fn the_net_effect_counts_only_messages_after_the_marker() {
        let thread = [send("p1", 0), reply("p2", 5)];
        let marker = Marker {
            message_id: "P2",
            internal_date: at(5).timestamp_millis(),
        };
        let folded = fold(&thread, ME, Some(marker));
        assert_eq!(folded.net, Net::Nothing);
        assert_eq!(folded.turn, Some(Turn::Yours));

        let marker = Marker {
            message_id: "p1",
            internal_date: at(0).timestamp_millis(),
        };
        assert_eq!(fold(&thread, ME, Some(marker)).net, Net::Lift);

        let with_auto = [
            send("q1", 0),
            message(
                "q2",
                5,
                &["INBOX"],
                &[
                    ("From", DANA),
                    ("To", ME),
                    ("Auto-Submitted", "auto-replied"),
                ],
            ),
        ];
        let marker = Marker {
            message_id: "q1",
            internal_date: at(0).timestamp_millis(),
        };
        let folded = fold(&with_auto, ME, Some(marker));
        assert_eq!(folded.net, Net::Nothing);
        assert_eq!(folded.turn, Some(Turn::Theirs));
    }

    #[test]
    fn a_vanished_marker_message_falls_back_to_its_internal_date() {
        let thread = [send("r1", 0), reply("r3", 10)];
        let marker = Marker {
            message_id: "r2-deleted",
            internal_date: at(5).timestamp_millis(),
        };
        assert_eq!(fold(&thread, ME, Some(marker)).net, Net::Lift);
        let marker = Marker {
            message_id: "r2-deleted",
            internal_date: at(10).timestamp_millis(),
        };
        assert_eq!(fold(&thread, ME, Some(marker)).net, Net::Nothing);
    }
}
