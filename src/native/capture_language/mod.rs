//! The capture grammar shared by `bob capture` and `bob capture-parse`.
//!
//! This module owns every position-agnostic classification rule for capture
//! text: draft-wide `@@` declarations, whitespace normalization, terminal
//! marker extraction, and `@token` routing. `capture/` layers execution
//! (files, clipboard, note mutation) on top of it, and `capture_parse.rs`
//! layers a span-aware, read-only editor view on the same functions. There
//! is exactly one grammar here; the editor path never re-implements token
//! classification.
//!
//! Fallible functions return `Result<T, String>` because this module has no
//! file I/O and therefore no use for `capture/`'s `CaptureError` kinds.
//! `capture/` wraps the returned message in `CaptureError::usage(...)`, so
//! the message text is the single source of truth for both callers.

mod alias_tip;
mod bare_name;
mod close_log;
mod close_selection;
mod completion;
mod dependencies;
mod directive;
mod draft;
mod drawer_bullet;
mod editor_classify;
mod editor_model;
mod editor_parse;
mod editor_pomodoro;
mod item;
mod line;
mod markers;
pub(crate) mod merge_in;
mod model;
mod move_bullet;
pub(crate) mod notebook;
pub(crate) mod past;
mod pomodoro_literal;
mod project_tasks;
pub(crate) mod record;
mod relations;
mod rewrite;
pub(crate) mod section_bullet;
mod start_selection;
pub(crate) mod target_runs;
pub(crate) mod task_sources;
pub(crate) mod template;
mod tokens;
pub(crate) mod transfer;
mod type_marker;

/// Revision of the capture grammar contract exposed to clients.
/// Revision 26 adds notebook section bullets (`- #Section` in a route-only
/// `@nb` list or `@parent^id@ %n` body). Revision 25 adds `@parent^id@ %n`
/// notebook creation. Revision 24 restores the established notebook span
/// labels with an additive `notebook` modifier. Revision 23 adds
/// `#section^id` on every route and notebook list, global, and forced
/// destination forms. Revision 22 makes sized `Nx` closes execute (see
/// `sized_pomodoro_close.md`); older clients parse the size as literal
/// task text.
pub(crate) const GRAMMAR_REVISION: u32 = 26;

#[cfg(test)]
mod tests;

pub(crate) use self::alias_tip::sigil_alias_tip;
pub(crate) use self::alias_tip::AliasStamp;
pub(crate) use self::alias_tip::SigilAliasTip;
pub(crate) use self::bare_name::can_spell_bare;
pub(crate) use self::bare_name::glued_named_start;
pub(crate) use self::bare_name::named_start_token;
pub(crate) use self::bare_name::trailing_glued_timing;
pub(crate) use self::close_log::assign_log_positions;
pub(crate) use self::close_log::lexical_worked_tasks_with_all;
pub(crate) use self::close_log::PositionalLogError;
pub(crate) use self::completion::claimed_task_complete_tokens;
pub(crate) use self::completion::completion_field_at;
pub(crate) use self::completion::completion_field_at_with;
pub(crate) use self::completion::drawer_bullet_completion_at;
pub(crate) use self::completion::merge_in_completion_role;
pub(crate) use self::completion::notebook_section_bullet_completion_at;
pub(crate) use self::completion::pomodoro_start_override_at;
pub(crate) use self::completion::project_task_block_id_detail;
pub(crate) use self::completion::project_task_move_detail;
pub(crate) use self::completion::relation_label_token_at;
pub(crate) use self::completion::transfer_completion_role;
pub(crate) use self::completion::CompletionContext;
pub(crate) use self::completion::CompletionField;
pub(crate) use self::completion::MergeInCompletionRole;
pub(crate) use self::completion::ProjectTaskMoveDetail;
pub(crate) use self::completion::TransferCompletionRole;
pub(crate) use self::dependencies::close_blocked_task_error;
pub(crate) use self::directive::clip_not_read_error;
#[cfg(test)]
pub(crate) use self::directive::incomplete_directive_error;
pub(crate) use self::draft::detect_list_header;
#[cfg(test)]
pub(crate) use self::draft::parse_capture_draft_with_options;
pub(crate) use self::draft::parse_capture_draft_with_options_coded;
#[cfg(test)]
pub(crate) use self::draft::parse_capture_text_with_options;
pub(crate) use self::draft::split_capture_draft;
pub(crate) use self::draft::split_physical_lines;
pub(crate) use self::draft::template_list_error;
pub(crate) use self::drawer_bullet::empty_drawer_bullet_error;
pub(crate) use self::drawer_bullet::existing_drawer_empty_error;
pub(crate) use self::drawer_bullet::first_level_groups as drawer_bullet_groups;
pub(crate) use self::drawer_bullet::is_all_caps_drawer_title;
pub(crate) use self::drawer_bullet::multi_note_clip_error;
pub(crate) use self::drawer_bullet::multi_note_priority_error;
pub(crate) use self::drawer_bullet::multi_note_schedule_error;
pub(crate) use self::drawer_bullet::DrawerBulletTarget;
pub(crate) use self::editor_model::DependencyEntry;
pub(crate) use self::editor_model::DependencyTarget;
pub(crate) use self::editor_model::DependencyTargetKind;
pub(crate) use self::editor_model::Diagnostic;
pub(crate) use self::editor_model::DiagnosticRewrite;
pub(crate) use self::editor_model::EditorGlobalDestination;
pub(crate) use self::editor_model::EditorItemParse;
pub(crate) use self::editor_model::EditorMergeIn;
pub(crate) use self::editor_model::EditorMode;
pub(crate) use self::editor_model::EditorParse;
pub(crate) use self::editor_model::EditorTarget;
pub(crate) use self::editor_model::EditorTransfer;
pub(crate) use self::editor_model::EditorTransferItem;
pub(crate) use self::editor_model::Need;
pub(crate) use self::editor_model::RelationDirection;
pub(crate) use self::editor_model::RelationEntry;
pub(crate) use self::editor_model::Severity;
pub(crate) use self::editor_model::Span;
pub(crate) use self::editor_model::SpanKind;
pub(crate) use self::editor_parse::editor_item_at;
pub(crate) use self::editor_parse::editor_item_at_with;
pub(crate) use self::editor_parse::parse_for_editor;
pub(crate) use self::editor_parse::parse_for_editor_with;
pub(crate) use self::editor_parse::tokenize_line_with_spans;
pub(crate) use self::editor_pomodoro::cursor_in_close_log_text;
pub(crate) use self::line::album_holds_no_tasks_error;
pub(crate) use self::line::missing_text_error;
pub(crate) use self::line::normalize_task_text;
pub(crate) use self::line::notebook_holds_no_tasks_error;
pub(crate) use self::line::selector_slug;
#[cfg(test)]
pub(crate) use self::markers::extract_terminal_markers;
pub(crate) use self::markers::is_route_token;
#[cfg(test)]
pub(crate) use self::markers::parse_priority_token;
#[cfg(test)]
pub(crate) use self::markers::parse_retired_priority_token;
#[cfg(test)]
pub(crate) use self::markers::parse_schedule_token;
pub(crate) use self::markers::unused_project_note_offer_pomodoro_error;
pub(crate) use self::markers::unused_project_note_pomodoro_error;
pub(crate) use self::markers::POMODORO_CLOSE_INTERNAL_BULLETS_ERROR;
pub(crate) use self::markers::POMODORO_START_FORCED_ERROR;
pub(crate) use self::model::close_selection_is_explicit;
pub(crate) use self::model::tokenize_with_spans;
pub(crate) use self::model::AuthoredDepth;
pub(crate) use self::model::AuthoredSubBullet;
pub(crate) use self::model::CaptureKind;
pub(crate) use self::model::CaptureParseOptions;
pub(crate) use self::model::ClipRequest;
pub(crate) use self::model::CloseLogEntry;
pub(crate) use self::model::CloseLogOrigin;
pub(crate) use self::model::CloseSize;
pub(crate) use self::model::DrawerSelector;
pub(crate) use self::model::EditorParseOptions;
pub(crate) use self::model::FanoutContext;
pub(crate) use self::model::ListContext;
pub(crate) use self::model::ListKind;
pub(crate) use self::model::MergeInClause;
pub(crate) use self::model::MergeInSource as ParsedMergeInSource;
pub(crate) use self::model::MoveBullet;
pub(crate) use self::model::MoveChoiceBullet;
pub(crate) use self::model::MoveVia;
pub(crate) use self::model::ParsedCaptureDraft;
pub(crate) use self::model::ParsedCaptureItem;
pub(crate) use self::model::ParsedCaptureText;
pub(crate) use self::model::ParsedDependency;
pub(crate) use self::model::ParsedDependencyTarget;
pub(crate) use self::model::ParsedDependencyTargetKind;
pub(crate) use self::model::ParsedRelation;
pub(crate) use self::model::PastEdge;
pub(crate) use self::model::PastSegment;
pub(crate) use self::model::PastSegmentKind;
pub(crate) use self::model::PastStackPos;
pub(crate) use self::model::PomodoroAdjustSpec;
pub(crate) use self::model::PomodoroCloseSpec;
pub(crate) use self::model::PomodoroLinkSpelling;
pub(crate) use self::model::PomodoroMergeSpec;
pub(crate) use self::model::PomodoroPastSpec;
pub(crate) use self::model::PomodoroShiftSpec;
pub(crate) use self::model::PomodoroSplitSpec;
pub(crate) use self::model::PomodoroStartSpec;
pub(crate) use self::model::ProjectNoteForm;
pub(crate) use self::model::ProjectTaskId;
pub(crate) use self::model::RawLine;
pub(crate) use self::model::RefParentSource;
pub(crate) use self::model::SubBulletTarget;
pub(crate) use self::model::TaskToggleIntent;
pub(crate) use self::model::TemplateRowSummary;
pub(crate) use self::notebook::{
    candidate_notebook_route, candidate_notebook_routes,
    discover_notebook_creates, notebook_section_ambiguous_message,
    notebook_section_near_miss_message, notebook_sections,
    resolve_notebook_section, staged_notebook_sections,
    DiscoveredNotebookCreate, NotebookRouteInfo, NotebookRoutePolicy,
    NotebookSectionResolution,
};
pub(crate) use self::pomodoro_literal::canonicalize_exact_name;
pub(crate) use self::project_tasks::split_leading_checkbox;
pub(crate) use self::record::album_task_home_invalid_message;
pub(crate) use self::record::candidate_collection_routes;
pub(crate) use self::record::candidate_dirs;
pub(crate) use self::record::is_addressable_update_stem;
pub(crate) use self::record::AlbumRouteMeta;
pub(crate) use self::record::PerRouteDecision;
pub(crate) use self::record::RecordNoteIntent;
pub(crate) use self::record::RecordRoutePolicy;
pub(crate) use self::record::ALBUM_HOLDS_NO_TASKS_CODE;
pub(crate) use self::record::ALBUM_SCHEMA_INVALID_CODE;
pub(crate) use self::record::RECORDS_CONFIG_INVALID_CODE;
pub(crate) use self::record::RECORD_ASSIST_VERSION;
pub(crate) use self::rewrite::rewrite_draft;
pub(crate) use self::rewrite::DraftRewrite;
pub(crate) use self::rewrite::RewriteRule;
pub(crate) use self::rewrite::TextEdit;
pub(crate) use self::tokens::is_block_id;
pub(crate) use self::tokens::is_pomodoro_selector_component;
pub(crate) use self::type_marker::completion_at_raw as task_type_completion_at;
pub(crate) use self::type_marker::not_applicable_error;
pub(crate) use self::type_marker::resolve_accepted_type_name;
pub(crate) use self::type_marker::resolve_exact_type_name;
pub(crate) use self::type_marker::scan_line;
pub(crate) use self::type_marker::unmatched_type_display;
pub(crate) use self::type_marker::ResolvedTypeName;
pub(crate) use self::type_marker::TypeMarkerOccurrence;
pub(crate) use self::type_marker::{replacement_for, TypeMarker};
