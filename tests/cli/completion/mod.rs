//! Golden integration tests for the hidden `__complete` endpoint.
//!
//! Each case runs the built binary, so the goldens cover request
//! parsing, the engine, the kinds table, and the presenter together.

mod protocol;
mod vault;
mod zsh_adapter;
