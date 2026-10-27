//! Add link records to saved-source catalogs written before URI links were
//! recorded.

use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
};

use clap::ArgMatches;

use super::{
    build_index,
    cli::list_config_from_matches,
    output::{print_find_error, print_show_error, Format},
    show::{resolve_show, ShowFailure},
    store::source_read,
    RefIndex, RefRow,
};
use crate::native::highlights_ref::{
    pdf_links, reference_mutation,
    source_store::{self, StoreRead, StoreStatus, StoredLink},
    COMMAND_NAME,
};

const RECATALOG_BATCH_SIZE: usize = 100;

struct Counts {
    added: usize,
    skipped: usize,
    links: usize,
    links_on_pages: usize,
}

fn eligible(read: &StoreRead) -> bool {
    matches!(read.status, StoreStatus::Available | StoreStatus::NoCode)
        && read
            .catalog
            .as_ref()
            .is_some_and(|catalog| catalog.links.is_none())
}

fn resolve_selected(
    index: &RefIndex,
    queries: &[String],
) -> Result<Vec<usize>, i32> {
    let mut selected = Vec::new();
    if queries.is_empty() {
        selected.extend(index.rows.iter().enumerate().filter_map(
            |(number, row)| {
                eligible(&source_read(index, row)).then_some(number)
            },
        ));
    } else {
        for query in queries {
            match resolve_show(index, query) {
                Ok(resolved) => selected.push(resolved.row),
                Err(ShowFailure::Ambiguous { query, candidates }) => {
                    print_show_error(
                        Format::Human,
                        "ref recatalog",
                        "ambiguous_reference",
                        &format!("ambiguous reference: {query}"),
                        &candidates,
                    );
                    return Err(1);
                }
                Err(ShowFailure::NotFound { query, candidates }) => {
                    print_show_error(
                        Format::Human,
                        "ref recatalog",
                        "unknown_reference",
                        &format!("no reference note matches {query}"),
                        &candidates,
                    );
                    return Err(1);
                }
            }
        }
    }
    selected.sort_unstable();
    selected.dedup();
    Ok(selected)
}

/// Run `bob ref recatalog` against eligible saved-source stores.
pub(crate) fn run_recatalog(matches: &ArgMatches) -> i32 {
    let config = list_config_from_matches(matches);
    let index = match build_index(&config) {
        Ok(index) => index,
        Err(error) => {
            print_find_error(
                Format::Human,
                "ref recatalog",
                "missing_ref_dir",
                &error.to_string(),
                Some("pass -b/--bob-dir or -r/--ref-dir, or set BOB_DIR"),
            );
            return 1;
        }
    };
    let queries = matches
        .get_many::<String>("ref")
        .map(|values| values.cloned().collect::<Vec<_>>())
        .unwrap_or_default();
    let selected = match resolve_selected(&index, &queries) {
        Ok(selected) => selected,
        Err(code) => return code,
    };
    let dry_run = matches.get_flag("dry-run");
    let _session = if dry_run {
        None
    } else {
        match reference_mutation::acquire_session(&config.bob_dir, || {
            eprintln!("waiting for vault maintenance lock to recatalog links…")
        }) {
            Ok(session) => Some(session),
            Err(error) => {
                eprintln!(
                    "{COMMAND_NAME}: error: could not take the reference-mutation session: {}",
                    mutation_session_error(&error),
                );
                return 1;
            }
        }
    };

    let mut counts = Counts {
        added: 0,
        skipped: 0,
        links: 0,
        links_on_pages: 0,
    };
    let mut failed = false;
    let mut batch_paths = BTreeSet::new();
    let mut batch_count = 0usize;
    let mut pandoc_hint_printed = false;

    for row_index in selected {
        let row = &index.rows[row_index];
        match recatalog_one(&index, row, dry_run) {
            Ok(RecatalogResult::Added {
                links,
                on_pages,
                path,
            }) => {
                println!(
                    "{}: {} {} links ({} on pages)",
                    row.path,
                    if dry_run { "would add" } else { "added" },
                    links,
                    on_pages,
                );
                counts.added += 1;
                counts.links += links;
                counts.links_on_pages += on_pages;
                if let Some(path) = path {
                    batch_paths.insert(path);
                    batch_count += 1;
                    if batch_count >= RECATALOG_BATCH_SIZE {
                        if let Err(error) = commit_batch(
                            &config.bob_dir,
                            &batch_paths,
                            batch_count,
                        ) {
                            eprintln!("{COMMAND_NAME}: error: {error}");
                            return 1;
                        }
                        batch_paths.clear();
                        batch_count = 0;
                    }
                }
            }
            Ok(RecatalogResult::Skipped(reason)) => {
                println!("{}: skipped ({reason})", row.path);
                counts.skipped += 1;
                failed |= reason.starts_with("pandoc failed:");
                if reason.contains("pandoc command not found")
                    && !pandoc_hint_printed
                {
                    eprintln!(
                        "hint: install pandoc or set BOB_PANDOC_COMMAND, then run bob ref doctor"
                    );
                    pandoc_hint_printed = true;
                }
            }
            Err(error) => {
                eprintln!("{COMMAND_NAME}: error: {error}");
                return 1;
            }
        }
    }

    if !dry_run && batch_count > 0 {
        if let Err(error) =
            commit_batch(&config.bob_dir, &batch_paths, batch_count)
        {
            eprintln!("{COMMAND_NAME}: error: {error}");
            return 1;
        }
    }

    println!(
        "summary: {} {}, {} skipped · {} links ({} on pages)",
        counts.added,
        if dry_run { "would add" } else { "added" },
        counts.skipped,
        counts.links,
        counts.links_on_pages,
    );
    if dry_run {
        println!("writes: none");
    }
    if failed {
        1
    } else {
        0
    }
}

enum RecatalogResult {
    Added {
        links: usize,
        on_pages: usize,
        path: Option<PathBuf>,
    },
    Skipped(String),
}

fn recatalog_one(
    index: &RefIndex,
    row: &RefRow,
    dry_run: bool,
) -> Result<RecatalogResult, String> {
    let StoreRead {
        status,
        bytes,
        catalog,
        ..
    } = source_read(index, row);
    let (Some(bytes), Some(catalog)) = (bytes, catalog) else {
        return Ok(RecatalogResult::Skipped(match status {
            StoreStatus::NoCatalog | StoreStatus::NoSource => {
                "no catalog".to_string()
            }
            StoreStatus::SourceMissing => "source missing".to_string(),
            StoreStatus::SourceInvalid => "source invalid".to_string(),
            StoreStatus::Available | StoreStatus::NoCode => {
                "no catalog".to_string()
            }
        }));
    };
    match status {
        StoreStatus::SourceMissing => {
            return Ok(RecatalogResult::Skipped("source missing".to_string()));
        }
        StoreStatus::SourceInvalid => {
            return Ok(RecatalogResult::Skipped("source invalid".to_string()));
        }
        StoreStatus::NoCatalog | StoreStatus::NoSource => {
            return Ok(RecatalogResult::Skipped("no catalog".to_string()));
        }
        StoreStatus::Available | StoreStatus::NoCode => {}
    }
    if catalog.links.is_some() {
        return Ok(RecatalogResult::Skipped("already recorded".to_string()));
    }
    let pdf_rel = row
        .source_pdf
        .as_deref()
        .map(str::trim)
        .filter(|path| !path.is_empty());
    let Some(pdf_rel) = pdf_rel else {
        return Ok(RecatalogResult::Skipped("no PDF".to_string()));
    };
    let pdf_path = index.bob_dir.join(pdf_rel);
    if !pdf_path.is_file() {
        return Ok(RecatalogResult::Skipped("no PDF".to_string()));
    }
    let mut links =
        match super::attach::record_source_catalog(&bytes, &catalog.route) {
            Ok(recorded) => recorded.links.unwrap_or_default(),
            Err(error)
                if error.starts_with("unsupported saved-source route") =>
            {
                return Ok(RecatalogResult::Skipped(
                    "source invalid".to_string(),
                ));
            }
            Err(error) => {
                let detail = if error.starts_with("pandoc command not found") {
                    format!("pandoc failed: {error}")
                } else if let Some(detail) =
                    error.strip_prefix("pandoc failed: ")
                {
                    format!("pandoc failed: {detail}")
                } else {
                    format!("pandoc failed: {error}")
                };
                return Ok(RecatalogResult::Skipped(detail));
            }
        };
    let (_, on_pages) =
        match pdf_links::annotate_catalog_links(&mut links, &pdf_path) {
            Ok(counts) => counts,
            Err(_) => {
                return Ok(RecatalogResult::Skipped("no PDF".to_string()))
            }
        };
    let links_count = links.len();
    if dry_run {
        return Ok(RecatalogResult::Added {
            links: links_count,
            on_pages,
            path: None,
        });
    }

    let pointer = row.source_markdown.as_deref().unwrap_or_default();
    let code_path = index
        .bob_dir
        .join(pointer)
        .parent()
        .map(|parent| parent.join("code.json"))
        .ok_or_else(|| {
            format!("catalog path has no parent for {}", row.path)
        })?;
    let raw = fs::read_to_string(&code_path)
        .map_err(|error| format!("read {}: {error}", code_path.display()))?;
    let mut value: serde_json::Value = serde_json::from_str(&raw)
        .map_err(|error| format!("parse {}: {error}", code_path.display()))?;
    let stored_links = links.iter().map(StoredLink::from).collect::<Vec<_>>();
    let encoded_links = serde_json::to_value(stored_links)
        .map_err(|error| format!("encode recatalog links: {error}"))?;
    value
        .as_object_mut()
        .ok_or_else(|| {
            format!("catalog {} is not a JSON object", code_path.display())
        })?
        .insert("links".to_string(), encoded_links);
    let rendered = serde_json::to_string_pretty(&value)
        .map_err(|error| format!("encode {}: {error}", code_path.display()))?;
    reference_mutation::atomic_write_bytes(&code_path, rendered.as_bytes())
        .map_err(|error| format!("write {}: {error}", code_path.display()))?;
    Ok(RecatalogResult::Added {
        links: links_count,
        on_pages,
        path: Some(code_path),
    })
}

fn commit_batch(
    bob_dir: &Path,
    paths: &BTreeSet<PathBuf>,
    count: usize,
) -> Result<(), String> {
    let (included, ignored) =
        reference_mutation::partition_ignored(bob_dir, paths);
    if !ignored.is_empty() {
        eprintln!("{}", source_store::ignored_catalog_warning());
    }
    let subject = format!("bob ref recatalog: {count} references");
    match reference_mutation::commit_paths(bob_dir, &subject, &included)? {
        Some(sha) => {
            println!("committed {sha} ({count} references)");
        }
        None => {
            eprintln!("warning: nothing to commit for {count} recatalog(s)")
        }
    }
    Ok(())
}

fn mutation_session_error(
    error: &reference_mutation::MutationSessionError,
) -> String {
    match error {
        reference_mutation::MutationSessionError::Maintenance(error) => {
            error.to_string()
        }
        reference_mutation::MutationSessionError::NotAWorktree => {
            "vault is not a Git worktree".to_string()
        }
        reference_mutation::MutationSessionError::Scan(error) => {
            error.to_string()
        }
    }
}
