use super::receipt::Outcome;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    fs,
    io::{self, Write},
    path::{Path, PathBuf},
};

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub(super) struct StepState {
    pub last_started_at: Option<DateTime<Utc>>,
    pub last_finished_at: Option<DateTime<Utc>>,
    pub last_outcome: Option<Outcome>,
    pub last_reason: Option<String>,
    pub last_summary: Option<String>,
    pub last_exit_code: Option<i32>,
    pub last_success_at: Option<DateTime<Utc>>,
    pub follow_up_pending: bool,
    pub yesterday_synced_on: Option<String>,
    pub fingerprint: Option<String>,
    pub last_full_scan_at: Option<DateTime<Utc>>,
}
#[derive(Serialize, Deserialize)]
pub(super) struct State {
    pub schema_version: u32,
    pub host: String,
    pub vault: PathBuf,
    pub steps: BTreeMap<String, StepState>,
}
impl State {
    pub fn load(dir: &Path, host: &str, vault: &Path) -> Self {
        let fresh = || Self {
            schema_version: 1,
            host: host.into(),
            vault: vault.into(),
            steps: BTreeMap::new(),
        };
        let parsed = match fs::read(dir.join("state.json")) {
            Ok(bytes) => serde_json::from_slice::<Self>(&bytes)
                .map_err(|e| e.to_string()),
            Err(e) if e.kind() == io::ErrorKind::NotFound => return fresh(),
            Err(e) => Err(e.to_string()),
        };
        match parsed {
            Ok(s)
                if s.schema_version == 1
                    && s.host == host
                    && s.vault == vault =>
            {
                s
            }
            other => {
                let why = other
                    .err()
                    .unwrap_or_else(|| "schema, host, or vault changed".into());
                eprintln!("bob tick: warning: ignoring state ({why}); treating steps as never run");
                fresh()
            }
        }
    }
    pub fn save(&self, dir: &Path) -> io::Result<()> {
        atomic_json(dir, "state.json", self)
    }
}
pub(super) fn atomic_json(
    dir: &Path,
    name: &str,
    value: &impl Serialize,
) -> io::Result<()> {
    fs::create_dir_all(dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(dir, fs::Permissions::from_mode(0o700))?;
    }
    let mut file = tempfile::NamedTempFile::new_in(dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        file.as_file()
            .set_permissions(fs::Permissions::from_mode(0o600))?;
    }
    serde_json::to_writer(&mut file, value)?;
    writeln!(file)?;
    file.as_file().sync_all()?;
    file.persist(dir.join(name)).map_err(|e| e.error)?;
    fs::File::open(dir)?.sync_all()
}
