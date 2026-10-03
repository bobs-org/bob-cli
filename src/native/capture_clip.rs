//! Clipboard capture planning, persistence, and history.
//!
//! Small facade over focused child modules. Production dependencies flow
//! `plan -> {model, files, render}`, `files -> {model, render}`,
//! `render/persist -> model`, and `clipboard -> clipy` on macOS history.

mod clipboard;
#[cfg(any(target_os = "macos", test))]
mod clipy;
mod files;
mod model;
mod persist;
mod plan;
mod render;
#[cfg(test)]
mod tests;

pub(crate) use clipboard::{read_clipboard, read_clipboard_history};
pub(crate) use model::{
    AttachmentKind, AttachmentOutput, ClipMode, ClipOutput, ClipPlan,
    ClipReservations,
};
pub(crate) use persist::{append_cleanup_message, cleanup_created};
#[cfg(test)]
pub(crate) use plan::{plan, plan_history};
pub(crate) use plan::{plan_history_with_reservations, plan_with_reservations};
pub(crate) use render::{is_valid_header, rendered_header};
