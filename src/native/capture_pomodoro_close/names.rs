//! R6 close names: narrow the closed session, shed the carried one.
//!
//! The pure planners live in `pomodoro_themes::merged`; this module feeds
//! them from a close's numbered Task Links. Each link carries its
//! _pre-close_ effective type key (read before the close stages anything,
//! so closed tasks do not stop suggesting) and its outcome.

use std::collections::{BTreeMap, BTreeSet};

use crate::native::{
    config::themes::ThemeSources,
    pomodoro_themes::{merged, routing},
};

use super::selection::{NumberedTaskLink, TaskLinkOutcome};

/// Theme vocabulary plus every Task Link's pre-close type, as resolved by
/// the capture layer. A close planned without naming inputs never renames.
#[derive(Debug, Clone)]
pub(crate) struct CloseNaming {
    pub sources: ThemeSources,
    /// `(path_part, block_id)` of a Task Link to its pre-close effective
    /// type key. Links without an effective type are absent.
    pub link_types: BTreeMap<(String, String), String>,
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
/// and a name the planners leave alone (single-type, MEET, unnamed)
/// yields [`CloseNames::unchanged`].
pub(crate) fn close_names(
    name: &str,
    links: &[NumberedTaskLink],
    naming: &CloseNaming,
) -> CloseNames {
    let typed: Vec<(&str, TaskLinkOutcome)> = links
        .iter()
        .filter_map(|link| {
            let key = naming
                .link_types
                .get(&(link.path_part.clone(), link.block_id.clone()))?;
            Some((key.as_str(), link.outcome))
        })
        .collect();
    let before: Vec<&str> = typed.iter().map(|(key, _)| *key).collect();
    let worked: Vec<&str> = typed
        .iter()
        .filter(|(_, outcome)| {
            matches!(
                outcome,
                TaskLinkOutcome::InProgress
                    | TaskLinkOutcome::Parked
                    | TaskLinkOutcome::Complete
            )
        })
        .map(|(key, _)| *key)
        .collect();
    let carried: Vec<&str> = typed
        .iter()
        .filter(|(_, outcome)| {
            matches!(
                outcome,
                TaskLinkOutcome::InProgress | TaskLinkOutcome::Deferred
            )
        })
        .map(|(key, _)| *key)
        .collect();
    let closed = merged::narrow_closed(name, &worked, &naming.sources);
    let continuation = merged::shed(name, &before, &carried, &naming.sources);
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

    fn naming(types: &[(&str, &str)]) -> CloseNaming {
        CloseNaming {
            sources: sources(),
            link_types: types
                .iter()
                .map(|(block, key)| {
                    (
                        ("n".to_string(), (*block).to_string()),
                        (*key).to_string(),
                    )
                })
                .collect(),
        }
    }

    #[test]
    fn only_fix_worked_narrows_and_the_last_fix_done_sheds() {
        let names = close_names(
            "BUILD + FIX",
            &[
                link(1, "b", TaskLinkOutcome::Deferred),
                link(2, "f", TaskLinkOutcome::Complete),
            ],
            &naming(&[("b", "build"), ("f", "fix")]),
        );
        assert_eq!(names.closed, "FIX");
        assert_eq!(names.continuation, "BUILD");
        assert_eq!(names.shed, vec![("fix".to_string(), "FIX".to_string())]);
    }

    #[test]
    fn parked_and_dropped_links_are_not_carried() {
        let names = close_names(
            "BUILD + FIX + READ",
            &[
                link(1, "b", TaskLinkOutcome::InProgress),
                link(2, "f", TaskLinkOutcome::Parked),
                link(3, "r", TaskLinkOutcome::Dropped),
            ],
            &naming(&[("b", "build"), ("f", "fix"), ("r", "read")]),
        );
        assert_eq!(names.closed, "BUILD + FIX");
        assert_eq!(names.continuation, "BUILD");
        assert_eq!(
            names.shed,
            vec![
                ("fix".to_string(), "FIX".to_string()),
                ("read".to_string(), "READ".to_string()),
            ]
        );
    }

    #[test]
    fn labels_stay_and_untyped_or_drifted_work_leaves_the_name() {
        let names = close_names(
            "BOB + BUILD + FIX",
            &[
                link(1, "b", TaskLinkOutcome::InProgress),
                link(2, "u", TaskLinkOutcome::Complete),
            ],
            &naming(&[("b", "read")]),
        );
        // Only drift (READ) and an untyped link were worked: unchanged.
        assert_eq!(names.closed, "BOB + BUILD + FIX");
        // READ is not a component, so nothing sheds; BUILD and FIX never
        // had links and stay.
        assert_eq!(names.continuation, "BOB + BUILD + FIX");
        assert!(names.shed.is_empty());
    }

    #[test]
    fn shedding_every_type_keeps_the_base() {
        let names = close_names(
            "BOB + BUILD + FIX",
            &[
                link(1, "b", TaskLinkOutcome::Complete),
                link(2, "f", TaskLinkOutcome::Complete),
            ],
            &naming(&[("b", "build"), ("f", "fix")]),
        );
        assert_eq!(names.closed, "BOB + BUILD + FIX");
        assert_eq!(names.continuation, "BOB + BUILD");
        assert_eq!(names.shed, vec![("fix".to_string(), "FIX".to_string())]);
    }

    #[test]
    fn single_type_meet_and_unnamed_never_rename() {
        let one = close_names(
            "FIX",
            &[link(1, "f", TaskLinkOutcome::Complete)],
            &naming(&[("f", "fix")]),
        );
        assert_eq!(one, CloseNames::unchanged("FIX"));
        let meet = close_names(
            "MEET",
            &[link(1, "f", TaskLinkOutcome::Complete)],
            &naming(&[("f", "fix")]),
        );
        assert_eq!(meet, CloseNames::unchanged("MEET"));
        let none = close_names("BUILD + FIX", &[], &naming(&[]));
        assert_eq!(none, CloseNames::unchanged("BUILD + FIX"));
    }
}
