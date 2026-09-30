//! Queued-task lineup for whole-item Pomodoro starts: list the started
//! entry's direct-child Task Links and resolve them read-only through the
//! batch planner's staged vault view. Resolution never fails the start and
//! never writes; unresolvable links become rows with `resolved: false` and
//! a `warning` in the close's wording. The pure drop engine
//! ([`plan_start_drop`]) validates `~<K>` numbers against the lineup and
//! removes each dropped Task Link subtree byte-exactly.

use std::{
    cmp::Reverse,
    collections::{BTreeMap, BTreeSet},
    fmt,
    path::Path,
};

use super::{
    capture::{leading_spaces_or_tabs_len, line_spans, LineSpan},
    capture_pomodoro_close::{
        bare_embedded_link, bare_plain_link, close_task_text, join_numbers,
        lookup_task, range_is_struck, strikethrough_inner_spans,
        strip_pomodoro_markers, sub_bullet_range, wikilink_tokens, CloseVault,
    },
    capture_task_toggle::child_block_end_line,
    markdown, note_tasks,
    vault_links::LinkResolution,
};

/// One listed Task Link on the started entry: its 1-based lineup number in
/// ledger order, the 1-based ledger line plus the parsed `[[path#^id]]`
/// target.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct StartLink {
    pub index: u32,
    pub ledger_line: usize,
    pub embedded: bool,
    pub path_part: String,
    pub block_id: String,
    pub block_link: String,
}

/// One resolved lineup row, following the close row's explicit-null
/// convention: unresolved rows carry `None` fields plus a `warning`.
/// `index` is the 1-based lineup number (kept rows hold their pre-image
/// number, so a drop leaves gaps like 1, 3).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct StartTaskRow {
    pub index: u32,
    pub block_link: String,
    pub embedded: bool,
    pub ledger_line: usize,
    pub resolved: bool,
    pub relative_target: Option<String>,
    pub block_id: String,
    pub text: Option<String>,
    pub status_symbol: Option<char>,
    pub status_name: Option<String>,
    pub warning: Option<String>,
}

/// List the started entry's direct-child Task Links in ledger order.
///
/// A listed line is an indented bullet at the entry's first child
/// indentation whose body, after stripping 🍅 markers, is exactly one
/// block link — plain `[[path#^id]]` or embedded `![[path#^id]]` — and is
/// not struck through. Notes, deeper descendants, mixed lines, struck
/// links, and fenced lines are not listed.
pub(crate) fn list_queued_links(
    contents: &str,
    entry_index: usize,
) -> Vec<StartLink> {
    let spans = line_spans(contents);
    let line_text: Vec<&str> = spans.iter().map(|span| span.text).collect();
    if line_text.get(entry_index).is_none() {
        return Vec::new();
    }
    let range = sub_bullet_range(&line_text, entry_index);
    let Some(first) = range.clone().next() else {
        return Vec::new();
    };
    let child_indent = leading_spaces_or_tabs_len(line_text[first]);
    let fenced = markdown::fenced_lines(&line_text, 0..line_text.len());
    let mut links = Vec::new();
    for index in range {
        if fenced.contains(&index) {
            continue;
        }
        let line = line_text[index];
        if leading_spaces_or_tabs_len(line) != child_indent {
            continue;
        }
        let stripped = strip_pomodoro_markers(line);
        let tokens = wikilink_tokens(&stripped);
        if tokens.len() != 1 {
            continue;
        }
        let token = &tokens[0];
        let bare = if token.embedded {
            bare_embedded_link(&stripped)
        } else {
            bare_plain_link(&stripped)
        };
        if bare.is_none() {
            continue;
        }
        let strike_spans = strikethrough_inner_spans(&stripped);
        if range_is_struck(token.start, token.end, &strike_spans) {
            continue;
        }
        links.push(StartLink {
            index: links.len() as u32 + 1,
            ledger_line: index + 1,
            embedded: token.embedded,
            path_part: token.path_part.clone(),
            block_id: token.block_id.clone(),
            block_link: token.token.clone(),
        });
    }
    links
}

/// Resolve listed links read-only through the staged vault view, using the
/// same vault view and `note_tasks` lookup the close planner uses so a
/// task created earlier in the same draft resolves and text matches the
/// close's rows. Embedded links resolve with the global filter cleared,
/// like the close's embedded rows.
pub(crate) fn resolve_queued_links<V: CloseVault>(
    vault: &V,
    day_path: &Path,
    links: &[StartLink],
) -> Vec<StartTaskRow> {
    let settings = note_tasks::read_settings(vault.bob_dir());
    let mut all_settings = settings.clone();
    all_settings.global_filter.clear();
    links
        .iter()
        .map(|link| {
            resolve_one(vault, day_path, link, &settings, &all_settings)
        })
        .collect()
}

fn resolve_one<V: CloseVault>(
    vault: &V,
    day_path: &Path,
    link: &StartLink,
    settings: &note_tasks::NoteTaskSettings,
    all_settings: &note_tasks::NoteTaskSettings,
) -> StartTaskRow {
    let unresolved =
        |relative_target: Option<String>, warning: String| StartTaskRow {
            index: link.index,
            block_link: link.block_link.clone(),
            embedded: link.embedded,
            ledger_line: link.ledger_line,
            resolved: false,
            relative_target,
            block_id: link.block_id.clone(),
            text: None,
            status_symbol: None,
            status_name: None,
            warning: Some(warning),
        };
    let path = match vault.resolve_target(day_path, &link.path_part) {
        LinkResolution::Found(path) => path,
        LinkResolution::Ambiguous => {
            return unresolved(
                None,
                format!("{} has an ambiguous note basename", link.block_link),
            );
        }
        LinkResolution::Missing => {
            return unresolved(
                None,
                format!("{} does not resolve to a vault note", link.block_link),
            );
        }
    };
    let relative_target = relative_name(vault.bob_dir(), &path);
    let contents = match vault.read_latest(&path) {
        Ok(Some(contents)) => contents,
        _ => {
            return unresolved(
                Some(relative_target.clone()),
                format!(
                    "{} resolves to {relative_target}, which cannot be read",
                    link.block_link
                ),
            );
        }
    };
    let lookup_settings = if link.embedded {
        all_settings
    } else {
        settings
    };
    match lookup_task(&contents, lookup_settings, &link.block_id) {
        Ok(task) => StartTaskRow {
            index: link.index,
            block_link: link.block_link.clone(),
            embedded: link.embedded,
            ledger_line: link.ledger_line,
            resolved: true,
            relative_target: Some(relative_target),
            block_id: link.block_id.clone(),
            text: Some(close_task_text(&task.description)),
            status_symbol: Some(task.status_symbol),
            status_name: Some(task.status_name.clone()),
            warning: None,
        },
        Err(reason) => unresolved(
            Some(relative_target.clone()),
            format!("{relative_target} {reason}"),
        ),
    }
}

fn relative_name(bob_dir: &Path, path: &Path) -> String {
    path.strip_prefix(bob_dir)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

/// Who owns the started session, for drop diagnostics: the entry name when
/// the session has one, "the next Pomodoro" for an unnamed placeholder, or
/// the created-session variant (whose lineup is always empty) for a named
/// start that matches nothing open.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum StartOwner {
    Next,
    Named { name: String },
    Created { name: String },
}

/// One dropped Task Link subtree: its lineup row plus the count of
/// non-blank descendant lines removed with it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct StartDroppedLink {
    pub link: StartLink,
    pub nested_lines: u32,
}

/// The pure drop plan: updated day-note contents with each dropped subtree
/// removed, the dropped rows (pre-image ledger lines), the kept lineup
/// numbers in ledger order, and non-blocking duplicate warnings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct StartDropPlan {
    pub contents: String,
    pub dropped: Vec<StartDroppedLink>,
    pub kept: Vec<u32>,
    pub warnings: Vec<String>,
}

/// One failed drop validation: every out-of-range number, collected into a
/// single diagnostic in the close's wording.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct StartDropError {
    pub token: String,
    pub bad_numbers: Vec<u32>,
    pub total: usize,
    pub owner: StartOwner,
}

impl fmt::Display for StartDropError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let names = if self.bad_numbers.len() == 1 {
            format!("names task {}", self.bad_numbers[0])
        } else {
            format!("names tasks {}", join_numbers(&self.bad_numbers))
        };
        // The suggested token is the typed token without its `~<K>`.
        let suggestion = self.token.split('~').next().unwrap_or(&self.token);
        match &self.owner {
            StartOwner::Created { name } => write!(
                f,
                "`{}` {names}, but {name} starts as a new session with no queued Task Links; start it with `{suggestion}`",
                self.token
            ),
            StartOwner::Named { name } if !name.is_empty() => {
                start_range_error(
                    f,
                    &self.token,
                    &names,
                    name,
                    suggestion,
                    self.total,
                )
            }
            _ => start_range_error(
                f,
                &self.token,
                &names,
                "the next Pomodoro",
                suggestion,
                self.total,
            ),
        }
    }
}

impl std::error::Error for StartDropError {}

fn start_range_error(
    f: &mut fmt::Formatter<'_>,
    token: &str,
    names: &str,
    owner: &str,
    suggestion: &str,
    total: usize,
) -> fmt::Result {
    if total == 0 {
        write!(
            f,
            "`{token}` {names}, but {owner} has no queued Task Links; start it with `{suggestion}`"
        )
    } else if total == 1 {
        write!(
            f,
            "`{token}` {names}, but {owner} has 1 queued Task Link (1)"
        )
    } else {
        write!(
            f,
            "`{token}` {names}, but {owner} has {total} queued Task Links (1–{total})"
        )
    }
}

/// Validate `drop` against the entry's queued lineup on the staged
/// pre-image and remove each dropped Task Link bullet together with its
/// nested child lines, back to front so offsets stay valid.
///
/// Subtree removal matches what Obsidian does when a Task Link leaves an
/// open Pomodoro, and differs from close drops on purpose: a closed session
/// keeps its notes as history, but in a session about to start a queued
/// link's children belong to that link. `entry_index` is unchanged because
/// every removal sits after the entry line. The bytes contract matches the
/// unnamed start: CRLF is preserved and removing a last line with no
/// terminator leaves no new final newline.
pub(crate) fn plan_start_drop(
    contents: &str,
    entry_index: usize,
    drop: &[u32],
    owner: &StartOwner,
    token: &str,
) -> Result<StartDropPlan, StartDropError> {
    let lineup = list_queued_links(contents, entry_index);
    let total = lineup.len();
    let requested: BTreeSet<u32> = drop.iter().copied().collect();
    let bad: Vec<u32> = requested
        .iter()
        .copied()
        .filter(|number| *number < 1 || (*number as usize) > total)
        .collect();
    if !bad.is_empty() {
        return Err(StartDropError {
            token: token.to_string(),
            bad_numbers: bad,
            total,
            owner: owner.clone(),
        });
    }
    let by_index: BTreeMap<u32, &StartLink> =
        lineup.iter().map(|link| (link.index, link)).collect();
    let spans = line_spans(contents);
    let mut ranges: Vec<(usize, usize)> = Vec::new();
    let mut dropped: Vec<StartDroppedLink> = Vec::new();
    // Duplicate targets grouped so a dropped link still queued under
    // another number warns instead of vanishing silently.
    let mut by_target: BTreeMap<(String, String), Vec<u32>> = BTreeMap::new();
    for link in &lineup {
        by_target
            .entry((link.path_part.clone(), link.block_id.clone()))
            .or_default()
            .push(link.index);
    }
    let mut warnings = Vec::new();
    for number in &requested {
        let link = by_index[number];
        let first_line = link.ledger_line.saturating_sub(1);
        let last_line = child_block_end_line(&spans, first_line);
        let nested_lines = spans
            .iter()
            .skip(first_line + 1)
            .take(last_line.saturating_sub(first_line))
            .filter(|span| !span.text.trim().is_empty())
            .count() as u32;
        ranges.push((first_line, last_line));
        if let Some(indices) =
            by_target.get(&(link.path_part.clone(), link.block_id.clone()))
        {
            let kept: Vec<u32> = indices
                .iter()
                .copied()
                .filter(|index| !requested.contains(index))
                .collect();
            if let Some(still) = kept.first() {
                warnings.push(format!(
                    "task {number} `{}` is still queued as task {still}",
                    link.block_link
                ));
            }
        }
        dropped.push(StartDroppedLink {
            link: (*link).clone(),
            nested_lines,
        });
    }
    dropped.sort_by_key(|row| row.link.index);
    let kept: Vec<u32> = lineup
        .iter()
        .map(|link| link.index)
        .filter(|index| !requested.contains(index))
        .collect();
    Ok(StartDropPlan {
        contents: remove_line_ranges(contents, &spans, &ranges),
        dropped,
        kept,
        warnings,
    })
}

/// Remove whole 0-based inclusive line ranges from `contents`, back to
/// front. Cuts sit on span boundaries so CRLF survives; when the removed
/// block runs to a final line with no terminator, the preceding terminator
/// goes instead so no new final newline appears.
fn remove_line_ranges(
    contents: &str,
    spans: &[LineSpan<'_>],
    ranges: &[(usize, usize)],
) -> String {
    // Byte ranges come from the pre-image spans; applying them back to
    // front keeps every lower offset valid.
    let mut cuts: Vec<(usize, usize)> = ranges
        .iter()
        .filter_map(|(first, last)| {
            let end = spans.get(*last)?.end;
            let start = if *first == 0 { 0 } else { spans[first - 1].end };
            if *last + 1 == spans.len()
                && !contents.ends_with('\n')
                && *first > 0
            {
                let prev_start = if *first - 1 == 0 {
                    0
                } else {
                    spans[first - 2].end
                };
                let term_len = spans[first - 1].end
                    - prev_start
                    - spans[first - 1].text.len();
                Some((start - term_len, end))
            } else {
                Some((start, end))
            }
        })
        .collect();
    cuts.sort_by_key(|cut| Reverse(cut.0));
    let mut updated = contents.to_string();
    for (start, end) in cuts {
        updated.replace_range(start..end, "");
    }
    updated
}

#[cfg(test)]
mod lineup_tests {
    use std::{
        collections::BTreeMap,
        path::{Path, PathBuf},
    };

    use super::*;

    struct MemoryVault {
        root: PathBuf,
        files: BTreeMap<PathBuf, String>,
    }

    impl MemoryVault {
        fn new(root: &str) -> Self {
            Self {
                root: PathBuf::from(root),
                files: BTreeMap::new(),
            }
        }

        fn with(mut self, name: &str, contents: &str) -> Self {
            self.files
                .insert(self.root.join(name), contents.to_string());
            self
        }
    }

    impl CloseVault for MemoryVault {
        fn bob_dir(&self) -> &Path {
            &self.root
        }

        fn resolve_target(
            &self,
            _from_path: &Path,
            target: &str,
        ) -> LinkResolution {
            if target.is_empty() {
                return LinkResolution::Found(self.root.clone());
            }
            let candidate = self.root.join(format!("{target}.md"));
            if self.files.contains_key(&candidate) {
                LinkResolution::Found(candidate)
            } else {
                LinkResolution::Missing
            }
        }

        fn read_latest(&self, path: &Path) -> Result<Option<String>, String> {
            Ok(self.files.get(path).cloned())
        }
    }

    fn day_path() -> PathBuf {
        PathBuf::from("/vault/day.md")
    }

    #[test]
    fn lists_direct_children_in_ledger_order() {
        let contents = concat!(
            "## Pomodoros\n",
            "- [ ] (**0945-1010** [t:: 25m]) — CAPTURE\n",
            "\t- [[bob#^first]]\n",
            "\t- [[bob#^second]]\n",
            "\t- a note, not a link\n",
            "\t- [[bob#^third]] and more text\n",
        );
        let links = list_queued_links(contents, 1);
        let ids: Vec<&str> =
            links.iter().map(|link| link.block_id.as_str()).collect();
        assert_eq!(ids, vec!["first", "second"]);
        assert_eq!(links[0].ledger_line, 3);
        assert_eq!(links[1].ledger_line, 4);
        assert!(!links[0].embedded);
        assert_eq!(links[0].block_link, "[[bob#^first]]");
    }

    #[test]
    fn skips_deeper_descendants_embeds_and_struck_lines() {
        let contents = concat!(
            "## Pomodoros\n",
            "- [ ] (**0945-1010** [t:: 25m]) — CAPTURE\n",
            "\t- [[bob#^direct]]\n",
            "\t\t- [[bob#^grandchild]]\n",
            "\t- ![[bob#^embed]]\n",
            "\t- ~~[[bob#^struck]]~~\n",
            "\t- 🍅 [[bob#^marked]]\n",
            "- [ ] () — NEXT\n",
        );
        let links = list_queued_links(contents, 1);
        let ids: Vec<&str> =
            links.iter().map(|link| link.block_id.as_str()).collect();
        assert_eq!(ids, vec!["direct", "embed", "marked"]);
        assert!(links[1].embedded);
        assert_eq!(links[1].block_link, "[[bob#^embed]]");
    }

    #[test]
    fn skips_fenced_lines() {
        let contents = concat!(
            "## Pomodoros\n",
            "- [ ] (**0945-1010** [t:: 25m]) — CAPTURE\n",
            "\t- [[bob#^visible]]\n",
            "```md\n",
            "\t- [[bob#^fenced]]\n",
            "```\n",
        );
        let links = list_queued_links(contents, 1);
        let ids: Vec<&str> =
            links.iter().map(|link| link.block_id.as_str()).collect();
        assert_eq!(ids, vec!["visible"]);
    }

    #[test]
    fn resolves_rows_through_the_staged_view() {
        let vault = MemoryVault::new("/vault").with(
            "bob.md",
            "## Tasks\n\n- [/] #task Stop capture from the panel ^ready\n",
        );
        let contents = concat!(
            "## Pomodoros\n",
            "- [ ] (**0945-1010** [t:: 25m]) — CAPTURE\n",
            "\t- [[bob#^ready]]\n",
            "\t- [[bob#^gone]]\n",
            "\t- [[missing#^nowhere]]\n",
        );
        let links = list_queued_links(contents, 1);
        assert_eq!(links.len(), 3);
        let rows = resolve_queued_links(&vault, &day_path(), &links);
        assert_eq!(rows.len(), 3);
        assert!(rows[0].resolved);
        assert_eq!(rows[0].relative_target.as_deref(), Some("bob.md"));
        assert_eq!(
            rows[0].text.as_deref(),
            Some("Stop capture from the panel")
        );
        assert_eq!(rows[0].status_symbol, Some('/'));
        assert!(rows[0].warning.is_none());
        assert!(!rows[1].resolved);
        assert_eq!(
            rows[1].warning.as_deref(),
            Some("bob.md has no task with block ID ^gone")
        );
        assert!(rows[1].text.is_none());
        assert!(rows[1].status_symbol.is_none());
        assert!(!rows[2].resolved);
        assert_eq!(
            rows[2].warning.as_deref(),
            Some("[[missing#^nowhere]] does not resolve to a vault note")
        );
        assert!(rows[2].relative_target.is_none());
    }
}

#[cfg(test)]
mod drop_tests {
    use super::*;

    fn named(name: &str) -> StartOwner {
        StartOwner::Named {
            name: name.to_string(),
        }
    }

    fn fixture() -> String {
        concat!(
            "## Pomodoros\n",
            "- [ ] () — CAPTURE\n",
            "\t- [[bob#^first]]\n",
            "\t- [[bob#^second]]\n",
            "\t  - remember the URL parser\n",
            "\t- [[sase#^third]]\n",
            "- [ ] () — SASE\n",
        )
        .to_string()
    }

    #[test]
    fn drops_numbered_subtree_and_keeps_gaps() {
        let plan =
            plan_start_drop(&fixture(), 1, &[2], &named("CAPTURE"), "=~2")
                .expect("in range");
        assert_eq!(
            plan.contents,
            concat!(
                "## Pomodoros\n",
                "- [ ] () — CAPTURE\n",
                "\t- [[bob#^first]]\n",
                "\t- [[sase#^third]]\n",
                "- [ ] () — SASE\n",
            )
        );
        assert_eq!(plan.kept, vec![1, 3]);
        assert_eq!(plan.warnings, Vec::<String>::new());
        assert_eq!(plan.dropped.len(), 1);
        assert_eq!(plan.dropped[0].link.index, 2);
        assert_eq!(plan.dropped[0].link.ledger_line, 4);
        assert_eq!(plan.dropped[0].link.block_link, "[[bob#^second]]");
        assert_eq!(plan.dropped[0].nested_lines, 1);
    }

    #[test]
    fn empty_drop_is_byte_identical() {
        let contents = fixture();
        let plan = plan_start_drop(&contents, 1, &[], &named("CAPTURE"), "=")
            .expect("empty drop");
        assert_eq!(plan.contents, contents);
        assert_eq!(plan.kept, vec![1, 2, 3]);
        assert!(plan.dropped.is_empty());
        assert!(plan.warnings.is_empty());
    }

    #[test]
    fn drops_nested_links_with_their_parent() {
        let contents = concat!(
            "## Pomodoros\n",
            "- [ ] () — CAPTURE\n",
            "\t- [[bob#^first]]\n",
            "\t\t- [[bob#^grandchild]]\n",
            "\t- [[bob#^second]]\n",
        );
        let plan = plan_start_drop(contents, 1, &[1], &named("CAPTURE"), "=~1")
            .expect("in range");
        assert_eq!(
            plan.contents,
            concat!(
                "## Pomodoros\n",
                "- [ ] () — CAPTURE\n",
                "\t- [[bob#^second]]\n",
            )
        );
        assert_eq!(plan.dropped[0].nested_lines, 1);
        assert_eq!(plan.kept, vec![2]);
    }

    #[test]
    fn drops_embedded_and_unresolved_rows_by_number() {
        let contents = concat!(
            "## Pomodoros\n",
            "- [ ] () — CAPTURE\n",
            "\t- ![[bob#^embed]]\n",
            "\t- [[missing#^nowhere]]\n",
            "\t- [[bob#^kept]]\n",
        );
        // Validation is numeric only: resolution never blocks a drop.
        let plan =
            plan_start_drop(contents, 1, &[1, 2], &named("CAPTURE"), "=~1,2")
                .expect("in range");
        assert_eq!(
            plan.contents,
            concat!(
                "## Pomodoros\n",
                "- [ ] () — CAPTURE\n",
                "\t- [[bob#^kept]]\n",
            )
        );
        assert!(plan.dropped[0].link.embedded);
        assert_eq!(plan.kept, vec![3]);
    }

    #[test]
    fn fenced_lines_are_not_numbered() {
        let contents = concat!(
            "## Pomodoros\n",
            "- [ ] () — CAPTURE\n",
            "\t- [[bob#^visible]]\n",
            "```md\n",
            "\t- [[bob#^fenced]]\n",
            "```\n",
        );
        let error =
            plan_start_drop(contents, 1, &[2], &named("CAPTURE"), "=~2")
                .expect_err("fenced link is not task 2");
        assert_eq!(
            error.to_string(),
            "`=~2` names task 2, but CAPTURE has 1 queued Task Link (1)"
        );
    }

    #[test]
    fn out_of_range_names_every_bad_number() {
        let contents = fixture();
        let error =
            plan_start_drop(&contents, 1, &[4], &named("CAPTURE"), "=~4")
                .expect_err("task 4 is out of range");
        assert_eq!(
            error.to_string(),
            "`=~4` names task 4, but CAPTURE has 3 queued Task Links (1–3)"
        );
        let error =
            plan_start_drop(&contents, 1, &[4, 5], &named("CAPTURE"), "=~4,5")
                .expect_err("tasks 4 and 5 are out of range");
        assert_eq!(
            error.to_string(),
            "`=~4,5` names tasks 4 and 5, but CAPTURE has 3 queued Task Links (1–3)"
        );
        let error =
            plan_start_drop(&contents, 1, &[0], &named("CAPTURE"), "=~0")
                .expect_err("task 0 is out of range");
        assert_eq!(
            error.to_string(),
            "`=~0` names task 0, but CAPTURE has 3 queued Task Links (1–3)"
        );
    }

    #[test]
    fn three_bad_numbers_join_with_commas() {
        let contents = "## Pomodoros\n- [ ] () — CAPTURE\n\t- [[bob#^only]]\n";
        let error = plan_start_drop(
            contents,
            1,
            &[2, 3, 4],
            &named("CAPTURE"),
            "=~2,3,4",
        )
        .expect_err("all out of range");
        assert_eq!(
            error.to_string(),
            "`=~2,3,4` names tasks 2, 3 and 4, but CAPTURE has 1 queued Task Link (1)"
        );
    }

    #[test]
    fn empty_lineup_suggests_the_token_without_its_drop() {
        let contents = "## Pomodoros\n- [ ] () — CAPTURE\n- [ ] () — SASE\n";
        let error =
            plan_start_drop(contents, 1, &[1], &named("CAPTURE"), "=~1")
                .expect_err("no queued links");
        assert_eq!(
            error.to_string(),
            "`=~1` names task 1, but CAPTURE has no queued Task Links; start it with `=`"
        );
        let error =
            plan_start_drop(contents, 1, &[1], &StartOwner::Next, "=3~2,4")
                .expect_err("no queued links");
        assert_eq!(
            error.to_string(),
            "`=3~2,4` names task 1, but the next Pomodoro has no queued Task Links; start it with `=3`"
        );
    }

    #[test]
    fn created_session_reports_the_new_session_variant() {
        let contents = "## Pomodoros\n- [ ] () — CAPTURE\n";
        let error = plan_start_drop(
            contents,
            1,
            &[1],
            &StartOwner::Created {
                name: "PLAN".to_string(),
            },
            "=#plan~1",
        )
        .expect_err("created sessions start empty");
        assert_eq!(
            error.to_string(),
            "`=#plan~1` names task 1, but PLAN starts as a new session with no queued Task Links; start it with `=#plan`"
        );
    }

    #[test]
    fn duplicate_still_queued_warns() {
        let contents = concat!(
            "## Pomodoros\n",
            "- [ ] () — CAPTURE\n",
            "\t- [[bob#^dup]]\n",
            "\t- [[bob#^other]]\n",
            "\t- [[bob#^dup]]\n",
        );
        let plan = plan_start_drop(contents, 1, &[1], &named("CAPTURE"), "=~1")
            .expect("in range");
        assert_eq!(
            plan.contents,
            concat!(
                "## Pomodoros\n",
                "- [ ] () — CAPTURE\n",
                "\t- [[bob#^other]]\n",
                "\t- [[bob#^dup]]\n",
            )
        );
        assert_eq!(
            plan.warnings,
            vec!["task 1 `[[bob#^dup]]` is still queued as task 3".to_string()]
        );
        // Dropping every copy warns about nothing.
        let plan =
            plan_start_drop(contents, 1, &[1, 3], &named("CAPTURE"), "=~1,3")
                .expect("in range");
        assert!(plan.warnings.is_empty());
        assert_eq!(plan.kept, vec![2]);
    }

    #[test]
    fn preserves_crlf() {
        let contents =
            "## Pomodoros\r\n- [ ] () — CAPTURE\r\n\t- [[bob#^first]]\r\n\t- [[bob#^second]]\r\n- [ ] () — SASE\r\n";
        let plan = plan_start_drop(contents, 1, &[1], &named("CAPTURE"), "=~1")
            .expect("in range");
        assert_eq!(
            plan.contents,
            "## Pomodoros\r\n- [ ] () — CAPTURE\r\n\t- [[bob#^second]]\r\n- [ ] () — SASE\r\n"
        );
    }

    #[test]
    fn dropped_last_line_without_final_newline_leaves_none() {
        let contents = "## Pomodoros\n- [ ] () — CAPTURE\n\t- [[bob#^first]]\n\t- [[bob#^second]]";
        let plan = plan_start_drop(contents, 1, &[2], &named("CAPTURE"), "=~2")
            .expect("in range");
        assert_eq!(
            plan.contents,
            "## Pomodoros\n- [ ] () — CAPTURE\n\t- [[bob#^first]]"
        );
        assert!(!plan.contents.ends_with('\n'));
    }

    #[test]
    fn dropped_middle_line_without_final_newline_keeps_structure() {
        let contents = "## Pomodoros\n- [ ] () — CAPTURE\n\t- [[bob#^first]]\n\t- [[bob#^second]]\n- [ ] () — SASE";
        let plan = plan_start_drop(contents, 1, &[1], &named("CAPTURE"), "=~1")
            .expect("in range");
        assert_eq!(
            plan.contents,
            "## Pomodoros\n- [ ] () — CAPTURE\n\t- [[bob#^second]]\n- [ ] () — SASE"
        );
    }
}
