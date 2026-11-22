//! The podcast route behind `bob ref create <Spotify episode>`.
//!
//! It mirrors the arXiv route: the shared recapture preflight already
//! resolved duplicates, rereads, and overwrites against the canonical
//! episode key before any network work. This route then fetches the
//! episode through the dossier builder, plans the `podcasts/` target from
//! the title stem, prints the dossier through the adapter's offline
//! `render` op, stamps and installs it, and saves the write-once source
//! store before intake files the note.

use super::super::podcast::dossier::{
    self, Dossier, EpisodeOverrides, RenderedDossier,
};
use super::super::spotify::SpotifyEpisode;
use super::*;

/// How reports name the source kind: `source: <url> (Spotify episode)`.
const SOURCE_KIND: &str = "Spotify episode";

/// Route `url` here when it names one Spotify episode. Earlier steps
/// already normalized URIs and short links and refused everything else,
/// so the episode keeps the original link's `?t=` shared moment while
/// every identity uses the canonical URL.
pub(super) fn spotify_episode(target_raw: &str) -> Option<SpotifyEpisode> {
    super::super::spotify::SpotifyLink::parse(target_raw)?.episode()
}

pub(super) fn create_podcast_route(
    config: &Config,
    episode: SpotifyEpisode,
    url: super::super::clip_url::WebUrl,
    scratch: &mut super::super::ScratchDir,
    options: &CreateOptions,
    recorded: &[sources_mod::RecordedSource],
) -> Result<()> {
    if let Some(selection) = options.overwrite().cloned() {
        return overwrite_podcast(
            config, episode, url, scratch, options, &selection,
        );
    }
    // Configuration errors surface before any network work.
    let tables = tables_settings()?;
    let ref_type =
        default_ref_type_for_kind("podcast", options.ref_type.as_deref())?;
    let name = options
        .name
        .as_deref()
        .map(super::super::clip_url::validate_name)
        .transpose()?;
    let clock = super::super::note::capture_clock();
    let dossier = prepare_dossier(&episode, options, &clock.date, scratch)?;
    let key = episode.dedupe_key();
    let mut keys = vec![key.clone()];
    if url.dedupe_key != key {
        keys.push(url.dedupe_key.clone());
    }
    let identity = DefaultTargetIdentity::Url {
        keys: keys.clone(),
        recorded,
    };
    // A reread plans its allocated `<root>__vN` stem with no suffix
    // fallback; ordinary captures plan from `-N` or the title stem.
    let target_plan = match options.reread() {
        Some(preflight) => super::super::recapture::plan_reread_target(
            config,
            &preflight.plan.stem,
            &ref_type,
            &identity,
        )?,
        None => plan_target_for(
            config,
            &name.unwrap_or_else(|| dossier.stem()),
            &ref_type,
            options.output.as_ref(),
            options.force,
            true,
            &identity,
            &url.cleaned,
        )?,
    };
    let final_stem = target_plan.stem.clone();
    // Post-fetch identity guard: the stem is only known now, so an
    // ordinary capture re-checks dedupe against its final target, and a
    // reread re-resolves its keys against the live index.
    match options.reread() {
        None => check_dedupe_with_listen_hint(
            config,
            recorded,
            &key,
            Some(&target_plan.target),
            options.force.is_some(),
        )?,
        Some(preflight) => {
            let fresh_recorded = sources_mod::collect_recorded_source_urls(
                config,
            )
            .map_err(|error| CommandError::new(error.message.clone()))?;
            let rows = super::super::recapture::index_rows(config);
            let wanted: std::collections::BTreeSet<String> =
                keys.iter().cloned().collect();
            let fresh = super::super::recapture::preflight_for_url(
                config,
                &rows,
                &fresh_recorded,
                &wanted,
                options.name.as_deref(),
                None,
                options.force,
                &url.cleaned,
            )?;
            if fresh.is_some_and(|next| next.plan.stem != target_plan.stem) {
                return Err(CommandError::new(format!(
                    "reread {} is already filed; nothing written",
                    target_plan.stem,
                )));
            }
            super::super::recapture::refuse_redirected_reread(
                config,
                &key,
                &dossier.canonical_url(),
                preflight,
            )?;
        }
    }
    let audio = plan_pdf_audio(config, &target_plan, options, None)?;
    let mut fields = marker_fields(&dossier);
    fields.push((FIELD_CAPTURED, MarkerValue::String(clock.stamp.clone())));
    let marker = compose_marker(
        &options.status,
        &options.parent,
        &dossier.title,
        Some(&final_stem),
        &fields,
    )?;
    let styler = Styler::detect();
    if options.dry_run {
        let mut legacy = Vec::new();
        for key in &keys {
            for hit in sources_mod::legacy_hits(recorded, key) {
                if !legacy.iter().any(|known: &sources_mod::RecordedSource| {
                    known.path == hit.path
                }) {
                    legacy.push(hit);
                }
            }
        }
        print_dry_run(
            config,
            &styler,
            &dossier,
            &target_plan,
            audio.as_ref(),
            &clock.stamp,
            &final_stem,
            &marker,
            options,
            &legacy,
        );
        return Ok(());
    }
    let rendered = render_dossier(&dossier, tables)?;
    let info = PdfInfo {
        title: Some(dossier.title.clone()),
        author: dossier.pdf_author(),
    };
    let (stamped, page_count) = pdf_target_mod::stamp_pdf_to_scratch(
        &rendered.pdf,
        scratch,
        &marker,
        &info,
    )?;
    let installed = pdf_target_mod::install_stamped_pdf(
        &stamped,
        &target_plan.target,
        audio.as_ref(),
        page_count,
    )?;
    let bytes = fs::metadata(&target_plan.target)
        .map(|metadata| metadata.len())
        .unwrap_or(0);
    println!(
        "{} created Highlights-ready podcast PDF",
        styler.success_prefix(false)
    );
    println!("source: {} ({SOURCE_KIND})", dossier.canonical_url());
    print_reread_or_renamed(&target_plan, options);
    println!("pdf: {}", target_plan.target.display());
    match &audio {
        Some(audio) => {
            println!("audio: {} (from {})", audio.dest.display(), audio.origin)
        }
        None => println!("audio: none"),
    }
    println!("title: {}", dossier.title);
    if let Some(author) = &dossier.author {
        println!("author: {author}");
    }
    if let Some(published) = &dossier.published {
        println!("published: {published}");
    }
    if let Some(duration) = dossier.duration_display() {
        println!("duration: {duration}");
    }
    println!("captured: {}", clock.stamp);
    println!("status: {}", options.status);
    println!("parent: {}", options.parent);
    println!("id: {final_stem}");
    println!("capture: {}", dossier.capture_summary());
    println!(
        "pages: {installed} · size: {}",
        super::super::clip::format_bytes(bytes)
    );
    // The printed dossier's source view is stored write-once with its
    // link catalog, keyed to the canonical episode URL; a queued (`-k`)
    // birth adopts it by PDF digest when `bob ref scan` files the note.
    let source_pointer = super::super::web_source::persist_article_store(
        config,
        &target_plan,
        &rendered.pdf,
        &target_plan.target,
        rendered.source_html.as_deref(),
        rendered.code_anchors.as_deref(),
        dossier.canonical_url(),
    );
    finish_create_intake(
        config,
        &target_plan,
        options.keep_intake_reason.as_deref(),
        source_pointer.as_deref(),
    )?;
    print_adapter_warnings(&styler, &rendered);
    Ok(())
}

/// Replace a filed episode reading in place. Eligibility was proven
/// before any fetch; the note, stem, id, task, and type are kept, and the
/// fresh dossier's source view is staged into the same transaction.
fn overwrite_podcast(
    config: &Config,
    episode: SpotifyEpisode,
    url: super::super::clip_url::WebUrl,
    scratch: &mut super::super::ScratchDir,
    options: &CreateOptions,
    selection: &OverwriteSelection,
) -> Result<()> {
    let (plan, mut request, replacing) =
        prepare_web_overwrite(config, options, selection)?;
    if options.dry_run {
        super::super::overwrite::local::print_dry_run(
            &plan,
            &plan.title,
            replacing,
            options.explicit_status.as_deref(),
        );
        return Ok(());
    }
    let tables = tables_settings()?;
    let clock = super::super::note::capture_clock();
    let dossier = prepare_dossier(&episode, options, &clock.date, scratch)?;
    let key = episode.dedupe_key();
    super::super::recapture::refuse_redirected_overwrite(
        config,
        &key,
        &dossier.canonical_url(),
        selection,
    )?;
    let rendered = render_dossier(&dossier, tables)?;
    let title = dossier.title.clone();
    let info = PdfInfo {
        title: Some(title.clone()),
        author: dossier.pdf_author(),
    };
    let stamped = stamp_overwrite_source(
        config,
        &plan,
        options,
        &rendered.pdf,
        scratch,
        &title,
        marker_fields(&dossier),
        &info,
    )?;
    let (plan, source_store) = stage_web_overwrite_store(
        config,
        selection,
        &mut request,
        plan,
        scratch.path(),
        &stamped,
        &rendered.pdf,
        rendered.source_html.as_deref(),
        rendered.code_anchors.as_deref(),
        dossier.canonical_url(),
        super::super::web_source::WEB_ARTICLE_ROUTE,
    )?;
    let audio = stage_overwrite_audio(
        options,
        &plan,
        &request,
        scratch,
        None,
        &url.cleaned,
        &stamped,
        &title,
    )?;
    finish_local_overwrite(
        config,
        options,
        &plan,
        &request,
        stamped,
        audio,
        &title,
        replacing,
        scratch,
        &url.cleaned,
        source_store,
    )?;
    print_adapter_warnings(&Styler::detect(), &rendered);
    Ok(())
}

fn tables_settings() -> Result<super::super::clip_adapter::TablesSettings> {
    let (layout, prose_words) = table_layout_settings()?;
    Ok(super::super::clip_adapter::TablesSettings {
        layout,
        prose_words,
    })
}

/// Fetch the episode, then report each degradation as one `warning:`
/// line. A metadata failure writes nothing.
fn prepare_dossier(
    episode: &SpotifyEpisode,
    options: &CreateOptions,
    captured: &str,
    scratch: &super::super::ScratchDir,
) -> Result<Dossier> {
    let overrides = EpisodeOverrides {
        title: options.title.clone(),
        author: options.author.clone(),
        published: options.published.clone(),
    };
    let enrich = super::podcast_enrich_setting()?;
    let dossier = dossier::prepare(
        episode,
        &overrides,
        captured,
        scratch.path(),
        Some(&fetch_progress),
        None,
        enrich,
        !options.dry_run,
    )
    .map_err(|error| CommandError::new(error.render()))?;
    for warning in &dossier.warnings {
        eprintln!("warning: {warning}");
    }
    Ok(dossier)
}

fn render_dossier(
    dossier: &Dossier,
    tables: super::super::clip_adapter::TablesSettings,
) -> Result<RenderedDossier> {
    crate::native::style::stderr_progress_line("rendering episode dossier…");
    dossier
        .render(tables)
        .map_err(|error| CommandError::new(error.render()))
}

/// The marker fields every podcast reading carries (besides `captured`).
fn marker_fields(dossier: &Dossier) -> Vec<(&'static str, MarkerValue)> {
    let mut fields =
        vec![("source_url", MarkerValue::String(dossier.canonical_url()))];
    if let Some(author) = &dossier.author {
        fields.push(("author", MarkerValue::String(author.clone())));
    }
    if let Some(published) = &dossier.published {
        fields.push(("published", MarkerValue::String(published.clone())));
    }
    fields
}

fn print_adapter_warnings(styler: &Styler, rendered: &RenderedDossier) {
    for warning in &rendered.warnings {
        eprintln!("{}: {warning}", styler.warning_prefix());
    }
}

#[allow(clippy::too_many_arguments)]
fn print_dry_run(
    config: &Config,
    styler: &Styler,
    dossier: &Dossier,
    target_plan: &TargetPlan,
    audio: Option<&AudioCopyPlan>,
    captured: &str,
    id: &str,
    marker: &str,
    options: &CreateOptions,
    legacy: &[sources_mod::RecordedSource],
) {
    println!(
        "{} would create Highlights-ready podcast PDF",
        styler.success_prefix(true)
    );
    println!("source: {} ({SOURCE_KIND})", dossier.canonical_url());
    print_reread_or_renamed(target_plan, options);
    println!("pdf: {}", target_plan.target.display());
    match audio {
        Some(audio) => {
            println!("audio: {} (from {})", audio.dest.display(), audio.origin)
        }
        None => println!("audio: none"),
    }
    println!("sidecar_guard: {}", target_plan.sidecar.display());
    match &target_plan.workflow {
        TargetWorkflow::Intake {
            library_destination,
        } => println!("library_destination: {}", library_destination.display()),
        _ => println!("library_destination: n/a (external target)"),
    }
    println!(
        "title: {} ({})",
        dossier.title,
        dossier.title_origin.label()
    );
    let attributed = |field: &str, value: &Option<String>, origin| {
        value.as_ref().map(|value| match origin {
            Some(origin) => format!(
                "{field}: {value} ({})",
                dossier::FieldOrigin::label(origin)
            ),
            None => format!("{field}: {value}"),
        })
    };
    let byline: Vec<String> = [
        attributed("author", &dossier.author, dossier.author_origin),
        attributed("published", &dossier.published, dossier.published_origin),
    ]
    .into_iter()
    .flatten()
    .collect();
    if !byline.is_empty() {
        println!("{}", byline.join(" · "));
    }
    println!(
        "duration: {} · chapters: {} · cover: {}",
        dossier
            .duration_display()
            .unwrap_or_else(|| "unknown".to_string()),
        dossier.chapter_count(),
        if dossier.has_cover() { "yes" } else { "no" },
    );
    if let Some(line) = dossier.dry_run_feed_line() {
        println!("{line}");
    }
    if let Some(line) = dossier.dry_run_transcript_line() {
        println!("{line}");
    }
    println!("captured: {captured}");
    for hit in legacy {
        println!(
            "legacy: {} (superseded by this capture)",
            hit.path.display()
        );
    }
    println!("status: {}", options.status);
    print_parent_line(options);
    println!("id: {id}");
    println!("marker:");
    print!("{marker}");
    if !marker.ends_with('\n') {
        println!();
    }
    print_create_intake_preview(
        config,
        target_plan,
        options.keep_intake_reason.as_deref(),
    );
    println!("writes: none");
}
