//! Vault-wide inbound heading-link repair for article notes.

use std::{
    collections::BTreeSet,
    fs, io,
    path::{Path, PathBuf},
};

use super::findings::Finding;
use super::links::{self, InboundArticle};
use super::plan::{sha256_hex, ArticlePlan, ArticleStatus, InboundLink};
use super::scan;
use super::vault::{self, VaultError};
use crate::native::{collect_done, vault_links::NoteIndex};

/// Fill each eligible plan's `inbound` rewrites and `inbound-heading-link`
/// findings. `always` is `--all` or `--stdin`: scan even when this run's
/// headings did not change, so stale links from earlier runs are repaired.
pub(crate) fn attach(
    root: &Path,
    plans: &mut [ArticlePlan],
    always: bool,
) -> Result<NoteIndex, VaultError> {
    let paths = vault::list_markdown(root)?;
    let index = NoteIndex::from_paths(paths.iter().cloned());
    let active = active_indices(plans, always);
    if active.is_empty() {
        return Ok(index);
    }

    let owned = owned_targets_for(plans, &active);
    let stems = active
        .iter()
        .copied()
        .zip(owned.iter())
        .filter_map(|(plan_index, target)| {
            let stem = stem_of(&target.path);
            (!stem.is_empty()).then_some((plan_index, stem.to_lowercase()))
        })
        .collect::<Vec<_>>();

    let mut hits = Vec::new();
    for path in &paths {
        let absolute = root.join(path);
        let text = match fs::read_to_string(&absolute) {
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
        let lowered = text.to_lowercase();
        let matching = stems
            .iter()
            .filter(|(_, stem)| lowered.contains(stem.as_str()))
            .map(|(plan_index, _)| *plan_index)
            .collect::<Vec<_>>();
        if matching.is_empty() {
            continue;
        }
        hits.extend(scan_file(path, &text, &matching, &owned, &active, &index));
    }

    for hit in hits {
        let plan = &mut plans[hit.article];
        plan.findings.extend(hit.findings);
        plan.inbound.extend(hit.inbound);
    }
    for plan in plans.iter_mut() {
        plan.findings.sort_by_key(|finding| finding.line);
        plan.inbound.sort_by(|left, right| {
            left.path.cmp(&right.path).then(left.line.cmp(&right.line))
        });
    }
    Ok(index)
}

fn target_index(active: &[usize], plan_index: usize) -> Option<usize> {
    active.iter().position(|index| *index == plan_index)
}

/// Apply inbound fragment rewrites to article `formatted` text in this
/// batch so `--all` can repair cross-article links in one write.
pub(crate) fn fold_into_formatted(
    plans: &mut [ArticlePlan],
    sources: &[String],
    index: &NoteIndex,
) {
    if plans.iter().all(|plan| plan.inbound.is_empty()) {
        return;
    }
    let owned = owned_targets(plans);
    if owned.is_empty() {
        return;
    }
    for (plan_index, plan) in plans.iter_mut().enumerate() {
        if matches!(plan.status, ArticleStatus::Skipped | ArticleStatus::Error)
        {
            continue;
        }
        let Some(formatted) = plan.formatted.clone() else {
            continue;
        };
        let articles = inbound_articles(&owned);
        let rewritten =
            rewrite_document(&formatted, &plan.path, index, &articles);
        if rewritten == formatted {
            continue;
        }
        let source = sources.get(plan_index).map(String::as_str).unwrap_or("");
        let original = scan::classify(source);
        let output = scan::classify(&rewritten);
        let original_lines: Vec<&str> =
            original.lines.iter().map(|line| line.content).collect();
        let output_lines: Vec<&str> =
            output.lines.iter().map(|line| line.content).collect();
        plan.edits = super::plan::line_edits(&original_lines, &output_lines);
        plan.formatted = Some(rewritten);
        if plan.status == ArticleStatus::Unchanged {
            plan.status = ArticleStatus::Changed;
        }
    }
}

/// Write inbound rewrites for notes this run did not already install as
/// changed articles. A linking file whose preimage drifted is skipped.
pub(crate) fn write_others(
    root: &Path,
    plans: &[ArticlePlan],
    index: &NoteIndex,
) -> Vec<String> {
    let owned = owned_targets(plans);
    if owned.is_empty() {
        return Vec::new();
    }
    let written = plans
        .iter()
        .filter(|plan| plan.status == ArticleStatus::Changed)
        .map(|plan| plan.path.clone())
        .collect::<BTreeSet<_>>();
    let mut skips = Vec::new();
    let mut seen = BTreeSet::new();
    for plan in plans {
        if matches!(plan.status, ArticleStatus::Skipped | ArticleStatus::Error)
        {
            continue;
        }
        for inbound in &plan.inbound {
            if written.contains(&inbound.path)
                || !seen.insert(inbound.path.clone())
            {
                continue;
            }
            let absolute = root.join(&inbound.path);
            let current = match fs::read_to_string(&absolute) {
                Ok(text) => text,
                Err(error) => {
                    skips.push(format!(
                        "skipped inbound {}: {error}",
                        inbound.path.display()
                    ));
                    continue;
                }
            };
            if sha256_hex(&current) != inbound.sha256 {
                skips.push(format!(
                    "skipped inbound {}: changed since the article plan was made",
                    inbound.path.display()
                ));
                continue;
            }
            let articles = inbound_articles(&owned);
            let rewritten =
                rewrite_document(&current, &inbound.path, index, &articles);
            if rewritten == current {
                continue;
            }
            if let Err(error) =
                collect_done::atomic_write(&absolute, &rewritten)
            {
                skips.push(format!(
                    "skipped inbound {}: {error}",
                    inbound.path.display()
                ));
            }
        }
    }
    skips
}

/// Combined rewritten text of a linking note, for dry-run diffs.
pub(crate) fn rewritten_file(
    text: &str,
    path: &Path,
    plans: &[ArticlePlan],
    index: &NoteIndex,
) -> String {
    let owned = owned_targets(plans);
    let articles = inbound_articles(&owned);
    rewrite_document(text, path, index, &articles)
}

struct ScanHit {
    article: usize,
    inbound: Vec<InboundLink>,
    findings: Vec<Finding>,
}

fn scan_file(
    path: &Path,
    text: &str,
    matching: &[usize],
    owned: &[OwnedTarget],
    active: &[usize],
    index: &NoteIndex,
) -> Vec<ScanHit> {
    let hash = sha256_hex(text);
    let document = scan::classify(text);
    let mut hits = Vec::new();
    for &plan_index in matching {
        let Some(owned_index) = target_index(active, plan_index) else {
            continue;
        };
        let target = &owned[owned_index];
        if target.path == path {
            continue;
        }
        let articles = [InboundArticle {
            index: plan_index,
            path: &target.path,
            headings: &target.headings,
            numbering: target.numbering,
        }];
        let mut inbound = Vec::new();
        let mut findings = Vec::new();
        for line in &document.lines {
            let result = links::rewrite_inbound_line(
                line.content,
                line.region,
                path,
                line.index + 1,
                index,
                &articles,
            );
            for (_, mut finding) in result.findings {
                finding.line = 1;
                findings.push(finding);
            }
            let Some(updated) = result.updated else {
                continue;
            };
            inbound.push(InboundLink {
                path: path.to_path_buf(),
                line: line.index + 1,
                sha256: hash.clone(),
                before: line.content.to_string(),
                after: updated,
            });
        }
        if !inbound.is_empty() || !findings.is_empty() {
            hits.push(ScanHit {
                article: plan_index,
                inbound,
                findings,
            });
        }
    }
    hits
}

fn active_indices(plans: &[ArticlePlan], always: bool) -> Vec<usize> {
    plans
        .iter()
        .enumerate()
        .filter(|(_, plan)| {
            !matches!(
                plan.status,
                ArticleStatus::Skipped | ArticleStatus::Error
            ) && !plan.headings.is_empty()
                && (always || plan.headings_text_changed())
        })
        .map(|(index, _)| index)
        .collect()
}

struct OwnedTarget {
    path: PathBuf,
    headings: Vec<super::sections::PlannedHeading>,
    numbering: bool,
}

fn owned_targets(plans: &[ArticlePlan]) -> Vec<OwnedTarget> {
    plans
        .iter()
        .filter(|plan| {
            !matches!(
                plan.status,
                ArticleStatus::Skipped | ArticleStatus::Error
            ) && !plan.headings.is_empty()
        })
        .map(|plan| OwnedTarget {
            path: plan.path.clone(),
            headings: plan.headings.clone(),
            numbering: plan.numbering,
        })
        .collect()
}

fn owned_targets_for(
    plans: &[ArticlePlan],
    active: &[usize],
) -> Vec<OwnedTarget> {
    active
        .iter()
        .map(|&index| OwnedTarget {
            path: plans[index].path.clone(),
            headings: plans[index].headings.clone(),
            numbering: plans[index].numbering,
        })
        .collect()
}

fn inbound_articles(owned: &[OwnedTarget]) -> Vec<InboundArticle<'_>> {
    owned
        .iter()
        .enumerate()
        .map(|(index, target)| InboundArticle {
            index,
            path: &target.path,
            headings: &target.headings,
            numbering: target.numbering,
        })
        .collect()
}

fn rewrite_document(
    text: &str,
    current_path: &Path,
    index: &NoteIndex,
    articles: &[InboundArticle<'_>],
) -> String {
    let document = scan::classify(text);
    let mut replacements = vec![None; document.lines.len()];
    for line in &document.lines {
        let result = links::rewrite_inbound_line(
            line.content,
            line.region,
            current_path,
            line.index + 1,
            index,
            articles,
        );
        if let Some(updated) = result.updated {
            replacements[line.index] = Some(updated);
        }
    }
    join_replacements(text, &document, &replacements)
}

fn join_replacements(
    text: &str,
    document: &scan::Document<'_>,
    replacements: &[Option<String>],
) -> String {
    let mut out = String::with_capacity(text.len());
    if document.bom {
        out.push('\u{feff}');
    }
    for line in &document.lines {
        out.push_str(
            replacements[line.index].as_deref().unwrap_or(line.content),
        );
        out.push_str(line.ending);
    }
    out
}

fn stem_of(path: &Path) -> String {
    path.file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or("")
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::native::articles::{format, ArticleContext};
    use std::fs;

    fn article_text() -> &'static str {
        "---\ntype: \"[[article]]\"\n---\n\n# Draft\n\n## 9. First\n\nTODO\n"
    }

    fn vault() -> (tempfile::TempDir, PathBuf) {
        let temp = tempfile::tempdir().expect("temporary vault");
        let root = temp.path().to_path_buf();
        fs::write(root.join("draft.md"), article_text()).expect("article");
        (temp, root)
    }

    #[test]
    fn attach_rewrites_two_notes_and_reports_an_unrepairable_link() {
        let (_temp, root) = vault();
        fs::write(
            root.join("journal.md"),
            "See [[draft#9. First]] and [[draft#Gone]].\n",
        )
        .expect("journal");
        fs::write(root.join("log.md"), "Also [[draft#First]].\n").expect("log");

        let source = fs::read_to_string(root.join("draft.md")).unwrap();
        let mut plans =
            vec![format(&source, &ArticleContext::new(Path::new("draft.md")))];
        attach(&root, &mut plans, false).expect("scan inbound");

        let inbound = &plans[0].inbound;
        assert_eq!(inbound.len(), 2, "{inbound:?}");
        let journal = inbound
            .iter()
            .find(|item| item.path == Path::new("journal.md"))
            .expect("journal rewrite");
        assert!(journal.after.contains("[[draft#1.1 First]]"), "{journal:?}");
        assert!(journal.before.contains("[[draft#9. First]]"));
        let log = inbound
            .iter()
            .find(|item| item.path == Path::new("log.md"))
            .expect("log rewrite");
        assert!(log.after.contains("[[draft#1.1 First]]"), "{log:?}");
        assert!(plans[0].findings.iter().any(|finding| {
            finding.code == "inbound-heading-link"
                && finding.message.contains("journal.md:1")
                && finding.message.contains("no longer has")
        }));
    }

    #[test]
    fn duplicate_titles_use_preformat_text_and_report_ambiguous_identity() {
        let temp = tempfile::tempdir().expect("temporary vault");
        let root = temp.path();
        fs::write(
            root.join("note.md"),
            "---\ntype: \"[[article]]\"\n---\n\n## 9. Examples\n\n## 8. Examples\n",
        )
        .expect("article");
        fs::write(root.join("a.md"), "[[note#9. Examples]]\n").expect("exact");
        fs::write(root.join("b.md"), "[[note#Examples]]\n").expect("identity");

        let source = fs::read_to_string(root.join("note.md")).unwrap();
        let mut plans =
            vec![format(&source, &ArticleContext::new(Path::new("note.md")))];
        attach(root, &mut plans, false).expect("scan inbound");

        let exact = plans[0]
            .inbound
            .iter()
            .find(|item| item.path == Path::new("a.md"))
            .expect("exact rewrite");
        assert!(exact.after.contains("[[note#1. Examples]]"), "{exact:?}");
        assert!(plans[0].findings.iter().any(|finding| {
            finding.code == "inbound-heading-link"
                && finding.message.contains("b.md:1")
                && finding.message.contains("multiple sections")
        }));
    }

    #[test]
    fn write_skips_a_linking_note_whose_preimage_changed() {
        let (_temp, root) = vault();
        fs::write(root.join("journal.md"), "See [[draft#9. First]].\n")
            .expect("journal");
        let source = fs::read_to_string(root.join("draft.md")).unwrap();
        let mut plans =
            vec![format(&source, &ArticleContext::new(Path::new("draft.md")))];
        let index = attach(&root, &mut plans, false).expect("scan inbound");
        fs::write(root.join("journal.md"), "changed in the meantime\n")
            .expect("race");

        let mut plans = plans;
        vault::write_plans(&root, &mut plans).expect("write article");
        let skips = write_others(&root, &plans, &index);

        assert!(
            skips.iter().any(|skip| skip.contains("journal.md")
                && skip.contains("changed since the article plan was made")),
            "{skips:?}"
        );
        assert_eq!(
            fs::read_to_string(root.join("journal.md")).unwrap(),
            "changed in the meantime\n"
        );
        assert!(fs::read_to_string(root.join("draft.md"))
            .unwrap()
            .contains("## 1.1 First"));
    }

    #[test]
    fn layout_only_format_skips_inbound_unless_forced() {
        let temp = tempfile::tempdir().expect("temporary vault");
        let root = temp.path();
        let formatted = "---\ntype: \"[[article]]\"\n---\n\n# 1. Draft\n\n## 1.1 First\n\nTODO\n";
        fs::write(root.join("draft.md"), formatted).expect("article");
        fs::write(root.join("journal.md"), "See [[draft#1. First]].\n")
            .expect("stale");

        let mut plans = vec![format(
            formatted,
            &ArticleContext::new(Path::new("draft.md")),
        )];
        assert_eq!(plans[0].status, ArticleStatus::Unchanged);
        attach(root, &mut plans, false).expect("no scan");
        assert!(plans[0].inbound.is_empty());

        attach(root, &mut plans, true).expect("forced scan");
        assert_eq!(plans[0].inbound.len(), 1);
        assert!(plans[0].inbound[0].after.contains("[[draft#1.1 First]]"));
    }
}
