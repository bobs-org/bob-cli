//! Durable ref-job spool, background worker, and `bob ref jobs`.
//!
//! Capture submits queue one JSON job per link under
//! `bob_cli_state_dir()/ref/jobs/`; a detached single-flight worker
//! clips them through the shared ingest. A failed clip falls back to
//! exactly the inbox task capture would have written, plus a ⚠️
//! bullet. Jobs whose fallback write fails park in `stuck/` and are
//! never lost.

pub(crate) mod cli;
mod doctor;
mod fallback;
mod kick;
mod output;
pub(crate) mod spool;
mod worker;

pub(crate) use cli::{command, run};
pub(crate) use doctor::append_ref_jobs_doctor_row;
pub(crate) use fallback::write_fallback;
pub(crate) use kick::{kick, kick_with, ENV_KICK_DISABLE};
pub(crate) use spool::{
    enqueue, jobs_dir, list_jobs, pending_keys, remove_created, JobFallback,
    JobFile, JobState, JobsView, ListedJob, NewJob, StoredError,
};
pub(crate) use worker::run_jobs;
