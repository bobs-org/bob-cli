//! Heading-link repair for renamed project sections.
//!
//! A `^prj` section rename changes a heading, so every `[[note#Old]]`,
//! `![[note#Old]]`, and Markdown `note.md#Old` link that resolves to a
//! renamed note is rewritten to the NEW section title. Block links
//! (`#^id`) never change, and neither do links inside code. A link whose
//! note cannot be resolved, or whose note was refused, is reported and left
//! as written.

use std::{
    collections::BTreeSet,
    path::{Component, Path, PathBuf},
};

use super::RenameSpec;
use crate::native::{
    capture::list_item_body,
    capture_drawers::drawer_key,
    markdown,
    task_dependencies::inline_code_spans,
    vault_links::{LinkResolution, NoteIndex},
};

/// One rewritten link target.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LinkRepair {
    /// 1-based line.
    pub(crate) line: usize,
    pub(crate) before: String,
    pub(crate) after: String,
}

/// A link to an OLD heading the pass could not repair.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct UnresolvedLink {
    /// 1-based line.
    pub(crate) line: usize,
    pub(crate) link: String,
    pub(crate) reason: &'static str,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct LinkPass {
    pub(crate) updated: Option<String>,
    pub(crate) repairs: Vec<LinkRepair>,
    pub(crate) unresolved: Vec<UnresolvedLink>,
}

/// The notes a heading link may point into.
pub(crate) struct LinkTargets<'a> {
    pub(crate) index: &'a NoteIndex,
    /// Project notes whose sections this run renames.
    pub(crate) renamed: &'a BTreeSet<PathBuf>,
    /// Notes this run refused.
    pub(crate) refused: &'a BTreeSet<PathBuf>,
}

enum Outcome {
    Skip,
    Repair(String),
    Unresolved(&'static str),
}

/// Repair every heading link in `contents` (the note at vault-relative
/// `current`) that points at a renamed OLD section.
pub(crate) fn repair_heading_links(
    contents: &str,
    current: &Path,
    targets: &LinkTargets<'_>,
    spec: &RenameSpec,
) -> LinkPass {
    let mut pass = LinkPass::default();
    if !spec.may_match(contents) || !contents.contains('#') {
        return pass;
    }
    let segments: Vec<&str> = contents.split_inclusive('\n').collect();
    let texts: Vec<&str> = segments
        .iter()
        .map(|segment| markdown::split_line_ending(segment).0)
        .collect();
    let logical: Vec<&str> = texts
        .iter()
        .map(|text| list_item_body(text).unwrap_or(text))
        .collect();
    let fenced = markdown::fenced_lines(&logical, 0..logical.len());
    let mut out = String::with_capacity(contents.len());
    for (index, segment) in segments.iter().enumerate() {
        let text = texts[index];
        if fenced.contains(&index) || !text.contains('#') {
            out.push_str(segment);
            continue;
        }
        let rewritten =
            repair_line(text, index + 1, current, targets, spec, &mut pass);
        out.push_str(&rewritten);
        out.push_str(&segment[text.len()..]);
    }
    if out != contents {
        pass.updated = Some(out);
    }
    pass
}

fn repair_line(
    text: &str,
    line: usize,
    current: &Path,
    targets: &LinkTargets<'_>,
    spec: &RenameSpec,
    pass: &mut LinkPass,
) -> String {
    let code = inline_code_spans(text);
    let in_code = |start: usize, end: usize| {
        code.iter().any(|span| start < span.end && end > span.start)
    };
    // (start, end, replacement) over `text`, in order.
    let mut edits: Vec<(usize, usize, String)> = Vec::new();
    let mut wiki_spans: Vec<(usize, usize)> = Vec::new();
    let mut cursor = 0;
    while let Some(relative) = text[cursor..].find("[[") {
        let open = cursor + relative;
        let inner_start = open + 2;
        let Some(close) = text[inner_start..].find("]]") else {
            break;
        };
        let inner_end = inner_start + close;
        wiki_spans.push((open, inner_end + 2));
        cursor = inner_end + 2;
        if in_code(open, inner_end + 2) {
            continue;
        }
        let inner = &text[inner_start..inner_end];
        match wiki_outcome(inner, current, targets, spec) {
            Outcome::Skip => {}
            Outcome::Repair(after) => {
                pass.repairs.push(LinkRepair {
                    line,
                    before: text[open..inner_end + 2].to_string(),
                    after: format!("[[{after}]]"),
                });
                edits.push((inner_start, inner_end, after));
            }
            Outcome::Unresolved(reason) => {
                pass.unresolved.push(UnresolvedLink {
                    line,
                    link: text[open..inner_end + 2].to_string(),
                    reason,
                })
            }
        }
    }
    let mut cursor = 0;
    while let Some(relative) = text[cursor..].find("](") {
        let open = cursor + relative;
        let start = open + 2;
        cursor = start;
        if wiki_spans
            .iter()
            .any(|&(from, to)| open >= from && open < to)
        {
            continue;
        }
        let end = if text[start..].starts_with('<') {
            text[start..].find('>').and_then(|close| {
                text[start + close..]
                    .find(')')
                    .map(|paren| start + close + paren)
            })
        } else {
            text[start..].find(')').map(|close| start + close)
        };
        let Some(end) = end else {
            break;
        };
        cursor = end + 1;
        if in_code(open, end + 1) {
            continue;
        }
        let destination = &text[start..end];
        match markdown_outcome(destination, current, targets, spec) {
            Outcome::Skip => {}
            Outcome::Repair(after) => {
                pass.repairs.push(LinkRepair {
                    line,
                    before: format!("({destination})"),
                    after: format!("({after})"),
                });
                edits.push((start, end, after));
            }
            Outcome::Unresolved(reason) => {
                pass.unresolved.push(UnresolvedLink {
                    line,
                    link: format!("({destination})"),
                    reason,
                })
            }
        }
    }
    if edits.is_empty() {
        return text.to_string();
    }
    edits.sort_by_key(|(start, _, _)| *start);
    let mut out = String::with_capacity(text.len() + 16);
    let mut last = 0;
    for (start, end, replacement) in edits {
        out.push_str(&text[last..start]);
        out.push_str(&replacement);
        last = end;
    }
    out.push_str(&text[last..]);
    out
}

/// Where a link's note target points: the current note when empty.
fn resolve(
    current: &Path,
    target: &str,
    index: &NoteIndex,
) -> Result<PathBuf, &'static str> {
    if target.is_empty() {
        return Ok(current.to_path_buf());
    }
    if let Some(relative) = relative_to(current, target)
        && index.contains(&relative)
    {
        return Ok(relative);
    }
    match index.resolve_detailed(Some(current), target) {
        LinkResolution::Found(path) => Ok(path),
        LinkResolution::Ambiguous => Err("its note name is ambiguous"),
        LinkResolution::Missing => Err("its note does not exist"),
    }
}

/// A `./` or `../` Markdown path resolved against the linking note.
fn relative_to(current: &Path, target: &str) -> Option<PathBuf> {
    if !(target.starts_with("./") || target.starts_with("../")) {
        return None;
    }
    let mut path: Vec<_> = current
        .parent()
        .map(|parent| parent.components().collect())
        .unwrap_or_default();
    for component in Path::new(target).components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                path.pop()?;
            }
            Component::Normal(_) => path.push(component),
            _ => return None,
        }
    }
    let mut resolved: PathBuf = path.into_iter().collect();
    if resolved
        .extension()
        .is_none_or(|extension| !extension.eq_ignore_ascii_case("md"))
    {
        let name = resolved.file_name()?.to_string_lossy().into_owned();
        resolved.set_file_name(format!("{name}.md"));
    }
    Some(resolved)
}

/// Replace every fragment segment keyed OLD; `None` when none is.
fn rewrite_fragment(
    fragment: &str,
    spec: &RenameSpec,
    decode: bool,
    encode: impl Fn(&str) -> String,
) -> Option<String> {
    if fragment.starts_with('^') {
        return None;
    }
    let mut hit = false;
    let segments: Vec<String> = fragment
        .split('#')
        .map(|segment| {
            let text = if decode {
                percent_decode(segment).unwrap_or_else(|| segment.to_string())
            } else {
                segment.to_string()
            };
            if drawer_key(&text) == spec.old_key {
                if spec.case_only() && text == spec.section_title {
                    segment.to_string()
                } else {
                    hit = true;
                    encode(&spec.section_title)
                }
            } else {
                segment.to_string()
            }
        })
        .collect();
    hit.then(|| segments.join("#"))
}

fn wiki_outcome(
    inner: &str,
    current: &Path,
    targets: &LinkTargets<'_>,
    spec: &RenameSpec,
) -> Outcome {
    // `\|` is a table-escaped alias separator: the backslash belongs to it.
    let split = inner.find('|').map(|bar| {
        if bar > 0 && inner.as_bytes()[bar - 1] == b'\\' {
            bar - 1
        } else {
            bar
        }
    });
    let (target, alias) = match split {
        Some(at) => (&inner[..at], &inner[at..]),
        None => (inner, ""),
    };
    let Some((note, fragment)) = target.split_once('#') else {
        return Outcome::Skip;
    };
    let Some(fragment) =
        rewrite_fragment(fragment, spec, false, str::to_string)
    else {
        return Outcome::Skip;
    };
    match resolve(current, note.trim(), targets.index) {
        Ok(path) if targets.renamed.contains(&path) => {
            Outcome::Repair(format!("{note}#{fragment}{alias}"))
        }
        Ok(path) if targets.refused.contains(&path) => {
            Outcome::Unresolved("its note was refused")
        }
        Ok(_) => Outcome::Skip,
        Err(reason) => Outcome::Unresolved(reason),
    }
}

fn markdown_outcome(
    destination: &str,
    current: &Path,
    targets: &LinkTargets<'_>,
    spec: &RenameSpec,
) -> Outcome {
    let (angle, link, rest) = if let Some(inner) = destination.strip_prefix('<')
    {
        let Some(close) = inner.find('>') else {
            return Outcome::Skip;
        };
        (true, &inner[..close], &inner[close..])
    } else {
        let end = destination
            .find(char::is_whitespace)
            .unwrap_or(destination.len());
        (false, &destination[..end], &destination[end..])
    };
    let Some((path, fragment)) = link.split_once('#') else {
        return Outcome::Skip;
    };
    if path.contains(':') {
        return Outcome::Skip;
    }
    let encode = |title: &str| {
        if angle {
            title.to_string()
        } else {
            title
                .replace(' ', "%20")
                .replace('(', "%28")
                .replace(')', "%29")
        }
    };
    let Some(fragment) = rewrite_fragment(fragment, spec, true, encode) else {
        return Outcome::Skip;
    };
    let decoded = percent_decode(path).unwrap_or_else(|| path.to_string());
    match resolve(current, &decoded, targets.index) {
        Ok(found) if targets.renamed.contains(&found) => {
            let link = format!("{path}#{fragment}");
            Outcome::Repair(if angle {
                format!("<{link}{rest}")
            } else {
                format!("{link}{rest}")
            })
        }
        Ok(found) if targets.refused.contains(&found) => {
            Outcome::Unresolved("its note was refused")
        }
        Ok(_) => Outcome::Skip,
        Err(reason) => Outcome::Unresolved(reason),
    }
}

/// `%XX` decoding; `None` for malformed escapes or non-UTF-8 bytes.
fn percent_decode(value: &str) -> Option<String> {
    if !value.contains('%') {
        return Some(value.to_string());
    }
    let bytes = value.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            let hex = value.get(index + 1..index + 3)?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            index += 3;
        } else {
            out.push(bytes[index]);
            index += 1;
        }
    }
    String::from_utf8(out).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec() -> RenameSpec {
        RenameSpec::parse("requirements", "Notes").expect("spec")
    }

    fn index() -> NoteIndex {
        NoteIndex::from_paths(
            [
                "proj.md",
                "other.md",
                "a/dup.md",
                "b/dup.md",
                "work/plan.md",
            ]
            .into_iter()
            .map(PathBuf::from),
        )
    }

    fn pass(contents: &str, current: &str) -> LinkPass {
        let index = index();
        let renamed: BTreeSet<PathBuf> = ["proj.md", "work/plan.md"]
            .into_iter()
            .map(PathBuf::from)
            .collect();
        let refused = BTreeSet::from([PathBuf::from("other.md")]);
        let targets = LinkTargets {
            index: &index,
            renamed: &renamed,
            refused: &refused,
        };
        repair_heading_links(contents, Path::new(current), &targets, &spec())
    }

    #[test]
    fn wikilinks_embeds_aliases_and_same_note_links_are_repaired() {
        let result = pass(
            "See [[proj#Requirements]], ![[proj#Requirements]], [[proj#requirements|reqs]], and [[#Requirements]].\n",
            "proj.md",
        );
        assert_eq!(
            result.updated.as_deref(),
            Some(
                "See [[proj#Notes]], ![[proj#Notes]], [[proj#Notes|reqs]], and [[#Notes]].\n"
            )
        );
        assert_eq!(result.repairs.len(), 4);
        assert_eq!(result.repairs[1].before, "[[proj#Requirements]]");
        assert_eq!(result.repairs[1].after, "[[proj#Notes]]");
    }

    #[test]
    fn markdown_links_are_repaired_and_encoded() {
        let spec =
            RenameSpec::parse("requirements", "Future Work").expect("spec");
        let index = index();
        let renamed = BTreeSet::from([PathBuf::from("work/plan.md")]);
        let refused = BTreeSet::new();
        let targets = LinkTargets {
            index: &index,
            renamed: &renamed,
            refused: &refused,
        };
        let result = repair_heading_links(
            "[a](plan.md#Requirements) [b](<work/plan.md#Requirements>) [c](./plan.md#Requirements \"t\")\n",
            Path::new("work/index.md"),
            &targets,
            &spec,
        );
        assert_eq!(
            result.updated.as_deref(),
            Some(
                "[a](plan.md#Future%20Work) [b](<work/plan.md#Future Work>) [c](./plan.md#Future%20Work \"t\")\n"
            )
        );
    }

    #[test]
    fn block_links_code_and_unrelated_headings_stay() {
        let text = "[[proj#^req]] `[[proj#Requirements]]` [[proj#Goals]] [[proj]]\n```\n[[proj#Requirements]]\n```\n";
        let result = pass(text, "x.md");
        assert_eq!(result.updated, None);
        assert!(result.repairs.is_empty());
    }

    #[test]
    fn unresolvable_and_refused_targets_are_reported() {
        let result = pass(
            "[[dup#Requirements]] [[other#Requirements]] [[gone#Requirements]] [[elsewhere#Goals]]\n",
            "x.md",
        );
        assert_eq!(result.updated, None);
        let reasons: Vec<&str> =
            result.unresolved.iter().map(|link| link.reason).collect();
        assert_eq!(
            reasons,
            [
                "its note name is ambiguous",
                "its note was refused",
                "its note does not exist"
            ]
        );
    }

    #[test]
    fn crlf_lines_keep_their_endings() {
        let result = pass("a [[proj#Requirements]]\r\nb\r\n", "x.md");
        assert_eq!(
            result.updated.as_deref(),
            Some("a [[proj#Notes]]\r\nb\r\n")
        );
    }

    #[test]
    fn nested_heading_paths_repair_the_old_segment() {
        let result = pass("[[proj#Plan#Requirements]]\n", "x.md");
        assert_eq!(result.updated.as_deref(), Some("[[proj#Plan#Notes]]\n"));
    }
}
