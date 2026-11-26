//! Plain-link/dependency ties (`docs/task-relations.md` §22).
//!
//! One byte-exact plain-entry drop shared by every Rust writer (reconcile
//! fold, capture absorption, merge pruning), pinned by the PT drop vectors
//! that the nav planner twin (`dropPlainLinkEntries`) mirrors byte for
//! byte. Each writer answers the pair question with its own engine's
//! dependency graph over the existing identity keys: reconcile's
//! `dependency_direction` and capture's `dependency_joins_pair`. The
//! test-only `PairTies` classification pins the PT pair vectors that the
//! JS `classifyPairTies` twin also runs.

use super::parse::{accepted_links_spans, links_label_spans, parse_links_line};
use super::{LinksLine, RelationEntry};

/// One endpoint in the additive reconcile/merge plain-link absorption report.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub(crate) struct TaskRelationEndpoint {
    pub(crate) path: String,
    pub(crate) line: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) block_id: Option<String>,
    pub(crate) text: String,
}

/// One plain LINKS entry removed because a dependency joins the same pair.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub(crate) struct AbsorbedPlainLink {
    pub(crate) path: String,
    pub(crate) line: usize,
    pub(crate) source: TaskRelationEndpoint,
    pub(crate) target: TaskRelationEndpoint,
    pub(crate) dependency: String,
}

/// What [`drop_plain_link_entries`] decided about one LINKS line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum DropOutcome {
    /// No tied plain entry; the line is byte-identical.
    Unchanged,
    /// At least one tied plain entry dropped; some entries remain.
    Rewritten {
        /// Canonical texts of the dropped entries, in line order.
        removed: Vec<String>,
        /// The new line bytes.
        line: String,
    },
    /// Every entry was a tied plain entry; the caller deletes the line.
    DeleteLine {
        /// Canonical texts of the dropped entries, in line order.
        removed: Vec<String>,
    },
    /// The line is malformed or is not a LINKS line; write nothing.
    Malformed,
}

/// Drop every plain (unlabelled) LINKS entry whose target the caller
/// reports as dependency-tied.
///
/// `is_tied` answers one question per entry: is this pair joined by a
/// dependency in either direction? Callers resolve
/// `(entry.target, entry.block_id)` to a vault path plus block id
/// first (PT10) and answer with their engine's dependency-joins
/// predicate; labelled entries (including `[[#^a|alias]]` per LK5)
/// never drop, whatever the predicate says.
///
/// Separator rule (PT2–PT4, PT9): a dropped non-first entry takes its
/// preceding separator; a dropped first entry takes its following
/// separator but keeps the next kept entry's gap label (`see ` in
/// PT2). All other bytes — indent, header, CRLF, trailing spaces —
/// are preserved (PT12–PT12c).
pub(crate) fn drop_plain_link_entries(
    line: &str,
    is_tied: impl Fn(&RelationEntry) -> bool,
) -> DropOutcome {
    let entries = match parse_links_line(line) {
        LinksLine::Accepted { entries, .. } => entries,
        LinksLine::Empty => return DropOutcome::Unchanged,
        LinksLine::Malformed | LinksLine::NotALine => {
            return DropOutcome::Malformed;
        }
    };
    let Some(spans) = accepted_links_spans(line) else {
        return DropOutcome::Malformed;
    };
    if entries.len() != spans.len() {
        return DropOutcome::Malformed;
    }
    let tied_plain: Vec<bool> = entries
        .iter()
        .map(|entry| entry.label.is_none() && is_tied(entry))
        .collect();
    if !tied_plain.iter().any(|tied| *tied) {
        return DropOutcome::Unchanged;
    }
    let removed: Vec<String> = entries
        .iter()
        .zip(&tied_plain)
        .filter(|pair| *pair.1)
        .map(|(entry, _)| entry.canonical_text())
        .collect();
    if tied_plain.iter().all(|tied| *tied) {
        return DropOutcome::DeleteLine { removed };
    }
    let labels = links_label_spans(line).unwrap_or_default();
    let mut cuts: Vec<(usize, usize)> = Vec::new();
    let mut index = 0;
    while index < entries.len() {
        if !tied_plain[index] {
            index += 1;
            continue;
        }
        let mut end = index;
        while end + 1 < entries.len() && tied_plain[end + 1] {
            end += 1;
        }
        if index == 0 {
            let next = end + 1;
            let gap_start = spans[end].full_end;
            let gap_end = spans[next].full_start;
            let cut_end = labels
                .iter()
                .filter(|label| {
                    label.range.start >= gap_start
                        && label.range.start < gap_end
                })
                .map(|label| label.range.start)
                .min()
                .unwrap_or(gap_end);
            cuts.push((spans[index].full_start, cut_end));
        } else {
            let previous = index - 1;
            cuts.push((spans[previous].full_end, spans[end].full_end));
        }
        index = end + 1;
    }
    cuts.sort();
    let mut out = String::with_capacity(line.len());
    let mut cursor = 0;
    for (start, end) in cuts {
        out.push_str(&line[cursor..start]);
        cursor = end;
    }
    out.push_str(&line[cursor..]);
    DropOutcome::Rewritten { removed, line: out }
}

/// The exclusive tie between two tasks, from the owner's side.
///
/// Test-only: it pins the PT13–PT21 pair vectors shared with the JS
/// `classifyPairTies` twin; Rust writers decide drops per entry.
#[cfg(test)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TieKind {
    /// No plain link and no dependency either way (PT13).
    Untied,
    /// Plain owner→other only, no dependency (PT14).
    PlainOut,
    /// Plain other→owner only, no dependency (PT15).
    PlainIn,
    /// Plain links both ways, no dependency (mutual plain; deferred).
    PlainBoth,
    /// Owner waits on other (PT16); absorbs plains both ways.
    DependsOut,
    /// Other waits on owner (PT17); absorbs plains both ways.
    DependsIn,
    /// Dependencies both ways; absorbs plains both ways.
    DependsBoth,
}

#[cfg(test)]
impl TieKind {
    /// Fixture and docs spelling (`docs/task-relations.md` §22).
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Untied => "untied",
            Self::PlainOut => "plain_out",
            Self::PlainIn => "plain_in",
            Self::PlainBoth => "plain_both",
            Self::DependsOut => "depends_out",
            Self::DependsIn => "depends_in",
            Self::DependsBoth => "depends_both",
        }
    }
}

/// One pair's tie plus which plain sides its dependency absorbs.
#[cfg(test)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct PairTies {
    /// The pair's exclusive tie (dependency wins over plain).
    pub(crate) tie: TieKind,
    /// Drop the owner's plain entry for the other (PT18, PT21).
    pub(crate) drop_owner_plain: bool,
    /// Drop the other's plain entry for the owner (PT19).
    pub(crate) drop_other_plain: bool,
}

#[cfg(test)]
impl PairTies {
    /// Classify one pair from already-resolved edge sets.
    ///
    /// Labelled relations never enter the verdict (PT20 stays legal);
    /// callers pass only plain-link presence per side. A dependency in
    /// either direction absorbs plain links in both directions.
    pub(crate) fn classify(
        owner_waits_on_other: bool,
        other_waits_on_owner: bool,
        plain_out: bool,
        plain_in: bool,
    ) -> Self {
        let dependency = owner_waits_on_other || other_waits_on_owner;
        let tie = if owner_waits_on_other && other_waits_on_owner {
            TieKind::DependsBoth
        } else if owner_waits_on_other {
            TieKind::DependsOut
        } else if other_waits_on_owner {
            TieKind::DependsIn
        } else if plain_out && plain_in {
            TieKind::PlainBoth
        } else if plain_out {
            TieKind::PlainOut
        } else if plain_in {
            TieKind::PlainIn
        } else {
            TieKind::Untied
        };
        Self {
            tie,
            drop_owner_plain: dependency && plain_out,
            drop_other_plain: dependency && plain_in,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::*;

    static TIE_VECTORS: std::sync::LazyLock<serde_json::Value> =
        std::sync::LazyLock::new(|| {
            serde_json::from_str(include_str!(
                "../../../tests/fixtures/plain_link_ties/vectors.json"
            ))
            .expect("plain-link tie vectors parse")
        });

    fn tied_set(vector: &serde_json::Value) -> BTreeSet<(String, String)> {
        vector["tied"]
            .as_array()
            .expect("tied array")
            .iter()
            .map(|entry| {
                (
                    entry["target"].as_str().expect("target").to_string(),
                    entry["block_id"].as_str().expect("block_id").to_string(),
                )
            })
            .collect()
    }

    #[test]
    fn pt_drop_vectors_are_byte_exact() {
        for vector in TIE_VECTORS["drop"].as_array().expect("drop array") {
            let id = vector["id"].as_str().expect("id");
            let line_in = vector["line_in"].as_str().expect("line_in");
            let tied = tied_set(vector);
            let outcome = drop_plain_link_entries(line_in, |entry| {
                tied.contains(&(entry.target.clone(), entry.block_id.clone()))
            });
            let expected_outcome = vector["outcome"].as_str().expect("outcome");
            let expected_removed: Vec<String> = vector["removed"]
                .as_array()
                .expect("removed array")
                .iter()
                .map(|entry| entry.as_str().expect("removed entry").to_string())
                .collect();
            match expected_outcome {
                "unchanged" => {
                    assert_eq!(outcome, DropOutcome::Unchanged, "{id}");
                }
                "malformed" => {
                    assert_eq!(outcome, DropOutcome::Malformed, "{id}");
                }
                "delete_line" => {
                    let DropOutcome::DeleteLine { removed } = outcome else {
                        panic!("{id}: expected DeleteLine, got {outcome:?}");
                    };
                    assert_eq!(removed, expected_removed, "{id}");
                }
                "rewritten" => {
                    let DropOutcome::Rewritten { removed, line } = outcome
                    else {
                        panic!("{id}: expected Rewritten, got {outcome:?}");
                    };
                    assert_eq!(removed, expected_removed, "{id}");
                    assert_eq!(
                        line,
                        vector["line_out"].as_str().expect("line_out"),
                        "{id}"
                    );
                }
                other => panic!("{id}: unknown outcome {other}"),
            }
        }
    }

    #[test]
    fn pt_pair_vectors_classify() {
        for vector in TIE_VECTORS["pair"].as_array().expect("pair array") {
            let id = vector["id"].as_str().expect("id");
            let actual = PairTies::classify(
                vector["owner_waits"].as_bool().expect("owner_waits"),
                vector["other_waits"].as_bool().expect("other_waits"),
                vector["plain_out"].as_bool().expect("plain_out"),
                vector["plain_in"].as_bool().expect("plain_in"),
            );
            assert_eq!(
                actual.tie.as_str(),
                vector["tie"].as_str().expect("tie"),
                "{id}"
            );
            assert_eq!(
                actual.drop_owner_plain,
                vector["drop_owner_plain"]
                    .as_bool()
                    .expect("drop_owner_plain"),
                "{id}"
            );
            assert_eq!(
                actual.drop_other_plain,
                vector["drop_other_plain"]
                    .as_bool()
                    .expect("drop_other_plain"),
                "{id}"
            );
        }
    }
}
