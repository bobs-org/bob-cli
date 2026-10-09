//! Strict area/project/inbox parent resolution with
//! `project_name_aliases`.
//!
//! `resolve_parent` is the single resolver for `bob ref create -P` (and,
//! later, ingest, ref jobs, capture `URL @route`, `gkeep pull`,
//! `bob ref list -P`, births, and `migrate-tasks`). Candidates are the
//! capture-target set: root area notes, non-terminal root project notes,
//! and the inboxes.

use std::path::Path;

use super::{
    capture_targets::{self, CaptureTargetKind},
    note_tasks::bounded_levenshtein,
    projects::{
        frontmatter_is_area, frontmatter_is_project, frontmatter_value,
        parse_frontmatter, ProjectStatus,
    },
};

/// One resolvable parent note.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ParentCandidate {
    pub(crate) route: String,
    pub(crate) kind: CaptureTargetKind,
    pub(crate) label: String,
    pub(crate) aliases: Vec<String>,
    pub(crate) status: Option<String>,
}

/// The capture-target set, in capture-targets order.
pub(crate) fn parent_candidates(bob_dir: &Path) -> Vec<ParentCandidate> {
    capture_targets::scan_capture_targets(bob_dir)
        .targets
        .into_iter()
        .map(|target| ParentCandidate {
            route: target.route,
            kind: target.kind,
            label: target.label,
            aliases: target.project_name_aliases,
            status: target.status,
        })
        .collect()
}

/// How an input matched its parent note.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ParentMatchKind {
    Stem,
    Alias(String),
}

/// A resolved parent: the canonical route, its kind, its label, and how
/// the input matched (`stem` or `alias:<name>`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ResolvedParent {
    pub(crate) route: String,
    pub(crate) kind: CaptureTargetKind,
    pub(crate) label: String,
    pub(crate) matched: ParentMatchKind,
}

impl ResolvedParent {
    /// `stem` or `alias:<name>` for dry-run and report lines. Later
    /// parent consumers (capture, gkeep, births) report this string.
    #[allow(dead_code)]
    pub(crate) fn how_matched(&self) -> String {
        match &self.matched {
            ParentMatchKind::Stem => "stem".to_string(),
            ParentMatchKind::Alias(alias) => format!("alias:{alias}"),
        }
    }

    pub(crate) fn kind_name(&self) -> &'static str {
        parent_kind_name(&self.kind)
    }

    /// `parent    bob  (project · bob.md · via alias bob-cli)`.
    pub(crate) fn dry_run_line(&self) -> String {
        let via = match &self.matched {
            ParentMatchKind::Stem => String::new(),
            ParentMatchKind::Alias(alias) => {
                format!(" · via alias {alias}")
            }
        };
        format!(
            "parent    {}  ({} · {}{via})",
            self.route,
            self.kind_name(),
            self.label,
        )
    }
}

pub(crate) fn parent_kind_name(kind: &CaptureTargetKind) -> &'static str {
    match kind {
        CaptureTargetKind::Inbox => "inbox",
        CaptureTargetKind::Area => "area",
        CaptureTargetKind::Project => "project",
    }
}

/// A failed parent resolution, rendered as the multi-line error/hint text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ParentError {
    Unknown {
        input: String,
        normalized: String,
        suggestions: Vec<ParentSuggestion>,
    },
    Ambiguous {
        alias: String,
        claimants: Vec<ParentSuggestion>,
    },
    Terminal {
        route: String,
        status: String,
    },
    NotParent {
        label: String,
    },
}

/// One near-miss or ambiguity claimant for hints.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ParentSuggestion {
    pub(crate) route: String,
    pub(crate) kind: &'static str,
    pub(crate) label: String,
}

impl ParentError {
    /// The full error text: one error line plus `hint:` lines.
    pub(crate) fn message(&self) -> String {
        match self {
            Self::Unknown {
                input,
                normalized,
                suggestions,
            } => {
                let mut lines =
                    vec![format!("no area or project named '{input}'")];
                for suggestion in suggestions {
                    lines.push(format!(
                        "hint: did you mean {} ({} · {})?",
                        suggestion.route, suggestion.kind, suggestion.label
                    ));
                }
                if let Some(first) = suggestions.first() {
                    lines.push(format!(
                        "hint: to accept this name, add `project_name_aliases: [\"{normalized}\"]` to {}",
                        first.label
                    ));
                } else {
                    lines.push(
                        "hint: run 'bob capture-targets' to list routable notes"
                            .to_string(),
                    );
                }
                lines.join("\n")
            }
            Self::Ambiguous { alias, claimants } => {
                let notes = claimants
                    .iter()
                    .map(|claimant| claimant.label.clone())
                    .collect::<Vec<_>>()
                    .join(", ");
                format!(
                    "alias '{alias}' is claimed by more than one note ({notes})\nhint: keep `project_name_aliases: [\"{alias}\"]` on only one note"
                )
            }
            Self::Terminal { route, status } => {
                format!(
                    "project '{route}' is {status}\nhint: pick an open area or project instead (run 'bob capture-targets' to list them)"
                )
            }
            Self::NotParent { label } => {
                format!(
                    "{label} is not an area or project note\nhint: run 'bob capture-targets' to list routable notes"
                )
            }
        }
    }
}

/// Resolve one `-P` input to its canonical parent route.
///
/// Accepts `sase`, `sase.md`, and `[[sase]]` (whitespace trimmed,
/// case-insensitive; `-` and `_` stay different characters). Exact stems
/// win over aliases; two notes claiming one alias is an ambiguity error.
/// Never fuzzy when mutating: near misses are hints, not matches.
pub(crate) fn resolve_parent(
    bob_dir: &Path,
    input: &str,
) -> Result<ResolvedParent, ParentError> {
    let normalized = normalize_parent_input(input);
    let lowered = normalized.to_ascii_lowercase();
    let candidates = parent_candidates(bob_dir);

    if !lowered.is_empty()
        && let Some(candidate) = candidates
            .iter()
            .find(|candidate| candidate.route == lowered)
    {
        return Ok(ResolvedParent {
            route: candidate.route.clone(),
            kind: candidate.kind,
            label: candidate.label.clone(),
            matched: ParentMatchKind::Stem,
        });
    }

    if !lowered.is_empty() {
        let mut claimants = Vec::new();
        for candidate in &candidates {
            if let Some(alias) = candidate
                .aliases
                .iter()
                .find(|alias| alias.to_ascii_lowercase() == lowered)
            {
                claimants.push((candidate, alias.clone()));
            }
        }
        match claimants.as_slice() {
            [(candidate, alias)] => {
                return Ok(ResolvedParent {
                    route: candidate.route.clone(),
                    kind: candidate.kind,
                    label: candidate.label.clone(),
                    matched: ParentMatchKind::Alias(alias.clone()),
                });
            }
            [_, _, ..] => {
                return Err(ParentError::Ambiguous {
                    alias: normalized.clone(),
                    claimants: claimants
                        .iter()
                        .map(|(candidate, _)| ParentSuggestion {
                            route: candidate.route.clone(),
                            kind: parent_kind_name(&candidate.kind),
                            label: candidate.label.clone(),
                        })
                        .collect(),
                });
            }
            [] => {}
        }
    }

    // A resolvable stem that fell out of the candidate set is either a
    // terminal project or a note that is not an area or project at all.
    if is_route_token_text(&lowered)
        && let Some(specific) = check_non_candidate_note(bob_dir, &lowered)
    {
        return specific;
    }

    Err(ParentError::Unknown {
        input: input.trim().to_string(),
        normalized,
        suggestions: near_miss_suggestions(&candidates, &lowered),
    })
}

/// Strip whitespace, one `.md` suffix, and one `[[…]]` wrapper (with any
/// `|alias` or `#heading` suffix) from a `-P` input.
pub(crate) fn normalize_parent_input(input: &str) -> String {
    let mut inner = input.trim();
    if inner.len() >= 4
        && let Some(stripped) = inner
            .strip_prefix("[[")
            .and_then(|rest| rest.strip_suffix("]]"))
    {
        inner = stripped.trim();
    }
    let inner = inner.split('|').next().unwrap_or(inner);
    let inner = inner.split('#').next().unwrap_or(inner);
    let mut inner = inner.trim().to_string();
    if inner.len() > 3 && inner[inner.len() - 3..].eq_ignore_ascii_case(".md") {
        inner.truncate(inner.len() - 3);
        inner = inner.trim().to_string();
    }
    inner
}

fn is_route_token_text(value: &str) -> bool {
    !value.is_empty()
        && value.bytes().all(|byte| {
            byte.is_ascii_lowercase()
                || byte.is_ascii_digit()
                || matches!(byte, b'_' | b'-')
        })
}

/// When a valid stem names no candidate, read that root note directly so a
/// terminal project and a non-parent hub report their own error.
fn check_non_candidate_note(
    bob_dir: &Path,
    lowered: &str,
) -> Option<Result<ResolvedParent, ParentError>> {
    let path = bob_dir.join(format!("{lowered}.md"));
    let contents = std::fs::read_to_string(&path).ok()?;
    let label = format!("{lowered}.md");
    let Some(frontmatter) = parse_frontmatter(&contents) else {
        return Some(Err(ParentError::NotParent { label }));
    };
    if frontmatter_is_area(&frontmatter) {
        return Some(Ok(ResolvedParent {
            route: lowered.to_string(),
            kind: CaptureTargetKind::Area,
            label,
            matched: ParentMatchKind::Stem,
        }));
    }
    if !frontmatter_is_project(&frontmatter) {
        return Some(Err(ParentError::NotParent { label }));
    }
    let status =
        ProjectStatus::parse(frontmatter_value(&frontmatter, "status"));
    if status.is_terminal() {
        return Some(Err(ParentError::Terminal {
            route: lowered.to_string(),
            status: status.label().to_string(),
        }));
    }
    Some(Ok(ResolvedParent {
        route: lowered.to_string(),
        kind: CaptureTargetKind::Project,
        label,
        matched: ParentMatchKind::Stem,
    }))
}

/// Up to three near misses (distance 2 over stems then aliases) for hints.
fn near_miss_suggestions(
    candidates: &[ParentCandidate],
    lowered: &str,
) -> Vec<ParentSuggestion> {
    if lowered.is_empty() {
        return Vec::new();
    }
    let mut scored: Vec<(usize, &ParentCandidate)> = Vec::new();
    for candidate in candidates {
        let distance = bounded_levenshtein(lowered, &candidate.route, 2)
            .or_else(|| {
                candidate
                    .aliases
                    .iter()
                    .filter_map(|alias| {
                        bounded_levenshtein(
                            lowered,
                            &alias.to_ascii_lowercase(),
                            2,
                        )
                    })
                    .min()
            });
        if let Some(distance) = distance
            && !scored.iter().any(|(_, seen)| seen.route == candidate.route)
        {
            scored.push((distance, candidate));
        }
    }
    scored.sort_by(|left, right| {
        left.0
            .cmp(&right.0)
            .then_with(|| left.1.route.cmp(&right.1.route))
    });
    scored
        .into_iter()
        .take(3)
        .map(|(_, candidate)| ParentSuggestion {
            route: candidate.route.clone(),
            kind: parent_kind_name(&candidate.kind),
            label: candidate.label.clone(),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        fs,
        path::PathBuf,
        sync::atomic::{AtomicUsize, Ordering},
        time::{SystemTime, UNIX_EPOCH},
    };

    static TEMP_COUNTER: AtomicUsize = AtomicUsize::new(0);

    struct TempDir {
        path: PathBuf,
    }

    impl TempDir {
        fn new(prefix: &str) -> Self {
            let mut path = std::env::temp_dir();
            path.push(format!(
                "{prefix}-{}-{}-{}",
                std::process::id(),
                current_time_nanos(),
                TEMP_COUNTER.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir_all(&path).expect("create temp dir");
            Self { path }
        }

        fn path(&self) -> &Path {
            &self.path
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }

    fn current_time_nanos() -> u128 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time before epoch")
            .as_nanos()
    }

    fn write_file(path: &Path, contents: &str) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("create parent");
        }
        fs::write(path, contents).expect("write file");
    }

    fn vault() -> TempDir {
        let temp = TempDir::new("bob-cli-parent-notes");
        write_file(&temp.path().join("sase.md"), "---\ntype: [[area]]\n---\n");
        write_file(
            &temp.path().join("bob.md"),
            "---\ntype: [[project]]\nstatus: wip\nproject_name_aliases: [\"bob-cli\"]\n---\n",
        );
        write_file(&temp.path().join("inbox.md"), "---\ntype: [[area]]\n---\n");
        write_file(
            &temp.path().join("gkeep_inbox.md"),
            "---\ntype: [[area]]\n---\n",
        );
        write_file(
            &temp.path().join("done_proj.md"),
            "---\ntype: [[project]]\nstatus: done\n---\n",
        );
        write_file(
            &temp.path().join("obsidian_ref.md"),
            "---\ntype: [[ref]]\n---\n",
        );
        temp
    }

    #[test]
    fn resolves_stem_md_and_wikilink_forms() {
        let temp = vault();
        for input in ["sase", "sase.md", "[[sase]]", "  SASE  "] {
            let resolved =
                resolve_parent(temp.path(), input).unwrap_or_else(|error| {
                    panic!("resolve {input:?}: {}", error.message())
                });
            assert_eq!(resolved.route, "sase");
            assert_eq!(resolved.matched, ParentMatchKind::Stem);
        }
    }

    #[test]
    fn resolves_alias_and_reports_how_it_matched() {
        let temp = vault();
        let resolved =
            resolve_parent(temp.path(), "bob-cli").expect("alias resolves");
        assert_eq!(resolved.route, "bob");
        assert_eq!(
            resolved.matched,
            ParentMatchKind::Alias("bob-cli".to_string())
        );
        assert_eq!(resolved.how_matched(), "alias:bob-cli");
        assert_eq!(
            resolved.dry_run_line(),
            "parent    bob  (project · bob.md · via alias bob-cli)"
        );
    }

    #[test]
    fn stem_wins_over_alias() {
        let temp = TempDir::new("bob-cli-parent-stem-wins");
        write_file(
            &temp.path().join("bob.md"),
            "---\ntype: [[project]]\nstatus: wip\n---\n",
        );
        write_file(
            &temp.path().join("sase.md"),
            "---\ntype: [[area]]\nproject_name_aliases: [\"bob\"]\n---\n",
        );
        let resolved = resolve_parent(temp.path(), "bob").expect("stem wins");
        assert_eq!(resolved.route, "bob");
        assert_eq!(resolved.matched, ParentMatchKind::Stem);
    }

    #[test]
    fn ambiguous_alias_names_both_notes() {
        let temp = TempDir::new("bob-cli-parent-ambiguous");
        write_file(
            &temp.path().join("a_one.md"),
            "---\ntype: [[area]]\nproject_name_aliases: [\"shared\"]\n---\n",
        );
        write_file(
            &temp.path().join("b_two.md"),
            "---\ntype: [[area]]\nproject_name_aliases: [\"shared\"]\n---\n",
        );
        let error = resolve_parent(temp.path(), "shared")
            .expect_err("ambiguous alias fails");
        let message = error.message();
        assert!(
            message.contains("a_one.md") && message.contains("b_two.md"),
            "ambiguity names both notes: {message}"
        );
    }

    #[test]
    fn unknown_names_near_miss_and_alias_hint() {
        let temp = vault();
        let error = resolve_parent(temp.path(), "bob-cli ")
            .expect("trailing space still resolves");
        assert_eq!(error.route, "bob");

        let error =
            resolve_parent(temp.path(), "bo").expect_err("unknown fails");
        let message = error.message();
        assert!(
            message.contains("no area or project named 'bo'"),
            "{message}"
        );
        assert!(message.contains("did you mean bob"), "{message}");
        assert!(message.contains("project_name_aliases"), "{message}");
    }

    #[test]
    fn terminal_project_and_non_parent_hub_report_themselves() {
        let temp = vault();
        let error = resolve_parent(temp.path(), "done_proj")
            .expect_err("terminal project fails");
        assert!(
            error.message().contains("project 'done_proj' is done"),
            "{}",
            error.message()
        );
        let error =
            resolve_parent(temp.path(), "obsidian_ref").expect_err("hub fails");
        assert!(
            error
                .message()
                .contains("obsidian_ref.md is not an area or project note"),
            "{}",
            error.message()
        );
    }

    #[test]
    fn dash_and_underscore_are_different() {
        let temp = TempDir::new("bob-cli-parent-dash");
        write_file(
            &temp.path().join("my_note.md"),
            "---\ntype: [[area]]\n---\n",
        );
        assert!(
            resolve_parent(temp.path(), "my-note").is_err(),
            "a dash must not match an underscore stem"
        );
        assert_eq!(
            resolve_parent(temp.path(), "my_note")
                .expect("exact stem")
                .route,
            "my_note"
        );
    }

    #[test]
    fn each_inbox_resolves() {
        let temp = vault();
        for inbox in ["mac_inbox", "inbox", "gkeep_inbox"] {
            let resolved =
                resolve_parent(temp.path(), inbox).unwrap_or_else(|error| {
                    panic!("resolve {inbox}: {}", error.message())
                });
            assert_eq!(resolved.route, inbox);
        }
    }
}
