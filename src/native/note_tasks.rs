use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
    sync::LazyLock,
};

use regex::Regex;
use sha2::{Digest, Sha256};

use super::{
    collect_done, markdown,
    task_status_hooks::{self, TasksSettings},
};

pub(crate) use super::task_status_hooks::TaskStatusType;

static INLINE_FIELD_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\[[A-Za-z][A-Za-z0-9_-]*::\s*[^\]]*\]")
        .expect("valid inline field regex")
});

pub(crate) type NoteTaskSettings = TasksSettings;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct NoteTask {
    pub(crate) line_index: usize,
    pub(crate) indentation: String,
    pub(crate) status_symbol: char,
    pub(crate) status_name: String,
    pub(crate) status_type: TaskStatusType,
    pub(crate) description: String,
    pub(crate) block_id: Option<String>,
    pub(crate) section: Option<String>,
    pub(crate) child_count: usize,
    pub(crate) block_end: usize,
    pub(crate) digest: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct BlockIdOccurrence {
    line_index: usize,
    excerpt: String,
    task_index: Option<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct NoteTaskScan {
    tasks: Vec<NoteTask>,
    block_ids: BTreeMap<String, Vec<BlockIdOccurrence>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BlockIdLookup<'a> {
    Found(&'a NoteTask),
    NotATask { line_index: usize, excerpt: &'a str },
    Duplicate(usize),
    Missing,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RefLookup<'a> {
    Found(&'a NoteTask),
    Stale,
    Ambiguous,
}

impl NoteTaskScan {
    pub(crate) fn open_tasks(&self) -> impl Iterator<Item = &NoteTask> {
        self.tasks.iter().filter(|task| task.status_type.is_open())
    }

    pub(crate) fn by_block_id(&self, id: &str) -> BlockIdLookup<'_> {
        let Some(occurrences) = self.block_ids.get(id) else {
            return BlockIdLookup::Missing;
        };
        if occurrences.len() > 1 {
            return BlockIdLookup::Duplicate(occurrences.len());
        }
        let occurrence = &occurrences[0];
        match occurrence.task_index {
            Some(index) => BlockIdLookup::Found(&self.tasks[index]),
            None => BlockIdLookup::NotATask {
                line_index: occurrence.line_index,
                excerpt: &occurrence.excerpt,
            },
        }
    }

    /// Resolve a task ref whose line component is one-based.
    pub(crate) fn by_ref(&self, line: usize, digest: &str) -> RefLookup<'_> {
        if line > 0
            && let Some(task) =
                self.tasks.iter().find(|task| task.line_index + 1 == line)
            && task.digest == digest
        {
            return RefLookup::Found(task);
        }

        let mut matches =
            self.tasks.iter().filter(|task| task.digest == digest);
        match (matches.next(), matches.next()) {
            (Some(task), None) => RefLookup::Found(task),
            (None, _) => RefLookup::Stale,
            (Some(_), Some(_)) => RefLookup::Ambiguous,
        }
    }

    pub(crate) fn suggest_block_id(&self, id: &str) -> Option<&str> {
        let task_ids = self
            .tasks
            .iter()
            .filter_map(|task| task.block_id.as_deref())
            .collect::<BTreeSet<_>>();
        if let Some(candidate) = task_ids
            .iter()
            .copied()
            .find(|candidate| candidate.eq_ignore_ascii_case(id))
        {
            return Some(candidate);
        }

        let requested = id.to_ascii_lowercase();
        let mut close = task_ids.into_iter().filter(|candidate| {
            bounded_levenshtein(&requested, &candidate.to_ascii_lowercase(), 2)
                .is_some()
        });
        match (close.next(), close.next()) {
            (Some(candidate), None) => Some(candidate),
            _ => None,
        }
    }
}

pub(crate) fn read_settings(bob_dir: &Path) -> NoteTaskSettings {
    task_status_hooks::read_tasks_settings(bob_dir)
}

pub(crate) fn scan(
    contents: &str,
    settings: &NoteTaskSettings,
) -> NoteTaskScan {
    let lines = note_lines(contents);
    let logical_lines = lines.iter().map(|line| line.text).collect::<Vec<_>>();
    let frontmatter_end =
        markdown::strictly_closed_frontmatter_end(&logical_lines);
    let scan_start = frontmatter_end.map_or(0, |end| end + 1);
    let fenced =
        markdown::fenced_lines(&logical_lines, scan_start..lines.len());
    let mut tasks = Vec::new();
    let mut block_ids = BTreeMap::<String, Vec<BlockIdOccurrence>>::new();
    let mut section = None;

    for (line_index, line) in lines.iter().enumerate() {
        if frontmatter_end.is_some_and(|end| line_index <= end)
            || fenced.contains(&line_index)
        {
            continue;
        }

        if let Some((_, title)) = markdown::atx_heading(line.text) {
            section = Some(title.to_string());
        }

        let parsed = parse_task_line(line.text, settings);
        let block_id = collect_done::trailing_block_id_in_line(line.text);
        let task_index = parsed.map(|parsed| {
            let (child_count, block_end) =
                task_block_extent(&lines, line_index, parsed.indentation.len());
            let task_index = tasks.len();
            tasks.push(NoteTask {
                line_index,
                indentation: parsed.indentation.to_string(),
                status_symbol: parsed.status_symbol,
                status_name: status_name(settings, parsed.status_symbol),
                status_type: settings
                    .status_types
                    .get(&parsed.status_symbol)
                    .copied()
                    .unwrap_or(TaskStatusType::Todo),
                description: clean_description(
                    parsed.body,
                    &settings.global_filter,
                    block_id.as_deref(),
                ),
                block_id: block_id.clone(),
                section: section.clone(),
                child_count,
                block_end,
                digest: task_digest(line.text),
            });
            task_index
        });

        if let Some(id) = block_id {
            block_ids.entry(id).or_default().push(BlockIdOccurrence {
                line_index,
                excerpt: line.text.trim().to_string(),
                task_index,
            });
        }
    }

    NoteTaskScan { tasks, block_ids }
}

#[derive(Debug, Clone, Copy)]
struct ParsedTask<'a> {
    indentation: &'a str,
    status_symbol: char,
    body: &'a str,
}

fn parse_task_line<'a>(
    line: &'a str,
    settings: &NoteTaskSettings,
) -> Option<ParsedTask<'a>> {
    let indentation_end = line
        .find(|character: char| !character.is_whitespace())
        .unwrap_or(line.len());
    let indentation = &line[..indentation_end];
    let after_open = line[indentation_end..].strip_prefix("- [")?;
    let mut chars = after_open.chars();
    let status_symbol = chars.next()?;
    let after_status = &after_open[status_symbol.len_utf8()..];
    let body = after_status.strip_prefix("] ")?;
    if !settings.global_filter.is_empty()
        && !body
            .split_whitespace()
            .any(|token| token == settings.global_filter)
    {
        return None;
    }
    Some(ParsedTask {
        indentation,
        status_symbol,
        body,
    })
}

fn status_name(settings: &NoteTaskSettings, symbol: char) -> String {
    settings
        .status_definitions
        .iter()
        .find(|definition| {
            let mut characters = definition.symbol.chars();
            characters.next() == Some(symbol) && characters.next().is_none()
        })
        .map(|definition| definition.name.clone())
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| "Unknown".to_string())
}

fn clean_description(
    body: &str,
    global_filter: &str,
    block_id: Option<&str>,
) -> String {
    let without_block = block_id
        .and_then(|id| body.trim_end().strip_suffix(&format!("^{id}")))
        .unwrap_or(body);
    let without_fields = INLINE_FIELD_RE.replace_all(without_block, " ");
    without_fields
        .split_whitespace()
        .filter(|token| global_filter.is_empty() || *token != global_filter)
        .collect::<Vec<_>>()
        .join(" ")
}

fn task_digest(line: &str) -> String {
    let digest = Sha256::digest(line.trim_end().as_bytes());
    hex::encode(digest)[..8].to_string()
}

#[derive(Debug, Clone, Copy)]
struct NoteLine<'a> {
    text: &'a str,
    end: usize,
}

fn note_lines(contents: &str) -> Vec<NoteLine<'_>> {
    let mut end = 0;
    contents
        .split_inclusive('\n')
        .map(|segment| {
            end += segment.len();
            let text = segment.strip_suffix('\n').unwrap_or(segment);
            let text = text.strip_suffix('\r').unwrap_or(text);
            NoteLine { text, end }
        })
        .collect()
}

fn task_block_extent(
    lines: &[NoteLine<'_>],
    task_index: usize,
    task_indentation: usize,
) -> (usize, usize) {
    let mut index = task_index + 1;
    let mut last_consumed = task_index;
    let mut child_count = 0;
    while index < lines.len() {
        if lines[index].text.trim().is_empty() {
            let next_nonblank = lines[index + 1..]
                .iter()
                .position(|line| !line.text.trim().is_empty())
                .map(|offset| index + 1 + offset);
            if next_nonblank.is_some_and(|next| {
                indentation_len(lines[next].text) > task_indentation
            }) {
                last_consumed = index;
                index += 1;
                continue;
            }
            break;
        }
        if indentation_len(lines[index].text) <= task_indentation {
            break;
        }
        last_consumed = index;
        child_count += 1;
        index += 1;
    }
    (child_count, lines[last_consumed].end)
}

fn indentation_len(line: &str) -> usize {
    line.find(|character: char| !character.is_whitespace())
        .unwrap_or(line.len())
}

fn bounded_levenshtein(left: &str, right: &str, limit: usize) -> Option<usize> {
    if left.len().abs_diff(right.len()) > limit {
        return None;
    }
    let mut previous = (0..=right.len()).collect::<Vec<_>>();
    let mut current = vec![0; right.len() + 1];
    for (left_index, left_byte) in left.bytes().enumerate() {
        current[0] = left_index + 1;
        for (right_index, right_byte) in right.bytes().enumerate() {
            current[right_index + 1] = (previous[right_index + 1] + 1)
                .min(current[right_index] + 1)
                .min(
                    previous[right_index]
                        + usize::from(left_byte != right_byte),
                );
        }
        if current
            .iter()
            .copied()
            .min()
            .is_some_and(|distance| distance > limit)
        {
            return None;
        }
        std::mem::swap(&mut previous, &mut current);
    }
    (previous[right.len()] <= limit).then_some(previous[right.len()])
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        path::{Path, PathBuf},
        sync::atomic::{AtomicUsize, Ordering},
    };

    use super::*;

    static TEMP_COUNTER: AtomicUsize = AtomicUsize::new(0);

    fn missing_settings() -> (TempDir, NoteTaskSettings) {
        let root = TempDir::new();
        let settings = read_settings(root.path());
        (root, settings)
    }

    fn task<'a>(scan: &'a NoteTaskScan, description: &str) -> &'a NoteTask {
        scan.tasks
            .iter()
            .find(|task| task.description == description)
            .unwrap_or_else(|| {
                panic!("missing task {description:?}: {scan:#?}")
            })
    }

    #[test]
    fn reads_real_statuses_and_missing_settings_fall_back_to_defaults() {
        let root = TempDir::new();
        let path = root
            .path()
            .join(".obsidian/plugins/obsidian-tasks-plugin/data.json");
        fs::create_dir_all(path.parent().expect("settings parent"))
            .expect("create settings parent");
        fs::write(
            path,
            r##"{
              "globalFilter": "#work",
              "statusSettings": {
                "coreStatuses": [
                  {"symbol":" ","name":"Todo","type":"TODO"},
                  {"symbol":"x","name":"Done","type":"DONE"},
                  {"symbol":"/","name":"In Progress","type":"IN_PROGRESS"},
                  {"symbol":"*","name":"Next","type":"ON_HOLD"},
                  {"symbol":"-","name":"Canceled","type":"CANCELLED"}
                ],
                "customStatuses": [
                  {"symbol":"?","name":"Blocked","type":"ON_HOLD"}
                ]
              }
            }"##,
        )
        .expect("write settings");
        let settings = read_settings(root.path());
        let configured_scan = scan(
            "- [ ] #work todo\n- [x] #work done\n- [-] #work canceled\n- [/] #work active\n- [*] #work next\n- [?] #work blocked\n- [Q] #work custom\n",
            &settings,
        );
        assert_eq!(
            configured_scan
                .open_tasks()
                .map(|task| task.status_symbol)
                .collect::<Vec<_>>(),
            vec![' ', '/', '*', '?', 'Q']
        );
        assert_eq!(task(&configured_scan, "blocked").status_name, "Blocked");
        assert_eq!(task(&configured_scan, "custom").status_name, "Unknown");

        let (_root, defaults) = missing_settings();
        let fallback = scan(
            "- [ ] #task todo\n- [x] #task done\n- [/] #task active\n- [*] #task next\n- [-] #task canceled\n",
            &defaults,
        );
        assert_eq!(
            fallback
                .open_tasks()
                .map(|task| task.status_symbol)
                .collect::<Vec<_>>(),
            vec![' ', '/', '*']
        );
    }

    #[test]
    fn gates_on_filter_and_cleans_descriptions_with_sections() {
        let (_root, settings) = missing_settings();
        let scan = scan(
            "- [ ] #task Above [created:: 2026-01-01]\n- [ ] Not a task\n## Work ##\n- [ ] #task   Keep #prj [[Link]] [due:: tomorrow] words   ^abc\n",
            &settings,
        );
        assert_eq!(scan.tasks.len(), 2);
        assert_eq!(task(&scan, "Above").section, None);
        let below = task(&scan, "Keep #prj [[Link]] words");
        assert_eq!(below.section.as_deref(), Some("Work"));
        assert_eq!(below.block_id.as_deref(), Some("abc"));
    }

    #[test]
    fn block_id_lookup_distinguishes_found_non_task_duplicate_and_missing() {
        let (_root, settings) = missing_settings();
        let scan = scan(
            "- [ ] #task Found ^found\nPlain line ^plain\n- [ ] #task First ^dupe\n- [x] #task Second ^dupe\n",
            &settings,
        );
        assert!(matches!(
            scan.by_block_id("found"),
            BlockIdLookup::Found(task) if task.description == "Found"
        ));
        assert!(matches!(
            scan.by_block_id("plain"),
            BlockIdLookup::NotATask {
                line_index: 1,
                excerpt: "Plain line ^plain"
            }
        ));
        assert_eq!(scan.by_block_id("dupe"), BlockIdLookup::Duplicate(2));
        assert_eq!(scan.by_block_id("absent"), BlockIdLookup::Missing);
    }

    #[test]
    fn computes_child_spans_for_mixed_indentation_blanks_and_eof() {
        let (_root, settings) = missing_settings();
        let contents = concat!(
            "- [ ] #task Tab\n",
            "\t- child\n",
            "\n",
            "\t\t- grandchild\n",
            "\n",
            "- [ ] #task Spaces\n",
            "  - child\n",
            "  - child two\n",
            "  - [ ] #task Nested\n",
            "    - nested child\n",
            "- [ ] #task EOF\n",
            "  - final child"
        );
        let scan = scan(contents, &settings);
        let tab = task(&scan, "Tab");
        assert_eq!(tab.child_count, 2);
        assert_eq!(
            &contents[..tab.block_end],
            "- [ ] #task Tab\n\t- child\n\n\t\t- grandchild\n"
        );
        let spaces = task(&scan, "Spaces");
        assert_eq!(spaces.child_count, 4);
        let nested = task(&scan, "Nested");
        assert_eq!(nested.indentation, "  ");
        assert_eq!(nested.child_count, 1);
        let eof = task(&scan, "EOF");
        assert_eq!(eof.child_count, 1);
        assert_eq!(eof.block_end, contents.len());
    }

    #[test]
    fn ignores_frontmatter_and_fenced_code_tasks() {
        let (_root, settings) = missing_settings();
        let scan = scan(
            "---\nexample: '- [ ] #task YAML'\n---\n# Tasks\n```md\n- [ ] #task Fence\n```\n- [ ] #task Real\n",
            &settings,
        );
        assert_eq!(scan.tasks.len(), 1);
        assert_eq!(scan.tasks[0].description, "Real");
        assert_eq!(scan.tasks[0].section.as_deref(), Some("Tasks"));
    }

    #[test]
    fn refs_resolve_exact_shifted_stale_and_ambiguous_tasks() {
        let (_root, settings) = missing_settings();
        let original =
            scan("Intro\n- [ ] #task Same\n- [ ] #task Unique\n", &settings);
        let same_digest = task(&original, "Same").digest.clone();
        let unique_digest = task(&original, "Unique").digest.clone();
        assert!(matches!(
            original.by_ref(2, &same_digest),
            RefLookup::Found(_)
        ));

        let shifted = scan(
            "New\nIntro\n- [ ] #task Same\n- [ ] #task Unique\n",
            &settings,
        );
        assert!(
            matches!(shifted.by_ref(2, &unique_digest), RefLookup::Found(task) if task.line_index == 3)
        );
        assert_eq!(shifted.by_ref(2, "deadbeef"), RefLookup::Stale);

        let ambiguous =
            scan("- [ ] #task Same\nOther\n- [ ] #task Same\n", &settings);
        assert_eq!(ambiguous.by_ref(2, &same_digest), RefLookup::Ambiguous);
    }

    #[test]
    fn suggests_case_matches_and_unique_nearby_block_ids_only() {
        let (_root, settings) = missing_settings();
        let scan = scan(
            "- [ ] #task One ^Alpha\n- [ ] #task Two ^bravo\n- [ ] #task Three ^brava\n",
            &settings,
        );
        assert_eq!(scan.suggest_block_id("alpha"), Some("Alpha"));
        assert_eq!(scan.suggest_block_id("Alphx"), Some("Alpha"));
        assert_eq!(scan.suggest_block_id("bravx"), None);
        assert_eq!(scan.suggest_block_id("nothing-close"), None);
    }

    struct TempDir {
        path: PathBuf,
    }

    impl TempDir {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "bob-note-tasks-{}-{}",
                std::process::id(),
                TEMP_COUNTER.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir_all(&path).expect("create temp vault");
            Self { path }
        }

        fn path(&self) -> &Path {
            &self.path
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            if let Err(error) = fs::remove_dir_all(&self.path) {
                eprintln!("failed to remove {}: {error}", self.path.display());
            }
        }
    }
}
