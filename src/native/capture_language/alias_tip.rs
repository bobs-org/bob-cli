//! Sigil alias ids and the typing-tip reverse check.
//!
//! Every forward sigil alias in `rewrite.rs` is stamped with an [`AliasId`].
//! [`sigil_alias_tip`] answers the reverse question for a sigil that was just
//! typed the hard way: would an unshifted alias key, typed at that exact
//! spot, have produced this very draft? It never decides that itself. It
//! swaps each candidate key in for the sigil and runs the whole
//! [`rewrite_draft`] pipeline, so rule precedence, position guards, and
//! completion eligibility stay owned by the forward rewrite. The check is
//! lexical and does no vault I/O.

use super::rewrite::{rewrite_draft, RewriteRule};

/// Declares [`AliasId`] together with its wire spelling and, for tests, the
/// full list of ids, so the three can never drift apart.
macro_rules! alias_ids {
    ($($(#[$doc:meta])* $variant:ident => $code:literal,)+) => {
        /// Stable id of one forward sigil alias. [`AliasId::code`] is the wire
        /// spelling shared with Bob Mac Capture's lesson map.
        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        pub(crate) enum AliasId {
            $($(#[$doc])* $variant,)+
        }

        impl AliasId {
            /// Every id, for tests that must cover each one.
            #[cfg(test)]
            pub(crate) const ALL: &'static [AliasId] = &[$(Self::$variant,)+];

            pub(crate) fn code(self) -> &'static str {
                match self {
                    $(Self::$variant => $code,)+
                }
            }
        }
    };
}

alias_ids! {
    /// `.` -> `@` after body text and a space or tab.
    RouteAfterText => "route_after_text",
    /// `/` -> `#` wherever a `#` menu would open.
    HashMenu => "hash_menu",
    /// `\` -> `%` wherever the directive menu would open.
    DirectiveMenu => "directive_menu",
    /// `.` -> `^` where the Block ID picker opens.
    BlockIdAfterRoute => "block_id_after_route",
    /// `;` -> `:` where the Pomodoro Block ID picker opens.
    LinkAfterRoute => "link_after_route",
    /// `;` -> `:` as a whole item, opening the Task Link picker.
    TaskLinkItem => "task_link_item",
    /// `.` -> `^` on a project-note move bullet.
    MoveBulletCaret => "move_bullet_caret",
    /// `;` -> `:` on a project-note move bullet.
    MoveBulletColon => "move_bullet_colon",
    /// `,` -> `+` where the Parent Task picker opens.
    ParentTask => "parent_task",
    /// `=` -> `~` where the Links To picker opens.
    Relation => "relation",
    /// `.` -> `&` where the Depends On picker opens.
    Dependency => "dependency",
    /// `;` -> `*` where the Blocks picker opens.
    BlockedTask => "blocked_task",
    /// `=` after `~` -> `~~`, and `=` after `~~` -> `~`.
    RelationFlip => "relation_flip",
    /// `.` completing a `- ^^` move choice.
    MoveChoice => "move_choice",
    /// A glued `,` -> ` +` continuing a target run or merge-in sources.
    NextParent => "next_parent",
    /// `-` -> `~` inside an `x=` past token.
    PastSegment => "past_segment",
    /// `x` -> `!` ending a close selection.
    CloseComplete => "close_complete",
    /// `s` -> `*` ending a close selection.
    ClosePark => "close_park",
}

impl AliasId {
    /// Whether the reverse check may teach this alias. Exhaustive on purpose:
    /// a new alias fails to compile until someone classifies it here.
    pub(crate) fn tipped(self) -> bool {
        match self {
            Self::RouteAfterText
            | Self::HashMenu
            | Self::DirectiveMenu
            | Self::BlockIdAfterRoute
            | Self::LinkAfterRoute
            | Self::TaskLinkItem
            | Self::MoveBulletCaret
            | Self::MoveBulletColon
            | Self::ParentTask
            | Self::Relation
            | Self::Dependency
            | Self::BlockedTask => true,
            // Sits next to the fast `==` double-tap escape (R8).
            Self::RelationFlip => false,
            // Sits next to the `- ^^` double-tap choice (R8).
            Self::MoveChoice => false,
            // A two-byte edit the one-byte reverse proof cannot cover (R7).
            Self::NextParent => false,
            // Deferred: past segments need their own lesson (R7).
            Self::PastSegment => false,
            // Deferred to the Close card (R7).
            Self::CloseComplete => false,
            // Deferred to the Close card (R7).
            Self::ClosePark => false,
        }
    }
}

/// Which forward alias a rewrite applied, stamped on every
/// `expand_sigil_alias` rewrite.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AliasStamp {
    pub(crate) id: AliasId,
    /// The unshifted key the user typed.
    pub(crate) key: String,
    /// The sigil the key stands for.
    pub(crate) sigil: char,
    /// A short true phrase for where the alias fires, set by the branch that
    /// knows the completion context; `None` when no short phrase is true.
    pub(crate) position: Option<&'static str>,
}

impl AliasStamp {
    pub(super) fn new(id: AliasId, key: &str, sigil: char) -> Self {
        Self {
            id,
            key: key.to_string(),
            sigil,
            position: None,
        }
    }

    pub(super) fn at(mut self, position: &'static str) -> Self {
        self.position = Some(position);
        self
    }

    pub(super) fn at_optional(
        mut self,
        position: Option<&'static str>,
    ) -> Self {
        self.position = position;
        self
    }
}

/// A proved typing tip: the unshifted `keys` (bob's preference order) that
/// would have typed the `sigil` ending at the cursor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SigilAliasTip {
    pub(crate) id: AliasId,
    pub(crate) sigil: char,
    pub(crate) keys: Vec<String>,
    /// UTF-8 byte range of the typed sigil.
    pub(crate) range: (usize, usize),
    pub(crate) position: Option<&'static str>,
}

/// Sigils a tip can teach, each with the alias keys that might have typed it
/// in bob's preference order. The list only proposes substitutions; the
/// forward rewrite decides.
const CANDIDATES: &[(char, &[&str])] = &[
    ('@', &["."]),
    ('#', &["/"]),
    ('%', &["\\"]),
    ('^', &["."]),
    (':', &[";"]),
    ('+', &[","]),
    ('~', &["="]),
    ('&', &["."]),
    ('*', &[";"]),
];

/// Prove the typing tip for the sigil that ends at `cursor`, if any.
///
/// Returns `None` for a template directive draft, when the byte ending at
/// `cursor` is not a candidate sigil, or when no candidate key reproduces
/// `raw` through the forward rewrite. A key is kept only when the forward
/// rewrite of the swapped draft is an `expand_sigil_alias` that yields
/// exactly `raw` with the caret at `cursor` and a tipped alias id.
pub(crate) fn sigil_alias_tip(
    raw: &str,
    cursor: usize,
) -> Option<SigilAliasTip> {
    let start = cursor.checked_sub(1)?;
    let sigil_text = raw.get(start..cursor)?;
    let sigil = sigil_text.chars().next()?;
    let keys = CANDIDATES
        .iter()
        .find_map(|(candidate, keys)| (*candidate == sigil).then_some(*keys))?;
    if super::template::analyze(raw).directive.is_some() {
        return None;
    }

    let mut proved: Vec<(AliasId, &str, Option<&'static str>)> = Vec::new();
    for key in keys {
        let mut candidate = raw.to_string();
        candidate.replace_range(start..cursor, key);
        let rewrite = rewrite_draft(&candidate, Some(cursor));
        if rewrite.rule != Some(RewriteRule::ExpandSigilAlias)
            || rewrite.text != raw
            || rewrite.cursor != Some(cursor)
        {
            continue;
        }
        let Some(stamp) = rewrite.alias else {
            continue;
        };
        if stamp.id.tipped() {
            proved.push((stamp.id, key, stamp.position));
        }
    }

    let (id, _, position) = *proved.first()?;
    let keys = proved
        .iter()
        .filter(|(candidate, _, _)| *candidate == id)
        .map(|(_, key, _)| (*key).to_string())
        .collect();
    Some(SigilAliasTip {
        id,
        sigil,
        keys,
        range: (start, cursor),
        position,
    })
}
