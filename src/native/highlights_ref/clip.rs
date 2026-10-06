//! `bob highlights clip`: capture a web article as a Highlights-ready PDF.
//!
//! A sibling of `create` that shares its target, collision, marker, and
//! install code. It captures reader-mode HTML through the pinned web-clip
//! adapter (protocol v1), stamps the rendered PDF with provenance, and
//! installs it into the Highlights intake. `scan` writes the ref note.

use std::{
    env,
    ffi::OsString,
    fs,
    io::{IsTerminal, Read},
    path::{Path, PathBuf},
    process,
    time::{SystemTime, UNIX_EPOCH},
};

use clap::{Arg, ArgAction, ArgMatches, Command as ClapCommand};

use super::{
    bob_dir_arg, compose_marker, current_local_date, lib_dir_arg,
    plan_default_target, plan_exact_output, print_next_step, ref_dir_arg,
    stamp_and_install, validate_ref_type, xlib_dir_arg, CommandError, Config,
    PdfInfo, TargetPlan, TargetWorkflow,
};
use super::{clip_adapter::*, clip_url::*, pdf_meta::*, sources::*};
use crate::native::style::Styler;

const DEFAULT_PARENT: &str = "obsidian_ref";
const DEFAULT_REF_TYPE: &str = "blogs";
const DEFAULT_STATUS: &str = "ready";
const ENV_KEEP_WORKDIR: &str = "BOB_WEB_CLIP_KEEP_WORKDIR";

/// A clip failure: the message goes after `error:`, the hint after `hint:`.
#[derive(Debug, Clone, PartialEq, Eq)]
struct ClipError {
    message: String,
    hint: Option<String>,
}

impl ClipError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            hint: None,
        }
    }

    fn with_hint(mut self, hint: impl Into<String>) -> Self {
        self.hint = Some(hint.into());
        self
    }
}

impl From<CommandError> for ClipError {
    fn from(error: CommandError) -> Self {
        Self::new(error.message)
    }
}

impl From<SourcesError> for ClipError {
    fn from(error: SourcesError) -> Self {
        Self {
            message: error.message,
            hint: error.hint,
        }
    }
}

impl From<AdapterFailure> for ClipError {
    fn from(failure: AdapterFailure) -> Self {
        Self {
            message: failure.message,
            hint: failure.hint,
        }
    }
}

pub(crate) fn command() -> ClapCommand {
    ClapCommand::new("clip")
        .about("Capture a web article as a Highlights-ready PDF")
        .arg(
            Arg::new("url")
                .value_name("URL")
                .required(true)
                .help("http(s) URL of the article to capture; recorded as the marker source_url"),
        )
        .arg(
            Arg::new("author")
                .long("author")
                .short('A')
                .value_name("NAME")
                .help("Override the extracted author"),
        )
        .arg(bob_dir_arg())
        .arg(
            Arg::new("dry-run")
                .long("dry-run")
                .short('d')
                .action(ArgAction::SetTrue)
                .help("Capture and extract, then print the plan, marker, metadata sources, and fidelity report; write nothing"),
        )
        .arg(
            Arg::new("force")
                .long("force")
                .short('f')
                .action(ArgAction::SetTrue)
                .help("Overwrite an existing intake PDF for this capture (never a library PDF)"),
        )
        .arg(
            Arg::new("html")
                .long("html")
                .short('H')
                .value_name("FILE")
                .value_parser(clap::builder::OsStringValueParser::new())
                .help("Use a page already saved from a browser (Save Page As, SingleFile); `-` reads stdin"),
        )
        .arg(lib_dir_arg())
        .arg(
            Arg::new("name")
                .long("name")
                .short('N')
                .value_name("STEM")
                .help("Output filename stem and marker id [default: derived from the URL]"),
        )
        .arg(
            Arg::new("output")
                .long("output")
                .short('o')
                .value_name("PDF")
                .value_parser(clap::builder::OsStringValueParser::new())
                .conflicts_with("ref-type")
                .conflicts_with("name")
                .help("Complete path for the generated PDF, including the .pdf filename"),
        )
        .arg(
            Arg::new("parent")
                .long("parent")
                .short('P')
                .value_name("NOTE")
                .default_value(DEFAULT_PARENT)
                .help("Bare Obsidian note target for the marker parent"),
        )
        .arg(
            Arg::new("published")
                .long("published")
                .short('p')
                .value_name("DATE")
                .help("Override the extracted publish date (YYYY-MM-DD)"),
        )
        .arg(ref_dir_arg())
        .arg(
            Arg::new("status")
                .long("status")
                .short('s')
                .value_name("STATUS")
                .default_value(DEFAULT_STATUS)
                .value_parser([
                    "ready",
                    "next",
                    "wip",
                    "read",
                    "abandoned",
                    "legacy",
                ])
                .help("Lifecycle status embedded in the marker"),
        )
        .arg(
            Arg::new("title")
                .long("title")
                .short('T')
                .value_name("TITLE")
                .help("Override the extracted title"),
        )
        .arg(
            Arg::new("ref-type")
                .long("ref-type")
                .short('t')
                .value_name("DIR")
                .default_value(DEFAULT_REF_TYPE)
                .conflicts_with("output")
                .help("Single library subdirectory for the generated PDF"),
        )
        .arg(xlib_dir_arg())
        .after_help(
            "Captures the article in reader mode and re-typesets it with a Bob-owned print template; \
            it never prints the live page. `bob highlights scan` later moves the intake PDF into the \
            library and writes the reference note. A site that blocks headless browsers is retried \
            headed automatically: on Linux under a private Xvfb display (as on athena) or in an \
            off-screen window on macOS; hosts with no browser fail closed with a hint. `--html FILE` \
            replays a page saved from a real browser instead of fetching. \
            Environment: BOB_WEB_CLIP_ADAPTER replaces the adapter invocation; BOB_CHROME selects the \
            browser executable; BOB_WEB_CLIP_TIMEOUT_SECS sets the adapter timeout in seconds \
            (default 300); BOB_WEB_CLIP_KEEP_WORKDIR=1 keeps the scratch directory for debugging. \
            `--output` cannot be combined with `--ref-type` or `--name` because those only \
            participate in default target derivation.",
        )
}

#[derive(Debug, Clone)]
struct ClipOptions {
    dry_run: bool,
    force: bool,
    author: Option<String>,
    html: Option<OsString>,
    name: Option<String>,
    output: Option<PathBuf>,
    parent: String,
    published: Option<String>,
    ref_type: String,
    status: String,
    title: Option<String>,
}

pub(super) fn run(matches: &ArgMatches) -> i32 {
    let styler = Styler::detect();
    match clip_pdf(&Config::from_matches(matches), matches) {
        Ok(()) => 0,
        Err(error) => {
            eprintln!(
                "bob highlights: {}: {}",
                styler.red("error"),
                error.message
            );
            if let Some(hint) = error.hint {
                eprintln!("hint: {hint}");
            }
            1
        }
    }
}

fn clip_options(
    matches: &ArgMatches,
) -> std::result::Result<ClipOptions, ClipError> {
    let name = matches
        .get_one::<String>("name")
        .map(|name| validate_name(name))
        .transpose()?;
    if let Some(published) = matches.get_one::<String>("published") {
        validate_published(published)?;
    }
    let ref_type = matches
        .get_one::<String>("ref-type")
        .expect("defaulted by clap")
        .clone();
    validate_ref_type(&ref_type)?;
    Ok(ClipOptions {
        dry_run: matches.get_flag("dry-run"),
        force: matches.get_flag("force"),
        author: matches.get_one::<String>("author").cloned(),
        html: matches.get_one::<OsString>("html").cloned(),
        name,
        output: matches.get_one::<OsString>("output").map(PathBuf::from),
        parent: matches
            .get_one::<String>("parent")
            .expect("defaulted by clap")
            .clone(),
        published: matches.get_one::<String>("published").cloned(),
        ref_type,
        status: matches
            .get_one::<String>("status")
            .expect("defaulted by clap")
            .clone(),
        title: matches.get_one::<String>("title").cloned(),
    })
}

fn clip_pdf(
    config: &Config,
    matches: &ArgMatches,
) -> std::result::Result<(), ClipError> {
    let raw_url = matches
        .get_one::<String>("url")
        .expect("required by clap")
        .clone();
    let options = clip_options(matches)?;
    let web_url = validate_and_clean(&raw_url)?;
    if let Some(html) = &options.html
        && html != "-"
        && !Path::new(html).is_file()
    {
        return Err(ClipError::new(format!(
            "HTML input does not exist or is not a file: {}",
            Path::new(html).display()
        )));
    }

    // Dedupe against ref notes and queued intake PDFs before launching
    // anything expensive. A ref-note hit always refuses. An xlib hit
    // refuses unless --force can still prove the same target: when the
    // stem is only known after capture, that proof waits for the final
    // plan below.
    let recorded = collect_recorded_source_urls(config)?;
    check_dedupe(&recorded, &web_url.dedupe_key, None, options.force)?;

    // Fail fast when the target is already known: --output, --name, or a
    // URL slug all plan before the adapter runs.
    let pre_stem = options.name.clone().or_else(|| stem_from_url(&web_url));
    let pre_plan = match &options.output {
        Some(output) => {
            Some(plan_exact_output_guarded(config, output, options.force)?)
        }
        None => pre_stem
            .as_ref()
            .map(|stem| {
                plan_default_target(
                    config,
                    std::ffi::OsStr::new(stem),
                    &options.ref_type,
                    options.force,
                )
            })
            .transpose()?,
    };
    if let Some(plan) = &pre_plan {
        check_dedupe(
            &recorded,
            &web_url.dedupe_key,
            Some(&plan.target),
            options.force,
        )?;
    }

    let workdir = ClipWorkdir::create()?;
    let html_path = match &options.html {
        None => None,
        Some(path) if path == "-" => {
            let replay = workdir.path().join("input.html");
            let mut stdin = String::new();
            std::io::stdin()
                .read_to_string(&mut stdin)
                .map_err(|error| {
                    ClipError::new(format!("read HTML from stdin: {error}"))
                })?;
            fs::write(&replay, stdin).map_err(|error| {
                ClipError::new(format!(
                    "write stdin HTML {}: {error}",
                    replay.display()
                ))
            })?;
            Some(replay.into_os_string())
        }
        Some(path) => Some(path.clone()),
    };
    let out_pdf = workdir.path().join("render.pdf");
    let captured = current_local_date();
    let overrides = MetadataOverrides {
        title: options.title.clone(),
        author: options.author.clone(),
        published: options.published.clone(),
    };
    let request = CaptureRequest::new(
        web_url.cleaned.clone(),
        html_path.map(|path| path.to_string_lossy().into_owned()),
        workdir.path().to_string_lossy().into_owned(),
        out_pdf.to_string_lossy().into_owned(),
        options.dry_run,
        captured.clone(),
        overrides,
    );

    let client = ClipAdapterClient::resolve()?;
    if std::io::stderr().is_terminal() {
        eprintln!("capturing {}…", web_url.host);
    }
    let response = client.capture(&request)?;

    // Finalize the metadata: overrides win, then the adapter extraction.
    // A direct-PDF capture carries no article metadata, so the PDF Info
    // title (then a humanized slug) stands in.
    let is_direct_pdf = response.kind.as_deref() == Some("pdf");
    let mut extracted_title = options.title.clone().or(response.title.clone());
    if extracted_title.is_none() && is_direct_pdf {
        let stem_hint = options
            .name
            .clone()
            .or_else(|| stem_from_url(&web_url))
            .unwrap_or_default();
        extracted_title = pdf_info_metadata(&out_pdf, &stem_hint).0;
    }
    let stem = options
        .name
        .clone()
        .or_else(|| stem_from_url(&web_url))
        .or_else(|| extracted_title.as_ref().map(|title| snake_case(title)))
        .filter(|stem| !stem.is_empty())
        .unwrap_or_else(|| {
            format!(
                "{}_{}",
                web_url.host.replace('.', "_"),
                captured.replace('-', "")
            )
        });
    let title = extracted_title
        .or_else(|| Some(humanize_stem(&stem)))
        .filter(|title| !title.is_empty())
        .ok_or_else(|| {
            ClipError::new("could not derive a title for the capture")
        })?;
    let author = options.author.clone().or(response.author.clone());
    let published = options.published.clone().or(response.published.clone());
    let marker = compose_marker(
        &options.status,
        &options.parent,
        &title,
        Some(&stem),
        &[
            ("source_url", web_url.cleaned.clone()),
            ("author", author.clone().unwrap_or_default()),
            ("published", published.clone().unwrap_or_default()),
            ("captured", captured.clone()),
        ],
    )?;
    let plan = match &options.output {
        Some(output) => {
            plan_exact_output_guarded(config, output, options.force)?
        }
        None => plan_default_target(
            config,
            std::ffi::OsStr::new(&stem),
            &options.ref_type,
            options.force,
        )?,
    };
    check_dedupe(
        &recorded,
        &web_url.dedupe_key,
        Some(&plan.target),
        options.force,
    )?;

    let styler = Styler::detect();
    if options.dry_run {
        print_dry_run(
            &styler, &web_url, &plan, &options, &title, &author, &published,
            &captured, &response,
        );
        return Ok(());
    }

    let info = PdfInfo {
        title: Some(title.clone()),
        author: author.clone(),
    };
    let page_count = stamp_and_install(&out_pdf, &plan.target, &marker, &info)?;
    let installed_bytes = fs::metadata(&plan.target)
        .map(|metadata| metadata.len())
        .unwrap_or(0);

    println!(
        "{} created Highlights-ready web PDF",
        styler.success_prefix(false)
    );
    println!("pdf: {}", plan.target.display());
    println!("title: {title}");
    if let Some(author) = &author {
        println!("author: {author}");
    }
    if let Some(published) = &published {
        println!("published: {published}");
    }
    println!("captured: {captured}");
    println!("source_url: {}", web_url.cleaned);
    println!("status: {}", options.status);
    println!("parent: {}", options.parent);
    println!("id: {stem}");
    println!("capture: {}", describe_capture(&response));
    println!(
        "pages: {page_count} · images: {}/{} · size: {} · fidelity: {}",
        response.images.kept,
        response.images.total,
        format_bytes(installed_bytes),
        response.fidelity.status.as_deref().unwrap_or("unknown"),
    );
    print_next_step(&plan);
    print_warnings(&styler, &response.warnings);
    Ok(())
}

/// Plan an exact `--output` target, refusing library destinations outright:
/// clip never overwrites an archived library PDF, even with `--force`.
fn plan_exact_output_guarded(
    config: &Config,
    output: &Path,
    force: bool,
) -> std::result::Result<TargetPlan, ClipError> {
    let plan = plan_exact_output(config, output, force)?;
    if matches!(plan.workflow, TargetWorkflow::Library) {
        return Err(ClipError::new(format!(
            "refusing to write {} because it is inside the Highlights library; clip only writes intake or explicit non-library paths",
            plan.target.display()
        )));
    }
    Ok(plan)
}

/// A 0700 scratch directory, removed on drop unless kept for debugging.
struct ClipWorkdir {
    path: PathBuf,
    keep: bool,
}

impl ClipWorkdir {
    fn create() -> std::result::Result<Self, ClipError> {
        let base = env::var_os("TMPDIR")
            .map(PathBuf::from)
            .filter(|base| base.as_os_str().as_encoded_bytes().len() <= 40)
            .unwrap_or_else(|| PathBuf::from("/tmp"));
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or(0);
        let path = base.join(format!("bob-clip-{}-{nanos}", process::id()));
        fs::create_dir_all(&path).map_err(|error| {
            ClipError::new(format!(
                "create workdir {}: {error}",
                path.display()
            ))
        })?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&path, fs::Permissions::from_mode(0o700))
                .map_err(|error| {
                    ClipError::new(format!(
                        "secure workdir {}: {error}",
                        path.display()
                    ))
                })?;
        }
        Ok(Self {
            path,
            keep: env::var_os(ENV_KEEP_WORKDIR).as_deref()
                == Some(std::ffi::OsStr::new("1")),
        })
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for ClipWorkdir {
    fn drop(&mut self) {
        if self.keep {
            eprintln!("workdir: {}", self.path.display());
        } else {
            let _ = fs::remove_dir_all(&self.path);
        }
    }
}

/// One-line `capture:` summary for the success and dry-run reports.
fn describe_capture(response: &CaptureSuccess) -> String {
    let browser = response
        .capture
        .browser
        .as_deref()
        .unwrap_or("unknown browser");
    let version = response
        .capture
        .browser_version
        .as_deref()
        .map(|version| format!(" {version}"))
        .unwrap_or_default();
    let mode = response
        .capture
        .mode
        .as_deref()
        .map(mode_label)
        .unwrap_or("unknown mode");
    let mut summary = format!("{browser}{version} · {mode}");
    if response.capture.retried_after_challenge {
        summary.push_str(" after a headless bot challenge");
    }
    summary
}

fn mode_label(mode: &str) -> &str {
    match mode {
        "headless" => "headless",
        "headed-xvfb" => "headed (Xvfb)",
        "headed-display" => "headed (display)",
        "headed-macos" => "headed (macOS)",
        "html-file" => "HTML file",
        "direct-pdf" => "direct PDF",
        _ => mode,
    }
}

fn format_bytes(bytes: u64) -> String {
    if bytes >= 1_000_000 {
        format!("{:.1} MB", bytes as f64 / 1_000_000.0)
    } else if bytes >= 1_000 {
        format!("{:.1} KB", bytes as f64 / 1_000.0)
    } else {
        format!("{bytes} B")
    }
}

fn field_source(
    override_value: &Option<String>,
    response: &CaptureSuccess,
    field: &str,
    fallback: &str,
) -> String {
    if override_value.is_some() {
        return "override".to_string();
    }
    if let Some(source) = response.metadata_sources.get(field) {
        return source.clone();
    }
    fallback.to_string()
}

#[allow(clippy::too_many_arguments)]
fn print_dry_run(
    styler: &Styler,
    web_url: &WebUrl,
    plan: &TargetPlan,
    options: &ClipOptions,
    title: &str,
    author: &Option<String>,
    published: &Option<String>,
    captured: &str,
    response: &CaptureSuccess,
) {
    println!(
        "{} would create Highlights-ready web PDF",
        styler.success_prefix(true)
    );
    println!("source_url: {}", web_url.cleaned);
    println!("pdf: {}", plan.target.display());
    println!("sidecar_guard: {}", plan.sidecar.display());
    match &plan.workflow {
        TargetWorkflow::Intake {
            library_destination,
        } => println!("library_destination: {}", library_destination.display()),
        _ => println!("library_destination: n/a (external target)"),
    }
    println!(
        "title: {title} ({})",
        field_source(&options.title, response, "title", "adapter")
    );
    if let Some(author) = author {
        println!(
            "author: {author} ({})",
            field_source(&options.author, response, "author", "adapter")
        );
    }
    if let Some(published) = published {
        println!(
            "published: {published} ({})",
            field_source(&options.published, response, "published", "adapter")
        );
    }
    println!("captured: {captured}");
    println!("status: {}", options.status);
    println!("parent: {}", options.parent);
    println!("capture: {}", describe_capture(response));
    println!(
        "fidelity: {} ({}/{} words, {}/{} images, {}/{} code blocks)",
        response.fidelity.status.as_deref().unwrap_or("unknown"),
        response.fidelity.kept_words,
        response.fidelity.page_words,
        response.images.kept,
        response.images.total,
        response.fidelity.kept_code_blocks,
        response.fidelity.page_code_blocks,
    );
    print_warnings(styler, &response.warnings);
    let marker = compose_marker(
        &options.status,
        &options.parent,
        title,
        plan.target.file_stem().and_then(|stem| stem.to_str()),
        &[
            ("source_url", web_url.cleaned.clone()),
            ("author", author.clone().unwrap_or_default()),
            ("published", published.clone().unwrap_or_default()),
            ("captured", captured.to_string()),
        ],
    );
    println!("marker:");
    match marker {
        Ok(marker) => print!("{marker}"),
        Err(error) => println!("(unavailable: {error})"),
    }
    println!("writes: none");
}

fn print_warnings(styler: &Styler, warnings: &[String]) {
    for warning in warnings {
        println!("{}: {warning}", styler.warning_prefix());
    }
}

/// Non-fatal `doctor` rows for the web-clip chain, printed after the pandoc
/// row. Missing pieces warn; they never fail the vault doctor.
pub(super) fn append_web_clip_doctor_rows(warnings: &mut Vec<String>) {
    match find_on_path("uv") {
        Some(path) => {
            println!("web clip uv: available ({})", path.display());
        }
        None => {
            println!("web clip uv: warn (uv not found on PATH)");
            warnings.push(
                "uv not found on PATH; bob highlights clip cannot run its capture adapter"
                    .to_string(),
            );
        }
    }
    match ClipAdapterClient::resolve_with_timeout(PING_TIMEOUT_SECS)
        .and_then(|client| client.ping())
    {
        Ok(ping) => {
            println!(
                "web clip adapter: ok (playwright {}, defuddle {})",
                ping.playwright.as_deref().unwrap_or("unknown"),
                ping.defuddle.as_deref().unwrap_or("unknown"),
            );
            match ping.browser {
                Some(browser) => {
                    println!(
                        "web clip browser: {} {} ({})",
                        browser.kind.as_deref().unwrap_or("browser"),
                        browser.version.as_deref().unwrap_or("unknown"),
                        browser.path.as_deref().unwrap_or("unknown"),
                    );
                }
                None => {
                    println!(
                        "web clip browser: warn (no usable browser reported)"
                    );
                    warnings.push(
                        "web clip adapter reports no usable browser; install Google Chrome or set BOB_CHROME=/path/to/chrome"
                            .to_string(),
                    );
                }
            }
            match ping.headed.as_deref() {
                Some("xvfb") => {
                    let path = find_on_path("Xvfb")
                        .or_else(|| find_on_path("xvfb-run"))
                        .map(|path| format!(" ({})", path.display()))
                        .unwrap_or_default();
                    println!("web clip headed fallback: xvfb{path}");
                }
                Some("display") => {
                    println!("web clip headed fallback: display");
                }
                Some("macos") => {
                    println!("web clip headed fallback: macos");
                }
                Some(other) => {
                    println!("web clip headed fallback: {other}");
                }
                None => {
                    println!(
                        "web clip headed fallback: warn (unavailable; bot-protected sites will fail closed)"
                    );
                    warnings.push(
                        "no headed-browser fallback; bot-protected sites will fail closed"
                            .to_string(),
                    );
                }
            }
        }
        Err(error) => {
            let short = error
                .message
                .lines()
                .next()
                .unwrap_or("unknown error")
                .to_string();
            println!("web clip adapter: warn ({short})");
            warnings.push(format!("web clip adapter ping failed: {short}"));
            println!("web clip browser: warn (unknown: adapter ping failed)");
            println!(
                "web clip headed fallback: warn (unknown: adapter ping failed)"
            );
        }
    }
}
