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

mod close_log;
mod close_selection;
mod completion;
mod draft;
mod editor_classify;
mod editor_model;
mod editor_parse;
mod editor_pomodoro;
mod item;
mod line;
mod markers;
mod model;
mod project_tasks;
mod rewrite;
mod start_selection;
mod tokens;

#[cfg(test)]
mod tests;

pub(crate) use self::completion::completion_field_at;
pub(crate) use self::completion::project_task_block_id_detail;
pub(crate) use self::completion::CompletionContext;
#[cfg(test)]
pub(crate) use self::completion::CompletionField;
pub(crate) use self::draft::parse_capture_draft_with_clip_control;
#[cfg(test)]
pub(crate) use self::draft::parse_capture_text_with_clip_control;
#[cfg(test)]
pub(crate) use self::draft::split_capture_draft;
#[cfg(test)]
pub(crate) use self::draft::split_physical_lines;
pub(crate) use self::editor_model::Diagnostic;
pub(crate) use self::editor_model::EditorGlobalDestination;
pub(crate) use self::editor_model::EditorItemParse;
pub(crate) use self::editor_model::EditorMode;
#[cfg(test)]
pub(crate) use self::editor_model::EditorParse;
pub(crate) use self::editor_model::Need;
pub(crate) use self::editor_model::Severity;
pub(crate) use self::editor_model::Span;
pub(crate) use self::editor_model::SpanKind;
pub(crate) use self::editor_parse::editor_item_at;
pub(crate) use self::editor_parse::parse_for_editor;
pub(crate) use self::editor_pomodoro::cursor_on_close_bullet_line;
pub(crate) use self::line::missing_text_error;
pub(crate) use self::line::normalize_task_text;
pub(crate) use self::line::selector_slug;
#[cfg(test)]
pub(crate) use self::markers::extract_terminal_markers;
pub(crate) use self::markers::is_route_token;
#[cfg(test)]
pub(crate) use self::markers::parse_priority_token;
#[cfg(test)]
pub(crate) use self::markers::parse_schedule_token;
pub(crate) use self::markers::unused_project_note_pomodoro_error;
pub(crate) use self::markers::POMODORO_START_FORCED_ERROR;
pub(crate) use self::model::AuthoredDepth;
pub(crate) use self::model::AuthoredSubBullet;
pub(crate) use self::model::CaptureKind;
pub(crate) use self::model::ClipRequest;
pub(crate) use self::model::CloseLogEntry;
pub(crate) use self::model::ParsedCaptureDraft;
pub(crate) use self::model::ParsedCaptureItem;
pub(crate) use self::model::ParsedCaptureText;
pub(crate) use self::model::PomodoroAdjustSpec;
pub(crate) use self::model::PomodoroCloseSpec;
pub(crate) use self::model::PomodoroLinkSpelling;
pub(crate) use self::model::PomodoroShiftSpec;
pub(crate) use self::model::PomodoroStartSpec;
pub(crate) use self::model::ProjectTaskId;
pub(crate) use self::model::SubBulletTarget;
pub(crate) use self::model::TaskSectionSelector;
pub(crate) use self::model::TaskToggleIntent;
pub(crate) use self::project_tasks::split_leading_checkbox;
pub(crate) use self::rewrite::rewrite_draft;
pub(crate) use self::rewrite::DraftRewrite;
pub(crate) use self::rewrite::RewriteRule;
pub(crate) use self::rewrite::TextEdit;
pub(crate) use self::tokens::is_block_id;
pub(crate) use self::tokens::is_pomodoro_selector_component;
