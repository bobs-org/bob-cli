//! Multi-URL `bob ref create` collections: fail-fast gate plus the
//! classify/capture/compose/stamp pipeline.

use std::{
    collections::{BTreeMap, BTreeSet},
    ffi::OsString,
    fs,
    io::IsTerminal,
    path::{Path, PathBuf},
};

use clap::ArgMatches;

use super::{
    clip::format_bytes,
    clip_adapter::{
        CaptureRequest, CaptureSuccess, ClipAdapterClient, CollectionMeta,
        ComposePart, ComposeRequest, ComposeSuccess,
    },
    clip_url::{self, WebUrl},
    collection_name,
    create::CreateOptions,
    sources::{self, RecordedSource},
    stamp::{self, DefaultTargetIdentity, PdfInfo, TargetPlan, TargetWorkflow},
    CommandError, Config, MarkerValue, ScratchDir, Styler,
};

const MAX_COLLECTION_URLS: usize = 20;
const CREATE_COMMAND_NAME: &str = "bob ref create";

#[derive(Debug)]
enum OverlapCheck {
    Clear { legacy: Vec<RecordedSource> },
    Attach(RecordedSource),
}

#[derive(Debug)]
struct SourceGroup {
    path: PathBuf,
    keys: BTreeSet<String>,
    is_ref_note: bool,
    source_pdf: Option<String>,
    first: RecordedSource,
}

impl SourceGroup {
    fn is_legacy(&self) -> bool {
        self.is_ref_note && self.source_pdf.is_none()
    }

    fn refuses(&self) -> bool {
        !self.is_legacy()
    }
}

struct PreparedCollection {
    urls: Vec<WebUrl>,
    options: CreateOptions,
    resolved_name: collection_name::ResolvedName,
    plan: Option<TargetPlan>,
    overlap: OverlapCheck,
}

struct CapturedPart {
    url: WebUrl,
    response: CaptureSuccess,
    workdir: PathBuf,
}

pub(super) fn run(
    matches: &ArgMatches,
    config: &Config,
    targets: &[OsString],
    keep_intake_reason: Option<String>,
    force: Option<super::force::ForceMode>,
) -> i32 {
    match run_collection(matches, config, targets, keep_intake_reason, force) {
        Ok(()) => 0,
        Err(error) => report_error(error),
    }
}

fn run_collection(
    matches: &ArgMatches,
    config: &Config,
    targets: &[OsString],
    keep_intake_reason: Option<String>,
    force: Option<super::force::ForceMode>,
) -> Result<(), CommandError> {
    let listen = if matches.get_flag("listen") {
        Some(
            super::listen::require_validated()
                .map_err(|error| error.into_command_error())?,
        )
    } else {
        None
    };
    let prepared =
        run_preflight(matches, config, targets, keep_intake_reason, force)?;
    if let (Some(command), OverlapCheck::Attach(hit)) =
        (listen.as_ref(), &prepared.overlap)
    {
        let attach = super::attach::attach_for_dedupe_hit(config, hit)?;
        let target = attach.existing_pdf.to_string_lossy().into_owned();
        let source_line = format!(
            "sources: {} URLs (same set as {})",
            prepared.urls.len(),
            attach.existing_pdf.display()
        );
        let mut scratch = ScratchDir::create("collection-listen")?;
        return super::attach::run_attach(
            config,
            command,
            &attach,
            &source_line,
            &target,
            prepared.options.dry_run,
            prepared.options.keep_intake_reason.as_deref(),
            &mut scratch,
        );
    }
    capture_and_install(config, prepared, listen.as_ref())
}

fn run_preflight(
    matches: &ArgMatches,
    config: &Config,
    targets: &[OsString],
    keep_intake_reason: Option<String>,
    force: Option<super::force::ForceMode>,
) -> Result<PreparedCollection, CommandError> {
    let urls = validate_targets(matches, targets)?;
    // Duplicate before parent: check overlap before requiring `-P`, so
    // an already-captured set refuses (exit 1) before a missing parent
    // errors (exit 2). `-L` attach never needs `-P`.
    let pre_listen = matches.get_flag("listen");
    let pre_recorded = sources::collect_recorded_source_urls(config)
        .map_err(|error| CommandError::new(error.message))?;
    // A pre-check refusal (exact or partial) wins over the parent error.
    // `check_collection_overlap` returns `Err` for refusals, `Ok` for
    // clear/attach. Run it first with no planned target.
    let pre_overlap = check_collection_overlap(
        config,
        &pre_recorded,
        &urls,
        None,
        force,
        pre_listen,
    );
    match &pre_overlap {
        Err(_) => return pre_overlap.map(|_| unreachable!()),
        Ok(OverlapCheck::Clear { .. })
            if super::force::is_overwrite(force)
                && !has_exact_queued_match(&pre_recorded, &urls) =>
        {
            // Under `--force=overwrite` only an exact queued set keeps
            // the old replacement permission; anything else is fresh
            // (legacy-only included) and refuses before parent
            // resolution, fetch, or adapter work.
            return Err(super::force::nothing_to_overwrite(
                &targets
                    .iter()
                    .map(|target| target.to_string_lossy().into_owned())
                    .collect::<Vec<_>>()
                    .join(" "),
            ));
        }
        Ok(OverlapCheck::Attach(hit)) => {
            let hit = hit.clone();
            let options = options_without_parent_for_attach(
                matches,
                keep_intake_reason.clone(),
                force,
            )?;
            let output_stem = options
                .output
                .as_deref()
                .and_then(Path::file_stem)
                .and_then(std::ffi::OsStr::to_str);
            let resolved_name = match collection_name::resolve_collection_name(
                options.name.as_deref(),
                output_stem,
                options.title.as_deref(),
                &urls,
            ) {
                Ok(name) => name,
                Err(error) => return Err(error),
            };
            if let Some(warning) = collection_name::part_order_warning(&urls) {
                eprintln!("{warning}");
            }
            return Ok(PreparedCollection {
                urls,
                options,
                resolved_name,
                plan: None,
                overlap: OverlapCheck::Attach(hit),
            });
        }
        Ok(OverlapCheck::Clear { .. }) => {}
    }
    let options =
        options_from_matches(matches, config, keep_intake_reason, force)?;
    let output_stem = options
        .output
        .as_deref()
        .and_then(Path::file_stem)
        .and_then(std::ffi::OsStr::to_str);
    let resolved_name = match collection_name::resolve_collection_name(
        options.name.as_deref(),
        output_stem,
        options.title.as_deref(),
        &urls,
    ) {
        Ok(name) => name,
        Err(error) => return Err(error),
    };

    let recorded = sources::collect_recorded_source_urls(config)
        .map_err(|error| CommandError::new(error.message))?;
    let first_overlap = check_collection_overlap(
        config,
        &recorded,
        &urls,
        None,
        options.force,
        options.listen,
    )?;

    let ref_type = options.ref_type.as_deref().unwrap_or("blogs");
    let plan = match &first_overlap {
        OverlapCheck::Attach(_) => None,
        OverlapCheck::Clear { .. } => Some(plan_collection_target(
            config,
            &resolved_name.name,
            ref_type,
            options.output.as_deref(),
            options.force.is_some(),
            &urls,
            &recorded,
        )?),
    };

    // A lost race between the pre-parent gate and locked planning files
    // nothing over an occupant: recheck before capturing.
    if let Some(plan) = &plan
        && super::force::is_overwrite(force)
        && !plan.target.exists()
    {
        return Err(super::force::nothing_to_overwrite(
            &targets
                .iter()
                .map(|target| target.to_string_lossy().into_owned())
                .collect::<Vec<_>>()
                .join(" "),
        ));
    }

    let final_overlap = if let Some(plan) = &plan {
        check_collection_overlap(
            config,
            &recorded,
            &urls,
            Some(&plan.target),
            options.force,
            options.listen,
        )?
    } else {
        first_overlap
    };

    if let Some(warning) = collection_name::part_order_warning(&urls) {
        eprintln!("{warning}");
    }
    match &final_overlap {
        OverlapCheck::Clear { legacy } => {
            sources::warn_for_legacy_hits(legacy);
        }
        OverlapCheck::Attach(_) => {}
    }

    Ok(PreparedCollection {
        urls,
        options,
        resolved_name,
        plan,
        overlap: final_overlap,
    })
}

fn validate_targets(
    matches: &ArgMatches,
    targets: &[OsString],
) -> Result<Vec<WebUrl>, CommandError> {
    let mut urls = Vec::with_capacity(targets.len());
    for (index, target) in targets.iter().enumerate() {
        let raw = target.to_string_lossy();
        if !super::target::looks_like_url(&raw) {
            return Err(usage_error(
                format!(
                    "TARGET {} is a local file ({}); several targets must all be web article URLs",
                    index + 1,
                    raw,
                ),
                "capture this target on its own",
            ));
        }
        let (url, paper) =
            super::target::resolve_url_syntactic(&raw).map_err(|error| {
                usage_error(
                    format!("URL {} is invalid: {}", index + 1, error.message),
                    "capture each target as a valid http(s) URL",
                )
            })?;
        if paper.is_some() {
            return Err(usage_error(
                format!(
                    "URL {} is an arXiv paper; merging supports web articles only",
                    index + 1,
                ),
                "capture this paper on its own",
            ));
        }
        urls.push(url);
    }

    if matches.get_one::<OsString>("html").is_some() {
        return Err(usage_error(
            "--html replays one saved page; several URLs are captured live",
            "capture each saved page on its own with --html",
        ));
    }

    let mut first_for_key = BTreeMap::<&str, usize>::new();
    for (index, url) in urls.iter().enumerate() {
        if let Some(first) = first_for_key.get(url.dedupe_key.as_str()) {
            return Err(usage_error(
                format!(
                    "URL {} and URL {} are the same page",
                    first,
                    index + 1,
                ),
                "remove one copy of the repeated URL",
            ));
        }
        first_for_key.insert(url.dedupe_key.as_str(), index + 1);
    }
    if urls.len() > MAX_COLLECTION_URLS {
        return Err(usage_error(
            format!(
                "too many URLs ({}); one reference takes at most {}",
                urls.len(),
                MAX_COLLECTION_URLS,
            ),
            "split this reading packet into smaller references",
        ));
    }
    Ok(urls)
}

fn options_without_parent_for_attach(
    matches: &ArgMatches,
    keep_intake_reason: Option<String>,
    force: Option<super::force::ForceMode>,
) -> Result<CreateOptions, CommandError> {
    if let Some(name) = matches.get_one::<String>("name") {
        clip_url::validate_name(name)?;
    }
    let ref_type = matches.get_one::<String>("ref-type").cloned();
    if let Some(ref_type) = &ref_type {
        stamp::validate_ref_type(ref_type)?;
    }
    let published = matches.get_one::<String>("published").cloned();
    if let Some(date) = &published {
        clip_url::validate_published(date)?;
    }
    Ok(CreateOptions {
        audio: matches.get_one::<OsString>("audio").map(PathBuf::from),
        author: matches.get_one::<String>("author").cloned(),
        closed: false,
        dry_run: matches.get_flag("dry-run"),
        force,
        html: matches.get_one::<OsString>("html").cloned(),
        include_id: matches.get_flag("include-id"),
        keep_intake_reason,
        listen: matches.get_flag("listen"),
        name: matches.get_one::<String>("name").cloned(),
        no_audio: matches.get_flag("no-audio"),
        output: matches.get_one::<OsString>("output").map(PathBuf::from),
        plain: false,
        parent: String::new(),
        parent_default_reason: None,
        resolved_parent: None,
        published,
        sections: Vec::new(),
        ref_type,
        status: matches
            .get_one::<String>("status")
            .expect("defaulted by clap")
            .clone(),
        title: matches.get_one::<String>("title").cloned(),
        reread: None,
    })
}

fn options_from_matches(
    matches: &ArgMatches,
    config: &Config,
    keep_intake_reason: Option<String>,
    force: Option<super::force::ForceMode>,
) -> Result<CreateOptions, CommandError> {
    if let Some(name) = matches.get_one::<String>("name") {
        clip_url::validate_name(name)?;
    }
    let ref_type = matches.get_one::<String>("ref-type").cloned();
    if let Some(ref_type) = &ref_type {
        stamp::validate_ref_type(ref_type)?;
    }
    let published = matches.get_one::<String>("published").cloned();
    if let Some(date) = &published {
        clip_url::validate_published(date)?;
    }

    let Some(parent_input) = matches.get_one::<String>("parent").cloned()
    else {
        return Err(CommandError::new(
            "--parent is required\nhint: pass -P <note>, for example `-P sase`; list area and project notes with `bob capture-targets`",
        )
        .with_exit_code(2));
    };
    let resolved_parent = crate::native::parent_notes::resolve_parent(
        &config.bob_dir,
        &parent_input,
    )
    .map_err(|error| CommandError::new(error.message()))?;

    Ok(CreateOptions {
        audio: matches.get_one::<OsString>("audio").map(PathBuf::from),
        author: matches.get_one::<String>("author").cloned(),
        closed: false,
        dry_run: matches.get_flag("dry-run"),
        force,
        html: matches.get_one::<OsString>("html").cloned(),
        include_id: matches.get_flag("include-id"),
        keep_intake_reason,
        listen: matches.get_flag("listen"),
        name: matches.get_one::<String>("name").cloned(),
        no_audio: matches.get_flag("no-audio"),
        output: matches.get_one::<OsString>("output").map(PathBuf::from),
        plain: false,
        parent: resolved_parent.route.clone(),
        parent_default_reason: None,
        resolved_parent: Some(resolved_parent),
        published,
        sections: Vec::new(),
        ref_type,
        status: matches
            .get_one::<String>("status")
            .expect("defaulted by clap")
            .clone(),
        title: matches.get_one::<String>("title").cloned(),
        reread: None,
    })
}

fn plan_collection_target(
    config: &Config,
    name: &str,
    ref_type: &str,
    output: Option<&Path>,
    force: bool,
    urls: &[WebUrl],
    recorded: &[RecordedSource],
) -> Result<TargetPlan, CommandError> {
    if let Some(output) = output {
        let plan = stamp::plan_exact_output(config, output, force)?;
        stamp::ensure_reference_id_available(config, &plan.stem)?;
        return Ok(plan);
    }
    let identity = DefaultTargetIdentity::Url {
        keys: urls.iter().map(|url| url.dedupe_key.clone()).collect(),
        recorded,
    };
    stamp::plan_default_target(
        config,
        std::ffi::OsStr::new(name),
        ref_type,
        force,
        &identity,
    )
}

fn capture_and_install(
    config: &Config,
    prepared: PreparedCollection,
    listen: Option<&super::listen::ListenCommand>,
) -> Result<(), CommandError> {
    let PreparedCollection {
        urls,
        options,
        resolved_name,
        plan,
        overlap,
    } = prepared;
    let plan = plan.ok_or_else(|| {
        CommandError::new("internal: collection target was not planned")
    })?;
    let total = urls.len();
    let mut scratch = ScratchDir::create("collection")?;
    classify_parts(&urls, &scratch)?;

    let client = ClipAdapterClient::resolve()?;
    let captured = super::current_local_date();
    let parts = capture_parts(
        &client,
        &urls,
        scratch.path(),
        options.dry_run,
        &captured,
    )?;
    let derived = derive_collection_meta(&parts, &options, &resolved_name.name);
    let styler = Styler::detect();
    if options.dry_run {
        let audio = match options.audio.as_deref() {
            Some(path) => super::companion::plan_explicit_audio(
                config,
                &plan,
                options.force.is_some(),
                path,
            )?,
            None => None,
        };
        let listen_flow = match listen {
            Some(_) => Some(super::listen::plan_listen_flow(
                &plan, &scratch, &plan.stem,
            )?),
            None => None,
        };
        if let (Some(command), Some(flow)) = (listen, &listen_flow) {
            let staged_pdf = scratch.path().join("collection.pdf");
            println!(
                "{}",
                super::listen::would_run_line(
                    command,
                    &super::listen::ListenValues {
                        target: staged_pdf.to_string_lossy().into_owned(),
                        pdf: staged_pdf,
                        audio: flow.scratch_audio.clone(),
                        title: derived.title.clone(),
                    },
                )
            );
        }
        print_dry_run(
            &styler,
            config,
            &urls,
            &parts,
            &plan,
            &options,
            &resolved_name,
            &derived,
            &overlap,
            audio.as_ref(),
            listen_flow.as_ref(),
        )?;
        return Ok(());
    }

    if std::io::stderr().is_terminal() {
        eprintln!("composing {total} articles…");
    }
    let compose_dir = scratch.path().join("compose");
    fs::create_dir_all(&compose_dir).map_err(|error| {
        CommandError::new(format!(
            "create compose workdir {}: {error}",
            compose_dir.display()
        ))
    })?;
    let composed_pdf = scratch.path().join("collection.pdf");
    let compose_request = ComposeRequest::new(
        parts
            .iter()
            .map(|part| ComposePart {
                article_json: article_json_path(part),
            })
            .collect(),
        CollectionMeta {
            title: derived.title.clone(),
            author: derived.author.clone(),
            published: derived.published.clone(),
            captured: captured.clone(),
        },
        compose_dir.to_string_lossy().into_owned(),
        composed_pdf.to_string_lossy().into_owned(),
    );
    let compose = client.compose(&compose_request).map_err(|failure| {
        with_nothing_written(partless_adapter_error(failure))
    })?;

    // Plan audio before the guarded install, and before the listener runs.
    let listen_flow = match listen {
        Some(_) => Some(super::listen::plan_listen_flow(
            &plan, &scratch, &plan.stem,
        )?),
        None => None,
    };

    let recorded = sources::collect_recorded_source_urls(config)
        .map_err(|error| CommandError::new(error.message))?;
    check_collection_overlap(
        config,
        &recorded,
        &urls,
        Some(&plan.target),
        options.force,
        options.listen,
    )?;
    stamp::refuse_target_collisions(
        &plan.target,
        &plan.sidecar,
        &plan.workflow,
        options.force.is_some(),
    )
    .map_err(with_nothing_written)?;

    let marker =
        collection_marker(&options, &plan, &urls, &derived, &captured)?;
    let info = PdfInfo {
        title: Some(derived.title.clone()),
        author: derived.author.clone(),
    };
    let audio = match &options.audio {
        Some(path) => super::companion::plan_explicit_audio(
            config,
            &plan,
            options.force.is_some(),
            path,
        )?,
        None => None,
    };
    let staged_stamp = if listen.is_some() {
        Some(super::pdf_target::stamp_pdf_to_scratch(
            &composed_pdf,
            &scratch,
            &marker,
            &info,
        )?)
    } else {
        None
    };
    let created = super::companion::copy_audio_for_install(audio.as_ref())?;
    let mut listened_audio_created: Option<Option<PathBuf>> = None;
    if let (Some(command), Some(flow)) = (listen, &listen_flow) {
        let values = super::listen::ListenValues {
            target: composed_pdf.to_string_lossy().into_owned(),
            pdf: composed_pdf.clone(),
            audio: flow.scratch_audio.clone(),
            title: derived.title.clone(),
        };
        super::listen::run_listen(command, &values)
            .map_err(|error| error.into_command_error())?;
        // Revalidate identity and the target after the potentially long render.
        let latest = sources::collect_recorded_source_urls(config)
            .map_err(|error| CommandError::new(error.message))?;
        if let Err(error) = check_collection_overlap(
            config,
            &latest,
            &urls,
            Some(&plan.target),
            options.force,
            false,
        ) {
            return Err(super::listen::post_listen_error(
                &mut scratch,
                &flow.scratch_audio,
                error.to_string(),
                bind_listen_hint(
                    &urls,
                    &options,
                    &resolved_name,
                    &flow.scratch_audio,
                ),
            ));
        }
        if let Err(error) = stamp::refuse_target_collisions(
            &plan.target,
            &plan.sidecar,
            &plan.workflow,
            options.force.is_some(),
        ) {
            return Err(super::listen::post_listen_error(
                &mut scratch,
                &flow.scratch_audio,
                error.to_string(),
                bind_listen_hint(
                    &urls,
                    &options,
                    &resolved_name,
                    &flow.scratch_audio,
                ),
            ));
        }
        listened_audio_created = Some(
            super::listen::install_listen_audio(flow, options.force.is_some())
                .map_err(|error| {
                    super::listen::post_listen_error(
                        &mut scratch,
                        &flow.scratch_audio,
                        error.to_string(),
                        bind_listen_hint(
                            &urls,
                            &options,
                            &resolved_name,
                            &flow.scratch_audio,
                        ),
                    )
                })?,
        );
    }
    let install_result = if let Some((stamped, page_count)) = &staged_stamp {
        super::atomic_copy(stamped, &plan.target).map(|()| *page_count)
    } else {
        super::stamp_and_install(&composed_pdf, &plan.target, &marker, &info)
    };
    let page_count = match install_result {
        Ok(page_count) => page_count,
        Err(error) => {
            super::companion::cleanup_audio_on_failure(created.as_ref());
            if let Some(Some(path)) = listened_audio_created.as_ref() {
                let _ = fs::remove_file(path);
            }
            if let (Some(flow), Some(_)) = (&listen_flow, listen) {
                return Err(super::listen::post_listen_error(
                    &mut scratch,
                    &flow.scratch_audio,
                    error.to_string(),
                    bind_listen_hint(
                        &urls,
                        &options,
                        &resolved_name,
                        &flow.scratch_audio,
                    ),
                ));
            }
            return Err(with_nothing_written(error));
        }
    };
    print_success(
        &styler,
        &urls,
        &parts,
        config,
        &plan,
        &options,
        &resolved_name,
        &derived,
        &compose,
        audio.as_ref(),
        &captured,
        page_count,
        listen_flow.as_ref(),
    )?;
    Ok(())
}

fn classify_parts(
    urls: &[WebUrl],
    scratch: &ScratchDir,
) -> Result<(), CommandError> {
    let total = urls.len();
    let progress = |line: &str| {
        if std::io::stderr().is_terminal() {
            eprintln!("{line}");
        }
    };
    for (index, url) in urls.iter().enumerate() {
        let part = index + 1;
        match super::target::fetch_and_route(
            url.clone(),
            scratch,
            Some(&progress),
        ) {
            Ok(super::target::CreateSource::WebArticle { .. }) => {}
            Ok(super::target::CreateSource::PdfUrl { url, .. }) => {
                return Err(pdf_part_error(part, total, &url));
            }
            Ok(_) => {
                return Err(with_nothing_written(CommandError::new(format!(
                    "part {part}/{total} classified as an unsupported target"
                ))));
            }
            Err(error) => {
                return Err(prefix_part_error(part, total, error));
            }
        }
    }
    Ok(())
}

fn capture_parts(
    client: &ClipAdapterClient,
    urls: &[WebUrl],
    scratch: &Path,
    dry_run: bool,
    captured: &str,
) -> Result<Vec<CapturedPart>, CommandError> {
    let total = urls.len();
    let mut parts = Vec::with_capacity(total);
    for (index, url) in urls.iter().enumerate() {
        let part = index + 1;
        let workdir = scratch.join(format!("part-{part}"));
        fs::create_dir_all(&workdir).map_err(|error| {
            CommandError::new(format!(
                "create part workdir {}: {error}",
                workdir.display()
            ))
        })?;
        if std::io::stderr().is_terminal() {
            eprintln!("capturing {part}/{total} {}…", url.host);
        }
        let request = CaptureRequest::for_collection_part(
            url.cleaned.clone(),
            workdir.to_string_lossy().into_owned(),
            dry_run,
            captured.to_string(),
        );
        let response = match client.capture(&request) {
            Ok(response) => response,
            Err(failure) => {
                return Err(capture_part_error(part, total, url, failure));
            }
        };
        if response.kind.as_deref() == Some("pdf") {
            return Err(pdf_part_error(part, total, url));
        }
        parts.push(CapturedPart {
            url: url.clone(),
            response,
            workdir,
        });
    }
    Ok(parts)
}

struct DerivedMeta {
    title: String,
    title_phrase: &'static str,
    author: Option<String>,
    published: Option<String>,
    published_from_flag: bool,
}

fn derive_collection_meta(
    parts: &[CapturedPart],
    options: &CreateOptions,
    name: &str,
) -> DerivedMeta {
    let part_titles: Vec<String> = parts
        .iter()
        .map(|part| part_title(&part.response))
        .collect();
    let part_authors: Vec<Option<String>> = parts
        .iter()
        .map(|part| {
            part.response
                .author
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_string)
        })
        .collect();
    let part_dates: Vec<Option<String>> = parts
        .iter()
        .map(|part| part.response.published.clone())
        .collect();
    let title = collection_name::collection_title(
        &part_titles,
        options.title.as_deref(),
        name,
    );
    let title_phrase = title_source_phrase(
        options.title.as_deref(),
        &part_titles,
        name,
        &title,
    );
    DerivedMeta {
        title,
        title_phrase,
        author: collection_name::collection_author(
            &part_authors,
            options.author.as_deref(),
        ),
        published: collection_name::collection_published(
            &part_dates,
            options.published.as_deref(),
        ),
        published_from_flag: options
            .published
            .as_deref()
            .map(str::trim)
            .is_some_and(|value| !value.is_empty()),
    }
}

fn title_source_phrase(
    title_flag: Option<&str>,
    part_titles: &[String],
    name: &str,
    title: &str,
) -> &'static str {
    if title_flag
        .map(str::trim)
        .is_some_and(|value| !value.is_empty())
    {
        return "from -T";
    }
    let derived = collection_name::collection_title(part_titles, None, name);
    if derived == title && derived != clip_url::humanize_stem(name) {
        "shared by every part title; -T overrides"
    } else {
        "from the reference name; -T overrides"
    }
}

fn part_title(response: &CaptureSuccess) -> String {
    response
        .title
        .as_deref()
        .map(str::trim)
        .filter(|title| !title.is_empty())
        .unwrap_or("untitled")
        .to_string()
}

fn article_json_path(part: &CapturedPart) -> String {
    part.response.article_json.clone().unwrap_or_else(|| {
        part.workdir
            .join("article.json")
            .to_string_lossy()
            .into_owned()
    })
}

fn collection_marker(
    options: &CreateOptions,
    plan: &TargetPlan,
    urls: &[WebUrl],
    derived: &DerivedMeta,
    captured: &str,
) -> Result<String, CommandError> {
    let source_url = MarkerValue::List(
        urls.iter()
            .map(|url| MarkerValue::String(url.cleaned.clone()))
            .collect(),
    );
    super::compose_marker(
        &options.status,
        &options.parent,
        &derived.title,
        Some(plan.stem.as_str()),
        &[
            ("source_url", source_url),
            (
                "author",
                MarkerValue::String(derived.author.clone().unwrap_or_default()),
            ),
            (
                "published",
                MarkerValue::String(
                    derived.published.clone().unwrap_or_default(),
                ),
            ),
            ("captured", MarkerValue::String(captured.to_string())),
        ],
    )
}

fn pdf_part_error(part: usize, total: usize, url: &WebUrl) -> CommandError {
    with_nothing_written(CommandError::new(format!(
        "part {part}/{total} is a PDF ({}); merging supports web articles only\nhint: capture this PDF on its own",
        url.cleaned
    )))
}

fn prefix_part_error(
    part: usize,
    total: usize,
    error: CommandError,
) -> CommandError {
    let exit_code = error.exit_code;
    let prefixed =
        if let Some((first, rest)) = error.message.split_once("\nhint: ") {
            format!("part {part}/{total}: {first}\nhint: {rest}")
        } else {
            format!("part {part}/{total}: {}", error.message)
        };
    let mut prefixed = CommandError::new(prefixed);
    if let Some(code) = exit_code {
        prefixed = prefixed.with_exit_code(code);
    }
    with_nothing_written(prefixed)
}

fn capture_part_error(
    part: usize,
    total: usize,
    url: &WebUrl,
    failure: super::clip_adapter::AdapterFailure,
) -> CommandError {
    let location = without_scheme(&url.cleaned);
    let mut message =
        format!("part {part}/{total} ({location}): {}", failure.message);
    if let Some(hint) = failure.hint {
        message.push_str("\nhint: ");
        message.push_str(&hint);
    }
    with_nothing_written(CommandError::new(message))
}

fn partless_adapter_error(
    failure: super::clip_adapter::AdapterFailure,
) -> CommandError {
    failure.into()
}

fn with_nothing_written(mut error: CommandError) -> CommandError {
    if !error.message.contains("nothing was written") {
        error.message.push_str("\nnothing was written");
    }
    error
}

fn bind_listen_hint(
    urls: &[WebUrl],
    options: &CreateOptions,
    resolved_name: &collection_name::ResolvedName,
    kept_audio: &Path,
) -> String {
    let mut command = String::from("bind it with bob ref create");
    for url in urls {
        command.push(' ');
        command.push_str(&super::listen::shell_quote(&url.cleaned));
    }
    if let Some(output) = &options.output {
        command.push_str(" -o ");
        command
            .push_str(&super::listen::shell_quote(&output.to_string_lossy()));
    } else {
        command.push_str(" -N ");
        command.push_str(&super::listen::shell_quote(&resolved_name.name));
    }
    command.push_str(" -P ");
    command.push_str(&super::listen::shell_quote(&options.parent));
    command.push_str(" --audio ");
    command
        .push_str(&super::listen::shell_quote(&kept_audio.to_string_lossy()));
    command
}

#[allow(clippy::too_many_arguments)]
fn print_success(
    styler: &Styler,
    urls: &[WebUrl],
    parts: &[CapturedPart],
    config: &Config,
    plan: &TargetPlan,
    options: &CreateOptions,
    resolved_name: &collection_name::ResolvedName,
    derived: &DerivedMeta,
    compose: &ComposeSuccess,
    audio: Option<&super::companion::AudioCopyPlan>,
    captured: &str,
    page_count: usize,
    listen_flow: Option<&super::listen::ListenFlow>,
) -> Result<(), CommandError> {
    let installed_bytes = fs::metadata(&plan.target)
        .map(|metadata| metadata.len())
        .unwrap_or(0);
    println!(
        "{} created Highlights-ready PDF from {} web articles",
        styler.success_prefix(false),
        urls.len()
    );
    if let Some(line) = plan.renamed_line() {
        println!("{line}");
    }
    println!(
        "name: {} ({})",
        resolved_name.name,
        resolved_name.source.report_phrase()
    );
    println!("title: {} ({})", derived.title, derived.title_phrase);
    println!("parts:");
    print_part_rows(styler, parts);
    println!("pdf: {}", plan.target.display());
    if let Some(audio) = audio {
        println!("audio: {} (from {})", audio.dest.display(), audio.origin);
    } else if let Some(flow) = listen_flow {
        println!("audio: {} (from --listen)", flow.dest.display());
    }
    if let Some(author) = &derived.author {
        println!("author: {author}");
    }
    if let Some(published) = published_line(derived) {
        println!("{published}");
    }
    println!("captured: {captured}");
    println!("status: {}", options.status);
    println!("parent: {}", options.parent);
    println!(
        "pages: {page_count} · images: {}/{} · size: {}",
        compose.images.kept,
        compose.images.total,
        format_bytes(installed_bytes),
    );
    super::create::finish_create_intake(
        config,
        plan,
        options.keep_intake_reason.as_deref(),
    )?;
    print_part_warnings(styler, parts);
    print_warnings(styler, &compose.warnings);
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn print_dry_run(
    styler: &Styler,
    config: &Config,
    urls: &[WebUrl],
    parts: &[CapturedPart],
    plan: &TargetPlan,
    options: &CreateOptions,
    resolved_name: &collection_name::ResolvedName,
    derived: &DerivedMeta,
    overlap: &OverlapCheck,
    audio: Option<&super::companion::AudioCopyPlan>,
    listen_flow: Option<&super::listen::ListenFlow>,
) -> Result<(), CommandError> {
    let captured = super::current_local_date();
    println!(
        "{} would create Highlights-ready PDF from {} web articles",
        styler.success_prefix(true),
        urls.len()
    );
    if let Some(line) = plan.renamed_line() {
        println!("{line}");
    }
    println!(
        "name: {} ({})",
        resolved_name.name,
        resolved_name.source.report_phrase()
    );
    println!("title: {} ({})", derived.title, derived.title_phrase);
    println!("parts:");
    print_part_rows(styler, parts);
    println!("pdf: {}", plan.target.display());
    if let Some(audio) = audio {
        println!("audio: {} (from {})", audio.dest.display(), audio.origin);
    } else if let Some(flow) = listen_flow {
        println!("audio: {} (from --listen)", flow.dest.display());
    }
    match &plan.workflow {
        TargetWorkflow::Intake {
            library_destination,
        } => println!("library_destination: {}", library_destination.display()),
        _ => println!("library_destination: n/a (external target)"),
    }
    if let Some(author) = &derived.author {
        println!("author: {author}");
    }
    if let Some(published) = published_line(derived) {
        println!("{published}");
    }
    println!("captured: {captured}");
    println!("status: {}", options.status);
    println!("parent: {}", options.parent);
    if let OverlapCheck::Clear { legacy } = overlap {
        for hit in legacy {
            println!(
                "legacy: {} (superseded by this capture)",
                path_for_report(config, &hit.path)
            );
        }
    }
    let marker = collection_marker(options, plan, urls, derived, &captured)?;
    println!("marker:");
    print!("{marker}");
    super::create::print_create_intake_preview(
        config,
        plan,
        options.keep_intake_reason.as_deref(),
    );
    println!("writes: none");
    print_part_warnings(styler, parts);
    Ok(())
}

fn print_part_rows(styler: &Styler, parts: &[CapturedPart]) {
    let total = parts.len();
    for (index, part) in parts.iter().enumerate() {
        let n = index + 1;
        let title = part_title(&part.response);
        let words = format_count(part.response.word_count);
        let fidelity = part
            .response
            .fidelity
            .status
            .as_deref()
            .unwrap_or("unknown");
        let meta =
            format!("{} · {words} words · fidelity {fidelity}", part.url.host);
        println!("  {n}/{total}  {title}   {}", styler.dim(&meta));
    }
}

fn print_part_warnings(styler: &Styler, parts: &[CapturedPart]) {
    let total = parts.len();
    for (index, part) in parts.iter().enumerate() {
        let n = index + 1;
        for warning in &part.response.warnings {
            println!(
                "{}: part {n}/{total}: {warning}",
                styler.warning_prefix()
            );
        }
    }
}

fn print_warnings(styler: &Styler, warnings: &[String]) {
    for warning in warnings {
        println!("{}: {warning}", styler.warning_prefix());
    }
}

fn published_line(derived: &DerivedMeta) -> Option<String> {
    let date = derived.published.as_ref()?;
    if derived.published_from_flag {
        Some(format!("published: {date}"))
    } else {
        Some(format!("published: {date} (first part)"))
    }
}

fn format_count(value: u64) -> String {
    let digits = value.to_string();
    let mut grouped = String::new();
    for (index, ch) in digits.chars().rev().enumerate() {
        if index > 0 && index % 3 == 0 {
            grouped.push(',');
        }
        grouped.push(ch);
    }
    grouped.chars().rev().collect()
}

fn check_collection_overlap(
    config: &Config,
    recorded: &[RecordedSource],
    urls: &[WebUrl],
    planned_target: Option<&Path>,
    force: Option<super::force::ForceMode>,
    listen: bool,
) -> Result<OverlapCheck, CommandError> {
    let force_flag = force.is_some();
    let wanted: BTreeSet<String> =
        urls.iter().map(|url| url.dedupe_key.clone()).collect();
    let groups = group_recorded_sources(recorded);
    let refusing: Vec<&SourceGroup> = groups
        .iter()
        .filter(|group| group.refuses() && !group.keys.is_disjoint(&wanted))
        .collect();

    if refusing.is_empty() {
        let mut legacy_by_path = BTreeMap::new();
        for group in groups.iter().filter(|group| {
            group.is_legacy() && !group.keys.is_disjoint(&wanted)
        }) {
            legacy_by_path
                .entry(group.path.clone())
                .or_insert_with(|| group.first.clone());
        }
        return Ok(OverlapCheck::Clear {
            legacy: legacy_by_path.into_values().collect(),
        });
    }

    let exact = refusing.len() == 1 && refusing[0].keys == wanted;
    if exact {
        let hit = &refusing[0].first;
        if listen {
            return Ok(OverlapCheck::Attach(hit.clone()));
        }
        if hit.is_ref_note {
            if super::force::is_overwrite(force) {
                return Err(super::force::overwrite_filed_unavailable());
            }
            let copy = super::duplicate::copy_from_occupant(config, &hit.path);
            let title = copy.stem.clone();
            let refusal = super::duplicate::AlreadyCaptured::new(
                title,
                vec![copy],
                vec![super::duplicate::Remedy::new(
                    "-L",
                    "add --listen to narrate it and attach the episode to the existing capture",
                )],
            );
            let styler = crate::native::style::Styler::detect();
            return Err(CommandError::new(refusal.render(&styler)));
        }
        let same_target = planned_target.is_some_and(|target| {
            stamp::normalize_lexically(target)
                == stamp::normalize_lexically(&hit.path)
        });
        if force_flag && (same_target || planned_target.is_none()) {
            return Ok(OverlapCheck::Clear { legacy: Vec::new() });
        }
        let copy = super::duplicate::copy_from_occupant(config, &hit.path);
        let title = copy.stem.clone();
        let refusal = super::duplicate::AlreadyCaptured::new(
            title,
            vec![copy],
            vec![
                super::duplicate::Remedy::new("-f", "replace the queued PDF"),
                super::duplicate::Remedy::new(
                    "-L",
                    "add --listen to narrate it and attach the episode to the existing capture",
                ),
            ],
        );
        let styler = crate::native::style::Styler::detect();
        return Err(CommandError::new(refusal.render(&styler)));
    }

    let overlaps: Vec<&SourceGroup> = groups
        .iter()
        .filter(|group| !group.keys.is_disjoint(&wanted))
        .collect();
    Err(partial_overlap_error(config, urls, &overlaps))
}

/// Under `--force=overwrite`, only an exact queued URL-set match keeps
/// the old replacement permission: one recorded group whose keys equal
/// the wanted set and whose PDFs are still queued (not filed notes).
/// Legacy-only groups count as fresh: they name no PDF to overwrite.
fn has_exact_queued_match(
    recorded: &[RecordedSource],
    urls: &[WebUrl],
) -> bool {
    let wanted: BTreeSet<String> =
        urls.iter().map(|url| url.dedupe_key.clone()).collect();
    group_recorded_sources(recorded)
        .iter()
        .any(|group| group.keys == wanted && !group.is_ref_note)
}

fn group_recorded_sources(recorded: &[RecordedSource]) -> Vec<SourceGroup> {
    let mut grouped = BTreeMap::<PathBuf, SourceGroup>::new();
    for source in recorded {
        let group =
            grouped
                .entry(source.path.clone())
                .or_insert_with(|| SourceGroup {
                    path: source.path.clone(),
                    keys: BTreeSet::new(),
                    is_ref_note: source.is_ref_note,
                    source_pdf: source.source_pdf.clone(),
                    first: source.clone(),
                });
        group.keys.insert(source.dedupe_key.clone());
    }
    grouped.into_values().collect()
}

fn partial_overlap_error(
    config: &Config,
    urls: &[WebUrl],
    groups: &[&SourceGroup],
) -> CommandError {
    let mut rows = BTreeMap::<usize, Vec<String>>::new();
    for (index, url) in urls.iter().enumerate() {
        for group in groups
            .iter()
            .filter(|group| group.keys.contains(&url.dedupe_key))
        {
            let descriptor = if group.is_legacy() {
                format!(
                    "legacy note without a PDF {}",
                    path_for_report(config, &group.path)
                )
            } else if group.is_ref_note {
                format!("captured as {}", path_for_report(config, &group.path))
            } else {
                format!("queued in {}", path_for_report(config, &group.path))
            };
            rows.entry(index + 1).or_default().push(descriptor);
        }
    }
    let count = rows.len();
    let verb = if count == 1 { "is" } else { "are" };
    let mut lines = vec![format!(
        "{count} of {} URLs {verb} already in the library",
        urls.len()
    )];
    for (index, descriptors) in rows {
        let descriptors = descriptors.join("; ");
        lines.push(format!(
            "  {index}  {}   {descriptors}",
            without_scheme(&urls[index - 1].cleaned),
        ));
    }
    lines.push(
        "hint: a URL belongs to one reference; remove those references first, or capture the other URLs on their own".into(),
    );
    CommandError::new(lines.join("\n"))
}

fn path_for_report(config: &Config, path: &Path) -> String {
    path.strip_prefix(&config.bob_dir)
        .unwrap_or(path)
        .display()
        .to_string()
}

fn without_scheme(value: &str) -> String {
    value
        .find("://")
        .map(|index| value[index + 3..].to_string())
        .unwrap_or_else(|| value.to_string())
}

fn usage_error(
    message: impl Into<String>,
    hint: impl Into<String>,
) -> CommandError {
    CommandError::new(format!("{}\nhint: {}", message.into(), hint.into()))
        .with_exit_code(2)
}

fn report_error(error: CommandError) -> i32 {
    let styler = Styler::detect();
    eprintln!("{}", style_error_message(&error.message, &styler));
    error.exit_code.unwrap_or(1)
}

fn style_error_message(message: &str, styler: &Styler) -> String {
    // Shared printer keeps every detail row and every `hint:` row.
    // Collection messages never carry the command prefix, so strip it
    // first when present to avoid doubling it.
    let command_prefix = format!("{CREATE_COMMAND_NAME}: error: ");
    let body = message
        .strip_prefix(&command_prefix)
        .or_else(|| {
            message.strip_prefix(&format!("{CREATE_COMMAND_NAME}: error:"))
        })
        .unwrap_or(message);
    super::duplicate::format_error(CREATE_COMMAND_NAME, body, styler)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config() -> Config {
        let bob_dir = PathBuf::from("/vault");
        Config {
            lib_dir: bob_dir.join("lib"),
            ref_dir: bob_dir.join("ref"),
            xlib_dir: bob_dir.join("xlib"),
            bob_dir,
        }
    }

    fn url(raw: &str) -> WebUrl {
        clip_url::validate_and_clean(raw).expect("valid test URL")
    }

    fn source(raw: &str, path: &str, is_ref_note: bool) -> RecordedSource {
        RecordedSource {
            dedupe_key: url(raw).dedupe_key,
            path: PathBuf::from(path),
            is_ref_note,
            source_pdf: is_ref_note.then(|| "lib/blogs/one.pdf".into()),
            has_audio: false,
        }
    }

    #[test]
    fn collection_multiline_errors_style_labels_and_indexes_only() {
        let no_name = "bob ref create: error: these 2 URLs need a reference name\n  1  example.com/a\n  2  example.com/b\nhint: pass -N <name>";
        assert_eq!(
            style_error_message(no_name, &Styler::plain()),
            no_name,
            "plain errors stay byte-identical"
        );
        let partial = "2 of 3 URLs are already in the library\n  1  example.com/a   captured as ref/blogs/a.md\n  3  example.com/c   queued in xlib/blogs/c.pdf\nhint: a URL belongs to one reference";
        assert_eq!(
            style_error_message(partial, &Styler::plain()),
            format!("bob ref create: error: {partial}"),
            "plain partial-overlap output keeps its existing text"
        );
        let styled = style_error_message(partial, &Styler::colored());
        assert!(styled.contains("\u{1b}[31;1merror\u{1b}[0m"), "{styled:?}");
        assert!(
            styled.contains("\u{1b}[2m1\u{1b}[0m  example.com/a"),
            "{styled:?}"
        );
        assert!(
            styled.contains("\u{1b}[2mhint:\u{1b}[0m a URL belongs"),
            "{styled:?}"
        );
    }

    #[test]
    fn partial_pdf_backed_and_queued_overlap_names_every_part_and_ignores_force(
    ) {
        let config = config();
        let urls = [
            url("https://example.com/series-part-1"),
            url("https://example.com/series-part-2"),
        ];
        let recorded = [
            source(
                "https://example.com/series-part-1",
                "/vault/ref/blogs/part_one.md",
                true,
            ),
            source(
                "https://example.com/series-part-2",
                "/vault/xlib/blogs/part_two.pdf",
                false,
            ),
        ];
        let error = check_collection_overlap(
            &config,
            &recorded,
            &urls,
            None,
            Some(super::super::force::ForceMode::Reread),
            false,
        )
        .expect_err("partial overlap refuses even with --force");
        assert!(error
            .message
            .contains("2 of 2 URLs are already in the library"));
        assert!(error.message.contains("ref/blogs/part_one.md"));
        assert!(error.message.contains("xlib/blogs/part_two.pdf"));
        assert!(error.message.contains("1  example.com/series-part-1"));
        assert!(error.message.contains("2  example.com/series-part-2"));
    }

    #[test]
    fn exact_pdf_backed_collection_refuses_or_returns_attach_for_listen() {
        let config = config();
        let urls = [
            url("https://example.com/series-part-1"),
            url("https://example.com/series-part-2"),
        ];
        // Recorded order does not matter for collection identity.
        let recorded = [
            source(
                "https://example.com/series-part-2",
                "/vault/ref/blogs/series.md",
                true,
            ),
            source(
                "https://example.com/series-part-1",
                "/vault/ref/blogs/series.md",
                true,
            ),
        ];
        let error = check_collection_overlap(
            &config, &recorded, &urls, None, None, false,
        )
        .expect_err("same existing collection refuses without listen");
        assert!(error.message.contains("already captured"));
        assert!(error.message.contains("ref/blogs/series.md"));
        assert!(error.message.contains("-L"));

        let attach = check_collection_overlap(
            &config, &recorded, &urls, None, None, true,
        )
        .expect("same set with listen is an attach candidate");
        assert!(matches!(attach, OverlapCheck::Attach(_)));
    }

    #[test]
    fn exact_queued_collection_requires_force_for_the_planned_target() {
        let config = config();
        let urls = [
            url("https://example.com/series-part-1"),
            url("https://example.com/series-part-2"),
        ];
        let target = PathBuf::from("/vault/xlib/blogs/series.pdf");
        let recorded = [
            source(
                "https://example.com/series-part-1",
                "/vault/xlib/blogs/series.pdf",
                false,
            ),
            source(
                "https://example.com/series-part-2",
                "/vault/xlib/blogs/series.pdf",
                false,
            ),
        ];
        let error = check_collection_overlap(
            &config,
            &recorded,
            &urls,
            Some(&target),
            None,
            false,
        )
        .expect_err("queued exact set refuses without force");
        assert!(error.message.contains("already queued"));
        assert!(error.message.contains("xlib/blogs/series.pdf"));

        let allowed = check_collection_overlap(
            &config,
            &recorded,
            &urls,
            Some(&target),
            Some(super::super::force::ForceMode::Reread),
            false,
        )
        .expect("force can overwrite the exact queued target");
        assert!(matches!(allowed, OverlapCheck::Clear { .. }));
    }

    #[test]
    fn encoded_twin_spelling_overlaps_the_recorded_capture() {
        // bob-cli-7u: a collection URL spelled with `%40` overlaps the
        // recorded capture spelled with `@`, because the shared dedupe
        // key normalizes the two spellings to one identity.
        let config = config();
        let urls = [url(
            "https://medium.com/%40edominguez.se/piece-163b85aa0935",
        )];
        let recorded = [source(
            "https://medium.com/@edominguez.se/piece-163b85aa0935",
            "/vault/ref/blogs/piece.md",
            true,
        )];
        assert_eq!(urls[0].dedupe_key, recorded[0].dedupe_key);
        let error = check_collection_overlap(
            &config, &recorded, &urls, None, None, false,
        )
        .expect_err("the twin spelling overlaps the recorded capture");
        assert!(
            error.message.contains("ref/blogs/piece.md"),
            "the overlap names the recorded capture: {}",
            error.message
        );
    }

    #[test]
    fn legacy_overlap_is_reported_once_per_note_and_does_not_refuse() {
        let config = config();
        let urls = [
            url("https://example.com/series-part-1"),
            url("https://example.com/series-part-2"),
        ];
        let mut first = source(
            "https://example.com/series-part-1",
            "/vault/ref/blogs/legacy.md",
            true,
        );
        first.source_pdf = None;
        let mut second = source(
            "https://example.com/series-part-2",
            "/vault/ref/blogs/legacy.md",
            true,
        );
        second.source_pdf = None;
        let outcome = check_collection_overlap(
            &config,
            &[first, second],
            &urls,
            None,
            None,
            false,
        )
        .expect("legacy-only overlap continues");
        match outcome {
            OverlapCheck::Clear { legacy } => assert_eq!(legacy.len(), 1),
            OverlapCheck::Attach(_) => panic!("legacy notes never attach"),
        }
    }
}
