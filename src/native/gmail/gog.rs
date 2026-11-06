//! Gmail API methods layered on the shared Google gog transport.

use std::ffi::OsString;

use super::GmailError;
pub(crate) use crate::native::google::Gog;

use crate::native::google::parse_json;

pub(crate) const CLIENT: &str = crate::native::google::GMAIL.client_bucket;
pub(crate) const API_CONSOLE_URL: &str =
    crate::native::google::GMAIL.api_console_url;
pub(crate) use crate::native::google::REVOKE_URL;

/// Read-only Gmail commands Bob may invoke.
const READ_COMMANDS: &str =
    "gmail.thread.get,gmail.get,gmail.messages,gmail.search,gmail.url,gmail.attachment";

fn read_flags(account: &str) -> Vec<OsString> {
    vec![
        OsString::from("--gmail-no-send"),
        OsString::from("--enable-commands"),
        OsString::from(READ_COMMANDS),
        OsString::from("--account"),
        OsString::from(account),
    ]
}

impl Gog {
    fn gmail_run(
        &self,
        account: &str,
        command: Vec<OsString>,
    ) -> Result<String, GmailError> {
        let mut args = self.common_flags(Some(account));
        args.extend(read_flags(account));
        args.extend(command);
        let output = self.run(args)?;
        Ok(output.stdout)
    }

    // Consumed by gmail-save; shell check uses `search` only.
    #[allow(dead_code)]
    /// Fetch one thread with all messages.
    pub(crate) fn thread_get(
        &self,
        account: &str,
        hex: &str,
    ) -> Result<serde_json::Value, GmailError> {
        let stdout = self.gmail_run(
            account,
            vec![
                OsString::from("gmail"),
                OsString::from("thread"),
                OsString::from("get"),
                OsString::from(hex),
                OsString::from("--full"),
            ],
        )?;
        let value = parse_json(&stdout)?;
        Ok(value.get("thread").cloned().unwrap_or(value))
    }

    #[allow(dead_code)]
    /// Fetch one message.
    pub(crate) fn message_get(
        &self,
        account: &str,
        hex: &str,
    ) -> Result<serde_json::Value, GmailError> {
        let stdout = self.gmail_run(
            account,
            vec![
                OsString::from("gmail"),
                OsString::from("get"),
                OsString::from(hex),
                OsString::from("--format"),
                OsString::from("full"),
            ],
        )?;
        let value = parse_json(&stdout)?;
        Ok(value.get("message").cloned().unwrap_or(value))
    }

    /// Search threads with Gmail query syntax.
    pub(crate) fn search(
        &self,
        account: &str,
        query: &str,
        max: u32,
    ) -> Result<serde_json::Value, GmailError> {
        let stdout = self.gmail_run(
            account,
            vec![
                OsString::from("gmail"),
                OsString::from("search"),
                OsString::from(query),
                OsString::from("--max"),
                OsString::from(max.to_string()),
            ],
        )?;
        parse_json(&stdout)
    }

    #[allow(dead_code)]
    /// Print Gmail web URLs for threads.
    pub(crate) fn url(
        &self,
        account: &str,
        hex: &str,
    ) -> Result<serde_json::Value, GmailError> {
        let stdout = self.gmail_run(
            account,
            vec![
                OsString::from("gmail"),
                OsString::from("url"),
                OsString::from(hex),
            ],
        )?;
        parse_json(&stdout)
    }

    #[allow(dead_code)]
    /// Download one attachment as bytes via `--inline` base64.
    pub(crate) fn attachment(
        &self,
        account: &str,
        message: &str,
        attachment: &str,
    ) -> Result<Vec<u8>, GmailError> {
        let stdout = self.gmail_run(
            account,
            vec![
                OsString::from("gmail"),
                OsString::from("attachment"),
                OsString::from(message),
                OsString::from(attachment),
                OsString::from("--inline"),
            ],
        )?;
        let value = parse_json(&stdout)?;
        let encoded = value
            .get("contentBase64")
            .and_then(|v| v.as_str())
            .or_else(|| value.get("data").and_then(|v| v.as_str()))
            .ok_or_else(|| {
                GmailError::runtime(
                    "bad_json",
                    "gog attachment printed no content".to_string(),
                )
            })?;
        let cleaned: String =
            encoded.chars().filter(|ch| !ch.is_whitespace()).collect();
        base64_decode(&cleaned).ok_or_else(|| {
            GmailError::runtime(
                "bad_json",
                "gog attachment content is not base64".to_string(),
            )
        })
    }
}

#[allow(dead_code)]
fn base64_decode(input: &str) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(input.len() * 3 / 4);
    let mut buffer: u32 = 0;
    let mut bits: u8 = 0;
    for ch in input.chars() {
        if ch == '=' {
            break;
        }
        let value = base64_value(ch)?;
        buffer = (buffer << 6) | value as u32;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push(((buffer >> bits) & 0xFF) as u8);
        }
    }
    Some(out)
}

#[allow(dead_code)]
fn base64_value(ch: char) -> Option<u8> {
    match ch {
        'A'..='Z' => Some(ch as u8 - b'A'),
        'a'..='z' => Some(ch as u8 - b'a' + 26),
        '0'..='9' => Some(ch as u8 - b'0' + 52),
        '+' => Some(62),
        '/' => Some(63),
        '-' => Some(62),
        '_' => Some(63),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_url_safe_base64() {
        let bytes = base64_decode("aGVsbG8td29ybGQ_").expect("decode");
        assert_eq!(bytes, b"hello-world?");
    }
}
