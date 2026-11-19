//! One shared draft-eligibility decision for `brief` stop advice and
//! `draft` guards.
//!
//! The contract lives in `docs/periods.md`. Both commands consult this
//! module so the brief never invites a draft the guard would refuse.
//! `-F/--force` overrides these ownership and time guards only: never
//! invalid text, malformed YAML, missing targets, locking, stale-note
//! detection, or verification failures.

use crate::native::intentions::{
    fields::Fields,
    state::{summary_state, SummaryState},
};

/// The eligibility verdict for a day or period subject.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Eligibility {
    /// A first draft is allowed (nothing to protect, or reflection only,
    /// or a clean re-draft of matching content).
    Allow {
        /// Short advice for the brief header, e.g. `ready to draft`.
        advice: String,
        /// Whether this is a re-draft of already-drafted content.
        redraft: bool,
    },
    /// The subject is open/current or in the future: `refused_open`.
    RefusedOpen { advice: String },
    /// Protected content needs an explicit redo: `refused_state`.
    RefusedState { advice: String },
}

/// Decide eligibility from the typed legacy-aware fields.
///
/// `has_review_body` is whether a `## Review` body is present (days never
/// have one). `state` is the subject state: `closed`, `open`/`current`,
/// or `future`. `noun` names the subject for advice text (`day`,
/// `week`, `quarter`, `year`).
pub(crate) fn decide(
    fields: &Fields,
    has_review_body: bool,
    state: &str,
    noun: &str,
) -> Eligibility {
    if matches!(state, "open" | "current" | "future") {
        let advice = if matches!(state, "future") {
            format!("This {noun} has not started: do not draft it.")
        } else {
            format!(
                "This {noun} is open: draft only if Bryan asked for \"so far\"."
            )
        };
        return Eligibility::RefusedOpen { advice };
    }
    let summary_present = fields
        .summary
        .value
        .as_deref()
        .is_some_and(|value| !value.trim().is_empty());
    let summary_matches =
        !summary_present || summary_state(fields) == SummaryState::Drafted;
    let review_present =
        has_review_body && !fields.review_body.trim().is_empty();
    let review_matches = !review_present
        || fields.review_digest.value.as_deref()
            == Some(&crate::native::intentions::digest(&fields.review_body));
    let reflected = fields
        .reflection
        .value
        .as_deref()
        .is_some_and(|value| !value.trim().is_empty());
    if !summary_matches || !review_matches {
        return Eligibility::RefusedState {
            advice: format!(
                "This {noun} is closed but its {} is already Bryan's: draft only if Bryan said \"redo\".",
                if !summary_matches && !review_matches {
                    "summary and Review"
                } else if !summary_matches {
                    "summary"
                } else {
                    "Review"
                },
            ),
        };
    }
    if reflected && (summary_present || review_present) {
        return Eligibility::RefusedState {
            advice: format!(
                "This {noun} is closed and reflected with an existing summary or Review: draft only if Bryan said \"redo\".",
            ),
        };
    }
    if !summary_present && !review_present {
        let advice = if reflected {
            format!(
                "This {noun} is closed with a reflection and no agent content: ready for a first draft (the reflection is preserved)."
            )
        } else {
            format!("This {noun} is closed: ready to draft.")
        };
        return Eligibility::Allow {
            advice,
            redraft: false,
        };
    }
    Eligibility::Allow {
        advice: format!(
            "This {noun} is closed with matching drafts: ready to re-draft."
        ),
        redraft: true,
    }
}
