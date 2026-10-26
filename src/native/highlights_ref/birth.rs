//! Birth lineage for manually queued reread-shaped PDFs.
//!
//! Before create can mint rereads itself, a reread-shaped PDF
//! (`<base>__vN`, N >= 2) may still arrive through the queue by hand. This
//! module derives its birth `earlier:` chain once, in the shared single-PDF
//! sync plan when the note is new, so inline intake and scan drains file
//! the same lineage. The chain is carried in birth-only plan state,
//! outside the PDF projection and hash; repeat syncs preserve it verbatim
//! and never re-derive it, and no backlinks are written into old notes.
//!
//! A stem probes lineage only when it parses as `<base>__vN` with N >= 2.
//! The probe requires a live `<base>` or lower `<base>__vM` sharing
//! identity with the new PDF's marker: equal normalized URL dedupe keys
//! (complete sets for collections), else equal `source_note`, else equal
//! normalized titles. The newborn's `earlier:` is the newest live family
//! member plus that member's own `earlier:` entries verbatim, so the
//! lineage-derived reading number tracks the filename version through
//! dangling middle members. Unrelated occupants never link: without
//! exactly one matching family the birth stays ordinary.

use std::collections::BTreeSet;
use std::path::Path;

use super::*;
use crate::native::ref_library::{
    build_index,
    lineage::{parse_earlier, reading_number, EarlierEntry, EarlierKind},
    LibraryConfig, ParsedFrontmatter, RefRow,
};
use crate::native::ref_tasks::ReadingIdentity;

/// One rendered `earlier:` row: the link target (vault-relative, without
/// `.md`) and its static display label.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct BirthEarlierEntry {
    pub target: String,
    pub label: Option<String>,
}

/// Lineage-derived birth metadata for one reread: the family root stem,
/// the newborn's lineage reading number, and the full `earlier:` chain,
/// newest first.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct BirthLineage {
    pub root_stem: String,
    pub reading: u32,
    pub entries: Vec<BirthEarlierEntry>,
}

/// Split a `<base>__vN` stem with N >= 2. Anything else — ordinary stems
/// and natural version-looking stems without lineage — probes nothing.
pub(super) fn split_reread_stem(stem: &str) -> Option<(&str, u32)> {
    let (base, version) = stem.rsplit_once("__v")?;
    if base.is_empty()
        || version.is_empty()
        || !version.bytes().all(|byte| byte.is_ascii_digit())
    {
        return None;
    }
    let number: u32 = version.parse().ok()?;
    if number < 2 {
        return None;
    }
    Some((base, number))
}

/// Derive birth lineage for a new note path from the PDF marker identity.
/// Returns `None` for ordinary births: a non-reread stem, an unreadable
/// index, no live predecessor sharing identity, or predecessors spanning
/// more than one family.
pub(super) fn derive_birth_lineage(
    config: &Config,
    note_path: &Path,
    marker: &Projection,
) -> Option<BirthLineage> {
    let stem = note_path.file_stem().and_then(|stem| stem.to_str())?;
    let (base, number) = split_reread_stem(stem)?;
    let library = LibraryConfig {
        bob_dir: config.bob_dir.clone(),
        ref_dir: config.ref_dir.clone(),
        xlib_dir: config.xlib_dir.clone(),
    };
    let index = build_index(&library).ok()?;
    let candidates = predecessor_candidates(&index.rows, base, number);
    if candidates.is_empty() {
        return None;
    }
    let wanted_urls = marker_url_keys(marker);
    let marker_source_note = marker
        .get("source_note")
        .and_then(MarkerValue::as_string)
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let marker_title = marker
        .get("title")
        .and_then(MarkerValue::as_string)
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let mut matched: Vec<&RefRow> = Vec::new();
    for candidate in &candidates {
        if identities_match(
            candidate,
            config,
            &wanted_urls,
            marker_source_note,
            marker_title,
        ) {
            matched.push(candidate);
        }
    }
    if matched.is_empty() {
        return None;
    }
    let mut families = BTreeSet::new();
    for row in &matched {
        families.insert(row.family.clone());
    }
    if families.len() != 1 {
        return None;
    }
    let family = families.into_iter().next().expect("one family");
    let mut members: Vec<&RefRow> = index
        .rows
        .iter()
        .filter(|row| row.family == family && row.superseded_by.is_none())
        .collect();
    if members.is_empty() {
        return None;
    }
    members.sort_by(|left, right| {
        left.reading
            .unwrap_or(1)
            .cmp(&right.reading.unwrap_or(1))
            .reverse()
            .then_with(|| left.path.cmp(&right.path))
    });
    let newest = members[0];
    let newest_reading = newest.reading.unwrap_or(1);
    // The newborn lists the newest live member plus that member's own
    // chain verbatim, so the lineage-derived reading number tracks the
    // allocated stem version through dangling middle members.
    let mut chain = vec![EarlierEntry {
        target: strip_md_suffix(&newest.path).to_string(),
        label: Some(member_label(newest_reading, newest.captured.as_deref())),
        kind: EarlierKind::Reading,
    }];
    chain.extend(newest.earlier.iter().cloned());
    let reading = reading_number(&chain);
    let entries = chain
        .into_iter()
        .map(|entry| BirthEarlierEntry {
            target: entry.target,
            label: entry.label,
        })
        .collect();
    let family_key = index.rows.iter().find(|row| row.path == family)?;
    Some(BirthLineage {
        root_stem: super::super::ref_library::lineage::family_root_stem(
            &family_key.path,
            &family_key.earlier,
        ),
        reading,
        entries,
    })
}

/// Live predecessor rows for a `<base>__vN` birth: the `<base>` note and
/// lower `<base>__vM` members, whatever their family. Superseded legacy
/// notes never seed lineage.
fn predecessor_candidates<'rows>(
    rows: &'rows [RefRow],
    base: &str,
    number: u32,
) -> Vec<&'rows RefRow> {
    rows.iter()
        .filter(|row| {
            if row.superseded_by.is_some() {
                return false;
            }
            let stem = path_stem(&row.path);
            if stem == base {
                return true;
            }
            let prefix = format!("{base}__v");
            let Some(rest) = stem.strip_prefix(prefix.as_str()) else {
                return false;
            };
            rest.bytes().all(|byte| byte.is_ascii_digit())
                && rest
                    .parse::<u32>()
                    .is_ok_and(|member| member >= 1 && member < number)
        })
        .collect()
}

/// Whether one candidate shares identity with the new PDF's marker: equal
/// normalized URL dedupe keys when the marker carries URLs, else equal
/// `source_note`, else equal normalized titles.
fn identities_match(
    candidate: &RefRow,
    config: &Config,
    wanted_urls: &[String],
    marker_source_note: Option<&str>,
    marker_title: Option<&str>,
) -> bool {
    if !wanted_urls.is_empty() {
        let mut row_keys: Vec<String> = candidate
            .identity
            .keys
            .iter()
            .filter(|key| {
                !key.starts_with("arxiv:")
                    && !key.starts_with("doi:")
                    && !key.starts_with("raw:")
            })
            .cloned()
            .collect();
        row_keys.sort();
        row_keys.dedup();
        let mut wanted = wanted_urls.to_vec();
        wanted.sort();
        wanted.dedup();
        return row_keys == wanted;
    }
    if let Some(marker_note) = marker_source_note {
        return candidate_source_note(candidate, config).is_some_and(|note| {
            normalize_source_note(&note) == normalize_source_note(marker_note)
        });
    }
    match (candidate_title(candidate), marker_title) {
        (Some(row_title), Some(marker)) => {
            super::stamp::normalize_title(&row_title)
                == super::stamp::normalize_title(marker)
        }
        _ => false,
    }
}

/// Normalized URL dedupe keys from the marker's `source_url` values.
fn marker_url_keys(marker: &Projection) -> Vec<String> {
    let Some(value) = marker.get("source_url") else {
        return Vec::new();
    };
    let raw: Vec<&str> = match value {
        MarkerValue::String(single) => vec![single.as_str()],
        MarkerValue::List(values) => {
            values.iter().filter_map(MarkerValue::as_string).collect()
        }
        _ => Vec::new(),
    };
    let mut keys = Vec::new();
    for url in raw {
        let trimmed = url.trim();
        if trimmed.is_empty() {
            continue;
        }
        if let Ok(cleaned) = validate_and_clean(trimmed) {
            push_unique_key(&mut keys, cleaned.dedupe_key);
        }
    }
    keys
}

fn push_unique_key(keys: &mut Vec<String>, key: String) {
    if !keys.contains(&key) {
        keys.push(key);
    }
}

/// The candidate note's frontmatter `source_note`, read directly: the
/// library row carries no `source_note` field, and only base/lower-`__vM`
/// candidates are ever read.
pub(super) fn candidate_source_note(
    candidate: &RefRow,
    config: &Config,
) -> Option<String> {
    let contents =
        std::fs::read_to_string(config.bob_dir.join(&candidate.path)).ok()?;
    let (frontmatter, _) = super::frontmatter::split_frontmatter(&contents)?;
    ParsedFrontmatter::parse(&frontmatter)
        .get_str("source_note")
        .map(str::to_string)
}

/// Mirror `create`'s snapshot comparison: bare routes, aliases stripped,
/// lowercased.
pub(super) fn normalize_source_note(value: &str) -> String {
    let value = value.trim().trim_start_matches("[[").trim_end_matches("]]");
    value
        .split_once('|')
        .map(|(target, _)| target)
        .unwrap_or(value)
        .to_lowercase()
}

fn candidate_title(candidate: &RefRow) -> Option<String> {
    let title = candidate.title.trim();
    (!title.is_empty()).then(|| title.to_string())
}

/// `vM · <captured date>` for a live family member, or bare `vM` when no
/// captured date is recorded. Labels are display-only; the lineage reader
/// never parses them for identity.
fn member_label(reading: u32, captured: Option<&str>) -> String {
    match captured.map(str::trim).filter(|date| !date.is_empty()) {
        Some(date) => format!("v{reading} · {date}"),
        None => format!("v{reading}"),
    }
}

fn strip_md_suffix(path: &str) -> &str {
    path.strip_suffix(".md")
        .or_else(|| path.strip_suffix(".MD"))
        .unwrap_or(path)
}

fn path_stem(path: &str) -> String {
    Path::new(path)
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or(path)
        .to_string()
}

/// Render a newborn's `earlier:` chain in the block style the lineage
/// reader parses (Obsidian's canonical Properties form), newest first.
pub(super) fn render_birth_earlier(
    entries: &[BirthEarlierEntry],
) -> Vec<String> {
    let mut lines = vec!["earlier:".to_string()];
    for entry in entries {
        let link = match &entry.label {
            Some(label) => format!("[[{}|{}]]", entry.target, label),
            None => format!("[[{}]]", entry.target),
        };
        lines.push(format!("  - {}", yaml_dquote(&link)));
    }
    lines
}

fn yaml_dquote(value: &str) -> String {
    let mut quoted = String::with_capacity(value.len() + 2);
    quoted.push('"');
    for ch in value.chars() {
        if ch == '"' || ch == '\\' {
            quoted.push('\\');
        }
        quoted.push(ch);
    }
    quoted.push('"');
    quoted
}

/// The confirmed reading for a reopen insert, derived from the existing
/// note's own `earlier:` list: frontmatter raw lines in file order. Only a
/// lineage reading (>= 2) confirms; the root is the last `Reading` entry's
/// stem.
pub(super) fn reading_from_note_frontmatter(
    raw_lines: &[String],
) -> Option<ReadingIdentity> {
    let front = ParsedFrontmatter::parse(raw_lines);
    let entries = parse_earlier(&front);
    let number = reading_number(&entries);
    if number < 2 {
        return None;
    }
    let root =
        super::super::ref_library::lineage::family_root_stem("", &entries);
    Some(ReadingIdentity {
        root_stem: root,
        number,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_config(root: &Path) -> Config {
        Config {
            bob_dir: root.to_path_buf(),
            lib_dir: root.join("lib"),
            ref_dir: root.join("ref"),
            xlib_dir: root.join("xlib"),
        }
    }

    fn temp_root(case: &str) -> std::path::PathBuf {
        let root = std::env::temp_dir().join(format!(
            "bob-birth-{}-{}-{case}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|duration| duration.as_nanos())
                .unwrap_or(0),
        ));
        std::fs::create_dir_all(root.join("ref/papers")).expect("ref dir");
        std::fs::create_dir_all(root.join("lib/papers")).expect("lib dir");
        std::fs::create_dir_all(root.join("xlib/papers")).expect("xlib dir");
        root
    }

    fn note(front_lines: &[&str], body: &str) -> String {
        format!("---\n{}\n---\n{body}", front_lines.join("\n"))
    }

    fn write_note(root: &Path, rel: &str, contents: &str) {
        let path = root.join(rel);
        std::fs::create_dir_all(path.parent().expect("note has a parent"))
            .expect("parent dir");
        std::fs::write(&path, contents).expect("write note");
    }

    fn marker(url: Option<&str>, title: &str) -> Projection {
        let mut projection = Projection::new();
        projection.insert(
            "status".to_string(),
            MarkerValue::String("ready".to_string()),
        );
        projection.insert(
            "title".to_string(),
            MarkerValue::String(title.to_string()),
        );
        if let Some(url) = url {
            projection.insert(
                "source_url".to_string(),
                MarkerValue::String(url.to_string()),
            );
        }
        projection
    }

    fn filed_note(url: &str, title: &str, extra: &[&str]) -> String {
        let mut lines = vec![
            format!("title: {title}"),
            format!("source_url: {url}"),
            "source_pdf: lib/papers/backing.pdf".to_string(),
            "status: ready".to_string(),
            "captured: 2026-09-01".to_string(),
        ];
        lines.extend(extra.iter().map(|line| line.to_string()));
        note(
            &lines.iter().map(String::as_str).collect::<Vec<_>>(),
            "# T\n",
        )
    }

    #[test]
    fn non_reread_stems_probe_nothing() {
        assert_eq!(split_reread_stem("alpha"), None);
        assert_eq!(split_reread_stem("alpha__v1"), None);
        assert_eq!(split_reread_stem("protocol__v2x"), None);
        assert_eq!(split_reread_stem("__v2"), None);
        assert_eq!(split_reread_stem("alpha__V2"), None);
        assert_eq!(split_reread_stem("alpha__v2"), Some(("alpha", 2)));
        assert_eq!(split_reread_stem("a__v2__v3"), Some(("a__v2", 3)));
    }

    #[test]
    fn version_looking_pdf_without_predecessor_stays_ordinary() {
        let root = temp_root("no-predecessor");
        let config = test_config(&root);
        let lineage = derive_birth_lineage(
            &config,
            &root.join("ref/papers/protocol__v2.md"),
            &marker(Some("https://example.com/protocol"), "Protocol"),
        );
        assert_eq!(lineage, None);
    }

    #[test]
    fn url_identity_derives_chain_and_reading() {
        let root = temp_root("url-chain");
        let config = test_config(&root);
        let url = "https://example.com/alpha";
        write_note(
            &root,
            "ref/papers/alpha.md",
            &filed_note(url, "Alpha Paper", &[]),
        );
        let lineage = derive_birth_lineage(
            &config,
            &root.join("ref/papers/alpha__v2.md"),
            &marker(Some(url), "Alpha Paper"),
        )
        .expect("lineage");
        assert_eq!(lineage.root_stem, "alpha");
        assert_eq!(lineage.reading, 2);
        assert_eq!(
            lineage.entries,
            vec![BirthEarlierEntry {
                target: "ref/papers/alpha".to_string(),
                label: Some("v1 · 2026-09-01".to_string()),
            }]
        );
        assert_eq!(
            render_birth_earlier(&lineage.entries),
            vec![
                "earlier:".to_string(),
                "  - \"[[ref/papers/alpha|v1 · 2026-09-01]]\"".to_string(),
            ]
        );
    }

    #[test]
    fn chain_copies_newest_member_entries_through_dangling_gaps() {
        let root = temp_root("dangling-chain");
        let config = test_config(&root);
        let url = "https://example.com/gap";
        write_note(&root, "ref/papers/gap.md", &filed_note(url, "Gap", &[]));
        write_note(
            &root,
            "ref/papers/gap__v3.md",
            &filed_note(
                url,
                "Gap",
                &[
                    "earlier:",
                    "  - \"[[ref/papers/gap__v2|v2 · 2026-02-01]]\"",
                    "  - \"[[ref/papers/gap|v1 · 2026-01-01]]\"",
                ],
            ),
        );
        let lineage = derive_birth_lineage(
            &config,
            &root.join("ref/papers/gap__v4.md"),
            &marker(Some(url), "Gap"),
        )
        .expect("lineage");
        assert_eq!(lineage.root_stem, "gap");
        assert_eq!(lineage.reading, 4);
        let targets: Vec<&str> = lineage
            .entries
            .iter()
            .map(|entry| entry.target.as_str())
            .collect();
        assert_eq!(
            targets,
            vec!["ref/papers/gap__v3", "ref/papers/gap__v2", "ref/papers/gap"]
        );
        assert_eq!(
            lineage.entries[0].label.as_deref(),
            Some("v3 · 2026-09-01")
        );
    }

    #[test]
    fn deleted_root_keeps_the_original_stem_for_birth_and_reopen() {
        let root = temp_root("deleted-root");
        let config = test_config(&root);
        let url = "https://example.com/deleted-root";
        write_note(
            &root,
            "ref/papers/foo__v2.md",
            &filed_note(
                url,
                "Foo",
                &["earlier:", "  - \"[[ref/papers/foo|v1 · 2026-01-01]]\""],
            ),
        );
        write_note(
            &root,
            "ref/papers/foo__v3.md",
            &filed_note(
                url,
                "Foo",
                &[
                    "earlier:",
                    "  - \"[[ref/papers/foo__v2|v2 · 2026-02-01]]\"",
                    "  - \"[[ref/papers/foo|v1 · 2026-01-01]]\"",
                ],
            ),
        );

        let lineage = derive_birth_lineage(
            &config,
            &root.join("ref/papers/foo__v4.md"),
            &marker(Some(url), "Foo"),
        )
        .expect("the new reading belongs to the family");
        assert_eq!(lineage.root_stem, "foo");
        assert_eq!(lineage.reading, 4);

        let raw = vec![
            "earlier:".to_string(),
            "  - \"[[ref/papers/foo__v3|v3]]\"".to_string(),
            "  - \"[[ref/papers/foo__v2|v2]]\"".to_string(),
            "  - \"[[ref/papers/foo|v1]]\"".to_string(),
        ];
        let reopened = reading_from_note_frontmatter(&raw).expect("reopen");
        assert_eq!(reopened.root_stem, lineage.root_stem);
        assert_eq!(reopened.number, lineage.reading);
    }

    #[test]
    fn unrelated_occupant_never_links() {
        let root = temp_root("unrelated");
        let config = test_config(&root);
        write_note(
            &root,
            "ref/papers/alpha.md",
            &filed_note("https://example.com/other", "Other", &[]),
        );
        let lineage = derive_birth_lineage(
            &config,
            &root.join("ref/papers/alpha__v2.md"),
            &marker(Some("https://example.com/alpha"), "Alpha"),
        );
        assert_eq!(lineage, None);
    }

    #[test]
    fn title_identity_matches_when_no_urls() {
        let root = temp_root("title-match");
        let config = test_config(&root);
        write_note(
            &root,
            "ref/papers/notesnap.md",
            &note(
                &[
                    "title: Weekly Notes",
                    "source_note: sase",
                    "source_pdf: lib/papers/notesnap.pdf",
                    "status: ready",
                    "captured: 2026-09-01",
                ],
                "# Weekly Notes\n",
            ),
        );
        // A marker with no URLs falls back to source_note identity.
        let mut projection = marker(None, "Weekly Notes");
        projection.insert(
            "source_note".to_string(),
            MarkerValue::String("sase".to_string()),
        );
        let lineage = derive_birth_lineage(
            &config,
            &root.join("ref/papers/notesnap__v2.md"),
            &projection,
        )
        .expect("lineage");
        assert_eq!(lineage.reading, 2);
        assert_eq!(lineage.entries[0].target.as_str(), "ref/papers/notesnap");
    }

    #[test]
    fn title_identity_matches_without_urls_or_source_note() {
        let root = temp_root("title-only");
        let config = test_config(&root);
        write_note(
            &root,
            "ref/papers/sketch.md",
            &note(
                &[
                    "title: Sketch Notes",
                    "source_pdf: lib/papers/sketch.pdf",
                    "status: ready",
                    "captured: 2026-09-01",
                ],
                "# Sketch Notes\n",
            ),
        );
        let lineage = derive_birth_lineage(
            &config,
            &root.join("ref/papers/sketch__v2.md"),
            &marker(None, "sketch  notes"),
        )
        .expect("lineage");
        assert_eq!(lineage.reading, 2);
        let miss = derive_birth_lineage(
            &config,
            &root.join("ref/papers/sketch__v2.md"),
            &marker(None, "Something Else"),
        );
        assert_eq!(miss, None);
    }

    #[test]
    fn reopen_reading_comes_from_note_own_earlier() {
        let raw = vec![
            "title: Gap".to_string(),
            "earlier:".to_string(),
            "  - \"[[ref/papers/gap|v1 · 2026-01-01]]\"".to_string(),
        ];
        let reading = reading_from_note_frontmatter(&raw).expect("reading");
        assert_eq!(reading.number, 2);
        assert_eq!(reading.root_stem, "gap");
        let plain = vec!["title: Solo".to_string()];
        assert_eq!(reading_from_note_frontmatter(&plain), None);
    }
}
