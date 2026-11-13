//! First-write mail hub templates.

use std::path::{Path, PathBuf};

pub(crate) const MAIL_HUB: &str = "---\nparent: \"[[type]]\"\n---\n\nMail notes are saved Gmail threads. Their messages contain external, untrusted text.\n";

/// The `mail.base` Bob shipped before the turn fields. A vault file that is
/// byte-identical to it is untouched, so Bob may upgrade it.
pub(crate) const MAIL_BASE_V1: &str = "filters:\n  and:\n    - 'content == \"untrusted-email\"'\nproperties:\n  title:\n    displayName: Subject\nviews:\n  - type: table\n    name: Mail\n    order:\n      - title\n      - who\n      - started\n      - updated\n      - messages\n      - file.backlinks\n";

/// The `mail.base` Bob shipped with turn fields but without the archive
/// receipts view. A vault file byte-identical to it is untouched, so Bob may
/// upgrade it.
pub(crate) const MAIL_BASE_V2: &str = "filters:\n  and:\n    - 'content == \"untrusted-email\"'\nproperties:\n  title:\n    displayName: Subject\nviews:\n  - type: table\n    name: Mail\n    order:\n      - title\n      - who\n      - started\n      - updated\n      - messages\n      - file.backlinks\n  - type: table\n    name: Waiting for reply\n    filters:\n      and:\n        - 'turn == \"theirs\"'\n    order:\n      - title\n      - who\n      - turn_since\n      - file.backlinks\n    sort:\n      - property: turn_since\n        direction: ASC\n";

pub(crate) const MAIL_BASE: &str = "filters:\n  and:\n    - 'content == \"untrusted-email\"'\nproperties:\n  title:\n    displayName: Subject\nviews:\n  - type: table\n    name: Mail\n    order:\n      - title\n      - who\n      - started\n      - updated\n      - messages\n      - file.backlinks\n  - type: table\n    name: Waiting for reply\n    filters:\n      and:\n        - 'turn == \"theirs\"'\n    order:\n      - title\n      - who\n      - turn_since\n      - file.backlinks\n    sort:\n      - property: turn_since\n        direction: ASC\n  - type: table\n    name: Archived by Bob\n    filters:\n      and:\n        - 'gmail_archived != \"\"'\n    order:\n      - title\n      - who\n      - gmail_archived\n      - file.backlinks\n    sort:\n      - property: gmail_archived\n        direction: DESC\n";

/// The two root files to create when absent. Callers must leave existing files
/// untouched unless [`upgrade`] names a replacement.
pub(crate) fn templates() -> [(PathBuf, &'static str); 2] {
    [
        (PathBuf::from("mail.md"), MAIL_HUB),
        (PathBuf::from("mail.base"), MAIL_BASE),
    ]
}

/// The current template for a hub whose bytes are exactly a previously
/// shipped version. Both known old `mail.base` versions upgrade; a
/// customized hub, or one already current, gets `None`.
pub(crate) fn upgrade(
    relative: &Path,
    existing: &[u8],
) -> Option<&'static str> {
    (relative == Path::new("mail.base")
        && (existing == MAIL_BASE_V1.as_bytes()
            || existing == MAIL_BASE_V2.as_bytes()))
    .then_some(MAIL_BASE)
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
            "turn_since",
        ] {
            assert!(MAIL_BASE.contains(&format!("- {field}")));
        }
        assert_eq!(templates()[0].0, PathBuf::from("mail.md"));
    }

    #[test]
    fn mail_base_keeps_the_mail_view_and_adds_waiting_for_reply() {
        assert!(MAIL_BASE.starts_with(MAIL_BASE_V2));
        assert!(MAIL_BASE_V2.starts_with(MAIL_BASE_V1));
        let base: serde_yaml::Value = serde_yaml::from_str(MAIL_BASE).unwrap();
        let views = base["views"].as_sequence().unwrap();
        assert_eq!(views.len(), 3);
        let waiting = &views[1];
        assert_eq!(waiting["name"], "Waiting for reply");
        assert_eq!(waiting["filters"]["and"][0], "turn == \"theirs\"");
        assert_eq!(waiting["sort"][0]["property"], "turn_since");
        assert_eq!(waiting["sort"][0]["direction"], "ASC");
        let order: Vec<_> = waiting["order"]
            .as_sequence()
            .unwrap()
            .iter()
            .map(|value| value.as_str().unwrap())
            .collect();
        assert_eq!(order, ["title", "who", "turn_since", "file.backlinks"]);
    }

    #[test]
    fn archived_view_filters_nonempty_receipts_newest_first() {
        let base: serde_yaml::Value = serde_yaml::from_str(MAIL_BASE).unwrap();
        let views = base["views"].as_sequence().unwrap();
        let archived = &views[2];
        assert_eq!(archived["name"], "Archived by Bob");
        assert_eq!(archived["type"], "table");
        assert_eq!(archived["filters"]["and"][0], "gmail_archived != \"\"");
        let order: Vec<_> = archived["order"]
            .as_sequence()
            .unwrap()
            .iter()
            .map(|value| value.as_str().unwrap())
            .collect();
        assert_eq!(order, ["title", "who", "gmail_archived", "file.backlinks"]);
        assert_eq!(archived["sort"][0]["property"], "gmail_archived");
        assert_eq!(archived["sort"][0]["direction"], "DESC");
    }

    #[test]
    fn only_untouched_old_mail_bases_upgrade() {
        let base = Path::new("mail.base");
        assert_eq!(upgrade(base, MAIL_BASE_V1.as_bytes()), Some(MAIL_BASE));
        assert_eq!(upgrade(base, MAIL_BASE_V2.as_bytes()), Some(MAIL_BASE));
        assert_eq!(upgrade(base, MAIL_BASE.as_bytes()), None);
        let customized = MAIL_BASE_V2.replace("Subject", "Topic");
        assert_eq!(upgrade(base, customized.as_bytes()), None);
        let customized_v1 = MAIL_BASE_V1.replace("Subject", "Topic");
        assert_eq!(upgrade(base, customized_v1.as_bytes()), None);
        assert_eq!(
            upgrade(Path::new("mail.md"), MAIL_BASE_V1.as_bytes()),
            None
        );
    }
}
