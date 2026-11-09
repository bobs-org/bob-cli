//! Label routing for `bob gmail sync`: a thread's Gmail labels choose one
//! area or project note, no note (unrouted), or a typed route problem.
//!
//! Pure over its inputs: the account's label list (one `labels list` call
//! per run), the thread's label ids, the enrollment label, the configured
//! prefixes, and a [`RouteContext`] scanned once per run. Bob never routes
//! by label-name search and never creates a note. See `sync.route_prefixes`
//! in `docs/gmail.md` "Configuration".

use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    path::{Path, PathBuf},
};

use super::gog::GmailLabel;
use crate::native::{
    capture,
    parent_notes::{self, ParentCandidate, ParentError, ResolvedParent},
    projects::{frontmatter_value, parse_frontmatter, wikilink_path_target},
};

/// The account's labels keyed by Gmail label id.
pub(crate) type LabelsById = HashMap<String, GmailLabel>;

/// Key one `labels list` response by id.
pub(crate) fn labels_by_id(labels: Vec<GmailLabel>) -> LabelsById {
    labels
        .into_iter()
        .map(|label| (label.id.clone(), label))
        .collect()
}

/// Everything routing reads from the vault, scanned once per run: the
/// parent candidates and each candidate's `parent:` link.
pub(crate) struct RouteContext {
    bob_dir: PathBuf,
    candidates: Vec<ParentCandidate>,
    parents: HashMap<String, String>,
}

impl RouteContext {
    pub(crate) fn load(bob_dir: &Path) -> Self {
        let candidates = parent_notes::parent_candidates(bob_dir);
        let parents = candidates
            .iter()
            .filter_map(|candidate| {
                read_parent_route(bob_dir, &candidate.route)
                    .map(|parent| (candidate.route.clone(), parent))
            })
            .collect();
        Self {
            bob_dir: bob_dir.to_path_buf(),
            candidates,
            parents,
        }
    }

    fn resolve(&self, input: &str) -> Result<ResolvedParent, ParentError> {
        parent_notes::resolve_parent_in(&self.bob_dir, &self.candidates, input)
    }

    /// Revalidate a canonical route against a fresh vault snapshot while a
    /// sync transaction holds the maintenance lock. This keeps a destination
    /// that was renamed, closed, or retyped during Gmail preparation from
    /// receiving a stale task.
    pub(crate) fn resolve_route(&self, route: &str) -> Option<ResolvedParent> {
        self.resolve(route)
            .ok()
            .filter(|parent| parent.route == route)
    }

    /// The route a note's `parent:` links to. A chain may pass through a
    /// note that is not a candidate (a done project), which is read here.
    fn parent_of(&self, route: &str) -> Option<String> {
        if let Some(parent) = self.parents.get(route) {
            return Some(parent.clone());
        }
        if self
            .candidates
            .iter()
            .any(|candidate| candidate.route == route)
        {
            return None;
        }
        read_parent_route(&self.bob_dir, route)
    }

    /// Whether `ancestor` lies on `route`'s `parent:` chain.
    fn is_ancestor(&self, ancestor: &str, route: &str) -> bool {
        let mut seen = BTreeSet::from([route.to_string()]);
        let mut current = route.to_string();
        while let Some(parent) = self.parent_of(&current) {
            if parent == ancestor {
                return true;
            }
            if !seen.insert(parent.clone()) {
                return false;
            }
            current = parent;
        }
        false
    }
}

/// The `parent:` route of one root note, when it links a routable stem.
fn read_parent_route(bob_dir: &Path, route: &str) -> Option<String> {
    if !capture::is_route_token(route) {
        return None;
    }
    let contents =
        std::fs::read_to_string(bob_dir.join(format!("{route}.md"))).ok()?;
    let frontmatter = parse_frontmatter(&contents)?;
    let target =
        wikilink_path_target(frontmatter_value(&frontmatter, "parent")?)?;
    let stem = target.rsplit('/').next()?.trim();
    let stem = stem
        .strip_suffix(".md")
        .unwrap_or(stem)
        .to_ascii_lowercase();
    capture::is_route_token(&stem).then_some(stem)
}

/// Where a thread's task goes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum RouteVerdict {
    /// File the task into this area or project note.
    Route(ResolvedParent),
    /// No routing label: file the task into the sync inbox.
    Unrouted,
    /// File the task into the sync inbox with a route warning.
    Problem(RouteProblem),
}

/// Why a thread's routing labels do not choose one note.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RouteProblemKind {
    UnknownNote,
    NotAreaOrProject,
    ClosedProject,
    AmbiguousAlias,
    NestedRoute,
    AmbiguousRoute,
}

impl RouteProblemKind {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::UnknownNote => "unknown_note",
            Self::NotAreaOrProject => "not_area_or_project",
            Self::ClosedProject => "closed_project",
            Self::AmbiguousAlias => "ambiguous_alias",
            Self::NestedRoute => "nested_route",
            Self::AmbiguousRoute => "ambiguous_route",
        }
    }
}

/// A typed route problem: the offending label (or labels, for
/// `ambiguous_route`) plus a one-line message and hint.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RouteProblem {
    pub(crate) kind: RouteProblemKind,
    pub(crate) label: String,
    pub(crate) message: String,
    pub(crate) hint: String,
}

impl RouteProblem {
    /// The resolver's own message line and first hint line.
    fn from_parent_error(label: &str, error: &ParentError) -> Self {
        let kind = match error {
            ParentError::Unknown { .. } => RouteProblemKind::UnknownNote,
            ParentError::NotParent { .. } => RouteProblemKind::NotAreaOrProject,
            ParentError::Notebook { .. } => RouteProblemKind::NotAreaOrProject,
            ParentError::Terminal { .. } => RouteProblemKind::ClosedProject,
            ParentError::Ambiguous { .. } => RouteProblemKind::AmbiguousAlias,
        };
        let text = error.message();
        let mut lines = text.lines();
        let message = lines.next().unwrap_or_default().to_string();
        let hint = lines
            .find_map(|line| line.strip_prefix("hint: "))
            .unwrap_or_default()
            .to_string();
        Self {
            kind,
            label: label.to_string(),
            message,
            hint,
        }
    }

    fn nested(label: &str, remainder: &str) -> Self {
        Self {
            kind: RouteProblemKind::NestedRoute,
            label: label.to_string(),
            message: format!(
                "routing label '{label}' names the nested path '{remainder}'"
            ),
            hint: "use one label per note".to_string(),
        }
    }

    fn ambiguous(routes: &[(&str, ResolvedParent)]) -> Self {
        let labels = routes
            .iter()
            .map(|(label, _)| *label)
            .collect::<Vec<_>>()
            .join(", ");
        let notes = routes
            .iter()
            .map(|(_, parent)| parent.label.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        Self {
            kind: RouteProblemKind::AmbiguousRoute,
            label: labels.clone(),
            message: format!(
                "routing labels {labels} name notes on different parent chains ({notes})"
            ),
            hint: "keep one routing label, or only labels whose notes share \
                   one parent: chain"
                .to_string(),
        }
    }

    /// `label → kind · hint`, as the doctor `routes` row lists it.
    pub(crate) fn audit_line(&self) -> String {
        format!("{} → {} · {}", self.label, self.kind.as_str(), self.hint)
    }
}

/// Route one thread from the union of its messages' label ids.
///
/// - Prefix mode (`prefixes` non-empty): a user label named `P/<rest>`
///   for a configured `P` is a routing label. Every routing label must
///   resolve; the first that doesn't (by label name) is the problem.
/// - No-prefix mode: every user label except the enrollment label is a
///   candidate; candidates that don't resolve are ignored.
///
/// Several resolving labels route to their deepest note when they name one
/// note or lie on one `parent:` chain, and are `ambiguous_route` otherwise.
pub(crate) fn route<I, S>(
    labels_by_id: &LabelsById,
    thread_label_ids: I,
    enrollment_label: Option<&str>,
    prefixes: &[String],
    ctx: &RouteContext,
) -> RouteVerdict
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let names = thread_label_ids
        .into_iter()
        .filter_map(|id| labels_by_id.get(id.as_ref()))
        .filter(|label| is_route_candidate(label, enrollment_label))
        .map(|label| label.name.as_str())
        .collect::<BTreeSet<_>>();

    let mut resolved = Vec::new();
    for name in names {
        if prefixes.is_empty() {
            if let Ok(parent) = ctx.resolve(name) {
                resolved.push((name, parent));
            }
            continue;
        }
        match check_routing_label(name, prefixes, ctx) {
            None => {}
            Some(Ok(parent)) => resolved.push((name, parent)),
            Some(Err(problem)) => return RouteVerdict::Problem(problem),
        }
    }
    deepest_route(resolved, ctx)
}

/// The doctor's view of every routing label in the account.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LabelAudit {
    pub(crate) routing_labels: usize,
    pub(crate) problems: Vec<RouteProblem>,
}

/// Check every user label under a configured prefix, sorted by name. Only
/// meaningful in prefix mode: no-prefix mode has no routing labels to audit.
pub(crate) fn audit_labels(
    labels: &[GmailLabel],
    enrollment_label: Option<&str>,
    prefixes: &[String],
    ctx: &RouteContext,
) -> LabelAudit {
    let names = labels
        .iter()
        .filter(|label| is_route_candidate(label, enrollment_label))
        .map(|label| label.name.as_str())
        .collect::<BTreeSet<_>>();
    let mut audit = LabelAudit {
        routing_labels: 0,
        problems: Vec::new(),
    };
    for name in names {
        match check_routing_label(name, prefixes, ctx) {
            None => {}
            Some(Ok(_)) => audit.routing_labels += 1,
            Some(Err(problem)) => {
                audit.routing_labels += 1;
                audit.problems.push(problem);
            }
        }
    }
    audit
}

fn is_route_candidate(
    label: &GmailLabel,
    enrollment_label: Option<&str>,
) -> bool {
    label.is_user()
        && !enrollment_label.is_some_and(|enrollment| {
            label.name.eq_ignore_ascii_case(enrollment)
        })
}

/// `None` when `name` is not a routing label (unprefixed, or a bare `P`
/// parent label); otherwise its resolution or its problem.
fn check_routing_label(
    name: &str,
    prefixes: &[String],
    ctx: &RouteContext,
) -> Option<Result<ResolvedParent, RouteProblem>> {
    let remainder = prefixes.iter().find_map(|prefix| {
        name.strip_prefix(prefix.as_str())?.strip_prefix('/')
    })?;
    if remainder.contains('/') {
        return Some(Err(RouteProblem::nested(name, remainder)));
    }
    Some(
        ctx.resolve(remainder)
            .map_err(|error| RouteProblem::from_parent_error(name, &error)),
    )
}

/// One note, or the deepest note of one `parent:` chain, or a conflict.
fn deepest_route(
    resolved: Vec<(&str, ResolvedParent)>,
    ctx: &RouteContext,
) -> RouteVerdict {
    let mut by_route: BTreeMap<String, (&str, ResolvedParent)> =
        BTreeMap::new();
    for (label, parent) in resolved {
        by_route
            .entry(parent.route.clone())
            .or_insert((label, parent));
    }
    let mut routes = by_route.into_values().collect::<Vec<_>>();
    routes.sort_by(|left, right| left.0.cmp(right.0));
    match routes.as_slice() {
        [] => RouteVerdict::Unrouted,
        [(_, parent)] => RouteVerdict::Route(parent.clone()),
        _ => routes
            .iter()
            .find(|(_, deepest)| {
                routes.iter().all(|(_, other)| {
                    other.route == deepest.route
                        || ctx.is_ancestor(&other.route, &deepest.route)
                })
            })
            .map_or_else(
                || RouteVerdict::Problem(RouteProblem::ambiguous(&routes)),
                |(_, deepest)| RouteVerdict::Route(deepest.clone()),
            ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::native::capture_targets::CaptureTargetKind;
    use std::{
        fs,
        sync::atomic::{AtomicUsize, Ordering},
        time::{SystemTime, UNIX_EPOCH},
    };

    static TEMP_COUNTER: AtomicUsize = AtomicUsize::new(0);

    struct TempVault {
        path: PathBuf,
    }

    impl TempVault {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "bob-cli-gmail-route-{}-{}-{}",
                std::process::id(),
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .expect("clock")
                    .as_nanos(),
                TEMP_COUNTER.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir_all(&path).expect("create temp vault");
            Self { path }
        }

        fn write(&self, name: &str, contents: &str) {
            fs::write(self.path.join(name), contents).expect("write note");
        }
    }

    impl Drop for TempVault {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }

    /// The plan's routing vault: areas `job` (alias `Job`), `body`, and
    /// `family`; projects `job_2026` → `job` and `job_2026_apply_29` →
    /// `job_2026`; the done project `old_trip`; and `home.md` with no type.
    fn vault() -> TempVault {
        let vault = TempVault::new();
        vault.write(
            "job.md",
            "---\ntype: \"[[area]]\"\naliases: [Job]\n---\n# Job\n",
        );
        vault.write("body.md", "---\ntype: \"[[area]]\"\n---\n");
        vault.write("family.md", "---\ntype: \"[[area]]\"\n---\n");
        vault.write(
            "job_2026.md",
            "---\ntype: \"[[project]]\"\nstatus: wip\nparent: \"[[job]]\"\n---\n",
        );
        vault.write(
            "job_2026_apply_29.md",
            "---\ntype: \"[[project]]\"\nstatus: wip\nparent: \"[[job_2026|Job 2026]]\"\n---\n",
        );
        vault.write(
            "old_trip.md",
            "---\ntype: \"[[project]]\"\nstatus: done\n---\n",
        );
        vault.write("home.md", "---\ntitle: Home\n---\n# Home\n");
        vault
    }

    fn label(id: &str, name: &str, kind: &str) -> GmailLabel {
        GmailLabel {
            id: id.to_string(),
            name: name.to_string(),
            kind: Some(kind.to_string()),
        }
    }

    /// Every label any vector uses; the thread picks names from it.
    fn account_labels() -> Vec<GmailLabel> {
        let mut labels = vec![
            label("INBOX", "INBOX", "system"),
            label("UNREAD", "UNREAD", "system"),
            label("Label_bare_hash", "#", "user"),
        ];
        for (index, name) in [
            "bob",
            "#/job",
            "+/Job",
            "+/job_2026_apply_29",
            "#/body",
            "#/dad",
            "#/famly",
            "#/home",
            "+/old_trip",
            "+/home/build",
            "+/dad",
            "+/job",
            "Job",
            "job",
            "body",
            "@/NEXT",
        ]
        .into_iter()
        .enumerate()
        {
            labels.push(label(&format!("Label_{index}"), name, "user"));
        }
        labels
    }

    fn prefixes() -> Vec<String> {
        vec!["#".to_string(), "+".to_string()]
    }

    fn route_names(
        ctx: &RouteContext,
        names: &[&str],
        prefixes: &[String],
    ) -> RouteVerdict {
        let labels = account_labels();
        let ids = names
            .iter()
            .map(|name| {
                labels
                    .iter()
                    .find(|label| label.name == *name)
                    .unwrap_or_else(|| panic!("no label {name}"))
                    .id
                    .clone()
            })
            .collect::<Vec<_>>();
        route(&labels_by_id(labels), &ids, Some("bob"), prefixes, ctx)
    }

    fn routed(verdict: &RouteVerdict) -> &str {
        match verdict {
            RouteVerdict::Route(parent) => &parent.route,
            other => panic!("expected a route, got {other:?}"),
        }
    }

    fn problem(verdict: &RouteVerdict) -> &RouteProblem {
        match verdict {
            RouteVerdict::Problem(problem) => problem,
            other => panic!("expected a problem, got {other:?}"),
        }
    }

    #[test]
    fn r1_one_routing_label_routes_to_its_area() {
        let vault = vault();
        let ctx = RouteContext::load(&vault.path);
        let verdict = route_names(&ctx, &["bob", "#/job"], &prefixes());
        assert_eq!(routed(&verdict), "job");
        let RouteVerdict::Route(parent) = verdict else {
            unreachable!()
        };
        assert_eq!(parent.kind, CaptureTargetKind::Area);
        assert_eq!(parent.label, "job.md");
    }

    #[test]
    fn r2_enrollment_label_alone_is_unrouted() {
        let vault = vault();
        let ctx = RouteContext::load(&vault.path);
        assert_eq!(
            route_names(&ctx, &["bob"], &prefixes()),
            RouteVerdict::Unrouted
        );
    }

    #[test]
    fn r3_routing_is_case_insensitive() {
        let vault = vault();
        let ctx = RouteContext::load(&vault.path);
        let verdict = route_names(&ctx, &["bob", "+/Job"], &prefixes());
        assert_eq!(routed(&verdict), "job");
    }

    #[test]
    fn r4_labels_on_one_chain_route_to_the_deepest_note() {
        let vault = vault();
        let ctx = RouteContext::load(&vault.path);
        let verdict = route_names(
            &ctx,
            &["bob", "#/job", "+/job_2026_apply_29"],
            &prefixes(),
        );
        assert_eq!(routed(&verdict), "job_2026_apply_29");
        let RouteVerdict::Route(parent) = verdict else {
            unreachable!()
        };
        assert_eq!(parent.kind, CaptureTargetKind::Project);
    }

    #[test]
    fn r5_labels_on_different_chains_are_ambiguous() {
        let vault = vault();
        let ctx = RouteContext::load(&vault.path);
        let verdict =
            route_names(&ctx, &["bob", "#/job", "#/body"], &prefixes());
        let problem = problem(&verdict);
        assert_eq!(problem.kind, RouteProblemKind::AmbiguousRoute);
        assert_eq!(problem.kind.as_str(), "ambiguous_route");
        assert_eq!(problem.label, "#/body, #/job");
        assert!(
            problem.message.contains("body.md")
                && problem.message.contains("job.md"),
            "{}",
            problem.message
        );
        assert!(!problem.hint.is_empty());
    }

    #[test]
    fn r6_unknown_note_is_a_problem_with_the_resolver_hint() {
        let vault = vault();
        let ctx = RouteContext::load(&vault.path);
        let verdict = route_names(&ctx, &["bob", "#/dad"], &prefixes());
        let problem = problem(&verdict);
        assert_eq!(problem.kind, RouteProblemKind::UnknownNote);
        assert_eq!(problem.kind.as_str(), "unknown_note");
        assert_eq!(problem.label, "#/dad");
        assert_eq!(problem.message, "no area or project named 'dad'");
        assert_eq!(
            problem.hint,
            "run 'bob capture-targets' to list routable notes"
        );
    }

    #[test]
    fn r6_unknown_note_carries_a_near_miss_when_the_resolver_has_one() {
        let vault = vault();
        let ctx = RouteContext::load(&vault.path);
        let verdict = route_names(&ctx, &["bob", "#/famly"], &prefixes());
        let problem = problem(&verdict);
        assert_eq!(problem.kind, RouteProblemKind::UnknownNote);
        assert_eq!(problem.hint, "did you mean family (area · family.md)?");
        assert_eq!(
            problem.audit_line(),
            "#/famly → unknown_note · did you mean family (area · family.md)?"
        );
    }

    #[test]
    fn r7_a_note_without_area_or_project_type_is_a_problem() {
        let vault = vault();
        let ctx = RouteContext::load(&vault.path);
        let verdict = route_names(&ctx, &["bob", "#/home"], &prefixes());
        let problem = problem(&verdict);
        assert_eq!(problem.kind, RouteProblemKind::NotAreaOrProject);
        assert_eq!(problem.kind.as_str(), "not_area_or_project");
        assert_eq!(problem.label, "#/home");
        assert_eq!(problem.message, "home.md is not an area or project note");
    }

    #[test]
    fn r8_a_closed_project_is_a_problem() {
        let vault = vault();
        let ctx = RouteContext::load(&vault.path);
        let verdict = route_names(&ctx, &["bob", "+/old_trip"], &prefixes());
        let problem = problem(&verdict);
        assert_eq!(problem.kind, RouteProblemKind::ClosedProject);
        assert_eq!(problem.kind.as_str(), "closed_project");
        assert_eq!(problem.message, "project 'old_trip' is done");
        assert!(problem.hint.contains("open area or project"));
    }

    #[test]
    fn r9_a_nested_remainder_is_a_problem() {
        let vault = vault();
        let ctx = RouteContext::load(&vault.path);
        let verdict = route_names(&ctx, &["bob", "+/home/build"], &prefixes());
        let problem = problem(&verdict);
        assert_eq!(problem.kind, RouteProblemKind::NestedRoute);
        assert_eq!(problem.kind.as_str(), "nested_route");
        assert_eq!(problem.label, "+/home/build");
        assert_eq!(problem.hint, "use one label per note");
    }

    #[test]
    fn r10_every_routing_label_must_resolve() {
        let vault = vault();
        let ctx = RouteContext::load(&vault.path);
        let verdict =
            route_names(&ctx, &["bob", "#/job", "+/dad"], &prefixes());
        let problem = problem(&verdict);
        assert_eq!(problem.kind, RouteProblemKind::UnknownNote);
        assert_eq!(problem.label, "+/dad");
    }

    #[test]
    fn r11_bare_parent_and_unprefixed_labels_are_not_routes() {
        let vault = vault();
        let ctx = RouteContext::load(&vault.path);
        assert_eq!(
            route_names(&ctx, &["bob", "#", "Job"], &prefixes()),
            RouteVerdict::Unrouted
        );
    }

    #[test]
    fn r12_no_prefix_mode_routes_by_any_resolving_label() {
        let vault = vault();
        let ctx = RouteContext::load(&vault.path);
        let verdict =
            route_names(&ctx, &["bob", "Job", "@/NEXT", "INBOX"], &[]);
        assert_eq!(routed(&verdict), "job");
    }

    #[test]
    fn r13_no_prefix_mode_flags_two_unrelated_notes() {
        let vault = vault();
        let ctx = RouteContext::load(&vault.path);
        let verdict = route_names(&ctx, &["bob", "job", "body"], &[]);
        assert_eq!(problem(&verdict).kind, RouteProblemKind::AmbiguousRoute);
    }

    #[test]
    fn r14_two_labels_naming_one_note_route_to_it() {
        let vault = vault();
        let ctx = RouteContext::load(&vault.path);
        let verdict =
            route_names(&ctx, &["bob", "#/job", "+/job"], &prefixes());
        assert_eq!(routed(&verdict), "job");
    }

    #[test]
    fn an_alias_two_notes_claim_is_an_ambiguous_alias_problem() {
        let vault = vault();
        vault.write(
            "body.md",
            "---\ntype: \"[[area]]\"\nproject_name_aliases: [\"health\"]\n---\n",
        );
        vault.write(
            "family.md",
            "---\ntype: \"[[area]]\"\nproject_name_aliases: [\"health\"]\n---\n",
        );
        let ctx = RouteContext::load(&vault.path);
        let labels = labels_by_id(vec![label("Label_h", "#/health", "user")]);
        let verdict = route(&labels, ["Label_h"], None, &prefixes(), &ctx);
        let problem = problem(&verdict);
        assert_eq!(problem.kind, RouteProblemKind::AmbiguousAlias);
        assert_eq!(problem.kind.as_str(), "ambiguous_alias");
        assert!(problem.message.contains("body.md, family.md"));
        assert!(problem.hint.contains("project_name_aliases"));
    }

    #[test]
    fn no_prefix_mode_ignores_labels_that_do_not_resolve() {
        let vault = vault();
        let ctx = RouteContext::load(&vault.path);
        assert_eq!(
            route_names(&ctx, &["bob", "@/NEXT", "#/dad", "+/old_trip"], &[]),
            RouteVerdict::Unrouted
        );
    }

    #[test]
    fn unknown_ids_and_system_labels_never_route() {
        let vault = vault();
        let ctx = RouteContext::load(&vault.path);
        let labels = labels_by_id(vec![
            label("job", "job", "system"),
            GmailLabel {
                id: "CATEGORY_UPDATES".to_string(),
                name: "body".to_string(),
                kind: None,
            },
        ]);
        assert_eq!(
            route(
                &labels,
                ["job", "CATEGORY_UPDATES", "Label_missing"],
                Some("bob"),
                &[],
                &ctx
            ),
            RouteVerdict::Unrouted
        );
    }

    #[test]
    fn a_chain_through_a_closed_project_still_counts() {
        let vault = vault();
        vault.write(
            "old_trip.md",
            "---\ntype: \"[[project]]\"\nstatus: done\nparent: \"[[family]]\"\n---\n",
        );
        vault.write(
            "trip_photos.md",
            "---\ntype: \"[[project]]\"\nstatus: wip\nparent: \"[[old_trip]]\"\n---\n",
        );
        let ctx = RouteContext::load(&vault.path);
        let labels = labels_by_id(vec![
            label("Label_a", "#/family", "user"),
            label("Label_b", "+/trip_photos", "user"),
        ]);
        let verdict =
            route(&labels, ["Label_a", "Label_b"], None, &prefixes(), &ctx);
        assert_eq!(routed(&verdict), "trip_photos");
    }

    #[test]
    fn a_parent_cycle_is_ambiguous_not_a_hang() {
        let vault = TempVault::new();
        vault.write(
            "loop_a.md",
            "---\ntype: \"[[project]]\"\nstatus: wip\nparent: \"[[loop_b]]\"\n---\n",
        );
        vault.write(
            "loop_b.md",
            "---\ntype: \"[[project]]\"\nstatus: wip\nparent: \"[[loop_a]]\"\n---\n",
        );
        vault.write("other.md", "---\ntype: \"[[area]]\"\n---\n");
        let ctx = RouteContext::load(&vault.path);
        let labels = labels_by_id(vec![
            label("Label_a", "+/loop_a", "user"),
            label("Label_o", "#/other", "user"),
        ]);
        let verdict =
            route(&labels, ["Label_a", "Label_o"], None, &prefixes(), &ctx);
        assert_eq!(problem(&verdict).kind, RouteProblemKind::AmbiguousRoute);
    }

    #[test]
    fn audit_lists_every_routing_label_that_does_not_resolve() {
        let vault = vault();
        let ctx = RouteContext::load(&vault.path);
        let audit =
            audit_labels(&account_labels(), Some("bob"), &prefixes(), &ctx);
        // #/job, +/Job, +/job_2026_apply_29, #/body, +/job resolve; #/dad,
        // #/famly, #/home, +/old_trip, +/home/build, and +/dad do not. The
        // bare `#`, the unprefixed labels, and system labels are not
        // routing labels.
        assert_eq!(audit.routing_labels, 11);
        let lines = audit
            .problems
            .iter()
            .map(RouteProblem::audit_line)
            .collect::<Vec<_>>();
        assert_eq!(
            lines,
            [
                "#/dad → unknown_note · run 'bob capture-targets' to list routable notes",
                "#/famly → unknown_note · did you mean family (area · family.md)?",
                "#/home → not_area_or_project · run 'bob capture-targets' to list routable notes",
                "+/dad → unknown_note · run 'bob capture-targets' to list routable notes",
                "+/home/build → nested_route · use one label per note",
                "+/old_trip → closed_project · pick an open area or project instead (run 'bob capture-targets' to list them)",
            ]
        );
    }

    #[test]
    fn audit_skips_the_enrollment_label_even_under_a_prefix() {
        let vault = vault();
        let ctx = RouteContext::load(&vault.path);
        let labels = vec![label("Label_1", "#/bob", "user")];
        let audit = audit_labels(&labels, Some("#/BOB"), &prefixes(), &ctx);
        assert_eq!(audit.routing_labels, 0);
        assert!(audit.problems.is_empty());
    }

    #[test]
    fn labels_without_a_type_use_the_user_id_prefix() {
        let user = GmailLabel {
            id: "Label_7".to_string(),
            name: "#/job".to_string(),
            kind: None,
        };
        let system = GmailLabel {
            id: "STARRED".to_string(),
            name: "STARRED".to_string(),
            kind: None,
        };
        assert!(user.is_user());
        assert!(!system.is_user());
    }
}
