//! Shared capture-family classification and reread bundle allocation.
//!
//! Read-only planning for explicit rereads (`-f`/`--force=reread`): it
//! classifies live filed hits into one selected family, an ambiguity, or
//! a partial overlap; allocates the whole-bundle `<root>__vN` stem; and
//! validates reread-only `-N`/`-o` spellings. It never mutates the vault:
//! the single-source, collection, and clip routes consume its preflight
//! to plan reread writes.
//!
//! The family read model in
//! [`crate::native::ref_library::lineage`] stays authority: a family's
//! root is the member with no `Reading` entries, and a member's reading
//! number is one plus its `ref/` entries. Nothing here infers a reading
//! number from a filename, so a natural `protocol__v2` without lineage
//! is reading 1 and keeps its full stem as the next root.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use super::sources::RecordedSource;
use super::{CommandError, Config};
use crate::native::ref_library::{
    build_index, LibraryConfig, ParsedFrontmatter, RefRow,
};

/// One live family member: a filed ref note, or a queued intake PDF.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct FamilyMember {
    /// Vault-relative path (`ref/papers/foo.md`, `xlib/blogs/foo.pdf`).
    pub path: String,
    /// Filename stem.
    pub stem: String,
    /// Computed reading number for filed members; `None` for queued PDFs
    /// and notes outside a multi-reading family.
    pub reading: Option<u32>,
    /// The member's library type directory, when known.
    pub ref_type: Option<String>,
    /// The located reading task's residence note, when the index found one.
    pub task_path: Option<String>,
    /// The located reading task's block id, when known.
    pub task_block: Option<String>,
    /// Whether the member's note carries companion audio.
    pub has_audio: bool,
    /// The member note's `audio` frontmatter value, when any.
    pub audio_path: Option<String>,
    /// Ref-note `status`, when known.
    pub status: Option<String>,
    /// Ref-note `captured` date, when known.
    pub captured: Option<String>,
    /// Ref-note `highlights_count`, when known.
    pub highlights: Option<u64>,
    /// Normalized identity keys (sorted, URL-derived; see
    /// [`comparison_keys`]).
    pub source_keys: Vec<String>,
    /// True for queued intake PDFs.
    pub is_queued: bool,
}

/// One selected capture family: the root plus every live member.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct SelectedFamily {
    /// The family root's full stem. A later reading never becomes the
    /// root, and a natural version-looking root (such as `protocol__v2`
    /// without lineage) keeps its full stem.
    pub root_stem: String,
    /// Vault-relative path of the family root note.
    pub root_path: String,
    /// Every live member, newest reading first.
    pub members: Vec<FamilyMember>,
    /// Index into [`SelectedFamily::members`] of the explicitly selected
    /// member; the newest reading by default.
    pub selected: usize,
    /// Union of the members' URL-derived identity keys.
    pub source_keys: BTreeSet<String>,
    /// Highest reading number across members (unread members count 1).
    pub max_reading: u32,
}

impl SelectedFamily {
    /// The newest reading.
    pub(super) fn newest(&self) -> &FamilyMember {
        &self.members[0]
    }

    /// The explicitly selected member (the newest by default).
    pub(super) fn selected_member(&self) -> &FamilyMember {
        &self.members[self.selected]
    }

    /// Index of the member with this stem, if any.
    pub(super) fn member_named(&self, stem: &str) -> Option<usize> {
        self.members.iter().position(|member| member.stem == stem)
    }
}

/// One overlapping group for a partial URL overlap: a filed family or a
/// legacy-only note group sharing keys with the wanted set.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct OverlapGroup {
    /// Family root stem, or the legacy note's stem.
    pub root_stem: String,
    /// Every member path, newest first.
    pub member_paths: Vec<String>,
    /// The group's URL-derived identity keys.
    pub keys: Vec<String>,
    /// True for legacy notes without a Highlights PDF (never reread roots).
    pub is_legacy: bool,
}

/// What a capture target resolves to against the live library.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum RecaptureMatch {
    /// No filed match: a bare force on a fresh capture is ordinary.
    NoMatch,
    /// Only queued intake PDFs match.
    QueuedOnly { members: Vec<FamilyMember> },
    /// Exactly one live family owns the complete wanted key set.
    Selected { family: SelectedFamily },
    /// Unrelated families share keys with the wanted set: list every
    /// member path and ask for `-N <stem>`.
    Ambiguous { families: Vec<SelectedFamily> },
    /// One family owns only part of the wanted key set (including a
    /// one-URL query into a multi-URL collection).
    PartialOverlap {
        wanted: Vec<String>,
        overlapping: Vec<OverlapGroup>,
    },
}

/// A resolved reread ready to write: the allocated plan plus the family
/// defaults (newest reading's task residence and type) the route applies
/// when explicit `-P`/`-t` are absent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct RereadPreflight {
    /// The allocated reread plan.
    pub plan: RereadPlan,
    /// Defaults from the newest reading.
    pub defaults: FamilyDefaults,
}

/// A filed member selected for in-place overwrite. Defaults come from the
/// selected member (explicit `-N`, else the newest reading), not from
/// reread's newest-member allocation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct OverwriteSelection {
    /// The family whose selected member will be replaced.
    pub family: SelectedFamily,
    /// Defaults from the selected member's located task and type.
    pub defaults: FamilyDefaults,
}

/// Typed capture outcome for ordinary create, reread, and overwrite.
/// Routes branch on this instead of inferring overwrite from error
/// strings or stacking booleans.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum CaptureOutcome {
    /// No filed family: an ordinary new capture (or a fresh-overwrite
    /// refusal later, once the target plan is known).
    Fresh,
    /// An allocated reread of a filed family.
    Reread(RereadPreflight),
    /// An in-place overwrite of the selected family member.
    Overwrite(OverwriteSelection),
}

impl CaptureOutcome {
    pub(super) fn reread(&self) -> Option<&RereadPreflight> {
        match self {
            Self::Reread(preflight) => Some(preflight),
            Self::Fresh | Self::Overwrite(_) => None,
        }
    }

    pub(super) fn overwrite(&self) -> Option<&OverwriteSelection> {
        match self {
            Self::Overwrite(selection) => Some(selection),
            Self::Fresh | Self::Reread(_) => None,
        }
    }

    pub(super) fn is_reread(&self) -> bool {
        matches!(self, Self::Reread(_))
    }

    pub(super) fn is_overwrite(&self) -> bool {
        matches!(self, Self::Overwrite(_))
    }
}

/// The planned reread: the selected family plus its allocated stem.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct RereadPlan {
    /// The family being read again.
    pub family: SelectedFamily,
    /// Allocated `<root>__vN` stem.
    pub stem: String,
}

impl RereadPlan {
    /// The `reread:` output line, printed at the existing `renamed:`
    /// position by the routes that enable rereads.
    pub(super) fn report_line(&self) -> String {
        let selected = self.family.selected_member();
        reread_report_line(
            &selected.stem,
            &self.stem,
            selected.reading.unwrap_or(1),
            selected.status.as_deref(),
            selected.highlights,
        )
    }
}

/// Defaults carried from a family member: its located task residence and
/// its library type. Reread uses the newest reading; overwrite uses the
/// selected member. Explicit `-P`/`-t` always win at the call site.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct FamilyDefaults {
    pub parent: Option<String>,
    pub ref_type: Option<String>,
}

/// Route-specific identity used when a planned title-route stem is already
/// occupied. Local Markdown/PDF captures compare PDF titles; same-day vault
/// snapshots compare their canonical `source_note` routes.
#[derive(Debug, Clone, Copy)]
pub(super) enum StemIdentity<'a> {
    Title(&'a str),
    SourceNote(&'a str),
}

/// Best-effort library rows for reread planning: an unreadable index
/// behaves like an empty library, and the legacy duplicate checks keep
/// refusing truthfully.
pub(super) fn index_rows(config: &Config) -> Vec<RefRow> {
    let library = LibraryConfig {
        bob_dir: config.bob_dir.clone(),
        ref_dir: config.ref_dir.clone(),
        xlib_dir: config.xlib_dir.clone(),
    };
    build_index(&library)
        .map(|index| index.rows)
        .unwrap_or_default()
}

/// Defaults from a family member's actual located task and type.
pub(super) fn family_defaults(member: &FamilyMember) -> FamilyDefaults {
    FamilyDefaults {
        parent: member.task_path.clone(),
        ref_type: member.ref_type.clone(),
    }
}

/// Vault-relative ref-note path to computed reading number, for every
/// row in a multi-reading family. Best-effort: an unreadable index
/// yields no readings and refusals render as before.
pub(super) fn family_readings_for(config: &Config) -> BTreeMap<String, u32> {
    let library = LibraryConfig {
        bob_dir: config.bob_dir.clone(),
        ref_dir: config.ref_dir.clone(),
        xlib_dir: config.xlib_dir.clone(),
    };
    let Ok(index) = build_index(&library) else {
        return BTreeMap::new();
    };
    index
        .rows
        .iter()
        .filter_map(|row| row.reading.map(|number| (row.path.clone(), number)))
        .collect()
}

/// Follow a single-source attach hit to the newest reading in its family.
/// Unindexed notes and queued PDFs keep the path selected by the original
/// refusal scan.
pub(super) fn newest_reading_hit(
    config: &Config,
    recorded: &[RecordedSource],
    hit: &RecordedSource,
) -> RecordedSource {
    if !hit.is_ref_note {
        return hit.clone();
    }
    let relative = vault_relative(&hit.path, &config.bob_dir);
    let rows = index_rows(config);
    let Some(selected) = rows.iter().find(|row| row.path == relative) else {
        return hit.clone();
    };
    if selected.superseded_by.is_some() {
        return hit.clone();
    }
    let Some(newest) = rows
        .iter()
        .filter(|row| {
            row.family == selected.family && row.superseded_by.is_none()
        })
        .max_by(|left, right| {
            left.reading
                .unwrap_or(1)
                .cmp(&right.reading.unwrap_or(1))
                .then_with(|| right.path.cmp(&left.path))
        })
    else {
        return hit.clone();
    };
    recorded
        .iter()
        .find(|source| {
            source.is_ref_note
                && source.dedupe_key == hit.dedupe_key
                && vault_relative(&source.path, &config.bob_dir) == newest.path
        })
        .cloned()
        .unwrap_or_else(|| hit.clone())
}

/// Exact collection identity at family granularity. All live readings in
/// one family must carry the complete wanted key set; the selected attach
/// target is that family's newest note. Multiple unrelated exact families
/// remain ambiguous and do not attach implicitly.
pub(super) fn newest_exact_family_hit(
    config: &Config,
    recorded: &[RecordedSource],
    wanted: &BTreeSet<String>,
) -> Option<RecordedSource> {
    let rows = index_rows(config);
    let by_path: BTreeMap<&str, &RefRow> =
        rows.iter().map(|row| (row.path.as_str(), row)).collect();
    let mut path_keys = BTreeMap::<String, BTreeSet<String>>::new();
    for source in recorded.iter().filter(|source| {
        source.is_ref_note
            && source.source_pdf.is_some()
            && wanted.contains(&source.dedupe_key)
    }) {
        path_keys
            .entry(vault_relative(&source.path, &config.bob_dir))
            .or_default()
            .insert(source.dedupe_key.clone());
    }

    let mut family_keys = BTreeSet::new();
    let mut unindexed_paths = BTreeSet::new();
    for (path, keys) in &path_keys {
        if let Some(row) = by_path.get(path.as_str()) {
            if row.superseded_by.is_none() {
                family_keys.insert(row.family.clone());
            }
        } else if keys == wanted {
            unindexed_paths.insert(path.clone());
        }
    }

    let mut exact_families = Vec::<(String, &RefRow)>::new();
    for family_key in family_keys {
        let mut members = rows.iter().filter(|row| {
            row.family == family_key && row.superseded_by.is_none()
        });
        let Some(first) = members.next() else {
            continue;
        };
        if comparison_keys(first).into_iter().collect::<BTreeSet<_>>()
            != *wanted
            || members.any(|row| {
                comparison_keys(row).into_iter().collect::<BTreeSet<_>>()
                    != *wanted
            })
        {
            continue;
        }
        let newest = rows
            .iter()
            .filter(|row| {
                row.family == family_key && row.superseded_by.is_none()
            })
            .max_by(|left, right| {
                left.reading
                    .unwrap_or(1)
                    .cmp(&right.reading.unwrap_or(1))
                    .then_with(|| right.path.cmp(&left.path))
            })?;
        exact_families.push((family_key, newest));
    }

    if exact_families.len() + unindexed_paths.len() != 1 {
        return None;
    }
    let newest_path = exact_families
        .first()
        .map(|(_, row)| row.path.as_str())
        .or_else(|| unindexed_paths.first().map(String::as_str))?;
    recorded
        .iter()
        .find(|source| {
            source.is_ref_note
                && wanted.contains(&source.dedupe_key)
                && vault_relative(&source.path, &config.bob_dir) == newest_path
        })
        .cloned()
}

/// Resolve URL-dedupe keys against the live library: complete normalized
/// URL-key sets decide, so a one-URL query into a multi-URL collection
/// is a partial overlap, while several readings sharing one exact set
/// count as one family. Legacy-only hits keep their warning behavior and
/// are never roots for the selected decision.
pub(super) fn resolve_for_url_keys(
    rows: &[RefRow],
    recorded: &[RecordedSource],
    wanted: &BTreeSet<String>,
    bob_dir: &Path,
) -> RecaptureMatch {
    let mut filed: Vec<&RecordedSource> = Vec::new();
    let mut queued: Vec<&RecordedSource> = Vec::new();
    for hit in recorded {
        if !wanted.contains(&hit.dedupe_key) {
            continue;
        }
        if hit.is_ref_note {
            if hit.source_pdf.is_some() {
                filed.push(hit);
            }
        } else {
            queued.push(hit);
        }
    }
    if filed.is_empty() {
        if queued.is_empty() {
            return RecaptureMatch::NoMatch;
        }
        return RecaptureMatch::QueuedOnly {
            members: queued
                .iter()
                .map(|hit| queued_member(hit, bob_dir))
                .collect(),
        };
    }
    let by_path: BTreeMap<&str, &RefRow> =
        rows.iter().map(|row| (row.path.as_str(), row)).collect();
    let mut families: BTreeMap<String, Vec<FamilyMember>> = BTreeMap::new();
    // One recorded key per note file is enough: a multi-URL collection
    // note records one hit per key but joins the family only once.
    let mut seen_paths: BTreeSet<String> = BTreeSet::new();
    for hit in &filed {
        let relative = vault_relative(&hit.path, bob_dir);
        if !seen_paths.insert(relative.clone()) {
            continue;
        }
        match by_path.get(relative.as_str()) {
            Some(row) => {
                families
                    .entry(row.family.clone())
                    .or_default()
                    .push(member_from_row(row, bob_dir));
            }
            None => {
                // Filed on disk but outside the index: keep it refusing
                // as its own singleton rather than silently passing.
                families
                    .entry(relative.clone())
                    .or_default()
                    .push(unindexed_member(hit, &relative));
            }
        }
    }
    let mut families: Vec<SelectedFamily> = families
        .into_iter()
        .map(|(root_path, members)| {
            select_family(
                &root_path,
                family_root_stem(rows, &root_path),
                members,
            )
        })
        .collect();
    families.sort_by(|left, right| left.root_path.cmp(&right.root_path));

    let mut exact: Vec<SelectedFamily> = families
        .iter()
        .filter(|family| family_has_exact_keys(family, wanted))
        .cloned()
        .collect();
    if !exact.is_empty() {
        if exact.len() == 1 {
            return RecaptureMatch::Selected {
                family: exact.pop().expect("one exact family"),
            };
        }
        return RecaptureMatch::Ambiguous { families: exact };
    }

    let overlapping: Vec<OverlapGroup> = families
        .iter()
        .filter(|family| !family.source_keys.is_disjoint(wanted))
        .map(|family| overlap_group(family, false))
        .collect();
    if !overlapping.is_empty() {
        return RecaptureMatch::PartialOverlap {
            wanted: wanted.iter().cloned().collect(),
            overlapping,
        };
    }
    RecaptureMatch::NoMatch
}

/// Every live family holding `stem` as a member stem: empty when the name
/// is free (an ordinary new capture), one family for the common case, or
/// several when unrelated families share a stem across type directories.
/// Superseded legacy notes never select. The chosen root stays the
/// family's root even when the named stem is a later reading.
pub(super) fn families_for_stem(
    rows: &[RefRow],
    stem: &str,
    bob_dir: &Path,
) -> Vec<SelectedFamily> {
    let mut grouped: BTreeMap<String, Vec<FamilyMember>> = BTreeMap::new();
    for row in rows {
        if row.superseded_by.is_some() {
            continue;
        }
        if stem_of_path(&row.path) != stem {
            continue;
        }
        grouped
            .entry(row.family.clone())
            .or_default()
            .push(member_from_row(row, bob_dir));
    }
    // Include every family member for context, not just the named stem.
    let mut families: Vec<SelectedFamily> = grouped
        .into_keys()
        .map(|family_key| {
            let members: Vec<FamilyMember> = rows
                .iter()
                .filter(|row| {
                    row.family == family_key && row.superseded_by.is_none()
                })
                .map(|row| member_from_row(row, bob_dir))
                .collect();
            select_family(
                &family_key,
                family_root_stem(rows, &family_key),
                members,
            )
        })
        .collect();
    families.sort_by(|left, right| left.root_path.cmp(&right.root_path));
    families
}

/// Keep only families whose member at the probed stem is the same
/// reference under the identity rules of the input route.
fn families_for_stem_identity(
    config: &Config,
    rows: &[RefRow],
    stem: &str,
    identity: StemIdentity<'_>,
) -> Vec<SelectedFamily> {
    families_for_stem(rows, stem, &config.bob_dir)
        .into_iter()
        .filter(|family| {
            rows.iter().any(|row| {
                row.family == family.root_path
                    && row.superseded_by.is_none()
                    && stem_of_path(&row.path) == stem
                    && row_matches_identity(row, config, identity)
            })
        })
        .collect()
}

fn row_matches_identity(
    row: &RefRow,
    config: &Config,
    identity: StemIdentity<'_>,
) -> bool {
    match identity {
        StemIdentity::Title(title) => {
            let stem = stem_of_path(&row.path);
            let mut candidates = Vec::new();
            if let Some(source_pdf) = &row.source_pdf {
                candidates.push(config.bob_dir.join(source_pdf));
            }
            if let Some(ref_type) = &row.ref_type {
                let filename = format!("{stem}.pdf");
                candidates.push(config.lib_dir.join(ref_type).join(&filename));
                candidates.push(config.xlib_dir.join(ref_type).join(filename));
            }
            candidates
                .iter()
                .any(|path| super::stamp::title_matches_pdf(title, path))
        }
        StemIdentity::SourceNote(source_note) => {
            super::birth::candidate_source_note(row, config).is_some_and(
                |candidate| {
                    super::birth::normalize_source_note(&candidate)
                        == super::birth::normalize_source_note(source_note)
                },
            )
        }
    }
}

fn family_root_stem(rows: &[RefRow], family_key: &str) -> String {
    rows.iter()
        .find(|row| row.path == family_key)
        .map(|row| {
            crate::native::ref_library::lineage::family_root_stem(
                &row.path,
                &row.earlier,
            )
        })
        .unwrap_or_else(|| stem_of_path(family_key))
}

/// Pick one family out of candidates, honoring an explicit `-N`: with no
/// name a single family wins and several stay ambiguous; with a name the
/// one family holding that member stem wins without joining families.
pub(super) fn disambiguate(
    mut families: Vec<SelectedFamily>,
    name: Option<&str>,
) -> Result<SelectedFamily, CommandError> {
    if families.is_empty() {
        return Err(CommandError::new("internal: no capture family to select"));
    }
    let Some(name) = name else {
        if families.len() == 1 {
            return Ok(families.pop().expect("one family"));
        }
        return Err(ambiguity_error(&families));
    };
    let mut hits = Vec::new();
    for (index, family) in families.iter().enumerate() {
        if let Some(member) = family.member_named(name) {
            hits.push((index, member));
        }
    }
    match hits.as_slice() {
        [(index, member)] => {
            let mut family = families.remove(*index);
            family.selected = *member;
            Ok(family)
        }
        [] if families.len() == 1 => Err(reread_name_error(&families[0], name)),
        _ => Err(ambiguity_error(&families)),
    }
}

/// `-N` must name a member of the selected family.
pub(super) fn validate_reread_name(
    family: &SelectedFamily,
    name: &str,
) -> Result<(), CommandError> {
    if family.member_named(name).is_some() {
        return Ok(());
    }
    Err(reread_name_error(family, name))
}

/// `-N <name>` names nothing in this family (exit 2).
pub(super) fn reread_name_error(
    family: &SelectedFamily,
    name: &str,
) -> CommandError {
    CommandError::new(format!(
        "-N {name} is not a reading of {}; readings are named {}__vN",
        family.root_stem, family.root_stem,
    ))
    .with_exit_code(2)
}

/// `-o` on a captured reread (exit 2): the next reading has a fixed name.
pub(super) fn reread_output_error(next_stem: &str) -> CommandError {
    CommandError::new(format!("a reread is named {next_stem}; drop -o"))
        .with_exit_code(2)
}

/// Several unrelated families share the capture: list every member path
/// and ask for `-N <stem>`.
pub(super) fn ambiguity_error(families: &[SelectedFamily]) -> CommandError {
    let mut paths: Vec<(&str, Option<u32>)> = families
        .iter()
        .flat_map(|family| {
            family
                .members
                .iter()
                .map(|member| (member.path.as_str(), member.reading))
        })
        .collect();
    paths.sort();
    let mut lines = vec![
        "multiple references share this source; pass -N <stem> to choose one"
            .to_string(),
    ];
    for (path, reading) in paths {
        match reading {
            Some(number) => {
                lines.push(format!("  {path} (reading v{number})"));
            }
            None => lines.push(format!("  {path}")),
        }
    }
    lines.push(
        "hint: -N <stem> names the family member to read again".to_string(),
    );
    CommandError::new(lines.join("\n"))
}

/// Allocate `<root>__vN` from `max_reading + 1` (at least 2), skipping
/// occupied candidates through 999. A family's later stem never becomes
/// the next root, and rereads never fall back to the generic `_N` rename:
/// exhaustion is an explicit refusal. `occupied` reports whether a
/// candidate stem is already reserved (see
/// [`super::stamp::DefaultTargetOccupancy`]).
pub(super) fn allocate_next_stem(
    root_stem: &str,
    max_reading: u32,
    occupied: &dyn Fn(&str) -> bool,
) -> Result<String, CommandError> {
    let start = max_reading.saturating_add(1).max(2);
    for number in start..=999 {
        let candidate = format!("{root_stem}__v{number}");
        if !occupied(&candidate) {
            return Ok(candidate);
        }
    }
    Err(CommandError::new(format!(
        "no free reread name for {root_stem}; __v{start} through __v999 are all taken"
    )))
}

/// Allocate the next reread stem for a selected family against the live
/// vault: whole-bundle reservation across every type under `ref/`,
/// `lib/`, and `xlib/`.
pub(super) fn allocate_for_family(
    config: &Config,
    family: &SelectedFamily,
) -> Result<String, CommandError> {
    let ref_type = family.newest().ref_type.as_deref().unwrap_or("papers");
    let occupancy =
        super::stamp::DefaultTargetOccupancy::build(config, ref_type)?;
    allocate_next_stem(&family.root_stem, family.max_reading, &|stem| {
        !occupancy.occupants(stem).is_empty()
    })
}

/// Resolve a single-source URL reread against the live library.
///
/// `wanted` holds the complete normalized URL-key set for the capture
/// (one key for single sources; collections pass the whole set and stay
/// on the collections path). Returns `Ok(None)` when no filed family
/// claims the keys: a bare force is an ordinary create, a queued-only
/// match keeps the old replacement permission, and legacy-only hits warn
/// elsewhere. Returns `Ok(Some(preflight))` for a selected family under
/// reread force, after `-N`/`-o` validation and whole-bundle allocation.
/// Filed refusals carry the `-f` remedy with the computed next stem;
/// overwrite force selects the member for in-place replacement.
pub(super) fn preflight_for_url(
    config: &Config,
    rows: &[RefRow],
    recorded: &[RecordedSource],
    wanted: &BTreeSet<String>,
    name: Option<&str>,
    output: Option<&Path>,
    force: Option<super::force::ForceMode>,
    title_fallback: &str,
) -> Result<Option<RereadPreflight>, CommandError> {
    match resolve_for_url_keys(rows, recorded, wanted, &config.bob_dir) {
        RecaptureMatch::NoMatch | RecaptureMatch::QueuedOnly { .. } => Ok(None),
        RecaptureMatch::Selected { family } => url_reread_from_selection(
            config,
            family,
            name,
            output,
            force,
            title_fallback,
            Some(wanted),
            false,
            true,
        ),
        RecaptureMatch::Ambiguous { families } => {
            let family = disambiguate(families, name)?;
            url_reread_from_selection(
                config,
                family,
                name,
                output,
                force,
                title_fallback,
                Some(wanted),
                false,
                true,
            )
        }
        RecaptureMatch::PartialOverlap {
            wanted,
            overlapping,
        } => Err(partial_overlap_single_error(&wanted, &overlapping)),
    }
}

/// Resolve a single URL against the live library, selecting the filed
/// member for in-place overwrite when `--force=overwrite` names one.
/// Exact-set collections use this route too, preserving their set match.
pub(super) fn preflight_for_url_outcome(
    config: &Config,
    rows: &[RefRow],
    recorded: &[RecordedSource],
    wanted: &BTreeSet<String>,
    name: Option<&str>,
    output: Option<&Path>,
    force: Option<super::force::ForceMode>,
    title_fallback: &str,
) -> Result<CaptureOutcome, CommandError> {
    match resolve_for_url_keys(rows, recorded, wanted, &config.bob_dir) {
        RecaptureMatch::NoMatch | RecaptureMatch::QueuedOnly { .. } => {
            Ok(CaptureOutcome::Fresh)
        }
        RecaptureMatch::Selected { family } => url_selection_from_selection(
            config,
            family,
            name,
            output,
            force,
            title_fallback,
            Some(wanted),
            false,
            true,
            true,
        ),
        RecaptureMatch::Ambiguous { families } => {
            let family = disambiguate(families, name)?;
            url_selection_from_selection(
                config,
                family,
                name,
                output,
                force,
                title_fallback,
                Some(wanted),
                false,
                true,
                true,
            )
        }
        RecaptureMatch::PartialOverlap {
            wanted,
            overlapping,
        } => Err(partial_overlap_single_error(&wanted, &overlapping)),
    }
}

/// Refuse an overwrite whose fetched redirect now claims another family.
/// A final URL inside the selected family's own key set (for example an
/// arXiv alias) still proceeds; anything already captured elsewhere
/// refuses before publication without retargeting the overwrite.
pub(super) fn refuse_redirected_overwrite(
    config: &Config,
    requested_dedupe: &str,
    final_url: &str,
    selection: &OverwriteSelection,
) -> std::result::Result<(), CommandError> {
    if final_url.trim().is_empty() {
        return Ok(());
    }
    let Ok(cleaned) = super::clip_url::validate_and_clean(final_url) else {
        return Ok(());
    };
    let final_dedupe = cleaned.dedupe_key;
    if final_dedupe == requested_dedupe {
        return Ok(());
    }
    if selection.family.source_keys.contains(&final_dedupe) {
        return Ok(());
    }
    let Ok(recorded) = super::sources::collect_recorded_source_urls(config)
        .map_err(|error| CommandError::new(error.message.clone()))
    else {
        return Ok(());
    };
    let Some(hit) = recorded.iter().find(|hit| {
        hit.dedupe_key == final_dedupe
            && ((hit.is_ref_note && hit.source_pdf.is_some())
                || !hit.is_ref_note)
    }) else {
        return Ok(());
    };
    let relative = hit
        .path
        .strip_prefix(&config.bob_dir)
        .map(|relative| relative.to_string_lossy().replace('\\', "/"))
        .unwrap_or_else(|_| hit.path.display().to_string());
    if selection
        .family
        .members
        .iter()
        .any(|member| member.path == relative)
    {
        return Ok(());
    }
    Err(CommandError::new(format!(
        "redirected to {final_url} which is already captured as {relative}; nothing written"
    )))
}

/// Resolve a destructive in-place overwrite without allocating a reread
/// stem. Unlike [`preflight_for_url`], the explicit `-N` selection names
/// the member to replace and `__v999` exhaustion is irrelevant.
pub(super) fn select_overwrite_for_url(
    rows: &[RefRow],
    recorded: &[RecordedSource],
    wanted: &BTreeSet<String>,
    bob_dir: &Path,
    name: Option<&str>,
) -> Result<Option<SelectedFamily>, CommandError> {
    let family = match resolve_for_url_keys(rows, recorded, wanted, bob_dir) {
        RecaptureMatch::NoMatch | RecaptureMatch::QueuedOnly { .. } => {
            return Ok(None);
        }
        RecaptureMatch::Selected { family } => family,
        RecaptureMatch::Ambiguous { families } => {
            select_overwrite_family(families, name)?
        }
        RecaptureMatch::PartialOverlap {
            wanted,
            overlapping,
        } => return Err(partial_overlap_single_error(&wanted, &overlapping)),
    };
    if let Some(name) = name {
        validate_reread_name(&family, name)?;
        let selected = family
            .member_named(name)
            .expect("validated overwrite member");
        let mut family = family;
        family.selected = selected;
        return Ok(Some(family));
    }
    Ok(Some(family))
}

fn select_overwrite_family(
    families: Vec<SelectedFamily>,
    name: Option<&str>,
) -> Result<SelectedFamily, CommandError> {
    if let Some(name) = name
        && !families
            .iter()
            .any(|family| family.member_named(name).is_some())
    {
        let choices = families
            .iter()
            .flat_map(|family| {
                family.members.iter().map(|member| member.stem.clone())
            })
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>();
        return Err(CommandError::new(format!(
            "-N {name} is not a reading of this source; choose one of {}",
            choices.join(", "),
        ))
        .with_exit_code(2));
    }
    disambiguate(families, name)
}

/// Resolve a title-identity overwrite. A free `-N` remains a fresh
/// capture; a name held by a family selects that member for replacement.
pub(super) fn select_overwrite_for_stem(
    config: &Config,
    rows: &[RefRow],
    stem: &str,
    name: Option<&str>,
    identity: StemIdentity<'_>,
) -> Result<Option<SelectedFamily>, CommandError> {
    if let Some(name) = name {
        let mut named =
            families_for_stem_identity(config, rows, name, identity);
        if named.is_empty() {
            if name != stem {
                return Ok(None);
            }
        } else {
            if name != stem {
                for family in
                    families_for_stem_identity(config, rows, stem, identity)
                {
                    if !named
                        .iter()
                        .any(|known| known.root_path == family.root_path)
                    {
                        named.push(family);
                    }
                }
            }
            let mut family = disambiguate(named, Some(name))?;
            family.selected = family
                .member_named(name)
                .expect("disambiguation selected the named member");
            return Ok(Some(family));
        }
    }
    let families = families_for_stem_identity(config, rows, stem, identity);
    if families.is_empty() {
        return Ok(None);
    }
    Ok(Some(disambiguate(families, name)?))
}

/// Resolve a title-route reread or overwrite by stem: Markdown, local PDF,
/// and same-day vault snapshots probe their planned stem against live
/// families. A free stem is an ordinary capture (`Fresh`); a claimed
/// stem follows the same force/`-N`/`-o` rules as URL routes, except
/// overwrite is enabled and selects the named (or newest) member in
/// place. An explicit `-N` naming a member of any family selects it;
/// naming a free stem is an ordinary new capture and never joins a family.
pub(super) fn preflight_for_stem(
    config: &Config,
    rows: &[RefRow],
    stem: &str,
    name: Option<&str>,
    output: Option<&Path>,
    force: Option<super::force::ForceMode>,
    title_fallback: &str,
    identity: StemIdentity<'_>,
) -> Result<CaptureOutcome, CommandError> {
    if let Some(name) = name {
        let mut named =
            families_for_stem_identity(config, rows, name, identity);
        if !named.is_empty() {
            // `-N` names a family member: include the stem's families so
            // an ambiguous name across families still asks, then select.
            if name != stem {
                for family in
                    families_for_stem_identity(config, rows, stem, identity)
                {
                    if !named
                        .iter()
                        .any(|known| known.root_path == family.root_path)
                    {
                        named.push(family);
                    }
                }
            }
            let family = disambiguate(named, Some(name))?;
            return finish_selection(
                config,
                family,
                Some(name),
                output,
                force,
                title_fallback,
                None,
                true,
                false,
                true,
            );
        }
        // A free explicit `-N` is an ordinary new capture.
        if name != stem {
            return Ok(CaptureOutcome::Fresh);
        }
    }
    let families = families_for_stem_identity(config, rows, stem, identity);
    if families.is_empty() {
        return Ok(CaptureOutcome::Fresh);
    }
    let family = disambiguate(families, name)?;
    finish_selection(
        config,
        family,
        name,
        output,
        force,
        title_fallback,
        None,
        true,
        false,
        true,
    )
}

/// Map a URL-route selection onto a capture outcome. Single-URL routes
/// and exact-set collections enable in-place overwrite.
fn url_selection_from_selection(
    config: &Config,
    family: SelectedFamily,
    name: Option<&str>,
    output: Option<&Path>,
    force: Option<super::force::ForceMode>,
    title_fallback: &str,
    expected_keys: Option<&BTreeSet<String>>,
    title_route: bool,
    attach_available: bool,
    overwrite_enabled: bool,
) -> Result<CaptureOutcome, CommandError> {
    finish_selection(
        config,
        family,
        name,
        output,
        force,
        title_fallback,
        expected_keys,
        title_route,
        attach_available,
        overwrite_enabled,
    )
}

fn url_reread_from_selection(
    config: &Config,
    family: SelectedFamily,
    name: Option<&str>,
    output: Option<&Path>,
    force: Option<super::force::ForceMode>,
    title_fallback: &str,
    expected_keys: Option<&BTreeSet<String>>,
    title_route: bool,
    attach_available: bool,
) -> Result<Option<RereadPreflight>, CommandError> {
    match url_selection_from_selection(
        config,
        family,
        name,
        output,
        force,
        title_fallback,
        expected_keys,
        title_route,
        attach_available,
        false,
    )? {
        CaptureOutcome::Reread(preflight) => Ok(Some(preflight)),
        CaptureOutcome::Fresh | CaptureOutcome::Overwrite(_) => {
            unreachable!(
                "URL finish_selection refuses overwrite and never returns Fresh"
            )
        }
    }
}

/// Finish a selected family: validate `-N`/`-o`, then either allocate a
/// reread stem, select the member for in-place overwrite, or refuse.
/// No force carries reread and overwrite remedies; overwrite selects the
/// member without allocating `__vN`.
fn finish_selection(
    config: &Config,
    family: SelectedFamily,
    name: Option<&str>,
    output: Option<&Path>,
    force: Option<super::force::ForceMode>,
    title_fallback: &str,
    expected_keys: Option<&BTreeSet<String>>,
    title_route: bool,
    attach_available: bool,
    overwrite_enabled: bool,
) -> Result<CaptureOutcome, CommandError> {
    if expected_keys
        .is_some_and(|wanted| !family_has_exact_keys(&family, wanted))
    {
        let wanted = expected_keys
            .expect("checked expected keys")
            .iter()
            .cloned()
            .collect::<Vec<_>>();
        return Err(partial_overlap_single_error(
            &wanted,
            &[overlap_group(&family, false)],
        ));
    }
    let mut family = family;
    if let Some(name) = name {
        validate_reread_name(&family, name)?;
        family.selected =
            family.member_named(name).expect("validated family member");
    }
    match force {
        None => {
            let next = allocate_for_family(config, &family)?;
            Err(filed_refusal(
                config,
                &family,
                &next,
                title_fallback,
                title_route,
                attach_available,
            ))
        }
        Some(super::force::ForceMode::Overwrite) if overwrite_enabled => {
            if output.is_some() {
                return Err(overwrite_output_error());
            }
            Ok(CaptureOutcome::Overwrite(OverwriteSelection {
                defaults: family_defaults(family.selected_member()),
                family,
            }))
        }
        Some(super::force::ForceMode::Overwrite) => Err(CommandError::new(
            "in-place overwrite must use the overwrite capture route",
        )
        .with_exit_code(2)),
        Some(super::force::ForceMode::Reread) => {
            let next = allocate_for_family(config, &family)?;
            if output.is_some() {
                return Err(reread_output_error(&next));
            }
            let defaults = family_defaults(family.newest());
            Ok(CaptureOutcome::Reread(RereadPreflight {
                plan: RereadPlan { family, stem: next },
                defaults,
            }))
        }
    }
}

fn overwrite_output_error() -> CommandError {
    CommandError::new(
        "-o cannot select a path for an in-place overwrite\nhint: drop -o; overwrite keeps the filed reference path",
    )
    .with_exit_code(2)
}

/// The filed refusal for a selected family: every copy newest-first with
/// reading numbers, plus the `-f` remedy naming the allocated next stem.
fn filed_refusal(
    config: &Config,
    family: &SelectedFamily,
    next_stem: &str,
    title_fallback: &str,
    title_route: bool,
    attach_available: bool,
) -> CommandError {
    let title = family
        .members
        .iter()
        .find_map(|member| note_title(&config.bob_dir, &member.path))
        .unwrap_or_else(|| title_fallback.to_string());
    let mut copies = Vec::new();
    for member in &family.members {
        let path = config.bob_dir.join(&member.path);
        let mut copy = super::duplicate::copy_from_occupant(config, &path);
        copy.reading = member.reading;
        copies.push(copy);
    }
    let mut remedies = Vec::new();
    if title_route {
        remedies.push(super::duplicate::Remedy::new(
            "-N <name>",
            "capture under a different name",
        ));
    }
    remedies.push(super::duplicate::Remedy::new(
        "-f",
        format!("read it again as {next_stem}"),
    ));
    if attach_available {
        remedies.push(super::duplicate::Remedy::new(
            "-L",
            "add --listen to narrate it and attach the episode to the existing capture",
        ));
    }
    let overwrite_target = family.selected_member().path.clone();
    let refusal =
        super::duplicate::AlreadyCaptured::new(title, copies, remedies)
            .with_next_stem(next_stem)
            .with_overwrite_target(overwrite_target);
    let styler = crate::native::style::Styler::detect();
    CommandError::new(refusal.render(&styler))
}

/// One URL of a single-source capture overlaps a filed collection (or a
/// filed single whose key set differs): refuse before any fetch under
/// every force mode, naming the overlapping members.
fn partial_overlap_single_error(
    wanted: &[String],
    overlapping: &[OverlapGroup],
) -> CommandError {
    let wanted_set: BTreeSet<&str> =
        wanted.iter().map(String::as_str).collect();
    let overlapping_keys: BTreeSet<&str> = overlapping
        .iter()
        .flat_map(|group| group.keys.iter().map(String::as_str))
        .filter(|key| wanted_set.contains(key))
        .collect();
    let mut lines = vec![format!(
        "{} of {} URLs are already in the library",
        overlapping_keys.len(),
        wanted.len(),
    )];
    let mut groups = overlapping.to_vec();
    groups.sort_by(|left, right| left.root_stem.cmp(&right.root_stem));
    for group in &groups {
        let mut paths = group.member_paths.clone();
        paths.sort();
        for path in paths {
            lines.push(format!("  {path}"));
        }
    }
    let stem = groups
        .first()
        .map(|group| group.root_stem.as_str())
        .unwrap_or("reference");
    lines.push(format!(
        "hint: -f recaptures only the exact URL set of {stem}"
    ));
    CommandError::new(lines.join("\n"))
}

/// Best-effort ref-note title for the filed refusal.
fn note_title(bob_dir: &Path, relative: &str) -> Option<String> {
    let contents = std::fs::read_to_string(bob_dir.join(relative)).ok()?;
    let (frontmatter, _) = super::frontmatter::split_frontmatter(&contents)?;
    crate::native::ref_library::ParsedFrontmatter::parse(&frontmatter)
        .get_str("title")
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

/// Plan the intake target for an enabled reread at its allocated
/// `<root>__vN` stem. The stem was free at preflight; a whole-bundle
/// recheck refuses when another host filed anything under it since, but
/// keeps the old queued-intake replacement permission (a queued-only
/// occupant is replaced through the normal same-intake path). Never
/// suffixes: a reread has a fixed name.
pub(super) fn plan_reread_target(
    config: &Config,
    stem: &str,
    ref_type: &str,
    identity: &super::stamp::DefaultTargetIdentity<'_>,
) -> Result<super::stamp::TargetPlan, CommandError> {
    let occupancy =
        super::stamp::DefaultTargetOccupancy::build(config, ref_type)?;
    let mut occupants = occupancy.occupants(stem);
    occupants.sort();
    if !occupants.is_empty() {
        let expected_queued =
            config.xlib_dir.join(ref_type).join(format!("{stem}.pdf"));
        let queued_only = occupants.len() == 1
            && occupants
                .first()
                .is_some_and(|only| *only == expected_queued);
        if !queued_only {
            let list = occupants
                .iter()
                .map(|path| path.display().to_string())
                .collect::<Vec<_>>()
                .join(", ");
            return Err(CommandError::new(format!(
                "reread {stem} is already filed ({list}); nothing written"
            )));
        }
    }
    let plan = super::stamp::plan_default_target(
        config,
        std::ffi::OsStr::new(stem),
        ref_type,
        true,
        identity,
    )?;
    if plan.stem != stem {
        return Err(CommandError::new(format!(
            "reread {stem} is already filed; nothing written"
        )));
    }
    Ok(plan)
}

/// When a reread is narrated (`-f` + `-L`) and the newest reading
/// already has an episode, say so: earlier audio is never inherited, and
/// the hint names the newest reading's number and its audio path.
pub(super) fn print_reread_listen_hint(preflight: &RereadPreflight) {
    let newest = preflight.plan.family.newest();
    if !newest.has_audio {
        return;
    }
    let reading = newest.reading.unwrap_or(1);
    match &newest.audio_path {
        Some(path) => println!(
            "hint: earlier reading v{reading} keeps its episode at {path}; the new reading is narrated fresh"
        ),
        None => println!(
            "hint: earlier reading v{reading} already has an episode; the new reading is narrated fresh"
        ),
    }
}

/// On a plain reread (no `-L` or `-a`) when any earlier reading is
/// narrated, point at the first (newest) narrated member: earlier audio
/// is never inherited, so the new reading needs `-L` to be narrated too.
pub(super) fn print_reread_narrated_hint(preflight: &RereadPreflight) {
    let Some(narrated) = preflight
        .plan
        .family
        .members
        .iter()
        .find(|member| member.has_audio)
    else {
        return;
    };
    let reading = narrated.reading.unwrap_or(1);
    match &narrated.audio_path {
        Some(path) => println!(
            "hint: v{reading} is narrated ({path}); add -L to narrate this reading"
        ),
        None => println!(
            "hint: v{reading} is narrated; add -L to narrate this reading"
        ),
    }
}

/// Route a reread's audio hint through one helper: a narrated new
/// reading keeps the existing `-L` hint, while a plain reread without
/// an explicit `--audio` names the newest narrated reading (if any).
pub(super) fn print_reread_audio_hint(
    preflight: &RereadPreflight,
    narrates_new: bool,
) {
    if narrates_new {
        print_reread_listen_hint(preflight);
    } else {
        print_reread_narrated_hint(preflight);
    }
}

/// Offer an in-place overwrite when the newest reading is still unread.
/// Called alongside the reread line by real and dry-run route reporters.
pub(super) fn print_overwrite_hint(preflight: &RereadPreflight) {
    let newest = preflight.plan.family.newest();
    if newest.status.as_deref() == Some("ready") && newest.highlights == Some(0)
    {
        println!(
            "hint: {} is still ready with 0 highlights; -f overwrite would replace it instead",
            newest.stem
        );
    }
}

/// Redirect guard for rereads: when the fetched final URL differs from
/// the requested key and is recorded by a filed or queued reference
/// outside the selected family, refuse before writing (exit 1).
/// The post-fetch rechecks otherwise reuse the pre-fetch key and only
/// catch races; this catches a redirect that now claims another family.
pub(super) fn refuse_redirected_reread(
    config: &Config,
    requested_dedupe: &str,
    final_url: &str,
    preflight: &RereadPreflight,
) -> std::result::Result<(), CommandError> {
    if final_url.trim().is_empty() {
        return Ok(());
    }
    let Ok(cleaned) = super::clip_url::validate_and_clean(final_url) else {
        return Ok(());
    };
    let final_dedupe = cleaned.dedupe_key;
    if final_dedupe == requested_dedupe {
        return Ok(());
    }
    // Inside the selected family (for example an arXiv alias) is fine.
    if preflight.plan.family.source_keys.contains(&final_dedupe) {
        return Ok(());
    }
    let Ok(recorded) = super::sources::collect_recorded_source_urls(config)
        .map_err(|error| CommandError::new(error.message.clone()))
    else {
        return Ok(());
    };
    let Some(hit) = recorded.iter().find(|hit| {
        hit.dedupe_key == final_dedupe
            && ((hit.is_ref_note && hit.source_pdf.is_some())
                || !hit.is_ref_note)
    }) else {
        return Ok(());
    };
    // A hit that is itself a member of the selected family is fine;
    // anything else refuses before writing.
    let relative = hit
        .path
        .strip_prefix(&config.bob_dir)
        .map(|relative| relative.to_string_lossy().replace('\\', "/"))
        .unwrap_or_else(|_| hit.path.display().to_string());
    if preflight
        .plan
        .family
        .members
        .iter()
        .any(|member| member.path == relative)
    {
        return Ok(());
    }
    Err(CommandError::new(format!(
        "redirected to {final_url} which is already captured as {relative}; nothing written"
    )))
}

/// The `reread:` output line for a finished plan.
pub(super) fn reread_report_line(
    old_stem: &str,
    new_stem: &str,
    reading: u32,
    status: Option<&str>,
    highlights: Option<u64>,
) -> String {
    let mut detail = format!("v{reading}");
    if let Some(status) =
        status.map(str::trim).filter(|value| !value.is_empty())
    {
        detail.push_str(&format!(" {status}"));
    }
    if let Some(count) = highlights {
        let label = if count == 1 {
            "1 highlight".to_string()
        } else {
            format!("{count} highlights")
        };
        detail.push_str(&format!(" · {label}"));
    }
    format!("reread: {old_stem} → {new_stem} ({detail})")
}

/// Live family keyed by the index family root path, or `None` when that
/// root has no remaining live members. Overwrite re-derives selection
/// from this after the locked pre-sync.
pub(super) fn family_for_root(
    rows: &[RefRow],
    root_path: &str,
    bob_dir: &Path,
) -> Option<SelectedFamily> {
    let members: Vec<FamilyMember> = rows
        .iter()
        .filter(|row| row.family == root_path && row.superseded_by.is_none())
        .map(|row| member_from_row(row, bob_dir))
        .collect();
    if members.is_empty() {
        return None;
    }
    Some(select_family(
        root_path,
        family_root_stem(rows, root_path),
        members,
    ))
}

/// Assemble one family: members newest first, the newest selected.
/// The root path comes from the index family key (never re-derived from
/// a filename), so a later stem never becomes the next root.
fn select_family(
    root_path: &str,
    root_stem: String,
    members: Vec<FamilyMember>,
) -> SelectedFamily {
    let mut members = members;
    members.sort_by(|left, right| {
        left.reading
            .unwrap_or(1)
            .cmp(&right.reading.unwrap_or(1))
            .reverse()
            .then_with(|| left.path.cmp(&right.path))
    });
    let source_keys: BTreeSet<String> = members
        .iter()
        .flat_map(|member| member.source_keys.iter().cloned())
        .collect();
    let max_reading = members
        .iter()
        .map(|member| member.reading.unwrap_or(1))
        .max()
        .unwrap_or(1);
    SelectedFamily {
        root_stem,
        root_path: root_path.to_string(),
        members,
        selected: 0,
        source_keys,
        max_reading,
    }
}

/// The URL-derived identity keys used for exact-set comparison: the
/// `arxiv:`, `doi:`, and `raw:` aliases identify the same source and
/// never split a family.
fn comparison_keys(row: &RefRow) -> Vec<String> {
    let mut keys: Vec<String> = row
        .identity
        .keys
        .iter()
        .filter(|key| {
            !key.starts_with("arxiv:")
                && !key.starts_with("doi:")
                && !key.starts_with("raw:")
        })
        .cloned()
        .collect();
    keys.sort();
    keys.dedup();
    keys
}

fn overlap_group(family: &SelectedFamily, is_legacy: bool) -> OverlapGroup {
    OverlapGroup {
        root_stem: family.root_stem.clone(),
        member_paths: family
            .members
            .iter()
            .map(|member| member.path.clone())
            .collect(),
        keys: family.source_keys.iter().cloned().collect(),
        is_legacy,
    }
}

fn family_has_exact_keys(
    family: &SelectedFamily,
    wanted: &BTreeSet<String>,
) -> bool {
    family.source_keys == *wanted
        && family.members.iter().all(|member| {
            member.source_keys.iter().cloned().collect::<BTreeSet<_>>()
                == *wanted
        })
}

fn member_from_row(row: &RefRow, bob_dir: &Path) -> FamilyMember {
    FamilyMember {
        path: row.path.clone(),
        stem: stem_of_path(&row.path),
        reading: row.reading,
        ref_type: row.ref_type.clone(),
        task_path: row.task.as_ref().map(|task| task.path.clone()),
        task_block: row.task.as_ref().and_then(|task| task.block_id.clone()),
        has_audio: row
            .audio
            .as_deref()
            .is_some_and(|value| !value.trim().is_empty()),
        audio_path: row
            .audio
            .clone()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty()),
        status: row.status.clone(),
        captured: row.captured.clone(),
        highlights: note_highlights(bob_dir, &row.path),
        source_keys: comparison_keys(row),
        is_queued: false,
    }
}

fn queued_member(hit: &RecordedSource, bob_dir: &Path) -> FamilyMember {
    let relative = vault_relative(&hit.path, bob_dir);
    FamilyMember {
        stem: stem_of_path(&relative),
        path: relative.clone(),
        reading: None,
        ref_type: queued_ref_type(&relative),
        task_path: None,
        task_block: None,
        has_audio: false,
        audio_path: None,
        status: None,
        captured: None,
        highlights: None,
        source_keys: vec![hit.dedupe_key.clone()],
        is_queued: true,
    }
}

fn unindexed_member(hit: &RecordedSource, relative: &str) -> FamilyMember {
    FamilyMember {
        path: relative.to_string(),
        stem: stem_of_path(relative),
        reading: None,
        ref_type: None,
        task_path: None,
        task_block: None,
        has_audio: hit.has_audio,
        audio_path: None,
        status: None,
        captured: None,
        highlights: None,
        source_keys: vec![hit.dedupe_key.clone()],
        is_queued: false,
    }
}

/// The type directory of a queued intake PDF (`xlib/<type>/…`).
fn queued_ref_type(relative: &str) -> Option<String> {
    let rest = relative.strip_prefix("xlib/")?;
    let component = rest.split('/').next()?;
    (!component.is_empty()).then(|| component.to_string())
}

fn stem_of_path(path: &str) -> String {
    Path::new(path)
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or(path)
        .to_string()
}

/// Vault-relative forward-slash path, mirroring the library index.
fn vault_relative(path: &Path, bob_dir: &Path) -> String {
    path.strip_prefix(bob_dir)
        .map(|relative| relative.to_string_lossy().replace('\\', "/"))
        .unwrap_or_else(|_| path.to_string_lossy().into_owned())
}

/// Best-effort `highlights_count` for the `reread:` report line.
fn note_highlights(bob_dir: &Path, relative: &str) -> Option<u64> {
    let contents = std::fs::read_to_string(bob_dir.join(relative)).ok()?;
    let (frontmatter, _) = super::frontmatter::split_frontmatter(&contents)?;
    ParsedFrontmatter::parse(&frontmatter)
        .get_str("highlights_count")
        .and_then(|value| value.trim().parse::<u64>().ok())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn unique_root(case: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "bob-recapture-{}-{}-{case}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|duration| duration.as_nanos())
                .unwrap_or(0),
        ));
        std::fs::create_dir_all(root.join("ref/papers")).expect("ref dir");
        std::fs::create_dir_all(root.join("ref/blogs")).expect("ref dir");
        std::fs::create_dir_all(root.join("lib/papers")).expect("lib dir");
        std::fs::create_dir_all(root.join("lib/blogs")).expect("lib dir");
        std::fs::create_dir_all(root.join("xlib/papers")).expect("xlib dir");
        std::fs::create_dir_all(root.join("xlib/blogs")).expect("xlib dir");
        root
    }

    fn test_config(root: &Path) -> Config {
        Config {
            bob_dir: root.to_path_buf(),
            lib_dir: root.join("lib"),
            ref_dir: root.join("ref"),
            xlib_dir: root.join("xlib"),
        }
    }

    fn note(front_lines: &[&str], body: &str) -> String {
        format!("---\n{}\n---\n{body}", front_lines.join("\n"))
    }

    fn write_note(root: &Path, rel: &str, contents: &str) {
        let path = root.join(rel);
        std::fs::create_dir_all(path.parent().expect("note has a parent"))
            .expect("note parent dir");
        std::fs::write(path, contents).expect("write fixture note");
    }

    fn index_rows(config: &Config) -> Vec<RefRow> {
        let library = LibraryConfig {
            bob_dir: config.bob_dir.clone(),
            ref_dir: config.ref_dir.clone(),
            xlib_dir: config.xlib_dir.clone(),
        };
        build_index(&library).expect("test index builds").rows
    }

    fn key(url: &str) -> String {
        super::super::clip_url::validate_and_clean(url)
            .expect("fixture url")
            .dedupe_key
    }

    fn wanted(urls: &[&str]) -> BTreeSet<String> {
        urls.iter().map(|url| key(url)).collect()
    }

    fn filed_hit(root: &Path, rel: &str, url: &str) -> RecordedSource {
        RecordedSource {
            dedupe_key: key(url),
            path: root.join(rel),
            is_ref_note: true,
            source_pdf: Some("lib/papers/backing.pdf".to_string()),
            has_audio: false,
        }
    }

    fn queued_hit(root: &Path, rel: &str, url: &str) -> RecordedSource {
        RecordedSource {
            dedupe_key: key(url),
            path: root.join(rel),
            is_ref_note: false,
            source_pdf: None,
            has_audio: false,
        }
    }

    fn legacy_hit(root: &Path, rel: &str, url: &str) -> RecordedSource {
        RecordedSource {
            dedupe_key: key(url),
            path: root.join(rel),
            is_ref_note: true,
            source_pdf: None,
            has_audio: false,
        }
    }

    fn resolve(
        rows: &[RefRow],
        recorded: &[RecordedSource],
        urls: &[&str],
        bob_dir: &Path,
    ) -> RecaptureMatch {
        resolve_for_url_keys(rows, recorded, &wanted(urls), bob_dir)
    }

    const ALPHA: &str = "https://example.com/alpha";

    fn alpha_note() -> String {
        note(
            &[
                "title: Alpha",
                &format!("source_url: {ALPHA}"),
                "source_pdf: lib/papers/alpha.pdf",
                "status: ready",
                "captured: 2026-03-02",
                "highlights_count: 41",
            ],
            "# Alpha\n",
        )
    }

    #[test]
    fn single_url_selects_a_family_of_one() {
        let root = unique_root("family-of-one");
        let config = test_config(&root);
        write_note(&root, "ref/papers/alpha.md", &alpha_note());
        let rows = index_rows(&config);
        let recorded = [filed_hit(&root, "ref/papers/alpha.md", ALPHA)];
        let matched = resolve(&rows, &recorded, &[ALPHA], &root);
        let RecaptureMatch::Selected { family } = matched else {
            panic!("expected a selected family, got {matched:?}");
        };
        assert_eq!(family.root_stem, "alpha");
        assert_eq!(family.root_path, "ref/papers/alpha.md");
        assert_eq!(family.members.len(), 1);
        assert_eq!(family.max_reading, 1);
        assert_eq!(family.source_keys, wanted(&[ALPHA]));
        assert_eq!(family.newest().stem, "alpha");
        assert_eq!(family.newest().status.as_deref(), Some("ready"));
        assert_eq!(family.newest().highlights, Some(41));

        let next =
            allocate_for_family(&config, &family).expect("allocates __v2");
        assert_eq!(next, "alpha__v2");
        let plan = RereadPlan { family, stem: next };
        assert_eq!(
            plan.report_line(),
            "reread: alpha → alpha__v2 (v1 ready · 41 highlights)",
        );
    }

    #[test]
    fn family_of_three_orders_newest_first() {
        let root = unique_root("family-of-three");
        let config = test_config(&root);
        let url = "https://example.com/beta";
        write_note(
            &root,
            "ref/papers/beta.md",
            &note(
                &[
                    "title: Beta",
                    &format!("source_url: {url}"),
                    "source_pdf: lib/papers/beta.pdf",
                    "status: read",
                ],
                "# Beta\n",
            ),
        );
        write_note(
            &root,
            "ref/papers/beta__v2.md",
            &note(
                &[
                    "title: Beta",
                    &format!("source_url: {url}"),
                    "source_pdf: lib/papers/beta__v2.pdf",
                    "status: wip",
                    "earlier:",
                    "  - \"[[ref/papers/beta|v1 · 2026-01-01]]\"",
                ],
                "# Beta\n",
            ),
        );
        write_note(
            &root,
            "ref/papers/beta__v3.md",
            &note(
                &[
                    "title: Beta",
                    &format!("source_url: {url}"),
                    "source_pdf: lib/papers/beta__v3.pdf",
                    "status: ready",
                    "earlier:",
                    "  - \"[[ref/papers/beta__v2|v2 · 2026-02-01]]\"",
                    "  - \"[[ref/papers/beta|v1 · 2026-01-01]]\"",
                ],
                "# Beta\n",
            ),
        );
        let rows = index_rows(&config);
        let recorded = [
            filed_hit(&root, "ref/papers/beta.md", url),
            filed_hit(&root, "ref/papers/beta__v2.md", url),
            filed_hit(&root, "ref/papers/beta__v3.md", url),
        ];
        let matched = resolve(&rows, &recorded, &[url], &root);
        let RecaptureMatch::Selected { family } = matched else {
            panic!("expected a selected family, got {matched:?}");
        };
        let stems: Vec<&str> = family
            .members
            .iter()
            .map(|member| member.stem.as_str())
            .collect();
        assert_eq!(stems, vec!["beta__v3", "beta__v2", "beta"]);
        assert_eq!(family.max_reading, 3);
        assert_eq!(
            allocate_for_family(&config, &family).expect("allocates"),
            "beta__v4",
        );

        let default_overwrite = select_overwrite_for_url(
            &rows,
            &recorded,
            &wanted(&[url]),
            &root,
            None,
        )
        .expect("default overwrite selection");
        assert_eq!(
            default_overwrite
                .as_ref()
                .expect("filed family")
                .selected_member()
                .stem,
            "beta__v3"
        );
        let older_overwrite = select_overwrite_for_url(
            &rows,
            &recorded,
            &wanted(&[url]),
            &root,
            Some("beta"),
        )
        .expect("explicit overwrite selection");
        assert_eq!(
            older_overwrite
                .as_ref()
                .expect("filed family")
                .selected_member()
                .stem,
            "beta"
        );
        assert_eq!(
            older_overwrite.expect("filed family").max_reading,
            3,
            "selecting an older reading does not allocate a new version"
        );
    }

    #[test]
    fn dangling_middle_member_keeps_max_reading() {
        let root = unique_root("dangling-middle");
        let config = test_config(&root);
        let url = "https://example.com/gap";
        write_note(
            &root,
            "ref/papers/gap.md",
            &note(
                &[
                    "title: Gap",
                    &format!("source_url: {url}"),
                    "source_pdf: lib/papers/gap.pdf",
                    "status: read",
                ],
                "# Gap\n",
            ),
        );
        write_note(
            &root,
            "ref/papers/gap__v3.md",
            &note(
                &[
                    "title: Gap",
                    &format!("source_url: {url}"),
                    "source_pdf: lib/papers/gap__v3.pdf",
                    "status: ready",
                    "earlier:",
                    "  - \"[[ref/papers/gap__v2|v2 · 2026-02-01]]\"",
                    "  - \"[[ref/papers/gap|v1 · 2026-01-01]]\"",
                ],
                "# Gap\n",
            ),
        );
        let rows = index_rows(&config);
        let recorded = [
            filed_hit(&root, "ref/papers/gap.md", url),
            filed_hit(&root, "ref/papers/gap__v3.md", url),
        ];
        let matched = resolve(&rows, &recorded, &[url], &root);
        let RecaptureMatch::Selected { family } = matched else {
            panic!("expected a selected family, got {matched:?}");
        };
        // The dangling v2 entry still counts: the next reading is v4,
        // never a reused v2.
        assert_eq!(family.max_reading, 3);
        assert_eq!(
            allocate_for_family(&config, &family).expect("allocates"),
            "gap__v4",
        );
        // A later stem resolves but never becomes the root.
        let families = families_for_stem(&rows, "gap__v3", &root);
        assert_eq!(families.len(), 1);
        assert_eq!(families[0].root_stem, "gap");
        assert_eq!(families[0].selected_member().stem, "gap__v3");
    }

    #[test]
    fn v2_v4_occupancy_yields_v5() {
        let root = unique_root("v2-v4-occupancy");
        let config = test_config(&root);
        let url = "https://example.com/occ";
        write_note(
            &root,
            "ref/papers/occ.md",
            &note(
                &[
                    "title: Occ",
                    &format!("source_url: {url}"),
                    "source_pdf: lib/papers/occ.pdf",
                    "status: read",
                ],
                "# Occ\n",
            ),
        );
        write_note(
            &root,
            "ref/papers/occ__v2.md",
            &note(
                &[
                    "title: Occ",
                    &format!("source_url: {url}"),
                    "source_pdf: lib/papers/occ__v2.pdf",
                    "status: read",
                    "earlier:",
                    "  - \"[[ref/papers/occ|v1 · 2026-01-01]]\"",
                ],
                "# Occ\n",
            ),
        );
        write_note(
            &root,
            "ref/papers/occ__v4.md",
            &note(
                &[
                    "title: Occ",
                    &format!("source_url: {url}"),
                    "source_pdf: lib/papers/occ__v4.pdf",
                    "status: ready",
                    "earlier:",
                    "  - \"[[ref/papers/occ__v3|v3 · 2026-03-01]]\"",
                    "  - \"[[ref/papers/occ__v2|v2 · 2026-02-01]]\"",
                    "  - \"[[ref/papers/occ|v1 · 2026-01-01]]\"",
                ],
                "# Occ\n",
            ),
        );
        let rows = index_rows(&config);
        let recorded = [
            filed_hit(&root, "ref/papers/occ.md", url),
            filed_hit(&root, "ref/papers/occ__v2.md", url),
            filed_hit(&root, "ref/papers/occ__v4.md", url),
        ];
        let matched = resolve(&rows, &recorded, &[url], &root);
        let RecaptureMatch::Selected { family } = matched else {
            panic!("expected a selected family, got {matched:?}");
        };
        // The fixture's family reading metadata is explicit: 1, 2, 4.
        let readings: Vec<Option<u32>> =
            family.members.iter().map(|member| member.reading).collect();
        assert_eq!(readings, vec![Some(4), Some(2), Some(1)]);
        assert_eq!(
            allocate_for_family(&config, &family).expect("allocates"),
            "occ__v5",
        );
    }

    #[test]
    fn unrelated_same_identity_families_are_ambiguous() {
        let root = unique_root("ambiguous");
        let config = test_config(&root);
        let url = "https://example.com/shared";
        for (rel, title) in [
            ("ref/papers/gamma.md", "Gamma"),
            ("ref/blogs/delta.md", "Delta"),
        ] {
            write_note(
                &root,
                rel,
                &note(
                    &[
                        &format!("title: {title}"),
                        &format!("source_url: {url}"),
                        "source_pdf: lib/papers/backing.pdf",
                        "status: ready",
                    ],
                    "# Shared\n",
                ),
            );
        }
        let rows = index_rows(&config);
        let recorded = [
            filed_hit(&root, "ref/papers/gamma.md", url),
            filed_hit(&root, "ref/blogs/delta.md", url),
        ];
        let matched = resolve(&rows, &recorded, &[url], &root);
        let RecaptureMatch::Ambiguous { families } = matched else {
            panic!("expected ambiguity, got {matched:?}");
        };
        assert_eq!(families.len(), 2);
        let error = ambiguity_error(&families);
        assert!(error.message.contains("ref/papers/gamma.md"), "{error:?}");
        assert!(error.message.contains("ref/blogs/delta.md"), "{error:?}");
        assert!(error.message.contains("pass -N <stem>"), "{error:?}",);
        assert!(error.message.contains("hint: -N <stem>"), "{error:?}");
        assert_eq!(error.exit_code, None);

        let overwrite_error = select_overwrite_for_url(
            &rows,
            &recorded,
            &wanted(&[url]),
            &root,
            None,
        )
        .expect_err("unselected unrelated families remain ambiguous");
        assert_eq!(overwrite_error.exit_code, None);
        let invalid_overwrite_name = select_overwrite_for_url(
            &rows,
            &recorded,
            &wanted(&[url]),
            &root,
            Some("not-a-reading"),
        )
        .expect_err("a nonmember -N is refused");
        assert_eq!(invalid_overwrite_name.exit_code, Some(2));
        let overwrite_selected = select_overwrite_for_url(
            &rows,
            &recorded,
            &wanted(&[url]),
            &root,
            Some("delta"),
        )
        .expect("explicit member selector disambiguates overwrite");
        assert_eq!(
            overwrite_selected
                .expect("filed family")
                .selected_member()
                .stem,
            "delta"
        );

        // A valid member selector disambiguates without joining families.
        let selected =
            disambiguate(families, Some("delta")).expect("selects delta");
        assert_eq!(selected.root_stem, "delta");
        assert_eq!(selected.members.len(), 1);
        assert!(disambiguate(vec![selected], None).is_ok());
    }

    #[test]
    fn same_stem_across_types_stays_ambiguous() {
        let root = unique_root("same-stem");
        let config = test_config(&root);
        write_note(
            &root,
            "ref/papers/sigma.md",
            &note(
                &[
                    "title: Sigma Paper",
                    "source_url: https://example.com/sigma-paper",
                    "source_pdf: lib/papers/sigma.pdf",
                    "status: ready",
                ],
                "# Sigma\n",
            ),
        );
        write_note(
            &root,
            "ref/blogs/sigma.md",
            &note(
                &[
                    "title: Sigma Blog",
                    "source_url: https://example.com/sigma-blog",
                    "source_pdf: lib/blogs/sigma.pdf",
                    "status: ready",
                ],
                "# Sigma\n",
            ),
        );
        let rows = index_rows(&config);
        let families = families_for_stem(&rows, "sigma", &root);
        assert_eq!(families.len(), 2);
        let error = disambiguate(families, None).expect_err("stays ambiguous");
        assert!(error.message.contains("ref/papers/sigma.md"));
        assert!(error.message.contains("ref/blogs/sigma.md"));
    }

    #[test]
    fn name_outside_the_family_refuses_with_expected_pattern() {
        let root = unique_root("name-guard");
        let config = test_config(&root);
        write_note(&root, "ref/papers/alpha.md", &alpha_note());
        let rows = index_rows(&config);
        let recorded = [filed_hit(&root, "ref/papers/alpha.md", ALPHA)];
        let RecaptureMatch::Selected { family } =
            resolve(&rows, &recorded, &[ALPHA], &root)
        else {
            panic!("expected a selected family");
        };
        assert!(validate_reread_name(&family, "alpha").is_ok());
        let error =
            validate_reread_name(&family, "other").expect_err("refuses");
        assert_eq!(
            error.message,
            "-N other is not a reading of alpha; readings are named alpha__vN",
        );
        assert_eq!(error.exit_code, Some(2));
    }

    #[test]
    fn output_on_a_captured_reread_names_the_allocated_stem() {
        let error = reread_output_error("alpha__v2");
        assert_eq!(error.message, "a reread is named alpha__v2; drop -o",);
        assert_eq!(error.exit_code, Some(2));
    }

    #[test]
    fn one_url_into_a_collection_is_a_partial_overlap() {
        let root = unique_root("partial");
        let config = test_config(&root);
        let one = "https://example.com/part-one";
        let two = "https://example.com/part-two";
        write_note(
            &root,
            "ref/blogs/coll.md",
            &note(
                &[
                    "title: Coll",
                    &format!("source_url: [{one}, {two}]"),
                    "source_pdf: lib/blogs/coll.pdf",
                    "status: ready",
                ],
                "# Coll\n",
            ),
        );
        let rows = index_rows(&config);
        let recorded = [filed_hit(&root, "ref/blogs/coll.md", one)];
        let matched = resolve(&rows, &recorded, &[one], &root);
        let RecaptureMatch::PartialOverlap {
            wanted,
            overlapping,
        } = matched
        else {
            panic!("expected a partial overlap, got {matched:?}");
        };
        assert_eq!(wanted, vec![key(one)]);
        assert_eq!(overlapping.len(), 1);
        assert_eq!(overlapping[0].root_stem, "coll");
        assert!(!overlapping[0].is_legacy);

        // The complete set selects the same family.
        let recorded = [
            filed_hit(&root, "ref/blogs/coll.md", one),
            filed_hit(&root, "ref/blogs/coll.md", two),
        ];
        let matched = resolve(&rows, &recorded, &[one, two], &root);
        assert!(
            matches!(matched, RecaptureMatch::Selected { .. }),
            "exact set selects, got {matched:?}",
        );
    }

    #[test]
    fn exact_set_multi_reading_family_counts_as_one() {
        let root = unique_root("exact-multi");
        let config = test_config(&root);
        let one = "https://example.com/part-one";
        let two = "https://example.com/part-two";
        write_note(
            &root,
            "ref/blogs/coll.md",
            &note(
                &[
                    "title: Coll",
                    &format!("source_url: [{one}, {two}]"),
                    "source_pdf: lib/blogs/coll.pdf",
                    "status: read",
                ],
                "# Coll\n",
            ),
        );
        write_note(
            &root,
            "ref/blogs/coll__v2.md",
            &note(
                &[
                    "title: Coll",
                    &format!("source_url: [{one}, {two}]"),
                    "source_pdf: lib/blogs/coll__v2.pdf",
                    "status: ready",
                    "earlier:",
                    "  - \"[[ref/blogs/coll|v1 · 2026-01-01]]\"",
                ],
                "# Coll\n",
            ),
        );
        let rows = index_rows(&config);
        let recorded = [
            filed_hit(&root, "ref/blogs/coll.md", one),
            filed_hit(&root, "ref/blogs/coll.md", two),
            filed_hit(&root, "ref/blogs/coll__v2.md", one),
            filed_hit(&root, "ref/blogs/coll__v2.md", two),
        ];
        let matched = resolve(&rows, &recorded, &[one, two], &root);
        let RecaptureMatch::Selected { family } = matched else {
            panic!("expected a selected family, got {matched:?}");
        };
        assert_eq!(family.members.len(), 2);
        assert_eq!(family.max_reading, 2);
    }

    #[test]
    fn exact_family_wins_over_a_larger_overlapping_collection() {
        let root = unique_root("exact-wins-overlap");
        let config = test_config(&root);
        let one = "https://example.com/one";
        let two = "https://example.com/two";
        write_note(
            &root,
            "ref/papers/exact.md",
            &note(
                &[
                    "title: Exact",
                    &format!("source_url: {one}"),
                    "source_pdf: lib/papers/exact.pdf",
                ],
                "# Exact\n",
            ),
        );
        write_note(
            &root,
            "ref/blogs/collection.md",
            &note(
                &[
                    "title: Collection",
                    &format!("source_url: [{one}, {two}]"),
                    "source_pdf: lib/blogs/collection.pdf",
                ],
                "# Collection\n",
            ),
        );
        let rows = index_rows(&config);
        let recorded = [
            filed_hit(&root, "ref/papers/exact.md", one),
            filed_hit(&root, "ref/blogs/collection.md", one),
            filed_hit(&root, "ref/blogs/collection.md", two),
        ];

        let matched = resolve(&rows, &recorded, &[one], &root);
        let RecaptureMatch::Selected { family } = matched else {
            panic!(
                "the exact family must win over a larger overlap: {matched:?}"
            );
        };
        assert_eq!(family.root_stem, "exact");
    }

    #[test]
    fn partial_overlap_count_is_distinct_urls_not_families() {
        let root = unique_root("partial-count");
        let config = test_config(&root);
        let one = "https://example.com/one";
        let two = "https://example.com/two";
        let three = "https://example.com/three";
        for (path, title) in [
            ("ref/papers/first.md", "First"),
            ("ref/blogs/second.md", "Second"),
        ] {
            write_note(
                &root,
                path,
                &note(
                    &[
                        &format!("title: {title}"),
                        &format!("source_url: {one}"),
                        "source_pdf: lib/papers/backing.pdf",
                    ],
                    "# Overlap\n",
                ),
            );
        }
        let rows = index_rows(&config);
        let recorded = [
            filed_hit(&root, "ref/papers/first.md", one),
            filed_hit(&root, "ref/blogs/second.md", one),
        ];
        let matched = resolve(&rows, &recorded, &[one, two, three], &root);
        let RecaptureMatch::PartialOverlap {
            wanted,
            overlapping,
        } = matched
        else {
            panic!("expected partial overlap: {matched:?}");
        };

        let error = partial_overlap_single_error(&wanted, &overlapping);
        assert!(
            error.message.starts_with("1 of 3 URLs are already"),
            "{error:?}"
        );
    }

    #[test]
    fn deleted_family_root_is_recovered_from_the_oldest_lineage_entry() {
        let root = unique_root("deleted-root");
        let config = test_config(&root);
        let url = "https://example.com/deleted-root";
        write_note(
            &root,
            "ref/papers/foo__v2.md",
            &note(
                &[
                    "title: Foo",
                    &format!("source_url: {url}"),
                    "source_pdf: lib/papers/foo__v2.pdf",
                    "earlier:",
                    "  - \"[[ref/papers/foo|v1 · 2026-01-01]]\"",
                ],
                "# Foo\n",
            ),
        );
        write_note(
            &root,
            "ref/papers/foo__v3.md",
            &note(
                &[
                    "title: Foo",
                    &format!("source_url: {url}"),
                    "source_pdf: lib/papers/foo__v3.pdf",
                    "earlier:",
                    "  - \"[[ref/papers/foo__v2|v2 · 2026-02-01]]\"",
                    "  - \"[[ref/papers/foo|v1 · 2026-01-01]]\"",
                ],
                "# Foo\n",
            ),
        );

        let rows = index_rows(&config);
        let family = families_for_stem(&rows, "foo__v3", &root)
            .pop()
            .expect("family");
        assert_eq!(family.root_stem, "foo");
        assert_eq!(family.max_reading, 3);
        assert_eq!(
            allocate_for_family(&config, &family).expect("next reading"),
            "foo__v4",
        );
    }

    #[test]
    fn attach_hit_follows_the_family_to_its_newest_reading() {
        let root = unique_root("newest-attach");
        let config = test_config(&root);
        let url = "https://example.com/attach";
        write_note(
            &root,
            "ref/papers/attach.md",
            &note(
                &[
                    "title: Attach",
                    &format!("source_url: {url}"),
                    "source_pdf: lib/papers/attach.pdf",
                ],
                "# Attach\n",
            ),
        );
        write_note(
            &root,
            "ref/papers/attach__v2.md",
            &note(
                &[
                    "title: Attach",
                    &format!("source_url: {url}"),
                    "source_pdf: lib/papers/attach__v2.pdf",
                    "earlier:",
                    "  - \"[[ref/papers/attach|v1 · 2026-01-01]]\"",
                ],
                "# Attach\n",
            ),
        );
        let recorded = [
            filed_hit(&root, "ref/papers/attach.md", url),
            filed_hit(&root, "ref/papers/attach__v2.md", url),
        ];

        let newest = newest_reading_hit(&config, &recorded, &recorded[0]);
        assert_eq!(
            vault_relative(&newest.path, &root),
            "ref/papers/attach__v2.md",
        );
    }

    #[test]
    fn no_force_output_keeps_the_normal_family_refusal() {
        let root = unique_root("output-refusal");
        let config = test_config(&root);
        let url = "https://example.com/output-refusal";
        write_note(&root, "ref/papers/alpha.md", &alpha_note());
        write_note(
            &root,
            "ref/papers/alpha__v2.md",
            &note(
                &[
                    "title: Alpha",
                    &format!("source_url: {url}"),
                    "source_pdf: lib/papers/alpha__v2.pdf",
                    "earlier:",
                    "  - \"[[ref/papers/alpha|v1 · 2026-01-01]]\"",
                ],
                "# Alpha\n",
            ),
        );
        let rows = index_rows(&config);
        let family = families_for_stem(&rows, "alpha", &root)
            .pop()
            .expect("family");
        let error = finish_selection(
            &config,
            family,
            None,
            Some(Path::new("out.pdf")),
            None,
            "Alpha",
            None,
            true,
            false,
            false,
        )
        .expect_err("no force keeps the captured refusal");
        assert!(error.message.contains("already captured"), "{error:?}");
        assert!(error.message.contains("reading v2"), "{error:?}");
        assert!(error.message.contains("reading v1"), "{error:?}");
        assert!(error.message.contains("-N <name>"), "{error:?}");
        assert!(!error.message.contains("-L"), "{error:?}");
        assert!(!error.message.contains("a reread is named"), "{error:?}");
    }

    #[test]
    fn queued_only_matches_stay_queued() {
        let root = unique_root("queued-only");
        let config = test_config(&root);
        let rows = index_rows(&config);
        let recorded = [queued_hit(&root, "xlib/blogs/queued.pdf", ALPHA)];
        let matched = resolve(&rows, &recorded, &[ALPHA], &root);
        let RecaptureMatch::QueuedOnly { members } = matched else {
            panic!("expected queued-only, got {matched:?}");
        };
        assert_eq!(members.len(), 1);
        assert_eq!(members[0].stem, "queued");
        assert!(members[0].is_queued);
        assert_eq!(members[0].ref_type.as_deref(), Some("blogs"));
    }

    #[test]
    fn legacy_only_hits_are_not_family_roots() {
        let root = unique_root("legacy-only");
        let config = test_config(&root);
        write_note(
            &root,
            "ref/papers/legacy.md",
            &note(
                &[
                    "title: Legacy",
                    &format!("source_url: {ALPHA}"),
                    "status: legacy",
                ],
                "# Legacy\n",
            ),
        );
        let rows = index_rows(&config);
        let recorded = [legacy_hit(&root, "ref/papers/legacy.md", ALPHA)];
        let matched = resolve(&rows, &recorded, &[ALPHA], &root);
        assert!(
            matches!(matched, RecaptureMatch::NoMatch),
            "legacy never roots, got {matched:?}",
        );
    }

    #[test]
    fn free_title_stem_is_an_ordinary_capture() {
        let root = unique_root("free-stem");
        let config = test_config(&root);
        write_note(&root, "ref/papers/alpha.md", &alpha_note());
        let rows = index_rows(&config);
        assert!(families_for_stem(&rows, "brand_new", &root).is_empty());
    }

    #[test]
    fn natural_version_root_keeps_its_full_stem() {
        let root = unique_root("natural-version");
        let config = test_config(&root);
        write_note(
            &root,
            "ref/papers/protocol__v2.md",
            &note(
                &[
                    "title: Protocol",
                    "source_url: https://example.com/protocol",
                    "source_pdf: lib/papers/protocol__v2.pdf",
                    "status: ready",
                ],
                "# Protocol\n",
            ),
        );
        let rows = index_rows(&config);
        let families = families_for_stem(&rows, "protocol__v2", &root);
        assert_eq!(families.len(), 1);
        // No lineage: reading 1 under its own full stem.
        assert_eq!(families[0].root_stem, "protocol__v2");
        assert_eq!(families[0].newest().reading, None);
        assert_eq!(
            allocate_for_family(&config, &families[0]).expect("allocates"),
            "protocol__v2__v2",
        );
    }

    #[test]
    fn real_later_reading_rereads_beside_its_root() {
        let root = unique_root("real-v2");
        let config = test_config(&root);
        let url = "https://example.com/why";
        write_note(
            &root,
            "ref/papers/why.md",
            &note(
                &[
                    "title: Why",
                    &format!("source_url: {url}"),
                    "source_pdf: lib/papers/why.pdf",
                    "status: read",
                ],
                "# Why\n",
            ),
        );
        write_note(
            &root,
            "ref/papers/why__v2.md",
            &note(
                &[
                    "title: Why",
                    &format!("source_url: {url}"),
                    "source_pdf: lib/papers/why__v2.pdf",
                    "status: ready",
                    "earlier:",
                    "  - \"[[ref/papers/why|v1 · 2026-01-01]]\"",
                ],
                "# Why\n",
            ),
        );
        let rows = index_rows(&config);
        let families = families_for_stem(&rows, "why__v2", &root);
        assert_eq!(families.len(), 1);
        assert_eq!(families[0].root_stem, "why");
        assert_eq!(families[0].selected_member().stem, "why__v2");
        assert_eq!(
            allocate_for_family(&config, &families[0]).expect("allocates"),
            "why__v3",
        );
    }

    #[test]
    fn allocation_skips_occupied_and_refuses_at_cap() {
        let occupied = |stem: &str| matches!(stem, "r__v2" | "r__v3" | "r__v4");
        assert_eq!(
            allocate_next_stem("r", 1, &occupied).expect("allocates"),
            "r__v5",
        );
        // An unrelated `foo_2` never blocks `foo__v2`.
        let occupied = |stem: &str| stem == "foo_2";
        assert_eq!(
            allocate_next_stem("foo", 1, &occupied).expect("allocates"),
            "foo__v2",
        );
        let always = |_: &str| true;
        let error =
            allocate_next_stem("full", 1, &always).expect_err("exhausted");
        assert!(
            error.message.contains("__v999"),
            "cap names the bound: {error:?}",
        );
        assert_eq!(error.exit_code, None);
    }

    #[test]
    fn occupancy_covers_sidecars_audio_assets_and_ids() {
        use super::super::stamp::DefaultTargetOccupancy;

        let root = unique_root("occupancy");
        let config = test_config(&root);
        std::fs::write(root.join("xlib/papers/orphan.mp3"), b"audio")
            .expect("orphan audio");
        std::fs::write(root.join("lib/papers/bundled.pdf"), b"pdf")
            .expect("library pdf");
        std::fs::write(root.join("lib/papers/bundled.md"), b"sidecar")
            .expect("library sidecar");
        std::fs::create_dir_all(root.join("ref/papers/paged.assets"))
            .expect("assets dir");
        write_note(
            &root,
            "ref/papers/id_holder.md",
            &note(&["title: Id", "id: wanted"], "# Id\n"),
        );
        let occupancy = DefaultTargetOccupancy::build(&config, "papers")
            .expect("occupancy builds");

        let names: Vec<String> = occupancy
            .occupants("wanted")
            .iter()
            .map(|path| {
                path.file_name()
                    .expect("occupant has a name")
                    .to_string_lossy()
                    .into_owned()
            })
            .collect();
        assert!(
            names.contains(&"id_holder.md".to_string()),
            "id-only collision reserves the stem: {names:?}",
        );
        assert!(
            !occupancy.occupants("orphan").is_empty(),
            "orphan companion audio reserves its stem",
        );
        assert!(
            occupancy.walk_occupants("orphan").is_empty(),
            "the ordinary walk keeps its targeted audio refusal instead",
        );
        assert!(
            occupancy.occupants("bundled").len() >= 2,
            "library PDF plus sidecar both reserve",
        );
        assert!(
            !occupancy.occupants("paged").is_empty(),
            ".assets/ reserves its stem",
        );
        assert!(
            occupancy.occupants("free").is_empty(),
            "free stems stay free",
        );

        // The locked-filing recheck ignores the queued unit's own files.
        let own = occupancy.occupants("bundled");
        assert!(
            occupancy.occupants_except("bundled", &own).is_empty(),
            "own files are ignored on recheck",
        );
        // …but a stranger's occupant still blocks.
        let foreign = occupancy.occupants("wanted");
        assert!(
            !occupancy.occupants_except("bundled", &foreign).is_empty(),
            "foreign occupants still block",
        );
    }

    #[test]
    fn report_line_covers_every_detail_row() {
        assert_eq!(
            reread_report_line("a", "a__v2", 1, Some("read"), Some(41)),
            "reread: a → a__v2 (v1 read · 41 highlights)",
        );
        assert_eq!(
            reread_report_line("a", "a__v2", 1, Some("read"), Some(1)),
            "reread: a → a__v2 (v1 read · 1 highlight)",
        );
        assert_eq!(
            reread_report_line("a", "a__v2", 3, Some("wip"), None),
            "reread: a → a__v2 (v3 wip)",
        );
        assert_eq!(
            reread_report_line("a", "a__v2", 2, None, None),
            "reread: a → a__v2 (v2)",
        );
        // Plain output carries no ANSI escapes.
        let line = reread_report_line("a", "a__v2", 1, Some("read"), Some(2));
        assert!(!line.contains('\u{1b}'), "{line:?}");
    }

    #[test]
    fn defaults_come_from_the_newest_located_task() {
        let root = unique_root("defaults");
        let config = test_config(&root);
        let url = "https://example.com/tasked";
        let task = "- [ ] #task #ref [[lib/papers/tasked.pdf]] #hide ^ref\n";
        let v1_task =
            "- [ ] #task #ref [[lib/papers/tasked__v2.pdf]] #hide ^ref\n";
        write_note(
            &root,
            "ref/papers/tasked.md",
            &note(
                &[
                    "title: Tasked",
                    &format!("source_url: {url}"),
                    "source_pdf: lib/papers/tasked.pdf",
                    "status: ready",
                ],
                &format!("# Tasked\n\n{task}"),
            ),
        );
        write_note(
            &root,
            "ref/papers/tasked__v2.md",
            &note(
                &[
                    "title: Tasked",
                    &format!("source_url: {url}"),
                    "source_pdf: lib/papers/tasked__v2.pdf",
                    "status: ready",
                    "earlier:",
                    "  - \"[[ref/papers/tasked|v1 · 2026-01-01]]\"",
                ],
                &format!("# Tasked\n\n{v1_task}"),
            ),
        );
        let rows = index_rows(&config);
        let recorded = [
            filed_hit(&root, "ref/papers/tasked.md", url),
            filed_hit(&root, "ref/papers/tasked__v2.md", url),
        ];
        let RecaptureMatch::Selected { family } =
            resolve(&rows, &recorded, &[url], &root)
        else {
            panic!("expected a selected family");
        };
        let defaults = family_defaults(family.newest());
        assert_eq!(
            defaults.parent.as_deref(),
            Some("ref/papers/tasked__v2.md"),
            "newest reading's located task residence",
        );
        assert_eq!(defaults.ref_type.as_deref(), Some("papers"));
    }
}
