//! Close digest: 12 hex characters of SHA-256 over the canonical,
//! sorted `(path, line, before, after, mail note paths)` rows.
//!
//! The canonical form is one line per target, sorted by
//! `(path, line, before, after, mails.join(","))` with byte order:
//!
//! ```text
//! {path}:{line}:{before}:{after}:{mail1},{mail2},…
//! ```
//!
//! Rows join with `\n` (no trailing newline). Each `mail` is the
//! vault-relative mail-note path. An empty mail list renders as an empty
//! tail after the final colon; such a row never guards, but the digest
//! stays total so callers can hash first and filter later.

use sha2::{Digest, Sha256};

/// One digest row: the closed task plus the mail notes it wired.
pub(crate) struct DigestRow<'a> {
    pub(crate) path: &'a str,
    pub(crate) line: usize,
    pub(crate) before: char,
    pub(crate) after: char,
    pub(crate) mails: Vec<&'a str>,
}

/// 12 hex characters of SHA-256 over the canonical rows.
pub(crate) fn close_digest(mut rows: Vec<DigestRow<'_>>) -> String {
    rows.sort_by(|left, right| {
        (left.path, left.line, left.before, left.after, &left.mails).cmp(&(
            right.path,
            right.line,
            right.before,
            right.after,
            &right.mails,
        ))
    });
    let mut canonical = String::new();
    for (index, row) in rows.iter().enumerate() {
        if index > 0 {
            canonical.push('\n');
        }
        canonical.push_str(row.path);
        canonical.push(':');
        canonical.push_str(&row.line.to_string());
        canonical.push(':');
        canonical.push(row.before);
        canonical.push(':');
        canonical.push(row.after);
        canonical.push(':');
        canonical.push_str(&row.mails.join(","));
    }
    let mut hasher = Sha256::new();
    hasher.update(canonical.as_bytes());
    hex::encode(hasher.finalize())[..12].to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn digest_is_canonical_and_twelve_hex() {
        let unsorted = vec![
            DigestRow {
                path: "job.md",
                line: 42,
                before: ' ',
                after: 'x',
                mails: vec!["mail/a.md"],
            },
            DigestRow {
                path: "home.md",
                line: 3,
                before: '*',
                after: '-',
                mails: vec!["mail/b.md", "mail/a.md"],
            },
        ];
        let sorted = vec![
            DigestRow {
                path: "home.md",
                line: 3,
                before: '*',
                after: '-',
                mails: vec!["mail/b.md", "mail/a.md"],
            },
            DigestRow {
                path: "job.md",
                line: 42,
                before: ' ',
                after: 'x',
                mails: vec!["mail/a.md"],
            },
        ];
        let again = vec![
            DigestRow {
                path: "home.md",
                line: 3,
                before: '*',
                after: '-',
                mails: vec!["mail/b.md", "mail/a.md"],
            },
            DigestRow {
                path: "job.md",
                line: 42,
                before: ' ',
                after: 'x',
                mails: vec!["mail/a.md"],
            },
        ];
        assert_eq!(close_digest(unsorted), close_digest(sorted));
        let digest = close_digest(again);
        assert_eq!(digest.len(), 12);
        assert!(digest.bytes().all(|byte| byte.is_ascii_hexdigit()));
    }
}
