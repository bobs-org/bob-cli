use std::{
    collections::{BTreeMap, BTreeSet},
    env, fs,
    path::{Path, PathBuf},
    process::{Command, Output},
    sync::LazyLock,
    time::{SystemTime, UNIX_EPOCH},
};

use chrono::NaiveDate;
use regex::Regex;
use serde_json::Value;

const BOB_BIN: &str = env!("CARGO_BIN_EXE_bob");
const REAL_PARITY_ENV: &str = "BOB_TASKS_REAL_VAULT_PARITY";

static TASK_LINE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^\s*(?:[-+*]|\d+[.)])\s+\[(?<symbol>.)\](?:\s+|$)(?<body>.*)$")
        .expect("valid raw task-line regex")
});
static SCHEDULED: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"[\[(]scheduled::\s*(?<value>\d{4}-\d{2}-\d{2})\s*[\])]")
        .expect("valid raw scheduled-date regex")
});
static TASK_ID: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"[\[(]id::\s*(?<value>[a-zA-Z0-9_-]+)\s*[\])]")
        .expect("valid raw task-id regex")
});
static DEPENDS_ON: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"[\[(]dependsOn::\s*(?<value>[a-zA-Z0-9_-]+(?:\s*,\s*[a-zA-Z0-9_-]+)*)\s*[\])]",
    )
    .expect("valid raw task-dependency regex")
});
static HIDE_TAG: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?:^|\s)#hide(?:$|[ !@#$%^&*(),.?\":{}|<>])"#)
        .expect("valid #hide tag regex")
});

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct TaskKey {
    path: String,
    line_number: u64,
    status_symbol: String,
}

#[derive(Debug)]
struct RawTask {
    key: TaskKey,
    body: String,
    status_name: String,
    status_type: String,
    is_done: bool,
    scheduled: Option<NaiveDate>,
    id: String,
    depends_on: Vec<String>,
}

struct VaultSnapshot {
    path: PathBuf,
}

impl VaultSnapshot {
    fn new(source: &Path) -> Self {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock before UNIX_EPOCH")
            .as_nanos();
        let path = env::temp_dir().join(format!(
            "bob-cli-real-tasks-parity-{}-{nanos}",
            std::process::id(),
        ));

        for source_path in collect_markdown_paths(source) {
            let relative = source_path
                .strip_prefix(source)
                .expect("real-vault Markdown path must be relative");
            copy_file(&source_path, &path.join(relative));
        }
        let settings =
            Path::new(".obsidian/plugins/obsidian-tasks-plugin/data.json");
        copy_file(&source.join(settings), &path.join(settings));
        Self { path }
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for VaultSnapshot {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

#[test]
fn real_vault_dash_matches_independent_raw_ground_truth_and_all_blocks_execute()
{
    if env::var_os(REAL_PARITY_ENV).is_none() {
        return;
    }

    let source_vault = real_vault_path();
    let snapshot = VaultSnapshot::new(&source_vault);
    let vault = snapshot.path().to_path_buf();
    let now = env::var("BOB_NOW").unwrap_or_else(|_| {
        panic!("{REAL_PARITY_ENV}=1 requires BOB_NOW for deterministic parity")
    });
    let today = NaiveDate::parse_from_str(
        now.get(..10).unwrap_or_default(),
        "%Y-%m-%d",
    )
    .unwrap_or_else(|error| {
        panic!("BOB_NOW must start with YYYY-MM-DD: {error}")
    });

    let settings = read_json(
        &vault.join(".obsidian/plugins/obsidian-tasks-plugin/data.json"),
    );
    let global_filter = settings["globalFilter"].as_str().unwrap_or_default();
    let statuses = raw_statuses(&settings);
    let markdown_paths = collect_markdown_paths(&vault);
    let tasks = raw_tasks(&vault, &markdown_paths, global_filter, &statuses);

    let note = run_json_query(
        &vault,
        &now,
        &["--format", "json", "--tasks-note", "dash.md"],
    );
    let blocks = note["blocks"].as_array().expect("dashboard blocks array");
    assert_eq!(blocks.len(), 3, "dash.md must contain three Tasks blocks");

    for block in blocks {
        let heading = block["heading"].as_str().expect("block heading");
        let query = block["query"].as_str().expect("raw block query");
        let note_tasks = json_task_keys(&block["result"]);
        let individual = run_json_query(
            &vault,
            &now,
            &["--format", "json", "--origin", "dash.md", "--tasks", query],
        );
        assert_eq!(
            json_task_keys(&individual["result"]),
            note_tasks,
            "{heading}: --tasks-note and --tasks/--origin diverged",
        );

        let expected = dashboard_ground_truth(&tasks, today, heading);
        assert_task_sets_equal(heading, &note_tasks, &expected);
    }

    let mut other_block_count = 0;
    let mut other_note_count = 0;
    for path in &markdown_paths {
        let relative = relative_path(&vault, path);
        if relative == "dash.md" {
            continue;
        }
        let contents = fs::read_to_string(path)
            .unwrap_or_else(|error| panic!("read {}: {error}", path.display()));
        let block_count = tasks_block_count(&contents);
        if block_count == 0 {
            continue;
        }
        let actual = run_json_query(
            &vault,
            &now,
            &["--format", "json", "--tasks-note", &relative],
        );
        assert_eq!(
            actual["blocks"].as_array().map_or(0, Vec::len),
            block_count,
            "{relative}: not every raw Tasks block executed",
        );
        other_note_count += 1;
        other_block_count += block_count;
    }

    let dashboard_counts = blocks
        .iter()
        .map(|block| {
            format!(
                "{}={}",
                block["heading"].as_str().unwrap_or("unknown"),
                block["result"]["count"].as_u64().unwrap_or(0),
            )
        })
        .collect::<Vec<_>>()
        .join(", ");
    eprintln!(
        "real-vault Tasks parity passed: {dashboard_counts}; \
         {other_block_count} other blocks across {other_note_count} notes",
    );
}

fn dashboard_ground_truth(
    tasks: &[RawTask],
    today: NaiveDate,
    heading: &str,
) -> BTreeSet<TaskKey> {
    tasks
        .iter()
        .filter(|task| {
            let folder = task
                .key
                .path
                .rsplit_once('/')
                .map_or("/", |(folder, _)| folder);
            task.key.path != "dash.md"
                && !folder.contains("_templates")
                && !HIDE_TAG.is_match(&task.body)
                && task.scheduled.is_none_or(|scheduled| scheduled <= today)
                && !is_blocked(task, tasks)
                && match heading {
                    "WIP Tasks" => task.status_type == "IN_PROGRESS",
                    "NEXT Tasks" => {
                        task.status_name.to_lowercase().contains("next")
                    }
                    "READY Tasks" => task.status_type == "TODO",
                    other => {
                        panic!("unexpected dash.md Tasks heading {other:?}")
                    }
                }
        })
        .map(|task| task.key.clone())
        .collect()
}

fn is_blocked(task: &RawTask, tasks: &[RawTask]) -> bool {
    !task.is_done
        && task.depends_on.iter().any(|dependency| {
            tasks.iter().any(|candidate| {
                !candidate.id.is_empty()
                    && candidate.id == *dependency
                    && !candidate.is_done
            })
        })
}

fn raw_tasks(
    vault: &Path,
    paths: &[PathBuf],
    global_filter: &str,
    statuses: &BTreeMap<String, (String, String)>,
) -> Vec<RawTask> {
    let mut tasks = Vec::new();
    for path in paths {
        let contents = fs::read_to_string(path)
            .unwrap_or_else(|error| panic!("read {}: {error}", path.display()));
        let relative = relative_path(vault, path);
        let mut frontmatter = false;
        let mut comment = false;
        let mut fence = None;

        for (line_number, line) in contents.lines().enumerate() {
            let trimmed = line.trim();
            if line_number == 0 && trimmed == "---" {
                frontmatter = true;
                continue;
            }
            if frontmatter {
                if matches!(trimmed, "---" | "...") {
                    frontmatter = false;
                }
                continue;
            }
            if comment {
                if trimmed.contains("-->") {
                    comment = false;
                }
                continue;
            }
            if trimmed.starts_with("<!--") {
                comment = !trimmed.contains("-->");
                continue;
            }
            if update_fence(line, &mut fence) || fence.is_some() {
                continue;
            }

            let Some(captures) = TASK_LINE.captures(line) else {
                continue;
            };
            let body = captures["body"].trim().to_string();
            if !global_filter.is_empty() && !body.contains(global_filter) {
                continue;
            }
            let symbol = captures["symbol"].to_string();
            let (status_name, status_type) = statuses
                .get(&symbol)
                .cloned()
                .unwrap_or_else(|| ("Unknown".to_string(), "TODO".to_string()));
            let is_done = matches!(
                status_type.as_str(),
                "DONE" | "CANCELLED" | "NON_TASK"
            );
            let scheduled = SCHEDULED.captures(&body).and_then(|value| {
                NaiveDate::parse_from_str(&value["value"], "%Y-%m-%d").ok()
            });
            let id = TASK_ID
                .captures(&body)
                .map_or_else(String::new, |value| value["value"].to_string());
            let depends_on = DEPENDS_ON
                .captures(&body)
                .map(|value| {
                    value["value"]
                        .split(',')
                        .map(str::trim)
                        .map(str::to_string)
                        .collect()
                })
                .unwrap_or_default();

            tasks.push(RawTask {
                key: TaskKey {
                    path: relative.clone(),
                    line_number: line_number as u64,
                    status_symbol: symbol,
                },
                body,
                status_name,
                status_type,
                is_done,
                scheduled,
                id,
                depends_on,
            });
        }
    }
    tasks
}

fn raw_statuses(settings: &Value) -> BTreeMap<String, (String, String)> {
    let mut statuses = BTreeMap::new();
    for collection in ["coreStatuses", "customStatuses"] {
        let configured = settings["statusSettings"][collection]
            .as_array()
            .into_iter()
            .flatten();
        for status in configured {
            let symbol = status["symbol"].as_str().unwrap_or_default();
            statuses.entry(symbol.to_string()).or_insert_with(|| {
                (
                    status["name"].as_str().unwrap_or("Unknown").to_string(),
                    status["type"].as_str().unwrap_or("TODO").to_string(),
                )
            });
        }
    }
    if statuses.is_empty() {
        statuses
            .insert(" ".to_string(), ("Todo".to_string(), "TODO".to_string()));
        statuses
            .insert("x".to_string(), ("Done".to_string(), "DONE".to_string()));
    }
    statuses
}

fn json_task_keys(result: &Value) -> BTreeSet<TaskKey> {
    result["tasks"]
        .as_array()
        .expect("native task rows")
        .iter()
        .map(|task| TaskKey {
            path: task["path"].as_str().expect("task path").to_string(),
            line_number: task["lineNumber"].as_u64().expect("task line number"),
            status_symbol: task["status"]["symbol"]
                .as_str()
                .expect("task status symbol")
                .to_string(),
        })
        .collect()
}

fn assert_task_sets_equal(
    heading: &str,
    actual: &BTreeSet<TaskKey>,
    expected: &BTreeSet<TaskKey>,
) {
    let missing = expected.difference(actual).collect::<Vec<_>>();
    let unexpected = actual.difference(expected).collect::<Vec<_>>();
    assert!(
        missing.is_empty() && unexpected.is_empty(),
        "{heading}: native/raw task-set mismatch\nmissing: {missing:#?}\nunexpected: {unexpected:#?}",
    );
}

fn run_json_query(vault: &Path, now: &str, args: &[&str]) -> Value {
    let output = Command::new(BOB_BIN)
        .arg("query")
        .arg("--bob-dir")
        .arg(vault)
        .args(args)
        .env("BOB_NOW", now)
        .output()
        .unwrap_or_else(|error| {
            panic!("run bob query against {}: {error}", vault.display())
        });
    assert_success(&output);
    serde_json::from_slice(&output.stdout).unwrap_or_else(|error| {
        panic!("parse query JSON: {error}\n{}", format_output(&output))
    })
}

fn assert_success(output: &Output) {
    assert!(output.status.success(), "{}", format_output(output));
}

fn format_output(output: &Output) -> String {
    format!(
        "status: {}\nstdout:\n{}\nstderr:\n{}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    )
}

fn read_json(path: &Path) -> Value {
    let contents = fs::read_to_string(path)
        .unwrap_or_else(|error| panic!("read {}: {error}", path.display()));
    serde_json::from_str(&contents)
        .unwrap_or_else(|error| panic!("parse {}: {error}", path.display()))
}

fn copy_file(source: &Path, destination: &Path) {
    if let Some(parent) = destination.parent() {
        fs::create_dir_all(parent).unwrap_or_else(|error| {
            panic!("create {}: {error}", parent.display())
        });
    }
    fs::copy(source, destination).unwrap_or_else(|error| {
        panic!(
            "copy {} to {}: {error}",
            source.display(),
            destination.display(),
        )
    });
}

fn real_vault_path() -> PathBuf {
    if let Some(path) = env::var_os("BOB_DIR") {
        return PathBuf::from(path);
    }
    let home = env::var_os("HOME").unwrap_or_else(|| {
        panic!("{REAL_PARITY_ENV}=1 requires BOB_DIR or HOME")
    });
    PathBuf::from(home).join("bob")
}

fn collect_markdown_paths(vault: &Path) -> Vec<PathBuf> {
    fn visit(directory: &Path, paths: &mut Vec<PathBuf>) {
        let entries = fs::read_dir(directory).unwrap_or_else(|error| {
            panic!("read {}: {error}", directory.display())
        });
        for entry in entries {
            let entry = entry.unwrap_or_else(|error| {
                panic!("read entry in {}: {error}", directory.display())
            });
            let path = entry.path();
            let file_type = entry.file_type().unwrap_or_else(|error| {
                panic!("read type for {}: {error}", path.display())
            });
            if file_type.is_dir() {
                if !entry.file_name().to_string_lossy().starts_with('.') {
                    visit(&path, paths);
                }
            } else if file_type.is_file()
                && path.extension().is_some_and(|value| {
                    value.to_string_lossy().eq_ignore_ascii_case("md")
                })
            {
                paths.push(path);
            }
        }
    }

    let mut paths = Vec::new();
    visit(vault, &mut paths);
    paths.sort();
    paths
}

fn relative_path(vault: &Path, path: &Path) -> String {
    path.strip_prefix(vault)
        .unwrap_or_else(|error| {
            panic!(
                "make {} relative to {}: {error}",
                path.display(),
                vault.display(),
            )
        })
        .to_string_lossy()
        .replace('\\', "/")
}

fn tasks_block_count(contents: &str) -> usize {
    contents
        .lines()
        .filter(|line| line.trim().eq_ignore_ascii_case("```tasks"))
        .count()
}

fn update_fence(line: &str, fence: &mut Option<(char, usize)>) -> bool {
    let trimmed = line.trim_start();
    let Some(marker @ ('`' | '~')) = trimmed.chars().next() else {
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
