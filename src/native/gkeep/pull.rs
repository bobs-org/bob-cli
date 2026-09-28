//! `bob gkeep pull` stub: later phases implement the transaction.

use super::{ui, GkeepError, PullArgs};

pub(crate) fn run(args: &PullArgs) -> i32 {
    ui::report_error(
        "pull",
        &GkeepError::runtime(
            "unimplemented",
            "not implemented yet".to_string(),
        ),
        args.error_format(),
    )
}
