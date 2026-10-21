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

fn write_test_bytes(path: &Path, contents: &[u8]) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("create test file parent");
    }
    fs::write(path, contents).expect("write test file");
}

fn png_crc(data: &[u8]) -> u32 {
    let mut crc = 0xffff_ffffu32;
    for &byte in data {
        crc ^= u32::from(byte);
        for _ in 0..8 {
            crc = if crc & 1 == 1 {
                (crc >> 1) ^ 0xedb8_8320
            } else {
                crc >> 1
            };
        }
    }
    !crc
}

fn png_chunk(kind: &[u8; 4], data: &[u8]) -> Vec<u8> {
    let mut chunk = Vec::new();
    chunk.extend_from_slice(&(data.len() as u32).to_be_bytes());
    chunk.extend_from_slice(kind);
    chunk.extend_from_slice(data);
    let mut crc_input = Vec::new();
    crc_input.extend_from_slice(kind);
    crc_input.extend_from_slice(data);
    chunk.extend_from_slice(&png_crc(&crc_input).to_be_bytes());
    chunk
}

fn test_png_bytes(
    width: u32,
    height: u32,
    px_per_meter: Option<u32>,
) -> Vec<u8> {
    let mut bytes = b"\x89PNG\r\n\x1a\n".to_vec();
    let mut ihdr = Vec::new();
    ihdr.extend_from_slice(&width.to_be_bytes());
    ihdr.extend_from_slice(&height.to_be_bytes());
    ihdr.extend_from_slice(&[8, 2, 0, 0, 0]);
    bytes.extend_from_slice(&png_chunk(b"IHDR", &ihdr));
    if let Some(density) = px_per_meter {
        let mut phys = Vec::new();
        phys.extend_from_slice(&density.to_be_bytes());
        phys.extend_from_slice(&density.to_be_bytes());
        phys.push(1);
        bytes.extend_from_slice(&png_chunk(b"pHYs", &phys));
    }
    bytes.extend_from_slice(&png_chunk(b"IDAT", &[]));
    bytes.extend_from_slice(&png_chunk(b"IEND", &[]));
    bytes
}

mod audio;
mod continuations;
mod highlight_identity;
mod image_notes;
mod marker;
mod projection;
mod reading_execute;
mod reading_plan;
mod region;
mod seams;
mod sidecar;
mod status;
mod tasks;
mod underlines;
