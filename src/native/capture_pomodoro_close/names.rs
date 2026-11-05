//! R6 close names: settle the closed session, agree the continuation.
//!
//! The pure planners live in `pomodoro_themes::agree` (plus
//! `merged::shed` for the continuation base); this module feeds them from
//! a close's numbered Task Links. Each link carries its _pre-close_
//! effective type key and catalog spelling (read before the close stages
//! anything, so closed tasks do not stop suggesting) and its outcome.

use std::collections::{BTreeMap, BTreeSet};

use crate::native::{
    config::themes::ThemeSources,
    pomodoro_themes::{agree, merged, routing},
};

use super::selection::{NumberedTaskLink, TaskLinkOutcome};

/// Theme vocabulary plus every Task Link's pre-close type, as resolved by
/// the capture layer. A close planned without naming inputs never renames.
#[derive(Debug, Clone)]
pub(crate) struct CloseNaming {
    pub sources: ThemeSources,
    /// `(path_part, block_id)` of a Task Link to its pre-close effective
    /// type `(key, catalog-spelled name)`. Links without an effective type
    /// are absent.
    pub link_types: BTreeMap<(String, String), (String, String)>,
}

/// The two names a close derives from the running entry's name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CloseNames {
    /// The completed session's name (narrowed).
    pub closed: String,
    /// The continuation's name (shed). Used whether the close creates the
    /// continuation or successor linking later reuses or creates one.
    pub continuation: String,
    /// Type components the continuation drops, as `(key, spelling)`, in
    /// the running entry's name order.
    pub shed: Vec<(String, String)>,
}

impl CloseNames {
    /// Names that change nothing: the running entry's own name.
    pub(crate) fn unchanged(name: &str) -> Self {
        Self {
            closed: name.to_string(),
            continuation: name.to_string(),
            shed: Vec::new(),
        }
    }
}

/// Apply R6 to `name`. Worked = in progress, parked, or complete.
/// Carried = in progress or deferred. Links without a type are ignored,
/// and a name the planners leave alone (MEET, container, untyped work)
/// yields [`CloseNames::unchanged`].
pub(crate) fn close_names(
    name: &str,
    links: &[NumberedTaskLink],
    naming: &CloseNaming,
) -> CloseNames {
    let typed: Vec<((&str, &str), TaskLinkOutcome)> = links
        .iter()
        .filter_map(|link| {
            let (key, display) = naming
                .link_types
                .get(&(link.path_part.clone(), link.block_id.clone()))?;
            Some(((key.as_str(), display.as_str()), link.outcome))
        })
        .collect();
    let is_worked = |outcome: &TaskLinkOutcome| {
        matches!(
            outcome,
            TaskLinkOutcome::InProgress
                | TaskLinkOutcome::Parked
                | TaskLinkOutcome::Complete
        )
    };
    let is_carried = |outcome: &TaskLinkOutcome| {
        matches!(
            outcome,
            TaskLinkOutcome::InProgress | TaskLinkOutcome::Deferred
        )
    };
    let worked: Vec<agree::WorkedType> = typed
        .iter()
        .filter(|(_, outcome)| is_worked(outcome))
        .map(|((key, display), _)| agree::WorkedType {
            key: (*key).to_string(),
            name: (*display).to_string(),
        })
        .collect();
    let before: Vec<&str> = typed.iter().map(|((key, _), _)| *key).collect();
    let carried: Vec<&str> = typed
        .iter()
        .filter(|(_, outcome)| is_carried(outcome))
        .map(|((key, _), _)| *key)
        .collect();
    let closed = agree::settle_closed(name, &worked, &naming.sources);
    // The continuation sheds, then agrees over its carried children, so it
    // is a fixed point of `agree_entry` the way reconcile would leave it.
    // Untyped carried links are Unknown, which protects every type.
    let shed_base = merged::shed(name, &before, &carried, &naming.sources);
    let children: Vec<agree::AgreeChild> = links
        .iter()
        .filter(|link| is_carried(&link.outcome))
        .map(|link| {
            naming
                .link_types
                .get(&(link.path_part.clone(), link.block_id.clone()))
                .map(|(key, display)| agree::AgreeChild::Live {
                    key: key.clone(),
                    name: display.clone(),
                })
                .unwrap_or(agree::AgreeChild::Unknown)
        })
        .collect();
    let continuation =
        agree::agree_entry(Some(&shed_base), true, &children, &naming.sources)
            .name;
    let kept: BTreeSet<String> =
        routing::type_components(Some(continuation.as_str()), &naming.sources)
            .into_iter()
            .map(|item| item.key)
            .collect();
    let shed = routing::type_components(Some(name), &naming.sources)
        .into_iter()
        .filter(|item| item.is_type && !kept.contains(&item.key))
        .map(|item| (item.key, item.name))
        .collect();
    CloseNames {
        closed,
        continuation,
        shed,
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::*;
    use crate::native::{
        capture_pomodoro_close::selection::{TaskLinkMarker, TaskLinkSource},
        config::themes::{ConfiguredTheme, ThemesConfig},
    };

    fn sources() -> ThemeSources {
        ThemeSources {
            exempt: BTreeSet::new(),
            inventory: ["bob".to_string()].into_iter().collect(),
            themes: Some(ThemesConfig {
                entries: ["BUILD", "FIX", "READ", "MEET"]
                    .iter()
                    .map(|name| ConfiguredTheme {
                        key: name.to_lowercase(),
                        name: (*name).to_string(),
                        color: None,
                    })
                    .collect(),
            }),
            warning: None,
        }
    }

    fn link(
        index: u32,
        block_id: &str,
        outcome: TaskLinkOutcome,
    ) -> NumberedTaskLink {
        NumberedTaskLink {
            index,
            line: index as usize + 1,
            block_link: format!("[[n#^{block_id}]]"),
            path_part: "n".to_string(),
            block_id: block_id.to_string(),
            marker: TaskLinkMarker::Plain,
            outcome,
            source: TaskLinkSource::Ledger,
            clock: None,
        }
    }

    fn naming(types: &[(&str, &str, &str)]) -> CloseNaming {
        CloseNaming {
            sources: sources(),
            link_types: types
                .iter()
                .map(|(block, key, name)| {
                    (
                        ("n".to_string(), (*block).to_string()),
                        ((*key).to_string(), (*name).to_string()),
                    )
                })
                .collect(),
        }
    }

    /// The continuation's carried children as `agree_entry` sees them.
    fn carried_children(
        links: &[NumberedTaskLink],
        naming: &CloseNaming,
    ) -> Vec<agree::AgreeChild> {
        links
            .iter()
            .filter(|link| {
                matches!(
                    link.outcome,
                    TaskLinkOutcome::InProgress | TaskLinkOutcome::Deferred
                )
            })
            .map(|link| {
                naming
                    .link_types
                    .get(&(link.path_part.clone(), link.block_id.clone()))
                    .map(|(key, display)| agree::AgreeChild::Live {
                        key: key.clone(),
                        name: display.clone(),
                    })
                    .unwrap_or(agree::AgreeChild::Unknown)
            })
            .collect()
    }

    fn assert_continuation_is_stable(
        continuation: &str,
        links: &[NumberedTaskLink],
        naming: &CloseNaming,
    ) {
        let children = carried_children(links, naming);
        let outcome = agree::agree_entry(
            Some(continuation),
            true,
            &children,
            &naming.sources,
        );
        assert_eq!(outcome.name, continuation);
        assert!(outcome.added.is_empty());
        assert!(outcome.removed.is_empty());
    }

    #[test]
    fn only_fix_worked_narrows_and_the_last_fix_done_sheds() {
        let links = [
            link(1, "b", TaskLinkOutcome::Deferred),
            link(2, "f", TaskLinkOutcome::Complete),
        ];
        let naming = naming(&[("b", "build", "BUILD"), ("f", "fix", "FIX")]);
        let names = close_names("BUILD + FIX", &links, &naming);
        assert_eq!(names.closed, "FIX");
        assert_eq!(names.continuation, "BUILD");
        assert_eq!(names.shed, vec![("fix".to_string(), "FIX".to_string())]);
        assert_continuation_is_stable(&names.continuation, &links, &naming);
    }

    #[test]
    fn parked_and_dropped_links_are_not_carried() {
        let links = [
            link(1, "b", TaskLinkOutcome::InProgress),
            link(2, "f", TaskLinkOutcome::Parked),
            link(3, "r", TaskLinkOutcome::Dropped),
        ];
        let naming = naming(&[
            ("b", "build", "BUILD"),
            ("f", "fix", "FIX"),
            ("r", "read", "READ"),
        ]);
        let names = close_names("BUILD + FIX + READ", &links, &naming);
        assert_eq!(names.closed, "BUILD + FIX");
        assert_eq!(names.continuation, "BUILD");
        assert_eq!(
            names.shed,
            vec![
                ("fix".to_string(), "FIX".to_string()),
                ("read".to_string(), "READ".to_string()),
            ]
        );
        assert_continuation_is_stable(&names.continuation, &links, &naming);
    }

    #[test]
    fn untyped_worked_links_keep_the_name() {
        let links = [link(1, "u", TaskLinkOutcome::Complete)];
        let naming = naming(&[]);
        let names = close_names("BOB + BUILD + FIX", &links, &naming);
        // No typed worked link: the closed session keeps its history.
        assert_eq!(names.closed, "BOB + BUILD + FIX");
        assert_eq!(names.continuation, "BOB + BUILD + FIX");
        assert!(names.shed.is_empty());
        assert_continuation_is_stable(&names.continuation, &links, &naming);
    }

    #[test]
    fn drifted_work_settles_both_sessions() {
        let links = [
            link(1, "b", TaskLinkOutcome::InProgress),
            link(2, "u", TaskLinkOutcome::Complete),
        ];
        let naming = naming(&[("b", "read", "READ")]);
        let names = close_names("BOB + BUILD + FIX", &links, &naming);
        // The worked type is drift (READ is no component): the closed
        // session records exactly the worked types, and the continuation
        // agrees the same way reconcile would.
        assert_eq!(names.closed, "BOB + READ");
        assert_eq!(names.continuation, "BOB + READ");
        assert_eq!(
            names.shed,
            vec![
                ("build".to_string(), "BUILD".to_string()),
                ("fix".to_string(), "FIX".to_string()),
            ]
        );
        assert_continuation_is_stable(&names.continuation, &links, &naming);
    }

    #[test]
    fn single_type_close_settles_to_the_worked_type() {
        let links = [link(1, "f", TaskLinkOutcome::Complete)];
        let naming = naming(&[("f", "build", "BUILD")]);
        let names = close_names("FIX", &links, &naming);
        // A `— FIX` session that worked only BUILD closes as `— BUILD`.
        assert_eq!(names.closed, "BUILD");
        // Nothing carries, so the empty continuation keeps the base.
        assert_eq!(names.continuation, "FIX");
        assert!(names.shed.is_empty());
        assert_continuation_is_stable(&names.continuation, &links, &naming);
    }

    #[test]
    fn carried_drift_retypes_the_continuation() {
        let links = [link(1, "f", TaskLinkOutcome::Deferred)];
        let naming = naming(&[("f", "build", "BUILD")]);
        let names = close_names("FIX", &links, &naming);
        // Deferred links never work the closed session, but the carried
        // BUILD link still agrees the continuation.
        assert_eq!(names.closed, "FIX");
        assert_eq!(names.continuation, "BUILD");
        assert_eq!(names.shed, vec![("fix".to_string(), "FIX".to_string())]);
        assert_continuation_is_stable(&names.continuation, &links, &naming);
    }

    #[test]
    fn deferring_everything_keeps_both_names() {
        let links = [
            link(1, "b", TaskLinkOutcome::Deferred),
            link(2, "f", TaskLinkOutcome::Deferred),
        ];
        let naming = naming(&[("b", "build", "BUILD"), ("f", "fix", "FIX")]);
        let names = close_names("BUILD + FIX", &links, &naming);
        // The `x0` shape: nothing worked, everything carried.
        assert_eq!(names.closed, "BUILD + FIX");
        assert_eq!(names.continuation, "BUILD + FIX");
        assert!(names.shed.is_empty());
        assert_continuation_is_stable(&names.continuation, &links, &naming);
    }

    #[test]
    fn shedding_every_type_keeps_the_base() {
        let links = [
            link(1, "b", TaskLinkOutcome::Complete),
            link(2, "f", TaskLinkOutcome::Complete),
        ];
        let naming = naming(&[("b", "build", "BUILD"), ("f", "fix", "FIX")]);
        let names = close_names("BOB + BUILD + FIX", &links, &naming);
        assert_eq!(names.closed, "BOB + BUILD + FIX");
        assert_eq!(names.continuation, "BOB + BUILD");
        assert_eq!(names.shed, vec![("fix".to_string(), "FIX".to_string())]);
        assert_continuation_is_stable(&names.continuation, &links, &naming);
    }

    #[test]
    fn single_type_meet_and_unnamed_never_rename() {
        let links = [link(1, "f", TaskLinkOutcome::Complete)];
        let naming = naming(&[("f", "fix", "FIX")]);
        let one = close_names("FIX", &links, &naming);
        assert_eq!(one, CloseNames::unchanged("FIX"));
        let meet = close_names("MEET", &links, &naming);
        assert_eq!(meet, CloseNames::unchanged("MEET"));
        let none = close_names("BUILD + FIX", &[], &naming);
        assert_eq!(none, CloseNames::unchanged("BUILD + FIX"));
    }
}
