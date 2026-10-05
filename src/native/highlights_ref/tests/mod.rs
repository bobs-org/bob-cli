//! Shared fixtures for highlights_ref unit tests.
use super::*;

use std::{
    fs,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

fn string_value(value: &str) -> MarkerValue {
    MarkerValue::String(value.to_string())
}

fn test_projection(entries: Vec<(&str, MarkerValue)>) -> Projection {
    entries
        .into_iter()
        .map(|(key, value)| (key.to_string(), value))
        .collect()
}

fn test_config() -> Config {
    Config {
        bob_dir: PathBuf::from("/tmp/bob"),
        lib_dir: PathBuf::from("/tmp/bob/lib"),
        ref_dir: PathBuf::from("/tmp/bob/ref"),
        xlib_dir: PathBuf::from("/tmp/bob/xlib"),
    }
}

fn test_config_for_bob_dir(bob_dir: PathBuf) -> Config {
    Config {
        lib_dir: bob_dir.join("lib"),
        ref_dir: bob_dir.join("ref"),
        xlib_dir: bob_dir.join("xlib"),
        bob_dir,
    }
}

fn temp_bob_dir(name: &str) -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock before epoch")
        .as_nanos();
    let path = std::env::temp_dir().join(format!(
        "bob-cli-highlights-ref-{name}-{}-{nonce}",
        std::process::id()
    ));
    fs::create_dir_all(&path).expect("create temp bob dir");
    path
}

fn write_test_file(path: &Path, contents: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("create test file parent");
    }
    fs::write(path, contents).expect("write test file");
}

mod audio;
mod marker;
mod projection;
mod sidecar;
mod status;
mod tasks;
