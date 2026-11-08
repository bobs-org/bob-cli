//! Read-only Gmail label pull into a task inbox.

use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Component, Path, PathBuf},
    time::Duration,
};

use serde_json::{json, Value};

use super::{
    cli::{HumanFormat, PullArgs, SaveArgs},
    config::GmailConfig,
    gog::Gog,
    journal, resolve, save, ui, GmailError,
};
use crate::native::{
    capture::{self, StagedTextFile},
    ob,
    style::Styler,
    vault_sync,
};

const MAX_LIMIT: u32 = 500;

#[derive(Clone)]
struct PullRow {
    thread_id: String,
    note: Option<String>,
    link: Option<String>,
    status: &'static str,
}

pub(crate) fn run(args: &PullArgs) -> i32 {
    match run_inner(args) {
        Ok((rows, warnings)) => {
            print_report(args, &rows, &warnings);
            0
        }
        Err(error) => ui::report_error("pull", &error, args.format.as_str()),
    }
}

fn run_inner(
    args: &PullArgs,
) -> Result<(Vec<PullRow>, Vec<String>), GmailError> {
    if args.limit > MAX_LIMIT {
        return Err(GmailError::setup(
            "limit",
            format!("pull limit must be at most {MAX_LIMIT}"),
        ));
    }
    let config = GmailConfig::resolve()?;
    let vault = args.bob_dir();
    if !vault.is_dir() {
        return Err(GmailError::setup(
            "vault",
            format!("Bob vault root does not exist: {}", vault.display()),
        ));
    }
    if !args.dry_run {
        check_writer(&config)?;
    }
    let label = config
        .pull_label()
        .filter(|label| !label.is_empty())
        .ok_or_else(|| {
            GmailError::setup(
                "pull_disabled",
                "Gmail label pull is disabled by gmail.pull.label".to_string(),
            )
            .with_hint(
                "set gmail.pull.label to a Gmail label name, such as bob",
            )
        })?;
    let inbox_relative = safe_relative_path(config.pull_inbox())?;
    let inbox_absolute = vault.join(&inbox_relative);
    let initial_inbox = read_inbox(&vault, &inbox_relative)?;
    let initial_index = resolve::scan_index(
        &vault,
        Path::new(config.folder()),
        config.link_label(),
    )?;
    let (pulled, mut warnings) =
        journal::pulled_identities(&vault).map_err(|error| {
            GmailError::runtime(
                "journal",
                format!("could not read the Gmail pull journal: {error}"),
            )
        })?;

    let gog = Gog::resolve(crate::native::google::GMAIL, config.timeout())?;
    gog.require_version()?;
    let account = resolve::resolve_account(&gog, None, config.account())?;
    let ids = if args.limit == 0 {
        Vec::new()
    } else {
        let response = gog.search(
            &account,
            &label_query(label),
            args.limit.min(MAX_LIMIT),
        )?;
        search_thread_ids(&response)?
    };

    let account_key = account.to_ascii_lowercase();
    let inbox_text = initial_inbox.as_deref().unwrap_or_default();
    let mut candidates = Vec::new();
    let mut repairs = Vec::new();
    let mut rows = Vec::with_capacity(ids.len());
    for thread_id in ids {
        let key = (account_key.clone(), thread_id.clone());
        let entry = initial_index.by_identity.get(&key);
        let target = entry.map(|entry| note_target(&entry.path));
        if pulled.contains(&key) {
            rows.push(PullRow {
                thread_id,
                note: entry.map(|entry| entry.path.clone()),
                link: entry.map(|entry| entry.label.clone()),
                status: "already_pulled",
            });
        } else if target
            .as_deref()
            .is_some_and(|target| inbox_has_task_link(inbox_text, target))
        {
            let entry = entry.expect("target exists only with index entry");
            repairs.push((
                entry.account.clone(),
                entry.thread_id.clone(),
                entry.path.clone(),
            ));
            rows.push(PullRow {
                thread_id,
                note: Some(entry.path.clone()),
                link: Some(entry.label.clone()),
                status: "already_filed",
            });
        } else {
            candidates.push(thread_id);
        }
    }

    let mut planned_notes = BTreeMap::new();
    if !candidates.is_empty() {
        let save_args = SaveArgs {
            bob_dir: Some(vault.clone()),
            body: None,
            dry_run: args.dry_run,
            format: HumanFormat::Human,
            inputs: candidates.clone(),
            parent: None,
            quiet: true,
            rerender: false,
        };
        for plan in save::run_internal(&save_args)? {
            planned_notes.insert(
                (
                    plan.account.to_ascii_lowercase(),
                    plan.thread_id.to_ascii_lowercase(),
                ),
                plan,
            );
        }
    }

    let mut task_rows = Vec::new();
    for thread_id in &candidates {
        let key = (account_key.clone(), thread_id.clone());
        let plan = planned_notes.get(&key).ok_or_else(|| {
            GmailError::runtime(
                "unresolvable",
                "the saved mail note was not present in the save plan"
                    .to_string(),
            )
        })?;
        let note_path = relative_path(&vault, &plan.path)?;
        task_rows.push((
            plan.account.clone(),
            plan.thread_id.clone(),
            note_path.clone(),
            plan.link.clone(),
        ));
        rows.push(PullRow {
            thread_id: thread_id.clone(),
            note: Some(note_path),
            link: Some(plan.link.clone()),
            status: if args.dry_run { "would_file" } else { "filed" },
        });
    }
    rows.sort_by(|left, right| left.thread_id.cmp(&right.thread_id));

    if args.dry_run {
        return Ok((rows, warnings));
    }

    let mut appended = Vec::new();
    if !task_rows.is_empty() || !repairs.is_empty() {
        let styler = Styler::detect();
        let _lock = ob::acquire_lock_waiting(Duration::from_secs(30), || {
            eprintln!(
                "{}",
                styler.dim("waiting for another vault maintenance run…")
            );
        })
        .map_err(|error| {
            GmailError::runtime(
                "lock",
                format!("could not acquire the vault lock: {error}"),
            )
            .with_hint("another run held the lock for 30 s; try again")
        })?;

        let current = read_inbox(&vault, &inbox_relative)?;
        let current_text = current.as_deref().unwrap_or_default();
        let mut additions = Vec::new();
        for (row_account, thread_id, note, link) in &task_rows {
            let target = link_target(link).ok_or_else(|| {
                GmailError::runtime(
                    "bad_note",
                    "the save plan returned an invalid mail link".to_string(),
                )
            })?;
            if !inbox_has_task_link(current_text, &target) {
                additions.push(format!("- [ ] #task {link}"));
                appended.push((
                    row_account.clone(),
                    thread_id.clone(),
                    note.clone(),
                ));
            } else {
                repairs.push((
                    row_account.clone(),
                    thread_id.clone(),
                    note.clone(),
                ));
            }
        }

        let updated = if additions.is_empty() {
            None
        } else {
            Some(append_tasks(current.as_deref(), &additions))
        };
        if let Some(updated) = updated {
            ensure_parent_dirs(&vault, &inbox_relative)?;
            let staged = [StagedTextFile {
                target: inbox_absolute.clone(),
                target_existed: current.is_some(),
                original_target: current.unwrap_or_default(),
                updated_target: updated,
            }];
            capture::write_staged_files(&staged).map_err(|error| {
                GmailError::runtime(
                    "write",
                    format!(
                        "could not safely write the Gmail inbox: {}",
                        error.message
                    ),
                )
            })?;

            let child_env = ob::child_env();
            let is_worktree = ob::detect_git_worktree(&vault, &child_env)
                .map_err(|error| {
                    GmailError::runtime(
                        "git",
                        format!("could not determine whether the vault is a Git worktree: {error}"),
                    )
                })?;
            if is_worktree {
                ob::commit_paths(
                    &vault,
                    &child_env,
                    "gmail: pull labelled threads",
                    &[inbox_relative.clone()],
                )
                .map_err(|error| {
                    GmailError::runtime(
                        "uncommitted",
                        format!("Gmail tasks were written but could not be committed: {error}"),
                    )
                    .with_hint("run `bob gmail pull` again to finish filing")
                })?;
            }
        }

        let host = vault_sync::hostname_slug();
        for (row_account, thread_id, note) in
            appended.into_iter().chain(repairs)
        {
            if let Err(error) = journal::append_pulled(
                &vault,
                &host,
                &row_account,
                &thread_id,
                config.pull_inbox(),
                &note,
            ) {
                warnings.push(format!(
                    "could not record Gmail pull for thread {thread_id}: {error}"
                ));
            }
        }
    }
    Ok((rows, warnings))
}

fn label_query(label: &str) -> String {
    if label.bytes().any(|byte| byte.is_ascii_whitespace()) {
        format!(
            "label:\"{}\"",
            label.replace('\\', "\\\\").replace('"', "\\\"")
        )
    } else {
        format!("label:{label}")
    }
}

fn search_thread_ids(response: &Value) -> Result<Vec<String>, GmailError> {
    let Some(threads) = response.get("threads").and_then(Value::as_array)
    else {
        return Err(GmailError::runtime(
            "bad_json",
            "gog label search returned no thread list".to_string(),
        ));
    };
    let mut seen = BTreeSet::new();
    let mut result = Vec::new();
    for thread in threads {
        let id = thread
            .get("id")
            .and_then(Value::as_str)
            .filter(|id| {
                !id.is_empty()
                    && id.len() <= 128
                    && id.bytes().all(|byte| byte.is_ascii_hexdigit())
            })
            .ok_or_else(|| {
                GmailError::runtime(
                    "bad_json",
                    "gog label search returned an invalid thread identity"
                        .to_string(),
                )
            })?
            .to_ascii_lowercase();
        if seen.insert(id.clone()) {
            result.push(id);
        }
    }
    Ok(result)
}

fn safe_relative_path(text: &str) -> Result<PathBuf, GmailError> {
    let path = PathBuf::from(text);
    if path.as_os_str().is_empty()
        || path.is_absolute()
        || path
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(GmailError::setup(
            "config",
            "gmail.pull.inbox must be a safe path inside the vault".to_string(),
        ));
    }
    Ok(path)
}

fn read_inbox(
    vault: &Path,
    relative: &Path,
) -> Result<Option<String>, GmailError> {
    validate_existing_parent_dirs(vault, relative)?;
    let target = vault.join(relative);
    match fs::symlink_metadata(&target) {
        Ok(metadata)
            if metadata.file_type().is_symlink() || !metadata.is_file() =>
        {
            Err(GmailError::runtime(
                "refused",
                "gmail.pull.inbox is not a regular in-vault file".to_string(),
            ))
        }
        Ok(_) => fs::read_to_string(&target).map(Some).map_err(|error| {
            GmailError::runtime(
                "inbox",
                format!("could not read Gmail inbox: {error}"),
            )
        }),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(GmailError::runtime(
            "inbox",
            format!("could not inspect Gmail inbox: {error}"),
        )),
    }
}

fn append_tasks(existing: Option<&str>, additions: &[String]) -> String {
    let mut text = existing
        .map(str::to_string)
        .unwrap_or_else(super::sync::file::inbox_template);
    let heading = find_tasks_heading(&text);
    let Some((_, section_end)) = heading else {
        if !text.is_empty() && !text.ends_with('\n') {
            text.push('\n');
        }
        if !text.is_empty() && !text.ends_with("\n\n") {
            text.push('\n');
        }
        text.push_str("## Tasks\n");
        let section_end = text.len();
        return insert_task_lines(text, section_end, additions);
    };
    insert_task_lines(text, section_end, additions)
}

fn find_tasks_heading(text: &str) -> Option<(usize, usize)> {
    let mut offset = 0;
    let mut section_start = None;
    for line in text.split_inclusive('\n') {
        let trimmed = line.trim();
        if section_start.is_none() && trimmed == "## Tasks" {
            section_start = Some(offset + line.len());
        } else if section_start.is_some() && is_section_heading(trimmed) {
            return Some((section_start?, offset));
        }
        offset += line.len();
    }
    section_start.map(|start| (start, text.len()))
}

fn is_section_heading(line: &str) -> bool {
    let level = line
        .chars()
        .take_while(|character| *character == '#')
        .count();
    (1..=2).contains(&level) && line.as_bytes().get(level) == Some(&b' ')
}

fn insert_task_lines(
    mut text: String,
    at: usize,
    additions: &[String],
) -> String {
    let mut insertion = String::new();
    if at > 0 && text.as_bytes().get(at - 1) != Some(&b'\n') {
        insertion.push('\n');
    }
    for row in additions {
        insertion.push_str(row);
        insertion.push('\n');
    }
    if at < text.len() && !text[..at].ends_with("\n\n") {
        insertion.push('\n');
    }
    text.insert_str(at, &insertion);
    text
}

fn inbox_has_task_link(text: &str, target: &str) -> bool {
    text.lines().any(|line| {
        let trimmed = line.trim_start();
        if !trimmed.starts_with("- [") || !trimmed.contains("] #task") {
            return false;
        }
        let Some(start) = trimmed.find("[[").map(|start| start + 2) else {
            return false;
        };
        let Some(end) = trimmed[start..].find("]]").map(|end| start + end)
        else {
            return false;
        };
        trimmed[start..end].split('|').next() == Some(target)
    })
}

fn link_target(link: &str) -> Option<String> {
    let contents = link.strip_prefix("[[")?.strip_suffix("]]")?;
    Some(contents.split('|').next()?.to_string())
}

fn note_target(note: &str) -> String {
    Path::new(note)
        .with_extension("")
        .to_string_lossy()
        .replace('\\', "/")
}

fn relative_path(vault: &Path, absolute: &Path) -> Result<String, GmailError> {
    let relative = absolute.strip_prefix(vault).map_err(|_| {
        GmailError::runtime(
            "refused",
            "the saved mail note escaped the vault".to_string(),
        )
    })?;
    Ok(relative
        .components()
        .map(|component| component.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/"))
}

fn ensure_parent_dirs(vault: &Path, relative: &Path) -> Result<(), GmailError> {
    let parent = relative.parent().unwrap_or_else(|| Path::new(""));
    let mut current = vault.to_path_buf();
    for component in parent.components() {
        let Component::Normal(part) = component else {
            return Err(GmailError::runtime(
                "refused",
                "gmail.pull.inbox escaped the vault".to_string(),
            ));
        };
        current.push(part);
        match fs::symlink_metadata(&current) {
            Ok(metadata)
                if metadata.is_dir() && !metadata.file_type().is_symlink() => {}
            Ok(_) => {
                return Err(GmailError::runtime(
                    "refused",
                    "a Gmail inbox parent is not a safe directory".to_string(),
                ));
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                fs::create_dir(&current).map_err(|error| {
                    GmailError::runtime(
                        "inbox",
                        format!(
                            "could not create the Gmail inbox folder: {error}"
                        ),
                    )
                })?;
            }
            Err(error) => {
                return Err(GmailError::runtime(
                    "inbox",
                    format!("could not inspect a Gmail inbox folder: {error}"),
                ));
            }
        }
    }
    Ok(())
}

fn validate_existing_parent_dirs(
    vault: &Path,
    relative: &Path,
) -> Result<(), GmailError> {
    let parent = relative.parent().unwrap_or_else(|| Path::new(""));
    let mut current = vault.to_path_buf();
    for component in parent.components() {
        let Component::Normal(part) = component else {
            return Err(GmailError::runtime(
                "refused",
                "gmail.pull.inbox escaped the vault".to_string(),
            ));
        };
        current.push(part);
        match fs::symlink_metadata(&current) {
            Ok(metadata)
                if metadata.is_dir() && !metadata.file_type().is_symlink() => {}
            Ok(_) => {
                return Err(GmailError::runtime(
                    "refused",
                    "a Gmail inbox parent is not a safe directory".to_string(),
                ));
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => break,
            Err(error) => {
                return Err(GmailError::runtime(
                    "inbox",
                    format!("could not inspect a Gmail inbox parent: {error}"),
                ));
            }
        }
    }
    Ok(())
}

fn check_writer(config: &GmailConfig) -> Result<(), GmailError> {
    let host = vault_sync::hostname_slug();
    if let Some(writer) = config.writer()
        && writer != host
    {
        return Err(GmailError::setup(
            "writer",
            format!("this host ({host}) is not the configured Gmail writer"),
        )
        .with_hint(&format!(
            "run this command on the configured writer {writer}"
        )));
    }
    Ok(())
}

fn print_report(args: &PullArgs, rows: &[PullRow], warnings: &[String]) {
    if args.format.is_json() {
        let results: Vec<_> = rows
            .iter()
            .map(|row| {
                json!({
                    "thread_id": row.thread_id,
                    "note": row.note,
                    "link": row.link,
                    "status": row.status,
                })
            })
            .collect();
        println!(
            "{}",
            json!({
                "schema_version": 1,
                "ok": true,
                "dry_run": args.dry_run,
                "pulled": rows.iter().filter(|row| row.status == "filed" || row.status == "would_file").count(),
                "results": results,
                "warnings": warnings,
            })
        );
        ui::flush_stdout();
        return;
    }
    if args.quiet {
        return;
    }
    if rows.is_empty() {
        println!("No labelled Gmail threads to pull.");
    } else {
        let styler = Styler::detect();
        for row in rows {
            let action = match row.status {
                "filed" => "filed",
                "would_file" => "would file",
                "already_filed" | "already_pulled" => "already filed",
                _ => "skipped",
            };
            let destination =
                row.note.as_deref().unwrap_or("saved note unavailable");
            let line = format!("{action} · {} → {destination}", row.thread_id);
            println!("{}", styler.cyan(&line));
        }
    }
    let styler = Styler::detect();
    for warning in warnings {
        eprintln!("{}", styler.dim(warning));
    }
    ui::flush_stdout();
}
