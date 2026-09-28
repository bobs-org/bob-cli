//! `bob gkeep list` stub: later phases implement the reconciliation view.

use super::{ui, GkeepError, ListArgs};

pub(crate) fn run(args: &ListArgs) -> i32 {
    ui::report_error(
        "list",
        &GkeepError::runtime(
            "unimplemented",
            "not implemented yet".to_string(),
        ),
        args.error_format(),
    )
}
