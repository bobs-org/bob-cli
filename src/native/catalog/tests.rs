//! Coverage tests for the command catalog.

use std::collections::{BTreeSet, HashSet};

use super::assemble::{document, option_tokens, public_leaves, PublicLeaf};
use super::table::{
    Effect, Policy, ENTRIES, EXCLUDED, KNOWN_INVALIDATES, KNOWN_SLOTS,
};
use crate::runner::{subcommands, Section};

fn leaf_paths() -> BTreeSet<Vec<String>> {
    public_leaves().into_iter().map(|leaf| leaf.path).collect()
}

fn path_key(path: &[&str]) -> Vec<String> {
    path.iter().map(|part| part.to_string()).collect()
}

fn find_leaf<'a>(
    leaves: &'a [PublicLeaf],
    path: &[&str],
) -> Option<&'a PublicLeaf> {
    let key = path_key(path);
    leaves.iter().find(|leaf| leaf.path == key)
}

fn kebab_case(id: &str) -> bool {
    let mut chars = id.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if !first.is_ascii_lowercase() {
        return false;
    }
    let mut prev_dash = false;
    for char in chars {
        if char == '-' {
            if prev_dash {
                return false;
            }
            prev_dash = true;
            continue;
        }
        if !char.is_ascii_lowercase() && !char.is_ascii_digit() {
            return false;
        }
        prev_dash = false;
    }
    !prev_dash
}

#[test]
fn every_path_resolves_to_a_public_leaf() {
    let leaves = public_leaves();
    for row in ENTRIES {
        assert!(
            find_leaf(&leaves, row.path).is_some(),
            "catalog path {:?} is not a public leaf",
            row.path
        );
    }
    for row in EXCLUDED {
        assert!(
            find_leaf(&leaves, row.path).is_some(),
            "excluded path {:?} is not a public leaf",
            row.path
        );
    }
}

#[test]
fn preview_and_fixed_options_exist_on_the_leaf() {
    let leaves = public_leaves();
    for row in ENTRIES {
        let leaf = find_leaf(&leaves, row.path)
            .unwrap_or_else(|| panic!("missing leaf {:?}", row.path));
        let mut tokens = option_tokens(row.fixed);
        if let Some(preview) = row.preview {
            tokens.extend(option_tokens(preview));
        }
        for token in tokens {
            assert!(
                leaf.flags.contains(&token),
                "{}: {token} is not a flag on {:?}",
                row.id,
                row.path
            );
        }
    }
}

#[test]
fn public_leaves_are_entries_or_excluded() {
    let catalog = document();
    let entry_paths: HashSet<Vec<String>> = catalog
        .entries
        .iter()
        .map(|entry| entry.path.clone())
        .collect();
    let excluded_paths: HashSet<Vec<String>> = catalog
        .excluded
        .iter()
        .map(|row| row.path.clone())
        .collect();
    for leaf in public_leaves() {
        let section = subcommands()
            .iter()
            .find(|entry| {
                Some(entry.name) == leaf.path.first().map(String::as_str)
            })
            .map(|entry| entry.section);
        if section == Some(Section::CaptureProtocol) {
            assert!(
                excluded_paths.contains(&leaf.path),
                "capture-protocol leaf {:?} must be excluded",
                leaf.path
            );
            continue;
        }
        let in_entries = entry_paths.contains(&leaf.path);
        let in_excluded = excluded_paths.contains(&leaf.path);
        assert!(
            in_entries || in_excluded,
            "public leaf {:?} is neither an entry nor excluded",
            leaf.path
        );
        if in_entries && in_excluded {
            let rows: Vec<_> = ENTRIES
                .iter()
                .filter(|row| path_key(row.path) == leaf.path)
                .collect();
            assert!(
                rows.iter().all(|row| {
                    !row.fixed.is_empty()
                        || row.slots.iter().any(|(_, slot)| *slot == "url")
                }),
                "path {:?} is in both lists without non-empty fixed or a url slot",
                leaf.path
            );
        }
    }
}

#[test]
fn ids_are_unique_kebab_case_and_keywords_are_lowercase() {
    let mut seen = HashSet::new();
    for row in ENTRIES {
        assert!(kebab_case(row.id), "id {} is not kebab-case", row.id);
        assert!(seen.insert(row.id), "duplicate id {}", row.id);
        for keyword in row.keywords {
            assert_eq!(
                *keyword,
                keyword.to_ascii_lowercase(),
                "{} keyword {keyword:?} is not lowercase",
                row.id
            );
            assert!(!keyword.is_empty(), "{} has an empty keyword", row.id);
        }
    }
}

#[test]
fn policy_invariants() {
    for row in ENTRIES {
        match row.policy {
            Policy::Confirm => assert!(
                row.confirm.is_some_and(|text| !text.is_empty()),
                "{} confirm policy needs confirm text",
                row.id
            ),
            Policy::PreviewFirst => assert!(
                row.preview.is_some_and(|parts| !parts.is_empty()),
                "{} preview-first policy needs preview args",
                row.id
            ),
            Policy::Run => {}
        }
        if row.effect == Effect::Reads {
            assert_eq!(
                row.policy,
                Policy::Run,
                "{} reads must use policy run",
                row.id
            );
            assert!(
                row.preview.is_none(),
                "{} reads must not set preview",
                row.id
            );
        }
    }
}

#[test]
fn section_matches_root_subcommands() {
    let catalog = document();
    for entry in &catalog.entries {
        let root = entry.path.first().expect("entry path");
        let expected = subcommands()
            .iter()
            .find(|row| row.name == root)
            .and_then(|row| row.section.catalog_id())
            .expect("root section");
        assert_eq!(
            entry.section, expected,
            "{} section should be {expected}",
            entry.id
        );
    }
}

#[test]
fn slots_and_invalidates_are_known() {
    let slots: HashSet<&str> = KNOWN_SLOTS.iter().copied().collect();
    let invalidates: HashSet<&str> =
        KNOWN_INVALIDATES.iter().copied().collect();
    for row in ENTRIES {
        for (_, slot) in row.slots {
            assert!(
                slots.contains(slot),
                "{} slot {slot} is not in the known set",
                row.id
            );
        }
        for extra in row.extra_args {
            assert!(
                slots.contains(extra.slot),
                "{} extra arg slot {} is not in the known set",
                row.id,
                extra.slot
            );
        }
        for key in row.invalidates {
            assert!(
                invalidates.contains(key),
                "{} invalidate {key} is not in the known set",
                row.id
            );
        }
    }
}

#[test]
fn aliases_resolve_to_entries_or_excluded_or_public_prefixes() {
    let catalog = document();
    let leaves = leaf_paths();
    let entry_paths: HashSet<&[String]> = catalog
        .entries
        .iter()
        .map(|entry| entry.path.as_slice())
        .collect();
    let excluded_paths: HashSet<&[String]> = catalog
        .excluded
        .iter()
        .map(|row| row.path.as_slice())
        .collect();
    for alias in &catalog.aliases {
        let is_leaf = leaves.contains(&alias.to);
        let is_prefix = leaves.iter().any(|leaf| leaf.starts_with(&alias.to));
        assert!(
            is_leaf || is_prefix,
            "alias {:?} → {:?} does not resolve to a public path",
            alias.from,
            alias.to
        );
        if is_leaf {
            assert!(
                entry_paths.contains(alias.to.as_slice())
                    || excluded_paths.contains(alias.to.as_slice()),
                "alias {:?} → {:?} leaf is neither an entry nor excluded",
                alias.from,
                alias.to
            );
        }
    }
}

#[test]
fn required_keyword_lookups() {
    let catalog = document();
    let by_keyword = |query: &str| -> BTreeSet<String> {
        catalog
            .entries
            .iter()
            .filter(|entry| {
                entry.keywords.iter().any(|keyword| keyword == query)
            })
            .map(|entry| entry.id.clone())
            .collect()
    };
    for query in ["push", "pull", "git", "commit"] {
        let ids = by_keyword(query);
        assert!(
            ids.contains("vault-sync"),
            "{query} should find vault-sync: {ids:?}"
        );
    }
    for query in ["keep", "inbox"] {
        let ids = by_keyword(query);
        for expected in ["gkeep-list", "gkeep-pull", "gkeep-doctor"] {
            assert!(
                ids.contains(expected),
                "{query} should find {expected}: {ids:?}"
            );
        }
    }
    let pdf = by_keyword("pdf");
    assert!(
        pdf.contains("ref-create"),
        "pdf should find ref-create: {pdf:?}"
    );
    assert!(
        pdf.contains("ref-scan"),
        "pdf should find ref-scan: {pdf:?}"
    );
    for query in ["deploy", "install"] {
        let ids = by_keyword(query);
        assert!(
            ids.contains("plugins-sync"),
            "{query} should find plugins-sync: {ids:?}"
        );
    }
    for query in ["pom", "tomato"] {
        let ids = by_keyword(query);
        for expected in [
            "pomodoro-status",
            "pomodoro-start",
            "pomodoro-stop",
            "pomodoro-extend",
            "pomodoro-shorten",
        ] {
            assert!(
                ids.contains(expected),
                "{query} should find {expected}: {ids:?}"
            );
        }
    }
    let reading = by_keyword("reading");
    assert!(
        reading.contains("ref-list"),
        "reading should find ref-list: {reading:?}"
    );
    assert!(
        reading.contains("ref-queue"),
        "reading should find ref-queue: {reading:?}"
    );
    let clip = by_keyword("clip");
    assert!(
        clip.contains("ref-create"),
        "clip should find ref-create: {clip:?}"
    );
    let alias = catalog
        .aliases
        .iter()
        .find(|alias| {
            alias.from.last().map(String::as_str) == Some("task-status-hooks")
        })
        .expect("task-status-hooks alias");
    assert_eq!(alias.to, vec!["task".to_string(), "reconcile".to_string()]);
    assert!(
        catalog.entries.iter().any(|entry| entry.id == "task-reconcile"
            && entry.path == alias.to),
        "task-status-hooks should resolve to task-reconcile"
    );
}

#[test]
fn document_shape() {
    let catalog = document();
    assert_eq!(catalog.schema_version, 1);
    assert!(catalog.ok);
    assert_eq!(catalog.protocol, 1);
    assert_eq!(catalog.bob_version, env!("CARGO_PKG_VERSION"));
    assert!(catalog.features.is_empty());
    assert_eq!(
        catalog
            .sections
            .iter()
            .map(|section| section.id.as_str())
            .collect::<Vec<_>>(),
        vec!["daily", "tasks", "vault", "integrations", "setup"]
    );
    assert_eq!(catalog.entries.len(), 52);
    assert_eq!(ENTRIES.len(), 52);
    assert!(catalog.entries.iter().any(|entry| entry.id == "plan"));
    assert!(catalog
        .entries
        .iter()
        .any(|entry| entry.id == "pomodoro-start"));
    assert!(catalog.excluded.iter().any(|row| row.path == ["capture"]));
}

#[test]
fn script_backed_leaves_resolve() {
    let leaves = public_leaves();
    for path in [
        &["task", "archive"][..],
        &["nightly"][..],
        &["pomodoro", "status"][..],
    ] {
        assert!(
            find_leaf(&leaves, path).is_some(),
            "script-backed path {path:?} must resolve through tree()"
        );
    }
}
