//! Article note discovery, path resolution, and guarded vault writes.

use std::{
    ffi::OsStr,
    fs, io,
    path::{Path, PathBuf},
};

use super::{format, plan::ArticlePlan, ArticleContext};
use crate::native::{
    collect_done, is_always_excluded_note_directory_name,
    projects::{frontmatter_is_article, is_markdown_file, parse_frontmatter},
    vault_links::{target_to_markdown_path, LinkResolution, VaultLinkResolver},
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ArticleFile {
    pub(crate) path: PathBuf,
    pub(crate) text: String,
}

#[derive(Debug)]
pub(crate) struct VaultError {
    pub(crate) path: PathBuf,
    pub(crate) action: &'static str,
    pub(crate) source: io::Error,
}

impl std::fmt::Display for VaultError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "{} {}: {}",
            self.action,
            self.path.display(),
            self.source
        )
    }
}

/// Vault-relative Markdown paths, skipping always-excluded directories.
pub(crate) fn list_markdown(root: &Path) -> Result<Vec<PathBuf>, VaultError> {
    let mut paths = Vec::new();
    walk_markdown(root, root, &mut paths)?;
    paths.sort();
    Ok(paths)
}

/// Discover marked article notes under the vault root. The directory
/// exclusions match article formatting's refusal list.
pub(crate) fn discover_articles(
    root: &Path,
) -> Result<Vec<ArticleFile>, VaultError> {
    let paths = list_markdown(root)?;
    let mut articles = Vec::new();
    for path in paths {
        let text = match fs::read_to_string(root.join(&path)) {
            Ok(text) => text,
            Err(error) if error.kind() == io::ErrorKind::InvalidData => {
                continue;
            }
            Err(source) => {
                return Err(VaultError {
                    path: path.clone(),
                    action: "read",
                    source,
                });
            }
        };
        if text_is_article(&text) {
            articles.push(ArticleFile { path, text });
        }
    }
    Ok(articles)
}

/// Resolve an article NOTE as an exact vault-relative path, or as a
/// unique case-insensitive basename. Exact excluded paths are returned
/// so the formatter can explain its refusal.
pub(crate) fn resolve_note(root: &Path, note: &str) -> Result<PathBuf, String> {
    if let Some(relative) = target_to_markdown_path(note)
        && root.join(&relative).is_file()
    {
        return Ok(relative);
    }

    let resolver = VaultLinkResolver::new(root);
    match resolver.resolve(Path::new(""), note) {
        LinkResolution::Found(path) => Ok(path),
        LinkResolution::Ambiguous => {
            let candidates = resolver
                .candidates(note)
                .iter()
                .map(|path| path.display().to_string())
                .collect::<Vec<_>>();
            Err(format!(
                "{note:?} is ambiguous; candidates: {}",
                candidates.join(", ")
            ))
        }
        LinkResolution::Missing => {
            let near = resolver.near_stems(note, 3);
            let suggestion = if near.is_empty() {
                String::new()
            } else {
                format!("; near matches: {}", near.join(", "))
            };
            Err(format!(
                "{note:?} does not resolve to one vault Markdown note{suggestion}"
            ))
        }
    }
}

/// Validate every planned source preimage before writing any result, then
/// install each changed note with the shared atomic writer.
pub(crate) fn preflight_plans(
    root: &Path,
    plans: &mut [ArticlePlan],
) -> Result<(), String> {
    for plan in plans {
        if !matches!(plan.status, super::plan::ArticleStatus::Changed) {
            continue;
        }
        let absolute = root.join(&plan.path);
        let bytes = match fs::read(&absolute) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                mark_preimage_changed(plan);
                continue;
            }
            Err(error) => {
                return Err(format!("read {}: {error}", plan.path.display()));
            }
        };
        if sha256_bytes(&bytes) != plan.sha256 {
            mark_preimage_changed(plan);
        }
    }
    Ok(())
}

pub(crate) fn write_plans(
    root: &Path,
    plans: &mut [ArticlePlan],
) -> Result<(), String> {
    preflight_plans(root, plans)?;
    let mut writes = Vec::new();
    for plan in plans {
        if !matches!(plan.status, super::plan::ArticleStatus::Changed) {
            continue;
        }
        let absolute = root.join(&plan.path);
        let formatted = plan.formatted.as_ref().ok_or_else(|| {
            format!("{} has no formatted output", plan.path.display())
        })?;
        writes.push((absolute, plan.path.clone(), formatted.clone()));
    }

    for (absolute, relative, formatted) in writes {
        collect_done::atomic_write(&absolute, &formatted).map_err(|error| {
            format!("write {}: {error}", relative.display())
        })?;
    }
    Ok(())
}

fn mark_preimage_changed(plan: &mut ArticlePlan) {
    let message = format!(
        "{} changed since the article plan was made; not written",
        plan.path.display()
    );
    plan.status = super::plan::ArticleStatus::Error;
    plan.reason = Some(super::plan::Refusal {
        code: "preimage-changed".to_string(),
        message: message.clone(),
    });
    plan.notice = format!("Article formatting failed · {message}");
    plan.formatted = None;
    plan.inbound.clear();
}

fn sha256_bytes(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    hex::encode(Sha256::digest(bytes))
}

/// Completion values are extensionless vault-relative paths paired with
/// their current section counts.
pub(crate) fn completion_candidates(root: &Path) -> Vec<(String, String)> {
    let Ok(articles) = discover_articles(root) else {
        return Vec::new();
    };
    articles
        .iter()
        .map(|article| {
            let plan =
                format(&article.text, &ArticleContext::new(&article.path));
            let value = article
                .path
                .with_extension("")
                .to_string_lossy()
                .replace('\\', "/");
            let sections = plan.outline.sections.len();
            (value, format!("article · {sections} sections"))
        })
        .collect()
}

fn walk_markdown(
    root: &Path,
    directory: &Path,
    paths: &mut Vec<PathBuf>,
) -> Result<(), VaultError> {
    let entries = fs::read_dir(directory).map_err(|source| VaultError {
        path: relative_or_root(root, directory),
        action: "read directory",
        source,
    })?;
    let mut entries =
        entries.collect::<Result<Vec<_>, _>>().map_err(|source| {
            VaultError {
                path: relative_or_root(root, directory),
                action: "read directory entry in",
                source,
            }
        })?;
    entries.sort_by_key(|entry| entry.file_name());

    for entry in entries {
        let path = entry.path();
        let file_type = entry.file_type().map_err(|source| VaultError {
            path: relative_or_root(root, &path),
            action: "inspect",
            source,
        })?;
        if file_type.is_dir() {
            let name = entry.file_name();
            if is_always_excluded_note_directory_name(&name) {
                continue;
            }
            walk_markdown(root, &path, paths)?;
        } else if file_type.is_file() && is_markdown_file_or_uppercase(&path) {
            let relative = path.strip_prefix(root).map_err(|_| VaultError {
                path: path.clone(),
                action: "resolve vault-relative path for",
                source: io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "note is outside the vault root",
                ),
            })?;
            paths.push(relative.to_path_buf());
        }
    }
    Ok(())
}

fn is_markdown_file_or_uppercase(path: &Path) -> bool {
    is_markdown_file(path)
        || path
            .extension()
            .and_then(OsStr::to_str)
            .is_some_and(|extension| extension.eq_ignore_ascii_case("md"))
}

fn relative_or_root(root: &Path, path: &Path) -> PathBuf {
    path.strip_prefix(root)
        .map(Path::to_path_buf)
        .unwrap_or_else(|_| path.to_path_buf())
}

fn text_is_article(text: &str) -> bool {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    if let Some(frontmatter) = parse_frontmatter(text) {
        return frontmatter_is_article(&frontmatter);
    }

    // Keep an article with an unclosed frontmatter marker in the `--all`
    // plan so the formatter can report `frontmatter-unclosed`.
    if text
        .lines()
        .next()
        .is_some_and(|line| line.trim_end() == "---")
    {
        let mut closed = text.trim_end_matches(['\n', '\r']).to_owned();
        closed.push_str("\n---\n");
        return parse_frontmatter(&closed)
            .is_some_and(|frontmatter| frontmatter_is_article(&frontmatter));
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn write_refuses_a_changed_preimage_before_replacing_it() {
        let root = tempfile::tempdir().expect("temporary vault");
        let path = PathBuf::from("draft.md");
        let source = "---\ntype: [[article]]\n---\n\n## Old\n";
        fs::write(root.path().join(&path), source).expect("write source");
        let plan = format(source, &ArticleContext::new(&path));
        let changed = "newer source\n";
        fs::write(root.path().join(&path), changed).expect("change source");

        let mut plans = vec![plan];
        write_plans(root.path(), &mut plans)
            .expect("stale source is reported per note");
        assert_eq!(plans[0].status, super::super::plan::ArticleStatus::Error);
        assert_eq!(
            plans[0].reason.as_ref().map(|reason| reason.code.as_str()),
            Some("preimage-changed")
        );
        assert_eq!(
            fs::read_to_string(root.path().join(&path)).unwrap(),
            changed
        );
    }
}
