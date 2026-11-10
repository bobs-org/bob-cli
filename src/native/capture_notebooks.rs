//! Invocation-local notebook route metadata for the capture grammar.

use std::{
    collections::{HashMap, HashSet},
    fs,
    path::{Path, PathBuf},
};

use super::{
    capture_language::{
        candidate_notebook_route, candidate_notebook_routes, NotebookRouteInfo,
        NotebookRoutePolicy,
    },
    notebooks::scan::{derive_title, first_alias},
    projects::{
        frontmatter_is_area, frontmatter_is_notebook, frontmatter_is_project,
        frontmatter_value, parse_frontmatter, wikilink_path_target,
    },
    vault_links::{LinkResolution, VaultLinkResolver},
};

/// Build one memoized lookup for the routes mentioned in a draft. The
/// lexical preflight means an ordinary unrouted draft opens no notes; the
/// cache means a repeated route or shared parent chain reads each note once.
pub(crate) fn notebook_policy_for_draft(
    vault_root: &Path,
    raw_text: &str,
) -> NotebookRoutePolicy {
    build_notebook_policy(vault_root, candidate_notebook_routes(raw_text)).0
}

/// Resolve a forced capture destination without interpreting any `@` text
/// from the draft, which remains literal under `--route`.
pub(crate) fn notebook_policy_for_forced_route(
    vault_root: &Path,
    forced_route: &str,
) -> NotebookRoutePolicy {
    let candidates =
        candidate_notebook_route(forced_route).into_iter().collect();
    build_notebook_policy(vault_root, candidates).0
}

fn build_notebook_policy(
    vault_root: &Path,
    candidates: Vec<String>,
) -> (NotebookRoutePolicy, Vec<PathBuf>) {
    if candidates.is_empty() {
        return (NotebookRoutePolicy::absent(), Vec::new());
    }

    let links = VaultLinkResolver::new(vault_root);
    let mut cache: HashMap<PathBuf, Option<String>> = HashMap::new();
    let mut reads = Vec::new();
    let mut policy = NotebookRoutePolicy::absent();
    for route in candidates {
        let path = PathBuf::from(format!("{route}.md"));
        if !vault_root.join(&path).is_file() {
            continue;
        }
        let Some(contents) =
            read_cached(vault_root, &path, &mut cache, &mut reads)
        else {
            continue;
        };
        let Some(frontmatter) = parse_frontmatter(&contents) else {
            continue;
        };
        if !frontmatter_is_notebook(&frontmatter) {
            continue;
        }
        let info = route_info(
            vault_root, &path, &contents, &links, &mut cache, &mut reads,
        );
        policy.insert_with_source(route, info, contents);
    }
    (policy, reads)
}

fn read_cached(
    vault_root: &Path,
    relative: &Path,
    cache: &mut HashMap<PathBuf, Option<String>>,
    reads: &mut Vec<PathBuf>,
) -> Option<String> {
    if !cache.contains_key(relative) {
        reads.push(relative.to_path_buf());
        cache.insert(
            relative.to_path_buf(),
            fs::read_to_string(vault_root.join(relative)).ok(),
        );
    }
    cache.get(relative).and_then(Clone::clone)
}

fn route_info(
    vault_root: &Path,
    candidate: &Path,
    contents: &str,
    links: &VaultLinkResolver,
    cache: &mut HashMap<PathBuf, Option<String>>,
    reads: &mut Vec<PathBuf>,
) -> NotebookRouteInfo {
    let candidate_stem = stem(candidate);
    let mut chain: Vec<(String, String)> = Vec::new();
    let mut current = candidate.to_path_buf();
    let mut owner = None;
    let mut visited = HashSet::new();
    visited.insert(current.clone());
    let mut current_contents = Some(contents.to_string());

    for _ in 0..16 {
        let Some(note) = current_contents
            .take()
            .or_else(|| read_cached(vault_root, &current, cache, reads))
        else {
            break;
        };
        let Some(frontmatter) = parse_frontmatter(&note) else {
            break;
        };
        if !frontmatter_is_notebook(&frontmatter) {
            if frontmatter_is_area(&frontmatter)
                || frontmatter_is_project(&frontmatter)
            {
                owner = Some(stem(&current));
            }
            break;
        }

        let title =
            derive_title(&note, first_alias(&note).as_deref(), &stem(&current));
        chain.push((stem(&current), title));

        let Some(parent) = frontmatter_value(&frontmatter, "parent")
            .and_then(wikilink_path_target)
        else {
            break;
        };
        let LinkResolution::Found(parent_path) =
            links.resolve(&current, &parent)
        else {
            break;
        };
        if !visited.insert(parent_path.clone()) {
            break;
        }
        current = parent_path;
    }

    let breadcrumb = if let Some(owner) = owner.as_ref() {
        std::iter::once(owner.clone())
            .chain(chain.iter().rev().map(|(_, title)| title.clone()))
            .collect()
    } else {
        chain
            .last()
            .map(|(_, title)| vec![title.clone()])
            .unwrap_or_else(|| vec![candidate_stem.clone()])
    };

    let summary = parse_frontmatter(contents).and_then(|frontmatter| {
        frontmatter_value(&frontmatter, "summary")
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
    });
    NotebookRouteInfo {
        title: chain
            .first()
            .map(|(_, title)| title.clone())
            .unwrap_or_else(|| candidate_stem),
        summary,
        owner,
        breadcrumb,
    }
}

fn stem(path: &Path) -> String {
    path.file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or_default()
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::native::capture_language::{
        parse_capture_text_with_options, CaptureKind, CaptureParseOptions,
    };
    use std::fs;

    fn write(root: &Path, path: &str, body: &str) {
        let path = root.join(path);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, body).unwrap();
    }

    #[test]
    fn route_metadata_uses_notebook_title_owner_and_breadcrumb() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        write(root, "dev.md", "---\ntype: \"[[area]]\"\n---\n");
        write(
            root,
            "dev_tools.md",
            "---\nparent: \"[[dev]]\"\ntype: \"[[notebook]]\"\nsummary: Tools.\n---\n\n# **Developer** [[tools]]\n",
        );
        write(
            root,
            "git.md",
            "---\nparent: \"[[dev_tools]]\"\ntype: \"[[notebook]]\"\nsummary: Git.\n---\n\n# Git\n",
        );
        let (policy, reads) = build_notebook_policy(
            root,
            candidate_notebook_routes("Try @git#config and @git^ideas"),
        );
        let info = policy.get("git").unwrap();
        assert_eq!(info.title, "Git");
        assert_eq!(info.owner.as_deref(), Some("dev"));
        assert_eq!(info.breadcrumb, ["dev", "Developer tools", "Git"]);
        assert_eq!(
            reads
                .iter()
                .filter(|path| path.as_path() == Path::new("git.md"))
                .count(),
            1
        );
    }

    #[test]
    fn every_notebook_frontmatter_type_shape_is_recognized() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        write(root, "dev.md", "---\ntype: \"[[area]]\"\n---\n");
        for (stem, type_line) in [
            ("scalar", "type: \"[[notebook]]\""),
            ("flow", "type: [\"[[notebook]]\"]"),
            ("block", "type:\n  - \"[[notebook]]\""),
            ("bare", "type: [[notebook]]"),
        ] {
            write(
                root,
                &format!("{stem}.md"),
                &format!(
                    "---\nparent: \"[[dev]]\"\n{type_line}\n---\n\n# {stem}\n"
                ),
            );
        }
        let (policy, _) = build_notebook_policy(
            root,
            candidate_notebook_routes(
                "@scalar note @flow note @block note @bare note",
            ),
        );
        for route in ["scalar", "flow", "block", "bare"] {
            assert!(policy.get(route).is_some(), "{route}");
        }
    }

    #[test]
    fn unrouted_draft_reads_no_files_and_repeated_candidates_read_once() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        write(root, "dev.md", "---\ntype: \"[[area]]\"\n---\n");
        write(
            root,
            "notes.md",
            "---\nparent: \"[[dev]]\"\ntype: \"[[notebook]]\"\n---\n\n# Notes\n",
        );
        let (_, no_reads) = build_notebook_policy(
            root,
            candidate_notebook_routes("Unrouted prose"),
        );
        assert!(no_reads.is_empty());
        let (_, reads) = build_notebook_policy(
            root,
            candidate_notebook_routes("@notes one\n@notes two\nlast @notes"),
        );
        assert_eq!(
            reads
                .iter()
                .filter(|path| path.as_path() == Path::new("notes.md"))
                .count(),
            1
        );
    }

    #[test]
    fn ordinary_routes_are_not_claimed_as_notebooks() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        write(root, "dev.md", "---\ntype: \"[[area]]\"\n---\n");
        let policy = notebook_policy_for_draft(root, "Try this @dev#Ideas");
        assert!(policy.get("dev").is_none());

        let options = CaptureParseOptions {
            notebook_policy: Some(&policy),
            ..CaptureParseOptions::routing_off()
        };
        let parsed = parse_capture_text_with_options(
            "Try this @dev#Ideas",
            None,
            None,
            &options,
        )
        .unwrap();
        assert!(matches!(
            parsed.kind,
            CaptureKind::Bullet {
                section_prefix: Some(ref section),
                exact: false,
                block_id: None,
            } if section == "Ideas"
        ));
        assert_eq!(parsed.route.as_deref(), Some("dev"));
        assert_eq!(parsed.body, "Try this");
    }

    #[test]
    fn forced_destination_is_indexed_without_reading_draft_routes() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        write(
            root,
            "notes.md",
            "---\ntype: \"[[notebook]]\"\n---\n\n# Notes\n",
        );
        let policy = notebook_policy_for_forced_route(root, "Notes");
        assert!(policy.get("notes").is_some());
    }
}
