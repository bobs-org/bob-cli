//! Shared Gmail Task close model: predicate, copy, and digest.
//!
//! A Gmail Task is a `#task` row whose own line, or a direct link-only
//! child bullet, wikilinks a mail note. A mail note satisfies the loose
//! glossary rule: frontmatter `type` is `"[[mail]]"` or `gmail_thread_id`
//! is a non-empty string. The strict mail index rule (which also requires
//! `content: untrusted-email` and hex ids) is never the gate, so bob and
//! the plugins agree.
//!
//! The copy builder and digest below are the shared contract rendered on
//! every surface (`docs/mail-close-card.md`). Conformance vectors live in
//! `tests/fixtures/mail_close/vectors.json`; bob-plugins copies that file
//! byte for byte.

pub(crate) mod copy;
pub(crate) mod digest;
pub(crate) mod predicate;

#[cfg(test)]
mod vectors_tests;
