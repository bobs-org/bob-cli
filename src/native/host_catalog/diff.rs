//! Shared unified-diff rendering for guarded catalog sync dry runs.
//!
//! Moved from `notebooks::sync` so every catalog writer prints byte-identical
//! diffs. Output is unchanged: a three-line context unified diff with
//! `a/{path}` / `b/{path}` headers, plus ANSI coloring for human runs.

use similar::TextDiff;

use crate::native::style::Styler;

/// Plain unified diff of `before` → `after` for one vault-relative path.
pub(crate) fn unified_diff(path: &str, before: &str, after: &str) -> String {
    TextDiff::from_lines(before, after)
        .unified_diff()
        .context_radius(3)
        .header(&format!("a/{path}"), &format!("b/{path}"))
        .to_string()
}

/// ANSI-colored rendering of a unified diff for human output.
pub(crate) fn color_diff(diff: &str, styler: &Styler) -> String {
    let mut out = String::new();
    for line in diff.lines() {
        let painted = if line.starts_with("+++") || line.starts_with("---") {
            styler.bold(line)
        } else if line.starts_with('+') {
            styler.green(line)
        } else if line.starts_with('-') {
            styler.red(line)
        } else if line.starts_with("@@") {
            styler.cyan(line)
        } else {
            line.to_string()
        };
        out.push_str(&painted);
        out.push('\n');
    }
    out
}
