//! Shared place-selector lexing and resolution.
//!
//! Task drawers, project `##` sections, and notebook `##` sections share
//! one leading-`#` lexer and one selection ladder. Each adapter supplies
//! its scope policy: key forms, slug availability, reserved and
//! structural guards, near-miss threshold, and create-title casing.
//! Catalog I/O and destination mutation stay outside this module.

use std::ops::Range;

use super::line::{normalize_task_text, selector_slug};
use super::pomodoro_literal::{lex_exact_name_after_hash, opens_exact_name};

/// Whitespace-collapsed Unicode uppercase identity for a drawer or
/// project-section title. Task drawers and project H2s share this key.
pub(crate) fn place_key(title: &str) -> String {
    normalize_task_text(title).to_uppercase()
}

/// Whether `value` is a valid unquoted place slug: ASCII letter start,
/// then ASCII letters, digits, or hyphens.
pub(crate) fn is_place_slug(value: &str) -> bool {
    let mut chars = value.bytes();
    let Some(first) = chars.next() else {
        return false;
    };
    if !first.is_ascii_alphabetic() {
        return false;
    }
    chars.all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
}

/// One existing place a selector can land on.
///
/// `line` is the caller's identity: 1-based `Drawer.line` for task
/// drawers and project H2s, 0-based `line_index` for notebook headings
/// (including `usize::MAX` draft sentinels).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PlaceCandidate {
    pub(crate) title: String,
    pub(crate) key: String,
    pub(crate) slug: String,
    pub(crate) line: usize,
}

impl PlaceCandidate {
    pub(crate) fn new(
        title: impl Into<String>,
        key: impl Into<String>,
        slug: impl Into<String>,
        line: usize,
    ) -> Self {
        Self {
            title: title.into(),
            key: key.into(),
            slug: slug.into(),
            line,
        }
    }

    pub(crate) fn from_heading(title: &str, line_index: usize) -> Self {
        Self::new(title, title, selector_slug(title), line_index)
    }
}

/// Shared selection result. Adapters map this onto their public types.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PlaceOutcome {
    Existing(PlaceCandidate),
    Create {
        title: String,
        key: String,
        slug: String,
        warning: Option<String>,
    },
    Ambiguous {
        matches: Vec<PlaceCandidate>,
    },
    NearMiss {
        slug: String,
        quoted_title: String,
        /// Drawer/project near-miss copy names the existing place and the
        /// quoted override that would create. Notebook leaves this `None`.
        message: Option<String>,
        code: Option<&'static str>,
    },
    Refuse {
        code: &'static str,
        message: String,
    },
}

/// Catalog lookup used by the drawer and project-section adapters.
/// Loading the catalog stays with the caller.
pub(crate) trait PlaceCatalog {
    fn spelling_for_slug(&self, slug: &str) -> String;
    fn near_title(&self, new_key: &str) -> Option<String>;
    fn is_legacy_title(&self, title: &str) -> bool;
}

/// Opening `#` on a first-level body, including a leading `\#` escape.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct HashOpener {
    /// Start of the `#` token after leading whitespace.
    pub(crate) start: usize,
    /// Byte offset of the `#` itself (`start` or `start + 1` when escaped).
    pub(crate) sigil: usize,
    pub(crate) escaped: bool,
}

/// Scan the start of a marker-stripped first-level body for a leading
/// `#` shortcut. The opener matches the type-marker predicate: an ASCII
/// letter, `"` or `“`, or a bare `#` followed by whitespace or end of
/// line. `#` plus a digit, `##`, and `#-` stay prose.
pub(crate) fn scan_hash_opener(body: &str) -> Option<HashOpener> {
    let trimmed_start = body.len() - body.trim_start().len();
    let rest = &body[trimmed_start..];
    if rest.is_empty() {
        return None;
    }
    let escaped = rest.starts_with("\\#");
    let sigil_rel = if escaped { 1 } else { 0 };
    if !rest[sigil_rel..].starts_with('#') {
        return None;
    }
    let after_hash = &rest[sigil_rel + 1..];
    let next = after_hash.chars().next();
    let opens = next.is_some_and(|character| {
        character.is_ascii_alphabetic() || matches!(character, '"' | '“')
    });
    let bare = next.is_none() || next.is_some_and(char::is_whitespace);
    if !opens && !bare {
        return None;
    }
    Some(HashOpener {
        start: trimmed_start,
        sigil: trimmed_start + sigil_rel,
        escaped,
    })
}

/// Quoted exact-name after `#`: decoded text, extent, completeness.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct QuotedSelector {
    pub(crate) decoded: String,
    pub(crate) literal_len: usize,
    pub(crate) complete: bool,
}

/// Lex a quoted exact-name that starts `after_hash`. `None` when the
/// text does not open with `"` or `“`.
pub(crate) fn quoted_after_hash(after_hash: &str) -> Option<QuotedSelector> {
    if !opens_exact_name(after_hash) {
        return None;
    }
    let lex = lex_exact_name_after_hash(after_hash, after_hash.len())?;
    Some(QuotedSelector {
        decoded: lex.decoded,
        literal_len: lex.literal_len,
        complete: lex.complete,
    })
}

/// First whitespace byte at or after `start`, or `line.len()`.
pub(crate) fn token_end(line: &str, start: usize) -> usize {
    let mut end = start;
    while end < line.len()
        && !line[end..].chars().next().unwrap().is_whitespace()
    {
        end += line[end..].chars().next().map(char::len_utf8).unwrap_or(1);
    }
    end
}

pub(crate) fn leftover_after(line: &str, end: usize) -> String {
    line.get(end..).unwrap_or("").trim().to_string()
}

pub(crate) fn shift_range(span: Range<usize>, base: usize) -> Range<usize> {
    (span.start + base)..(span.end + base)
}

/// Create-title for an unquoted notebook selector: `-` becomes a space,
/// then each whitespace-separated word that contains an uppercase letter
/// keeps the typed capitals (`#API-keys` → `API Keys`) and every other
/// word takes first-character Title Case (`#git` → `Git`).
pub(crate) fn format_notebook_section_create_title(selector: &str) -> String {
    selector
        .replace('-', " ")
        .split_whitespace()
        .map(format_create_word)
        .collect::<Vec<_>>()
        .join(" ")
}

fn format_create_word(word: &str) -> String {
    if word.chars().any(char::is_uppercase) {
        return word.to_string();
    }
    let mut chars = word.chars();
    match chars.next() {
        None => String::new(),
        Some(first) => first.to_uppercase().chain(chars).collect(),
    }
}

/// Task-drawer adapter: reserved names, key-form matching, prefix or
/// edit-distance-1 near-miss, case-preserving exact titles.
pub(crate) struct TaskDrawer;

impl TaskDrawer {
    pub(crate) fn resolve<C: PlaceCatalog>(
        selector: &str,
        exact: bool,
        items: &[PlaceCandidate],
        catalog: &C,
        is_reserved: impl Fn(&str) -> bool,
    ) -> PlaceOutcome {
        resolve_drawer_place(
            selector,
            exact,
            items,
            catalog,
            "drawer",
            |title| title.to_string(),
            is_reserved,
        )
    }
}

/// Project-`##` adapter: the same key ladder as [`TaskDrawer`], with
/// section nouns and promotion Title Case on creation. Structural
/// headings are refused by the H2 write wrapper before this matcher
/// runs, so a managed `## Tasks` cannot become eligible by matching.
pub(crate) struct ProjectSection;

impl ProjectSection {
    pub(crate) fn resolve<C: PlaceCatalog>(
        selector: &str,
        exact: bool,
        items: &[PlaceCandidate],
        catalog: &C,
        is_reserved: impl Fn(&str) -> bool,
        title_case: impl Fn(&str) -> String,
    ) -> PlaceOutcome {
        resolve_drawer_place(
            selector,
            exact,
            items,
            catalog,
            "section",
            title_case,
            is_reserved,
        )
    }
}

/// Notebook-`##` adapter: ASCII title match, then unique slug, then
/// prefix or edit-distance-2 near-miss. No reserved set. Broader token
/// syntax (`#section^id`) stays in the notebook lexer.
pub(crate) struct NotebookSection;

impl NotebookSection {
    pub(crate) fn resolve(
        selector: &str,
        exact: bool,
        items: &[PlaceCandidate],
    ) -> PlaceOutcome {
        if exact {
            return match classify_matches(title_matches(selector, items)) {
                MatchClass::Unique(item) => {
                    PlaceOutcome::Existing(item.clone())
                }
                MatchClass::None => PlaceOutcome::Create {
                    title: selector.to_string(),
                    key: place_key(selector),
                    slug: selector_slug(selector),
                    warning: None,
                },
                MatchClass::Many(matches) => PlaceOutcome::Ambiguous {
                    matches: owned_matches(matches),
                },
            };
        }

        match classify_matches(title_matches(selector, items)) {
            MatchClass::Unique(item) => {
                return PlaceOutcome::Existing(item.clone());
            }
            MatchClass::Many(matches) => {
                return PlaceOutcome::Ambiguous {
                    matches: owned_matches(matches),
                };
            }
            MatchClass::None => {}
        }

        let query_slug = selector_slug(selector);
        match classify_matches(slug_matches(&query_slug, items)) {
            MatchClass::Unique(item) => {
                return PlaceOutcome::Existing(item.clone());
            }
            MatchClass::Many(matches) => {
                return PlaceOutcome::Ambiguous {
                    matches: owned_matches(matches),
                };
            }
            MatchClass::None => {}
        }

        if let Some((slug, _)) = notebook_near_miss(items, &query_slug) {
            return PlaceOutcome::NearMiss {
                slug,
                quoted_title: format_notebook_section_create_title(selector),
                message: None,
                code: None,
            };
        }

        let title = format_notebook_section_create_title(selector);
        PlaceOutcome::Create {
            key: place_key(&title),
            slug: selector_slug(&title),
            title,
            warning: None,
        }
    }
}

fn resolve_drawer_place<C, Case, Reserved>(
    selector: &str,
    exact: bool,
    items: &[PlaceCandidate],
    catalog: &C,
    noun: &'static str,
    title_case: Case,
    is_reserved: Reserved,
) -> PlaceOutcome
where
    C: PlaceCatalog,
    Case: Fn(&str) -> String,
    Reserved: Fn(&str) -> bool,
{
    if exact {
        if let Some(key) = reserved_of(selector, true, &is_reserved) {
            return reserved_outcome(&key, noun);
        }
        let key = place_key(selector);
        let matches: Vec<&PlaceCandidate> =
            items.iter().filter(|item| item.key == key).collect();
        return match classify_matches(matches) {
            MatchClass::Unique(item) => PlaceOutcome::Existing(item.clone()),
            MatchClass::Many(matches) => {
                ambiguous_drawer(selector, exact, noun, matches)
            }
            MatchClass::None => {
                let title = normalize_task_text(selector);
                drawer_create(title, catalog, noun)
            }
        };
    }

    let slug = selector.to_string();
    let key_plain = place_key(&slug);
    let key_spaced = place_key(&slug.replace('-', " "));
    let mut matches: Vec<&PlaceCandidate> = items
        .iter()
        .filter(|item| item.key == key_plain || item.key == key_spaced)
        .collect();
    matches.sort_by_key(|item| item.line);
    matches.dedup_by_key(|item| item.line);
    match classify_matches(matches) {
        MatchClass::Unique(item) => {
            return PlaceOutcome::Existing(item.clone());
        }
        MatchClass::Many(matches) => {
            return ambiguous_drawer(&slug, false, noun, matches);
        }
        MatchClass::None => {}
    }

    if let Some(key) = reserved_of(&slug, false, &is_reserved) {
        return reserved_outcome(&key, noun);
    }

    if let Some(near) = find_key_near_duplicate(items, &slug) {
        let intended = catalog.spelling_for_slug(&slug);
        let intended = title_case(if catalog.is_legacy_title(&intended) {
            &intended
        } else {
            &key_spaced
        });
        let suggestion = if !is_place_slug(&near.slug) {
            quoted_legacy_suggestion(&near.title, catalog)
                .unwrap_or_else(|| format!("#{}", near.slug))
        } else {
            format!("#{}", near.slug)
        };
        return PlaceOutcome::NearMiss {
            slug: near.slug.clone(),
            quoted_title: intended.clone(),
            message: Some(format!(
                "did you mean `{suggestion}`? write `#\"{intended}\"` to create {intended}"
            )),
            code: Some("drawer_near_duplicate"),
        };
    }

    let title = catalog.spelling_for_slug(&slug);
    let title = title_case(if catalog.is_legacy_title(&title) {
        &title
    } else {
        &key_spaced
    });
    drawer_create(title, catalog, noun)
}

fn drawer_create<C: PlaceCatalog>(
    title: String,
    catalog: &C,
    noun: &'static str,
) -> PlaceOutcome {
    let key = place_key(&title);
    let slug = selector_slug(&title);
    let warning = catalog
        .near_title(&key)
        .map(|near| format!("new {noun} {key} · did you mean {near}?"));
    PlaceOutcome::Create {
        title,
        key,
        slug,
        warning,
    }
}

fn reserved_of(
    selector: &str,
    exact: bool,
    is_reserved: impl Fn(&str) -> bool,
) -> Option<String> {
    if exact {
        let key = place_key(selector);
        return is_reserved(&key).then_some(key);
    }
    let key_plain = place_key(selector);
    let key_spaced = place_key(&selector.replace('-', " "));
    [key_plain, key_spaced]
        .into_iter()
        .find(|key| is_reserved(key))
}

fn reserved_outcome(key: &str, noun: &str) -> PlaceOutcome {
    PlaceOutcome::Refuse {
        code: "reserved_drawer",
        message: format!(
            "`{key}` is managed by Bob; name the {noun} something else"
        ),
    }
}

fn ambiguous_drawer(
    selector: &str,
    exact: bool,
    noun: &str,
    matches: Vec<&PlaceCandidate>,
) -> PlaceOutcome {
    let lines = matches
        .iter()
        .map(|item| item.line.to_string())
        .collect::<Vec<_>>()
        .join(", ");
    let shown = if exact {
        selector.to_string()
    } else {
        format!("#{selector}")
    };
    let hint = if exact {
        "rename one"
    } else {
        "use a quoted literal to pick one"
    };
    PlaceOutcome::Refuse {
        code: "ambiguous_drawer",
        message: format!(
            "{noun} `{shown}` matches more than one {noun} (lines {lines}); {hint}"
        ),
    }
}

fn quoted_legacy_suggestion<C: PlaceCatalog>(
    title: &str,
    catalog: &C,
) -> Option<String> {
    let canonical = normalize_task_text(title).to_ascii_uppercase();
    catalog
        .is_legacy_title(&canonical)
        .then(|| format!("#\"{canonical}\""))
}

/// Unique nearby drawer for a failed slug: strict key prefix or edit
/// distance 1, earliest line wins.
pub(crate) fn find_key_near_duplicate<'a>(
    items: &'a [PlaceCandidate],
    slug: &str,
) -> Option<&'a PlaceCandidate> {
    let needle_key = place_key(slug);
    let needle_spaced = place_key(&slug.replace('-', " "));
    let mut candidates: Vec<&PlaceCandidate> = Vec::new();
    for item in items {
        if item.key.starts_with(&needle_key)
            && item.key.len() > needle_key.len()
        {
            candidates.push(item);
            continue;
        }
        if item.key.starts_with(&needle_spaced)
            && item.key.len() > needle_spaced.len()
        {
            candidates.push(item);
            continue;
        }
        if crate::native::note_tasks::bounded_levenshtein(
            &needle_key,
            &item.key,
            1,
        )
        .is_some()
        {
            candidates.push(item);
            continue;
        }
        if crate::native::note_tasks::bounded_levenshtein(
            &needle_spaced,
            &item.key,
            1,
        )
        .is_some()
        {
            candidates.push(item);
        }
    }
    candidates.sort_by_key(|item| item.line);
    candidates.into_iter().next()
}

enum MatchClass<'a> {
    None,
    Unique(&'a PlaceCandidate),
    Many(Vec<&'a PlaceCandidate>),
}

fn classify_matches<'a>(matches: Vec<&'a PlaceCandidate>) -> MatchClass<'a> {
    match matches.len() {
        0 => MatchClass::None,
        1 => MatchClass::Unique(matches[0]),
        _ => MatchClass::Many(matches),
    }
}

fn owned_matches(matches: Vec<&PlaceCandidate>) -> Vec<PlaceCandidate> {
    matches.into_iter().cloned().collect()
}

fn title_matches<'a>(
    selector: &str,
    items: &'a [PlaceCandidate],
) -> Vec<&'a PlaceCandidate> {
    items
        .iter()
        .filter(|item| item.title.eq_ignore_ascii_case(selector))
        .collect()
}

fn slug_matches<'a>(
    query_slug: &str,
    items: &'a [PlaceCandidate],
) -> Vec<&'a PlaceCandidate> {
    if query_slug.is_empty() {
        return Vec::new();
    }
    items
        .iter()
        .filter(|item| !item.slug.is_empty() && item.slug == query_slug)
        .collect()
}

fn notebook_near_miss(
    items: &[PlaceCandidate],
    query_slug: &str,
) -> Option<(String, String)> {
    if query_slug.is_empty() {
        return None;
    }
    if let Some(item) = items
        .iter()
        .find(|item| !item.slug.is_empty() && item.slug.starts_with(query_slug))
    {
        return Some((item.slug.clone(), item.title.clone()));
    }
    let mut best: Option<(usize, String)> = None;
    for item in items {
        if item.slug.is_empty() {
            continue;
        }
        if let Some(distance) = crate::native::note_tasks::bounded_levenshtein(
            query_slug, &item.slug, 2,
        ) && best
            .as_ref()
            .is_none_or(|(best_distance, _)| distance < *best_distance)
        {
            best = Some((distance, item.slug.clone()));
        }
    }
    best.map(|(_, slug)| {
        let title = items
            .iter()
            .find(|item| item.slug == slug)
            .map(|item| item.title.clone())
            .unwrap_or_else(|| slug.clone());
        (slug, title)
    })
}
