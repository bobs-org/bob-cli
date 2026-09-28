//! `bob gkeep login` stub: later phases implement the token exchange.

use super::{ui, GkeepError, LoginArgs};

pub(crate) fn run(args: &LoginArgs) -> i32 {
    // The email override is pinned here; the auth phase consumes it.
    let _ = args.email.as_deref();
    ui::report_error(
        "login",
        &GkeepError::runtime(
            "unimplemented",
            "not implemented yet".to_string(),
        ),
        "human",
    )
}
