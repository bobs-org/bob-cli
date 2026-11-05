//! Clipboard capture planning, persistence, and history.
//!
//! Small facade over focused child modules. Production dependencies flow
//! `plan -> {model, files, render}`, `files -> {model, render}`,
//! `render/persist -> model`, and `clipboard -> history -> {alfred, clipy}`
//! on macOS history.

#[cfg(any(target_os = "macos", test))]
mod alfred;
mod clipboard;
#[cfg(any(target_os = "macos", test))]
mod clipy;
mod files;
mod history;
mod model;
mod persist;
mod plan;
mod render;
#[cfg(test)]
mod tests;

pub(crate) use clipboard::{
    read_clipboard, read_clipboard_history_with_current,
    AgreedClipboardHistory, ClipboardHistoryError,
};
pub(crate) use files::{attachment_reference, sha256_hex};
#[cfg(test)]
pub(crate) use model::ClipHistoryEntryOutput;
pub(crate) use model::{
    AttachmentKind, AttachmentOutput, ClipHistoryOutput, ClipMode, ClipOutput,
    ClipPlan, ClipReservations, ClipStatus,
};
pub(crate) use persist::{append_cleanup_message, cleanup_created};
#[cfg(test)]
pub(crate) use plan::{plan, plan_history};
pub(crate) use plan::{
    plan_explicit_paths, plan_history_with_reservations,
    plan_with_reservations, ExplicitPathError,
};
