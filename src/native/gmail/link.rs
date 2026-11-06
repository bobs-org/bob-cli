//! `bob gmail link`: swap Gmail URLs for labelled wikilinks, and backfill.
//!
//! Named notes (or the whole vault with `-a`) are scanned for Gmail URLs.
//! Each confirmed thread is saved through the shared save boundary, then
//! exactly the URL tokens that still hold the same text are replaced with
//! labelled wikilinks under the vault lock. Dead links are remembered by URL
//! digest in the per-host journal so later sweeps skip them; sign-in and
//! network failures set a host-local marker instead so they are reported
//! once. `-d` prints a unified diff and touches nothing.

use std::{
    collections::{BTreeMap, BTreeSet},
    ffi::OsStr,
    fs,
    path::{Path, PathBuf},
    time::Duration,
};

use chrono::{DateTime, Utc};
use serde_json::json;

use super::{
    cli::LinkArgs,
    config::GmailConfig,
    gog::Gog,
    ids::{self, GmailVerdict, GmailWrapper},
    note::index::MailIndex,
    policy::{self, NeverSavePolicy},
    prepare, resolve,
    save::{self, PreparedSave},
    ui, GmailError,
};
use crate::native::{
    attachments::journal as attachment_journal,
    capture::{self, CaptureFailureKind, StagedTextFile},
    env as bob_env, is_always_excluded_note_directory_name, ob, pref_journal,
    style::Styler,
    vault_sync,
};

/// Journal event kind for remembered dead links. The value carries only the
/// URL digest and the reason: never the URL, which holds a Gmail token.
const LINK_FAILED_KIND: &str = "gmail.link_failed";

/// Failure reasons remembered by URL digest; later sweeps skip them.
const DEAD_REASONS: &[&str] =
    &["gone", "refused", "never_save", "unresolvable"];

/// Failures that set the host-local marker instead of the journal: they may
/// clear on their own, so they are reported once, not remembered.
const GATE_KINDS: &[&str] =
    &["awaiting_sign_in", "network", "timeout", "api_disabled"];

/// Vault directories never scanned by `-a`, besides the mail folder itself
/// and any dot directory.
const ALWAYS_SKIPPED_DIRS: &[&str] = &["_conflicts", "_generated", "done"];

/// One Gmail URL token found in a source note.
struct FoundUrl {
    note: PathBuf,
    start: usize,
    end: usize,
    url: String,
    wrapper: GmailWrapper,
    verdict: GmailVerdict,
}

struct AppliedSwap {
    note: String,
    digest: String,
    link: String,
}

struct DroppedSwap {
    note: String,
    digest: String,
    link: String,
}

struct UrlFailure {
    digest: Option<String>,
    note: Option<String>,
    kind: String,
    message: String,
    dead: bool,
}

struct SavedEntry {
    account: String,
    thread_id: String,
    note: String,
    link: String,
    status: &'static str,
    message_count: usize,
    added_count: usize,
}

struct LinkOutcome {
    dry_run: bool,
    scanned_notes: usize,
    found_urls: usize,
    skipped_dead: usize,
    saved: Vec<SavedEntry>,
    swaps: Vec<AppliedSwap>,
    needs_review: Vec<DroppedSwap>,
    failures: Vec<UrlFailure>,
    diffs: Vec<(String, String)>,
    would_create: Vec<(String, usize)>,
    warnings: Vec<String>,
}

/// Run the link command.
pub(crate) fn run(args: &LinkArgs) -> i32 {
    if super::worker::is_worker() {
        return super::worker::run();
    }
    run_direct(args)
}

pub(super) fn run_direct(args: &LinkArgs) -> i32 {
    match run_inner(args) {
        Ok(outcome) => {
            print_report(args, &outcome);
            0
        }
        Err(error) => ui::report_error(
            "link",
            &error,
            if args.format.is_json() {
                "json"
            } else {
                "human"
            },
        ),
    }
}

fn run_inner(args: &LinkArgs) -> Result<LinkOutcome, GmailError> {
    if args.notes.is_empty() && !args.all {
        return Err(GmailError::setup(
            "usage",
            "link needs at least one NOTE or -a to scan the vault".to_string(),
        )
        .with_hint("pass note paths, or `bob gmail link -a -d` to preview"));
    }
    let config = GmailConfig::resolve()?;
    let vault = args.bob_dir();
    if !vault.is_dir() {
        return Err(GmailError::setup(
            "vault",
            format!("Bob vault root does not exist: {}", vault.display()),
        ));
    }
    let host = vault_sync::hostname_slug();
    if !args.dry_run
        && let Some(writer) = config.writer()
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
    let folder = Path::new(config.folder()).to_path_buf();
    let scope = collect_scope_notes(&vault, &folder, args)?;
    let mut contents = BTreeMap::new();
    let mut read_failures = Vec::new();
    for note in &scope {
        match fs::read_to_string(note) {
            Ok(text) => {
                contents.insert(note.clone(), text);
            }
            Err(error) => read_failures.push(UrlFailure {
                digest: None,
                note: Some(relative_slashes(&vault, note)),
                kind: "note".to_string(),
                message: format!("could not read note: {error}"),
                dead: false,
            }),
        }
    }

    let mut found = Vec::new();
    for (note, text) in &contents {
        for found_url in ids::find_gmail_urls(text) {
            if inline_link_destination(text, &found_url) {
                continue;
            }
            found.push(FoundUrl {
                note: note.clone(),
                start: found_url.start,
                end: found_url.end,
                url: found_url.url.clone(),
                wrapper: found_url.wrapper.clone(),
                verdict: found_url.verdict.clone(),
            });
        }
    }

    let mut outcome = LinkOutcome {
        dry_run: args.dry_run,
        scanned_notes: scope.len(),
        found_urls: found.len(),
        skipped_dead: 0,
        saved: Vec::new(),
        swaps: Vec::new(),
        needs_review: Vec::new(),
        failures: read_failures,
        diffs: Vec::new(),
        would_create: Vec::new(),
        warnings: Vec::new(),
    };
    if found.is_empty() {
        return Ok(outcome);
    }

    let gog = Gog::resolve(crate::native::google::GMAIL, config.timeout())?;
    gog.require_version()?;
    let account = match resolve::resolve_account(&gog, None, config.account()) {
        Ok(account) => account,
        Err(error) => {
            return Err(gate_error(&error));
        }
    };
    let body_mode = config.body();
    let initial_index =
        resolve::scan_index(&vault, &folder, config.link_label())?;
    let never_save = NeverSavePolicy::from_config(&config);
    let dead = dead_digests(&vault);

    // Group distinct URLs: every occurrence shares one resolution, and
    // offline-dead URLs never touch the network.
    let mut by_url: BTreeMap<String, (GmailVerdict, Vec<usize>)> =
        BTreeMap::new();
    for (position, found_url) in found.iter().enumerate() {
        by_url
            .entry(found_url.url.clone())
            .or_insert_with(|| (found_url.verdict.clone(), Vec::new()))
            .1
            .push(position);
    }
    let mut candidates: BTreeMap<String, Vec<usize>> = BTreeMap::new();
    for (url, (verdict, positions)) in &by_url {
        let digest = save::sha256(url.as_bytes());
        if dead.contains(&digest) {
            outcome.skipped_dead += positions.len();
            continue;
        }
        match verdict {
            GmailVerdict::Resolvable(_) => {
                candidates.insert(url.clone(), positions.clone());
            }
            GmailVerdict::Unresolvable { reason } => {
                outcome.failures.push(UrlFailure {
                    digest: Some(digest.clone()),
                    note: first_note(&vault, &found, positions[0]),
                    kind: "unresolvable".to_string(),
                    message: reason.clone(),
                    dead: true,
                });
            }
            GmailVerdict::Refused { reason } => {
                outcome.failures.push(UrlFailure {
                    digest: Some(digest.clone()),
                    note: first_note(&vault, &found, positions[0]),
                    kind: "refused".to_string(),
                    message: reason.clone(),
                    dead: true,
                });
            }
        }
    }

    // Resolve each distinct URL once; every occurrence shares the result.
    let mut works: BTreeMap<(String, String), PreparedSave> = BTreeMap::new();
    let mut url_keys: BTreeMap<String, (String, String)> = BTreeMap::new();
    let mut gate_failure = false;
    for (url, positions) in &candidates {
        let note = first_note(&vault, &found, positions[0]);
        match resolve_url(
            &gog,
            &account,
            &vault,
            &folder,
            &initial_index,
            &never_save,
            body_mode,
            url,
        ) {
            Ok(prepared) => {
                let key = (
                    prepared.confirmed.identity.account.clone(),
                    prepared.confirmed.identity.thread_id.clone(),
                );
                url_keys.insert(url.clone(), key.clone());
                works.entry(key).or_insert(prepared);
            }
            Err(error) => {
                let kind = error.kind().to_string();
                if GATE_KINDS.contains(&kind.as_str()) {
                    gate_failure = true;
                }
                outcome.failures.push(UrlFailure {
                    digest: Some(save::sha256(url.as_bytes())),
                    note,
                    kind: kind.clone(),
                    message: error.message().to_string(),
                    dead: DEAD_REASONS.contains(&kind.as_str()),
                });
            }
        }
    }
    if gate_failure {
        note_gate_failure();
    }

    let timezone = bob_env::configured_timezone().unwrap_or(chrono_tz::UTC);
    let saved_at = save::current_utc();

    if args.dry_run {
        let plans = save::plan_prepared_saves(
            &vault,
            &folder,
            config.link_label(),
            &mut works,
            &initial_index,
            None,
            false,
            saved_at,
            timezone,
        )?;
        fill_saved_entries(&vault, &plans, true, &mut outcome);
        let targets = link_targets(&vault, &plans);
        let grouped = build_candidates(&found, &url_keys);
        let (_, applied, dropped, diffs) =
            plan_source_swaps(&vault, &grouped, &targets, true)?;
        outcome.diffs = diffs;
        outcome.swaps = applied;
        outcome.needs_review = dropped;
        return Ok(outcome);
    }

    commit_link(
        &vault,
        &host,
        &folder,
        config.link_label(),
        &mut works,
        &found,
        &url_keys,
        saved_at,
        timezone,
        &mut outcome,
    )?;
    if !gate_failure {
        clear_gate_failure();
    }
    Ok(outcome)
}

/// Resolve one URL through the shared save resolver and prepare its thread.
/// Everything network-bound finishes here, before the vault lock exists.
fn resolve_url(
    gog: &Gog,
    account: &str,
    vault: &Path,
    folder: &Path,
    index: &MailIndex,
    never_save: &NeverSavePolicy,
    body_mode: crate::native::config::GmailBodyMode,
    url: &str,
) -> Result<PreparedSave, GmailError> {
    let identity =
        resolve::resolve_identity(gog, account, vault, folder, index, url)?;
    let confirmed = resolve::fetch_confirmed_thread(gog, identity)?;
    policy::check(gog, account, &confirmed.thread, never_save)?;
    let prepared =
        prepare::prepare_thread(gog, account, &confirmed.thread, body_mode)?;
    Ok(PreparedSave {
        confirmed,
        prepared,
        body_mode,
    })
}

/// A sign-in or network failure at account resolution: the sweep cannot run.
/// The first occurrence is reported in full; repeats stay terse so detached
/// sweeps do not spam, but the exit code still signals the failure.
fn gate_error(error: &GmailError) -> GmailError {
    if GATE_KINDS.contains(&error.kind()) {
        if gate_marker_present() {
            return GmailError::runtime(
                error.kind(),
                format!(
                    "{} (already reported; sign in, then retry)",
                    error.message()
                ),
            );
        }
        note_gate_failure();
    }
    error.clone()
}

fn gmail_state_dir() -> PathBuf {
    bob_env::bob_cli_state_dir().join("gmail")
}

fn gate_marker_present() -> bool {
    gmail_state_dir().join("awaiting_sign_in").is_file()
}

fn note_gate_failure() {
    let directory = gmail_state_dir();
    if fs::create_dir_all(&directory).is_err() {
        return;
    }
    let marker = directory.join("awaiting_sign_in");
    if marker.is_file() {
        return;
    }
    let _ = fs::write(&marker, "awaiting_sign_in\n");
}

fn clear_gate_failure() {
    let _ = fs::remove_file(gmail_state_dir().join("awaiting_sign_in"));
}

/// Notes in scope: the named notes plus, with `-a`, the vault walk.
fn collect_scope_notes(
    vault: &Path,
    folder: &Path,
    args: &LinkArgs,
) -> Result<Vec<PathBuf>, GmailError> {
    let mut notes = BTreeSet::new();
    for input in &args.notes {
        notes.insert(resolve_named_note(vault, input)?);
    }
    if args.all {
        for note in scan_vault_notes(vault, folder)? {
            notes.insert(note);
        }
    }
    Ok(notes.into_iter().collect())
}

/// A named note must be an in-vault, non-symlink Markdown file.
fn resolve_named_note(
    vault: &Path,
    input: &str,
) -> Result<PathBuf, GmailError> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Err(GmailError::runtime(
            "note",
            "a link note must not be empty".to_string(),
        ));
    }
    let joined = vault.join(trimmed.replace('\\', "/"));
    let canonical_vault = vault.canonicalize().map_err(|_| {
        GmailError::runtime("note", "the vault path is unavailable".to_string())
    })?;
    let canonical = joined.canonicalize().map_err(|_| {
        GmailError::runtime(
            "note",
            format!("link note does not exist: {trimmed}"),
        )
    })?;
    if !canonical.starts_with(&canonical_vault) {
        return Err(GmailError::runtime(
            "note",
            format!("link note is outside the vault: {trimmed}"),
        ));
    }
    let metadata = fs::symlink_metadata(&canonical).map_err(|_| {
        GmailError::runtime(
            "note",
            format!("link note does not exist: {trimmed}"),
        )
    })?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(GmailError::runtime(
            "note",
            format!("link note is not a regular file: {trimmed}"),
        ));
    }
    if !canonical
        .extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("md"))
    {
        return Err(GmailError::runtime(
            "note",
            format!("link note is not a Markdown file: {trimmed}"),
        ));
    }
    Ok(canonical)
}

/// Every Markdown note in the vault except the mail folder, generated and
/// state directories, the done archive, and zorg-era generated logs.
fn scan_vault_notes(
    vault: &Path,
    folder: &Path,
) -> Result<Vec<PathBuf>, GmailError> {
    let canonical_vault = vault.canonicalize().map_err(|_| {
        GmailError::runtime("note", "the vault path is unavailable".to_string())
    })?;
    let mut notes = Vec::new();
    collect_vault_notes(
        &canonical_vault,
        &canonical_vault,
        folder,
        &mut notes,
    )?;
    notes.sort();
    Ok(notes)
}

fn collect_vault_notes(
    root: &Path,
    directory: &Path,
    folder: &Path,
    notes: &mut Vec<PathBuf>,
) -> Result<(), GmailError> {
    let entries = fs::read_dir(directory).map_err(|error| {
        GmailError::runtime(
            "note",
            format!("could not scan the vault: {error}"),
        )
    })?;
    for entry in entries {
        let entry = entry.map_err(|error| {
            GmailError::runtime(
                "note",
                format!("could not scan the vault: {error}"),
            )
        })?;
        let kind = entry.file_type().map_err(|error| {
            GmailError::runtime(
                "note",
                format!("could not inspect a vault path: {error}"),
            )
        })?;
        if kind.is_symlink() {
            continue;
        }
        let path = entry.path();
        if kind.is_dir() {
            let name = entry.file_name();
            if name.to_string_lossy().starts_with('.')
                || is_always_excluded_note_directory_name(&name)
                || ALWAYS_SKIPPED_DIRS
                    .iter()
                    .any(|skipped| OsStr::new(skipped) == name.as_os_str())
            {
                continue;
            }
            let Ok(relative) = path.strip_prefix(root) else {
                continue;
            };
            if path_in_folder(relative, folder) {
                continue;
            }
            collect_vault_notes(root, &path, folder, notes)?;
        } else if kind.is_file()
            && path
                .extension()
                .is_some_and(|extension| extension.eq_ignore_ascii_case("md"))
        {
            let Ok(relative) = path.strip_prefix(root) else {
                continue;
            };
            if path_in_folder(relative, folder) {
                continue;
            }
            let Ok(text) = fs::read_to_string(&path) else {
                continue;
            };
            if is_zorg_generated(&text) {
                continue;
            }
            notes.push(path);
        }
    }
    Ok(())
}

/// Whether a vault-relative path lives under the mail folder.
fn path_in_folder(relative: &Path, folder: &Path) -> bool {
    if folder.as_os_str().is_empty() {
        return false;
    }
    let mut folder_parts = folder.components();
    let mut relative_parts = relative.components();
    loop {
        match (folder_parts.next(), relative_parts.next()) {
            (Some(expected), Some(actual)) => {
                if expected != actual {
                    return false;
                }
            }
            (None, _) => return true,
            (Some(_), None) => return false,
        }
    }
}

/// A zorg-era generated log carries this frontmatter flag and is never
/// rewritten by a backfill.
fn is_zorg_generated(text: &str) -> bool {
    let mut lines = text.lines();
    if lines.next().is_some_and(|first| first.trim() != "---") {
        return false;
    }
    for line in lines {
        let trimmed = line.trim();
        if trimmed == "---" || trimmed == "..." {
            break;
        }
        if let Some((key, value)) = trimmed.split_once(':')
            && key.trim() == "generated_from_zorg"
            && value.trim().eq_ignore_ascii_case("true")
        {
            return true;
        }
    }
    false
}

/// Digests remembered as dead by every host's journal.
fn dead_digests(vault: &Path) -> BTreeSet<String> {
    let Ok((events, _)) = pref_journal::read_raw(vault, save::GMAIL_JOURNAL)
    else {
        return BTreeSet::new();
    };
    events
        .into_iter()
        .filter(|event| event.kind == LINK_FAILED_KIND)
        .filter_map(|event| {
            let payload = event.value.get("value")?;
            let reason = payload.get("reason")?.as_str()?;
            if !DEAD_REASONS.contains(&reason) {
                return None;
            }
            Some(payload.get("url_digest")?.as_str()?.to_string())
        })
        .collect()
}

fn append_link_failed(
    vault: &Path,
    host: &str,
    digest: &str,
    reason: &str,
) -> std::io::Result<()> {
    let timestamp = pref_journal::event_timestamp()
        .to_rfc3339_opts(chrono::SecondsFormat::Secs, false);
    let mut line = serde_json::to_string(&json!({
        "v": 1,
        "ts": timestamp,
        "host": host,
        "kind": LINK_FAILED_KIND,
        "value": {
            "url_digest": digest,
            "reason": reason,
        },
    }))
    .unwrap_or_else(|_| "{}".to_string());
    line.push('\n');
    pref_journal::append_line(vault, save::GMAIL_JOURNAL, host, &line)
}

fn first_note(
    vault: &Path,
    found: &[FoundUrl],
    position: usize,
) -> Option<String> {
    found
        .get(position)
        .map(|found_url| relative_slashes(vault, &found_url.note))
}

fn relative_slashes(vault: &Path, absolute: &Path) -> String {
    save::path_to_slashes(absolute.strip_prefix(vault).unwrap_or(absolute))
}

/// A bare or angle URL that sits in an inline-link destination (`](...)`)
/// cannot be swapped for a wikilink without breaking the link syntax, so it
/// is left as typed. Markdown-link wrappers carry their own range and are
/// replaced whole, keeping the user's label.
pub(super) fn inline_link_destination(
    text: &str,
    found_url: &ids::GmailUrlMatch,
) -> bool {
    if !matches!(found_url.wrapper, GmailWrapper::Bare | GmailWrapper::Angle) {
        return false;
    }
    text[..found_url.start].ends_with("](")
}

/// One candidate swap with its scan-time identity.
struct SwapCandidate {
    start: usize,
    end: usize,
    url: String,
    wrapper: GmailWrapper,
    digest: String,
    key: (String, String),
}

/// Wikilink targets and full links for every planned thread.
fn link_targets(
    vault: &Path,
    plans: &[save::StagedSaveNote],
) -> BTreeMap<(String, String), (String, String)> {
    let mut targets = BTreeMap::new();
    for plan in plans {
        let relative = plan.path.strip_prefix(vault).unwrap_or(&plan.path);
        let without_extension = relative.with_extension("");
        let target = save::path_to_slashes(&without_extension);
        targets.insert(
            (plan.account.clone(), plan.thread_id.clone()),
            (target, plan.link.clone()),
        );
    }
    targets
}

/// Group scan hits by source note into verified candidates. Every hit for
/// one URL shares the resolution, but each token gets its own swap so two
/// identical URLs each become links.
fn build_candidates(
    found: &[FoundUrl],
    url_keys: &BTreeMap<String, (String, String)>,
) -> BTreeMap<PathBuf, Vec<SwapCandidate>> {
    let mut grouped: BTreeMap<PathBuf, Vec<SwapCandidate>> = BTreeMap::new();
    for found_url in found {
        let Some(key) = url_keys.get(&found_url.url) else {
            continue;
        };
        grouped.entry(found_url.note.clone()).or_default().push(
            SwapCandidate {
                start: found_url.start,
                end: found_url.end,
                url: found_url.url.clone(),
                wrapper: found_url.wrapper.clone(),
                digest: save::sha256(found_url.url.as_bytes()),
                key: key.clone(),
            },
        );
    }
    for candidates in grouped.values_mut() {
        candidates.sort_by_key(|candidate| candidate.start);
    }
    grouped
}

/// Read each source note fresh, verify every token range still holds the
/// same text, and stage the rewrites. Unverifiable ranges are dropped for a
/// `saved; link needs review` report; an unscoped replace never happens.
#[allow(clippy::too_many_lines)]
fn plan_source_swaps(
    vault: &Path,
    grouped: &BTreeMap<PathBuf, Vec<SwapCandidate>>,
    targets: &BTreeMap<(String, String), (String, String)>,
    with_diffs: bool,
) -> Result<
    (
        Vec<StagedTextFile>,
        Vec<AppliedSwap>,
        Vec<DroppedSwap>,
        Vec<(String, String)>,
    ),
    GmailError,
> {
    let mut staged = Vec::new();
    let mut applied = Vec::new();
    let mut dropped = Vec::new();
    let mut diffs = Vec::new();
    for (note, candidates) in grouped {
        let original = fs::read_to_string(note).map_err(|error| {
            GmailError::runtime(
                "note",
                format!(
                    "could not read {}: {error}",
                    relative_slashes(vault, note)
                ),
            )
        })?;
        let mut updated = String::with_capacity(original.len() + 256);
        let mut cursor = 0;
        let mut note_applied = Vec::new();
        let mut note_dropped = Vec::new();
        for candidate in candidates {
            let Some((target, link)) = targets.get(&candidate.key) else {
                continue;
            };
            let Some(replacement) =
                verify_token(&original, candidate, target, link)
            else {
                note_dropped.push(DroppedSwap {
                    note: relative_slashes(vault, note),
                    digest: candidate.digest.clone(),
                    link: link.clone(),
                });
                continue;
            };
            if candidate.start < cursor
                || original.get(cursor..candidate.start).is_none()
                || original.get(candidate.start..candidate.end).is_none()
            {
                note_dropped.push(DroppedSwap {
                    note: relative_slashes(vault, note),
                    digest: candidate.digest.clone(),
                    link: link.clone(),
                });
                continue;
            }
            updated.push_str(&original[cursor..candidate.start]);
            updated.push_str(&replacement);
            cursor = candidate.end;
            note_applied.push(AppliedSwap {
                note: relative_slashes(vault, note),
                digest: candidate.digest.clone(),
                link: replacement,
            });
        }
        if note_applied.is_empty() {
            dropped.extend(note_dropped);
            continue;
        }
        updated.push_str(original.get(cursor..).unwrap_or_default());
        if updated == original {
            dropped.extend(note_dropped);
            continue;
        }
        if with_diffs {
            let relative = relative_slashes(vault, note);
            diffs.push((
                relative.clone(),
                unified_diff(&relative, &original, &updated),
            ));
        }
        let existed = fs::symlink_metadata(note)
            .map(|metadata| {
                metadata.is_file() && !metadata.file_type().is_symlink()
            })
            .unwrap_or(false);
        staged.push(StagedTextFile {
            target: note.clone(),
            target_existed: existed,
            original_target: original,
            updated_target: updated,
        });
        applied.extend(note_applied);
        dropped.extend(note_dropped);
    }
    Ok((staged, applied, dropped, diffs))
}

/// The exact replacement for one token, or `None` when the range no longer
/// holds the same text. A user-written `[their words](gmail-url)` keeps its
/// label; anything else becomes the saved labelled link.
fn verify_token(
    original: &str,
    candidate: &SwapCandidate,
    target: &str,
    link: &str,
) -> Option<String> {
    let slice = original.get(candidate.start..candidate.end)?;
    match &candidate.wrapper {
        GmailWrapper::Bare => {
            if slice != candidate.url {
                return None;
            }
            if original[..candidate.start].ends_with("](") {
                return None;
            }
            Some(link.to_string())
        }
        GmailWrapper::Angle => {
            if slice != format!("<{}>", candidate.url) {
                return None;
            }
            if original[..candidate.start].ends_with("](") {
                return None;
            }
            Some(link.to_string())
        }
        GmailWrapper::MarkdownLink { .. } => {
            let rest = slice.strip_prefix('[')?;
            let close = rest.find("](")?;
            let label = &rest[..close];
            let after = &rest[close + 2..];
            let url = after.strip_suffix(')')?;
            if url.trim_end() != candidate.url {
                return None;
            }
            Some(format!("[[{target}|{label}]]"))
        }
    }
}

/// A small unified diff for dry-run previews: one hunk over the changed
/// region with three lines of context on each side.
fn unified_diff(note: &str, original: &str, updated: &str) -> String {
    const CONTEXT: usize = 3;
    let old_lines: Vec<&str> = original.lines().collect();
    let new_lines: Vec<&str> = updated.lines().collect();
    let mut prefix = 0;
    while prefix < old_lines.len()
        && prefix < new_lines.len()
        && old_lines[prefix] == new_lines[prefix]
    {
        prefix += 1;
    }
    let mut suffix = 0;
    while suffix < old_lines.len() - prefix
        && suffix < new_lines.len() - prefix
        && old_lines[old_lines.len() - 1 - suffix]
            == new_lines[new_lines.len() - 1 - suffix]
    {
        suffix += 1;
    }
    let old_start = prefix.saturating_sub(CONTEXT);
    let new_start = prefix.saturating_sub(CONTEXT);
    let old_end = (old_lines.len() - suffix).min(old_lines.len());
    let new_end = (new_lines.len() - suffix).min(new_lines.len());
    let old_count = old_end.saturating_sub(old_start);
    let new_count = new_end.saturating_sub(new_start);
    let mut diff = format!(
        "--- {note}\n+++ {note}\n@@ -{},{} +{},{} @@\n",
        old_start + 1,
        old_count,
        new_start + 1,
        new_count
    );
    for line in &old_lines[old_start..prefix] {
        diff.push(' ');
        diff.push_str(line);
        diff.push('\n');
    }
    for line in &old_lines[prefix..old_lines.len() - suffix] {
        diff.push('-');
        diff.push_str(line);
        diff.push('\n');
    }
    for line in &new_lines[prefix..new_lines.len() - suffix] {
        diff.push('+');
        diff.push_str(line);
        diff.push('\n');
    }
    for line in &new_lines[new_lines.len() - suffix..new_end] {
        diff.push(' ');
        diff.push_str(line);
        diff.push('\n');
    }
    diff
}

/// Mail notes plus source rewrites under one lock hold, with one retry.
/// When contention persists, the mail notes are persisted alone so the next
/// sweep reuses them, and every pending swap is reported as needs-review.
#[allow(clippy::too_many_arguments)]
#[allow(clippy::too_many_lines)]
fn commit_link(
    vault: &Path,
    host: &str,
    folder: &Path,
    link_template: &str,
    works: &mut BTreeMap<(String, String), PreparedSave>,
    found: &[FoundUrl],
    url_keys: &BTreeMap<String, (String, String)>,
    saved_at: DateTime<Utc>,
    timezone: chrono_tz::Tz,
    outcome: &mut LinkOutcome,
) -> Result<(), GmailError> {
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
    let child_env = ob::child_env();
    let is_worktree = ob::detect_git_worktree(vault, &child_env)
        .map_err(|error| {
            GmailError::runtime(
                "git",
                format!(
                    "could not determine whether the vault is a Git worktree: {error}"
                ),
            )
        })?;
    let grouped = build_candidates(found, url_keys);
    let mut completed = None;
    for attempt in 0..2 {
        save::validate_mail_folder(vault, folder)?;
        let index = resolve::scan_index(vault, folder, link_template)?;
        let plans = save::plan_prepared_saves(
            vault,
            folder,
            link_template,
            works,
            &index,
            None,
            false,
            saved_at,
            timezone,
        )?;
        let auxiliary =
            save::plan_auxiliary_files(vault, folder, is_worktree, &child_env)?;
        let targets = link_targets(vault, &plans);
        let (mut source_staged, applied, dropped, _) =
            plan_source_swaps(vault, &grouped, &targets, false)?;
        let mut staged: Vec<StagedTextFile> = plans
            .iter()
            .filter(|plan| plan.changed)
            .map(save::staged_note)
            .collect::<Result<_, _>>()?;
        staged.extend(auxiliary.staged.iter().map(save::clone_staged));
        let mut source_relatives = Vec::new();
        for file in &source_staged {
            let relative = file.target.strip_prefix(vault).map_err(|_| {
                GmailError::runtime(
                    "refused",
                    "a rewritten note escaped the vault".to_string(),
                )
            })?;
            source_relatives.push(relative.to_path_buf());
        }
        staged.append(&mut source_staged);
        save::validate_write_targets(vault, &plans, works, &staged)?;

        let mut commit_paths =
            mail_commit_paths(vault, &child_env, is_worktree, &plans, works)?;
        let mut created_images = Vec::new();
        let mut install_error = None;
        for plan in &plans {
            let relative = plan.path.strip_prefix(vault).map_err(|_| {
                GmailError::runtime(
                    "refused",
                    "a planned note escaped the vault".to_string(),
                )
            })?;
            if let Err(error) = save::ensure_parent_dirs(vault, relative) {
                install_error = Some(error);
                break;
            }
        }
        if install_error.is_none() {
            for plan in works.values_mut() {
                match save::install_imports(vault, &plan.prepared.imports) {
                    Ok(created) => created_images.extend(created),
                    Err(error) => {
                        install_error = Some(error);
                        break;
                    }
                }
            }
        }
        if let Some(error) = install_error {
            save::cleanup_created_images(vault, &created_images);
            if error.kind() == "note_changed" && attempt == 0 {
                continue;
            }
            return Err(error);
        }
        commit_paths.extend(
            created_images.iter().map(|(relative, _)| relative.clone()),
        );
        commit_paths.extend(source_relatives);
        commit_paths.extend(auxiliary.commit_paths.iter().cloned());
        commit_paths.sort();
        commit_paths.dedup();

        match capture::write_staged_files(&staged) {
            Ok(()) => {
                completed = Some((plans, commit_paths, applied, dropped));
                break;
            }
            Err(error)
                if error.typed == Some(CaptureFailureKind::NoteChanged)
                    && attempt == 0 =>
            {
                save::cleanup_created_images(vault, &created_images);
                continue;
            }
            Err(error) => {
                save::cleanup_created_images(vault, &created_images);
                return Err(GmailError::runtime(
                    "write",
                    format!(
                        "could not safely write the link plan: {}",
                        error.message
                    ),
                ));
            }
        }
    }
    let (plans, commit_paths, applied, dropped) = match completed {
        Some(done) => done,
        None => {
            return persist_mail_only(
                vault,
                host,
                folder,
                link_template,
                works,
                saved_at,
                timezone,
                is_worktree,
                &child_env,
                &grouped,
                outcome,
            );
        }
    };

    commit_and_journal(
        vault,
        host,
        &child_env,
        is_worktree,
        works,
        &plans,
        &commit_paths,
        outcome,
    )?;
    outcome.swaps = applied;
    outcome.needs_review = dropped;
    Ok(())
}

/// Mirror of save's mail commit-path scope: changed notes, dirty retries,
/// and matching image imports.
fn mail_commit_paths(
    vault: &Path,
    child_env: &ob::ChildEnv,
    is_worktree: bool,
    plans: &[save::StagedSaveNote],
    works: &BTreeMap<(String, String), PreparedSave>,
) -> Result<Vec<PathBuf>, GmailError> {
    let mut commit_paths = Vec::new();
    for plan in plans {
        let relative = plan.path.strip_prefix(vault).map_err(|_| {
            GmailError::runtime(
                "refused",
                "a planned note escaped the vault".to_string(),
            )
        })?;
        let retry_path_is_dirty = is_worktree
            && save::git_path_is_dirty(vault, child_env, relative).map_err(
                |error| {
                    GmailError::runtime(
                        "git",
                        format!("could not inspect a saved mail path: {error}"),
                    )
                },
            )?;
        if plan.changed || retry_path_is_dirty {
            commit_paths.push(relative.to_path_buf());
        }
    }
    for plan in works.values() {
        for import in &plan.prepared.imports {
            let relative = save::safe_relative_path(&import.relative_path)?;
            let target = vault.join(&relative);
            let matches_import = fs::read(&target).ok().is_some_and(|bytes| {
                save::sha256(&bytes) == save::sha256(&import.bytes)
            });
            let retry_path_is_dirty = is_worktree
                && matches_import
                && save::git_path_is_dirty(vault, child_env, &relative)
                    .map_err(|error| {
                        GmailError::runtime(
                            "git",
                            format!(
                                "could not inspect an imported image path: {error}"
                            ),
                        )
                    })?;
            if retry_path_is_dirty {
                commit_paths.push(relative);
            }
        }
    }
    Ok(commit_paths)
}

/// Contention fallback: persist the mail notes alone, journal them, and
/// report every pending swap as `saved; link needs review`.
#[allow(clippy::too_many_arguments)]
#[allow(clippy::too_many_lines)]
fn persist_mail_only(
    vault: &Path,
    host: &str,
    folder: &Path,
    link_template: &str,
    works: &mut BTreeMap<(String, String), PreparedSave>,
    saved_at: DateTime<Utc>,
    timezone: chrono_tz::Tz,
    is_worktree: bool,
    child_env: &ob::ChildEnv,
    grouped: &BTreeMap<PathBuf, Vec<SwapCandidate>>,
    outcome: &mut LinkOutcome,
) -> Result<(), GmailError> {
    save::validate_mail_folder(vault, folder)?;
    let index = resolve::scan_index(vault, folder, link_template)?;
    let plans = save::plan_prepared_saves(
        vault,
        folder,
        link_template,
        works,
        &index,
        None,
        false,
        saved_at,
        timezone,
    )?;
    let auxiliary =
        save::plan_auxiliary_files(vault, folder, is_worktree, child_env)?;
    let mut staged: Vec<StagedTextFile> = plans
        .iter()
        .filter(|plan| plan.changed)
        .map(save::staged_note)
        .collect::<Result<_, _>>()?;
    staged.extend(auxiliary.staged.iter().map(save::clone_staged));
    save::validate_write_targets(vault, &plans, works, &staged)?;
    let mut commit_paths =
        mail_commit_paths(vault, child_env, is_worktree, &plans, works)?;
    for plan in &plans {
        let relative = plan.path.strip_prefix(vault).map_err(|_| {
            GmailError::runtime(
                "refused",
                "a planned note escaped the vault".to_string(),
            )
        })?;
        save::ensure_parent_dirs(vault, relative)?;
    }
    let mut created_images = Vec::new();
    for plan in works.values_mut() {
        match save::install_imports(vault, &plan.prepared.imports) {
            Ok(created) => created_images.extend(created),
            Err(error) => {
                save::cleanup_created_images(vault, &created_images);
                return Err(error);
            }
        }
    }
    commit_paths
        .extend(created_images.iter().map(|(relative, _)| relative.clone()));
    commit_paths.extend(auxiliary.commit_paths.iter().cloned());
    commit_paths.sort();
    commit_paths.dedup();
    if let Err(error) = capture::write_staged_files(&staged) {
        save::cleanup_created_images(vault, &created_images);
        return Err(GmailError::runtime(
            "note_changed",
            format!(
                "the vault kept changing while link retried: {}",
                error.message
            ),
        ));
    }
    commit_and_journal(
        vault,
        host,
        child_env,
        is_worktree,
        works,
        &plans,
        &commit_paths,
        outcome,
    )?;
    let targets = link_targets(vault, &plans);
    for (note, candidates) in grouped {
        for candidate in candidates {
            if let Some((_, link)) = targets.get(&candidate.key) {
                outcome.needs_review.push(DroppedSwap {
                    note: relative_slashes(vault, note),
                    digest: candidate.digest.clone(),
                    link: link.clone(),
                });
            }
        }
    }
    Ok(())
}

/// One scoped commit of mail notes and rewrites, then save and failure
/// journals. Journal failures are warnings: the notes stay valid.
fn commit_and_journal(
    vault: &Path,
    host: &str,
    child_env: &ob::ChildEnv,
    is_worktree: bool,
    works: &BTreeMap<(String, String), PreparedSave>,
    plans: &[save::StagedSaveNote],
    commit_paths: &[PathBuf],
    outcome: &mut LinkOutcome,
) -> Result<(), GmailError> {
    let changed: Vec<_> = plans.iter().filter(|plan| plan.changed).collect();
    // Read the pre-commit baselines before the scoped commit below: the save
    // journal facts (created/added counts and newly referenced origins) must
    // describe what the commit changed, exactly like `bob gmail save`.
    let event_facts =
        save::save_event_facts(vault, child_env, is_worktree, plans, works)?;
    if is_worktree && !commit_paths.is_empty() {
        let subject = changed
            .first()
            .map(|plan| plan.title.as_str())
            .or_else(|| plans.first().map(|plan| plan.title.as_str()))
            .unwrap_or("mail");
        let suffix = if changed.len() > 1 {
            format!(" (+{} threads)", changed.len() - 1)
        } else {
            String::new()
        };
        ob::commit_paths(
            vault,
            child_env,
            &format!("gmail: link {}{}", save::commit_subject(subject), suffix),
            commit_paths,
        )
        .map_err(|error| {
            GmailError::runtime(
                "uncommitted",
                format!("link files were written but could not be committed: {error}"),
            )
            .with_hint("run `bob gmail link` again to finish the scoped commit")
        })?;
    }
    fill_saved_entries(vault, plans, false, outcome);
    for plan in changed {
        let key = (plan.account.clone(), plan.thread_id.clone());
        if let Some(facts) = event_facts.get(&key)
            && let Err(error) =
                save::append_save_event(vault, host, plan, facts)
        {
            outcome
                .warnings
                .push(format!("could not append Gmail save journal: {error}"));
        }
    }
    let mut origins = Vec::new();
    for plan in plans.iter().filter(|plan| plan.changed) {
        let key = (plan.account.clone(), plan.thread_id.clone());
        if let Some(facts) = event_facts.get(&key) {
            origins.extend(facts.origins.iter().cloned());
        }
    }
    for failure in
        attachment_journal::append_origin_events(vault, &origins, "gmail")
    {
        outcome.warnings.push(format!(
            "could not record imported image {}: {}",
            failure.saved, failure.error
        ));
    }
    let mut journaled = BTreeSet::new();
    for failure in outcome.failures.iter().filter(|item| item.dead) {
        let (Some(digest), kind) =
            (failure.digest.as_ref(), failure.kind.as_str())
        else {
            continue;
        };
        if !journaled.insert((digest.clone(), kind.to_string())) {
            continue;
        }
        if let Err(error) =
            append_link_failed(vault, host, digest, failure.kind.as_str())
        {
            outcome
                .warnings
                .push(format!("could not append Gmail link journal: {error}"));
        }
    }
    Ok(())
}

fn fill_saved_entries(
    vault: &Path,
    plans: &[save::StagedSaveNote],
    dry_run: bool,
    outcome: &mut LinkOutcome,
) {
    for plan in plans {
        let relative = relative_slashes(vault, &plan.path);
        let status = if dry_run {
            match plan.status {
                "created" => "would_create",
                "refreshed" => "would_refresh",
                _ => "would_unchanged",
            }
        } else {
            plan.status
        };
        if dry_run && plan.created {
            outcome
                .would_create
                .push((relative.clone(), plan.message_count));
        }
        outcome.saved.push(SavedEntry {
            account: plan.account.clone(),
            thread_id: plan.thread_id.clone(),
            note: relative,
            link: plan.link.clone(),
            status,
            message_count: plan.message_count,
            added_count: plan.added_count,
        });
    }
}

fn scope_description(args: &LinkArgs) -> &'static str {
    if args.all && args.notes.is_empty() {
        "the vault"
    } else if args.all {
        "these notes and the vault"
    } else {
        "these notes"
    }
}

fn print_report(args: &LinkArgs, outcome: &LinkOutcome) {
    if args.format.is_json() {
        print_json_report(outcome);
        return;
    }
    let styler = Styler::detect();
    if outcome.dry_run {
        for (_, diff) in &outcome.diffs {
            print!("{diff}");
        }
        for entry in &outcome.saved {
            let line = match entry.status {
                "would_create" => format!(
                    "would create {} ({} messages)",
                    entry.note, entry.message_count
                ),
                "would_refresh" => format!(
                    "would refresh {} (+{} messages)",
                    entry.note, entry.added_count
                ),
                _ => format!("would leave {} unchanged", entry.note),
            };
            println!("{line}");
        }
        for swap in &outcome.swaps {
            println!("would link {} in {}", swap.link, swap.note);
        }
    } else if !args.quiet {
        if outcome.found_urls == 0 {
            println!("No Gmail URLs in {}.", scope_description(args));
        }
        for swap in &outcome.swaps {
            println!("✉️ {} → {}", swap.note, swap.link);
        }
    }
    for review in &outcome.needs_review {
        eprintln!(
            "! {}: saved {}; link needs review",
            review.note, review.link
        );
    }
    for failure in &outcome.failures {
        let context = match (&failure.note, &failure.digest) {
            (Some(note), Some(digest)) => {
                format!("{} in {note}", short_digest(digest))
            }
            (Some(note), None) => note.clone(),
            (None, Some(digest)) => short_digest(digest),
            (None, None) => "link".to_string(),
        };
        if failure.dead {
            eprintln!(
                "link {context}: {} (remembered; skipped next time)",
                failure.message
            );
        } else {
            eprintln!("link {context}: {}", failure.message);
        }
    }
    for warning in &outcome.warnings {
        eprintln!("{}", styler.dim(warning));
    }
    ui::flush_stdout();
}

fn short_digest(digest: &str) -> String {
    digest.chars().take(12).collect()
}

fn print_json_report(outcome: &LinkOutcome) {
    let saved: Vec<_> = outcome
        .saved
        .iter()
        .map(|entry| {
            json!({
                "thread_id": entry.thread_id,
                "account": entry.account,
                "note": entry.note,
                "link": entry.link,
                "status": entry.status,
                "message_count": entry.message_count,
                "added_count": entry.added_count,
            })
        })
        .collect();
    let swaps: Vec<_> = outcome
        .swaps
        .iter()
        .map(|swap| {
            json!({
                "note": swap.note,
                "url_digest": swap.digest,
                "link": swap.link,
                "status": if outcome.dry_run {
                    "would_link"
                } else {
                    "linked"
                },
            })
        })
        .collect();
    let needs_review: Vec<_> = outcome
        .needs_review
        .iter()
        .map(|review| {
            json!({
                "note": review.note,
                "url_digest": review.digest,
                "link": review.link,
            })
        })
        .collect();
    let failures: Vec<_> = outcome
        .failures
        .iter()
        .map(|failure| {
            json!({
                "url_digest": failure.digest,
                "note": failure.note,
                "kind": failure.kind,
                "message": failure.message,
                "remembered": failure.dead,
            })
        })
        .collect();
    let mut document = json!({
        "schema_version": 1,
        "ok": true,
        "dry_run": outcome.dry_run,
        "scanned": outcome.scanned_notes,
        "urls": outcome.found_urls,
        "skipped_dead": outcome.skipped_dead,
        "saved": saved,
        "swaps": swaps,
        "needs_review": needs_review,
        "failures": failures,
        "warnings": outcome.warnings,
    });
    if outcome.dry_run {
        document["diffs"] = outcome
            .diffs
            .iter()
            .map(|(note, diff)| json!({"note": note, "diff": diff}))
            .collect::<Vec<_>>()
            .into();
    }
    println!("{document}");
    ui::flush_stdout();
}

/// The `list` footer owned by this phase: waiting links still resolve
/// offline and are not remembered as dead; gone links are remembered dead.
/// Absent when both are zero, so it never claims zero links.
pub(crate) fn waiting_gone_footer(
    vault: &Path,
    folder: &Path,
) -> Option<String> {
    let notes = scan_vault_notes(vault, folder).ok()?;
    let dead = dead_digests(vault);
    let mut waiting = 0usize;
    let mut gone = 0usize;
    for note in notes {
        let Ok(text) = fs::read_to_string(&note) else {
            continue;
        };
        for found_url in ids::find_gmail_urls(&text) {
            if inline_link_destination(&text, &found_url) {
                continue;
            }
            let digest = save::sha256(found_url.url.as_bytes());
            if dead.contains(&digest) {
                gone += 1;
            } else if matches!(found_url.verdict, GmailVerdict::Resolvable(_)) {
                waiting += 1;
            }
        }
    }
    if waiting + gone == 0 {
        return None;
    }
    Some(format_footer(waiting, gone))
}

fn format_footer(waiting: usize, gone: usize) -> String {
    format!("{waiting} Gmail links waiting · {gone} gone")
}

#[cfg(test)]
mod tests {
    use super::*;

    const URL: &str = "https://mail.google.com/mail/u/0/#all/0123456789abcdef";

    fn bare_candidate(start: usize, end: usize) -> SwapCandidate {
        SwapCandidate {
            start,
            end,
            url: URL.to_string(),
            wrapper: GmailWrapper::Bare,
            digest: "digest".to_string(),
            key: ("me@example.com".to_string(), "hex".to_string()),
        }
    }

    #[test]
    fn bare_token_verifies_and_angle_needs_brackets() {
        let text = format!("see {URL} now");
        let candidate = bare_candidate("see ".len(), "see ".len() + URL.len());
        assert_eq!(
            verify_token(&text, &candidate, "mail/x", "[[mail/x|L]]"),
            Some("[[mail/x|L]]".to_string())
        );
        let edited = text.replace("0123456789abcdef", "ffffffffffffffff");
        assert_eq!(
            verify_token(&edited, &candidate, "mail/x", "[[mail/x|L]]"),
            None
        );
    }

    #[test]
    fn markdown_link_keeps_the_user_label() {
        let text = format!("[my words]({URL})");
        let candidate = SwapCandidate {
            start: 0,
            end: text.len(),
            url: URL.to_string(),
            wrapper: GmailWrapper::MarkdownLink {
                label: "my words".to_string(),
            },
            digest: "digest".to_string(),
            key: ("me@example.com".to_string(), "hex".to_string()),
        };
        assert_eq!(
            verify_token(&text, &candidate, "mail/x", "[[mail/x|Saved]]"),
            Some("[[mail/x|my words]]".to_string())
        );
    }

    #[test]
    fn inline_destination_urls_stay_as_typed() {
        // A titled link destination scans as a bare URL; swapping it would
        // leave `[label]([[...]])` behind, so it stays as typed.
        let text = format!("[label]({URL} \"title\")");
        let start = "[label](".len();
        let found = ids::find_gmail_urls(&text);
        assert_eq!(found.len(), 1);
        assert!(matches!(found[0].wrapper, GmailWrapper::Bare));
        assert!(inline_link_destination(&text, &found[0]));
        assert_eq!(
            verify_token(
                &text,
                &bare_candidate(start, start + URL.len()),
                "mail/x",
                "[[mail/x|L]]"
            ),
            None
        );
        // An untitled `[label](url)` scans whole as a Markdown link and is
        // replaced in full, so it is not an inline destination.
        let plain = format!("[label]({URL})");
        let plain_found = ids::find_gmail_urls(&plain);
        assert_eq!(plain_found.len(), 1);
        assert!(matches!(
            plain_found[0].wrapper,
            GmailWrapper::MarkdownLink { .. }
        ));
        assert!(!inline_link_destination(&plain, &plain_found[0]));
    }

    #[test]
    fn identical_urls_each_become_links() {
        let text = format!("{URL}\n\n{URL}\n");
        let found = ids::find_gmail_urls(&text);
        assert_eq!(found.len(), 2);
        let note = PathBuf::from("/vault/tasks.md");
        let found_urls: Vec<FoundUrl> = found
            .into_iter()
            .map(|item| FoundUrl {
                note: note.clone(),
                start: item.start,
                end: item.end,
                url: item.url,
                wrapper: item.wrapper,
                verdict: item.verdict,
            })
            .collect();
        let mut keys = BTreeMap::new();
        keys.insert(
            URL.to_string(),
            ("me@example.com".to_string(), "hex".to_string()),
        );
        let grouped = build_candidates(&found_urls, &keys);
        assert_eq!(grouped[&note].len(), 2);
    }

    #[test]
    fn unified_diff_shows_one_hunk_with_context() {
        let original = "a\nb\nc\nd\ne\nf\ng\n";
        let updated = "a\nb\nc\nCHANGED\ne\nf\ng\n";
        let diff = unified_diff("tasks.md", original, updated);
        assert!(diff.starts_with("--- tasks.md\n+++ tasks.md\n@@ "));
        assert!(diff.contains("-d\n"));
        assert!(diff.contains("+CHANGED\n"));
        assert!(diff.contains(" a\n"));
    }

    #[test]
    fn zorg_frontmatter_is_detected() {
        assert!(is_zorg_generated(
            "---\ngenerated_from_zorg: true\n---\nbody\n"
        ));
        assert!(!is_zorg_generated(
            "---\ngenerated_from_zorg: false\n---\nbody\n"
        ));
        assert!(!is_zorg_generated("no frontmatter\n"));
        assert!(!is_zorg_generated("---\ntitle: x\n---\nbody\n"));
    }

    #[test]
    fn folder_prefix_matching() {
        assert!(path_in_folder(Path::new("mail/a.md"), Path::new("mail")));
        assert!(path_in_folder(
            Path::new("mail/2026/a.md"),
            Path::new("mail")
        ));
        assert!(!path_in_folder(
            Path::new("mailbox/a.md"),
            Path::new("mail")
        ));
        assert!(!path_in_folder(Path::new("tasks.md"), Path::new("mail")));
        assert!(!path_in_folder(Path::new("tasks.md"), Path::new("")));
    }

    #[test]
    fn scan_skips_excluded_dirs_mail_folder_and_zorg() {
        let temp = tempfile::tempdir().unwrap();
        let vault = temp.path();
        let write = |relative: &str, text: &str| {
            let path = vault.join(relative);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, text).unwrap();
        };
        write("tasks.md", "body\n");
        write("done/old.md", "body\n");
        write("mail/2026/saved.md", "body\n");
        write("log.md", "---\ngenerated_from_zorg: true\n---\nbody\n");
        write(".obsidian/hidden.md", "body\n");
        let notes = scan_vault_notes(vault, Path::new("mail")).unwrap();
        let names: Vec<String> = notes
            .iter()
            .map(|note| relative_slashes(vault, note))
            .collect();
        assert_eq!(names, vec!["tasks.md".to_string()]);
    }

    #[test]
    fn footer_format_counts() {
        assert_eq!(format_footer(2, 1), "2 Gmail links waiting · 1 gone");
        assert_eq!(format_footer(1, 0), "1 Gmail links waiting · 0 gone");
    }

    #[test]
    fn dead_and_gate_kinds_are_disjoint_and_complete() {
        for kind in DEAD_REASONS {
            assert!(!GATE_KINDS.contains(kind));
        }
        for kind in ["gone", "refused", "never_save", "unresolvable"] {
            assert!(DEAD_REASONS.contains(&kind));
        }
    }
}
