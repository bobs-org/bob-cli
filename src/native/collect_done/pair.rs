//! Metadata-first bidirectional archive pair resolution.
//!
//! Every archive consumer shares one immutable pair index built from
//! vault-relative paths, captured note contents/frontmatter, and the shared
//! [`crate::native::vault_links::NoteIndex`]. [`ArchivePairIndex::for_source`]
//! and [`ArchivePairIndex::for_archive`] return distinct bound, unpaired, and
//! refused outcomes. Readers must never treat a refused result as an
//! invitation to derive a filename.
//!
//! Resolution is global and deterministic, not first-match iteration:
//!
//! 1. Source `done_tasks:` and archive `parent:` resolve from actual YAML
//!    frontmatter (body and fenced occurrences are ignored). When both
//!    resolve and reciprocally agree, that association wins even if the
//!    archive filename is old. Targets resolve with the shared exact-path
//!    and case-insensitive unique-basename rules; aliases, fragments, and
//!    optional `.md` follow existing link parsing conventions.
//! 2. Otherwise the existing source-to-archive and archive-to-source filename
//!    rule applies only when it has no contradictory or ambiguous claim. A
//!    filename fallback requires a real source for an existing archive. A new
//!    archive destination may be absent, but its absence is not an existing
//!    bound archive or grandfathering evidence.
//! 3. Contradictory metadata, ambiguous metadata targets, two archives
//!    claiming one source, two sources claiming one archive, or an occupied
//!    canonical rename destination are refused. A reciprocal pair never hides
//!    a second claimant. Nothing overwrites claims, merges archives, or
//!    chooses authority by file age, content equality, or order.
//! 4. Missing/stale metadata falls back when the filename pair is unique and
//!    no resolved metadata contradicts it. A one-sided noncanonical link is
//!    insufficient evidence to invent a renamed pair. Unresolved archives are
//!    reported as orphans.
//!
//! Filename helpers stay canonical path construction and fallback inside this
//! module. No lookup performs disk I/O: [`ArchivePairIndex::read_vault`]
//! walks the vault once up front. A note that cannot be read is retained as
//! a census failure rather than dropped, and any lookup that failure could
//! plausibly affect is refused (`incomplete_census`), so an incomplete
//! census is never presented as proof of no conflict.

use std::{
    collections::{BTreeMap, BTreeSet},
    ffi::OsStr,
    fs, io,
    path::{Component, Path, PathBuf},
};

use super::super::task_dependencies::is_archive_path;
use super::super::vault_links::{LinkResolution, NoteIndex};

/// Canonical archive destination for a source note, if one is computable.
///
/// Pure path construction (`done/<parent>/<stem>_done.md`); no existence or
/// metadata claim is implied.
pub(crate) fn canonical_archive_path(source: &Path) -> Option<PathBuf> {
    if source.as_os_str().is_empty()
        || is_archive_path(source)
        || source
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return None;
    }
    super::transform::archive_relative_path(source).ok()
}

/// Filename-derived source for an archive path, without existence checks.
///
/// This is only canonical path inversion; callers pair it with a census
/// lookup before treating it as a binding.
fn filename_source_for(archive: &Path) -> Option<PathBuf> {
    super::link_repair::source_relative_path_from_archive(archive)
}

/// One established source/archive binding.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BoundPair {
    /// Vault-relative source note.
    pub(crate) source: PathBuf,
    /// Vault-relative existing archive.
    pub(crate) archive: PathBuf,
    /// Canonical destination for the source (`done/<parent>/<stem>_done.md`).
    pub(crate) canonical_archive: PathBuf,
    /// True when reciprocal metadata established the binding; false when the
    /// filename fallback did.
    pub(crate) via_metadata: bool,
}

/// Lookup outcome for a source note.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SourceLookup {
    /// The source is safely bound to an existing archive.
    Bound(BoundPair),
    /// The source has no archive and no conflict. The canonical destination
    /// is available for creation when computable.
    Missing { canonical_archive: Option<PathBuf> },
    /// Contradictory or ambiguous claims make binding unsafe. `related`
    /// lists the competing paths in sorted order.
    Refused {
        reason: &'static str,
        canonical_archive: Option<PathBuf>,
        related: Vec<PathBuf>,
    },
}

/// Lookup outcome for an archive note.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ArchiveLookup {
    /// The archive is safely bound to its source.
    Bound(BoundPair),
    /// The archive has no resolvable source and no conflict.
    Orphan { archive: PathBuf },
    /// Contradictory or ambiguous claims make binding unsafe. `related`
    /// lists the competing paths in sorted order.
    Refused {
        reason: &'static str,
        related: Vec<PathBuf>,
    },
}

/// Captured note census behind a pair index: contents plus link resolution.
///
/// Reads that fail are retained as failure paths rather than aborting the
/// build: lookups refuse when a failure could plausibly affect them and
/// proceed only with proof that it cannot (no shared endpoint or basename
/// stem, and no existing archive for a third party to claim).
#[derive(Debug, Clone, Default)]
struct PairCensus {
    contents: BTreeMap<PathBuf, String>,
    note_index: NoteIndex,
    failures: BTreeSet<PathBuf>,
}

impl PairCensus {
    fn read_vault(vault: &Path) -> io::Result<Self> {
        let mut paths = Vec::new();
        collect_markdown_files(vault, vault, &mut paths)?;
        paths.sort();
        paths.dedup();
        let mut contents = BTreeMap::new();
        let mut failures = BTreeSet::new();
        for relative in &paths {
            match fs::read_to_string(vault.join(relative)) {
                Ok(text) => {
                    contents.insert(relative.clone(), text);
                }
                Err(_) => {
                    failures.insert(relative.clone());
                }
            }
        }
        let note_index = NoteIndex::from_paths(paths);
        Ok(Self {
            contents,
            note_index,
            failures,
        })
    }
}

fn collect_markdown_files(
    vault: &Path,
    directory: &Path,
    files: &mut Vec<PathBuf>,
) -> io::Result<()> {
    let entries = fs::read_dir(directory)
        .map_err(|error| {
            io::Error::new(
                error.kind(),
                format!("read {}: {error}", directory.display()),
            )
        })?
        .collect::<Result<Vec<_>, io::Error>>()?;
    for entry in entries {
        let file_type = entry.file_type()?;
        if file_type.is_symlink() {
            continue;
        }
        let path = entry.path();
        if file_type.is_dir() {
            if entry
                .file_name()
                .to_str()
                .is_some_and(|name| name.starts_with('.'))
            {
                continue;
            }
            collect_markdown_files(vault, &path, files)?;
        } else if file_type.is_file()
            && path
                .extension()
                .and_then(OsStr::to_str)
                .is_some_and(|extension| extension.eq_ignore_ascii_case("md"))
            && let Ok(relative) = path.strip_prefix(vault)
        {
            files.push(relative.to_path_buf());
        }
    }
    Ok(())
}

/// Metadata claim resolution for one note.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Claim {
    /// No metadata, or metadata that resolves to nothing on disk.
    Absent,
    /// Metadata resolves to exactly one indexed note.
    Found(PathBuf),
    /// Metadata matches several indexed basenames.
    Ambiguous,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Refusal {
    reason: &'static str,
    related: Vec<PathBuf>,
}

/// Shared immutable archive pair index.
#[derive(Debug, Clone)]
pub(crate) struct ArchivePairIndex {
    census: PairCensus,
    sources: BTreeMap<PathBuf, SourceLookup>,
    archives: BTreeMap<PathBuf, ArchiveLookup>,
    raw_sources: BTreeMap<PathBuf, Option<String>>,
    raw_archives: BTreeMap<PathBuf, Option<String>>,
}

impl ArchivePairIndex {
    /// Build from captured in-memory buffers (`vault-relative path, bytes`).
    pub(crate) fn from_buffers(
        buffers: &[(String, String)],
        note_index: &NoteIndex,
    ) -> Self {
        let census = PairCensus {
            contents: buffers
                .iter()
                .map(|(relative, text)| (PathBuf::from(relative), text.clone()))
                .collect(),
            note_index: note_index.clone(),
            failures: BTreeSet::new(),
        };
        Self::from_census(census)
    }

    /// Build from a captured contents map. No disk I/O.
    pub(crate) fn from_contents(contents: &BTreeMap<PathBuf, String>) -> Self {
        let note_index = NoteIndex::from_paths(contents.keys().cloned());
        Self::from_census(PairCensus {
            contents: contents.clone(),
            note_index,
            failures: BTreeSet::new(),
        })
    }

    /// Source lookup table built from the census.
    pub(crate) fn source_entries(
        &self,
    ) -> impl Iterator<Item = (&Path, &SourceLookup)> {
        self.sources
            .iter()
            .map(|(path, lookup)| (path.as_path(), lookup))
    }

    /// Archive lookup table built from the census.
    pub(crate) fn archive_entries(
        &self,
    ) -> impl Iterator<Item = (&Path, &ArchiveLookup)> {
        self.archives
            .iter()
            .map(|(path, lookup)| (path.as_path(), lookup))
    }

    /// Walk the vault once and build from disk. Reads that fail are
    /// retained as failure paths; lookups refuse when a failure could
    /// plausibly affect them. No lookup performs I/O after this returns.
    pub(crate) fn read_vault(vault: &Path) -> io::Result<Self> {
        PairCensus::read_vault(vault).map(Self::from_census)
    }

    /// Rebuild with one note's bytes replaced, without disk I/O.
    ///
    /// Insertion callers hold bytes fresher than the census (a destination
    /// reread inside the retry loop); rebinding is pure and keeps the
    /// single-scan-per-operation contract. Fresh bytes supersede a retained
    /// read failure for the same path.
    pub(crate) fn with_contents(&self, path: &Path, contents: &str) -> Self {
        let mut census = self.census.clone();
        census
            .contents
            .insert(path.to_path_buf(), contents.to_string());
        census.failures.remove(path);
        census.note_index =
            census.note_index.with_additional([path.to_path_buf()]);
        Self::from_census(census)
    }

    fn from_census(census: PairCensus) -> Self {
        let resolved = resolve_pairs(&census);
        Self {
            census,
            sources: resolved.sources,
            archives: resolved.archives,
            raw_sources: resolved.raw_sources,
            raw_archives: resolved.raw_archives,
        }
    }

    /// Sorted retained read-failure paths, for refusal diagnostics.
    fn failed_paths(&self) -> Vec<PathBuf> {
        self.census.failures.iter().cloned().collect()
    }

    /// Whether a metadata target could name a failed note: the exact path
    /// is unreadable, or the exact path is absent and the target stem
    /// matches a failed note's stem (the failed note may be the intended
    /// unique referent, or the ambiguity it would create).
    fn target_hits_failure(&self, target: &str) -> bool {
        if self.census.failures.is_empty() {
            return false;
        }
        let Some(exact) =
            super::super::vault_links::target_to_markdown_path(target)
        else {
            let stem = target_stem(target);
            return self
                .census
                .failures
                .iter()
                .any(|failure| failure_stem(failure) == stem);
        };
        if self.census.failures.contains(&exact) {
            return true;
        }
        if self.census.contents.contains_key(&exact) {
            return false;
        }
        let stem = exact
            .file_stem()
            .and_then(|stem| stem.to_str())
            .unwrap_or_default()
            .to_lowercase();
        self.census
            .failures
            .iter()
            .any(|failure| failure_stem(failure) == stem)
    }

    /// Whether a metadata target resolves to an existing indexed note.
    fn target_resolves_existing(&self, target: &str) -> bool {
        self.census
            .note_index
            .resolve_detailed(None, target)
            .path()
            .is_some()
    }

    /// Refuse when a retained failure could plausibly affect this source
    /// lookup. Reciprocal bindings with readable endpoints and
    /// failure-free targets survive unknown third parties (the pair keeps
    /// its owner against any extra claimant); every other involvement with
    /// an existing archive, endpoint, or stem refuses.
    fn source_poison(&self, source: &Path) -> Option<SourceLookup> {
        if self.census.failures.is_empty() {
            return None;
        }
        if let Some(SourceLookup::Bound(bound)) = self.sources.get(source)
            && bound.via_metadata
            && !self.census.failures.contains(&bound.source)
            && !self.census.failures.contains(&bound.archive)
            && !raw_target(&self.raw_sources, source)
                .is_some_and(|target| self.target_hits_failure(target))
            && !raw_target(&self.raw_archives, &bound.archive)
                .is_some_and(|target| self.target_hits_failure(target))
        {
            return None;
        }
        if self.census.failures.contains(source) {
            return Some(self.incomplete_source(source));
        }
        let canonical = canonical_archive_path(source);
        if canonical
            .as_deref()
            .is_some_and(|path| self.census.failures.contains(path))
        {
            return Some(self.incomplete_source(source));
        }
        match raw_target(&self.raw_sources, source) {
            Some(target) if self.target_hits_failure(target) => {
                Some(self.incomplete_source(source))
            }
            Some(target) if self.target_resolves_existing(target) => {
                Some(self.incomplete_source(source))
            }
            _ => {
                if canonical
                    .as_deref()
                    .is_some_and(|path| self.census.contents.contains_key(path))
                {
                    Some(self.incomplete_source(source))
                } else {
                    None
                }
            }
        }
    }

    fn incomplete_source(&self, source: &Path) -> SourceLookup {
        SourceLookup::Refused {
            reason: "incomplete_census",
            canonical_archive: canonical_archive_path(source),
            related: self.failed_paths(),
        }
    }

    /// Mirror of [`ArchivePairIndex::source_poison`] for archive lookups.
    fn archive_poison(&self, archive: &Path) -> Option<ArchiveLookup> {
        if self.census.failures.is_empty() {
            return None;
        }
        if let Some(ArchiveLookup::Bound(bound)) = self.archives.get(archive)
            && bound.via_metadata
            && !self.census.failures.contains(&bound.source)
            && !self.census.failures.contains(&bound.archive)
            && !raw_target(&self.raw_sources, &bound.source)
                .is_some_and(|target| self.target_hits_failure(target))
            && !raw_target(&self.raw_archives, archive)
                .is_some_and(|target| self.target_hits_failure(target))
        {
            return None;
        }
        if self.census.failures.contains(archive) {
            return Some(self.incomplete_archive());
        }
        match raw_target(&self.raw_archives, archive) {
            Some(target) if self.target_hits_failure(target) => {
                Some(self.incomplete_archive())
            }
            Some(target) if self.target_resolves_existing(target) => {
                Some(self.incomplete_archive())
            }
            _ => {
                let derived = filename_source_for(archive);
                if derived.is_some_and(|source| {
                    !is_archive_path(&source)
                        && self.census.contents.contains_key(&source)
                }) {
                    Some(self.incomplete_archive())
                } else {
                    None
                }
            }
        }
    }

    fn incomplete_archive(&self) -> ArchiveLookup {
        ArchiveLookup::Refused {
            reason: "incomplete_census",
            related: self.failed_paths(),
        }
    }

    /// Look up the archive bound to a vault-relative source note.
    pub(crate) fn for_source(&self, source: &Path) -> SourceLookup {
        if let Some(known) = self.sources.get(source) {
            if matches!(known, SourceLookup::Refused { .. }) {
                return known.clone();
            }
            if let Some(poisoned) = self.source_poison(source) {
                return poisoned;
            }
            return known.clone();
        }
        if is_archive_path(source) {
            return SourceLookup::Refused {
                reason: "not_a_source",
                canonical_archive: None,
                related: vec![source.to_path_buf()],
            };
        }
        let canonical = canonical_archive_path(source);
        // Unknown sources carry no metadata, but a retained failure keeps
        // the census from proving the canonical slot is untouched.
        if !self.census.failures.is_empty()
            && (self.census.failures.contains(source)
                || canonical.as_deref().is_some_and(|path| {
                    self.census.failures.contains(path)
                        || self.census.contents.contains_key(path)
                }))
        {
            return SourceLookup::Refused {
                reason: "incomplete_census",
                canonical_archive: canonical,
                related: self.failed_paths(),
            };
        }
        if let Some(canonical) = &canonical
            && let Some(existing) = self.archives.get(canonical)
        {
            let occupied = match existing {
                ArchiveLookup::Bound(bound) => bound.source != source,
                ArchiveLookup::Refused { .. } => true,
                ArchiveLookup::Orphan { .. } => false,
            };
            if occupied {
                return SourceLookup::Refused {
                    reason: "occupied_canonical",
                    canonical_archive: Some(canonical.clone()),
                    related: vec![canonical.clone()],
                };
            }
        }
        SourceLookup::Missing {
            canonical_archive: canonical,
        }
    }

    /// Look up the source bound to a vault-relative archive note.
    pub(crate) fn for_archive(&self, archive: &Path) -> ArchiveLookup {
        if let Some(known) = self.archives.get(archive) {
            if matches!(known, ArchiveLookup::Refused { .. }) {
                return known.clone();
            }
            if let Some(poisoned) = self.archive_poison(archive) {
                return poisoned;
            }
            return known.clone();
        }
        if self.census.failures.contains(archive) {
            return self.incomplete_archive();
        }
        ArchiveLookup::Orphan {
            archive: archive.to_path_buf(),
        }
    }
}

/// Raw frontmatter target for one census note, if it names any.
fn raw_target<'a>(
    raw: &'a BTreeMap<PathBuf, Option<String>>,
    path: &Path,
) -> Option<&'a str> {
    raw.get(path).and_then(|target| target.as_deref())
}

/// Lowercased basename stem of a failed note, for target-stem comparison.
fn failure_stem(path: &Path) -> String {
    path.file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or_default()
        .to_lowercase()
}

/// Lowercased stem of a raw link target that could not become a path.
fn target_stem(target: &str) -> String {
    let base = target.rsplit(['/', '\\']).next().unwrap_or(target).trim();
    base.strip_suffix(".md")
        .or_else(|| base.strip_suffix(".MD"))
        .unwrap_or(base)
        .to_lowercase()
}

struct ResolvedPairs {
    sources: BTreeMap<PathBuf, SourceLookup>,
    archives: BTreeMap<PathBuf, ArchiveLookup>,
    raw_sources: BTreeMap<PathBuf, Option<String>>,
    raw_archives: BTreeMap<PathBuf, Option<String>>,
}

fn resolve_pairs(census: &PairCensus) -> ResolvedPairs {
    let mut source_claims: BTreeMap<PathBuf, Claim> = BTreeMap::new();
    let mut archive_claims: BTreeMap<PathBuf, Claim> = BTreeMap::new();
    let mut raw_sources: BTreeMap<PathBuf, Option<String>> = BTreeMap::new();
    let mut raw_archives: BTreeMap<PathBuf, Option<String>> = BTreeMap::new();
    let mut refused_sources: BTreeMap<PathBuf, Refusal> = BTreeMap::new();
    let mut refused_archives: BTreeMap<PathBuf, Refusal> = BTreeMap::new();

    for (path, contents) in &census.contents {
        if is_archive_path(path) {
            raw_archives.insert(
                path.clone(),
                super::link_repair::archive_parent_target(contents),
            );
            let claim =
                resolve_parent_claim(&census.note_index, path, contents);
            match claim {
                Claim::Ambiguous => {
                    refused_archives.insert(
                        path.clone(),
                        Refusal {
                            reason: "ambiguous_metadata",
                            related: vec![path.clone()],
                        },
                    );
                }
                Claim::Found(ref target) if is_archive_path(target) => {
                    refused_archives.insert(
                        path.clone(),
                        Refusal {
                            reason: "invalid_claim_target",
                            related: vec![target.clone()],
                        },
                    );
                }
                _ => {}
            }
            archive_claims.insert(path.clone(), claim);
        } else {
            raw_sources.insert(
                path.clone(),
                frontmatter_value(contents, "done_tasks:"),
            );
            let claim =
                resolve_done_tasks_claim(&census.note_index, path, contents);
            match claim {
                Claim::Ambiguous => {
                    refused_sources.insert(
                        path.clone(),
                        Refusal {
                            reason: "ambiguous_metadata",
                            related: vec![path.clone()],
                        },
                    );
                }
                Claim::Found(ref target) if !is_archive_path(target) => {
                    refused_sources.insert(
                        path.clone(),
                        Refusal {
                            reason: "invalid_claim_target",
                            related: vec![target.clone()],
                        },
                    );
                }
                _ => {}
            }
            source_claims.insert(path.clone(), claim);
        }
    }

    // Reciprocal metadata bindings, before conflict checks dissolve them.
    let mut tentative: BTreeMap<PathBuf, PathBuf> = BTreeMap::new();
    for (archive, claim) in &archive_claims {
        if refused_archives.contains_key(archive) {
            continue;
        }
        if let Claim::Found(source) = claim
            && !refused_sources.contains_key(source)
            && source_claims.get(source) == Some(&Claim::Found(archive.clone()))
        {
            tentative.insert(archive.clone(), source.clone());
        }
    }

    // Two sources claiming one archive: the reciprocal owner keeps the pair
    // and every extra claimant is refused; with no reciprocal owner the
    // archive and every claimant are refused.
    let mut claimants: BTreeMap<PathBuf, Vec<PathBuf>> = BTreeMap::new();
    for (source, claim) in &source_claims {
        if refused_sources.contains_key(source) {
            continue;
        }
        if let Claim::Found(archive) = claim {
            claimants
                .entry(archive.clone())
                .or_default()
                .push(source.clone());
        }
    }
    for (archive, sources) in &claimants {
        if sources.len() < 2 {
            continue;
        }
        let owner = tentative.get(archive).filter(|bound| {
            sources.contains(bound) && !refused_archives.contains_key(archive)
        });
        match owner {
            Some(bound) => {
                for source in sources {
                    if source != bound {
                        refuse_source(
                            &mut refused_sources,
                            source,
                            "duplicate_claim",
                            &[archive.clone(), bound.clone()],
                        );
                    }
                }
            }
            None => {
                let mut related = sources.clone();
                related.push(archive.clone());
                related.sort();
                related.dedup();
                for source in sources {
                    refused_sources.insert(
                        source.clone(),
                        Refusal {
                            reason: "duplicate_claim",
                            related: related.clone(),
                        },
                    );
                }
                refused_archives.insert(
                    archive.clone(),
                    Refusal {
                        reason: "duplicate_claim",
                        related,
                    },
                );
            }
        }
    }

    // Two archives claiming one source, mirrored.
    let mut archive_claimants: BTreeMap<PathBuf, Vec<PathBuf>> =
        BTreeMap::new();
    for (archive, claim) in &archive_claims {
        if refused_archives.contains_key(archive) {
            continue;
        }
        if let Claim::Found(source) = claim {
            archive_claimants
                .entry(source.clone())
                .or_default()
                .push(archive.clone());
        }
    }
    for (source, archives) in &archive_claimants {
        if archives.len() < 2 {
            continue;
        }
        let owner = archives.iter().find(|archive| {
            tentative.get(*archive) == Some(source)
                && !refused_sources.contains_key(source)
        });
        match owner {
            Some(bound) => {
                for archive in archives {
                    if archive != bound {
                        refuse_archive(
                            &mut refused_archives,
                            archive,
                            "duplicate_claim",
                            &[source.clone(), bound.clone()],
                        );
                    }
                }
            }
            None => {
                let mut related = archives.clone();
                related.push(source.clone());
                related.sort();
                related.dedup();
                for archive in archives {
                    refused_archives.insert(
                        archive.clone(),
                        Refusal {
                            reason: "duplicate_claim",
                            related: related.clone(),
                        },
                    );
                }
                refused_sources.insert(
                    source.clone(),
                    Refusal {
                        reason: "duplicate_claim",
                        related,
                    },
                );
            }
        }
    }

    // Contradicted one-sided claims: each side names a different partner.
    for (archive, claim) in &archive_claims {
        if refused_archives.contains_key(archive) {
            continue;
        }
        if let Claim::Found(source) = claim
            && let Some(Claim::Found(other)) = source_claims.get(source)
            && *other != *archive
        {
            refuse_archive(
                &mut refused_archives,
                archive,
                "contradictory_metadata",
                &[source.clone(), other.clone()],
            );
        }
    }
    for (source, claim) in &source_claims {
        if refused_sources.contains_key(source) {
            continue;
        }
        if let Claim::Found(archive) = claim
            && let Some(Claim::Found(other)) = archive_claims.get(archive)
            && *other != *source
        {
            refuse_source(
                &mut refused_sources,
                source,
                "contradictory_metadata",
                &[archive.clone(), other.clone()],
            );
        }
    }

    // A one-sided noncanonical claim is insufficient evidence to invent a
    // renamed pair.
    for (source, claim) in &source_claims {
        if refused_sources.contains_key(source) {
            continue;
        }
        if let Claim::Found(archive) = claim
            && tentative.get(archive) != Some(source)
        {
            let canonical = canonical_archive_path(source);
            if canonical.as_deref() != Some(archive.as_path()) {
                refuse_source(
                    &mut refused_sources,
                    source,
                    "one_sided_rename",
                    &[archive.clone()],
                );
            }
        }
    }
    for (archive, claim) in &archive_claims {
        if refused_archives.contains_key(archive) {
            continue;
        }
        if let Claim::Found(source) = claim
            && tentative.get(archive) != Some(source)
        {
            let corroborated =
                filename_source_for(archive).is_some_and(|derived| {
                    derived == *source && census.contents.contains_key(&derived)
                });
            if !corroborated {
                refuse_archive(
                    &mut refused_archives,
                    archive,
                    if filename_source_for(archive).is_some_and(|derived| {
                        census.contents.contains_key(&derived)
                            && derived != *source
                    }) {
                        "contradictory_metadata"
                    } else {
                        "one_sided_rename"
                    },
                    &[source.clone()],
                );
            }
        }
    }

    // Claims against a refused partner poison the claimant too: no binding
    // is safe when its counterparty is conflicted. Iterate to a fixpoint so
    // cascades settle deterministically.
    loop {
        let mut progressed = false;
        for (source, claim) in &source_claims {
            if refused_sources.contains_key(source) {
                continue;
            }
            if let Claim::Found(archive) = claim
                && refused_archives.contains_key(archive)
            {
                refuse_source(
                    &mut refused_sources,
                    source,
                    "contradictory_metadata",
                    &[archive.clone()],
                );
                progressed = true;
            }
        }
        for (archive, claim) in &archive_claims {
            if refused_archives.contains_key(archive) {
                continue;
            }
            if let Claim::Found(source) = claim
                && refused_sources.contains_key(source)
            {
                refuse_archive(
                    &mut refused_archives,
                    archive,
                    "contradictory_metadata",
                    &[source.clone()],
                );
                progressed = true;
            }
        }
        if !progressed {
            break;
        }
    }

    // Surviving reciprocal bindings, with canonical destinations attached.
    let mut bound: BTreeMap<PathBuf, BoundPair> = BTreeMap::new();
    for (archive, source) in &tentative {
        if refused_archives.contains_key(archive)
            || refused_sources.contains_key(source)
        {
            continue;
        }
        let canonical =
            canonical_archive_path(source).unwrap_or_else(|| archive.clone());
        bound.insert(
            archive.clone(),
            BoundPair {
                source: source.clone(),
                archive: archive.clone(),
                canonical_archive: canonical,
                via_metadata: true,
            },
        );
    }

    // Filename fallback for the clean remainder: the archive's derived
    // source must exist and be unbound, and neither side may name a
    // different partner.
    for (archive, claim) in &archive_claims {
        if refused_archives.contains_key(archive) || bound.contains_key(archive)
        {
            continue;
        }
        let Some(derived) = filename_source_for(archive) else {
            continue;
        };
        if is_archive_path(&derived)
            || !census.contents.contains_key(&derived)
            || refused_sources.contains_key(&derived)
            || bound
                .values()
                .any(|pair: &BoundPair| pair.source == derived)
        {
            continue;
        }
        if let Claim::Found(source) = claim
            && *source != derived
        {
            continue;
        }
        if let Some(Claim::Found(other)) = source_claims.get(&derived)
            && *other != *archive
        {
            continue;
        }
        let canonical =
            canonical_archive_path(&derived).unwrap_or_else(|| archive.clone());
        bound.insert(
            archive.clone(),
            BoundPair {
                source: derived,
                archive: archive.clone(),
                canonical_archive: canonical,
                via_metadata: false,
            },
        );
    }

    // An existing canonical destination occupied by another binding (or by
    // conflict) refuses the clean leftover source.
    let mut sources = BTreeMap::new();
    for source in source_claims.keys() {
        if let Some(refusal) = refused_sources.get(source) {
            sources.insert(
                source.clone(),
                SourceLookup::Refused {
                    reason: refusal.reason,
                    canonical_archive: canonical_archive_path(source),
                    related: refusal.related.clone(),
                },
            );
            continue;
        }
        if let Some(pair) = bound.values().find(|pair| pair.source == *source) {
            sources.insert(source.clone(), SourceLookup::Bound(pair.clone()));
            continue;
        }
        let canonical = canonical_archive_path(source);
        if let Some(canonical) = &canonical
            && census.contents.contains_key(canonical)
        {
            sources.insert(
                source.clone(),
                SourceLookup::Refused {
                    reason: "occupied_canonical",
                    canonical_archive: Some(canonical.clone()),
                    related: vec![canonical.clone()],
                },
            );
            continue;
        }
        sources.insert(
            source.clone(),
            SourceLookup::Missing {
                canonical_archive: canonical,
            },
        );
    }

    let mut archives = BTreeMap::new();
    for archive in archive_claims.keys() {
        if let Some(refusal) = refused_archives.get(archive) {
            archives.insert(
                archive.clone(),
                ArchiveLookup::Refused {
                    reason: refusal.reason,
                    related: refusal.related.clone(),
                },
            );
            continue;
        }
        if let Some(pair) = bound.get(archive) {
            archives
                .insert(archive.clone(), ArchiveLookup::Bound(pair.clone()));
            continue;
        }
        archives.insert(
            archive.clone(),
            ArchiveLookup::Orphan {
                archive: archive.clone(),
            },
        );
    }

    ResolvedPairs {
        sources,
        archives,
        raw_sources,
        raw_archives,
    }
}

fn refuse_source(
    refused: &mut BTreeMap<PathBuf, Refusal>,
    source: &Path,
    reason: &'static str,
    related: &[PathBuf],
) {
    let mut related = related.to_vec();
    related.push(source.to_path_buf());
    related.sort();
    related.dedup();
    refused.insert(source.to_path_buf(), Refusal { reason, related });
}

fn refuse_archive(
    refused: &mut BTreeMap<PathBuf, Refusal>,
    archive: &Path,
    reason: &'static str,
    related: &[PathBuf],
) {
    let mut related = related.to_vec();
    related.push(archive.to_path_buf());
    related.sort();
    related.dedup();
    refused.insert(archive.to_path_buf(), Refusal { reason, related });
}

/// Resolve a source note's `done_tasks:` frontmatter link, if any.
///
/// Only actual YAML frontmatter counts: `done_tasks:` text in the body or
/// inside a fence never establishes a claim.
fn resolve_done_tasks_claim(
    note_index: &NoteIndex,
    current: &Path,
    contents: &str,
) -> Claim {
    let Some(target) = frontmatter_value(contents, "done_tasks:") else {
        return Claim::Absent;
    };
    resolve_target(note_index, current, &target)
}

/// Resolve an archive note's `parent:` frontmatter link, if any.
fn resolve_parent_claim(
    note_index: &NoteIndex,
    current: &Path,
    contents: &str,
) -> Claim {
    let Some(target) = super::link_repair::archive_parent_target(contents)
    else {
        return Claim::Absent;
    };
    resolve_target(note_index, current, &target)
}

fn resolve_target(
    note_index: &NoteIndex,
    current: &Path,
    target: &str,
) -> Claim {
    match note_index.resolve_detailed(Some(current), target) {
        LinkResolution::Found(path) => Claim::Found(path),
        LinkResolution::Ambiguous => Claim::Ambiguous,
        LinkResolution::Missing => Claim::Absent,
    }
}

/// Raw value of a frontmatter key from actual YAML frontmatter.
///
/// The block between the first `---` line and its closing `---` is the only
/// source; body lines, fences, and comments never count.
fn frontmatter_value(contents: &str, key: &str) -> Option<String> {
    let lines: Vec<&str> = contents.split_inclusive('\n').collect();
    if lines.first().map(|line| super::split_line_ending(line).0) != Some("---")
    {
        return None;
    }
    let closing =
        lines.iter().enumerate().skip(1).find_map(|(index, line)| {
            (super::split_line_ending(line).0 == "---").then_some(index)
        })?;
    for line in lines.iter().take(closing).skip(1) {
        let (content, _) = super::split_line_ending(line);
        let Some(value) = content.trim_start().strip_prefix(key).map(str::trim)
        else {
            continue;
        };
        if value.is_empty() {
            return None;
        }
        return super::link_repair::frontmatter_wiki_target(value)
            .map(str::to_string);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn index_of(entries: &[(&str, &str)]) -> ArchivePairIndex {
        let buffers: Vec<(String, String)> = entries
            .iter()
            .map(|(path, contents)| (path.to_string(), contents.to_string()))
            .collect();
        let paths: Vec<PathBuf> = entries
            .iter()
            .map(|(path, _)| PathBuf::from(path))
            .collect();
        let note_index = NoteIndex::from_paths(paths);
        ArchivePairIndex::from_buffers(&buffers, &note_index)
    }

    fn bound_pair(
        index: &ArchivePairIndex,
        source: &str,
        archive: &str,
    ) -> BoundPair {
        let lookup = index.for_source(Path::new(source));
        let SourceLookup::Bound(pair) = lookup else {
            panic!("expected {source} to bind {archive}, got {lookup:?}");
        };
        assert_eq!(pair.source, PathBuf::from(source), "source");
        assert_eq!(pair.archive, PathBuf::from(archive), "archive");
        let back = index.for_archive(Path::new(archive));
        assert_eq!(
            back,
            ArchiveLookup::Bound(pair.clone()),
            "for_archive must agree with for_source"
        );
        pair
    }

    #[test]
    fn legacy_pair_without_metadata_binds_by_filename() {
        let index = index_of(&[
            ("sase.md", "# Sase\n"),
            ("done/sase_done.md", "# Done\n"),
        ]);
        let pair = bound_pair(&index, "sase.md", "done/sase_done.md");
        assert!(!pair.via_metadata);
        assert_eq!(pair.canonical_archive, PathBuf::from("done/sase_done.md"));
    }

    #[test]
    fn nested_pair_binds_by_filename() {
        let index = index_of(&[
            ("child/Source.md", "# Source\n"),
            ("done/child/Source_done.md", "# Done\n"),
        ]);
        let pair =
            bound_pair(&index, "child/Source.md", "done/child/Source_done.md");
        assert!(!pair.via_metadata);
    }

    #[test]
    fn reciprocal_canonical_metadata_binds() {
        let index = index_of(&[
            (
                "sase.md",
                "---\ndone_tasks: \"[[done/sase_done]]\"\n---\n\n# Sase\n",
            ),
            (
                "done/sase_done.md",
                "---\nparent: \"[[sase]]\"\ntype: \"[[done]]\"\n---\n",
            ),
        ]);
        let pair = bound_pair(&index, "sase.md", "done/sase_done.md");
        assert!(pair.via_metadata);
    }

    #[test]
    fn reciprocal_noncanonical_metadata_survives_rename() {
        // The source moved from `old.md` to `new.md`; the archive keeps its
        // old filename but both sides name each other.
        let index = index_of(&[
            (
                "new.md",
                "---\ndone_tasks: \"[[done/old_done]]\"\n---\n\n# New\n",
            ),
            (
                "done/old_done.md",
                "---\nparent: \"[[new]]\"\ntype: \"[[done]]\"\n---\n",
            ),
        ]);
        let pair = bound_pair(&index, "new.md", "done/old_done.md");
        assert!(pair.via_metadata);
        assert_eq!(pair.canonical_archive, PathBuf::from("done/new_done.md"));
    }

    #[test]
    fn renamed_source_leaves_occupied_canonical_refused() {
        // `old.md` still exists as a fresh note, but the canonical
        // `done/old_done.md` belongs to the renamed `new.md`.
        let index = index_of(&[
            ("old.md", "# Old reoccupied\n"),
            (
                "new.md",
                "---\ndone_tasks: \"[[done/old_done]]\"\n---\n\n# New\n",
            ),
            (
                "done/old_done.md",
                "---\nparent: \"[[new]]\"\ntype: \"[[done]]\"\n---\n",
            ),
        ]);
        bound_pair(&index, "new.md", "done/old_done.md");
        let lookup = index.for_source(Path::new("old.md"));
        let SourceLookup::Refused { reason, .. } = lookup else {
            panic!("reoccupied old path must refuse, got {lookup:?}");
        };
        assert_eq!(reason, "occupied_canonical");
    }

    #[test]
    fn corroborating_one_sided_metadata_binds_by_filename() {
        // Only the archive names the source, and it agrees with the
        // filename pair.
        let index = index_of(&[
            ("child/Source.md", "---\nparent: \"[[Project]]\"\n---\n"),
            (
                "done/child/Source_done.md",
                "---\nparent: \"[[child/Source]]\"\ntype: \"[[done]]\"\n---\n",
            ),
        ]);
        let pair =
            bound_pair(&index, "child/Source.md", "done/child/Source_done.md");
        assert!(!pair.via_metadata);
    }

    #[test]
    fn contradictory_archive_parent_refuses() {
        let index = index_of(&[
            ("child/Source.md", "---\nparent: \"[[Project]]\"\n---\n"),
            ("Project.md", "# Project\n"),
            (
                "done/child/Source_done.md",
                "---\nparent: \"[[Project]]\"\ntype: \"[[done]]\"\n---\n",
            ),
        ]);
        let lookup = index.for_archive(Path::new("done/child/Source_done.md"));
        assert!(
            matches!(
                lookup,
                ArchiveLookup::Refused {
                    reason: "contradictory_metadata",
                    ..
                }
            ),
            "contradictory parent must refuse, got {lookup:?}"
        );
        let source = index.for_source(Path::new("child/Source.md"));
        assert!(
            matches!(
                source,
                SourceLookup::Refused {
                    reason: "occupied_canonical",
                    ..
                }
            ),
            "source with occupied canonical must refuse, got {source:?}"
        );
    }

    #[test]
    fn contradictory_source_claim_refuses() {
        let index = index_of(&[
            (
                "sase.md",
                "---\ndone_tasks: \"[[done/other_done]]\"\n---\n\n# Sase\n",
            ),
            ("other.md", "# Other\n"),
            (
                "done/sase_done.md",
                "---\nparent: \"[[sase]]\"\ntype: \"[[done]]\"\n---\n",
            ),
            (
                "done/other_done.md",
                "---\nparent: \"[[other]]\"\ntype: \"[[done]]\"\n---\n",
            ),
        ]);
        // `sase.md` claims `done/other_done.md`, which reciprocally belongs
        // to `other.md`.
        let lookup = index.for_source(Path::new("sase.md"));
        assert!(
            matches!(
                lookup,
                SourceLookup::Refused {
                    reason: "contradictory_metadata",
                    ..
                }
            ),
            "got {lookup:?}"
        );
        // The canonical archive names the conflicted source back, so the
        // poison spreads: it refuses rather than binding a guessed source.
        let archive = index.for_archive(Path::new("done/sase_done.md"));
        assert!(
            matches!(
                archive,
                ArchiveLookup::Refused {
                    reason: "contradictory_metadata",
                    ..
                }
            ),
            "got {archive:?}"
        );
        bound_pair(&index, "other.md", "done/other_done.md");
    }

    #[test]
    fn ambiguous_metadata_targets_refuse() {
        let index = index_of(&[
            ("a/shared.md", "# A\n"),
            ("b/shared.md", "# B\n"),
            ("ref.md", "---\ndone_tasks: \"[[shared]]\"\n---\n\n# Ref\n"),
            ("done/shared_done.md", "---\nparent: \"[[shared]]\"\n---\n"),
        ]);
        let source = index.for_source(Path::new("ref.md"));
        assert!(
            matches!(
                source,
                SourceLookup::Refused {
                    reason: "ambiguous_metadata",
                    ..
                }
            ),
            "got {source:?}"
        );
        let archive = index.for_archive(Path::new("done/shared_done.md"));
        assert!(
            matches!(
                archive,
                ArchiveLookup::Refused {
                    reason: "ambiguous_metadata",
                    ..
                }
            ),
            "got {archive:?}"
        );
    }

    #[test]
    fn two_sources_claiming_one_archive_refuse() {
        let index = index_of(&[
            ("one.md", "---\ndone_tasks: \"[[done/shared_done]]\"\n---\n"),
            ("two.md", "---\ndone_tasks: \"[[done/shared_done]]\"\n---\n"),
            ("done/shared_done.md", "---\nparent: \"[[one]]\"\n---\n"),
        ]);
        // `done/shared_done.md` reciprocally belongs to `one.md`; the extra
        // claimant is refused while the pair stands.
        bound_pair(&index, "one.md", "done/shared_done.md");
        let extra = index.for_source(Path::new("two.md"));
        assert!(
            matches!(
                extra,
                SourceLookup::Refused {
                    reason: "duplicate_claim",
                    ..
                }
            ),
            "got {extra:?}"
        );
    }

    #[test]
    fn two_archives_claiming_one_source_refuse() {
        let index = index_of(&[
            ("sase.md", "# Sase\n"),
            ("done/sase_done.md", "---\nparent: \"[[sase]]\"\n---\n"),
            ("done/legacy_done.md", "---\nparent: \"[[sase]]\"\n---\n"),
        ]);
        // Neither archive filename-pairs `sase.md`, so both claims collide.
        for archive in ["done/sase_done.md", "done/legacy_done.md"] {
            let lookup = index.for_archive(Path::new(archive));
            assert!(
                matches!(
                    lookup,
                    ArchiveLookup::Refused {
                        reason: "duplicate_claim",
                        ..
                    }
                ),
                "{archive}: got {lookup:?}"
            );
        }
        let source = index.for_source(Path::new("sase.md"));
        assert!(
            matches!(
                source,
                SourceLookup::Refused {
                    reason: "duplicate_claim",
                    ..
                }
            ),
            "got {source:?}"
        );
    }

    #[test]
    fn one_sided_rename_claim_refuses() {
        // The source names a noncanonical archive that does not name it
        // back: insufficient evidence to invent a renamed pair.
        let index = index_of(&[
            (
                "sase.md",
                "---\ndone_tasks: \"[[done/legacy_done]]\"\n---\n",
            ),
            ("done/legacy_done.md", "# Legacy\n"),
        ]);
        let lookup = index.for_source(Path::new("sase.md"));
        assert!(
            matches!(
                lookup,
                SourceLookup::Refused {
                    reason: "one_sided_rename",
                    ..
                }
            ),
            "got {lookup:?}"
        );
        assert_eq!(
            index.for_archive(Path::new("done/legacy_done.md")),
            ArchiveLookup::Orphan {
                archive: PathBuf::from("done/legacy_done.md"),
            }
        );
    }

    #[test]
    fn missing_endpoints_stay_unpaired() {
        let index = index_of(&[
            ("lonely.md", "# Lonely\n"),
            ("done/stray_done.md", "# Stray\n"),
        ]);
        assert_eq!(
            index.for_source(Path::new("lonely.md")),
            SourceLookup::Missing {
                canonical_archive: Some(PathBuf::from("done/lonely_done.md")),
            }
        );
        assert_eq!(
            index.for_archive(Path::new("done/stray_done.md")),
            ArchiveLookup::Orphan {
                archive: PathBuf::from("done/stray_done.md"),
            }
        );
    }

    #[test]
    fn stale_metadata_falls_back_to_filename() {
        // Both sides point at notes that no longer exist; the unique
        // filename pair still binds.
        let index = index_of(&[
            (
                "sase.md",
                "---\ndone_tasks: \"[[done/vanished_done]]\"\n---\n",
            ),
            ("done/sase_done.md", "---\nparent: \"[[vanished]]\"\n---\n"),
        ]);
        let pair = bound_pair(&index, "sase.md", "done/sase_done.md");
        assert!(!pair.via_metadata);
    }

    #[test]
    fn body_and_fence_done_tasks_do_not_claim() {
        // `done_tasks:` outside frontmatter never establishes a claim, even
        // when it names a real archive.
        let index = index_of(&[
            (
                "sase.md",
                concat!(
                    "# Sase\n\n",
                    "done_tasks: \"[[done/other_done]]\"\n\n",
                    "```\n",
                    "done_tasks: \"[[done/other_done]]\"\n",
                    "```\n",
                ),
            ),
            ("other.md", "# Other\n"),
            ("done/other_done.md", "# Other archive\n"),
        ]);
        // Body text never claims: `sase.md` stays missing with a canonical
        // destination, while the real filename pair still binds.
        assert_eq!(
            index.for_source(Path::new("sase.md")),
            SourceLookup::Missing {
                canonical_archive: Some(PathBuf::from("done/sase_done.md")),
            }
        );
        bound_pair(&index, "other.md", "done/other_done.md");
    }

    #[test]
    fn alias_fragment_and_extensionless_targets_resolve() {
        let index = index_of(&[
            (
                "Work/Notes.md",
                "---\ndone_tasks: \"[[done/Work/Notes_done|archive]]\"\n---\n",
            ),
            (
                "done/Work/Notes_done.md",
                "---\nparent: \"[[Work/Notes#^abc|label]]\"\ntype: \"[[done]]\"\n---\n",
            ),
        ]);
        let pair =
            bound_pair(&index, "Work/Notes.md", "done/Work/Notes_done.md");
        assert!(pair.via_metadata);
    }

    #[test]
    fn with_contents_rebinds_without_disk_io() {
        let index = index_of(&[("sase.md", "# Sase\n")]);
        assert!(matches!(
            index.for_source(Path::new("sase.md")),
            SourceLookup::Missing { .. }
        ));
        let rebound = index.with_contents(
            Path::new("sase.md"),
            "---\ndone_tasks: \"[[done/sase_done]]\"\n---\n",
        );
        // The archive still does not exist, so the source stays missing with
        // a canonical destination rather than binding thin air.
        assert_eq!(
            rebound.for_source(Path::new("sase.md")),
            SourceLookup::Missing {
                canonical_archive: Some(PathBuf::from("done/sase_done.md")),
            }
        );
        let rebound = rebound.with_contents(
            Path::new("done/sase_done.md"),
            "---\nparent: \"[[sase]]\"\n---\n",
        );
        bound_pair(&rebound, "sase.md", "done/sase_done.md");
    }

    #[test]
    fn read_vault_builds_reciprocal_bindings_from_disk() {
        let vault = tempfile::TempDir::new().expect("temp vault");
        let root = vault.path();
        for (relative, contents) in [
            ("sase.md", "---\ndone_tasks: \"[[done/sase_done]]\"\n---\n"),
            ("done/sase_done.md", "---\nparent: \"[[sase]]\"\n---\n"),
        ] {
            let path = root.join(relative);
            std::fs::create_dir_all(path.parent().expect("parent"))
                .expect("mkdir");
            std::fs::write(path, contents).expect("write");
        }
        let index = ArchivePairIndex::read_vault(root).expect("read");
        bound_pair(&index, "sase.md", "done/sase_done.md");
    }

    #[test]
    fn unreadable_sibling_only_poisons_involved_lookups() {
        let vault = tempfile::TempDir::new().expect("temp vault");
        let root = vault.path();
        // Invalid UTF-8 bytes: the census retains the failure instead of
        // aborting the build.
        std::fs::write(root.join("unreadable.md"), [0xff, 0xfe, 0x00, 0x9f])
            .expect("write unreadable");
        std::fs::write(
            root.join("sase.md"),
            "---\ndone_tasks: \"[[done/sase_done]]\"\n---\n",
        )
        .expect("write source");
        std::fs::create_dir_all(root.join("done")).expect("done dir");
        std::fs::write(root.join("done/sase_done.md"), "# Archive\n")
            .expect("write archive");
        let index = ArchivePairIndex::read_vault(root).expect("read");
        assert_eq!(index.census.failures.len(), 1);

        // A source with no claims and no canonical archive is provably
        // unaffected: the failed note cannot be its endpoint, target, or
        // third-party claimant.
        assert_eq!(
            index.for_source(Path::new("lonely.md")),
            SourceLookup::Missing {
                canonical_archive: Some(PathBuf::from("done/lonely_done.md")),
            }
        );
        // A source naming an existing archive is not: the failed note could
        // dispute that archive.
        let lookup = index.for_source(Path::new("sase.md"));
        assert!(
            matches!(
                lookup,
                SourceLookup::Refused {
                    reason: "incomplete_census",
                    ..
                }
            ),
            "got {lookup:?}"
        );
        // The reciprocal survivor keeps its owner: an unknown third party
        // cannot take the pair away.
        std::fs::write(
            root.join("new.md"),
            "---\ndone_tasks: \"[[done/old_done]]\"\n---\n",
        )
        .expect("write renamed");
        std::fs::write(
            root.join("done/old_done.md"),
            "---\nparent: \"[[new]]\"\n---\n",
        )
        .expect("write old archive");
        let index = ArchivePairIndex::read_vault(root).expect("reread");
        bound_pair(&index, "new.md", "done/old_done.md");
    }
}
