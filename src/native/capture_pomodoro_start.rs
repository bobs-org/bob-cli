//! Queued-task lineup for whole-item Pomodoro starts: list the started
//! entry's direct-child Task Links and resolve them read-only through the
//! batch planner's staged vault view. Resolution never fails the start and
//! never writes; unresolvable links become rows with `resolved: false` and
//! a `warning` in the close's wording.

use std::path::Path;

use super::{
    capture::{leading_spaces_or_tabs_len, line_spans},
    capture_pomodoro_close::{
        bare_embedded_link, bare_plain_link, close_task_text, lookup_task,
        range_is_struck, strikethrough_inner_spans, strip_pomodoro_markers,
        sub_bullet_range, wikilink_tokens, CloseVault,
    },
    markdown, note_tasks,
    vault_links::LinkResolution,
};

/// One listed Task Link on the started entry: the 1-based post-image
/// ledger line plus the parsed `[[path#^id]]` target.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct StartLink {
    pub ledger_line: usize,
    pub embedded: bool,
    pub path_part: String,
    pub block_id: String,
    pub block_link: String,
}

/// One resolved lineup row, following the close row's explicit-null
/// convention: unresolved rows carry `None` fields plus a `warning`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct StartTaskRow {
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
