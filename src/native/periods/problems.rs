//! `bob periods list -p` problem report.
//!
//! Adopted ISO years are those with at least one weekly note. The
//! contract lives in `docs/periods.md`. Report-only: exit 0.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::fs;
use std::path::{Component, Path, PathBuf};

use chrono::{Datelike, NaiveDate};
use serde_json::json;

use super::super::{
    env as bob_env, highlights_ref::note_snapshot, intentions, markdown, stats,
    style::Styler, task_dependencies, vault_links::VaultLinkResolver,
};
use super::calendar::PeriodId;
use super::instructions::INSTRUCTIONS_VERSION;
use super::note::{self, ReviewState};
use intentions::{
    fields::{self, Field, FieldProvenance},
    legacy::{self, IntentionsKind},
    NoteSubject,
};
use note_snapshot::{FragmentLookup, LinkFragment};

/// One reported problem. `stale_instructions` is informational.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Problem {
    pub(crate) kind: ProblemKind,
    pub(crate) path: String,
    pub(crate) detail: String,
    pub(crate) fix: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ProblemKind {
    DanglingDailyParent,
    MissingPeriodNote,
    Unscaffolded,
    WrongParent,
    TaskRows,
    StaleInstructions,
    DanglingPeriodLink,
    IntentionsShape,
    IntentionsOverSoftCap,
    LegacyPeriodText,
}

impl ProblemKind {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::DanglingDailyParent => "dangling_daily_parent",
            Self::MissingPeriodNote => "missing_period_note",
            Self::Unscaffolded => "unscaffolded",
            Self::WrongParent => "wrong_parent",
            Self::TaskRows => "task_rows",
            Self::StaleInstructions => "stale_instructions",
            Self::DanglingPeriodLink => "dangling_period_link",
            Self::IntentionsShape => "intentions_shape",
            Self::IntentionsOverSoftCap => "intentions_over_soft_cap",
            Self::LegacyPeriodText => "legacy_period_text",
        }
    }

    fn level(self) -> &'static str {
        match self {
            Self::StaleInstructions | Self::IntentionsOverSoftCap => "info",
            _ => "error",
        }
    }
}

impl Problem {
    pub(crate) fn to_json(&self) -> serde_json::Value {
        json!({
            "kind": self.kind.as_str(),
            "path": self.path,
            "detail": self.detail,
            "fix": self.fix,
            "level": self.kind.level(),
        })
    }
}

/// Scan the vault for period-note problems as of `today`.
pub(crate) fn collect_problems(
    vault: &Path,
    today: NaiveDate,
) -> Result<Vec<Problem>, String> {
    let period_notes = scan_period_notes(vault);
    let dailies = scan_canonical_dailies(vault);
    let adopted_years: BTreeSet<i32> = period_notes
        .iter()
        .filter_map(|(id, _, _)| match id {
            PeriodId::Week { year, .. } => Some(*year),
            _ => None,
        })
        .collect();
    let existing: HashSet<PeriodId> =
        period_notes.iter().map(|(id, _, _)| *id).collect();

    let mut problems = Vec::new();

    for (date, path, contents) in &dailies {
        let week = PeriodId::week_for_date(*date);
        let PeriodId::Week { year, .. } = week else {
            continue;
        };
        if !adopted_years.contains(&year) {
            continue;
        }
        let expected = format!("[[{}]]", week.link_target());
        let parent = note::parse(contents)
            .frontmatter
            .get("parent")
            .cloned()
            .unwrap_or_default();
        if parent != expected {
            problems.push(Problem {
                kind: ProblemKind::DanglingDailyParent,
                path: path.clone(),
                detail: if parent.is_empty() {
                    format!("missing parent (expected {expected})")
                } else {
                    format!("parent is {parent} (expected {expected})")
                },
                fix: "run bob periods migrate".to_string(),
            });
        }
        if *date <= today {
            let mut chain = vec![week];
            let mut cursor = week;
            while let Some(parent_id) = cursor.parent() {
                chain.push(parent_id);
                cursor = parent_id;
            }
            for id in chain {
                if !existing.contains(&id) {
                    problems.push(Problem {
                        kind: ProblemKind::MissingPeriodNote,
                        path: id.path().to_string_lossy().replace('\\', "/"),
                        detail: format!(
                            "expected {id} for daily {path} ({date})"
                        ),
                        fix: "run bob periods ensure or bob periods migrate"
                            .to_string(),
                    });
                }
            }
        }
    }

    for (id, path, contents) in &period_notes {
        if contents.trim().is_empty() || !note::structure_ok(*id, contents) {
            problems.push(Problem {
                kind: ProblemKind::Unscaffolded,
                path: path.clone(),
                detail: "empty or missing required headings or fences"
                    .to_string(),
                fix: "fill the note from the scaffold or recreate it"
                    .to_string(),
            });
        }
        let parsed = note::parse(contents);
        let expected_parent = match id.parent() {
            Some(parent) => format!("[[{}]]", parent.link_target()),
            None => "[[org]]".to_string(),
        };
        let parent = parsed
            .frontmatter
            .get("parent")
            .cloned()
            .unwrap_or_default();
        if parent != expected_parent {
            problems.push(Problem {
                kind: ProblemKind::WrongParent,
                path: path.clone(),
                detail: if parent.is_empty() {
                    format!("missing parent (expected {expected_parent})")
                } else {
                    format!("parent is {parent} (expected {expected_parent})")
                },
                fix: "run bob periods migrate".to_string(),
            });
        }
        if has_task_row(contents) {
            problems.push(Problem {
                kind: ProblemKind::TaskRows,
                path: path.clone(),
                detail: "period notes hold no task rows".to_string(),
                fix: "move #task checkbox lines out of the period note"
                    .to_string(),
            });
        }
        if parsed.review_state != ReviewState::Reflected {
            let version = parsed.instructions_version.unwrap_or(0);
            if version < INSTRUCTIONS_VERSION {
                problems.push(Problem {
                    kind: ProblemKind::StaleInstructions,
                    path: path.clone(),
                    detail: format!(
                        "callout v{version} is older than v{INSTRUCTIONS_VERSION}"
                    ),
                    fix: "run bob periods ensure -R".to_string(),
                });
            }
        }
    }

    collect_field_problems(vault, &dailies, &period_notes, &mut problems);

    problems.sort_by(|a, b| {
        (a.kind.as_str(), a.path.as_str())
            .cmp(&(b.kind.as_str(), b.path.as_str()))
    });
    Ok(problems)
}

fn collect_field_problems(
    vault: &Path,
    dailies: &[(NaiveDate, String, String)],
    period_notes: &[(PeriodId, String, String)],
    problems: &mut Vec<Problem>,
) {
    let mut notes = Vec::new();
    let adopted_years: BTreeSet<i32> = period_notes
        .iter()
        .filter_map(|(id, _, _)| match id {
            PeriodId::Week { year, .. } => Some(*year),
            _ => None,
        })
        .collect();
    for (date, path, contents) in dailies {
        if adopted_years.contains(&date.iso_week().year())
            && let Some(subject) = NoteSubject::canonical(path)
        {
            notes.push((path.as_str(), contents.as_str(), subject));
        }
    }
    for (_, path, contents) in period_notes {
        if let Some(subject) = NoteSubject::canonical(path)
            && !matches!(subject, NoteSubject::Period(PeriodId::AllTime))
        {
            notes.push((path.as_str(), contents.as_str(), subject));
        }
    }
    notes.sort_by_key(|(path, _, _)| *path);

    // This resolver builds one deterministic note index on its first lookup.
    // Fragment targets are then read and indexed once per resolved note.
    let resolver = VaultLinkResolver::new(vault);
    let mut fragment_indexes: HashMap<
        PathBuf,
        Option<note_snapshot::FragmentIndex>,
    > = HashMap::new();
    let mut target_contents: HashMap<PathBuf, Option<String>> = HashMap::new();

    for (path, contents, subject) in notes {
        let fields = fields::read(contents, subject);
        let shape_messages = fields
            .diagnostics
            .iter()
            .filter(|diagnostic| diagnostic.code != "intentions_over_soft_cap")
            .map(|diagnostic| {
                format!("{}: {}", diagnostic.code, diagnostic.message)
            })
            .collect::<Vec<_>>();
        if !shape_messages.is_empty() {
            problems.push(Problem {
                kind: ProblemKind::IntentionsShape,
                path: path.to_owned(),
                detail: shape_messages.join("; "),
                fix: "correct the listed frontmatter field shape or value; `bob periods migrate` handles recognized legacy layouts only".to_owned(),
            });
        }

        if fields
            .intentions
            .value
            .as_ref()
            .is_some_and(|values| values.len() > 3)
        {
            problems.push(Problem {
                kind: ProblemKind::IntentionsOverSoftCap,
                path: path.to_owned(),
                detail: format!(
                    "{} readable intentions exceed the suggested three-item soft cap; all text is retained",
                    fields.intentions.value.as_ref().map_or(0, Vec::len)
                ),
                fix: "review the list and keep the items that still represent current intentions; Bob does not truncate it".to_owned(),
            });
        }

        let legacy_kind = match subject {
            NoteSubject::Day(_) => IntentionsKind::Day,
            NoteSubject::Period(id) => IntentionsKind::Period(id),
        };
        let legacy_fields = legacy::find_legacy_fields(legacy_kind, contents);
        if !legacy_fields.slots.is_empty() {
            let mut comparisons = Vec::new();
            if let Some(old) = legacy_fields.intentions.as_ref()
                && let Some(result) = compare_list_field(
                    old,
                    &fields.intentions,
                    fields.frontmatter_state,
                )
            {
                comparisons.push(format!("intentions {result}"));
            }
            if let Some(old) = legacy_fields.summary.as_ref()
                && let Some(result) = compare_text_field(
                    old,
                    &fields.summary,
                    fields.frontmatter_state,
                )
            {
                comparisons.push(format!("summary {result}"));
            }
            if let Some(old) = legacy_fields.reflection.as_ref()
                && let Some(result) = compare_text_field(
                    old,
                    &fields.reflection,
                    fields.frontmatter_state,
                )
            {
                comparisons.push(format!("reflection {result}"));
            }
            let conflicts =
                comparisons.iter().any(|item| item.contains("conflicts"));
            let malformed =
                comparisons.iter().any(|item| item.contains("malformed"));
            let fix = if conflicts {
                "reconcile the conflicting legacy and frontmatter text, then run `bob periods migrate`".to_owned()
            } else if malformed {
                "repair malformed frontmatter before running `bob periods migrate`".to_owned()
            } else {
                "run `bob periods migrate` to move recognized legacy text into frontmatter and remove the old shape".to_owned()
            };
            let mut detail = format!(
                "recognized legacy slots remain: {}",
                legacy_fields.slots.join(", ")
            );
            if !comparisons.is_empty() {
                detail.push_str("; ");
                detail.push_str(&comparisons.join("; "));
                detail.push_str("; new frontmatter wins");
            } else {
                detail.push_str("; legacy fallback remains active");
            }
            problems.push(Problem {
                kind: ProblemKind::LegacyPeriodText,
                path: path.to_owned(),
                detail,
                fix,
            });
        }

        if fields.frontmatter_state == fields::FrontmatterState::Valid {
            let owned = owned_markdown_values(&fields);
            for value in owned {
                for target in markdown_targets(value.text) {
                    if let Some((detail, fix)) = check_period_link(
                        vault,
                        Path::new(path),
                        &target,
                        &resolver,
                        &mut target_contents,
                        &mut fragment_indexes,
                    ) {
                        let location = value.item.map_or_else(
                            || value.key.to_owned(),
                            |item| format!("{}[{}]", value.key, item),
                        );
                        problems.push(Problem {
                            kind: ProblemKind::DanglingPeriodLink,
                            path: path.to_owned(),
                            detail: format!("{location} {detail}"),
                            fix,
                        });
                    }
                }
            }
        }
    }
}

fn compare_list_field(
    legacy: &[String],
    field: &Field<Vec<String>>,
    state: fields::FrontmatterState,
) -> Option<String> {
    if state == fields::FrontmatterState::Malformed {
        return Some(
            "cannot be compared because frontmatter is malformed".to_owned(),
        );
    }
    if field.provenance != FieldProvenance::Frontmatter {
        return Some("legacy fallback remains active".to_owned());
    }
    if !field.valid {
        return Some(
            "cannot be compared because frontmatter intentions is malformed"
                .to_owned(),
        );
    }
    let stored = field.value.as_deref().unwrap_or_default();
    Some(if same_text_list(legacy, stored) {
        "is an equal duplicate of frontmatter".to_owned()
    } else {
        "conflicts with frontmatter".to_owned()
    })
}

fn compare_text_field(
    legacy: &str,
    field: &Field<String>,
    state: fields::FrontmatterState,
) -> Option<String> {
    if state == fields::FrontmatterState::Malformed {
        return Some(
            "cannot be compared because frontmatter is malformed".to_owned(),
        );
    }
    if field.provenance != FieldProvenance::Frontmatter {
        return Some("legacy fallback remains active".to_owned());
    }
    if !field.valid {
        return Some(
            "cannot be compared because the frontmatter value is malformed"
                .to_owned(),
        );
    }
    let stored = field.value.as_deref().unwrap_or_default();
    Some(if normalized_text(legacy) == normalized_text(stored) {
        "is an equal duplicate of frontmatter".to_owned()
    } else {
        "conflicts with frontmatter".to_owned()
    })
}

fn same_text_list(left: &[String], right: &[String]) -> bool {
    left.len() == right.len()
        && left.iter().zip(right).all(|(left, right)| {
            normalized_text(left) == normalized_text(right)
        })
}

fn normalized_text(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[derive(Debug)]
struct OwnedMarkdownValue<'a> {
    key: &'static str,
    item: Option<usize>,
    text: &'a str,
}

fn owned_markdown_values(
    fields: &fields::Fields,
) -> Vec<OwnedMarkdownValue<'_>> {
    let mut values = Vec::new();
    if fields.intentions.provenance == FieldProvenance::Frontmatter
        && let Some(intentions) = fields.intentions.value.as_deref()
    {
        values.extend(intentions.iter().enumerate().map(|(index, text)| {
            OwnedMarkdownValue {
                key: "intentions",
                item: Some(index + 1),
                text,
            }
        }));
    }
    if fields.summary.provenance == FieldProvenance::Frontmatter
        && let Some(text) = fields.summary.value.as_deref()
    {
        values.push(OwnedMarkdownValue {
            key: "summary",
            item: None,
            text,
        });
    }
    if fields.reflection.provenance == FieldProvenance::Frontmatter
        && let Some(text) = fields.reflection.value.as_deref()
    {
        values.push(OwnedMarkdownValue {
            key: "reflection",
            item: None,
            text,
        });
    }
    values
}

#[derive(Debug)]
struct MarkdownTarget {
    raw: String,
    fragment: Option<LinkFragment>,
}

fn markdown_targets(text: &str) -> Vec<MarkdownTarget> {
    let lines: Vec<_> = text.lines().collect();
    let hidden = markdown::non_content_line_indexes(&lines);
    let mut targets = Vec::new();
    for (line_index, line) in lines.iter().enumerate() {
        if hidden.contains(&line_index) || markdown::is_indented_code_line(line)
        {
            continue;
        }
        let code_spans = task_dependencies::inline_code_spans(line);
        let mut cursor = 0;
        while cursor < line.len() {
            let wiki = line[cursor..].find("[[").map(|offset| cursor + offset);
            let markdown_link =
                line[cursor..].find("](").map(|offset| cursor + offset);
            let Some(start) = (match (wiki, markdown_link) {
                (Some(wiki), Some(markdown)) => Some(wiki.min(markdown)),
                (Some(wiki), None) => Some(wiki),
                (None, Some(markdown)) => Some(markdown),
                (None, None) => None,
            }) else {
                break;
            };
            if code_spans
                .iter()
                .any(|span| start >= span.start && start < span.end)
                || inline_comment_contains(line, start)
            {
                cursor = start + 1;
                continue;
            }
            if line[start..].starts_with("[[") {
                let inner_start = start + 2;
                let Some(relative_end) = line[inner_start..].find("]]") else {
                    break;
                };
                let end = inner_start + relative_end;
                let parsed =
                    note_snapshot::parse_link_target(&line[inner_start..end]);
                targets.push(MarkdownTarget {
                    raw: parsed.note,
                    fragment: parsed.fragment,
                });
                cursor = end + 2;
                continue;
            }

            let destination_start = start + 2;
            let Some(relative_close) = line[destination_start..].find(')')
            else {
                break;
            };
            let destination_end = destination_start + relative_close;
            let destination = line[destination_start..destination_end].trim();
            let destination =
                if destination.starts_with('<') && destination.ends_with('>') {
                    &destination[1..destination.len() - 1]
                } else {
                    destination.split_whitespace().next().unwrap_or("")
                };
            if destination.is_empty()
                || destination.contains(['(', ')', '<', '>'])
                || code_spans.iter().any(|span| {
                    start < span.end && destination_end + 1 > span.start
                })
                || inline_comment_contains(line, start)
            {
                cursor = destination_end + 1;
                continue;
            }
            let parsed = note_snapshot::parse_link_target(destination);
            targets.push(MarkdownTarget {
                raw: parsed.note,
                fragment: parsed.fragment,
            });
            cursor = destination_end + 1;
        }
    }
    targets
}

fn inline_comment_contains(line: &str, index: usize) -> bool {
    [("%%", "%%"), ("<!--", "-->")]
        .into_iter()
        .any(|(open, close)| {
            let Some(start) = line.find(open) else {
                return false;
            };
            let close_start = line[start + open.len()..]
                .find(close)
                .map(|offset| start + open.len() + offset);
            index >= start
                && close_start.is_none_or(|end| index < end + close.len())
        })
}

fn check_period_link(
    vault: &Path,
    current_path: &Path,
    link: &MarkdownTarget,
    resolver: &VaultLinkResolver,
    target_contents: &mut HashMap<PathBuf, Option<String>>,
    fragment_indexes: &mut HashMap<
        PathBuf,
        Option<note_snapshot::FragmentIndex>,
    >,
) -> Option<(String, String)> {
    let target = link.raw.trim();
    if is_external_target(target) {
        return None;
    }
    if !target.is_empty()
        && !target
            .rsplit_once('.')
            .is_some_and(|(_, extension)| extension.eq_ignore_ascii_case("md"))
        && existing_attachment(vault, current_path, target)
    {
        return None;
    }
    match resolver.resolve(current_path, target) {
        super::super::vault_links::LinkResolution::Found(path) => {
            if let Some(fragment) = link.fragment.as_ref() {
                let text =
                    target_contents.entry(path.clone()).or_insert_with(|| {
                        fs::read_to_string(vault.join(&path)).ok()
                    });
                let index =
                    fragment_indexes.entry(path.clone()).or_insert_with(|| {
                        text.as_deref().map(note_snapshot::build_fragment_index)
                    });
                if let Some(index) = index {
                    match index.lookup(fragment) {
                        FragmentLookup::Found(_) => {}
                        FragmentLookup::Missing => {
                            return Some((
                                format!("links to missing fragment {} in {}", fragment_label(fragment), path.display()),
                                "correct the heading or block fragment, or restore it in the target note".to_owned(),
                            ));
                        }
                        FragmentLookup::Ambiguous { count } => {
                            return Some((
                                format!("links to ambiguous fragment {} in {} ({count} matches)", fragment_label(fragment), path.display()),
                                "make the target heading or block fragment unique".to_owned(),
                            ));
                        }
                    }
                }
            }
            None
        }
        super::super::vault_links::LinkResolution::Ambiguous => Some((
            format!("links to ambiguous note target {target:?}"),
            "qualify the link with a vault-relative Markdown path".to_owned(),
        )),
        super::super::vault_links::LinkResolution::Missing => Some((
            format!("links to missing note or attachment target {target:?}"),
            "correct the owned field link or restore the missing target"
                .to_owned(),
        )),
    }
}

fn fragment_label(fragment: &LinkFragment) -> String {
    match fragment {
        LinkFragment::Heading(heading) => format!("#{heading}"),
        LinkFragment::Block(block) => format!("#^{block}"),
    }
}

fn is_external_target(target: &str) -> bool {
    target.contains("://")
        || ["mailto:", "tel:", "data:", "obsidian:"]
            .iter()
            .any(|prefix| target.to_ascii_lowercase().starts_with(prefix))
}

fn existing_attachment(
    vault: &Path,
    current_path: &Path,
    target: &str,
) -> bool {
    let path = Path::new(target);
    if path.is_absolute()
        || path.components().any(|component| {
            !matches!(component, Component::Normal(_) | Component::CurDir)
        })
    {
        return false;
    }
    let note_directory = current_path.parent().unwrap_or_else(|| Path::new(""));
    vault.join(path).is_file()
        || vault.join(note_directory).join(path).is_file()
}

/// Nightly report-only step. Athena never creates notes.
pub(crate) fn nightly_check(vault: &Path) -> i32 {
    let today = bob_env::current_datetime().date();
    nightly_check_at(vault, today)
}

fn nightly_check_at(vault: &Path, today: NaiveDate) -> i32 {
    match collect_problems(vault, today) {
        Ok(problems) => {
            print_human(&problems);
            0
        }
        Err(message) => {
            eprintln!("bob nightly: {message}");
            1
        }
    }
}

pub(crate) fn print_human(problems: &[Problem]) {
    let styler = Styler::detect();
    if problems.is_empty() {
        println!("no period-note problems");
        return;
    }
    for problem in problems {
        let kind = if problem.kind.level() == "info" {
            styler.cyan(problem.kind.as_str())
        } else {
            styler.yellow(problem.kind.as_str())
        };
        println!("{kind}  {}", problem.path);
        println!("  {}", problem.detail);
        println!("  fix: {}", problem.fix);
    }
}

pub(crate) fn scan_period_notes(
    vault: &Path,
) -> Vec<(PeriodId, String, String)> {
    let mut rows = Vec::new();
    if let Ok(entries) = fs::read_dir(vault) {
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_file() {
                continue;
            }
            let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            if name == "all_time.md" {
                push_period(vault, &path, PeriodId::AllTime, &mut rows);
            } else if name.len() == 7
                && name.ends_with(".md")
                && let Ok(id) = PeriodId::parse(&name[..4])
                && matches!(id, PeriodId::Year(_))
            {
                push_period(vault, &path, id, &mut rows);
            }
        }
    }
    if let Ok(entries) = fs::read_dir(vault) {
        for entry in entries.flatten() {
            let dir = entry.path();
            if !dir.is_dir() {
                continue;
            }
            let Some(dir_name) = dir.file_name().and_then(|n| n.to_str())
            else {
                continue;
            };
            if dir_name.len() != 4
                || !dir_name.bytes().all(|byte| byte.is_ascii_digit())
            {
                continue;
            }
            let Ok(inner) = fs::read_dir(&dir) else {
                continue;
            };
            for entry in inner.flatten() {
                let note_path = entry.path();
                if !note_path.is_file() {
                    continue;
                }
                let Some(name) = note_path.file_name().and_then(|n| n.to_str())
                else {
                    continue;
                };
                if !name.ends_with(".md") || !name.contains('-') {
                    continue;
                }
                let stem = name.trim_end_matches(".md");
                if let Ok(id) = PeriodId::parse(stem)
                    && !matches!(id, PeriodId::Year(_) | PeriodId::AllTime)
                {
                    push_period(vault, &note_path, id, &mut rows);
                }
            }
        }
    }
    rows
}

pub(crate) fn scan_canonical_dailies(
    vault: &Path,
) -> Vec<(NaiveDate, String, String)> {
    let mut rows = Vec::new();
    let Ok(entries) = fs::read_dir(vault) else {
        return rows;
    };
    for entry in entries.flatten() {
        let dir = entry.path();
        if !dir.is_dir() {
            continue;
        }
        let Some(dir_name) = dir.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if dir_name.len() != 4
            || !dir_name.bytes().all(|byte| byte.is_ascii_digit())
        {
            continue;
        }
        let Ok(inner) = fs::read_dir(&dir) else {
            continue;
        };
        for entry in inner.flatten() {
            let path = entry.path();
            if !path.is_file() {
                continue;
            }
            let relative = path
                .strip_prefix(vault)
                .map(|p| p.to_string_lossy().replace('\\', "/"))
                .unwrap_or_else(|_| path.to_string_lossy().replace('\\', "/"));
            let Some(date) = stats::index::canonical_daily_date(&relative)
            else {
                continue;
            };
            let contents = fs::read_to_string(&path).unwrap_or_default();
            rows.push((date, relative, contents));
        }
    }
    rows.sort_by_key(|(date, path, _)| (*date, path.clone()));
    rows
}

fn push_period(
    vault: &Path,
    path: &Path,
    id: PeriodId,
    rows: &mut Vec<(PeriodId, String, String)>,
) {
    let contents = fs::read_to_string(path).unwrap_or_default();
    let display = path
        .strip_prefix(vault)
        .map(|relative| relative.to_string_lossy().replace('\\', "/"))
        .unwrap_or_else(|_| path.to_string_lossy().replace('\\', "/"));
    rows.push((id, display, contents));
}

fn has_task_row(contents: &str) -> bool {
    contents.lines().any(|line| {
        let trimmed = line.trim_start();
        trimmed.starts_with("- [") && trimmed.contains("#task")
    })
}

#[allow(dead_code)]
pub(crate) fn display_path(vault: &Path, target: &Path) -> String {
    target
        .strip_prefix(vault)
        .map(|relative| relative.to_string_lossy().replace('\\', "/"))
        .unwrap_or_else(|_| target.to_string_lossy().replace('\\', "/"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn temp_vault(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir()
            .join(format!("bob-periods-problems-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("create temp vault");
        dir
    }

    #[test]
    fn each_problem_kind_is_reported() {
        let vault = temp_vault("kinds");
        // A weekly note makes 2026 an adopted year.
        let week = vault.join("2026/2026-W45.md");
        fs::create_dir_all(week.parent().unwrap()).unwrap();
        fs::write(
            &week,
            "---\nparent: \"[[journal]]\"\n---\n\n# 2026-W45\n\n- [ ] do it #task\n",
        )
        .unwrap();
        let daily = vault.join("2026/20261103.md");
        fs::write(&daily, "---\nparent: '[[2026/202611]]'\n---\n\n# day\n")
            .unwrap();
        let today = NaiveDate::from_ymd_opt(2026, 11, 3).unwrap();
        let problems = collect_problems(&vault, today).expect("scan");
        let kinds: BTreeSet<_> =
            problems.iter().map(|p| p.kind.as_str()).collect();
        assert!(kinds.contains("dangling_daily_parent"), "{problems:?}");
        assert!(kinds.contains("missing_period_note"), "{problems:?}");
        assert!(kinds.contains("unscaffolded"), "{problems:?}");
        assert!(kinds.contains("wrong_parent"), "{problems:?}");
        assert!(kinds.contains("task_rows"), "{problems:?}");
        assert!(kinds.contains("stale_instructions"), "{problems:?}");
        let _ = fs::remove_dir_all(&vault);
    }

    #[test]
    fn field_findings_cover_canonical_scales_and_legacy_conflicts() {
        let vault = temp_vault("fields");
        fs::create_dir_all(vault.join("2026")).unwrap();
        fs::write(
            vault.join("2026/2026-W45.md"),
            "---\nintentions: [same]\nsummary: same headline\nreflection: same reflection\n---\n\
             ## Intentions\n\n- same\n\n## Review\n\n**Headline:** same headline\n\n## Reflection\n\nsame reflection\n",
        )
        .unwrap();
        fs::write(
            vault.join("2026/2026-W46.md"),
            "---\nintentions: []\n---\n## Intentions\n\n## Review\n",
        )
        .unwrap();
        fs::write(
            vault.join("2026/2026-W47.md"),
            "---\nintentions: [one, two, three]\n---\n",
        )
        .unwrap();
        fs::write(
            vault.join("2026/2026-Q3.md"),
            "## Intentions\n\n- legacy-only quarter\n\n## Reflection\n",
        )
        .unwrap();
        fs::write(
            vault.join("2026/2026-Q4.md"),
            "---\nintentions: scalar\n---\n\
             ## Outcomes\n\n- old outcome\n",
        )
        .unwrap();
        fs::write(
            vault.join("2026.md"),
            "---\nintentions: [one, two, three, four]\n---\n\
             ## Theme\n\nold theme\n\n## Outcomes\n\n- old result\n",
        )
        .unwrap();
        fs::write(
            vault.join("2026/20261103.md"),
            "---\nintentions: [new daily]\nreflection: new reflection\n---\n\
             **Headline:** old daily\n\n**Reflection:** old reflection\n\n## Pomodoros\n",
        )
        .unwrap();
        fs::write(
            vault.join("all_time.md"),
            "---\nintentions: [ignored]\n---\n## Intentions\n\n- ignored\n",
        )
        .unwrap();
        fs::write(
            vault.join("2026/20261103-notebook.md"),
            "---\nintentions: [ignored]\n---\n**Headline:** ignored\n",
        )
        .unwrap();
        fs::write(
            vault.join("2026/20261104.md"),
            "---\nintentions: []\n---\n## Pomodoros\n\n```md\n\
             **Headline:** example\n**Reflection:** example\n## Reflection\n```\n\
             The word Headline: in prose is not a legacy slot.\n",
        )
        .unwrap();
        fs::create_dir_all(vault.join("9999")).unwrap();
        fs::write(
            vault.join("9999/2026-W44.md"),
            "---\nintentions: scalar\n---\n",
        )
        .unwrap();
        fs::create_dir_all(vault.join("2025")).unwrap();
        fs::write(
            vault.join("2025/20250101.md"),
            "---\nintentions: scalar\n---\n**Headline:** ignored\n",
        )
        .unwrap();

        let today = NaiveDate::from_ymd_opt(2026, 11, 3).unwrap();
        let problems = collect_problems(&vault, today).expect("scan");
        let fields: Vec<_> = problems
            .iter()
            .filter(|problem| {
                matches!(
                    problem.kind,
                    ProblemKind::IntentionsShape
                        | ProblemKind::IntentionsOverSoftCap
                        | ProblemKind::LegacyPeriodText
                )
            })
            .collect();
        assert!(
            fields.iter().any(|problem| {
                problem.kind == ProblemKind::IntentionsShape
                    && problem.path == "2026/2026-Q4.md"
                    && problem.detail.contains("scalar read as one item")
            }),
            "{fields:?}"
        );
        assert!(
            fields.iter().any(|problem| {
                problem.kind == ProblemKind::IntentionsOverSoftCap
                    && problem.path == "2026.md"
                    && problem.kind.level() == "info"
            }),
            "{fields:?}"
        );
        let week = fields
            .iter()
            .find(|problem| {
                problem.kind == ProblemKind::LegacyPeriodText
                    && problem.path == "2026/2026-W45.md"
            })
            .expect("week legacy finding");
        assert!(week.detail.contains("equal duplicate"), "{week:?}");
        assert!(week.detail.contains("new frontmatter wins"), "{week:?}");
        let empty_week = fields
            .iter()
            .find(|problem| {
                problem.kind == ProblemKind::LegacyPeriodText
                    && problem.path == "2026/2026-W46.md"
            })
            .expect("empty legacy heading");
        assert!(
            empty_week.detail.contains("equal duplicate"),
            "{empty_week:?}"
        );
        let legacy_only = fields
            .iter()
            .find(|problem| {
                problem.kind == ProblemKind::LegacyPeriodText
                    && problem.path == "2026/2026-Q3.md"
            })
            .expect("legacy-only period note");
        assert!(
            legacy_only
                .detail
                .contains("legacy fallback remains active"),
            "{legacy_only:?}"
        );
        assert!(
            !fields.iter().any(|problem| {
                problem.kind == ProblemKind::IntentionsOverSoftCap
                    && problem.path == "2026/2026-W47.md"
            }),
            "three intentions are within the cap: {fields:?}"
        );
        for path in ["2026/2026-Q4.md", "2026.md", "2026/20261103.md"] {
            assert!(
                fields.iter().any(|problem| {
                    problem.kind == ProblemKind::LegacyPeriodText
                        && problem.path == path
                }),
                "missing legacy shape for {path}: {fields:?}"
            );
        }
        assert!(
            fields.iter().all(|problem| {
                problem.path != "all_time.md"
                    && problem.path != "2026/20261103-notebook.md"
                    && problem.path != "2026/20261104.md"
                    && problem.path != "2025/20250101.md"
                    && problem.path != "9999/2026-W44.md"
            }),
            "noncanonical notes acquired field findings: {fields:?}"
        );
        assert_eq!(nightly_check_at(&vault, today), 0);
        let _ = fs::remove_dir_all(&vault);
    }

    #[test]
    fn shape_findings_preserve_yaml_reasons_and_iso_adopted_scope() {
        let vault = temp_vault("shapes");
        fs::create_dir_all(vault.join("2026")).unwrap();
        fs::create_dir_all(vault.join("2025")).unwrap();
        fs::write(
            vault.join("2026/2026-W01.md"),
            "---\nparent: [[2026]]\n---\n",
        )
        .unwrap();
        fs::write(
            vault.join("2026/20260105.md"),
            "---\nintentions: [one]\nintentions: [two]\n---\n",
        )
        .unwrap();
        fs::write(
            vault.join("2026/20260106.md"),
            "---\nintentions: [one\n---\n",
        )
        .unwrap();
        fs::write(vault.join("2026/20260107.md"), "---\nintentions: [one]\n")
            .unwrap();
        fs::write(
            vault.join("2026/20260108.md"),
            "---\nintentions: [ok, 2, \"\"]\n---\n",
        )
        .unwrap();
        // 2025-12-29 is in ISO week-year 2026, so this daily belongs to the
        // adopted report scope established by the 2026-W01 note.
        fs::write(
            vault.join("2025/20251229.md"),
            "---\nintentions: [ok, 2]\n---\n",
        )
        .unwrap();
        let before = [
            "2026/2026-W01.md",
            "2026/20260105.md",
            "2026/20260106.md",
            "2026/20260107.md",
            "2026/20260108.md",
            "2025/20251229.md",
        ]
        .map(|path| (path, fs::read(vault.join(path)).unwrap()));

        let today = NaiveDate::from_ymd_opt(2026, 1, 8).unwrap();
        let problems = collect_problems(&vault, today).expect("scan");
        let shapes: Vec<_> = problems
            .iter()
            .filter(|problem| problem.kind == ProblemKind::IntentionsShape)
            .collect();
        for (path, reason) in [
            ("2026/20260105.md", "duplicate frontmatter key"),
            ("2026/20260106.md", "invalid frontmatter YAML"),
            ("2026/20260107.md", "frontmatter block is not closed"),
            ("2026/20260108.md", "intentions_type"),
            ("2025/20251229.md", "intentions_type"),
        ] {
            assert!(
                shapes.iter().any(|problem| {
                    problem.path == path && problem.detail.contains(reason)
                }),
                "missing {reason} for {path}: {shapes:?}"
            );
        }
        for (path, bytes) in before {
            assert_eq!(
                fs::read(vault.join(path)).unwrap(),
                bytes,
                "report changed {path}"
            );
        }
        let _ = fs::remove_dir_all(&vault);
    }

    #[test]
    fn dangling_links_resolve_notes_fragments_and_attachments_report_only() {
        let vault = temp_vault("links");
        fs::create_dir_all(vault.join("2026")).unwrap();
        fs::create_dir_all(vault.join("A")).unwrap();
        fs::create_dir_all(vault.join("B")).unwrap();
        fs::create_dir_all(vault.join("files")).unwrap();
        fs::create_dir_all(vault.join("done")).unwrap();
        fs::write(
            vault.join("2026/2026-W45.md"),
            "---\nparent: [[2026/2026-Q4]]\n---\n## Review\n",
        )
        .unwrap();
        fs::write(
            vault.join("target.md"),
            "# Target\n\n## Header\n\nText ^block\n\n## Repeat\n\n## Repeat\n",
        )
        .unwrap();
        fs::write(
            vault.join("done/archived.md"),
            "# Archived\n\n## Archive heading\n",
        )
        .unwrap();
        fs::write(vault.join("A/Same.md"), "# A\n").unwrap();
        fs::write(vault.join("B/Same.md"), "# B\n").unwrap();
        fs::write(vault.join("files/pic.png"), b"image").unwrap();
        let daily = vault.join("2026/20261103.md");
        fs::write(
            &daily,
            "---\nintentions:\n  - \"[[target#Header|heading]]\"\n  - \"![[target#^block|embed]]\"\n  - \"[broken](missing.md#Nope)\"\n  - \"[asset](files/pic.png)\"\n  - \"[web](https://example.com) and `[[inline-code]]`\"\n  - \"[[Same]]\"\n  - \"[bad fragment](target.md#Missing)\"\n  - \"[[target#Repeat]]\"\n  - \"[[#^own]]\"\n  - \"[[done/archived#Archive heading]]\"\nsummary: \"[[missing-summary]]\"\nreflection: |\n  ```md\n  [[hidden-fence]]\n  ```\n---\n## Pomodoros\nOwn anchor ^own\n",
        )
        .unwrap();
        fs::write(
            vault.join("all_time.md"),
            "---\nintentions: [\"[[hidden-all-time]]\"]\n---\n",
        )
        .unwrap();
        let before_daily = fs::read(&daily).unwrap();
        let before_week = fs::read(vault.join("2026/2026-W45.md")).unwrap();

        let today = NaiveDate::from_ymd_opt(2026, 11, 3).unwrap();
        let problems = collect_problems(&vault, today).expect("scan");
        let links: Vec<_> = problems
            .iter()
            .filter(|problem| problem.kind == ProblemKind::DanglingPeriodLink)
            .collect();
        assert_eq!(links.len(), 5, "{links:?}");
        assert!(
            links.iter().any(|problem| {
                problem.detail.contains("intentions[3]")
                    && problem.detail.contains("missing note")
            }),
            "{links:?}"
        );
        assert!(
            links.iter().any(|problem| {
                problem.detail.contains("intentions[6]")
                    && problem.detail.contains("ambiguous note")
            }),
            "{links:?}"
        );
        assert!(
            links.iter().any(|problem| {
                problem.detail.contains("intentions[7]")
                    && problem.detail.contains("missing fragment #Missing")
            }),
            "{links:?}"
        );
        assert!(
            links.iter().any(|problem| {
                problem.detail.contains("intentions[8]")
                    && problem.detail.contains("ambiguous fragment #Repeat")
            }),
            "{links:?}"
        );
        assert!(
            links.iter().any(|problem| {
                problem.detail.contains("summary")
                    && problem.detail.contains("missing note")
            }),
            "{links:?}"
        );
        assert!(
            links.iter().all(|problem| {
                !problem.detail.contains("target#Header")
                    && !problem.detail.contains("target#^block")
                    && !problem.detail.contains("files/pic.png")
                    && !problem.detail.contains("example.com")
                    && !problem.detail.contains("#^own")
                    && !problem.detail.contains("done/archived")
                    && !problem.detail.contains("hidden-fence")
                    && !problem.detail.contains("hidden-all-time")
            }),
            "{links:?}"
        );
        assert_eq!(fs::read(&daily).unwrap(), before_daily);
        assert_eq!(
            fs::read(vault.join("2026/2026-W45.md")).unwrap(),
            before_week
        );
        assert!(links.iter().all(|problem| {
            problem.to_json()["level"] == "error" && !problem.fix.is_empty()
        }));
        let _ = fs::remove_dir_all(&vault);
    }
}
