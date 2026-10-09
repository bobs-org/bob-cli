//! Shared types and their inherent impls for highlights_ref.
use super::*;
use crate::native::env as bob_env;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Config {
    pub(crate) bob_dir: PathBuf,
    pub(crate) lib_dir: PathBuf,
    pub(crate) ref_dir: PathBuf,
    pub(crate) xlib_dir: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct PreScanHook {
    pub(super) command: OsString,
}

impl PreScanHook {
    pub(super) fn display(&self) -> String {
        self.command.to_string_lossy().into_owned()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Prefer {
    Marker,
    Frontmatter,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct SyncOptions {
    pub(super) dry_run: bool,
    pub(super) write_pdf: bool,
    pub(super) prefer: Option<Prefer>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "type", content = "value")]
pub(super) enum MarkerValue {
    Null,
    Bool(bool),
    Number(String),
    String(String),
    List(Vec<MarkerValue>),
}

pub(super) type Projection = BTreeMap<String, MarkerValue>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CommandError {
    pub(super) message: String,
    /// Process exit code for this failure (`None` means 1). An
    /// interrupted listen command exits 130.
    pub(super) exit_code: Option<i32>,
    /// Machine-readable `ref scan --format json` failure code
    /// (`scan_busy`, `hook_failed`, `intake_collision`,
    /// `output_collision`, `dirty_targets`); `None` means `scan_failed`.
    /// Human rendering ignores it.
    pub(super) code: Option<&'static str>,
    /// Vault paths the failure site knows (colliding destinations,
    /// colliding targets, dirty files); human rendering ignores them.
    pub(super) paths: Vec<PathBuf>,
}

impl CommandError {
    pub(super) fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            exit_code: None,
            code: None,
            paths: Vec::new(),
        }
    }

    pub(super) fn with_exit_code(mut self, code: i32) -> Self {
        self.exit_code = Some(code);
        self
    }

    pub(super) fn with_code(mut self, code: &'static str) -> Self {
        self.code = Some(code);
        self
    }

    pub(super) fn with_paths(mut self, paths: Vec<PathBuf>) -> Self {
        self.paths = paths;
        self
    }
}

impl fmt::Display for CommandError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl StdError for CommandError {}

pub(crate) type Result<T> = std::result::Result<T, CommandError>;

#[derive(Debug, Clone)]
pub(super) struct PdfMarker {
    pub(super) annotation_id: ObjectId,
    pub(super) contents: String,
    pub(super) page_number: u32,
    pub(super) note_number: usize,
    pub(super) source_pdf_sha256: String,
}

#[derive(Debug, Clone)]
pub(super) struct FrontmatterEntry {
    pub(super) key: Option<String>,
    pub(super) value: Option<MarkerValue>,
    pub(super) raw: String,
}

#[derive(Debug, Clone)]
pub(super) struct ParsedNote {
    pub(super) frontmatter: Vec<FrontmatterEntry>,
    pub(super) body: String,
    pub(super) original: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct PdfTaskLine {
    pub(super) line_index: usize,
    pub(super) checkbox_mark_index: usize,
    pub(super) checked: bool,
    pub(super) mark: char,
}

impl PdfTaskLine {
    pub(super) fn status(self) -> PdfTaskStatus {
        match self.mark {
            ' ' => PdfTaskStatus::Ready,
            '*' => PdfTaskStatus::Next,
            '/' => PdfTaskStatus::Wip,
            '?' => PdfTaskStatus::Blocked,
            'x' | 'X' => PdfTaskStatus::Read,
            '-' => PdfTaskStatus::Abandoned,
            _ => unreachable!("parsed PDF task has an unsupported mark"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum PdfTaskLineState {
    Missing,
    Present(PdfTaskLine),
}

impl PdfTaskLineState {
    pub(super) fn status(self) -> PdfTaskStatus {
        match self {
            PdfTaskLineState::Missing => PdfTaskStatus::Missing,
            PdfTaskLineState::Present(task_line) => task_line.status(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum PdfTaskStatus {
    Missing,
    Ready,
    Next,
    Wip,
    Blocked,
    Read,
    Abandoned,
}

impl PdfTaskStatus {
    pub(super) fn target_status(self) -> Option<&'static str> {
        match self {
            PdfTaskStatus::Ready => Some(STATUS_READY),
            PdfTaskStatus::Next => Some(STATUS_NEXT),
            PdfTaskStatus::Wip => Some(STATUS_WIP),
            PdfTaskStatus::Blocked => None,
            PdfTaskStatus::Read => Some(STATUS_READ),
            PdfTaskStatus::Abandoned => Some(STATUS_ABANDONED),
            PdfTaskStatus::Missing => None,
        }
    }

    /// Status-aware target for the sync signal and the `ref_library` seam.
    /// Lifecycle variants return their fixed target. `Blocked` agrees with
    /// any open selected status (returning its matching constant, so the
    /// signal is a no-op) and reopens a terminal selected status to `ready`.
    pub(super) fn target_status_given(
        self,
        current: Option<&str>,
    ) -> Option<&'static str> {
        match self {
            PdfTaskStatus::Ready => Some(STATUS_READY),
            PdfTaskStatus::Next => Some(STATUS_NEXT),
            PdfTaskStatus::Wip => Some(STATUS_WIP),
            PdfTaskStatus::Read => Some(STATUS_READ),
            PdfTaskStatus::Abandoned => Some(STATUS_ABANDONED),
            PdfTaskStatus::Missing => None,
            PdfTaskStatus::Blocked => match current {
                Some(STATUS_READY) => Some(STATUS_READY),
                Some(STATUS_NEXT) => Some(STATUS_NEXT),
                Some(STATUS_WIP) => Some(STATUS_WIP),
                Some(STATUS_LEGACY) => Some(STATUS_LEGACY),
                Some(STATUS_READ) | Some(STATUS_ABANDONED) => {
                    Some(STATUS_READY)
                }
                _ => None,
            },
        }
    }

    pub(super) fn label(self) -> &'static str {
        match self {
            PdfTaskStatus::Missing => "missing",
            PdfTaskStatus::Ready => "ready",
            PdfTaskStatus::Next => "next",
            PdfTaskStatus::Wip => "in-progress",
            PdfTaskStatus::Blocked => "blocked",
            PdfTaskStatus::Read => "checked",
            PdfTaskStatus::Abandoned => "cancelled",
        }
    }

    pub(super) fn contribution_reason(self) -> Option<&'static str> {
        match self {
            PdfTaskStatus::Ready => Some("ready PDF task set status ready"),
            PdfTaskStatus::Next => Some("next PDF task set status next"),
            PdfTaskStatus::Wip => Some("in-progress PDF task set status wip"),
            PdfTaskStatus::Blocked => {
                Some("blocked PDF task reopened status ready")
            }
            PdfTaskStatus::Read => Some("checked PDF task set status read"),
            PdfTaskStatus::Abandoned => {
                Some("cancelled PDF task set status abandoned")
            }
            PdfTaskStatus::Missing => None,
        }
    }

    pub(super) fn conflict_action(self) -> &'static str {
        match self {
            PdfTaskStatus::Ready
            | PdfTaskStatus::Next
            | PdfTaskStatus::Wip
            | PdfTaskStatus::Blocked => "change",
            PdfTaskStatus::Read => "uncheck",
            PdfTaskStatus::Abandoned => "uncancel",
            PdfTaskStatus::Missing => "clear",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct PdfTaskStatusSignal {
    pub(super) status: PdfTaskStatus,
    pub(super) status_contributed: Option<&'static str>,
}

#[derive(Debug, Clone)]
pub(super) struct PipelineMetadata {
    pub(super) source_pdf: String,
    pub(super) source_pdf_sha256: String,
    pub(super) ref_type: Option<String>,
    pub(super) highlights_sidecar: Option<MarkerValue>,
    pub(super) highlights_count: Option<MarkerValue>,
    pub(super) highlights_synced_at: Option<MarkerValue>,
    /// New-note creation timestamp (`created` frontmatter value) generated
    /// from the note-writing invocation's clock. `Some` only when planning a
    /// note that does not exist yet; existing notes keep their authored
    /// `created` line instead of a fresh timestamp.
    pub(super) created: Option<String>,
    /// Vault-relative library path of a same-stem companion audio file
    /// (`lib/<type>/<stem>.mp3`). Omitted when no companion exists.
    pub(super) audio: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct PdfPathMetadata {
    pub(super) relative_pdf_path: Option<PathBuf>,
    pub(super) note_relative_path: PathBuf,
    pub(super) ref_type: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct IntakeMove {
    pub(super) source: PathBuf,
    pub(super) destination: PathBuf,
    pub(super) companions: Vec<(PathBuf, PathBuf)>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum SyncSource {
    Marker,
    Frontmatter,
    AutoMerge,
}

#[derive(Debug, Clone)]
pub(super) struct SyncDecision {
    pub(super) source: SyncSource,
    pub(super) reason: String,
    pub(super) marker_contributed: bool,
    pub(super) frontmatter_contributed: bool,
}

#[derive(Debug, Clone)]
pub(super) struct SyncResolution {
    pub(super) decision: SyncDecision,
    pub(super) projection: Projection,
}

#[derive(Debug, Clone, Copy)]
pub(super) struct SyncInputs<'a> {
    pub(super) last_hash: Option<&'a str>,
    pub(super) base_projection: Option<&'a Projection>,
    pub(super) marker_projection: &'a Projection,
    pub(super) marker_hash: &'a str,
    pub(super) frontmatter_projection: &'a Projection,
    pub(super) frontmatter_hash: &'a str,
    pub(super) note_exists: bool,
    pub(super) prefer: Option<Prefer>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ProjectionConflict {
    pub(super) key: String,
    pub(super) base: Option<MarkerValue>,
    pub(super) marker: Option<MarkerValue>,
    pub(super) frontmatter: Option<MarkerValue>,
}

#[derive(Debug, Clone)]
pub(super) struct SidecarInput {
    pub(super) path: PathBuf,
    pub(super) annotations: Vec<SidecarAnnotation>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct SidecarAnnotation {
    pub(super) kind: SidecarAnnotationKind,
    pub(super) page_label: Option<String>,
    pub(super) linked_page_style: bool,
    pub(super) text: String,
    pub(super) comment: Option<String>,
    pub(super) task_source: Option<String>,
    pub(super) image: Option<SidecarImage>,
    pub(super) order: usize,
    pub(super) ordinal_on_page: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct SidecarImage {
    pub(super) target: String,
    pub(super) alt_text: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct SidecarPageHeading {
    pub(super) label: String,
    pub(super) linked_page_style: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum SidecarAnnotationKind {
    Highlight,
    Image,
    StandaloneNote,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct RenderedHighlights {
    pub(super) content: String,
    pub(super) count: usize,
    pub(super) image_count: usize,
    pub(super) image_assets: Vec<ImageAssetWrite>,
    pub(super) block_ids_by_annotation_order: BTreeMap<usize, String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ImageAssetWrite {
    pub(super) annotation_order: usize,
    pub(super) source_path: PathBuf,
    pub(super) dest_path: PathBuf,
    pub(super) vault_relative_dest_path: PathBuf,
    pub(super) source_sha256: String,
    pub(super) block_id: String,
    pub(super) action: ImageAssetAction,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ImageAssetAction {
    Copy,
    None,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct AnnotationTaskCandidate {
    pub(super) identity: String,
    pub(super) task_text: String,
    pub(super) source_block_id: String,
    pub(super) target: AnnotationTaskTarget,
    pub(super) source_ref_note_path: PathBuf,
    pub(super) processed_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum AnnotationTaskTarget {
    ReferenceNote,
    RoutedNote(PathBuf),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct AnnotationTaskSource {
    pub(super) identity: String,
    pub(super) task_text: String,
    pub(super) route_name: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) struct ProcessedTaskIndex {
    pub(super) legacy_source_task_anchors: BTreeSet<String>,
    pub(super) processed_ids: BTreeSet<String>,
    pub(super) legacy_identities: BTreeSet<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct RoutedTaskNoteWrite {
    pub(super) path: PathBuf,
    pub(super) original_contents: String,
    pub(super) rendered_contents: String,
    pub(super) action: &'static str,
}

#[derive(Debug, Clone)]
pub(super) struct PdfSyncPlan {
    pub(super) pdf: PathBuf,
    pub(super) note_path: PathBuf,
    pub(super) sidecar_path: Option<PathBuf>,
    pub(super) marker: PdfMarker,
    pub(super) decision: SyncDecision,
    pub(super) rendered_highlights_count: Option<usize>,
    pub(super) synced_projection: Projection,
    pub(super) synced_hash: String,
    pub(super) rendered_marker: String,
    pub(super) marker_write_needed: bool,
    pub(super) note: ParsedNote,
    pub(super) sidecar: Option<SidecarInput>,
    pub(super) rendered_highlights: Option<RenderedHighlights>,
    pub(super) stable_metadata: PipelineMetadata,
    pub(super) rendered_body: String,
    pub(super) stable_rendered_note: String,
    pub(super) stable_note_action: &'static str,
    pub(super) image_assets: Vec<ImageAssetWrite>,
    pub(super) annotation_task_candidates: Vec<AnnotationTaskCandidate>,
    pub(super) annotation_tasks_created: usize,
    pub(super) annotation_tasks_skipped: usize,
    pub(super) routed_task_note_writes: Vec<RoutedTaskNoteWrite>,
    pub(super) pdf_task_signal: PdfTaskStatusSignal,
    pub(super) status_normalization: StatusNormalization,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct SyncWriteReport {
    pub(super) note_action: &'static str,
    pub(super) marker_action: &'static str,
    pub(super) image_count: usize,
    pub(super) image_assets_written: usize,
    pub(super) image_assets_skipped: usize,
    pub(super) routed_note_actions: usize,
    pub(super) annotation_tasks_created: usize,
    pub(super) annotation_tasks_skipped: usize,
}

#[derive(Debug, Clone)]
pub(super) struct ScanFailure {
    pub(super) pdf: PathBuf,
    pub(super) error: CommandError,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(super) struct StatusNormalization {
    pub(super) marker: Option<DeprecatedStatusNormalization>,
    pub(super) frontmatter: Option<DeprecatedStatusNormalization>,
    pub(super) base: Option<DeprecatedStatusNormalization>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum DeprecatedStatusNormalization {
    UnreadToReady,
    DoneToRead,
}

impl DeprecatedStatusNormalization {
    pub(super) fn label(self) -> &'static str {
        match self {
            DeprecatedStatusNormalization::UnreadToReady => "unread->ready",
            DeprecatedStatusNormalization::DoneToRead => "done->read",
        }
    }
}

#[derive(Debug, Clone)]
pub(super) struct NormalizedProjection {
    pub(super) projection: Projection,
    pub(super) status_normalized: Option<DeprecatedStatusNormalization>,
}

#[derive(Debug, Clone)]
pub(super) enum ScanPlanOutcome {
    Planned(Box<PdfSyncPlan>),
    Failed(ScanFailure),
}

#[derive(Debug, Clone)]
pub(super) enum ScanWriteOutcome {
    Written(SyncWriteReport),
    Failed(ScanFailure),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum GitStatus {
    MissingCommand,
    NotWorktree,
    Worktree { entries: Vec<GitStatusEntry> },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct GitStatusEntry {
    pub(super) index_status: char,
    pub(super) worktree_status: char,
    pub(super) path: PathBuf,
    pub(super) raw: String,
}

impl Config {
    pub(crate) fn for_vault(bob_dir: &Path) -> Self {
        Self {
            lib_dir: vault_configured_path(
                bob_dir,
                ENV_LIB_DIR,
                DEFAULT_LIB_DIR,
            ),
            ref_dir: vault_configured_path(
                bob_dir,
                ENV_REF_DIR,
                DEFAULT_REF_DIR,
            ),
            xlib_dir: vault_configured_path(
                bob_dir,
                ENV_XLIB_DIR,
                DEFAULT_XLIB_DIR,
            ),
            bob_dir: bob_dir.to_path_buf(),
        }
    }

    pub(super) fn from_matches(matches: &ArgMatches) -> Self {
        let bob_dir = matches
            .get_one::<OsString>("bob-dir")
            .map(PathBuf::from)
            .map(|path| bob_env::expand_tilde(&path))
            .unwrap_or_else(bob_env::bob_dir);
        let lib_dir = configured_path(
            matches,
            "lib-dir",
            ENV_LIB_DIR,
            DEFAULT_LIB_DIR,
            &bob_dir,
        );
        let ref_dir = configured_path(
            matches,
            "ref-dir",
            ENV_REF_DIR,
            DEFAULT_REF_DIR,
            &bob_dir,
        );
        let xlib_dir = configured_path(
            matches,
            "xlib-dir",
            ENV_XLIB_DIR,
            DEFAULT_XLIB_DIR,
            &bob_dir,
        );
        Self {
            bob_dir,
            lib_dir,
            ref_dir,
            xlib_dir,
        }
    }
}

/// Resolve a vault-relative directory for [`Config::for_vault`]: the env
/// override when set and non-empty, else the default under `bob_dir`.
/// Mirrors [`configured_path`](super::note::configured_path) without Clap.
fn vault_configured_path(
    bob_dir: &Path,
    env_name: &str,
    default_value: &str,
) -> PathBuf {
    let configured = bob_env::var_os(env_name)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(default_value));
    super::note::resolve_under_bob(bob_dir, &configured)
}

impl Prefer {
    pub(super) fn as_str(self) -> &'static str {
        match self {
            Prefer::Marker => "marker",
            Prefer::Frontmatter => "frontmatter",
        }
    }
}

impl SyncSource {
    pub(super) fn as_str(self) -> &'static str {
        match self {
            SyncSource::Marker => "marker",
            SyncSource::Frontmatter => "frontmatter",
            SyncSource::AutoMerge => "auto-merge",
        }
    }
}

impl MarkerValue {
    pub(super) fn as_marker_value(&self) -> String {
        match self {
            MarkerValue::Null => "null".to_string(),
            MarkerValue::Bool(value) => value.to_string(),
            MarkerValue::Number(value) => value.clone(),
            MarkerValue::String(value) => render_marker_scalar_string(value),
            MarkerValue::List(values) => {
                let values = values
                    .iter()
                    .map(MarkerValue::as_marker_list_value)
                    .collect::<Vec<_>>()
                    .join(", ");
                format!("[{values}]")
            }
        }
    }

    pub(super) fn as_marker_list_value(&self) -> String {
        match self {
            MarkerValue::Null => "null".to_string(),
            MarkerValue::Bool(value) => value.to_string(),
            MarkerValue::Number(value) => value.clone(),
            MarkerValue::String(value) => render_marker_list_string(value),
            MarkerValue::List(values) => {
                let values = values
                    .iter()
                    .map(MarkerValue::as_marker_list_value)
                    .collect::<Vec<_>>()
                    .join(", ");
                format!("[{values}]")
            }
        }
    }

    pub(super) fn as_frontmatter_value(&self) -> String {
        match self {
            MarkerValue::Null => "null".to_string(),
            MarkerValue::Bool(value) => value.to_string(),
            MarkerValue::Number(value) => value.clone(),
            MarkerValue::String(value) => render_frontmatter_string(value),
            MarkerValue::List(values) => {
                let values = values
                    .iter()
                    .map(MarkerValue::as_frontmatter_value)
                    .collect::<Vec<_>>()
                    .join(", ");
                format!("[{values}]")
            }
        }
    }

    pub(super) fn as_string(&self) -> Option<&str> {
        match self {
            MarkerValue::String(value) => Some(value.as_str()),
            _ => None,
        }
    }

    pub(super) fn is_empty_required_value(&self) -> bool {
        match self {
            MarkerValue::Null => true,
            MarkerValue::String(value) => value.trim().is_empty(),
            MarkerValue::List(values) => values.is_empty(),
            _ => false,
        }
    }
}

impl ParsedNote {
    pub(super) fn empty() -> Self {
        Self {
            frontmatter: Vec::new(),
            body: String::new(),
            original: None,
        }
    }

    pub(super) fn exists(&self) -> bool {
        !self.frontmatter.is_empty() || !self.body.is_empty()
    }

    pub(super) fn contents(&self) -> Option<String> {
        self.exists().then(|| {
            self.original
                .clone()
                .unwrap_or_else(|| self.render_original())
        })
    }

    pub(super) fn render_original(&self) -> String {
        if self.frontmatter.is_empty() {
            return self.body.clone();
        }
        let mut rendered = String::from("---\n");
        for entry in &self.frontmatter {
            rendered.push_str(&entry.raw);
            rendered.push('\n');
        }
        rendered.push_str("---\n");
        rendered.push_str(&self.body);
        rendered
    }

    pub(super) fn marker_hash(&self) -> Option<String> {
        self.frontmatter.iter().find_map(|entry| {
            (entry.key.as_deref() == Some(FIELD_MARKER_HASH))
                .then(|| entry.value.as_ref()?.as_string().map(str::to_string))
                .flatten()
        })
    }

    pub(super) fn marker_base_projection_with_normalization(
        &self,
    ) -> Result<(Option<Projection>, Option<DeprecatedStatusNormalization>)>
    {
        let Some(entry) = self
            .frontmatter
            .iter()
            .find(|entry| entry.key.as_deref() == Some(FIELD_MARKER_BASE))
        else {
            return Ok((None, None));
        };
        let Some(value) = &entry.value else {
            return Err(CommandError::new(format!(
                "{FIELD_MARKER_BASE} must be a compact JSON string"
            )));
        };
        let Some(json) = value.as_string() else {
            return Err(CommandError::new(format!(
                "{FIELD_MARKER_BASE} must be a compact JSON string"
            )));
        };
        let mut projection = projection_from_snapshot_json(json)?;
        let status_normalized = normalize_deprecated_status(&mut projection);
        Ok((Some(projection), status_normalized))
    }

    pub(super) fn marker_fields(&self) -> BTreeSet<String> {
        self.frontmatter
            .iter()
            .find_map(|entry| {
                (entry.key.as_deref() == Some(FIELD_MARKER_FIELDS))
                    .then(|| entry.value.as_ref().and_then(value_as_string_set))
                    .flatten()
            })
            .unwrap_or_default()
    }

    pub(super) fn frontmatter_value(&self, field: &str) -> Option<MarkerValue> {
        self.frontmatter.iter().find_map(|entry| {
            (entry.key.as_deref() == Some(field))
                .then(|| entry.value.clone())
                .flatten()
        })
    }

    pub(super) fn synced_projection_with_normalization(
        &self,
    ) -> Result<NormalizedProjection> {
        let marker_fields = self.marker_fields();
        let mut projection = Projection::new();

        for entry in &self.frontmatter {
            let Some(key) = &entry.key else {
                continue;
            };
            // `created` is note-local provenance, never part of the synced
            // projection: exclude it even when a stale
            // `highlights_marker_fields` list still names it.
            if key == FIELD_CREATED {
                continue;
            }
            if is_managed_frontmatter_field(key) {
                continue;
            }
            if (is_standard_user_field(key) || marker_fields.contains(key))
                && let Some(value) = &entry.value
            {
                projection.insert(key.clone(), value.clone());
            }
        }

        canonicalize_parent(&mut projection, "frontmatter")?;
        let status_normalized = normalize_deprecated_status(&mut projection);
        Ok(NormalizedProjection {
            projection,
            status_normalized,
        })
    }

    pub(super) fn render_with_projection(
        &self,
        projection: &Projection,
        marker_hash: &str,
        metadata: &PipelineMetadata,
        body: &str,
    ) -> String {
        let old_marker_fields = self.marker_fields();
        let mut removed_keys = BTreeSet::new();
        removed_keys
            .extend(PIPELINE_FIELDS.iter().map(|field| (*field).to_string()));
        removed_keys.extend(
            COMMAND_MANAGED_FIELDS
                .iter()
                .map(|field| (*field).to_string()),
        );
        removed_keys.extend(
            COMMON_USER_FIELDS
                .iter()
                .chain(MARKER_REQUIRED_KEYS.iter())
                .map(|field| (*field).to_string()),
        );
        removed_keys.extend(old_marker_fields);
        removed_keys.extend(projection.keys().cloned());
        // `created` is note-local provenance: keep the authored raw line on
        // existing notes instead of treating it as managed output, even when
        // a stale `highlights_marker_fields` list still names it.
        removed_keys.remove(FIELD_CREATED);

        let mut lines = Vec::new();
        let mut rendered_command_managed_fields = false;
        for key in ordered_projection_keys(projection) {
            let Some(value) = projection.get(&key) else {
                continue;
            };
            lines.push(format!("{key}: {}", value.as_frontmatter_value()));
            if key == FIELD_PARENT {
                push_command_managed_frontmatter_lines(&mut lines, metadata);
                rendered_command_managed_fields = true;
            }
        }
        if !rendered_command_managed_fields {
            push_command_managed_frontmatter_lines(&mut lines, metadata);
        }

        for entry in &self.frontmatter {
            match &entry.key {
                Some(key) if removed_keys.contains(key) => {}
                _ => lines.push(entry.raw.clone()),
            }
        }

        let marker_fields = unknown_synced_fields(projection);
        // New notes carry exactly one generated `created` line. Existing
        // notes keep their preserved authored raw line above and never
        // receive a fresh timestamp here.
        if let Some(created) = &metadata.created
            && !self
                .frontmatter
                .iter()
                .any(|entry| entry.key.as_deref() == Some(FIELD_CREATED))
        {
            lines.push(format!("{FIELD_CREATED}: {created}"));
        }
        lines.push(format!(
            "{FIELD_SOURCE_PDF}: {}",
            MarkerValue::String(metadata.source_pdf.clone())
                .as_frontmatter_value()
        ));
        lines.push(format!(
            "{FIELD_SOURCE_PDF_SHA256}: {}",
            MarkerValue::String(metadata.source_pdf_sha256.clone())
                .as_frontmatter_value()
        ));
        if let Some(value) = &metadata.highlights_sidecar {
            lines.push(format!(
                "{FIELD_HIGHLIGHTS_SIDECAR}: {}",
                value.as_frontmatter_value()
            ));
        }
        if let Some(value) = &metadata.highlights_count {
            lines.push(format!(
                "{FIELD_HIGHLIGHTS_COUNT}: {}",
                value.as_frontmatter_value()
            ));
        }
        if let Some(value) = &metadata.highlights_synced_at {
            lines.push(format!(
                "{FIELD_HIGHLIGHTS_SYNCED_AT}: {}",
                value.as_frontmatter_value()
            ));
        }
        lines.push(format!(
            "{FIELD_MARKER_HASH}: {}",
            MarkerValue::String(marker_hash.to_string()).as_frontmatter_value()
        ));
        lines.push(format!(
            "{FIELD_MARKER_BASE}: {}",
            MarkerValue::String(projection_snapshot_json(projection))
                .as_frontmatter_value()
        ));
        if !marker_fields.is_empty() {
            let value = MarkerValue::List(
                marker_fields
                    .into_iter()
                    .map(MarkerValue::String)
                    .collect::<Vec<_>>(),
            );
            lines.push(format!(
                "{FIELD_MARKER_FIELDS}: {}",
                value.as_frontmatter_value()
            ));
        }
        lines.push(format!(
            "{FIELD_PIPELINE_VERSION}: {}",
            MarkerValue::String(PIPELINE_VERSION.to_string())
                .as_frontmatter_value()
        ));

        let mut rendered = String::from("---\n");
        for line in lines {
            rendered.push_str(&line);
            rendered.push('\n');
        }
        rendered.push_str("---\n");
        rendered.push_str(body);
        rendered
    }

    pub(super) fn render_body(
        &self,
        pdf: &Path,
        projection: &Projection,
        source_pdf: &str,
        rendered_highlights: Option<&RenderedHighlights>,
        metadata: &PipelineMetadata,
    ) -> Result<String> {
        if !self.exists() {
            return Ok(default_note_body(
                pdf,
                projection,
                source_pdf,
                rendered_highlights,
                metadata.audio.as_deref(),
            ));
        }

        let Some(_) = self.managed_region()? else {
            return Err(CommandError::new(
                "existing reference note is missing the managed Highlights region; add <!-- highlights:begin --> and <!-- highlights:end --> before syncing",
            ));
        };

        let body = if let Some(rendered_highlights) = rendered_highlights {
            let replacement = rendered_highlights.content.as_str();
            let body = replace_managed_region(&self.body, replacement)?;
            rewrite_pdf_task_checkbox_for_projection(&body, projection)?
        } else {
            rewrite_pdf_task_checkbox_for_projection(&self.body, projection)?
        };
        Ok(maybe_insert_audio_embed(self, metadata, &body))
    }

    pub(super) fn managed_region(&self) -> Result<Option<&str>> {
        if !self.exists() {
            return Ok(None);
        }
        managed_region(&self.body)
    }

    pub(super) fn generated_block_ids(&self) -> Result<BTreeSet<String>> {
        let Some(region) = self.managed_region()? else {
            return Ok(BTreeSet::new());
        };
        Ok(generated_block_ids(region))
    }
}
