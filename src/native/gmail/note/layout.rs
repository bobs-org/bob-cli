//! Render a fetched Gmail thread into a complete mail note.

use std::collections::{BTreeMap, BTreeSet};

use chrono::{DateTime, Datelike, Utc};
use chrono_tz::Tz;
use serde::Serialize;

use crate::native::config::GmailBodyMode;
use crate::native::gmail::{
    body::{importable_messages, inert_single_line, RenderedBody},
    ids,
    model::{GmailMessage, GmailThread, HeaderKind},
};

use super::{text, NoteError};

#[derive(Clone, PartialEq, Eq)]
pub(crate) struct LayoutInput<'a> {
    pub(crate) account: &'a str,
    pub(crate) body_mode: GmailBodyMode,
    pub(crate) current_year: i32,
    pub(crate) parent: &'a str,
    pub(crate) rendered: &'a BTreeMap<String, RenderedBody>,
    pub(crate) saved_at: DateTime<Utc>,
    pub(crate) thread: &'a GmailThread,
    pub(crate) timezone: Tz,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct NoteMetadata {
    pub(crate) account: String,
    pub(crate) content: String,
    pub(crate) gmail_thread_id: String,
    pub(crate) gmail_url: String,
    pub(crate) messages: usize,
    pub(crate) saved: String,
    pub(crate) started: String,
    pub(crate) title: String,
    pub(crate) updated: String,
    pub(crate) who: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RenderedCard {
    pub(crate) id: String,
    pub(crate) header: String,
    pub(crate) markdown: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RenderedNote {
    pub(crate) content: String,
    pub(crate) metadata: NoteMetadata,
    pub(crate) open_gmail: String,
    pub(crate) summary: String,
    pub(crate) cards: Vec<RenderedCard>,
}

#[derive(Serialize)]
struct Frontmatter<'a> {
    parent: &'a str,
    #[serde(rename = "type")]
    note_type: &'a str,
    cssclasses: [&'a str; 1],
    title: &'a str,
    who: &'a str,
    started: &'a str,
    updated: &'a str,
    messages: usize,
    gmail_account: &'a str,
    gmail_thread_id: &'a str,
    gmail_url: &'a str,
    saved: &'a str,
    content: &'a str,
}

/// Render the first saved version of a mail note. All dates are generated in
/// Bob's configured timezone, while the saved instant itself is supplied by
/// the caller so this function remains deterministic.
pub(crate) fn render(
    input: &LayoutInput<'_>,
) -> Result<RenderedNote, NoteError> {
    let thread_id = normalize_hex(&input.thread.id)?;
    let messages: Vec<_> = importable_messages(input.thread).collect();
    if messages.is_empty() {
        return Err(NoteError::InvalidIdentity);
    }
    let mut seen = BTreeSet::new();
    let mut timed = Vec::with_capacity(messages.len());
    for message in &messages {
        let message_id = normalize_hex(&message.id)?;
        if normalize_hex(&message.thread_id)? != thread_id {
            return Err(NoteError::InvalidIdentity);
        }
        if !seen.insert(message_id) {
            return Err(NoteError::InvalidIdentity);
        }
        timed.push(
            message_time(message)
                .ok_or(NoteError::MissingMessageDate)?
                .with_timezone(&input.timezone),
        );
    }

    let first = messages[0];
    let title_raw = text::subject_without_prefixes(
        first.first_header(HeaderKind::Subject).unwrap_or_default(),
    );
    let title_raw = if title_raw.is_empty() {
        "(no subject)".to_string()
    } else {
        title_raw
    };
    let who_raw = thread_who(first, input.account);
    let title = inert_single_line(&title_raw);
    let who = inert_single_line(&who_raw);
    let started = timed[0].format("%Y-%m-%dT%H:%M:%S%:z").to_string();
    let updated = timed[timed.len() - 1]
        .format("%Y-%m-%dT%H:%M:%S%:z")
        .to_string();
    let saved = input
        .saved_at
        .with_timezone(&input.timezone)
        .format("%Y-%m-%dT%H:%M:%S%:z")
        .to_string();
    let gmail_url = ids::canonical_url(input.account, &thread_id);
    let metadata = NoteMetadata {
        account: inert_single_line(input.account),
        content: "untrusted-email".to_string(),
        gmail_thread_id: thread_id,
        gmail_url,
        messages: messages.len(),
        saved,
        started,
        title,
        updated,
        who,
    };
    let open_gmail = format!("[Open in Gmail ↗]({})", metadata.gmail_url);
    let summary =
        summary_line(&messages, input.account, &timed, input.current_year);

    let mut cards = Vec::with_capacity(messages.len());
    let mut attachment_number = 1u32;
    for (index, message) in messages.iter().enumerate() {
        let id = normalize_hex(&message.id)?;
        let body = input
            .rendered
            .get(&message.id)
            .ok_or(NoteError::MissingRenderedBody)?;
        let card = render_card(
            message,
            &id,
            body,
            input.body_mode,
            input.account,
            &timed[index],
            index + 1 == messages.len(),
            input.current_year,
            &mut attachment_number,
        );
        cards.push(card);
    }

    let content =
        assemble(&metadata, &open_gmail, &summary, &cards, input.parent)?;
    Ok(RenderedNote {
        content,
        metadata,
        open_gmail,
        summary,
        cards,
    })
}

fn assemble(
    metadata: &NoteMetadata,
    open_gmail: &str,
    summary: &str,
    cards: &[RenderedCard],
    parent: &str,
) -> Result<String, NoteError> {
    let yaml = serde_yaml::to_string(&Frontmatter {
        parent,
        note_type: "[[mail]]",
        cssclasses: ["bob-mail"],
        title: &metadata.title,
        who: &metadata.who,
        started: &metadata.started,
        updated: &metadata.updated,
        messages: metadata.messages,
        gmail_account: &metadata.account,
        gmail_thread_id: &metadata.gmail_thread_id,
        gmail_url: &metadata.gmail_url,
        saved: &metadata.saved,
        content: &metadata.content,
    })
    .map_err(|_| NoteError::InvalidIdentity)?;
    let yaml = yaml.strip_prefix("---\n").unwrap_or(&yaml);
    let mut out = String::from("---\n");
    out.push_str(yaml);
    if !yaml.ends_with('\n') {
        out.push('\n');
    }
    out.push_str("---\n\n# ");
    out.push_str(&metadata.title);
    out.push_str("\n\n");
    out.push_str(open_gmail);
    out.push_str("\n· ");
    out.push_str(summary);
    out.push_str(
        "\n\n## Notes\n\n## Conversation\n\n<!-- bob:gmail:begin v1 -->\n",
    );
    for (index, card) in cards.iter().enumerate() {
        if index > 0 {
            out.push('\n');
        }
        out.push_str(&card.markdown);
        out.push('\n');
    }
    out.push_str("\n<!-- bob:gmail:end -->\n");
    Ok(out)
}

fn render_card(
    message: &GmailMessage,
    id: &str,
    body: &RenderedBody,
    body_mode: GmailBodyMode,
    account: &str,
    time: &DateTime<Tz>,
    newest: bool,
    current_year: i32,
    attachment_number: &mut u32,
) -> RenderedCard {
    let sender = message_mailbox_name(message, HeaderKind::From, account)
        .unwrap_or_else(|| "Unknown sender".to_string());
    let own = message_mailboxes(message, HeaderKind::From)
        .first()
        .and_then(|mailbox| mailbox.address.as_deref())
        .is_some_and(|address| address.eq_ignore_ascii_case(account.trim()));
    let sender = if own { "You".to_string() } else { sender };
    let sender = inert_single_line(&sender);
    let date = format_card_date(time, current_year);
    let recipients = recipient_summary(message, account);
    let fold = if newest { '+' } else { '-' };
    let header = format!("> [!mail{fold}] **{sender}** · {date}{recipients}");
    let rfc_message_id = message
        .first_header(HeaderKind::MessageId)
        .map(percent_encode_marker)
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "-".to_string());
    let mut markdown =
        format!("<!-- bob:gmail:msg {id} {rfc_message_id} -->\n{header}");

    let mut body_text = String::new();
    if body_mode == GmailBodyMode::Headers {
        let snippet = message
            .snippet
            .as_deref()
            .map(inert_single_line)
            .unwrap_or_default();
        if !snippet.is_empty() {
            body_text.push_str(&snippet);
            body_text.push_str("\n\n");
        }
        body_text.push_str("*(body not saved: headers mode)*");
    } else {
        body_text.push_str(body.markdown.trim_end());
        if let Some(quoted) =
            body.quoted.as_deref().filter(|text| !text.is_empty())
        {
            if !body_text.is_empty() {
                body_text.push_str("\n\n");
            }
            body_text.push_str("> [!quote]- Earlier in the thread\n>\n");
            body_text.push_str(&quote_nested(quoted));
        }
        if let Some(signature) =
            body.signature.as_deref().filter(|text| !text.is_empty())
        {
            if !body_text.is_empty() {
                body_text.push_str("\n\n");
            }
            body_text.push_str("> [!mail-signature]- Signature\n>\n");
            body_text.push_str(&quote_nested(signature));
        }
    }
    for attachment in &body.attachments {
        if !body_text.is_empty() {
            body_text.push_str("\n\n");
        }
        let name = attachment
            .filename
            .as_deref()
            .filter(|name| !name.is_empty())
            .map(inert_single_line)
            .unwrap_or_else(|| "attachment".to_string());
        body_text.push_str("📎 ");
        body_text.push_str(&attachment_number.to_string());
        body_text.push_str(". ");
        *attachment_number = attachment_number.saturating_add(1);
        body_text.push_str(&name);
        if let Some(size) = attachment.size {
            body_text.push_str(" · ");
            body_text.push_str(&format_size(size));
        }
    }
    for line in body_text.lines() {
        markdown.push_str("\n> ");
        markdown.push_str(line);
    }
    markdown.push_str(&format!("\n\n^m-{id}"));
    RenderedCard {
        id: id.to_string(),
        header,
        markdown,
    }
}

pub(crate) fn message_time(message: &GmailMessage) -> Option<DateTime<Utc>> {
    if let Some(internal_date) = message.internal_date.as_deref()
        && let Ok(millis) = internal_date.parse::<i64>()
        && let Some(date) = DateTime::from_timestamp_millis(millis)
    {
        return Some(date);
    }
    message
        .first_header(HeaderKind::Date)
        .and_then(|value| DateTime::parse_from_rfc2822(value).ok())
        .map(|value| value.with_timezone(&Utc))
}

fn normalize_hex(value: &str) -> Result<String, NoteError> {
    if value.is_empty() || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(NoteError::InvalidIdentity);
    }
    Ok(value.to_ascii_lowercase())
}

pub(crate) fn thread_who(message: &GmailMessage, account: &str) -> String {
    let sender = message_mailboxes(message, HeaderKind::From)
        .into_iter()
        .next();
    if let Some(sender) = sender {
        let is_self = sender.address.as_deref().is_some_and(|address| {
            address.eq_ignore_ascii_case(account.trim())
        });
        if !is_self {
            return sender.display;
        }
    }
    let recipient = [HeaderKind::To, HeaderKind::Cc]
        .into_iter()
        .flat_map(|kind| message_mailboxes(message, kind))
        .find(|mailbox| {
            !mailbox.address.as_deref().is_some_and(|address| {
                address.eq_ignore_ascii_case(account.trim())
            })
        });
    match recipient {
        Some(recipient) => format!("to {}", recipient.display),
        None => "you".to_string(),
    }
}

fn summary_line(
    messages: &[&GmailMessage],
    account: &str,
    times: &[DateTime<Tz>],
    current_year: i32,
) -> String {
    let mut counterparts: Vec<String> = Vec::new();
    let mut keys = BTreeSet::new();
    for message in messages {
        for mailbox in message_mailboxes(message, HeaderKind::From)
            .into_iter()
            .chain(message_mailboxes(message, HeaderKind::To))
            .chain(message_mailboxes(message, HeaderKind::Cc))
        {
            if mailbox.address.as_deref().is_some_and(|address| {
                address.eq_ignore_ascii_case(account.trim())
            }) {
                continue;
            }
            let key = mailbox
                .address
                .as_deref()
                .unwrap_or(&mailbox.display)
                .to_ascii_lowercase();
            if keys.insert(key) {
                counterparts.push(inert_single_line(&mailbox.display));
            }
        }
    }
    let names = if counterparts.is_empty() {
        "you".to_string()
    } else {
        let shown = counterparts
            .iter()
            .take(2)
            .cloned()
            .collect::<Vec<_>>()
            .join(", ");
        if counterparts.len() > 2 {
            format!("{shown} +{}", counterparts.len() - 2)
        } else {
            shown
        }
    };
    let first = format_summary_date(&times[0], current_year);
    let last = format_summary_date(&times[times.len() - 1], current_year);
    format!(
        "{names} ↔ you · {} messages · {first} → {last}",
        messages.len()
    )
}

fn message_mailbox_name(
    message: &GmailMessage,
    kind: HeaderKind,
    account: &str,
) -> Option<String> {
    message_mailboxes(message, kind)
        .first()
        .map(|mailbox| text::display_for_account(mailbox, account))
}

fn message_mailboxes(
    message: &GmailMessage,
    kind: HeaderKind,
) -> Vec<text::Mailbox> {
    text::message_mailboxes(message, kind)
}

fn recipient_summary(message: &GmailMessage, account: &str) -> String {
    let mut result = String::new();
    for (kind, label) in [(HeaderKind::To, "to"), (HeaderKind::Cc, "cc")] {
        let mailboxes = message_mailboxes(message, kind);
        if mailboxes.is_empty() {
            continue;
        }
        let names: Vec<_> = mailboxes
            .iter()
            .map(|mailbox| text::display_for_account(mailbox, account))
            .collect();
        let shown =
            names.iter().take(2).cloned().collect::<Vec<_>>().join(", ");
        let shown = if names.len() > 2 {
            format!("{shown} +{}", names.len() - 2)
        } else {
            shown
        };
        result.push(' ');
        result.push_str(label);
        result.push(' ');
        result.push_str(&inert_single_line(&shown));
    }
    result
}

fn format_card_date(time: &DateTime<Tz>, current_year: i32) -> String {
    let year = if time.year() != current_year {
        format!(" {}", time.format("%Y"))
    } else {
        String::new()
    };
    format!(
        "{}, {}{} · {}",
        time.format("%a"),
        time.format("%b %-d"),
        year,
        time.format("%l:%M %p").to_string().trim_start(),
    )
}

fn format_summary_date(time: &DateTime<Tz>, current_year: i32) -> String {
    if time.year() == current_year {
        time.format("%b %-d").to_string()
    } else {
        time.format("%b %-d, %Y").to_string()
    }
}

fn quote_nested(text: &str) -> String {
    text.lines()
        .map(|line| {
            if line.is_empty() {
                ">".to_string()
            } else {
                format!("> {line}")
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn percent_encode_marker(raw: &str) -> String {
    let mut output = String::new();
    for byte in raw.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'.' | b'~') {
            output.push(byte as char);
        } else {
            output.push_str(&format!("%{byte:02X}"));
        }
    }
    output
}

fn format_size(bytes: u64) -> String {
    if bytes < 1024 {
        format!("{bytes} B")
    } else if bytes < 1024 * 1024 {
        format!("{} KB", (bytes + 512) / 1024)
    } else {
        format!("{} MB", (bytes + 512 * 1024) / (1024 * 1024))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::native::gmail::body::{
        render_message, Attachment, FetchedBodies,
    };
    use crate::native::gmail::model::{Header, MimeBody, MimePart};
    use base64::engine::general_purpose::URL_SAFE_NO_PAD;
    use base64::Engine as _;
    use chrono::TimeZone;

    fn header(name: &str, value: &str) -> Header {
        Header {
            name: name.to_string(),
            value: value.to_string(),
        }
    }

    fn message(
        id: &str,
        date: &str,
        from: &str,
        to: &str,
        subject: &str,
        body: &str,
    ) -> GmailMessage {
        let bytes = URL_SAFE_NO_PAD.encode(body.as_bytes());
        let payload = MimePart {
            part_id: Some("0".to_string()),
            mime_type: "text/plain".to_string(),
            filename: None,
            headers: vec![header("Content-Type", "text/plain; charset=utf-8")],
            body: Some(MimeBody {
                size: Some(body.len() as u64),
                data: Some(bytes),
                attachment_id: None,
            }),
            parts: Vec::new(),
        };
        GmailMessage {
            id: id.to_string(),
            thread_id: "199c3f1e8b2d4a10".to_string(),
            label_ids: Vec::new(),
            internal_date: Some(date.to_string()),
            snippet: Some(body.to_string()),
            headers: vec![
                header("From", from),
                header("To", to),
                header("Subject", subject),
                header("Message-ID", "<one--two@example.com>"),
            ],
            payload: Some(payload),
        }
    }

    fn rendered(messages: &[GmailMessage]) -> BTreeMap<String, RenderedBody> {
        messages
            .iter()
            .map(|message| {
                (
                    message.id.clone(),
                    render_message(message, &FetchedBodies::default()).unwrap(),
                )
            })
            .collect()
    }

    #[test]
    fn full_note_has_frontmatter_markers_safe_cards_and_summary() {
        let thread = GmailThread {
            id: "199c3f1e8b2d4a10".to_string(),
            history_id: None,
            messages: vec![
                message(
                    "199c3f1e8b2d4a10",
                    "1799847992000",
                    "Dana Whitfield <dana@example.com>",
                    "Bryan <you@example.com>",
                    "Re: Lease renewal",
                    "Offer attached.",
                ),
                message(
                    "19a4b0c2d3e4f501",
                    "1799848200000",
                    "Bryan <you@example.com>",
                    "Dana Whitfield <dana@example.com>",
                    "Re: Lease renewal",
                    "Thanks Dana.",
                ),
            ],
        };
        let bodies = rendered(&thread.messages);
        let note = render(&LayoutInput {
            account: "you@example.com",
            body_mode: GmailBodyMode::Full,
            current_year: 2026,
            parent: "[[mail]]",
            rendered: &bodies,
            saved_at: Utc.timestamp_millis_opt(1799848500000).unwrap(),
            thread: &thread,
            timezone: chrono_tz::America::New_York,
        })
        .unwrap();
        let frontmatter =
            crate::native::gmail::note::parse::parse_frontmatter_only(
                &note.content,
            )
            .unwrap();
        assert_eq!(
            frontmatter
                .as_mapping()
                .unwrap()
                .get(serde_yaml::Value::String("parent".to_string()))
                .and_then(serde_yaml::Value::as_str),
            Some("[[mail]]")
        );
        assert!(note.content.contains("# Lease renewal"));
        assert!(note.content.contains("Dana Whitfield ↔ you · 2 messages"));
        assert!(note.content.contains("<!-- bob:gmail:begin v1 -->"));
        assert!(note.content.contains("<!-- bob:gmail:msg 199c3f1e8b2d4a10 %3Cone%2D%2Dtwo%40example.com%3E -->"));
        assert!(note.content.contains("> [!mail-] **Dana Whitfield**"));
        assert!(note.content.contains("> [!mail+] **You**"));
        assert!(note.content.contains("^m-19a4b0c2d3e4f501"));
        assert!(note.content.contains("https://mail.google.com/mail/?authuser=you%40example.com#all/199c3f1e8b2d4a10"));
    }

    #[test]
    fn headers_mode_uses_snippet_and_keeps_the_body_notice() {
        let thread = GmailThread {
            id: "199c3f1e8b2d4a10".to_string(),
            history_id: None,
            messages: vec![message(
                "aabbccddeeff0011",
                "1799847992000",
                "Dana <dana@example.com>",
                "you@example.com",
                "Subject",
                "snippet <script>",
            )],
        };
        let bodies = rendered(&thread.messages);
        let note = render(&LayoutInput {
            account: "you@example.com",
            body_mode: GmailBodyMode::Headers,
            current_year: 2026,
            parent: "[[mail]]",
            rendered: &bodies,
            saved_at: Utc.timestamp_millis_opt(1799848500000).unwrap(),
            thread: &thread,
            timezone: chrono_tz::America::New_York,
        })
        .unwrap();
        assert!(note.content.contains("snippet &lt;script"));
        assert!(note.content.contains("*(body not saved: headers mode)*"));
    }

    #[test]
    fn drafts_are_excluded_and_invalid_message_ids_are_rejected() {
        let mut draft = message(
            "1111111111111111",
            "1799847992000",
            "Dana <dana@example.com>",
            "you@example.com",
            "Subject",
            "body",
        );
        draft.label_ids.push("DRAFT".to_string());
        let thread = GmailThread {
            id: "aabbccddeeff0011".to_string(),
            history_id: None,
            messages: vec![draft],
        };
        let result = render(&LayoutInput {
            account: "you@example.com",
            body_mode: GmailBodyMode::Headers,
            current_year: 2026,
            parent: "[[mail]]",
            rendered: &BTreeMap::new(),
            saved_at: Utc::now(),
            thread: &thread,
            timezone: chrono_tz::UTC,
        });
        assert_eq!(result.unwrap_err(), NoteError::InvalidIdentity);
    }

    #[test]
    fn attachment_numbers_are_thread_wide_in_document_order() {
        let mut first = message(
            "aabbccddeeff0011",
            "1799847992000",
            "Dana <dana@example.com>",
            "you@example.com",
            "Lease",
            "one",
        );
        first.thread_id = "aabbccddeeff0011".to_string();
        let mut second = message(
            "bbbbccddeeff0022",
            "1799848200000",
            "Dana <dana@example.com>",
            "you@example.com",
            "Lease",
            "two",
        );
        second.thread_id = "aabbccddeeff0011".to_string();
        let thread = GmailThread {
            id: "aabbccddeeff0011".to_string(),
            history_id: None,
            messages: vec![first, second],
        };
        let mut bodies = rendered(&thread.messages);
        for (message_id, names) in [
            ("aabbccddeeff0011", vec!["offer.pdf", "plan.txt"]),
            ("bbbbccddeeff0022", vec!["photo.jpg"]),
        ] {
            bodies.get_mut(message_id).unwrap().attachments = names
                .into_iter()
                .enumerate()
                .map(|(index, filename)| Attachment {
                    index: index as u32 + 1,
                    filename: Some(filename.to_string()),
                    mime_type: "application/octet-stream".to_string(),
                    size: Some(1024),
                    attachment_id: Some(format!("part-{index}")),
                })
                .collect();
        }
        let note = render(&LayoutInput {
            account: "you@example.com",
            body_mode: GmailBodyMode::Full,
            current_year: 2026,
            parent: "[[mail]]",
            rendered: &bodies,
            saved_at: Utc.timestamp_millis_opt(1799848500000).unwrap(),
            thread: &thread,
            timezone: chrono_tz::UTC,
        })
        .unwrap();
        for expected in [
            "📎 1. offer.pdf · 1 KB",
            "📎 2. plan.txt · 1 KB",
            "📎 3. photo.jpg · 1 KB",
        ] {
            assert!(note.content.contains(expected), "missing {expected}");
        }
    }
}
