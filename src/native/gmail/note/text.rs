//! Shared pure text helpers for mail-note modules.

use crate::native::gmail::model::{GmailMessage, HeaderKind};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Mailbox {
    pub(super) display: String,
    pub(super) address: Option<String>,
}

pub(super) fn subject_without_prefixes(subject: &str) -> String {
    let mut subject = subject.trim();
    loop {
        let Some((prefix, rest)) = subject.split_once(':') else {
            break;
        };
        if !matches!(
            prefix.trim().to_ascii_lowercase().as_str(),
            "re" | "fw" | "fwd" | "aw" | "sv"
        ) {
            break;
        }
        subject = rest.trim_start();
    }
    subject.trim().to_string()
}

pub(super) fn parse_mailboxes(raw: &str) -> Vec<Mailbox> {
    split_mailbox_list(raw)
        .into_iter()
        .filter_map(|piece| parse_mailbox(piece.trim()))
        .collect()
}

pub(super) fn message_mailboxes(
    message: &GmailMessage,
    kind: HeaderKind,
) -> Vec<Mailbox> {
    message
        .header_values_of(kind)
        .flat_map(parse_mailboxes)
        .collect()
}

pub(super) fn display_for_account(mailbox: &Mailbox, account: &str) -> String {
    if mailbox
        .address
        .as_deref()
        .is_some_and(|address| address.eq_ignore_ascii_case(account.trim()))
    {
        "you".to_string()
    } else {
        mailbox.display.clone()
    }
}

fn split_mailbox_list(raw: &str) -> Vec<&str> {
    let mut result = Vec::new();
    let mut start = 0;
    let mut quoted = false;
    let mut escaped = false;
    let mut angle_depth = 0usize;
    for (index, character) in raw.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        match character {
            '\\' if quoted => escaped = true,
            '"' => quoted = !quoted,
            '<' if !quoted => angle_depth = angle_depth.saturating_add(1),
            '>' if !quoted => angle_depth = angle_depth.saturating_sub(1),
            ',' if !quoted && angle_depth == 0 => {
                result.push(&raw[start..index]);
                start = index + character.len_utf8();
            }
            _ => {}
        }
    }
    result.push(&raw[start..]);
    result
}

fn parse_mailbox(raw: &str) -> Option<Mailbox> {
    if raw.is_empty() {
        return None;
    }
    if let Some(open) = raw.rfind('<')
        && let Some(close_rel) = raw[open + 1..].find('>')
    {
        let close = open + 1 + close_rel;
        let address = raw[open + 1..close].trim();
        if address.is_empty() {
            return None;
        }
        let display = unquote_display(raw[..open].trim());
        return Some(Mailbox {
            display: if display.is_empty() {
                address.to_string()
            } else {
                display
            },
            address: Some(address.to_string()),
        });
    }
    let value = raw.trim_matches(['<', '>']).trim();
    if value.is_empty() {
        return None;
    }
    let address = value.contains('@').then(|| value.to_string());
    Some(Mailbox {
        display: value.to_string(),
        address,
    })
}

fn unquote_display(display: &str) -> String {
    let display = display.trim();
    if display.len() >= 2 && display.starts_with('"') && display.ends_with('"')
    {
        display[1..display.len() - 1]
            .replace("\\\"", "\"")
            .replace("\\\\", "\\")
    } else {
        display.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_repeated_reply_and_forward_prefixes() {
        assert_eq!(
            subject_without_prefixes(" Re: FWD: Sv: Renewal "),
            "Renewal"
        );
        assert_eq!(subject_without_prefixes("Review: lease"), "Review: lease");
    }

    #[test]
    fn splits_quoted_and_display_name_addresses() {
        let parsed = parse_mailboxes(
            "\"Dana, Whitfield\" <dana@example.com>, Bryan <me@example.com>",
        );
        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed[0].display, "Dana, Whitfield");
        assert_eq!(parsed[0].address.as_deref(), Some("dana@example.com"));
        assert_eq!(display_for_account(&parsed[1], "me@example.com"), "you");
    }
}
