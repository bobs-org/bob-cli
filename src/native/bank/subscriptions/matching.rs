//! Deterministic matching for validated subscription needles.

use super::registry::{SubscriptionRecord, SubscriptionStatus};

/// Pattern-attribution result. Paths are stable, vault-relative record
/// identities, and an exact cross-record tie deliberately has no winner.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SubscriptionMatch {
    pub(crate) record_path: Option<String>,
    pub(crate) needle: Option<String>,
    pub(crate) tied_paths: Vec<String>,
}

impl SubscriptionMatch {
    fn none() -> Self {
        Self {
            record_path: None,
            needle: None,
            tied_paths: Vec::new(),
        }
    }
}

/// Resolve one description/payee against valid needles. A record contributes
/// at most once, even when several of its needles match.
pub(crate) fn resolve_match(
    records: &[SubscriptionRecord],
    text: &str,
) -> SubscriptionMatch {
    let mut matched = records
        .iter()
        .filter_map(|record| {
            let mut needles = record
                .needles
                .iter()
                .filter(|needle| {
                    crate::native::bank::naming::contains_folded(text, needle)
                })
                .collect::<Vec<_>>();
            if needles.is_empty() {
                return None;
            }
            needles.sort_by(|left, right| {
                right
                    .chars()
                    .count()
                    .cmp(&left.chars().count())
                    .then_with(|| {
                        left.to_lowercase().cmp(&right.to_lowercase())
                    })
                    .then_with(|| left.cmp(right))
            });
            Some((record, needles[0].clone()))
        })
        .collect::<Vec<_>>();
    if matched.is_empty() {
        return SubscriptionMatch::none();
    }

    if matched
        .iter()
        .any(|(record, _)| record.status != SubscriptionStatus::Cancelled)
    {
        matched.retain(|(record, _)| {
            record.status != SubscriptionStatus::Cancelled
        });
    }

    let longest = matched
        .iter()
        .map(|(_, needle)| needle.chars().count())
        .max()
        .unwrap_or_default();
    let mut finalists = matched
        .into_iter()
        .filter(|(_, needle)| needle.chars().count() == longest)
        .collect::<Vec<_>>();
    finalists.sort_by(|(left, _), (right, _)| left.path.cmp(&right.path));

    if finalists.len() == 1 {
        let (record, needle) = finalists.pop().expect("one finalist");
        return SubscriptionMatch {
            record_path: Some(record.path.clone()),
            needle: Some(needle.clone()),
            tied_paths: Vec::new(),
        };
    }

    let tied_paths = finalists
        .into_iter()
        .map(|(record, _)| record.path.clone())
        .collect();
    SubscriptionMatch {
        record_path: None,
        needle: None,
        tied_paths,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::native::bank::subscriptions::registry::{
        SubscriptionCadence, SubscriptionPricing,
    };

    fn record(
        path: &str,
        status: SubscriptionStatus,
        needles: &[&str],
    ) -> SubscriptionRecord {
        SubscriptionRecord {
            stem: path.rsplit('/').next().unwrap().to_owned(),
            path: path.to_owned(),
            link_target: path.trim_end_matches(".md").to_owned(),
            title: path.to_owned(),
            status,
            status_label: status.as_str().to_owned(),
            amount_cents: 999,
            cadence: SubscriptionCadence::Monthly,
            pricing: SubscriptionPricing::Fixed,
            needles: needles
                .iter()
                .map(|needle| (*needle).to_owned())
                .collect(),
            invalid_needles: Vec::new(),
            category: None,
            url: None,
            renews: None,
            ended: None,
        }
    }

    #[test]
    fn folded_matching_and_descriptor_drift_needles() {
        let record = record(
            "billing/tunely.md",
            SubscriptionStatus::Active,
            &["TUNELY", "TUNELY PREMIUM"],
        );
        let result =
            resolve_match(&[record], "Recurring Payment tunely premium");
        assert_eq!(result.record_path.as_deref(), Some("billing/tunely.md"));
        assert_eq!(result.needle.as_deref(), Some("TUNELY PREMIUM"));
    }

    #[test]
    fn live_matches_suppress_cancelled_matches() {
        let records = [
            record(
                "billing/old.md",
                SubscriptionStatus::Cancelled,
                &["CLOUDBOX PREMIUM EXTRA"],
            ),
            record(
                "billing/current.md",
                SubscriptionStatus::Active,
                &["CLOUDBOX"],
            ),
        ];
        let result = resolve_match(&records, "Cloudbox Premium Extra");
        assert_eq!(result.record_path.as_deref(), Some("billing/current.md"));
        assert_eq!(result.needle.as_deref(), Some("CLOUDBOX"));
    }

    #[test]
    fn cancelled_only_match_remains_attributable() {
        let records = [record(
            "billing/old.md",
            SubscriptionStatus::Cancelled,
            &["CLOUDBOX"],
        )];
        let result = resolve_match(&records, "Cloudbox charge");
        assert_eq!(result.record_path.as_deref(), Some("billing/old.md"));
    }

    #[test]
    fn longest_needle_wins_and_one_record_counts_once() {
        let records = [
            record(
                "billing/short.md",
                SubscriptionStatus::Active,
                &["CLOUD", "CLOUDBOX"],
            ),
            record(
                "billing/long.md",
                SubscriptionStatus::Active,
                &["CLOUDBOX PREMIUM"],
            ),
        ];
        let result = resolve_match(&records, "Cloudbox Premium monthly");
        assert_eq!(result.record_path.as_deref(), Some("billing/long.md"));
        assert_eq!(result.needle.as_deref(), Some("CLOUDBOX PREMIUM"));
    }

    #[test]
    fn exact_cross_record_tie_is_unattributed_and_order_independent() {
        let left = record(
            "billing/alpha.md",
            SubscriptionStatus::Active,
            &["CLOUDBOX PLAN"],
        );
        let right = record(
            "billing/zeta.md",
            SubscriptionStatus::ToDecide,
            &["CLOUDBOX PLAN"],
        );
        let first =
            resolve_match(&[right.clone(), left.clone()], "Cloudbox Plan");
        let second = resolve_match(&[left, right], "Cloudbox Plan");
        assert_eq!(first, second);
        assert_eq!(first.record_path, None);
        assert_eq!(first.tied_paths, ["billing/alpha.md", "billing/zeta.md"]);
    }
}
