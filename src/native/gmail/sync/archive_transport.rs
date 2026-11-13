//! Constrained label operations used by Gmail archive-on-close.
//!
//! Callers cannot supply arbitrary label deltas or arbitrary message IDs. A
//! receipt label is resolved to a user label first, and every operation is
//! reduced to the one allowed label pair before it reaches gog.

use std::collections::{BTreeSet, HashSet};

use super::super::{
    gog::{GmailLabel, Gog},
    GmailError,
};

pub(crate) const MAX_BATCH_MESSAGES: usize = 1000;

/// A validated user label with a stable Gmail ID.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ReceiptLabel {
    id: String,
    name: String,
}

impl ReceiptLabel {
    pub(crate) fn new(
        label: &GmailLabel,
        enrollment: &GmailLabel,
    ) -> Result<Self, GmailError> {
        Self::validated(label, Some(enrollment))
    }

    fn validated(
        label: &GmailLabel,
        enrollment: Option<&GmailLabel>,
    ) -> Result<Self, GmailError> {
        let id = label.id.trim();
        let name = label.name.trim();
        let valid_id = !id.is_empty()
            && id.len() <= 512
            && id.bytes().all(|byte| {
                byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-')
            });
        let reserved = is_reserved_label_name(name)
            || matches!(
                id.to_ascii_uppercase().as_str(),
                "INBOX"
                    | "SPAM"
                    | "TRASH"
                    | "UNREAD"
                    | "STARRED"
                    | "IMPORTANT"
                    | "SENT"
                    | "DRAFT"
                    | "CHAT"
            )
            || id.to_ascii_uppercase().starts_with("CATEGORY_");
        let user_label = label.is_user();
        let aliases_enrollment = enrollment.is_some_and(|enrollment| {
            name.eq_ignore_ascii_case(enrollment.name.trim())
                || id == enrollment.id
        });
        if !valid_id
            || name.is_empty()
            || name.len() > 512
            || name.chars().any(char::is_control)
            || !user_label
            || reserved
            || aliases_enrollment
        {
            return Err(GmailError::setup(
                "archive_label",
                "the Gmail receipt label is invalid, reserved, or aliases the enrollment label".to_string(),
            ));
        }
        Ok(Self {
            id: id.to_string(),
            name: name.to_string(),
        })
    }

    pub(crate) fn id(&self) -> &str {
        &self.id
    }

    #[allow(dead_code)] // Retained for the preview/apply outcome boundary.
    pub(crate) fn name(&self) -> &str {
        &self.name
    }
}

/// Resolve the configured label, creating it on first approved use. A
/// concurrent create may win between list and create; after a create error,
/// re-list and accept only the exact now-existing valid user label.
#[allow(dead_code)] // Wired by the serial preview/apply phases.
pub(crate) fn resolve_or_create_receipt_label(
    gog: &Gog,
    account: &str,
    name: &str,
    enrollment: &GmailLabel,
) -> Result<ReceiptLabel, GmailError> {
    validate_receipt_name(name, enrollment)?;
    let labels = gog.labels_list(account)?;
    if let Some(label) = unique_named_label(&labels, name)? {
        return ReceiptLabel::new(label, enrollment);
    }

    match gog.archive_label_create(account, name) {
        Ok(label) => ReceiptLabel::new(&label, enrollment),
        Err(create_error) => {
            if let Ok(labels) = gog.labels_list(account)
                && let Some(label) = unique_named_label(&labels, name)?
            {
                return ReceiptLabel::new(label, enrollment);
            }
            Err(create_error)
        }
    }
}

fn unique_named_label<'a>(
    labels: &'a [GmailLabel],
    name: &str,
) -> Result<Option<&'a GmailLabel>, GmailError> {
    let mut matches = labels.iter().filter(|label| label.name == name);
    let Some(first) = matches.next() else {
        return Ok(None);
    };
    if matches.next().is_some() {
        return Err(GmailError::setup(
            "archive_label",
            "the configured Gmail receipt label name is ambiguous".to_string(),
        ));
    }
    Ok(Some(first))
}

/// Resolve a journaled receipt through the current Gmail label list. A
/// recorded ID is authoritative; legacy ID-less receipts require one exact
/// name match. Both paths validate the actual label against the enrollment
/// label before allowing a mutation.
pub(crate) fn resolve_existing_receipt_label(
    labels: &[GmailLabel],
    identity: &super::archive_state::ArchiveLabelIdentity,
    enrollment: &GmailLabel,
) -> Result<Option<ReceiptLabel>, GmailError> {
    let label = if let Some(id) = identity.id.as_deref() {
        let mut matches = labels.iter().filter(|label| label.id == id);
        let Some(label) = matches.next() else {
            return Ok(None);
        };
        if matches.next().is_some() {
            return Err(GmailError::setup(
                "archive_label",
                "the recorded Gmail receipt label ID is ambiguous".to_string(),
            ));
        }
        label
    } else {
        let Some(label) = unique_named_label(labels, &identity.name)? else {
            return Ok(None);
        };
        label
    };
    ReceiptLabel::new(label, enrollment).map(Some)
}

/// Resolve the stable recorded receipt identity without requiring the
/// current enrollment label to remain configured. A caller that still has
/// an enrollment label passes it to reject a current alias; the journal
/// proves the receipt was valid when it was created.
pub(crate) fn resolve_recorded_receipt_label(
    labels: &[GmailLabel],
    identity: &super::archive_state::ArchiveLabelIdentity,
    enrollment: Option<&GmailLabel>,
) -> Result<Option<ReceiptLabel>, GmailError> {
    let label = if let Some(id) = identity.id.as_deref() {
        let mut matches = labels.iter().filter(|label| label.id == id);
        let Some(label) = matches.next() else {
            return Ok(None);
        };
        if matches.next().is_some() {
            return Err(GmailError::setup(
                "archive_label",
                "the recorded Gmail receipt label ID is ambiguous".to_string(),
            ));
        }
        label
    } else {
        let Some(label) = unique_named_label(labels, &identity.name)? else {
            return Ok(None);
        };
        label
    };
    ReceiptLabel::validated(label, enrollment).map(Some)
}

fn validate_receipt_name(
    name: &str,
    enrollment: &GmailLabel,
) -> Result<(), GmailError> {
    let name = name.trim();
    if name.is_empty()
        || name.len() > 512
        || name.chars().any(char::is_control)
        || is_reserved_label_name(name)
        || name.eq_ignore_ascii_case(enrollment.name.trim())
    {
        return Err(GmailError::setup(
            "archive_label",
            "the Gmail receipt label is invalid, reserved, or aliases the enrollment label".to_string(),
        ));
    }
    Ok(())
}

/// One explicit, sorted list of message IDs accepted by Gmail batch modify.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct MessageIds(Vec<String>);

impl MessageIds {
    pub(crate) fn new(
        ids: impl IntoIterator<Item = String>,
    ) -> Result<Self, GmailError> {
        let ids: Vec<String> = ids.into_iter().collect();
        if ids.is_empty()
            || ids.len() > MAX_BATCH_MESSAGES
            || ids.iter().any(|id| {
                id.is_empty()
                    || id.len() > 256
                    || !id.bytes().all(|byte| byte.is_ascii_hexdigit())
            })
        {
            return Err(invalid_message_ids());
        }
        let unique: BTreeSet<String> = ids.iter().cloned().collect();
        if unique.len() != ids.len() {
            return Err(invalid_message_ids());
        }
        Ok(Self(unique.into_iter().collect()))
    }

    pub(crate) fn as_slice(&self) -> &[String] {
        &self.0
    }
}

/// The only operations this boundary can express.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LabelOperation {
    Archive,
    #[allow(dead_code)] // Wired by the outer receipts phase.
    Restore,
    LetGo,
}

/// Current labels observed for one validated message ID.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct MessageLabelState {
    pub(crate) message_id: String,
    pub(crate) label_ids: BTreeSet<String>,
}

/// A safe batch request. Fields are private so arbitrary remote deltas cannot
/// escape this module's operation mapping.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LabelDiff {
    message_ids: MessageIds,
    add: Vec<String>,
    remove: Vec<String>,
}

impl LabelDiff {
    pub(crate) fn for_messages(
        operation: LabelOperation,
        receipt: &ReceiptLabel,
        messages: &[MessageLabelState],
    ) -> Result<Self, GmailError> {
        let message_ids = MessageIds::new(
            messages.iter().map(|message| message.message_id.clone()),
        )?;
        let expected: HashSet<&str> =
            message_ids.as_slice().iter().map(String::as_str).collect();
        if messages.len() != expected.len()
            || messages.iter().any(|message| {
                !expected.contains(message.message_id.as_str())
                    || message.label_ids.iter().any(|label| {
                        label.is_empty() || label.chars().any(char::is_control)
                    })
            })
        {
            return Err(invalid_message_ids());
        }

        let has_any = |label: &str| {
            messages
                .iter()
                .any(|message| message.label_ids.contains(label))
        };
        let lacks_any = |label: &str| {
            messages
                .iter()
                .any(|message| !message.label_ids.contains(label))
        };

        let (add, remove) = match operation {
            LabelOperation::Archive => (
                lacks_any(receipt.id()).then(|| receipt.id().to_string()),
                has_any("INBOX").then(|| "INBOX".to_string()),
            ),
            LabelOperation::Restore => (
                lacks_any("INBOX").then(|| "INBOX".to_string()),
                has_any(receipt.id()).then(|| receipt.id().to_string()),
            ),
            LabelOperation::LetGo => (
                None,
                has_any(receipt.id()).then(|| receipt.id().to_string()),
            ),
        };

        Ok(Self {
            message_ids,
            add: add.into_iter().collect(),
            remove: remove.into_iter().collect(),
        })
    }

    pub(crate) fn message_ids(&self) -> &[String] {
        self.message_ids.as_slice()
    }

    pub(crate) fn add_labels(&self) -> &[String] {
        &self.add
    }

    pub(crate) fn remove_labels(&self) -> &[String] {
        &self.remove
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.add.is_empty() && self.remove.is_empty()
    }
}

fn invalid_message_ids() -> GmailError {
    GmailError::setup(
        "message_ids",
        "Gmail label mutation requires 1–1000 unique, valid message IDs"
            .to_string(),
    )
}

fn is_reserved_label_name(name: &str) -> bool {
    const RESERVED: &[&str] = &[
        "INBOX",
        "SPAM",
        "TRASH",
        "UNREAD",
        "STARRED",
        "IMPORTANT",
        "SENT",
        "DRAFT",
        "CHAT",
    ];
    RESERVED
        .iter()
        .any(|reserved| name.eq_ignore_ascii_case(reserved))
        || name
            .get(.."CATEGORY_".len())
            .is_some_and(|prefix| prefix.eq_ignore_ascii_case("CATEGORY_"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn receipt() -> ReceiptLabel {
        ReceiptLabel::new(
            &GmailLabel {
                id: "Label_42".to_string(),
                name: "Saved, closed\\later".to_string(),
                kind: Some("user".to_string()),
            },
            &GmailLabel {
                id: "Label_1".to_string(),
                name: "saved".to_string(),
                kind: Some("user".to_string()),
            },
        )
        .unwrap()
    }

    fn message(id: &str, labels: &[&str]) -> MessageLabelState {
        MessageLabelState {
            message_id: id.to_string(),
            label_ids: labels
                .iter()
                .map(|label| (*label).to_string())
                .collect(),
        }
    }

    fn apply(labels: &mut BTreeSet<String>, diff: &LabelDiff) {
        for label in diff.remove_labels() {
            labels.remove(label);
        }
        for label in diff.add_labels() {
            labels.insert(label.clone());
        }
    }

    fn apply_all(states: &mut [MessageLabelState], diff: &LabelDiff) {
        for state in states {
            apply(&mut state.label_ids, diff);
        }
    }

    fn arbitrary_label_id() -> impl Strategy<Value = String> {
        prop::collection::vec(
            prop_oneof![
                Just(','),
                Just('\\'),
                Just('/'),
                Just('_'),
                Just('-'),
                (b'a'..=b'z').prop_map(|value| value as char),
                (b'0'..=b'9').prop_map(|value| value as char),
            ],
            1..20,
        )
        .prop_map(|chars| chars.into_iter().collect())
    }

    #[test]
    fn receipt_label_requires_a_non_reserved_non_alias_user_label() {
        for label in [
            GmailLabel {
                id: "INBOX".into(),
                name: "Inbox".into(),
                kind: Some("system".into()),
            },
            GmailLabel {
                id: "Label_2".into(),
                name: "saved".into(),
                kind: Some("user".into()),
            },
            GmailLabel {
                id: "Label_3".into(),
                name: "Custom".into(),
                kind: Some("system".into()),
            },
            GmailLabel {
                id: "Label_4".into(),
                name: "CATEGORY_PERSONAL".into(),
                kind: Some("user".into()),
            },
        ] {
            assert!(
                ReceiptLabel::new(
                    &label,
                    &GmailLabel {
                        id: "Label_1".into(),
                        name: "saved".into(),
                        kind: Some("user".into())
                    }
                )
                .is_err(),
                "accepted {label:?}"
            );
        }
        assert!(ReceiptLabel::new(
            &GmailLabel {
                id: "Label_2".into(),
                name: "Separate".into(),
                kind: Some("user".into())
            },
            &GmailLabel {
                id: "Label_2".into(),
                name: "Enrollment".into(),
                kind: Some("user".into())
            }
        )
        .is_err());
    }

    #[test]
    fn idless_receipt_resolves_by_unique_name_and_real_enrollment_identity() {
        let labels = vec![
            GmailLabel {
                id: "Label_9".into(),
                name: "enrolled".into(),
                kind: Some("user".into()),
            },
            GmailLabel {
                id: "Label_42".into(),
                name: "saved, closed\\later".into(),
                kind: Some("user".into()),
            },
        ];
        let enrollment = labels[0].clone();
        let legacy = super::super::archive_state::ArchiveLabelIdentity::new(
            "saved, closed\\later",
            None,
        )
        .unwrap();
        let resolved =
            resolve_existing_receipt_label(&labels, &legacy, &enrollment)
                .unwrap()
                .unwrap();
        assert_eq!(resolved.id(), "Label_42");

        let mut duplicate = labels.clone();
        duplicate.push(GmailLabel {
            id: "Label_43".into(),
            name: "saved, closed\\later".into(),
            kind: Some("user".into()),
        });
        assert!(resolve_existing_receipt_label(
            &duplicate,
            &legacy,
            &enrollment
        )
        .is_err());

        let aliases_enrollment =
            super::super::archive_state::ArchiveLabelIdentity::new(
                "saved, closed\\later",
                Some("Label_9".into()),
            )
            .unwrap();
        assert!(resolve_existing_receipt_label(
            &labels,
            &aliases_enrollment,
            &enrollment
        )
        .is_err());
    }

    #[test]
    fn message_ids_reject_empty_duplicates_invalid_and_over_limit_lists() {
        assert!(MessageIds::new(Vec::<String>::new()).is_err());
        assert!(MessageIds::new(vec!["abcd".into(), "abcd".into()]).is_err());
        assert!(MessageIds::new(vec!["not-hex".into()]).is_err());
        assert!(MessageIds::new(
            (0..=MAX_BATCH_MESSAGES).map(|_| "abcd".to_string())
        )
        .is_err());
    }

    #[test]
    fn label_diffs_preserve_unrelated_labels_and_are_idempotent() {
        let receipt = receipt();
        // Enumerate every combination of the two owned labels and six
        // unrelated labels over a three-message thread.
        let labels = [
            "INBOX", "Label_42", "UNREAD", "STARRED", "Label_8", "Label_9",
            "Label_10", "Label_11",
        ];
        for mask in 0u16..(1 << labels.len()) {
            let base: BTreeSet<String> = labels
                .iter()
                .enumerate()
                .filter(|(index, _)| mask & (1 << index) != 0)
                .map(|(_, label)| (*label).to_string())
                .collect();
            for operation in [
                LabelOperation::Archive,
                LabelOperation::Restore,
                LabelOperation::LetGo,
            ] {
                let states = ["a1", "a2", "a3"].map(|id| MessageLabelState {
                    message_id: id.to_string(),
                    label_ids: base.clone(),
                });
                let diff =
                    LabelDiff::for_messages(operation, &receipt, &states)
                        .unwrap();
                let changed: BTreeSet<&str> = diff
                    .add_labels()
                    .iter()
                    .chain(diff.remove_labels())
                    .map(String::as_str)
                    .collect();
                let allowed: BTreeSet<&str> =
                    ["INBOX", receipt.id()].into_iter().collect();
                assert!(
                    changed.is_subset(&allowed),
                    "unexpected diff {diff:?}"
                );
                assert!(
                    diff.add_labels().len() <= 1
                        && diff.remove_labels().len() <= 1
                );
                for state in &states {
                    let mut after = state.label_ids.clone();
                    apply(&mut after, &diff);
                    for unrelated in [
                        "UNREAD", "STARRED", "Label_8", "Label_9", "Label_10",
                        "Label_11",
                    ] {
                        assert_eq!(
                            after.contains(unrelated),
                            state.label_ids.contains(unrelated)
                        );
                    }
                }
                let after_states: Vec<_> = states
                    .iter()
                    .map(|state| {
                        let mut label_ids = state.label_ids.clone();
                        apply(&mut label_ids, &diff);
                        MessageLabelState {
                            message_id: state.message_id.clone(),
                            label_ids,
                        }
                    })
                    .collect();
                let repeated =
                    LabelDiff::for_messages(operation, &receipt, &after_states)
                        .unwrap();
                assert!(repeated.is_empty(), "not idempotent: {operation:?}");
                if operation == LabelOperation::LetGo {
                    for (before, after) in
                        states.iter().zip(after_states.iter())
                    {
                        assert_eq!(
                            before.label_ids.contains("INBOX"),
                            after.label_ids.contains("INBOX")
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn restoring_an_archived_message_reverses_only_the_owned_pair() {
        let receipt = receipt();
        let initial = BTreeSet::from([
            "INBOX".to_string(),
            "UNREAD".to_string(),
            "Label_8".to_string(),
        ]);
        let before = ["a1"].map(|id| MessageLabelState {
            message_id: id.to_string(),
            label_ids: initial.clone(),
        });
        let archive =
            LabelDiff::for_messages(LabelOperation::Archive, &receipt, &before)
                .unwrap();
        let mut archived_labels = initial.clone();
        apply(&mut archived_labels, &archive);
        let archived = ["a1"].map(|id| MessageLabelState {
            message_id: id.to_string(),
            label_ids: archived_labels.clone(),
        });
        let restore = LabelDiff::for_messages(
            LabelOperation::Restore,
            &receipt,
            &archived,
        )
        .unwrap();
        apply(&mut archived_labels, &restore);
        assert_eq!(archived_labels, initial);
    }

    #[test]
    fn label_diff_requires_exact_message_state_membership() {
        let receipt = receipt();
        assert!(LabelDiff::for_messages(
            LabelOperation::Archive,
            &receipt,
            &[message("a1", &["INBOX"]), message("a1", &["INBOX"])],
        )
        .is_err());
    }

    #[test]
    fn label_diff_boundary_memberships_preserve_inbox_and_unrelated_labels() {
        let receipt = receipt();
        let initial = vec![
            message("a1", &["INBOX", "Label_1", "Label_42", "odd,\\literal"]),
            message("b2", &["INBOX", "Label_1", "odd,\\literal"]),
            message("c3", &["Label_1", "Label_42", "odd,\\literal"]),
            message("d4", &["Label_1", "odd,\\literal"]),
        ];
        for operation in [
            LabelOperation::Archive,
            LabelOperation::Restore,
            LabelOperation::LetGo,
        ] {
            let mut after = initial.clone();
            let diff = LabelDiff::for_messages(operation, &receipt, &after)
                .expect("valid boundary label sets");
            apply_all(&mut after, &diff);
            for (before, changed) in initial.iter().zip(&after) {
                if operation == LabelOperation::LetGo {
                    assert_eq!(
                        before.label_ids.contains("INBOX"),
                        changed.label_ids.contains("INBOX"),
                        "let-go preserves Inbox membership"
                    );
                }
                assert_eq!(
                    before.label_ids.contains("Label_1"),
                    changed.label_ids.contains("Label_1"),
                    "{operation:?} preserves enrollment"
                );
                assert_eq!(
                    before.label_ids.contains("odd,\\literal"),
                    changed.label_ids.contains("odd,\\literal"),
                    "{operation:?} preserves unrelated literal labels"
                );
            }
            assert!(LabelDiff::for_messages(operation, &receipt, &after)
                .expect("idempotent diff")
                .is_empty());
        }
    }

    proptest! {
        #[test]
        fn label_diff_properties_hold_for_independent_message_labels(
            cases in prop::collection::vec(
                (
                    "[0-9a-f]{1,16}",
                    any::<bool>(),
                    any::<bool>(),
                    any::<bool>(),
                    prop::collection::btree_set(arbitrary_label_id(), 0..8),
                ),
                1..7,
            ),
            label_suffix in 2u16..10000,
            name_suffix in arbitrary_label_id(),
        ) {
            let ids: BTreeSet<_> = cases.iter().map(|(id, _, _, _, _)| id).collect();
            prop_assume!(ids.len() == cases.len());

            // Start outside the enrollment ID by construction. Alias
            // rejection stays covered separately above.
            let receipt = ReceiptLabel::new(
                &GmailLabel {
                    id: format!("Label_{label_suffix}"),
                    name: format!("receipt,{name_suffix}"),
                    kind: Some("user".to_string()),
                },
                &GmailLabel {
                    id: "Label_1".to_string(),
                    name: "enrolled".to_string(),
                    kind: Some("user".to_string()),
                },
            ).unwrap();
            let initial: Vec<_> = cases.iter().map(|(id, inbox, has_receipt, enrollment, labels)| {
                let mut label_ids = labels.clone();
                if *inbox {
                    label_ids.insert("INBOX".to_string());
                }
                if *has_receipt {
                    label_ids.insert(receipt.id().to_string());
                }
                if *enrollment {
                    label_ids.insert("Label_1".to_string());
                }
                MessageLabelState {
                    message_id: id.clone(),
                    label_ids,
                }
            }).collect();

            for operation in [LabelOperation::Archive, LabelOperation::Restore, LabelOperation::LetGo] {
                let diff = LabelDiff::for_messages(operation, &receipt, &initial).unwrap();
                let allowed = BTreeSet::from(["INBOX", receipt.id()]);
                let changed: BTreeSet<_> = diff.add_labels().iter()
                    .chain(diff.remove_labels())
                    .map(String::as_str)
                    .collect();
                prop_assert!(changed.is_subset(&allowed));
                prop_assert!(diff.add_labels().len() <= 1 && diff.remove_labels().len() <= 1);

                let mut after = initial.clone();
                apply_all(&mut after, &diff);
                for (before, after) in initial.iter().zip(&after) {
                    let actual: BTreeSet<_> = before.label_ids.symmetric_difference(&after.label_ids)
                        .map(String::as_str)
                        .collect();
                    prop_assert!(actual.is_subset(&allowed));
                }
                let repeated = LabelDiff::for_messages(operation, &receipt, &after).unwrap();
                prop_assert!(repeated.is_empty());

                if operation == LabelOperation::LetGo {
                    for (before, after) in initial.iter().zip(&after) {
                        prop_assert_eq!(before.label_ids.contains("INBOX"), after.label_ids.contains("INBOX"));
                    }
                }
            }

            // Every saved message in a real archive batch starts in INBOX
            // and has no receipt. Restore must reverse that exact operation.
            let mut archiveable = initial.clone();
            for state in &mut archiveable {
                state.label_ids.remove(receipt.id());
                state.label_ids.insert("INBOX".to_string());
            }
            let before = archiveable.clone();
            let archive = LabelDiff::for_messages(LabelOperation::Archive, &receipt, &archiveable).unwrap();
            apply_all(&mut archiveable, &archive);
            let restore = LabelDiff::for_messages(LabelOperation::Restore, &receipt, &archiveable).unwrap();
            apply_all(&mut archiveable, &restore);
            prop_assert_eq!(archiveable, before);
        }
    }
}
