//! Hidden `bob __catalog --protocol 1` command catalog.
//!
//! The handshake mirrors `__complete`: an early intercept in `run_bob`
//! answers the request from a static table plus the clap tree, without
//! reading the vault or writing anything. See `docs/command-catalog.md`.

mod assemble;
mod json;
mod protocol;
mod table;

#[cfg(test)]
mod tests;

use std::ffi::OsString;

/// Answer a hidden `__catalog` request.
///
/// Protocol 1 prints one JSON document to stdout and exits 0. A
/// malformed request prints usage to stderr and exits 2. An unsupported
/// protocol prints a JSON error to stdout and exits 2.
pub(crate) fn run_catalog(argv: &[OsString]) -> i32 {
    match protocol::parse_request(argv) {
        Err(message) => {
            eprintln!("{message}");
            2
        }
        Ok(request) => {
            if let Some(error) = protocol::unsupported_json(request.protocol) {
                println!("{error}");
                2
            } else {
                let payload =
                    json::render(&assemble::document()).expect("catalog json");
                println!("{payload}");
                0
            }
        }
    }
}
