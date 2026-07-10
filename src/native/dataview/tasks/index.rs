use std::{
    collections::BTreeMap,
    ffi::OsStr,
    fs,
    path::{Path, PathBuf},
};

use chrono::NaiveDateTime;

use super::{
    settings::TasksSettings,
    task::{parse_list_line, StatusRegistry, Task, TaskFile},
};
use crate::native::dataview::DataviewError;

#[derive(Debug)]
pub(super) struct TaskIndex {
    pub(super) tasks: Vec<Task>,
}

impl TaskIndex {
    pub(super) fn read(
        vault: &Path,
        settings: &TasksSettings,
        now: NaiveDateTime,
    ) -> Result<Self, DataviewError> {
        let statuses = StatusRegistry::new(&settings.status_settings);
        let mut markdown_paths = Vec::new();
        collect_markdown_paths(vault, &mut markdown_paths)?;
        markdown_paths.sort();

        let mut tasks = Vec::new();
        for path in markdown_paths {
            let contents = fs::read_to_string(&path).map_err(|error| {
                DataviewError::NativeVaultRead {
                    path: path.clone(),
                    error,
                }
            })?;
            let relative_path = path.strip_prefix(vault).map_err(|error| {
                DataviewError::TasksQuery {
                    message: format!(
                        "failed to make {} vault-relative: {error}",
                        path.display()
                    ),
                }
            })?;
            let relative_path =
                relative_path.to_string_lossy().replace('\\', "/");
            parse_file(
                &contents,
                TaskFile::new(relative_path),
                settings,
                &statuses,
                now,
                &mut tasks,
            );
        }
        apply_dependencies(&mut tasks);
        Ok(Self { tasks })
    }
}

fn collect_markdown_paths(
    directory: &Path,
    paths: &mut Vec<PathBuf>,
) -> Result<(), DataviewError> {
    let entries = fs::read_dir(directory).map_err(|error| {
        DataviewError::NativeVaultRead {
            path: directory.to_path_buf(),
            error,
        }
    })?;

    for entry in entries {
        let entry = entry.map_err(|error| DataviewError::NativeVaultRead {
            path: directory.to_path_buf(),
            error,
        })?;
        let path = entry.path();
        let file_type = entry.file_type().map_err(|error| {
            DataviewError::NativeVaultRead {
                path: path.clone(),
                error,
            }
        })?;
        if file_type.is_dir() {
            if !entry.file_name().to_string_lossy().starts_with('.') {
                collect_markdown_paths(&path, paths)?;
            }
        } else if file_type.is_file() && is_markdown(&path) {
            paths.push(path);
        }
    }
    Ok(())
}

fn is_markdown(path: &Path) -> bool {
    path.extension()
        .and_then(OsStr::to_str)
        .is_some_and(|extension| extension.eq_ignore_ascii_case("md"))
}

#[derive(Debug)]
struct ListNode {
    line_number: usize,
    depth: usize,
    parent: Option<usize>,
    children: Vec<usize>,
    task_index: Option<usize>,
}

fn parse_file(
    contents: &str,
    file: TaskFile,
    settings: &TasksSettings,
    statuses: &StatusRegistry,
    now: NaiveDateTime,
    all_tasks: &mut Vec<Task>,
) {
    let task_start = all_tasks.len();
    let mut nodes = Vec::<ListNode>::new();
    let mut stack = Vec::<usize>::new();
    let mut current_heading = None;
    let mut previous_plain_line = None::<String>;
    let mut fence = None;
    let mut in_frontmatter = false;
    let mut in_comment = false;

    for (line_number, line) in contents.lines().enumerate() {
        let trimmed = line.trim();
        if line_number == 0 && trimmed == "---" {
            in_frontmatter = true;
            previous_plain_line = None;
            continue;
        }
        if in_frontmatter {
            if matches!(trimmed, "---" | "...") {
                in_frontmatter = false;
            }
            continue;
        }
        if in_comment {
            if trimmed.contains("-->") {
                in_comment = false;
            }
            continue;
        }
        if trimmed.starts_with("<!--") {
            in_comment = !trimmed.contains("-->");
            continue;
        }

        if update_fence(line, &mut fence) {
            stack.clear();
            previous_plain_line = None;
            continue;
        }
        if fence.is_some() {
            continue;
        }

        if let Some(heading) = parse_atx_heading(line) {
            current_heading = Some(heading);
            stack.clear();
            previous_plain_line = None;
            continue;
        }
        if is_setext_underline(trimmed)
            && let Some(heading) = previous_plain_line.take()
        {
            current_heading = Some(heading);
            stack.clear();
            continue;
        }

        let Some(list) = parse_list_line(line) else {
            if trimmed.is_empty() {
                previous_plain_line = None;
            } else {
                previous_plain_line = Some(trimmed.to_string());
            }
            continue;
        };
        previous_plain_line = None;

        while stack
            .last()
            .is_some_and(|index| nodes[*index].depth >= list.depth)
        {
            stack.pop();
        }
        let parent = stack.last().copied();
        let node_index = nodes.len();
        if let Some(parent) = parent {
            nodes[parent].children.push(node_index);
        }

        let task_index = Task::from_line(
            line,
            file.clone(),
            line_number,
            current_heading.clone(),
            settings,
            statuses,
            now,
        )
        .map(|task| {
            let index = all_tasks.len();
            all_tasks.push(task);
            index
        });
        nodes.push(ListNode {
            line_number,
            depth: list.depth,
            parent,
            children: Vec::new(),
            task_index,
        });
        stack.push(node_index);
    }

    apply_hierarchy(&nodes, all_tasks, task_start);
}

fn apply_hierarchy(nodes: &[ListNode], tasks: &mut [Task], task_start: usize) {
    let mut task_by_line = BTreeMap::new();
    for node in nodes {
        if let Some(task_index) = node.task_index {
            task_by_line.insert(node.line_number, task_index);
        }
    }

    let mut relationships = Vec::new();
    for (node_index, node) in nodes.iter().enumerate() {
        let Some(task_index) = node.task_index else {
            continue;
        };
        let parent_line_number =
            node.parent.map(|parent| nodes[parent].line_number);
        let mut ancestor = node.parent;
        let mut parent_task_line_number = None;
        let mut root = node_index;
        while let Some(index) = ancestor {
            root = index;
            if parent_task_line_number.is_none()
                && nodes[index].task_index.is_some()
            {
                parent_task_line_number = Some(nodes[index].line_number);
            }
            ancestor = nodes[index].parent;
        }
        let child_line_numbers = node
            .children
            .iter()
            .map(|child| nodes[*child].line_number)
            .collect::<Vec<_>>();
        relationships.push((
            task_index,
            parent_line_number,
            parent_task_line_number,
            child_line_numbers,
            nodes[root].line_number,
        ));
    }

    for (
        task_index,
        parent_line_number,
        parent_task_line_number,
        child_line_numbers,
        root_line_number,
    ) in relationships
    {
        let task = &mut tasks[task_index];
        task.parent_line_number = parent_line_number;
        task.parent_task_line_number = parent_task_line_number;
        task.child_line_numbers = child_line_numbers;
        task.root_line_number = root_line_number;
        task.is_root = parent_line_number.is_none();
    }

    let child_tasks = tasks[task_start..]
        .iter()
        .filter_map(|task| {
            task.parent_task_line_number
                .map(|parent| (parent, task.line_number))
        })
        .collect::<Vec<_>>();
    for (parent_line, child_line) in child_tasks {
        if let Some(parent_index) = task_by_line.get(&parent_line) {
            tasks[*parent_index]
                .child_task_line_numbers
                .push(child_line);
        }
    }
}

fn apply_dependencies(tasks: &mut [Task]) {
    let dependency_data = tasks
        .iter()
        .map(|task| (task.id.clone(), task.depends_on.clone(), task.is_done))
        .collect::<Vec<_>>();

    for task in tasks {
        task.is_blocked = !task.is_done
            && !task.depends_on.is_empty()
            && task.depends_on.iter().any(|dependency| {
                dependency_data.iter().any(|(id, _, done)| {
                    id == dependency && !id.is_empty() && !done
                })
            });
        task.is_blocking = !task.is_done
            && !task.id.is_empty()
            && dependency_data.iter().any(|(_, depends_on, done)| {
                !done && depends_on.contains(&task.id)
            });
    }
}

fn parse_atx_heading(line: &str) -> Option<String> {
    let trimmed = line.trim_start_matches(' ');
    if line.len() - trimmed.len() > 3 {
        return None;
    }
    let hashes = trimmed.chars().take_while(|value| *value == '#').count();
    if !(1..=6).contains(&hashes) {
        return None;
    }
    let rest = &trimmed[hashes..];
    if !rest.is_empty() && !rest.starts_with(char::is_whitespace) {
        return None;
    }
    let heading = rest.trim().trim_end_matches('#').trim().to_string();
    Some(heading)
}

fn is_setext_underline(line: &str) -> bool {
    let mut characters = line.chars();
    let Some(marker @ ('=' | '-')) = characters.next() else {
        return false;
    };
    characters.count() >= 2 && line.chars().all(|value| value == marker)
}

fn update_fence(line: &str, fence: &mut Option<(char, usize)>) -> bool {
    let trimmed = line.trim_start();
    let marker = trimmed.chars().next();
    let Some(marker @ ('`' | '~')) = marker else {
        return false;
    };
    let count = trimmed.chars().take_while(|value| *value == marker).count();
    if count < 3 {
        return false;
    }

    match *fence {
        Some((open_marker, open_count))
            if marker == open_marker && count >= open_count =>
        {
            *fence = None;
        }
        None => *fence = Some((marker, count)),
        Some(_) => {}
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::native::env as bob_env;

    #[test]
    fn fixture_index_builds_hierarchy_and_ignores_fences_and_dot_directories() {
        let vault = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/tasks_parity/vault");
        let settings = TasksSettings::read(&vault).unwrap();
        let index =
            TaskIndex::read(&vault, &settings, bob_env::current_datetime())
                .unwrap();

        assert_eq!(index.tasks.len(), 33);
        assert!(
            index.tasks.iter().all(|task| !task.path.starts_with('.')),
            "dot directories must not be indexed"
        );
        assert!(index
            .tasks
            .iter()
            .all(|task| !task.description.contains("Fenced example")));

        let nested = index
            .tasks
            .iter()
            .filter(|task| task.path == "Tasks/Nested.md")
            .collect::<Vec<_>>();
        let parent = nested
            .iter()
            .find(|task| task.description.contains("Parent task"))
            .unwrap();
        let child = nested
            .iter()
            .find(|task| task.description.contains("Child task"))
            .unwrap();
        let grandchild = nested
            .iter()
            .find(|task| task.description.contains("Done grandchild"))
            .unwrap();
        let non_task_child = nested
            .iter()
            .find(|task| task.description.contains("non-task list item"))
            .unwrap();
        assert_eq!(parent.child_task_line_numbers, [child.line_number]);
        assert_eq!(child.parent_task_line_number, Some(parent.line_number));
        assert_eq!(child.child_task_line_numbers, [grandchild.line_number]);
        assert_eq!(grandchild.parent_task_line_number, Some(child.line_number));
        assert_eq!(non_task_child.parent_line_number, Some(5));
        assert_eq!(non_task_child.parent_task_line_number, None);
        assert_eq!(parent.heading.as_deref(), Some("Nested Tasks"));
    }

    #[test]
    fn dependency_graph_matches_direct_tasks_v8_semantics() {
        let settings = TasksSettings::default();
        let statuses = StatusRegistry::new(&settings.status_settings);
        let now = bob_env::current_datetime();
        let file = TaskFile::new("Dependencies.md".to_string());
        let lines = [
            "- [ ] root 🆔 root",
            "- [ ] blocked ⛔ root",
            "- [x] done 🆔 done-root",
            "- [ ] ready ⛔ done-root,missing",
            "- [ ] self 🆔 self ⛔ self",
            "- [x] duplicate done 🆔 duplicate",
            "- [ ] duplicate open 🆔 duplicate",
            "- [ ] duplicate dependent ⛔ duplicate",
        ];
        let mut tasks = lines
            .iter()
            .enumerate()
            .map(|(line_number, line)| {
                Task::from_line(
                    line,
                    file.clone(),
                    line_number,
                    None,
                    &settings,
                    &statuses,
                    now,
                )
                .unwrap()
            })
            .collect::<Vec<_>>();
        apply_dependencies(&mut tasks);

        assert!(tasks[0].is_blocking);
        assert!(tasks[1].is_blocked);
        assert!(!tasks[2].is_blocking);
        assert!(!tasks[3].is_blocked);
        assert!(tasks[4].is_blocked && tasks[4].is_blocking);
        assert!(!tasks[5].is_blocking);
        assert!(tasks[6].is_blocking);
        assert!(tasks[7].is_blocked);
    }

    #[test]
    fn heading_parser_supports_atx_and_setext_headings() {
        assert_eq!(
            parse_atx_heading("## A heading ##").as_deref(),
            Some("A heading")
        );
        assert!(parse_atx_heading("####### Not a heading").is_none());
        assert!(is_setext_underline("---"));
        assert!(is_setext_underline("===="));
    }
}
