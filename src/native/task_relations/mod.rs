//! Task relation lines: the `docs/task-relations.md` contract.
//!
//! A task's outgoing relations live on one authored
//! `- 🪢 **LINKS:**` child; incoming relations are projected onto a
//! generated `- 🪢 **LINKED FROM:**` child. This module owns the
//! parsers, label rules, formatters, slot, and edit planners. Capture
//! and reconcile emit lines through these planners.

pub(crate) mod format;
pub(crate) mod inverse;
pub(crate) mod label;
pub(crate) mod mentions;
pub(crate) mod parse;
pub(crate) mod plan;

#[cfg(test)]
mod tests;

pub(crate) const KNOT_EMOJI: &str = "🪢";
pub(crate) const LINKS_LABEL: &str = "LINKS";
pub(crate) const LINKS_TO_LABEL: &str = "LINKS TO";
pub(crate) const LINKED_FROM_LABEL: &str = "LINKED FROM";
pub(crate) const BACKLINKS_LABEL: &str = "BACKLINKS";
pub(crate) const RELATION_SEPARATOR: &str = " • ";
pub(crate) const MENTION_LABEL: &str = "💬 mentions";

const TOLERATED_EMOJI: &[&str] = &["🪢", "🔗"];

/// One directed, optionally labelled relation entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RelationEntry {
    pub(crate) target: String,
    pub(crate) block_id: String,
    pub(crate) label: Option<String>,
    pub(crate) kind: RelationEntryKind,
    pub(crate) embedded: bool,
    pub(crate) struck: bool,
    pub(crate) aliased: bool,
}

/// A stored LINKED FROM entry is either an authored relation mirror or a
/// derived text mention. Mentions never have relation labels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RelationEntryKind {
    Relation,
    Mention,
}

impl RelationEntry {
    /// The label spelling used by the existing LINKED FROM wire grammar.
    /// Mention spelling is an encoding of its kind, never a relation label.
    pub(crate) fn storage_label(&self) -> Option<String> {
        match self.kind {
            RelationEntryKind::Relation => self.label.clone(),
            RelationEntryKind::Mention => Some(MENTION_LABEL.to_string()),
        }
    }

    pub(crate) fn canonical_text(&self) -> String {
        if self.target.is_empty() {
            format!("[[#^{}]]", self.block_id)
        } else {
            format!("[[{}#^{}]]", self.target, self.block_id)
        }
    }
}

/// Parse verdict for one candidate LINKS line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum LinksLine {
    Accepted {
        entries: Vec<RelationEntry>,
        canonical: bool,
    },
    Empty,
    Malformed,
    NotALine,
}

/// Parse verdict for one candidate LINKED FROM line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum LinkedFromLine {
    Accepted {
        entries: Vec<RelationEntry>,
        canonical: bool,
    },
    Empty,
    Malformed,
    NotALine,
}

pub(crate) fn is_links_line(line: &str) -> bool {
    !matches!(parse::parse_links_line(line), LinksLine::NotALine)
}

pub(crate) fn is_linked_from_line(line: &str) -> bool {
    !matches!(
        parse::parse_linked_from_line(line),
        LinkedFromLine::NotALine
    )
}

pub(crate) fn is_relation_line(line: &str) -> bool {
    is_links_line(line) || is_linked_from_line(line)
}
