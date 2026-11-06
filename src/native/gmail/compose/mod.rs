//! Local compose-note creation and reply insertion.

use std::{
    collections::BTreeSet,
    fs,
    path::{Component, Path, PathBuf},
    time::Duration,
};

use chrono::{DateTime, Local, TimeZone, Utc};
use serde_yaml::{Mapping, Value};

use super::{
    cli::{DraftArgs, HumanFormat, NewArgs, ReplyArgs},
    config::GmailConfig,
    note, ui, GmailError,
};
use crate::native::{
    capture::{self, StagedTextFile},
    env as bob_env, ob, vault_sync,
};

// The draft transport phase consumes the render and attachment boundaries.
#[allow(dead_code)]
pub(crate) mod attachments;
pub(super) mod draft;
// The draft transport phase consumes the render and attachment boundaries.
#[allow(dead_code)]
pub(crate) mod render;
// The send-recovery engine stays internal until the confirmed send lands.
#[allow(dead_code)]
pub(crate) mod send;
// Durable send state transitions shared by recovery and the confirmed send.
#[allow(dead_code)]
pub(crate) mod state;
// The following compose and draft phases consume the full source model.
#[allow(dead_code)]
pub(crate) mod source;

pub(crate) fn run_new(args: &NewArgs) -> i32 {
    match run_new_inner(args) {
        Ok((path, cursor)) => {
            report_success("new", &path, cursor, args.format);
            0
        }
        Err(error) => ui::report_error("new", &error, args.format.as_str()),
    }
}

pub(crate) fn run_reply(args: &ReplyArgs) -> i32 {
    match run_reply_inner(args) {
        Ok((path, cursor)) => {
            report_success("reply", &path, cursor, args.format);
            0
        }
        Err(error) => ui::report_error("reply", &error, args.format.as_str()),
    }
}

pub(crate) fn run_draft(args: &DraftArgs) -> i32 {
    draft::run(args)
}

fn run_new_inner(
    args: &NewArgs,
) -> Result<(String, (usize, usize)), GmailError> {
    let config = GmailConfig::resolve()?;
    let vault = args.bob_dir();
    validate_vault(&vault)?;
    require_writer(&config)?;
    let recipients_to =
        source::parse_recipient_list(args.to.as_deref().unwrap_or_default())
            .map_err(|error| {
                GmailError::setup("recipients", error.to_string())
            })?;
    let recipients_cc =
        source::parse_recipient_list(args.cc.as_deref().unwrap_or_default())
            .map_err(|error| {
                GmailError::setup("recipients", error.to_string())
            })?;
    let recipients_bcc = source::parse_recipient_list(
        args.bcc.as_deref().unwrap_or_default(),
    )
    .map_err(|error| GmailError::setup("recipients", error.to_string()))?;
    let raw_subject = args.subject.as_deref().unwrap_or_default();
    source::validate_subject(raw_subject)
        .map_err(|error| GmailError::setup("subject", error.to_string()))?;
    let subject = raw_subject.trim().to_string();
    let folder = Path::new(config.folder());
    validate_mail_folder(&vault, folder)?;
    let _lock = maintenance_lock()?;

    validate_mail_folder(&vault, folder)?;
    let occupied = collect_occupied(&vault, &vault.join(folder))?;
    let now = current_utc();
    let timezone = bob_env::configured_timezone().unwrap_or(chrono_tz::UTC);
    let relative =
        note::path::allocate(folder, &subject, now, timezone, &occupied)
            .map_err(|error| GmailError::runtime("path", error.to_string()))?;
    let path = vault.join(&relative);
    validate_target(&vault, &relative, false)?;
    let content = render_compose_note(
        &recipients_to,
        &recipients_cc,
        &recipients_bcc,
        &subject,
    )?;
    let cursor = cursor_at(&content, content.len());
    let mut staged = plan_missing_hubs(&vault)?;
    staged.push(StagedTextFile {
        target: path.clone(),
        target_existed: false,
        original_target: String::new(),
        updated_target: content,
    });
    write_and_commit(&vault, staged, "gmail: compose")?;
    Ok((path_to_slashes(&relative), cursor))
}

fn run_reply_inner(
    args: &ReplyArgs,
) -> Result<(String, (usize, usize)), GmailError> {
    let config = GmailConfig::resolve()?;
    let vault = args.bob_dir();
    validate_vault(&vault)?;
    require_writer(&config)?;
    let folder = Path::new(config.folder());
    validate_mail_folder(&vault, folder)?;
    let _lock = maintenance_lock()?;
    validate_mail_folder(&vault, folder)?;
    let relative = resolve_note_path(&vault, folder, &args.note)?;
    validate_target(&vault, &relative, true)?;
    let target = vault.join(&relative);
    let original = fs::read_to_string(&target).map_err(|error| {
        GmailError::runtime(
            "note",
            format!("could not read saved Gmail note: {error}"),
        )
    })?;
    source::reply_target_for_append(&original)
        .map_err(|error| GmailError::setup("note", error.to_string()))?;
    let updated = append_reply(&original, args.all);
    let cursor = cursor_at(&updated, updated.len());
    let mut staged = plan_missing_hubs(&vault)?;
    staged.push(StagedTextFile {
        target,
        target_existed: true,
        original_target: original,
        updated_target: updated,
    });
    write_and_commit(&vault, staged, "gmail: reply")?;
    Ok((path_to_slashes(&relative), cursor))
}

fn render_compose_note(
    to: &[String],
    cc: &[String],
    bcc: &[String],
    subject: &str,
) -> Result<String, GmailError> {
    let mut frontmatter = Mapping::new();
    insert(
        &mut frontmatter,
        "parent",
        Value::String("[[mail]]".to_string()),
    );
    insert(
        &mut frontmatter,
        "type",
        Value::String("[[mail]]".to_string()),
    );
    insert(
        &mut frontmatter,
        "cssclasses",
        Value::Sequence(vec![Value::String("bob-mail".to_string())]),
    );
    insert(
        &mut frontmatter,
        "status",
        Value::String("draft".to_string()),
    );
    insert(&mut frontmatter, "to", string_sequence(to));
    insert(&mut frontmatter, "cc", string_sequence(cc));
    insert(&mut frontmatter, "bcc", string_sequence(bcc));
    insert(
        &mut frontmatter,
        "subject",
        Value::String(subject.to_string()),
    );
    insert(&mut frontmatter, "attachments", Value::Sequence(Vec::new()));
    let yaml =
        serde_yaml::to_string(&Value::Mapping(frontmatter)).map_err(|_| {
            GmailError::runtime(
                "note",
                "could not render compose frontmatter".to_string(),
            )
        })?;
    let title = if subject.is_empty() {
        "(no subject)"
    } else {
        subject
    };
    Ok(format!("---\n{yaml}---\n\n# {title}\n\n"))
}

fn insert(mapping: &mut Mapping, key: &str, value: Value) {
    mapping.insert(Value::String(key.to_string()), value);
}

fn string_sequence(values: &[String]) -> Value {
    Value::Sequence(values.iter().cloned().map(Value::String).collect())
}

fn append_reply(contents: &str, all: bool) -> String {
    let separator = if contents.ends_with("\n\n") {
        ""
    } else if contents.ends_with('\n') {
        "\n"
    } else {
        "\n\n"
    };
    let heading = if all { "## Reply all" } else { "## Reply" };
    format!("{contents}{separator}{heading}\n\n")
}

fn plan_missing_hubs(vault: &Path) -> Result<Vec<StagedTextFile>, GmailError> {
    let mut staged = Vec::new();
    for (relative, template) in note::hub::templates() {
        let target = vault.join(&relative);
        match fs::symlink_metadata(&target) {
            Ok(metadata)
                if metadata.is_file() && !metadata.file_type().is_symlink() => {
            }
            Ok(_) => {
                return Err(GmailError::runtime(
                    "refused",
                    format!(
                        "{} is not a regular mail hub file",
                        relative.display()
                    ),
                ));
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                staged.push(StagedTextFile {
                    target,
                    target_existed: false,
                    original_target: String::new(),
                    updated_target: template.to_string(),
                });
            }
            Err(error) => {
                return Err(GmailError::runtime(
                    "write",
                    format!(
                        "could not inspect {}: {error}",
                        relative.display()
                    ),
                ));
            }
        }
    }
    Ok(staged)
}

fn write_and_commit(
    vault: &Path,
    staged: Vec<StagedTextFile>,
    message: &str,
) -> Result<(), GmailError> {
    let child_env = ob::child_env();
    let is_worktree =
        ob::detect_git_worktree(vault, &child_env).map_err(|error| {
            GmailError::runtime(
                "git",
                format!("could not inspect the vault repository: {error}"),
            )
        })?;
    let mut commit_paths = Vec::with_capacity(staged.len());
    for file in &staged {
        let relative = file.target.strip_prefix(vault).map_err(|_| {
            GmailError::runtime(
                "refused",
                "a staged compose note escaped the vault".to_string(),
            )
        })?;
        validate_target(vault, relative, file.target_existed)?;
        ensure_parent_dirs(vault, relative)?;
        commit_paths.push(relative.to_path_buf());
    }
    commit_paths.sort();
    commit_paths.dedup();
    capture::write_staged_files(&staged).map_err(|error| {
        GmailError::runtime(
            "write",
            format!("could not safely write compose notes: {}", error.message),
        )
    })?;
    if is_worktree {
        ob::commit_paths(vault, &child_env, message, &commit_paths).map_err(|error| {
            GmailError::runtime(
                "uncommitted",
                format!("compose note was written but could not be committed: {error}"),
            )
            .with_hint("the note remains in the vault; commit its path to finish the scoped write")
        })?;
    }
    Ok(())
}

fn resolve_note_path(
    vault: &Path,
    folder: &Path,
    input: &Path,
) -> Result<PathBuf, GmailError> {
    let input_text = input.to_str().ok_or_else(|| {
        GmailError::setup("note", "NOTE must be valid UTF-8".to_string())
    })?;
    let candidate = if input.is_absolute() {
        input
            .strip_prefix(vault)
            .map_err(|_| {
                GmailError::setup(
                    "note",
                    "NOTE must be inside the Bob vault".to_string(),
                )
            })?
            .to_path_buf()
    } else {
        let mut path = input.to_path_buf();
        if !path.starts_with(folder) {
            path = folder.join(path);
        }
        path
    };
    if candidate.as_os_str().is_empty()
        || candidate.is_absolute()
        || candidate
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(GmailError::setup(
            "note",
            "NOTE is not a safe vault-relative path".to_string(),
        ));
    }
    let candidate = if candidate.extension().is_none() {
        candidate.with_extension("md")
    } else {
        candidate
    };
    if candidate.starts_with(folder) && vault.join(&candidate).is_file() {
        return Ok(candidate);
    }
    let matches = find_note_names(vault, folder, input_text)?;
    match matches.as_slice() {
        [path] => Ok(path.clone()),
        [] => Err(GmailError::setup(
            "note",
            "NOTE is not a saved Gmail note".to_string(),
        )),
        _ => Err(GmailError::setup(
            "note",
            "NOTE matches more than one saved Gmail note".to_string(),
        )),
    }
}

fn find_note_names(
    vault: &Path,
    folder: &Path,
    requested: &str,
) -> Result<Vec<PathBuf>, GmailError> {
    let requested_path = Path::new(requested);
    let requested_name = requested_path.file_name().unwrap_or_default();
    let requested_stem = requested_path.file_stem().unwrap_or_default();
    let mut paths = Vec::new();
    collect_markdown_paths(&vault.join(folder), vault, &mut paths)?;
    Ok(paths
        .into_iter()
        .filter(|path| {
            path.file_name() == Some(requested_name)
                || path.file_stem() == Some(requested_stem)
        })
        .collect())
}

fn collect_markdown_paths(
    directory: &Path,
    vault: &Path,
    paths: &mut Vec<PathBuf>,
) -> Result<(), GmailError> {
    let entries = match fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(())
        }
        Err(error) => {
            return Err(GmailError::runtime(
                "path",
                format!("could not scan mail notes: {error}"),
            ))
        }
    };
    for entry in entries {
        let entry = entry.map_err(|error| {
            GmailError::runtime(
                "path",
                format!("could not scan mail notes: {error}"),
            )
        })?;
        let kind = entry.file_type().map_err(|error| {
            GmailError::runtime(
                "path",
                format!("could not inspect mail note path: {error}"),
            )
        })?;
        if kind.is_symlink() {
            continue;
        }
        let path = entry.path();
        if kind.is_dir() {
            collect_markdown_paths(&path, vault, paths)?;
        } else if kind.is_file()
            && path.extension().is_some_and(|extension| extension == "md")
        {
            let relative = path.strip_prefix(vault).map_err(|_| {
                GmailError::runtime(
                    "refused",
                    "a mail note escaped the vault".to_string(),
                )
            })?;
            paths.push(relative.to_path_buf());
        }
    }
    Ok(())
}

fn collect_occupied(
    vault: &Path,
    folder: &Path,
) -> Result<BTreeSet<PathBuf>, GmailError> {
    let mut occupied = BTreeSet::new();
    collect_entries(vault, folder, &mut occupied)?;
    Ok(occupied)
}

fn collect_entries(
    root: &Path,
    directory: &Path,
    occupied: &mut BTreeSet<PathBuf>,
) -> Result<(), GmailError> {
    let entries = match fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(())
        }
        Err(error) => {
            return Err(GmailError::runtime(
                "path",
                format!("could not scan Gmail folder: {error}"),
            ))
        }
    };
    for entry in entries {
        let entry = entry.map_err(|error| {
            GmailError::runtime(
                "path",
                format!("could not scan Gmail folder: {error}"),
            )
        })?;
        let kind = entry.file_type().map_err(|error| {
            GmailError::runtime(
                "path",
                format!("could not inspect Gmail path: {error}"),
            )
        })?;
        let path = entry.path();
        if let Ok(relative) = path.strip_prefix(root) {
            occupied.insert(relative.to_path_buf());
        }
        if kind.is_dir() {
            collect_entries(root, &path, occupied)?;
        }
    }
    Ok(())
}

fn validate_vault(vault: &Path) -> Result<(), GmailError> {
    if !vault.is_dir() {
        return Err(GmailError::setup(
            "vault",
            format!("Bob vault root does not exist: {}", vault.display()),
        ));
    }
    Ok(())
}

fn require_writer(config: &GmailConfig) -> Result<(), GmailError> {
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

fn validate_mail_folder(vault: &Path, folder: &Path) -> Result<(), GmailError> {
    note::path::validate_folder(folder).map_err(|_| {
        GmailError::runtime(
            "refused",
            "gmail.folder is not a safe relative path".to_string(),
        )
    })?;
    let mut current = vault.to_path_buf();
    for component in folder.components() {
        let Component::Normal(part) = component else {
            return Err(GmailError::runtime(
                "refused",
                "gmail.folder escaped the vault".to_string(),
            ));
        };
        current.push(part);
        match fs::symlink_metadata(&current) {
            Ok(metadata)
                if metadata.is_dir() && !metadata.file_type().is_symlink() => {}
            Ok(_) => {
                return Err(GmailError::runtime(
                    "refused",
                    "gmail.folder contains an unsafe path".to_string(),
                ))
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(())
            }
            Err(error) => {
                return Err(GmailError::runtime(
                    "path",
                    format!("could not inspect gmail.folder: {error}"),
                ))
            }
        }
    }
    Ok(())
}

fn validate_target(
    vault: &Path,
    relative: &Path,
    must_exist: bool,
) -> Result<(), GmailError> {
    if relative.as_os_str().is_empty()
        || relative.is_absolute()
        || relative
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(GmailError::runtime(
            "refused",
            "a compose path escaped the vault".to_string(),
        ));
    }
    let mut current = vault.to_path_buf();
    let components: Vec<_> = relative.components().collect();
    for (index, component) in components.iter().enumerate() {
        current.push(component.as_os_str());
        let final_component = index + 1 == components.len();
        match fs::symlink_metadata(&current) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                return Err(GmailError::runtime(
                    "refused",
                    "compose paths may not traverse symlinks".to_string(),
                ));
            }
            Ok(metadata) if final_component && metadata.is_file() => {
                if !must_exist {
                    return Err(GmailError::runtime(
                        "refused",
                        "compose target already exists".to_string(),
                    ));
                }
            }
            Ok(metadata) if !final_component && metadata.is_dir() => {}
            Ok(_) => {
                return Err(GmailError::runtime(
                    "refused",
                    "compose path is not a regular file path".to_string(),
                ))
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                if must_exist || index < components.len().saturating_sub(1) {
                    // A missing intermediate directory is allowed for new
                    // compose notes and created after all path checks pass.
                    if must_exist {
                        return Err(GmailError::runtime(
                            "refused",
                            "saved Gmail note path does not exist".to_string(),
                        ));
                    }
                }
            }
            Err(error) => {
                return Err(GmailError::runtime(
                    "path",
                    format!("could not inspect compose path: {error}"),
                ))
            }
        }
    }
    if must_exist {
        let metadata =
            fs::symlink_metadata(vault.join(relative)).map_err(|error| {
                GmailError::runtime(
                    "path",
                    format!("could not inspect saved Gmail note: {error}"),
                )
            })?;
        if !metadata.is_file() || metadata.file_type().is_symlink() {
            return Err(GmailError::runtime(
                "refused",
                "saved Gmail note is not a regular file".to_string(),
            ));
        }
    }
    Ok(())
}

fn ensure_parent_dirs(vault: &Path, relative: &Path) -> Result<(), GmailError> {
    let mut current = vault.to_path_buf();
    for component in relative
        .parent()
        .unwrap_or_else(|| Path::new(""))
        .components()
    {
        let Component::Normal(part) = component else {
            return Err(GmailError::runtime(
                "refused",
                "compose parent path escaped the vault".to_string(),
            ));
        };
        current.push(part);
        match fs::symlink_metadata(&current) {
            Ok(metadata)
                if metadata.is_dir() && !metadata.file_type().is_symlink() => {}
            Ok(_) => {
                return Err(GmailError::runtime(
                    "refused",
                    "compose parent is not a safe directory".to_string(),
                ))
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                fs::create_dir(&current).map_err(|error| {
                    GmailError::runtime(
                        "write",
                        format!("could not create compose folder: {error}"),
                    )
                })?;
            }
            Err(error) => {
                return Err(GmailError::runtime(
                    "path",
                    format!("could not inspect compose folder: {error}"),
                ))
            }
        }
    }
    Ok(())
}

fn maintenance_lock() -> Result<std::fs::File, GmailError> {
    ob::acquire_lock_waiting(Duration::from_secs(30), || {
        eprintln!("waiting for another vault maintenance run…");
    })
    .map_err(|error| {
        GmailError::runtime(
            "lock",
            format!("could not acquire the vault lock: {error}"),
        )
        .with_hint("another run held the lock for 30 s; try again")
    })
}

fn current_utc() -> DateTime<Utc> {
    let naive = bob_env::current_datetime();
    if let Some(timezone) = bob_env::configured_timezone()
        && let Some(value) = timezone
            .from_local_datetime(&naive)
            .single()
            .or_else(|| timezone.from_local_datetime(&naive).earliest())
    {
        return value.with_timezone(&Utc);
    }
    Local
        .from_local_datetime(&naive)
        .single()
        .or_else(|| Local.from_local_datetime(&naive).earliest())
        .map(|value| value.with_timezone(&Utc))
        .unwrap_or_else(Utc::now)
}

fn cursor_at(contents: &str, offset: usize) -> (usize, usize) {
    let prefix = &contents[..offset.min(contents.len())];
    let line = prefix.bytes().filter(|byte| *byte == b'\n').count();
    let column = prefix
        .rsplit('\n')
        .next()
        .unwrap_or_default()
        .encode_utf16()
        .count();
    (line, column)
}

fn report_success(
    command: &str,
    note: &str,
    (line, column): (usize, usize),
    format: HumanFormat,
) {
    if format.is_json() {
        let status = if command == "new" {
            "created"
        } else {
            "reply_added"
        };
        println!(
            "{}",
            serde_json::json!({
                "schema_version": 1,
                "ok": true,
                "status": status,
                "note": note,
                "cursor": {"line": line, "column": column},
            })
        );
    } else {
        println!(
            "{} · {note} · cursor line {line}, column {column}",
            if command == "new" {
                "Created compose note"
            } else {
                "Added Reply section"
            }
        );
    }
    ui::flush_stdout();
}

fn path_to_slashes(path: &Path) -> String {
    path.components()
        .map(|component| component.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn staged_reply_refuses_a_stale_note_preimage() {
        let temp = tempfile::tempdir().expect("temporary vault");
        let note = temp.path().join("mail/thread.md");
        fs::create_dir_all(note.parent().expect("note parent")).unwrap();
        fs::write(&note, "planned source\n").unwrap();
        let staged = StagedTextFile {
            target: note.clone(),
            target_existed: true,
            original_target: "planned source\n".to_string(),
            updated_target: "planned source\n\n## Reply\n\n".to_string(),
        };
        fs::write(&note, "authored edit after planning\n").unwrap();

        let result =
            write_and_commit(temp.path(), vec![staged], "gmail: reply");

        assert!(result.is_err());
        assert_eq!(
            fs::read_to_string(note).unwrap(),
            "authored edit after planning\n"
        );
    }
}
