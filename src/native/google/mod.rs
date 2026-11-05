//! Shared OAuth and gog plumbing for Bob's Google integrations.

pub(crate) mod account;
pub(crate) mod client;
pub(crate) mod client_setup;
pub(crate) mod doctor;
mod error;
pub(crate) mod gog;
pub(crate) mod login_ui;
mod profile;
pub(crate) mod signin;

pub(crate) use error::GoogleError;
pub(crate) use gog::{parse_json, Gog};
#[allow(unused_imports)]
pub(crate) use profile::{GoogleService, GCAL, GMAIL};

pub(crate) const REVOKE_URL: &str = "https://myaccount.google.com/permissions";
