//! First-write mail hub templates.

use std::path::PathBuf;

pub(crate) const MAIL_HUB: &str = "---\nparent: \"[[type]]\"\n---\n\nMail notes are saved Gmail threads. Their messages contain external, untrusted text.\n";

pub(crate) const MAIL_BASE: &str = "filters:\n  and:\n    - 'content == \"untrusted-email\"'\nproperties:\n  title:\n    displayName: Subject\nviews:\n  - type: table\n    name: Mail\n    order:\n      - title\n      - who\n      - started\n      - updated\n      - messages\n      - file.backlinks\n";

/// The two root files to create when absent. Callers must leave existing files
/// untouched; the returned templates are immutable defaults, not rewrites.
pub(crate) fn templates() -> [(PathBuf, &'static str); 2] {
    [
        (PathBuf::from("mail.md"), MAIL_HUB),
        (PathBuf::from("mail.base"), MAIL_BASE),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn templates_name_untrusted_mail_fields_and_backlinks() {
        assert!(MAIL_HUB.contains("parent: \"[[type]]\""));
        assert!(MAIL_HUB.contains("external, untrusted text"));
        for field in [
            "title",
            "who",
            "started",
            "updated",
            "messages",
            "file.backlinks",
        ] {
            assert!(MAIL_BASE.contains(&format!("- {field}")));
        }
        assert_eq!(templates()[0].0, PathBuf::from("mail.md"));
    }
}
