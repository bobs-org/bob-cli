//! Watching tasks: open `#task` rows that link a thread's mail note.
//!
//! A task watches a thread when its box is `[ ]`, `[*]`, `[/]`, or `[?]`,
//! its line carries the exact `#task` token, and it links the thread's
//! indexed mail note either on its own line or in a direct child bullet
//! that holds only that link. Deeper descendants, prose child bullets,
//! mentions elsewhere in a note, closed rows, frontmatter, and fenced code
//! never watch. Links resolve to note identity (an exact vault path, then a
//! unique case-insensitive basename), never to the display label; an
//! ambiguous basename or a mail note whose thread identity is duplicated is
//! skipped with a body-free warning.
//!
//! The walk shares `bob gmail link -a`'s exclusions ([`vault_walk`]) and
//! only reads: source bytes are never rewritten here.

use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
};

use super::{
    super::{
        note::index::MailIndex,
        vault_walk::{self, WalkedNote},
        GmailError,
    },
    ThreadKey,
};
use crate::native::{
    collect_done, markdown,
    task_dependencies::raw_wikilink_spans,
    vault_links::{target_to_markdown_path, LinkResolution, VaultLinkResolver},
};

/// Checkbox statuses that keep a task open for watching.
pub(crate) const OPEN_STATUSES: [char; 4] = [' ', '*', '/', '?'];

/// The tag token a watching task must carry exactly.
const TASK_TAG: &str = "#task";

/// Every watching task in the vault, with a per-thread index. Neither
/// mapping is one-to-one: one task may link several threads, and one
/// thread may be linked by several tasks.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct WatchScan {
    tasks: Vec<WatchingTask>,
    by_thread: BTreeMap<ThreadKey, Vec<usize>>,
    warnings: Vec<String>,
}

/// One open task that watches at least one thread.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct WatchingTask {
    /// Vault-relative note path, `/`-separated.
    pub(crate) path: String,
    /// 1-based line number of the task row.
    pub(crate) line: usize,
    /// The task row's exact source text, without its line ending.
    pub(crate) text: String,
    /// The row's trailing `^block-id`, when present.
    pub(crate) block_id: Option<String>,
    /// The checkbox status: one of [`OPEN_STATUSES`].
    pub(crate) status: char,
    /// Watched threads in source order, each once.
    pub(crate) targets: Vec<WatchTarget>,
}

/// One thread a task watches, through one of its links.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct WatchTarget {
    pub(crate) key: ThreadKey,
    /// Vault-relative mail-note path, `/`-separated.
    pub(crate) note: String,
    /// Where the (first) link to the mail note sits.
    pub(crate) placement: LinkPlacement,
}

/// Where a watching task's link to a mail note sits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LinkPlacement {
    /// On the task row itself.
    TaskLine,
    /// In a direct link-only child bullet at this 1-based line.
    ChildBullet { line: usize },
}

impl WatchScan {
    fn new(mut tasks: Vec<WatchingTask>, warnings: Vec<String>) -> Self {
        tasks.sort_by(|left, right| {
            left.path.cmp(&right.path).then(left.line.cmp(&right.line))
        });
        let mut by_thread: BTreeMap<ThreadKey, Vec<usize>> = BTreeMap::new();
        for (index, task) in tasks.iter().enumerate() {
            for target in &task.targets {
                by_thread.entry(target.key.clone()).or_default().push(index);
            }
        }
        Self {
            tasks,
            by_thread,
            warnings,
        }
    }

    /// Every watching task, sorted by path, then line.
    pub(crate) fn tasks(&self) -> &[WatchingTask] {
        &self.tasks
    }

    /// The open tasks watching `key`, sorted by path, then line.
    pub(crate) fn watchers<'s>(
        &'s self,
        key: &ThreadKey,
    ) -> impl Iterator<Item = &'s WatchingTask> + use<'s> {
        self.by_thread
            .get(key)
            .map(Vec::as_slice)
            .unwrap_or_default()
            .iter()
            .map(|&index| &self.tasks[index])
    }

    /// Whether any open task links the thread's mail note.
    pub(crate) fn is_watched(&self, key: &ThreadKey) -> bool {
        self.by_thread.contains_key(key)
    }

    /// Every linked thread (enrolled or not), sorted by key.
    pub(crate) fn threads(&self) -> impl Iterator<Item = &ThreadKey> {
        self.by_thread.keys()
    }

    /// Body-free warnings about links that could not name one thread.
    pub(crate) fn warnings(&self) -> &[String] {
        &self.warnings
    }

    #[cfg(test)]
    pub(crate) fn from_tasks_for_test(tasks: Vec<WatchingTask>) -> Self {
        Self::new(tasks, Vec::new())
    }

    /// Projected scan for the watch transaction, including newly filed rows.
    /// Test helper shares this constructor.
    pub(crate) fn from_tasks(tasks: Vec<WatchingTask>) -> Self {
        Self::new(tasks, Vec::new())
    }
}

impl WatchingTask {
    /// The mail-note path this task links for `key`.
    pub(crate) fn target_note(&self, key: &ThreadKey) -> Option<&str> {
        self.targets
            .iter()
            .find(|target| &target.key == key)
            .map(|target| target.note.as_str())
    }
}

/// Walk the vault (minus the mail folder and `bob gmail link -a`'s other
/// exclusions) for watching tasks. `index` is the mail-note index of
/// `mail_folder`; only its uniquely identified notes can be watched.
pub(crate) fn scan_watching_tasks(
    vault: &Path,
    mail_folder: &Path,
    index: &MailIndex,
) -> Result<WatchScan, GmailError> {
    let mut links = MailLinks::new(vault, index);
    let mut tasks = Vec::new();
    vault_walk::visit_vault_notes(
        vault,
        mail_folder,
        &mut |note: WalkedNote<'_>| {
            if !note.text.contains(TASK_TAG) || !note.text.contains("[[") {
                return Ok(());
            }
            let relative = slashes(note.relative);
            tasks.extend(scan_note(&relative, note.text, &mut links));
            Ok(())
        },
    )?;
    Ok(WatchScan::new(tasks, links.warnings()))
}

/// Resolves wikilink targets to indexed mail-note identities.
struct MailLinks {
    by_path: BTreeMap<PathBuf, ThreadKey>,
    duplicate_paths: BTreeSet<PathBuf>,
    /// Lowercase basename stems of every mail note: a link whose basename
    /// is not one of these can never resolve to a mail note, so it skips
    /// the filesystem entirely.
    stems: BTreeSet<String>,
    resolver: VaultLinkResolver,
    ambiguous: usize,
    duplicated: usize,
}

impl MailLinks {
    fn new(vault: &Path, index: &MailIndex) -> Self {
        let mut by_path = BTreeMap::new();
        let mut stems = BTreeSet::new();
        for entry in index.by_identity.values() {
            let Some(key) = ThreadKey::new(&entry.account, &entry.thread_id)
            else {
                continue;
            };
            let path = PathBuf::from(&entry.path);
            if let Some(stem) = lower_stem(&entry.path) {
                stems.insert(stem);
            }
            by_path.insert(path, key);
        }
        let mut duplicate_paths = BTreeSet::new();
        for (_, _, paths) in &index.duplicates {
            for path in paths {
                if let Some(stem) = lower_stem(path) {
                    stems.insert(stem);
                }
                duplicate_paths.insert(PathBuf::from(path));
            }
        }
        Self {
            by_path,
            duplicate_paths,
            stems,
            resolver: VaultLinkResolver::new(vault),
            ambiguous: 0,
            duplicated: 0,
        }
    }

    /// The thread and mail-note path a wikilink's inner text names.
    fn resolve(
        &mut self,
        current: &Path,
        inner: &str,
    ) -> Option<(ThreadKey, String)> {
        let target = link_target(inner)?;
        let stem = lower_stem(target)?;
        if !self.stems.contains(&stem) {
            return None;
        }
        match self.resolver.resolve(current, target) {
            LinkResolution::Found(path) => {
                if let Some(key) = self.by_path.get(&path) {
                    return Some((key.clone(), slashes(&path)));
                }
                if self.duplicate_paths.contains(&path) {
                    self.duplicated += 1;
                }
                None
            }
            LinkResolution::Ambiguous => {
                self.ambiguous += 1;
                None
            }
            LinkResolution::Missing => None,
        }
    }

    fn warnings(&self) -> Vec<String> {
        let mut warnings = Vec::new();
        if self.ambiguous > 0 {
            warnings.push(format!(
                "skipped {} task links to a mail note name that matches \
                 several notes; link the mail note by its full path",
                self.ambiguous
            ));
        }
        if self.duplicated > 0 {
            warnings.push(format!(
                "skipped {} task links to mail notes that share one Gmail \
                 thread; keep one note per thread",
                self.duplicated
            ));
        }
        warnings
    }
}

/// Watching tasks in one note's text. `relative` is the note's
/// vault-relative path.
fn scan_note(
    relative: &str,
    text: &str,
    links: &mut MailLinks,
) -> Vec<WatchingTask> {
    let lines: Vec<&str> = text
        .split_inclusive('\n')
        .map(|line| markdown::split_line_ending(line).0)
        .collect();
    let body_start = markdown::strictly_closed_frontmatter_end(&lines)
        .map_or(0, |end| end + 1);
    let fenced = fenced_lines(&lines, body_start);
    let hidden = |index: usize| index < body_start || fenced.contains(&index);
    let current = Path::new(relative);

    let mut tasks = Vec::new();
    for (index, line) in lines.iter().enumerate() {
        if hidden(index) {
            continue;
        }
        let Some(task) = parse_open_task(line) else {
            continue;
        };
        let mut targets: Vec<WatchTarget> = Vec::new();
        let mut add = |found: Option<(ThreadKey, String)>, placement| {
            if let Some((key, note)) = found
                && !targets.iter().any(|target| target.key == key)
            {
                targets.push(WatchTarget {
                    key,
                    note,
                    placement,
                });
            }
        };
        for inner in line_wikilinks(line) {
            add(links.resolve(current, inner), LinkPlacement::TaskLine);
        }
        for child in direct_children(&lines, index, task.width, &hidden) {
            if let Some(inner) = link_only_bullet(lines[child]) {
                add(
                    links.resolve(current, inner),
                    LinkPlacement::ChildBullet { line: child + 1 },
                );
            }
        }
        if targets.is_empty() {
            continue;
        }
        tasks.push(WatchingTask {
            path: relative.to_string(),
            line: index + 1,
            text: (*line).to_string(),
            block_id: collect_done::trailing_block_id_in_line(line),
            status: task.status,
            targets,
        });
    }
    tasks
}

/// Fenced-code line indexes from `start`, including fences nested in list
/// items at any indentation, so code examples under a task never watch.
fn fenced_lines(lines: &[&str], start: usize) -> BTreeSet<usize> {
    let mut fenced = BTreeSet::new();
    let mut open = None;
    for (index, line) in lines.iter().enumerate().skip(start) {
        let line = line.trim_start();
        if let Some(marker) = open {
            fenced.insert(index);
            if markdown::closes_fence(line, marker) {
                open = None;
            }
        } else if let Some(marker) = markdown::fence_marker(line) {
            fenced.insert(index);
            open = Some(marker);
        }
    }
    fenced
}

struct OpenTask {
    width: usize,
    status: char,
}

/// An open `- [s] … #task …` row.
fn parse_open_task(line: &str) -> Option<OpenTask> {
    let indentation_end = line
        .find(|character: char| !character.is_whitespace())
        .unwrap_or(line.len());
    let after_open = line[indentation_end..].strip_prefix("- [")?;
    let status = after_open.chars().next()?;
    if !OPEN_STATUSES.contains(&status) {
        return None;
    }
    let body = after_open[status.len_utf8()..].strip_prefix("] ")?;
    if !body.split_whitespace().any(|token| token == TASK_TAG) {
        return None;
    }
    Some(OpenTask {
        width: indent_width(line),
        status,
    })
}

/// Inner text of every non-embedded wikilink on a line, skipping inline
/// code spans.
fn line_wikilinks(line: &str) -> impl Iterator<Item = &str> {
    raw_wikilink_spans(line)
        .into_iter()
        .filter(|span| !line[..span.open].ends_with('!'))
        .map(|span| &line[span.open + 2..span.end - 2])
}

/// Line indexes of the task's direct child list items: items inside the
/// task's block with no other list item of the block between them and the
/// task. Blank lines stay in the block while the next non-blank line is
/// indented deeper than the task; hidden (fenced) lines never count.
fn direct_children(
    lines: &[&str],
    task_index: usize,
    task_width: usize,
    hidden: &dyn Fn(usize) -> bool,
) -> Vec<usize> {
    let mut children = Vec::new();
    let mut open_items: Vec<usize> = Vec::new();
    let mut index = task_index + 1;
    while index < lines.len() {
        let line = lines[index];
        if line.trim().is_empty() {
            let next = lines[index + 1..]
                .iter()
                .position(|line| !line.trim().is_empty())
                .map(|offset| index + 1 + offset);
            if next.is_some_and(|next| indent_width(lines[next]) > task_width) {
                index += 1;
                continue;
            }
            break;
        }
        let width = indent_width(line);
        if width <= task_width {
            break;
        }
        if !hidden(index) && is_list_item(line) {
            while open_items.last().is_some_and(|open| *open >= width) {
                open_items.pop();
            }
            if open_items.is_empty() {
                children.push(index);
            }
            open_items.push(width);
        }
        index += 1;
    }
    children
}

/// The inner text of a bullet that holds exactly one wikilink.
fn link_only_bullet(line: &str) -> Option<&str> {
    let content = bullet_content(line)?.trim();
    let inner = content.strip_prefix("[[")?.strip_suffix("]]")?;
    (!inner.contains("[[") && !inner.contains("]]")).then_some(inner)
}

/// Text after a `-`, `*`, or `+` bullet marker and its whitespace.
fn bullet_content(line: &str) -> Option<&str> {
    let trimmed = line.trim_start();
    let rest = trimmed.strip_prefix(['-', '*', '+'])?;
    rest.starts_with([' ', '\t']).then_some(rest)
}

fn is_list_item(line: &str) -> bool {
    if bullet_content(line).is_some() {
        return true;
    }
    let trimmed = line.trim_start();
    let digits = trimmed.bytes().take_while(u8::is_ascii_digit).count();
    digits > 0
        && trimmed[digits..].starts_with(['.', ')'])
        && trimmed[digits + 1..].starts_with([' ', '\t'])
}

/// Leading indentation in columns, with tabs stopping every 4 columns.
fn indent_width(line: &str) -> usize {
    let mut width = 0;
    for character in line.chars() {
        match character {
            ' ' => width += 1,
            '\t' => width += 4 - width % 4,
            _ => break,
        }
    }
    width
}

/// The note a wikilink's inner text points at: the alias, any `#heading`
/// or `#^block` subpath, a table-escaped pipe, and a vault-root `/` are
/// dropped. Same-note links (`[[#heading]]`) name no other note.
fn link_target(inner: &str) -> Option<&str> {
    let before_alias = inner.split('|').next().unwrap_or("");
    let before_alias = before_alias.strip_suffix('\\').unwrap_or(before_alias);
    let target = before_alias.split('#').next().unwrap_or("").trim();
    let target = target.strip_prefix('/').unwrap_or(target);
    (!target.is_empty()).then_some(target)
}

fn lower_stem(target: &str) -> Option<String> {
    let path = target_to_markdown_path(target)?;
    let stem = path.file_stem()?.to_str()?;
    Some(stem.to_lowercase())
}

fn slashes(path: &Path) -> String {
    path.components()
        .map(|component| component.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;
    use crate::native::gmail::note::index::MailIndexEntry;

    const ACCOUNT: &str = "me@example.com";

    struct Vault {
        _temp: tempfile::TempDir,
        root: PathBuf,
        index: MailIndex,
    }

    impl Vault {
        fn new() -> Self {
            let temp = tempfile::tempdir().unwrap();
            let root = temp.path().to_path_buf();
            Self {
                _temp: temp,
                root,
                index: MailIndex::default(),
            }
        }

        fn write(&self, relative: &str, text: &str) {
            let path = self.root.join(relative);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, text).unwrap();
        }

        /// Write a mail note and index it.
        fn mail(&mut self, relative: &str, account: &str, thread: &str) {
            self.write(relative, "---\ntype: \"[[mail]]\"\n---\nbody\n");
            self.index.by_identity.insert(
                (account.to_ascii_lowercase(), thread.to_ascii_lowercase()),
                MailIndexEntry {
                    account: account.to_string(),
                    thread_id: thread.to_ascii_lowercase(),
                    path: relative.to_string(),
                    title: "Lease".to_string(),
                    who: "Dana".to_string(),
                    label: "✉️ Lease · Dana".to_string(),
                    started: String::new(),
                    updated: String::new(),
                    messages: 1,
                    turn: None,
                    turn_since: None,
                },
            );
        }

        fn scan(&self) -> WatchScan {
            scan_watching_tasks(&self.root, Path::new("mail"), &self.index)
                .unwrap()
        }
    }

    fn key(thread: &str) -> ThreadKey {
        ThreadKey::new(ACCOUNT, thread).unwrap()
    }

    fn lines(scan: &WatchScan, thread: &str) -> Vec<(String, usize)> {
        scan.watchers(&key(thread))
            .map(|task| (task.path.clone(), task.line))
            .collect()
    }

    fn standard_vault() -> Vault {
        let mut vault = Vault::new();
        vault.mail("mail/2026/lease.md", ACCOUNT, "aa01");
        vault.mail("mail/2026/quote.md", ACCOUNT, "bb02");
        vault.mail("mail/2026/kickoff.md", ACCOUNT, "cc03");
        vault
    }

    #[test]
    fn own_line_and_link_only_children_watch_with_exact_tags_and_open_boxes() {
        let vault = standard_vault();
        vault.write(
            "job.md",
            "---\nparent: \"[[area]]\"\n---\n# Job\n\n## Tasks\n\
             - [ ] #task Renew [[mail/2026/lease|✉️ Lease · Dana]] [created:: 2026-11-07] ^lease\n\
             - [*] #task Next one [[mail/2026/quote.md|label]]\n\
             - [/] #task Started [[mail/2026/kickoff]]\n\
             - [?] #task Waiting on someone\n\
             \t- [[mail/2026/lease|✉️ Lease · Dana]]\n\
             - [x] #task Done [[mail/2026/quote]]\n\
             - [-] #task Cancelled [[mail/2026/quote]]\n\
             - [>] #task Deferred custom [[mail/2026/quote]]\n\
             - [ ] #tasks Not the tag [[mail/2026/quote]]\n\
             - [ ] #task/sub Not the tag [[mail/2026/quote]]\n\
             - [ ] `#task` code tag [[mail/2026/quote]]\n\
             - [ ] no tag [[mail/2026/quote]]\n\
             * [ ] #task star bullet is not a task row [[mail/2026/quote]]\n",
        );
        let before = fs::read(vault.root.join("job.md")).unwrap();
        let scan = vault.scan();
        assert_eq!(fs::read(vault.root.join("job.md")).unwrap(), before);

        let tasks = scan.tasks();
        assert_eq!(tasks.len(), 4, "{tasks:#?}");
        assert_eq!(tasks[0].line, 7);
        assert_eq!(tasks[0].status, ' ');
        assert_eq!(tasks[0].block_id.as_deref(), Some("lease"));
        assert_eq!(
            tasks[0].text,
            "- [ ] #task Renew [[mail/2026/lease|✉️ Lease · Dana]] \
             [created:: 2026-11-07] ^lease"
        );
        assert_eq!(tasks[0].targets[0].note, "mail/2026/lease.md");
        assert_eq!(tasks[0].targets[0].placement, LinkPlacement::TaskLine);
        assert_eq!(tasks[1].status, '*');
        assert_eq!(tasks[2].status, '/');
        assert_eq!(tasks[3].status, '?');
        assert_eq!(tasks[3].line, 10);
        assert_eq!(
            tasks[3].targets[0].placement,
            LinkPlacement::ChildBullet { line: 11 }
        );

        assert_eq!(
            lines(&scan, "aa01"),
            vec![("job.md".to_string(), 7), ("job.md".to_string(), 10)]
        );
        assert_eq!(lines(&scan, "bb02"), vec![("job.md".to_string(), 8)]);
        assert_eq!(scan.watchers(&key("cc03")).count(), 1);
        assert!(scan.warnings().is_empty());
    }

    #[test]
    fn prose_children_deeper_descendants_and_mentions_do_not_watch() {
        let vault = standard_vault();
        vault.write(
            "notes.md",
            "# Notes\n\
             See [[mail/2026/lease]] for context.\n\
             - a plain bullet [[mail/2026/lease]]\n\
             - [ ] #task Prose child\n\
             \t- read [[mail/2026/lease]] first\n\
             - [ ] #task Deeper descendant\n\
             \t- context\n\
             \t\t- [[mail/2026/quote]]\n\
             - [ ] #task Embed only\n\
             \t- ![[mail/2026/kickoff]]\n\
             - [ ] #task Two links in one child\n\
             \t- [[mail/2026/lease]] [[mail/2026/quote]]\n\
             - [ ] #task Inline code `[[mail/2026/lease]]`\n\
             - [ ] #task Sibling after the block\n\
             - [[mail/2026/kickoff]]\n\
             - [ ] #task Link-only child with a block id\n\
             \t- [[mail/2026/quote]] ^child\n",
        );
        let scan = vault.scan();
        assert!(scan.tasks().is_empty(), "{:#?}", scan.tasks());
    }

    #[test]
    fn code_fences_and_frontmatter_never_watch() {
        let vault = standard_vault();
        vault.write(
            "howto.md",
            "---\nexample: \"- [ ] #task [[mail/2026/lease]]\"\n---\n\
             ```markdown\n\
             - [ ] #task Example [[mail/2026/lease]]\n\
             ```\n\
             - [ ] #task Real one\n\
             \t```\n\
             \t- [[mail/2026/quote]]\n\
             \t```\n\
             \t- [[mail/2026/kickoff]]\n",
        );
        let scan = vault.scan();
        assert_eq!(scan.tasks().len(), 1);
        let task = &scan.tasks()[0];
        assert_eq!(task.line, 7);
        assert_eq!(task.targets.len(), 1);
        assert_eq!(task.targets[0].key, key("cc03"));
        assert_eq!(
            task.targets[0].placement,
            LinkPlacement::ChildBullet { line: 11 }
        );
    }

    #[test]
    fn direct_children_survive_blank_lines_and_sibling_nesting() {
        let vault = standard_vault();
        vault.write(
            "a.md",
            "- [ ] #task Parent\n\
             \t- first child\n\
             \t\t- grandchild [[mail/2026/quote]]\n\
             \n\
             \t- [[mail/2026/lease]]\n\
             \t- [ ] #task Subtask\n\
             \t\t- [[mail/2026/kickoff]]\n\
             \n\
             - [ ] #task Spaces\n    - [[mail/2026/quote]]\n",
        );
        let scan = vault.scan();
        let summary: Vec<(usize, Vec<String>)> = scan
            .tasks()
            .iter()
            .map(|task| {
                (
                    task.line,
                    task.targets
                        .iter()
                        .map(|target| target.key.thread_id().to_string())
                        .collect(),
                )
            })
            .collect();
        assert_eq!(
            summary,
            vec![
                (1, vec!["aa01".to_string()]),
                (6, vec!["cc03".to_string()]),
                (9, vec!["bb02".to_string()]),
            ]
        );
    }

    #[test]
    fn multi_thread_tasks_and_multi_task_threads_keep_every_mapping() {
        let vault = standard_vault();
        vault.write(
            "job.md",
            "- [ ] #task Both [[mail/2026/lease]] and [[mail/2026/quote]]\n\
             \t- [[mail/2026/kickoff]]\n\
             \t- [[mail/2026/lease|again]]\n",
        );
        vault.write("cash.md", "- [/] #task Pay [[mail/2026/lease]]\n");
        vault.write("body.md", "- [ ] #task Visit\n\t- [[lease]]\n");
        let scan = vault.scan();
        let both = &scan.tasks()[2];
        assert_eq!(both.path, "job.md");
        let threads: Vec<&str> = both
            .targets
            .iter()
            .map(|target| target.key.thread_id())
            .collect();
        assert_eq!(threads, vec!["aa01", "bb02", "cc03"]);
        assert!(both.targets.iter().any(|target| target.key == key("cc03")));
        assert_eq!(
            lines(&scan, "aa01"),
            vec![
                ("body.md".to_string(), 1),
                ("cash.md".to_string(), 1),
                ("job.md".to_string(), 1),
            ]
        );
        let watched: Vec<&str> =
            scan.threads().map(|key| key.thread_id()).collect();
        assert_eq!(watched, vec!["aa01", "bb02", "cc03"]);
    }

    #[test]
    fn identities_normalize_and_ambiguous_or_duplicate_links_are_skipped() {
        let mut vault = Vault::new();
        vault.mail("mail/2026/lease.md", "Me@Example.COM", "AA01");
        vault.mail("mail/2026/notes.md", ACCOUNT, "bb02");
        // A non-mail note shares the second mail note's basename.
        vault.write("projects/notes.md", "plain\n");
        vault.write("mail/2026/dup-a.md", "dup\n");
        vault.write("mail/2026/dup-b.md", "dup\n");
        vault.index.duplicates.push((
            ACCOUNT.to_string(),
            "cc03".to_string(),
            vec![
                "mail/2026/dup-a.md".to_string(),
                "mail/2026/dup-b.md".to_string(),
            ],
        ));
        vault.write(
            "job.md",
            "- [ ] #task Basename [[LEASE|label]]\n\
             - [ ] #task Ambiguous [[notes]]\n\
             - [ ] #task Exact [[mail/2026/notes]]\n\
             - [ ] #task Duplicate identity [[mail/2026/dup-a]]\n\
             - [ ] #task Root slash and heading [[/mail/2026/lease#Notes|x]]\n\
             - [ ] #task Missing [[mail/2026/gone]]\n",
        );
        let scan = vault.scan();
        let found: Vec<(usize, String)> = scan
            .tasks()
            .iter()
            .map(|task| (task.line, task.targets[0].key.thread_id().into()))
            .collect();
        assert_eq!(
            found,
            vec![
                (1, "aa01".to_string()),
                (3, "bb02".to_string()),
                (5, "aa01".to_string()),
            ]
        );
        assert_eq!(scan.tasks()[0].targets[0].key, key("aa01"));
        assert_eq!(scan.warnings().len(), 2, "{:?}", scan.warnings());
        assert!(scan.warnings()[0].starts_with("skipped 1 task links"));
        assert!(scan.warnings()[1].contains("share one Gmail thread"));
    }

    #[test]
    fn excluded_folders_and_zorg_logs_are_not_scanned() {
        let vault = standard_vault();
        let row = "- [ ] #task Watch [[mail/2026/lease]]\n";
        for excluded in [
            "done/old.md",
            "_generated/report.md",
            "_conflicts/job.md",
            ".obsidian/x.md",
            ".bob/gmail/x.md",
            ".hidden/x.md",
            "mail/2026/inside-mail.md",
        ] {
            vault.write(excluded, row);
        }
        vault.write(
            "log.md",
            &format!("---\ngenerated_from_zorg: true\n---\n{row}"),
        );
        vault.write("kept.md", row);
        let scan = vault.scan();
        assert_eq!(lines(&scan, "aa01"), vec![("kept.md".to_string(), 1)]);
    }

    #[test]
    fn moved_tasks_are_found_wherever_they_live_and_closed_ones_drop() {
        let vault = standard_vault();
        vault.write("gmail_inbox.md", "- [ ] #task [[mail/2026/lease]]\n");
        assert_eq!(
            lines(&vault.scan(), "aa01"),
            vec![("gmail_inbox.md".to_string(), 1)]
        );
        // Ctrl+Shift+M: the row moves to a nested project note.
        vault.write("gmail_inbox.md", "# Inbox\n");
        vault.write(
            "projects/job/apply.md",
            "## Tasks\n\n- [ ] #task [[mail/2026/lease]]\n",
        );
        assert_eq!(
            lines(&vault.scan(), "aa01"),
            vec![("projects/job/apply.md".to_string(), 3)]
        );
        // Closing it ends the watch.
        vault.write(
            "projects/job/apply.md",
            "## Tasks\n\n- [x] #task [[mail/2026/lease]] ✅ 2026-11-07\n",
        );
        assert!(!vault.scan().is_watched(&key("aa01")));
    }

    #[test]
    fn crlf_lines_keep_their_text_without_the_terminator() {
        let vault = standard_vault();
        vault.write(
            "job.md",
            "- [ ] #task Windows\r\n\t- [[mail/2026/quote]]\r\n",
        );
        let scan = vault.scan();
        assert_eq!(scan.tasks()[0].text, "- [ ] #task Windows");
        assert!(scan.is_watched(&key("bb02")));
    }

    #[test]
    fn link_targets_drop_alias_subpath_escape_and_root() {
        assert_eq!(link_target("a/b|label"), Some("a/b"));
        assert_eq!(link_target("a/b\\|label"), Some("a/b"));
        assert_eq!(link_target("a/b#^block|x"), Some("a/b"));
        assert_eq!(link_target("/a/b.md"), Some("a/b.md"));
        assert_eq!(link_target("#heading"), None);
        assert_eq!(link_target(""), None);
    }

    #[test]
    fn indent_width_expands_tabs_to_four_columns() {
        assert_eq!(indent_width("- x"), 0);
        assert_eq!(indent_width("\t- x"), 4);
        assert_eq!(indent_width("  \t- x"), 4);
        assert_eq!(indent_width("    \t- x"), 8);
    }
}
