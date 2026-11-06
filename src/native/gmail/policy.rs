//! Fail-closed save policy for sender addresses, domains, and Gmail labels.

use std::collections::BTreeSet;

use super::{
    config::GmailConfig,
    gog::GmailLabel,
    model::{GmailThread, HeaderKind},
    note,
    resolve::GmailRead,
    GmailError,
};

#[derive(Clone, Default)]
pub(crate) struct NeverSavePolicy {
    from: Vec<String>,
    labels: Vec<String>,
}

impl NeverSavePolicy {
    pub(crate) fn from_config(config: &GmailConfig) -> Self {
        Self {
            from: config.never_save_from().to_vec(),
            labels: config.never_save_labels().to_vec(),
        }
    }

    #[cfg(test)]
    fn new(from: &[&str], labels: &[&str]) -> Self {
        Self {
            from: from.iter().map(|value| (*value).to_string()).collect(),
            labels: labels.iter().map(|value| (*value).to_string()).collect(),
        }
    }
}

/// Refuse a thread when any non-draft message matches a configured rule.
/// Label lookup is read-only and required when labels were configured; a
/// lookup failure never permits a save to proceed.
pub(crate) fn check<R: GmailRead>(
    reader: &R,
    account: &str,
    thread: &GmailThread,
    policy: &NeverSavePolicy,
) -> Result<(), GmailError> {
    let blocked_label_ids = if policy.labels.is_empty() {
        BTreeSet::new()
    } else {
        let labels = reader.labels_list(account).map_err(label_lookup_error)?;
        resolve_label_ids(&policy.labels, &labels)
    };

    for message in &thread.messages {
        if is_draft(message) {
            continue;
        }
        if message
            .label_ids
            .iter()
            .any(|id| blocked_label_ids.contains(id))
        {
            return Err(never_save());
        }
        for value in message.header_values_of(HeaderKind::From) {
            if note::mailbox_addresses(value)
                .iter()
                .any(|address| blocked_address(address, &policy.from))
            {
                return Err(never_save());
            }
        }
    }
    Ok(())
}

fn resolve_label_ids(
    names: &[String],
    labels: &[GmailLabel],
) -> BTreeSet<String> {
    labels
        .iter()
        .filter(|label| {
            !label.id.is_empty()
                && names.iter().any(|name| {
                    !name.trim().is_empty()
                        && label.name.eq_ignore_ascii_case(name.trim())
                })
        })
        .map(|label| label.id.clone())
        .collect()
}

fn is_draft(message: &super::model::GmailMessage) -> bool {
    message
        .label_ids
        .iter()
        .any(|label| label.eq_ignore_ascii_case("DRAFT"))
}

fn blocked_address(address: &str, blocked: &[String]) -> bool {
    let address = address.trim();
    blocked.iter().any(|entry| {
        let entry = entry.trim();
        if let Some(domain) = entry.strip_prefix('@') {
            !domain.is_empty()
                && address
                    .to_ascii_lowercase()
                    .ends_with(&format!("@{}", domain.to_ascii_lowercase()))
        } else {
            !entry.is_empty() && address.eq_ignore_ascii_case(entry)
        }
    })
}

fn label_lookup_error(error: GmailError) -> GmailError {
    match error.kind() {
        "network" | "timeout" => error,
        "invalid_grant" | "not_logged_in" | "insufficient_scope" => {
            GmailError::runtime(
                "awaiting_sign_in",
                "the bob-gmail read grant needs attention".to_string(),
            )
        }
        _ => GmailError::runtime(
            "refused",
            "could not verify configured never_save labels".to_string(),
        )
        .with_hint("check Gmail read access and the configured label names"),
    }
}

fn never_save() -> GmailError {
    GmailError::runtime(
        "never_save",
        "this thread matches a configured never_save rule".to_string(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::native::gmail::model::GmailThread;

    fn thread(messages: &str) -> GmailThread {
        GmailThread::from_gog_json(
            format!(r#"{{"thread":{{"id":"t1","messages":[{messages}]}}}}"#)
                .as_bytes(),
        )
        .expect("synthetic thread")
    }

    #[test]
    fn sender_rules_use_addresses_domains_and_ignore_display_names() {
        let policy = NeverSavePolicy::new(
            &["blocked@example.com", "@spam.example"],
            &[],
        );
        let blocked = thread(
            r#"{"id":"m1","threadId":"t1","payload":{"headers":[{"name":"From","value":"Offers <daily@SPAM.EXAMPLE>"}]}}"#,
        );
        assert_eq!(
            check(&NoLabels, "me@example.com", &blocked, &policy)
                .expect_err("domain match")
                .kind(),
            "never_save"
        );

        let lookalike = thread(
            r#"{"id":"m1","threadId":"t1","payload":{"headers":[{"name":"From","value":"offers@notspam.example"}]}}"#,
        );
        assert!(check(&NoLabels, "me@example.com", &lookalike, &policy).is_ok());

        let exact = thread(
            r#"{"id":"m1","threadId":"t1","payload":{"headers":[{"name":"From","value":"Blocked <Blocked@Example.com>"}]}}"#,
        );
        assert_eq!(
            check(&NoLabels, "me@example.com", &exact, &policy)
                .expect_err("exact address match")
                .kind(),
            "never_save"
        );

        let display_only = thread(
            r#"{"id":"m1","threadId":"t1","payload":{"headers":[{"name":"From","value":"blocked@example.com <not-an-address>"}]}}"#,
        );
        assert!(
            check(&NoLabels, "me@example.com", &display_only, &policy).is_ok()
        );
    }

    #[test]
    fn label_names_fold_but_label_ids_remain_exact_and_drafts_are_ignored() {
        let policy = NeverSavePolicy::new(&[], &["Promotions"]);
        let draft = thread(
            r#"{"id":"m1","threadId":"t1","labelIds":["DRAFT","promo-id"],"payload":{"headers":[]}}"#,
        );
        assert!(check(&LabelReader, "me@example.com", &draft, &policy).is_ok());

        let delivered = thread(
            r#"{"id":"m2","threadId":"t1","labelIds":["promo-id"],"payload":{"headers":[]}}"#,
        );
        assert_eq!(
            check(&LabelReader, "me@example.com", &delivered, &policy)
                .expect_err("blocked label")
                .kind(),
            "never_save"
        );

        let unrelated = thread(
            r#"{"id":"m3","threadId":"t1","labelIds":["promotions"],"payload":{"headers":[]}}"#,
        );
        assert!(
            check(&LabelReader, "me@example.com", &unrelated, &policy).is_ok()
        );
    }

    #[test]
    fn label_lookup_failure_refuses_instead_of_skipping_policy() {
        let policy = NeverSavePolicy::new(&[], &["Promotions"]);
        let thread = thread(
            r#"{"id":"m1","threadId":"t1","labelIds":["promo-id"],"payload":{"headers":[]}}"#,
        );
        let error = check(&FailLabels, "me@example.com", &thread, &policy)
            .expect_err("failed lookup must refuse");
        assert_eq!(error.kind(), "refused");
        assert!(!error.message().contains("PRIVATE_SENTINEL"));
    }

    struct NoLabels;
    struct LabelReader;
    struct FailLabels;

    impl GmailRead for NoLabels {
        fn thread_get(
            &self,
            _: &str,
            _: &str,
        ) -> Result<GmailThread, GmailError> {
            unreachable!()
        }
        fn message_metadata(
            &self,
            _: &str,
            _: &str,
        ) -> Result<serde_json::Value, GmailError> {
            unreachable!()
        }
        fn search_page(
            &self,
            _: &str,
            _: &str,
            _: u32,
            _: Option<&str>,
        ) -> Result<serde_json::Value, GmailError> {
            unreachable!()
        }
        fn labels_list(&self, _: &str) -> Result<Vec<GmailLabel>, GmailError> {
            Ok(Vec::new())
        }
    }

    impl GmailRead for LabelReader {
        fn thread_get(
            &self,
            _: &str,
            _: &str,
        ) -> Result<GmailThread, GmailError> {
            unreachable!()
        }
        fn message_metadata(
            &self,
            _: &str,
            _: &str,
        ) -> Result<serde_json::Value, GmailError> {
            unreachable!()
        }
        fn search_page(
            &self,
            _: &str,
            _: &str,
            _: u32,
            _: Option<&str>,
        ) -> Result<serde_json::Value, GmailError> {
            unreachable!()
        }
        fn labels_list(&self, _: &str) -> Result<Vec<GmailLabel>, GmailError> {
            Ok(vec![GmailLabel {
                id: "promo-id".to_string(),
                name: "promotions".to_string(),
            }])
        }
    }

    impl GmailRead for FailLabels {
        fn thread_get(
            &self,
            _: &str,
            _: &str,
        ) -> Result<GmailThread, GmailError> {
            unreachable!()
        }
        fn message_metadata(
            &self,
            _: &str,
            _: &str,
        ) -> Result<serde_json::Value, GmailError> {
            unreachable!()
        }
        fn search_page(
            &self,
            _: &str,
            _: &str,
            _: u32,
            _: Option<&str>,
        ) -> Result<serde_json::Value, GmailError> {
            unreachable!()
        }
        fn labels_list(&self, _: &str) -> Result<Vec<GmailLabel>, GmailError> {
            Err(GmailError::runtime(
                "gog",
                "PRIVATE_SENTINEL remote error".to_string(),
            ))
        }
    }
}
