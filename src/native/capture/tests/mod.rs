//! Shared fixtures for capture unit tests.
use super::*;

use std::num::NonZeroUsize;

// Grammar helpers now live in `capture_language`; these tests keep
// exercising them through `bob capture` so the move stays behavior
// preserving.
use crate::native::capture_language::{
    extract_terminal_markers, normalize_task_text, parse_priority_token,
    parse_schedule_token,
};

const TASK: &str = "- [ ] #task new thing [created::2026-06-15]";
const BULLET: &str = "- new idea [created::2026-06-15]";

fn parse_capture_text(
    raw_text: &str,
    forced_route: Option<&str>,
) -> Result<ParsedCaptureText, CaptureError> {
    super::parse_capture_text(raw_text, forced_route, None)
}

fn insert_bullet_line(
    contents: &str,
    bullet_line: &str,
    section_prefix: Option<&str>,
) -> (String, Placement) {
    super::insert_bullet_line(contents, bullet_line, section_prefix, false)
}

mod assembly;
mod grammar;
mod placement;
mod started;
