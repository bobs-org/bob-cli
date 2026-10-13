//! Typed, non-printing URL ingest extracted from `bob ref create`.
//!
//! The background paths that clip a link — the capture worker and
//! `bob gkeep pull` — call [`ingest_url`]. It composes the same building
//! blocks as create (`resolve_url_syntactic`, `fetch_and_route`, `sources`
//! dedupe, `pdf_target` planning/stamp/install, `ClipAdapterClient`) with
//! create's fixed reading-queue defaults (blogs/papers, status ready,
//! no audio, no force) plus the caller's resolved parent route, holds the
//! machine-wide ingest lock serializing the capture worker and Keep pull,
//! and installs with fsync. `bob ref create` keeps its own printing routes
//! and never calls [`ingest_url`]. Ingest writes nothing to stdout and
//! prints nothing to stderr itself; short status lines go through
//! [`IngestRequest::progress`] only.

use std::{fs, path::Path};

use super::arxiv::ArxivPaper;
use super::clip_adapter::{
    CaptureRequest, ClipAdapterClient, MetadataOverrides,
};
use super::clip_url::{humanize_stem, snake_case, stem_from_url, WebUrl};
use super::companion::cleanup_audio_on_failure;
use super::io::vault_relative_path_value;
use super::listen::{ListenCommand, ListenError, ListenOutput, ListenValues};
use super::model::MarkerValue;
use super::model::{CommandError, Config};
use super::note::current_local_date;
use super::stamp::{
    compose_marker, plan_default_target, stamp_and_install,
    DefaultTargetIdentity, PdfInfo, TargetPlan,
};

/// Fixed reading-queue defaults for every ingest call.
const INGEST_STATUS: &str = "ready";
const INGEST_ARTICLE_REF_TYPE: &str = "blogs";
const INGEST_PDF_REF_TYPE: &str = "papers";

/// One URL ingest request.
pub(crate) struct IngestRequest<'a> {
    /// Effective vault root; never from Clap.
    pub(crate) bob_dir: &'a Path,
    /// [`WebUrl::cleaned`] for the link.
    pub(crate) url: &'a str,
    /// Resolved parent route stamped into the marker (e.g. `mac_inbox`).
    pub(crate) parent: &'a str,
    /// Whether standalone `%` (or the compatible `@` alias) requested
    /// companion audio.
    pub(crate) listen: bool,
    /// Short status lines; never stdout.
    pub(crate) progress: Option<&'a dyn Fn(&str)>,
}

/// Which clip route produced the PDF.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum IngestRoute {
    Article,
    Pdf,
    Arxiv,
}

/// The outcome of [`ingest_url`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum IngestOutcome {
    Created {
        /// Vault-relative intake PDF, e.g. `xlib/blogs/post.pdf`.
        pdf: String,
        ref_type: String,
        title: Option<String>,
        route: IngestRoute,
        /// Vault-relative legacy note superseded by this capture, if any.
        superseded_legacy: Option<String>,
    },
    AlreadyInLibrary {
        /// Vault-relative ref note that already records the link.
        note: String,
    },
    AlreadyQueued {
        /// Vault-relative intake PDF already queued for scan.
        pdf: String,
    },
}

/// Snake-case error kinds for [`IngestError`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum IngestErrorKind {
    Network,
    Timeout,
    HttpStatus,
    Browser,
    Dependency,
    Blocked,
    Thin,
    Render,
    UnsupportedContent,
    Collision,
    InvalidUrl,
    Internal,
    ListenConfig,
    ListenFailed,
    ListenNoAudio,
    ListenInterrupted,
}

impl IngestErrorKind {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            IngestErrorKind::Network => "network",
            IngestErrorKind::Timeout => "timeout",
            IngestErrorKind::HttpStatus => "http_status",
            IngestErrorKind::Browser => "browser",
            IngestErrorKind::Dependency => "dependency",
            IngestErrorKind::Blocked => "blocked",
            IngestErrorKind::Thin => "thin",
            IngestErrorKind::Render => "render",
            IngestErrorKind::UnsupportedContent => "unsupported_content",
            IngestErrorKind::Collision => "collision",
            IngestErrorKind::InvalidUrl => "invalid_url",
            IngestErrorKind::Internal => "internal",
            IngestErrorKind::ListenConfig => "listen_config",
            IngestErrorKind::ListenFailed => "listen_failed",
            IngestErrorKind::ListenNoAudio => "listen_no_audio",
            IngestErrorKind::ListenInterrupted => "listen_interrupted",
        }
    }

    /// Whether the caller should leave the link queued for another try.
    /// Narration failures are retryable: fixing the command and re-pulling
    /// succeeds without narrating twice.
    pub(crate) fn retryable_by_kind(self) -> bool {
        matches!(
            self,
            IngestErrorKind::Network
                | IngestErrorKind::Timeout
                | IngestErrorKind::Browser
                | IngestErrorKind::Dependency
                | IngestErrorKind::ListenConfig
                | IngestErrorKind::ListenFailed
                | IngestErrorKind::ListenNoAudio
                | IngestErrorKind::ListenInterrupted
        )
    }
}

/// A typed ingest failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct IngestError {
    pub(crate) kind: IngestErrorKind,
    pub(crate) message: String,
    pub(crate) hint: Option<String>,
    /// Only `http_status` is conditionally retryable; `None` means the
    /// kind's default from [`IngestErrorKind::retryable_by_kind`].
    retryable_override: Option<bool>,
}

impl IngestError {
    pub(crate) fn retryable(&self) -> bool {
        self.retryable_override
            .unwrap_or_else(|| self.kind.retryable_by_kind())
    }

    /// A synthetic non-retryable failure, for worker recovery paths
    /// that fail without clipping (stale `running/` jobs at 2+
    /// attempts).
    pub(crate) fn internal(message: impl Into<String>) -> Self {
        Self {
            kind: IngestErrorKind::Internal,
            message: message.into(),
            hint: None,
            retryable_override: None,
        }
    }

    /// The shared fallback bullet used by capture and Keep when a clip
    /// fails: exactly `⚠️ Clip failed (<kind>): <message> · retry:
    /// bob ref create <quoted url> -P <parent>[ -L]`. The `-L` suffix is
    /// preserved when the original request asked for companion audio.
    pub(crate) fn fallback_note(
        &self,
        url: &str,
        parent: &str,
        listen: bool,
    ) -> String {
        fallback_note_for_with_listen(
            self.kind.as_str(),
            &self.message,
            url,
            parent,
            listen,
        )
    }
}

/// Render the shared fallback bullet from stored data, so the worker can
/// retry a `stuck/` fallback without the original [`IngestError`]. Identical
/// to [`IngestError::fallback_note`]. The listen-preserving form appends
/// ` -L` when the original request asked for companion audio.
pub(crate) fn fallback_note_for_with_listen(
    kind: &str,
    message: &str,
    url: &str,
    parent: &str,
    listen: bool,
) -> String {
    let collapsed = message
        .lines()
        .next()
        .unwrap_or_default()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    let truncated = if collapsed.chars().count() > 120 {
        format!("{}…", collapsed.chars().take(120).collect::<String>())
    } else {
        collapsed
    };
    let escaped = crate::native::gkeep::render::escape_child_text(&truncated);
    let suffix = if listen { " -L" } else { "" };
    format!(
        "⚠️ Clip failed ({kind}): {escaped} · retry: bob ref create {} -P {parent}{suffix}",
        shell_quote_url(url),
    )
}

/// Map a [`ListenError`] to its deliberate [`IngestError`] without
/// substring matching on unrelated network messages.
fn listen_ingest_error(error: ListenError) -> IngestError {
    let (kind, message, hint) = match &error {
        ListenError::NotConfigured { message, hint }
        | ListenError::Invalid { message, hint } => {
            (IngestErrorKind::ListenConfig, message.clone(), hint.clone())
        }
        ListenError::Failed { message, hint } => {
            (IngestErrorKind::ListenFailed, message.clone(), hint.clone())
        }
        ListenError::NoAudio { message, hint } => (
            IngestErrorKind::ListenNoAudio,
            message.clone(),
            hint.clone(),
        ),
        ListenError::Interrupted { message, hint } => (
            IngestErrorKind::ListenInterrupted,
            message.clone(),
            hint.clone(),
        ),
    };
    IngestError {
        kind,
        message,
        hint: Some(hint),
        retryable_override: Some(true),
    }
}

/// Resolve and validate the configured listen command before any
/// fetch/render, while the ingest lock is held. A missing or invalid
/// command fails fast without touching the network or vault.
fn require_listen_command() -> Result<ListenCommand, IngestError> {
    super::listen::require_validated().map_err(listen_ingest_error)
}

/// Run narration quietly (never stdout) and map its typed failure.
fn run_listen_quiet(
    command: &ListenCommand,
    values: &ListenValues,
) -> Result<std::path::PathBuf, IngestError> {
    super::listen::run_listen_with_output(command, values, ListenOutput::Quiet)
        .map_err(listen_ingest_error)
}

/// Shell-quote a cleaned URL: bare when it matches
/// `^[A-Za-z0-9._~:/%+=-]+$`, else single-quoted with `'\''` for `'`.
fn shell_quote_url(url: &str) -> String {
    let bare = !url.is_empty()
        && url.bytes().all(|byte| {
            byte.is_ascii_alphanumeric()
                || matches!(
                    byte,
                    b'.' | b'_'
                        | b'~'
                        | b':'
                        | b'/'
                        | b'%'
                        | b'+'
                        | b'='
                        | b'-'
                )
        });
    if bare {
        url.to_string()
    } else {
        format!("'{}'", url.replace('\'', "'\\''"))
    }
}

/// Classify an error message into its [`IngestErrorKind`], preserving the
/// adapter's `kind: msg` prefix when present.
fn classify_message(message: &str) -> IngestErrorKind {
    if let Some((kind, _)) = message.split_once(": ") {
        match kind {
            "network" => return IngestErrorKind::Network,
            "timeout" => return IngestErrorKind::Timeout,
            "browser" => return IngestErrorKind::Browser,
            "blocked" => return IngestErrorKind::Blocked,
            "thin" => return IngestErrorKind::Thin,
            "render" => return IngestErrorKind::Render,
            _ => {}
        }
    }
    classify_unprefixed(message)
}

fn classify_unprefixed(message: &str) -> IngestErrorKind {
    if message.contains("invalid URL")
        || message.contains("unsupported URL scheme")
        || message.contains("URLs with userinfo")
        || message.contains("URL has no host")
        || message.contains("is_private")
        || message.contains("private address")
        || message.contains("not a global")
    {
        return IngestErrorKind::InvalidUrl;
    }
    if message.contains("cannot resolve host")
        || message.contains("connection failed")
        || message.contains("TLS failure")
        || message.contains("navigation failed")
        || message.contains("could not download the PDF")
        || message.contains("curl failed (exit")
    {
        return IngestErrorKind::Network;
    }
    if message.contains("timed out") {
        return IngestErrorKind::Timeout;
    }
    if message.contains("curl command not found")
        || message.contains("uv was not found")
        || message.contains("web clip adapter is not installed")
        || message.contains("start the web clip adapter")
        || message.contains("install curl")
    {
        return IngestErrorKind::Dependency;
    }
    if message.contains("server returned HTTP") {
        return IngestErrorKind::HttpStatus;
    }
    if message.contains("unsupported content type")
        || message.contains("server said PDF but sent something else")
        || message.contains("larger than 95 MiB")
        || message.contains("PDF larger than")
        || message.contains("PDF is encrypted")
        || message.contains("PDF has no pages")
        || message.contains("is not an article")
        || message.contains("article extraction")
    {
        return IngestErrorKind::UnsupportedContent;
    }
    if message.contains("refusing to create")
        || message.contains("already carries a Highlights marker")
        || message.contains("already captured")
        || message.contains("already queued")
        || message.contains("sidecar") && message.contains("already exists")
    {
        return IngestErrorKind::Collision;
    }
    if message.contains("the web clip adapter crashed")
        || message.contains("terminated by a signal")
        || message.contains("invalid JSON")
        || message.contains("spoke protocol")
    {
        return IngestErrorKind::Internal;
    }
    IngestErrorKind::Internal
}

/// Only 408, 429, and 5xx HTTP statuses are retryable.
fn http_status_retryable(message: &str) -> bool {
    let Some(code) = message
        .split("HTTP")
        .nth(1)
        .and_then(|tail| {
            tail.split(|c: char| !c.is_ascii_digit())
                .find(|part| !part.is_empty())
        })
        .and_then(|digits| digits.parse::<u16>().ok())
    else {
        return false;
    };
    code == 408 || code == 429 || (500..600).contains(&code)
}

fn ingest_error(message: String, hint: Option<String>) -> IngestError {
    let kind = classify_message(&message);
    let retryable_override = match kind {
        IngestErrorKind::HttpStatus => Some(http_status_retryable(&message)),
        _ => None,
    };
    IngestError {
        kind,
        message,
        hint,
        retryable_override,
    }
}

fn command_error(error: CommandError) -> IngestError {
    let (message, hint) = split_hint(&error.message);
    ingest_error(message, hint)
}

fn split_hint(message: &str) -> (String, Option<String>) {
    match message.split_once("\nhint: ") {
        Some((first, hint)) => (first.to_string(), Some(hint.to_string())),
        None => (message.to_string(), None),
    }
}

/// Recovery hint for a post-listen install failure: rebind the kept
/// episode explicitly.
fn bind_ingest_hint(cleaned_url: &str, scratch_audio: &Path) -> String {
    format!(
        "bind it with bob ref create {} --audio {}",
        shell_quote_url(cleaned_url),
        scratch_audio.display()
    )
}

/// Stamp the PDF into scratch *before* narration, run the listen quietly
/// (never stdout), recheck collisions, install audio then PDF, and clean
/// up only audio created by this attempt if PDF installation fails. Paid
/// audio is retained with the recovery hint on post-listen errors. Shares
/// guarantees with create; never installs a PDF first and then narrates.
#[allow(clippy::too_many_arguments)]
fn stamp_listen_and_install_for_ingest(
    scratch: &mut super::workdir::ScratchDir,
    target_plan: &TargetPlan,
    command: &ListenCommand,
    listen_target: &str,
    listen_pdf: &Path,
    listen_title: &str,
    cleaned_url: &str,
    stamp_source: &Path,
    marker: &str,
    info: &PdfInfo,
    progress: Option<&dyn Fn(&str)>,
) -> Result<usize, IngestError> {
    // Fail fast before the paid render: no audio exists yet.
    let (stamped, page_count) = super::pdf_target::stamp_pdf_to_scratch(
        stamp_source,
        scratch,
        marker,
        info,
    )
    .map_err(command_error)?;
    let flow = super::listen::plan_listen_flow(
        target_plan,
        scratch,
        &target_plan.stem,
    )
    .map_err(command_error)?;
    let values = ListenValues {
        target: listen_target.to_string(),
        pdf: listen_pdf.to_path_buf(),
        audio: flow.scratch_audio.clone(),
        title: listen_title.to_string(),
    };
    run_listen_quiet(command, &values)?;
    // The vault may have changed during a long listen: recheck
    // target/audio collisions after narration.
    if let Err(error) = super::stamp::refuse_target_collisions(
        &target_plan.target,
        &target_plan.sidecar,
        &target_plan.workflow,
        false,
    ) {
        let cmd = command_error(error);
        let hint = bind_ingest_hint(cleaned_url, &flow.scratch_audio);
        let kept = super::listen::post_listen_error_quiet(
            scratch,
            &flow.scratch_audio,
            cmd.message.clone(),
            hint,
            progress,
        );
        let (message, hint) = split_hint(&kept.message);
        return Err(IngestError {
            kind: IngestErrorKind::Collision,
            message,
            hint,
            retryable_override: Some(false),
        });
    }
    let created =
        super::listen::install_listen_audio(&flow, false).map_err(|error| {
            let cmd = command_error(error);
            let hint = bind_ingest_hint(cleaned_url, &flow.scratch_audio);
            let kept = super::listen::post_listen_error_quiet(
                scratch,
                &flow.scratch_audio,
                cmd.message.clone(),
                hint,
                progress,
            );
            let (message, hint) = split_hint(&kept.message);
            // Audio install collisions are non-retryable without --force,
            // like the non-listen collision path.
            IngestError {
                kind: IngestErrorKind::Collision,
                message,
                hint,
                retryable_override: Some(false),
            }
        })?;
    match super::pdf_target::install_stamped_pdf(
        &stamped,
        &target_plan.target,
        None,
        page_count,
    ) {
        Ok(_) => Ok(page_count),
        Err(error) => {
            cleanup_audio_on_failure(created.as_ref());
            let cmd = command_error(error);
            let hint = bind_ingest_hint(cleaned_url, &flow.scratch_audio);
            let kept = super::listen::post_listen_error_quiet(
                scratch,
                &flow.scratch_audio,
                cmd.message.clone(),
                hint,
                progress,
            );
            let (message, hint) = split_hint(&kept.message);
            Err(IngestError {
                kind: IngestErrorKind::Collision,
                message,
                hint,
                retryable_override: Some(false),
            })
        }
    }
}

/// Machine-wide ingest lock: `bob_cli_state_dir()/ref/ingest.lock` (fs2
/// exclusive, blocking). Reports `waiting for another clip…` once through
/// `progress` while it waits.
fn lock_ingest(
    progress: Option<&dyn Fn(&str)>,
) -> Result<fs::File, IngestError> {
    lock_ingest_at(
        &crate::native::env::bob_cli_state_dir().join("ref"),
        progress,
    )
}

fn lock_ingest_at(
    dir: &Path,
    progress: Option<&dyn Fn(&str)>,
) -> Result<fs::File, IngestError> {
    let path = dir.join("ingest.lock");
    let file = crate::native::env::create_state_lock_file(dir, "ingest.lock")
        .map_err(|error| {
        ingest_error(
            format!("open ingest lock {}: {error}", path.display()),
            None,
        )
    })?;
    {
        use fs2::FileExt;
        if file.try_lock_exclusive().is_err() {
            if let Some(report) = progress {
                report("waiting for another clip…");
            }
            file.lock_exclusive().map_err(|error| {
                ingest_error(
                    format!("take ingest lock {}: {error}", path.display()),
                    None,
                )
            })?;
        }
        Ok(file)
    }
}

/// Clip one URL into the reading queue without printing anything.
///
/// Fixed defaults: route-default ref type, status `ready`, no force, no
/// title or name override. The marker parent is the request's resolved
/// parent route. When `request.listen` is true, companion audio is
/// narrated with the configured listen command using the same cleaned
/// URL, resolved title, unstamped PDF, and scratch audio placeholder
/// meanings as CLI create.
pub(crate) fn ingest_url(
    request: &IngestRequest,
) -> Result<IngestOutcome, IngestError> {
    let config = Config::for_vault(request.bob_dir);
    let (url, arxiv) = super::target::resolve_url_syntactic(request.url)
        .map_err(command_error)?;
    let dedupe_key = match &arxiv {
        Some(paper) => paper.dedupe_key(),
        None => url.dedupe_key.clone(),
    };
    // Pre-lock dedupe so already-known links never take the lock.
    // Existing references stay fast unchanged outcomes even when marked:
    // the shorthand never adds audio to them.
    let recorded = super::sources::collect_recorded_source_urls(&config)
        .map_err(|error| {
            ingest_error(error.message.clone(), error.hint.clone())
        })?;
    if let Some(outcome) =
        outcome_for_refusing_hit(&config, &recorded, &dedupe_key, None)
    {
        return Ok(outcome);
    }
    let legacy = super::sources::legacy_hits(&recorded, &dedupe_key);
    let superseded_legacy = legacy
        .first()
        .map(|hit| vault_relative_path_value(&config, &hit.path));

    let _lock = lock_ingest(request.progress)?;
    // Re-check under the lock: another worker may have queued the link
    // while this call waited.
    let recorded = super::sources::collect_recorded_source_urls(&config)
        .map_err(|error| {
            ingest_error(error.message.clone(), error.hint.clone())
        })?;
    if let Some(outcome) =
        outcome_for_refusing_hit(&config, &recorded, &dedupe_key, None)
    {
        return Ok(outcome);
    }
    // For a new marked import, resolve/validate the listen command
    // before fetch/render, while retaining the ingest lock and its
    // under-lock dedupe recheck.
    let listen_command = if request.listen {
        Some(require_listen_command()?)
    } else {
        None
    };

    if let Some(paper) = arxiv {
        return ingest_arxiv_route(
            &config,
            &paper,
            &url,
            &recorded,
            superseded_legacy,
            request.parent,
            request.progress,
            listen_command.as_ref(),
        );
    }
    let mut scratch =
        super::workdir::ScratchDir::create("ingest").map_err(command_error)?;
    // Ingest stays silent: the TTY `fetching …` line is dropped (None).
    match super::target::fetch_and_route(url.clone(), &scratch, None)
        .map_err(command_error)?
    {
        super::target::CreateSource::PdfUrl { url, downloaded } => {
            ingest_pdf_url_route(
                &config,
                &url,
                &downloaded,
                &mut scratch,
                &recorded,
                superseded_legacy,
                request.parent,
                request.progress,
                listen_command.as_ref(),
            )
        }
        super::target::CreateSource::WebArticle { url } => {
            ingest_article_route(
                &config,
                &url,
                &recorded,
                superseded_legacy,
                request.parent,
                request.progress,
                listen_command.as_ref(),
            )
        }
        _ => Err(ingest_error(
            "unexpected target classification".to_string(),
            None,
        )),
    }
}

/// Map a refusing dedupe hit to its typed outcome.
fn outcome_for_refusing_hit(
    config: &Config,
    recorded: &[super::sources::RecordedSource],
    dedupe_key: &str,
    planned_target: Option<&Path>,
) -> Option<IngestOutcome> {
    let hit = super::sources::find_refusing_hit(
        recorded,
        dedupe_key,
        planned_target,
        false,
    )?;
    if hit.is_ref_note {
        Some(IngestOutcome::AlreadyInLibrary {
            note: vault_relative_path_value(config, &hit.path),
        })
    } else {
        Some(IngestOutcome::AlreadyQueued {
            pdf: vault_relative_path_value(config, &hit.path),
        })
    }
}

#[allow(clippy::too_many_arguments)]
fn ingest_pdf_url_route(
    config: &Config,
    url: &WebUrl,
    downloaded: &Path,
    scratch: &mut super::workdir::ScratchDir,
    recorded: &[super::sources::RecordedSource],
    superseded_legacy: Option<String>,
    parent: &str,
    progress: Option<&dyn Fn(&str)>,
    listen: Option<&ListenCommand>,
) -> Result<IngestOutcome, IngestError> {
    let captured = current_local_date();
    let pdf_plan = super::pdf_target::plan_pdf_url(
        url, downloaded, None, None, None, None, &captured, progress,
    )
    .map_err(command_error)?;
    let identity = DefaultTargetIdentity::Url {
        keys: vec![url.dedupe_key.clone()],
        recorded,
    };
    let target_plan = plan_default_target(
        config,
        std::ffi::OsStr::new(&pdf_plan.stem),
        INGEST_PDF_REF_TYPE,
        false,
        &identity,
    )
    .map_err(command_error)?;
    let final_stem = target_plan.stem.clone();
    if let Some(outcome) = outcome_for_refusing_hit(
        config,
        recorded,
        &url.dedupe_key,
        Some(&target_plan.target),
    ) {
        return Ok(outcome);
    }
    let library_hint: Option<&Path> = match &target_plan.workflow {
        super::stamp::TargetWorkflow::Intake {
            library_destination,
        } => Some(library_destination),
        _ => None,
    };
    super::pdf_target::refuse_marked_pdf(downloaded, library_hint)
        .map_err(command_error)?;
    let id = Some(final_stem);
    let marker = super::pdf_target::compose_pdf_marker(
        INGEST_STATUS,
        parent,
        &pdf_plan,
        id.as_deref(),
    )
    .map_err(command_error)?;
    let info = PdfInfo {
        title: Some(pdf_plan.title.clone()),
        author: pdf_plan.author.clone(),
    };
    // With --listen `{target}` is the cleaned URL and `{pdf}` is the
    // downloaded scratch copy, matching CLI create.
    if let Some(command) = listen {
        stamp_listen_and_install_for_ingest(
            scratch,
            &target_plan,
            command,
            &url.cleaned,
            downloaded,
            &pdf_plan.title,
            &url.cleaned,
            downloaded,
            &marker,
            &info,
            progress,
        )?;
    } else {
        let (stamped, page_count) = super::pdf_target::stamp_pdf_to_scratch(
            downloaded, scratch, &marker, &info,
        )
        .map_err(command_error)?;
        super::pdf_target::install_stamped_pdf(
            &stamped,
            &target_plan.target,
            None,
            page_count,
        )
        .map_err(command_error)?;
    }
    Ok(IngestOutcome::Created {
        pdf: vault_relative_path_value(config, &target_plan.target),
        ref_type: INGEST_PDF_REF_TYPE.to_string(),
        title: Some(pdf_plan.title),
        route: IngestRoute::Pdf,
        superseded_legacy,
    })
}

#[allow(clippy::too_many_arguments)]
fn ingest_arxiv_route(
    config: &Config,
    paper: &ArxivPaper,
    url: &WebUrl,
    recorded: &[super::sources::RecordedSource],
    superseded_legacy: Option<String>,
    parent: &str,
    progress: Option<&dyn Fn(&str)>,
    listen: Option<&ListenCommand>,
) -> Result<IngestOutcome, IngestError> {
    let captured = current_local_date();
    let scratch =
        super::workdir::ScratchDir::create("ingest").map_err(command_error)?;
    let dest = scratch.path().join("arxiv.pdf");
    let pdf_url = paper.pdf_url();
    let fetch = super::fetch::fetch_url(&pdf_url, &dest, 300, None).map_err(
        |error| {
            ingest_error(
                format!("fetch arXiv PDF {pdf_url}: {}", error.message()),
                error.hint().map(str::to_string),
            )
        },
    )?;
    if !(200..300).contains(&fetch.status) {
        let message =
            format!("server returned HTTP {} for {pdf_url}", fetch.status);
        return Err(ingest_error(message, None));
    }
    if !super::target::file_starts_with_pdf(&dest) {
        return Err(ingest_error(
            format!("server said PDF but sent something else: {pdf_url}"),
            None,
        ));
    }
    let (metadata, warning) =
        super::arxiv::fetch_metadata(paper, scratch.path(), None);
    if let Some(warning) = warning
        && let Some(report) = progress
    {
        report(&format!("warning: {warning}"));
    }
    let pdf_plan = super::pdf_target::plan_arxiv(
        paper,
        None,
        None,
        None,
        None,
        metadata.as_ref(),
        &dest,
        &captured,
        progress,
    )
    .map_err(command_error)?;
    let key = paper.dedupe_key();
    let identity = DefaultTargetIdentity::Url {
        keys: vec![key.clone(), url.dedupe_key.clone()],
        recorded,
    };
    let target_plan = plan_default_target(
        config,
        std::ffi::OsStr::new(&pdf_plan.stem),
        INGEST_PDF_REF_TYPE,
        false,
        &identity,
    )
    .map_err(command_error)?;
    let final_stem = target_plan.stem.clone();
    if let Some(outcome) = outcome_for_refusing_hit(
        config,
        recorded,
        &key,
        Some(&target_plan.target),
    )
    .or_else(|| {
        outcome_for_refusing_hit(
            config,
            recorded,
            &url.dedupe_key,
            Some(&target_plan.target),
        )
    }) {
        return Ok(outcome);
    }
    let library_hint: Option<&Path> = match &target_plan.workflow {
        super::stamp::TargetWorkflow::Intake {
            library_destination,
        } => Some(library_destination),
        _ => None,
    };
    super::pdf_target::refuse_marked_pdf(&dest, library_hint)
        .map_err(command_error)?;
    let id = Some(final_stem);
    let marker = super::pdf_target::compose_pdf_marker(
        INGEST_STATUS,
        parent,
        &pdf_plan,
        id.as_deref(),
    )
    .map_err(command_error)?;
    let info = PdfInfo {
        title: Some(pdf_plan.title.clone()),
        author: pdf_plan.author.clone(),
    };
    // With --listen `{target}` is the cleaned URL (the listen renderer does
    // its own arXiv rewrite) and `{pdf}` is the fetched scratch copy.
    if let Some(command) = listen {
        // `dest` lives in a function-local scratch that would drop before
        // install; move it into the outer scratch's namespace by copying
        // to a stable path. Simplest: use `dest` directly since `scratch`
        // below is a different dir – copy to the outer scratch via the
        // helper's scratch param by reusing `dest` as stamp source while
        // the helper uses its own scratch for stamped output. To keep
        // lifetimes simple, create the listen flow against the local
        // `scratch` variable (shadowed) – we need a mutable scratch for
        // the helper, so use a new scratch for stamping.
        // Actually `dest` is inside the local `scratch` dir which lives
        // until return, so passing `&mut scratch` (local) works if we
        // shadow: the helper needs `&mut ScratchDir`; we have `scratch`
        // (local) – but it was moved? No, `scratch` is local mut, `dest`
        // borrows from it. We need a second scratch for stamped output to
        // avoid borrowing conflicts: reuse the same `scratch` by cloning
        // `dest` path (it lives as long as `scratch`).
        let mut listen_scratch =
            super::workdir::ScratchDir::create("ingest-listen")
                .map_err(command_error)?;
        // Copy the fetched PDF into the listen scratch as the unstamped
        // source so the helper's stamp/output paths are stable.
        let staged = listen_scratch.path().join("arxiv-source.pdf");
        std::fs::copy(&dest, &staged).map_err(|error| {
            ingest_error(
                format!("stage arXiv PDF {}: {error}", dest.display()),
                None,
            )
        })?;
        stamp_listen_and_install_for_ingest(
            &mut listen_scratch,
            &target_plan,
            command,
            &url.cleaned,
            &staged,
            &pdf_plan.title,
            &url.cleaned,
            &staged,
            &marker,
            &info,
            progress,
        )?;
    } else {
        let (stamped, page_count) = super::pdf_target::stamp_pdf_to_scratch(
            &dest, &scratch, &marker, &info,
        )
        .map_err(command_error)?;
        super::pdf_target::install_stamped_pdf(
            &stamped,
            &target_plan.target,
            None,
            page_count,
        )
        .map_err(command_error)?;
    }
    Ok(IngestOutcome::Created {
        pdf: vault_relative_path_value(config, &target_plan.target),
        ref_type: INGEST_PDF_REF_TYPE.to_string(),
        title: Some(pdf_plan.title),
        route: IngestRoute::Arxiv,
        superseded_legacy,
    })
}

#[allow(clippy::too_many_arguments)]
fn ingest_article_route(
    config: &Config,
    url: &WebUrl,
    recorded: &[super::sources::RecordedSource],
    superseded_legacy: Option<String>,
    parent: &str,
    progress: Option<&dyn Fn(&str)>,
    listen: Option<&ListenCommand>,
) -> Result<IngestOutcome, IngestError> {
    // Fail fast when the stem is already known, mirroring the web-article engine.
    if let Some(stem) = stem_from_url(url) {
        let pre_identity = DefaultTargetIdentity::Url {
            keys: vec![url.dedupe_key.clone()],
            recorded,
        };
        let pre_plan = plan_default_target(
            config,
            std::ffi::OsStr::new(&stem),
            INGEST_ARTICLE_REF_TYPE,
            false,
            &pre_identity,
        )
        .map_err(command_error)?;
        if let Some(outcome) = outcome_for_refusing_hit(
            config,
            recorded,
            &url.dedupe_key,
            Some(&pre_plan.target),
        ) {
            return Ok(outcome);
        }
    }
    let workdir =
        super::workdir::ScratchDir::create("ingest").map_err(command_error)?;
    let out_pdf = workdir.path().join("render.pdf");
    let captured = current_local_date();
    let client = ClipAdapterClient::resolve().map_err(|failure| {
        ingest_error(failure.message.clone(), failure.hint.clone())
    })?;
    if let Some(report) = progress {
        report(&format!("capturing {}…", url.host));
    }
    let request = CaptureRequest::new(
        url.cleaned.clone(),
        None,
        workdir.path().to_string_lossy().into_owned(),
        out_pdf.to_string_lossy().into_owned(),
        false,
        captured.clone(),
        MetadataOverrides::default(),
    );
    let response = client.capture(&request).map_err(|failure| {
        ingest_error(failure.message.clone(), failure.hint.clone())
    })?;
    let is_direct_pdf = response.kind.as_deref() == Some("pdf");
    let mut extracted_title = response.title.clone();
    if extracted_title.is_none() && is_direct_pdf {
        let stem_hint = stem_from_url(url).unwrap_or_default();
        extracted_title =
            super::pdf_meta::pdf_info_metadata(&out_pdf, &stem_hint).0;
    }
    let stem = stem_from_url(url)
        .or_else(|| extracted_title.as_ref().map(|title| snake_case(title)))
        .filter(|stem| !stem.is_empty())
        .unwrap_or_else(|| {
            format!(
                "{}_{}",
                url.host.replace('.', "_"),
                captured.replace('-', "")
            )
        });
    let title = extracted_title
        .or_else(|| Some(humanize_stem(&stem)))
        .filter(|title| !title.is_empty())
        .ok_or_else(|| {
            ingest_error(
                "could not derive a title for the capture".to_string(),
                None,
            )
        })?;
    let author = response.author.clone();
    let published = response.published.clone();
    let final_identity = DefaultTargetIdentity::Url {
        keys: vec![url.dedupe_key.clone()],
        recorded,
    };
    let plan = plan_default_target(
        config,
        std::ffi::OsStr::new(&stem),
        INGEST_ARTICLE_REF_TYPE,
        false,
        &final_identity,
    )
    .map_err(command_error)?;
    let final_stem = plan.stem.clone();
    let marker = compose_marker(
        INGEST_STATUS,
        parent,
        &title,
        Some(&final_stem),
        &[
            ("source_url", MarkerValue::String(url.cleaned.clone())),
            (
                "author",
                MarkerValue::String(author.clone().unwrap_or_default()),
            ),
            (
                "published",
                MarkerValue::String(published.clone().unwrap_or_default()),
            ),
            ("captured", MarkerValue::String(captured.clone())),
        ],
    )
    .map_err(command_error)?;
    if let Some(outcome) = outcome_for_refusing_hit(
        config,
        recorded,
        &url.dedupe_key,
        Some(&plan.target),
    ) {
        return Ok(outcome);
    }
    let info = PdfInfo {
        title: Some(title.clone()),
        author: author.clone(),
    };
    // With --listen the staged render doubles as the narration source:
    // `{target}` and `{pdf}` are the scratch render PDF, matching CLI.
    if let Some(command) = listen {
        let mut listen_scratch =
            super::workdir::ScratchDir::create("ingest-listen")
                .map_err(command_error)?;
        let target_display = out_pdf.display().to_string();
        stamp_listen_and_install_for_ingest(
            &mut listen_scratch,
            &plan,
            command,
            &target_display,
            &out_pdf,
            &title,
            &url.cleaned,
            &out_pdf,
            &marker,
            &info,
            progress,
        )?;
    } else {
        stamp_and_install(&out_pdf, &plan.target, &marker, &info)
            .map_err(command_error)?;
    }
    // Keep the workdir alive until after install; cleanup via drop.
    let _ = workdir.path();
    Ok(IngestOutcome::Created {
        pdf: vault_relative_path_value(config, &plan.target),
        ref_type: INGEST_ARTICLE_REF_TYPE.to_string(),
        title: Some(title),
        route: IngestRoute::Article,
        superseded_legacy,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::env;
    use std::path::PathBuf;

    fn test_root(name: &str) -> PathBuf {
        let dir = env::temp_dir().join(format!(
            "bob-ingest-test-{}-{}-{name}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|duration| duration.as_nanos())
                .unwrap_or(0),
        ));
        fs::create_dir_all(&dir).expect("create ingest test dir");
        dir
    }

    #[test]
    fn error_kinds_and_retryable_flags() {
        let cases = [
            ("network: down", IngestErrorKind::Network, true),
            ("timeout: slow", IngestErrorKind::Timeout, true),
            ("browser: no chrome", IngestErrorKind::Browser, true),
            ("blocked: wall", IngestErrorKind::Blocked, false),
            ("thin: empty", IngestErrorKind::Thin, false),
            ("render: broken", IngestErrorKind::Render, false),
            ("curl command not found: curl", IngestErrorKind::Dependency, true),
            (
                "uv was not found on PATH",
                IngestErrorKind::Dependency,
                true,
            ),
            (
                "server returned HTTP 503 for https://example.com/a",
                IngestErrorKind::HttpStatus,
                true,
            ),
            (
                "server returned HTTP 404 for https://example.com/a",
                IngestErrorKind::HttpStatus,
                false,
            ),
            (
                "server returned HTTP 429 for https://example.com/a",
                IngestErrorKind::HttpStatus,
                true,
            ),
            (
                "server said PDF but sent something else: https://example.com/a",
                IngestErrorKind::UnsupportedContent,
                false,
            ),
            (
                "refusing to create x because the library destination already exists",
                IngestErrorKind::Collision,
                false,
            ),
            (
                "already captured as x (title report)",
                IngestErrorKind::Collision,
                false,
            ),
            (
                "already queued in x; pass --force to overwrite it",
                IngestErrorKind::Collision,
                false,
            ),
            ("invalid URL \"x\": bad", IngestErrorKind::InvalidUrl, false),
            (
                "curl failed (exit 9) for https://example.com/a",
                IngestErrorKind::Network,
                true,
            ),
            (
                "curl failed (exit 9): boom",
                IngestErrorKind::Network,
                true,
            ),
            ("anything else", IngestErrorKind::Internal, false),
        ];
        for (message, kind, retryable) in cases {
            let error = ingest_error(message.to_string(), None);
            assert_eq!(error.kind, kind, "{message}");
            assert_eq!(error.retryable(), retryable, "{message}");
            assert_eq!(error.kind.as_str(), error.kind.as_str());
        }
    }

    #[test]
    fn fetch_hint_splits_out_of_message() {
        let raw = "server returned HTTP 404 for https://example.com/a\nhint: save the page from a browser and pass --html FILE";
        let error =
            command_error(super::super::CommandError::new(raw.to_string()));
        assert_eq!(
            error.message,
            "server returned HTTP 404 for https://example.com/a"
        );
        assert!(!error.message.contains('\n'));
        assert_eq!(
            error.hint.as_deref(),
            Some("save the page from a browser and pass --html FILE")
        );
    }

    #[test]
    fn fallback_note_quoting_truncation_and_escaping() {
        // Bare URLs stay unquoted.
        let error = ingest_error("blocked: no entry".to_string(), None);
        assert_eq!(
            error.fallback_note("https://example.com/post", "mac_inbox", false),
            "⚠️ Clip failed (blocked): blocked: no entry · retry: bob ref create https://example.com/post -P mac_inbox",
        );
        // Query strings need single quotes.
        assert!(error
            .fallback_note("https://example.com/a?b=1&c=2", "mac_inbox", false)
            .ends_with(
                "bob ref create 'https://example.com/a?b=1&c=2' -P mac_inbox"
            ),);
        // Embedded quotes use '\''.
        assert!(error
            .fallback_note("https://example.com/a'b", "mac_inbox", false)
            .ends_with(
                "bob ref create 'https://example.com/a'\\''b' -P mac_inbox"
            ),);
        // Only the first line survives, whitespace-collapsed, truncated.
        let long = format!("line one\nline two {}", "x".repeat(200));
        let long_note = ingest_error(long, None).fallback_note(
            "https://example.com/post",
            "mac_inbox",
            false,
        );
        assert!(long_note.contains("line one"));
        assert!(!long_note.contains("line two"));
        let long_first = format!("{} tail", "x".repeat(200));
        let long_first_note = ingest_error(long_first, None).fallback_note(
            "https://example.com/post",
            "mac_inbox",
            false,
        );
        let long_first_part = long_first_note
            .split("Clip failed (internal): ")
            .nth(1)
            .unwrap_or_default()
            .split(" · retry:")
            .next()
            .unwrap_or_default();
        assert_eq!(long_first_part, format!("{}…", "x".repeat(120)));
        let multi = ingest_error("first   line\nsecond line".to_string(), None);
        assert!(multi
            .fallback_note("https://example.com/post", "mac_inbox", false)
            .contains("first line · retry:"),);
        let huge = ingest_error(format!("{} end", "y".repeat(200)), None);
        let note =
            huge.fallback_note("https://example.com/post", "mac_inbox", false);
        let message_part = note
            .split("Clip failed (internal): ")
            .nth(1)
            .unwrap_or_default()
            .split(" · retry:")
            .next()
            .unwrap_or_default();
        assert!(message_part.ends_with('…'));
        assert!(message_part.chars().count() <= 121);
        // Keep child-text escaping applies (leading `#task` is escaped).
        let tasky = ingest_error("#task list".to_string(), None);
        assert!(tasky
            .fallback_note("https://example.com/post", "mac_inbox", false)
            .contains("\\#task list"),);
    }

    #[test]
    fn for_vault_agrees_with_from_matches() {
        let root = test_root("for-vault");
        let vault = root.join("vault");
        let matches = super::super::create::command()
            .try_get_matches_from([
                "create",
                "report.md",
                "-b",
                vault.to_str().expect("utf8 vault"),
            ])
            .expect("parse create matches");
        let from_matches = Config::from_matches(&matches);
        let for_vault = Config::for_vault(&vault);
        assert_eq!(from_matches, for_vault);
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn second_ingest_waits_for_the_lock() {
        use std::sync::{Arc, Barrier};
        use std::time::Duration;
        let dir = test_root("ingest-lock");
        let ref_dir = dir.join("ref");
        let held = lock_ingest_at(&ref_dir, None).expect("take ingest lock");
        let entered = Arc::new(Barrier::new(2));
        let entered_child = Arc::clone(&entered);
        let progress_seen = Arc::new(std::sync::Mutex::new(Vec::new()));
        let progress_child = Arc::clone(&progress_seen);
        let handle = std::thread::spawn(move || {
            entered_child.wait();
            let _guard = lock_ingest_at(
                &ref_dir,
                Some(&|line: &str| {
                    progress_child
                        .lock()
                        .expect("lock progress")
                        .push(line.to_string());
                }),
            )
            .expect("second lock");
        });
        entered.wait();
        std::thread::sleep(Duration::from_millis(200));
        drop(held);
        handle.join().expect("join waiter");
        let seen = progress_seen.lock().expect("read progress").clone();
        assert_eq!(seen, vec!["waiting for another clip…".to_string()]);
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn already_in_library_needs_no_network() {
        let root = test_root("already-in-library");
        let vault = root.join("vault");
        let note_dir = vault.join("ref/papers");
        fs::create_dir_all(&note_dir).expect("create ref dir");
        fs::write(
            note_dir.join("post.md"),
            "---\ntitle: Post\nsource_url: https://example.com/post\nsource_pdf: lib/papers/post.pdf\n---\n\n# Post\n",
        )
        .expect("write ref note");
        let request = IngestRequest {
            bob_dir: &vault,
            url: "https://example.com/post",
            parent: "mac_inbox",
            listen: false,
            progress: None,
        };
        match ingest_url(&request).expect("ingest library hit") {
            IngestOutcome::AlreadyInLibrary { note } => {
                assert!(note.ends_with("post.md"), "{note}");
            }
            other => panic!("expected AlreadyInLibrary, got {other:?}"),
        }
        // An invalid URL is a typed invalid_url error, never a panic.
        let bad = IngestRequest {
            bob_dir: &vault,
            url: "not a url",
            parent: "mac_inbox",
            listen: false,
            progress: None,
        };
        let error = ingest_url(&bad).expect_err("invalid URL must fail");
        assert_eq!(error.kind, IngestErrorKind::InvalidUrl);
        assert!(!error.retryable());
        fs::remove_dir_all(&root).ok();
    }
}
