//! `bob gkeep doctor` stub: later phases implement the checklist.

use super::{ui, DoctorArgs, GkeepError};

pub(crate) fn run(args: &DoctorArgs) -> i32 {
    ui::report_error(
        "doctor",
        &GkeepError::runtime(
            "unimplemented",
            "not implemented yet".to_string(),
        ),
        args.error_format(),
    )
}
