//! On-demand import of one attachment listed in a saved mail note.

use std::{
    fs::{self, OpenOptions},
    io::Write,
    path::{Component, Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    time::Duration,
};

use serde_json::json;

use super::{
    body::{self, FetchedBodies},
    cli::AttachArgs,
    config::GmailConfig,
    gog::Gog,
    ids,
    note::{self, parse::ParsedNote},
    resolve, ui, GmailError,
};
use crate::native::{
    attachments::journal as attachment_journal,
    capture::{self, StagedTextFile},
    capture_clip, ob,
    style::Styler,
    vault_sync,
};

const MAX_ATTACHMENT_BYTES: u64 = 25 * 1024 * 1024;
static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);

#[derive(Clone)]
struct AttachmentLine {
    number: u32,
    message_id: String,
    range: std::ops::Range<usize>,
    imported_path: Option<String>,
}

struct AttachmentTarget {
    message_id: String,
    attachment_id: String,
    filename: String,
    declared_size: Option<u64>,
}

struct PlannedAttachment {
    relative: PathBuf,
    reused: bool,
    expected_hash: String,
}

pub(crate) fn run(args: &AttachArgs) -> i32 {
    match run_inner(args) {
        Ok((path, size, status, warning)) => {
            print_report(args, &path, size, status, warning.as_deref());
            0
        }
        Err(error) => ui::report_error("attach", &error, args.format.as_str()),
    }
}

fn run_inner(
    args: &AttachArgs,
) -> Result<(String, u64, &'static str, Option<String>), GmailError> {
    if args.number == 0 {
        return Err(GmailError::setup(
            "attachment_number",
            "attachment number must be greater than zero".to_string(),
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
    let folder = Path::new(config.folder());
    let index = resolve::scan_index(&vault, folder, config.link_label())?;
    let gog = Gog::resolve(
        crate::native::google::GMAIL,
        config.gog(),
        config.timeout(),
    )?;
    gog.require_version()?;
    let account = resolve::resolve_account(&gog, None, config.account())?;
    let identity = resolve::resolve_identity(
        &gog, &account, &vault, folder, &index, &args.note,
    )?;
    let note_path = identity.known_path.clone().ok_or_else(|| {
        GmailError::setup(
            "not_found",
            "attach needs a saved mail note".to_string(),
        )
        .with_hint("run `bob gmail save` before importing an attachment")
    })?;
    let note_relative = safe_relative_path(&note_path)?;
    validate_existing_parent_dirs(&vault, &note_relative)?;
    let note_absolute = vault.join(&note_relative);
    let note_metadata =
        fs::symlink_metadata(&note_absolute).map_err(|error| {
            GmailError::runtime(
                "note",
                format!("could not inspect the saved mail note: {error}"),
            )
        })?;
    if note_metadata.file_type().is_symlink() || !note_metadata.is_file() {
        return Err(GmailError::runtime(
            "refused",
            "the selected mail note is not a regular in-vault file".to_string(),
        ));
    }
    let original = fs::read_to_string(&note_absolute).map_err(|error| {
        GmailError::runtime(
            "note",
            format!("could not read the saved mail note: {error}"),
        )
    })?;
    let parsed = note::parse::parse(&original).map_err(|error| {
        GmailError::runtime(
            "refused",
            format!("cannot attach to this mail note: {error}"),
        )
    })?;
    if !parsed.account.eq_ignore_ascii_case(&identity.account)
        || !parsed.thread_id.eq_ignore_ascii_case(&identity.thread_id)
    {
        return Err(GmailError::runtime(
            "refused",
            "the saved mail note identity changed during attach".to_string(),
        ));
    }
    let mut lines = attachment_lines(&original, &parsed);
    let matching: Vec<_> = lines
        .iter()
        .filter(|line| line.number == args.number)
        .cloned()
        .collect();
    if matching.len() > 1 {
        return Err(GmailError::runtime(
            "ambiguous",
            "the selected attachment number appears more than once in the note"
                .to_string(),
        ));
    }
    if let Some(line) = matching.first()
        && let Some(imported) = &line.imported_path
    {
        validate_imported_target(imported, &parsed.thread_id)?;
        let relative = safe_relative_path(imported)?;
        let absolute = vault.join(&relative);
        match fs::symlink_metadata(&absolute) {
            Ok(metadata)
                if metadata.file_type().is_symlink() || !metadata.is_file() =>
            {
                return Err(GmailError::runtime(
                    "refused",
                    "the linked attachment is not a regular in-vault file"
                        .to_string(),
                ));
            }
            Ok(_) => {
                let bytes = fs::read(&absolute).map_err(|error| {
                    GmailError::runtime(
                        "attachment",
                        format!(
                            "could not read the linked attachment: {error}"
                        ),
                    )
                })?;
                let digest = capture_clip::sha256_hex(&bytes);
                let source =
                    ids::canonical_url(&identity.account, &identity.thread_id);
                let warning = if args.dry_run {
                    None
                } else {
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
                    commit_attachment_paths(&vault, &note_relative, &relative)?;
                    ensure_origin(&vault, &relative, &source, &digest)?
                };
                let path = path_to_slashes(&relative);
                return Ok((
                    path,
                    bytes.len() as u64,
                    if args.dry_run {
                        "already_imported"
                    } else {
                        "unchanged"
                    },
                    warning,
                ));
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(GmailError::runtime(
                    "attachment",
                    format!("could not inspect the linked attachment: {error}"),
                ));
            }
        }
    }

    let confirmed = resolve::fetch_confirmed_thread(&gog, identity)?;
    let target = find_attachment(&confirmed.thread, args.number)?;
    if matching.first().is_some_and(|line| {
        !line.message_id.eq_ignore_ascii_case(&target.message_id)
    }) {
        return Err(GmailError::runtime(
            "refused",
            "the selected attachment row no longer matches the Gmail message"
                .to_string(),
        ));
    }
    let filename = sanitize_filename(&target.filename);
    if target
        .declared_size
        .is_some_and(|size| size > MAX_ATTACHMENT_BYTES)
    {
        return Err(too_large());
    }
    let bytes = gog.attachment_bytes(
        &confirmed.identity.account,
        &target.message_id,
        &target.attachment_id,
    )?;
    if bytes.len() as u64 > MAX_ATTACHMENT_BYTES {
        return Err(too_large());
    }
    let digest = capture_clip::sha256_hex(&bytes);
    let relative = matching
        .first()
        .and_then(|line| line.imported_path.as_deref())
        .map(safe_relative_path)
        .transpose()?
        .unwrap_or_else(|| {
            PathBuf::from("file")
                .join("mail")
                .join(&parsed.thread_id)
                .join(&filename)
        });
    validate_imported_target(&path_to_slashes(&relative), &parsed.thread_id)?;
    let planned = choose_destination(&vault, relative, &digest)?;
    let saved = path_to_slashes(&planned.relative);
    let size = bytes.len() as u64;
    if args.dry_run {
        return Ok((saved, size, "would_import", None));
    }

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
    let current = fs::read_to_string(&note_absolute).map_err(|error| {
        GmailError::runtime(
            "note_changed",
            format!("could not re-read the saved mail note: {error}"),
        )
    })?;
    if current != original {
        return Err(GmailError::runtime(
            "note_changed",
            "the saved mail note changed during attachment download; retry"
                .to_string(),
        ));
    }
    let current_parsed = note::parse::parse(&current).map_err(|error| {
        GmailError::runtime(
            "note_changed",
            format!("the saved mail note changed during attachment download: {error}"),
        )
    })?;
    lines = attachment_lines(&current, &current_parsed);
    let line = lines
        .iter()
        .find(|line| line.number == args.number)
        .ok_or_else(|| {
            GmailError::runtime(
                "note_changed",
                "the selected attachment row disappeared during download"
                    .to_string(),
            )
        })?;
    if line.imported_path.is_some() {
        return Err(GmailError::runtime(
            "note_changed",
            "the selected attachment row was imported during download; retry"
                .to_string(),
        ));
    }
    let created = if planned.reused {
        false
    } else {
        install_attachment(&vault, &planned.relative, &bytes)?
    };
    let new_line =
        format!("> 📎 [[{}|{}]] · {}", saved, filename, format_size(size));
    let mut updated = current.clone();
    updated.replace_range(line.range.clone(), &new_line);
    let note_staged = [StagedTextFile {
        target: note_absolute.clone(),
        target_existed: true,
        original_target: current,
        updated_target: updated,
    }];
    if let Err(error) = capture::write_staged_files(&note_staged) {
        if created {
            cleanup_created(&vault, &planned.relative, &planned.expected_hash);
        }
        return Err(GmailError::runtime(
            "write",
            format!("could not safely update the mail note: {}", error.message),
        ));
    }

    commit_attachment_paths(&vault, &note_relative, &planned.relative)?;
    let source =
        ids::canonical_url(&confirmed.identity.account, &parsed.thread_id);
    let warning = ensure_origin(&vault, &planned.relative, &source, &digest)?;
    Ok((
        saved,
        size,
        if planned.reused { "reused" } else { "imported" },
        warning,
    ))
}

fn find_attachment(
    thread: &super::model::GmailThread,
    wanted: u32,
) -> Result<AttachmentTarget, GmailError> {
    let fetched = FetchedBodies::default();
    let mut number = 1u32;
    for message in body::importable_messages(thread) {
        let rendered =
            body::render_message(message, &fetched).map_err(|error| {
                GmailError::runtime(
                    "bad_body",
                    format!("could not read attachment metadata: {error}"),
                )
            })?;
        for attachment in rendered.attachments {
            if number == wanted {
                let attachment_id = attachment
                    .attachment_id
                    .filter(|value| !value.is_empty())
                    .ok_or_else(|| {
                        GmailError::runtime(
                            "unavailable",
                            "the selected attachment has no downloadable Gmail id".to_string(),
                        )
                    })?;
                return Ok(AttachmentTarget {
                    message_id: message.id.clone(),
                    attachment_id,
                    filename: attachment
                        .filename
                        .filter(|value| !value.is_empty())
                        .unwrap_or_else(|| "attachment".to_string()),
                    declared_size: attachment.size,
                });
            }
            number = number.saturating_add(1);
        }
    }
    Err(GmailError::runtime(
        "not_found",
        format!("mail note has no attachment numbered {wanted}"),
    ))
}

fn attachment_lines(
    contents: &str,
    parsed: &ParsedNote,
) -> Vec<AttachmentLine> {
    let mut result = Vec::new();
    let mut offset = 0usize;
    let mut next_number = 1u32;
    for segment in contents.split_inclusive('\n') {
        let raw_end = offset + segment.len();
        let mut content_end = if segment.ends_with('\n') {
            raw_end - 1
        } else {
            raw_end
        };
        if content_end > offset && contents.as_bytes()[content_end - 1] == b'\r'
        {
            content_end -= 1;
        }
        if let Some(card) = parsed.cards.iter().find(|card| {
            offset >= card.card_range.start && offset < card.card_range.end
        }) {
            let line = &contents[offset..content_end];
            let body = line.strip_prefix("> ").unwrap_or(line);
            if let Some((number, value)) = parse_attachment_row(body) {
                let imported_path = wiki_target(value);
                next_number = number.saturating_add(1);
                result.push(AttachmentLine {
                    number,
                    message_id: card.id.clone(),
                    range: offset..content_end,
                    imported_path,
                });
            } else if let Some(value) = parse_imported_attachment_row(body) {
                let number = next_number;
                next_number = next_number.saturating_add(1);
                result.push(AttachmentLine {
                    number,
                    message_id: card.id.clone(),
                    range: offset..content_end,
                    imported_path: wiki_target(value),
                });
            }
        }
        offset = raw_end;
    }
    result
}

fn parse_imported_attachment_row(line: &str) -> Option<&str> {
    let rest = line.strip_prefix("📎 ")?;
    rest.starts_with("[[").then_some(rest)
}

fn parse_attachment_row(line: &str) -> Option<(u32, &str)> {
    let rest = line.strip_prefix("📎 ")?;
    let (number, value) = rest.split_once(". ")?;
    let number = number.parse::<u32>().ok()?;
    Some((number, value))
}

fn wiki_target(value: &str) -> Option<String> {
    let start = value.find("[[")? + 2;
    let end = value[start..].find("]]")? + start;
    let target = value[start..end].split('|').next()?.trim();
    (!target.is_empty()).then(|| target.to_string())
}

fn validate_imported_target(
    path: &str,
    thread_id: &str,
) -> Result<(), GmailError> {
    let relative = safe_relative_path(path)?;
    let expected = Path::new("file")
        .join("mail")
        .join(thread_id.to_ascii_lowercase());
    if relative.parent() != Some(expected.as_path()) {
        return Err(GmailError::runtime(
            "refused",
            "the attachment link does not point into its Gmail thread folder"
                .to_string(),
        ));
    }
    Ok(())
}

fn sanitize_filename(raw: &str) -> String {
    let basename = raw.rsplit(['/', '\\']).next().unwrap_or(raw);
    let sanitized = capture_clip::sanitize_file_name(basename);
    let mut chars = sanitized.chars();
    let mut output: String = chars.by_ref().take(120).collect();
    while output.ends_with('.') || output.ends_with(' ') {
        output.pop();
    }
    if output.is_empty() {
        "attachment".to_string()
    } else {
        output
    }
}

fn choose_destination(
    vault: &Path,
    initial: PathBuf,
    digest: &str,
) -> Result<PlannedAttachment, GmailError> {
    let initial = safe_relative_path(&path_to_slashes(&initial))?;
    validate_existing_parent_dirs(vault, &initial)?;
    let target = vault.join(&initial);
    match fs::symlink_metadata(&target) {
        Ok(metadata)
            if metadata.file_type().is_symlink() || !metadata.is_file() =>
        {
            Err(GmailError::runtime(
                "refused",
                "the attachment destination is not a regular in-vault file"
                    .to_string(),
            ))
        }
        Ok(_) => {
            let old = fs::read(&target).map_err(|error| {
                GmailError::runtime(
                    "attachment",
                    format!("could not read the existing attachment: {error}"),
                )
            })?;
            if capture_clip::sha256_hex(&old) == digest {
                return Ok(PlannedAttachment {
                    relative: initial,
                    reused: true,
                    expected_hash: digest.to_string(),
                });
            }
            let hashed = with_hash_suffix(&initial, &digest[..8]);
            let hashed_abs = vault.join(&hashed);
            match fs::symlink_metadata(&hashed_abs) {
                Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_file() => {
                    Err(GmailError::runtime(
                        "refused",
                        "the hashed attachment destination is not a regular file".to_string(),
                    ))
                }
                Ok(_) => {
                    let existing = fs::read(&hashed_abs).map_err(|error| {
                        GmailError::runtime(
                            "attachment",
                            format!("could not read the existing attachment: {error}"),
                        )
                    })?;
                    if capture_clip::sha256_hex(&existing) == digest {
                        Ok(PlannedAttachment {
                            relative: hashed,
                            reused: true,
                            expected_hash: digest.to_string(),
                        })
                    } else {
                        Err(GmailError::runtime(
                            "conflict",
                            "the attachment filename already exists with different content".to_string(),
                        ))
                    }
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    Ok(PlannedAttachment {
                        relative: hashed,
                        reused: false,
                        expected_hash: digest.to_string(),
                    })
                }
                Err(error) => Err(GmailError::runtime(
                    "attachment",
                    format!("could not inspect the hashed attachment path: {error}"),
                )),
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            Ok(PlannedAttachment {
                relative: initial,
                reused: false,
                expected_hash: digest.to_string(),
            })
        }
        Err(error) => Err(GmailError::runtime(
            "attachment",
            format!("could not inspect the attachment destination: {error}"),
        )),
    }
}

fn install_attachment(
    vault: &Path,
    relative: &Path,
    bytes: &[u8],
) -> Result<bool, GmailError> {
    ensure_parent_dirs(vault, relative)?;
    let target = vault.join(relative);
    match fs::symlink_metadata(&target) {
        Ok(metadata)
            if metadata.file_type().is_symlink() || !metadata.is_file() =>
        {
            return Err(GmailError::runtime(
                "refused",
                "the attachment destination is not a regular in-vault file"
                    .to_string(),
            ));
        }
        Ok(_) => {
            let existing = fs::read(&target).map_err(|error| {
                GmailError::runtime(
                    "attachment",
                    format!("could not read the existing attachment: {error}"),
                )
            })?;
            if existing == bytes {
                return Ok(false);
            }
            return Err(GmailError::runtime(
                "note_changed",
                "the attachment destination changed during download; retry"
                    .to_string(),
            ));
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => {
            return Err(GmailError::runtime(
                "attachment",
                format!(
                    "could not inspect the attachment destination: {error}"
                ),
            ));
        }
    }
    let parent = target.parent().unwrap_or(vault);
    let temp = parent.join(format!(
        ".bob-gmail-attachment-{}-{}.tmp",
        std::process::id(),
        TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed)
    ));
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temp)
        .map_err(|error| {
            GmailError::runtime(
                "attachment",
                format!("could not stage the attachment: {error}"),
            )
        })?;
    if let Err(error) = file.write_all(bytes).and_then(|()| file.sync_all()) {
        let _ = fs::remove_file(&temp);
        return Err(GmailError::runtime(
            "attachment",
            format!("could not stage the attachment: {error}"),
        ));
    }
    drop(file);
    match fs::hard_link(&temp, &target) {
        Ok(()) => {
            let _ = fs::remove_file(temp);
            Ok(true)
        }
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            let _ = fs::remove_file(temp);
            let same = fs::symlink_metadata(&target).ok().is_some_and(|meta| {
                !meta.file_type().is_symlink()
                    && meta.is_file()
                    && fs::read(&target).ok().is_some_and(|old| old == bytes)
            });
            if same {
                Ok(false)
            } else {
                Err(GmailError::runtime(
                    "note_changed",
                    "an attachment appeared during download; retry".to_string(),
                ))
            }
        }
        Err(error) => {
            let _ = fs::remove_file(temp);
            Err(GmailError::runtime(
                "attachment",
                format!("could not install the attachment: {error}"),
            ))
        }
    }
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
                "the path escaped the vault".to_string(),
            ));
        };
        current.push(part);
        match fs::symlink_metadata(&current) {
            Ok(metadata)
                if metadata.is_dir() && !metadata.file_type().is_symlink() => {}
            Ok(_) => {
                return Err(GmailError::runtime(
                    "refused",
                    "a path parent is not a safe in-vault directory"
                        .to_string(),
                ));
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => break,
            Err(error) => {
                return Err(GmailError::runtime(
                    "attachment",
                    format!("could not inspect a path parent: {error}"),
                ));
            }
        }
    }
    Ok(())
}

fn ensure_parent_dirs(vault: &Path, relative: &Path) -> Result<(), GmailError> {
    let parent = relative.parent().unwrap_or_else(|| Path::new(""));
    let mut current = vault.to_path_buf();
    for component in parent.components() {
        let Component::Normal(part) = component else {
            return Err(GmailError::runtime(
                "refused",
                "the attachment path escaped the vault".to_string(),
            ));
        };
        current.push(part);
        match fs::symlink_metadata(&current) {
            Ok(metadata)
                if metadata.is_dir() && !metadata.file_type().is_symlink() => {}
            Ok(_) => {
                return Err(GmailError::runtime(
                    "refused",
                    "an attachment parent is not a safe directory".to_string(),
                ));
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                fs::create_dir(&current).map_err(|error| {
                    GmailError::runtime(
                        "attachment",
                        format!(
                            "could not create an attachment folder: {error}"
                        ),
                    )
                })?;
            }
            Err(error) => {
                return Err(GmailError::runtime(
                    "attachment",
                    format!("could not inspect an attachment folder: {error}"),
                ));
            }
        }
    }
    Ok(())
}

fn cleanup_created(vault: &Path, relative: &Path, expected_hash: &str) {
    let target = vault.join(relative);
    if fs::read(&target)
        .ok()
        .is_some_and(|bytes| capture_clip::sha256_hex(&bytes) == expected_hash)
    {
        let _ = fs::remove_file(target);
    }
}

fn ensure_origin(
    vault: &Path,
    relative: &Path,
    source: &str,
    digest: &str,
) -> Result<Option<String>, GmailError> {
    let (events, _) =
        attachment_journal::read_journal(vault).map_err(|error| {
            GmailError::runtime(
                "journal",
                format!(
                    "could not read the attachment origin journal: {error}"
                ),
            )
        })?;
    let saved = path_to_slashes(relative);
    if events.iter().any(|event| {
        event.saved == saved && event.sha256 == digest && event.via == "gmail"
    }) {
        return Ok(None);
    }
    if events.iter().any(|event| {
        event.saved == saved && event.via == "gmail" && event.sha256 != digest
    }) {
        return Err(GmailError::runtime(
            "changed",
            "the imported attachment has changed since Gmail saved it; the local file was left untouched".to_string(),
        ));
    }
    let host = attachment_journal::host_label();
    let display = source.to_string();
    attachment_journal::append_event(
        vault,
        &host,
        &saved,
        source,
        &display,
        digest,
        "gmail",
        attachment_journal::event_timestamp(),
    )
    .map_err(|error| {
        GmailError::runtime(
            "journal",
            format!("could not record the attachment origin: {error}"),
        )
        .with_hint("run `bob gmail attach` again to complete the journal entry")
    })?;
    Ok(None)
}

fn safe_relative_path(text: &str) -> Result<PathBuf, GmailError> {
    let path = PathBuf::from(text);
    if path.as_os_str().is_empty()
        || path.is_absolute()
        || path
            .components()
            .any(|part| !matches!(part, Component::Normal(_)))
    {
        return Err(GmailError::runtime(
            "refused",
            "an attachment path escaped the vault".to_string(),
        ));
    }
    Ok(path)
}

fn with_hash_suffix(path: &Path, hash: &str) -> PathBuf {
    let stem = path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or("attachment");
    let extension = path.extension().and_then(|extension| extension.to_str());
    let name = match extension.filter(|extension| !extension.is_empty()) {
        Some(extension) => format!("{stem}-{hash}.{extension}"),
        None => format!("{stem}-{hash}"),
    };
    path.with_file_name(name)
}

fn path_to_slashes(path: &Path) -> String {
    path.components()
        .map(|component| component.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

fn format_size(bytes: u64) -> String {
    if bytes < 1024 {
        format!("{bytes} B")
    } else if bytes < 1024 * 1024 {
        format!("{} KB", (bytes + 512) / 1024)
    } else {
        format!("{} MB", (bytes + 512 * 1024) / (1024 * 1024))
    }
}

fn too_large() -> GmailError {
    GmailError::runtime(
        "too_large",
        "the attachment exceeds the 25 MiB import limit".to_string(),
    )
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

fn commit_attachment_paths(
    vault: &Path,
    note_relative: &Path,
    attachment_relative: &Path,
) -> Result<(), GmailError> {
    let child_env = ob::child_env();
    let is_worktree = ob::detect_git_worktree(vault, &child_env).map_err(|error| {
        GmailError::runtime(
            "git",
            format!(
                "could not determine whether the vault is a Git worktree: {error}"
            ),
        )
    })?;
    if !is_worktree {
        return Ok(());
    }
    let mut paths = vec![
        note_relative.to_path_buf(),
        attachment_relative.to_path_buf(),
    ];
    paths.sort();
    paths.dedup();
    ob::commit_paths(vault, &child_env, "gmail: import attachment", &paths)
        .map_err(|error| {
            GmailError::runtime(
                "uncommitted",
                format!(
                    "the attachment was written but could not be committed: {error}"
                ),
            )
            .with_hint("run `bob gmail attach` again to finish the scoped commit")
        })
        .map(|_| ())
}

fn print_report(
    args: &AttachArgs,
    path: &str,
    size: u64,
    status: &str,
    warning: Option<&str>,
) {
    if args.format.is_json() {
        println!(
            "{}",
            json!({
                "schema_version": 1,
                "ok": true,
                "dry_run": args.dry_run,
                "path": path,
                "size": size,
                "status": status,
                "warning": warning,
            })
        );
        ui::flush_stdout();
        return;
    }
    let styler = Styler::detect();
    let action = match status {
        "would_import" => "would import",
        "already_imported" | "unchanged" => "already imported",
        "reused" => "reused",
        _ => "imported",
    };
    println!("{} · {} · {}", styler.cyan(action), path, format_size(size));
    if let Some(warning) = warning {
        eprintln!("{}", styler.dim(warning));
    }
    ui::flush_stdout();
}
