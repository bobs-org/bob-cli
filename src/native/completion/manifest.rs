//! The install manifest: which adapters bob owns and where they live.
//!
//! The manifest lives at `$XDG_STATE_HOME/bob-cli/completion/manifest.json`
//! (through [`bob_cli_state_dir`](crate::native::env::bob_cli_state_dir))
//! with `schema_version: 1`. A corrupt manifest warns on stderr and reads
//! as empty, so a damaged record can only make installs more conservative.

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use super::cli::Shell;
use crate::native::env;

const SCHEMA_VERSION: u32 = 1;

fn manifest_path() -> PathBuf {
    env::bob_cli_state_dir()
        .join("completion")
        .join("manifest.json")
}

/// The verification result recorded alongside an install.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct VerificationRecord {
    /// Human registration text, e.g. `registered as _bob`.
    pub(crate) registration: String,
    /// sha256 of the adapter file the probe applied to.
    pub(crate) digest: String,
    /// Adapter path the probe applied to.
    pub(crate) path: String,
    /// `install` when recorded at install time, `now` for a live probe.
    pub(crate) checked: String,
}

/// One shell's manifest entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct ShellEntry {
    pub(crate) path: String,
    pub(crate) protocol: u32,
    pub(crate) sha256: String,
    pub(crate) bob_version: String,
    pub(crate) installed_at: String,
    pub(crate) target_reason: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) verification: Option<VerificationRecord>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ManifestFile {
    schema_version: u32,
    #[serde(default)]
    shells: BTreeMap<String, ShellEntry>,
}

/// The loaded manifest: this binary's record of the adapters it owns.
#[derive(Debug, Clone, Default)]
pub(crate) struct Manifest {
    shells: BTreeMap<String, ShellEntry>,
}

impl Manifest {
    pub(crate) fn load() -> Self {
        let path = manifest_path();
        let bytes = match std::fs::read(&path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Self::default();
            }
            Err(error) => {
                eprintln!(
                    "bob completion: warning: cannot read {}: {error}; treating installs as unrecorded",
                    path.display()
                );
                return Self::default();
            }
        };
        match serde_json::from_slice::<ManifestFile>(&bytes) {
            Ok(file) => Self {
                shells: file.shells,
            },
            Err(error) => {
                eprintln!(
                    "bob completion: warning: corrupt manifest at {}: {error}; treating installs as unrecorded",
                    path.display()
                );
                Self::default()
            }
        }
    }

    pub(crate) fn entry(&self, shell: Shell) -> Option<&ShellEntry> {
        self.shells.get(shell.name())
    }

    pub(crate) fn set_verification(
        &mut self,
        shell: Shell,
        verification: VerificationRecord,
    ) {
        if let Some(entry) = self.shells.get_mut(shell.name()) {
            entry.verification = Some(verification);
        }
    }

    /// Every shell with a recorded entry, in [`Shell::all`] order.
    pub(crate) fn owned_shells(&self) -> Vec<Shell> {
        Shell::all()
            .iter()
            .copied()
            .filter(|shell| self.shells.contains_key(shell.name()))
            .collect()
    }

    pub(crate) fn set(&mut self, shell: Shell, entry: ShellEntry) {
        self.shells.insert(shell.name().to_string(), entry);
    }

    pub(crate) fn remove(&mut self, shell: Shell) {
        self.shells.remove(shell.name());
    }

    pub(crate) fn save(&self) -> Result<(), String> {
        let path = manifest_path();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|error| {
                format!("cannot create {}: {error}", parent.display())
            })?;
        }
        let file = ManifestFile {
            schema_version: SCHEMA_VERSION,
            shells: self.shells.clone(),
        };
        let bytes = serde_json::to_vec_pretty(&file)
            .map_err(|error| format!("cannot encode manifest: {error}"))?;
        atomic_write(&path, &bytes)
    }
}

/// Write `path` atomically: a temp file in the same directory, then rename.
pub(crate) fn atomic_write(
    path: &std::path::Path,
    bytes: &[u8],
) -> Result<(), String> {
    let parent = path.parent().ok_or_else(|| {
        format!(
            "cannot install above the filesystem root: {}",
            path.display()
        )
    })?;
    std::fs::create_dir_all(parent).map_err(|error| {
        format!("cannot create {}: {error}", parent.display())
    })?;
    let temp = parent.join(format!(
        ".bob-{}-{}.tmp",
        std::process::id(),
        nanos_suffix()
    ));
    std::fs::write(&temp, bytes)
        .map_err(|error| format!("cannot write {}: {error}", temp.display()))?;
    std::fs::rename(&temp, path).map_err(|error| {
        let _ = std::fs::remove_file(&temp);
        format!("cannot move {} into place: {error}", path.display())
    })
}

fn nanos_suffix() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.subsec_nanos() as u128)
        .unwrap_or(0)
}

/// sha256 hex of the adapter bytes bob would install for `shell`.
pub(crate) fn adapter_sha256(shell: Shell) -> String {
    use sha2::Digest;
    let bytes: &[u8] = match shell {
        Shell::Bash => super::adapters::bash_adapter().as_bytes(),
        Shell::Zsh => super::adapters::zsh_adapter().as_bytes(),
    };
    hex::encode(sha2::Sha256::digest(bytes))
}

/// sha256 hex of arbitrary bytes.
pub(crate) fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::Digest;
    hex::encode(sha2::Sha256::digest(bytes))
}
