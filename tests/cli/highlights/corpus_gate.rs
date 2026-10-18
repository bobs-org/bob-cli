//! Synthetic coverage for `scripts/dev/note_snapshot_corpus_gate`.
//!
//! The complete gate is exercised by the sibling bash harness with a fake
//! `BOB_BIN` and fake PDF tools. This test only checks that harness's
//! exit status so `just check` covers the gate.

use std::process::Command;

#[test]
fn classifies_file_warnings_against_the_complete_gate() {
    let script = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("scripts/dev/note_snapshot_corpus_gate_test");
    let output = Command::new("bash")
        .arg(&script)
        .output()
        .unwrap_or_else(|error| panic!("run {}: {error}", script.display()));
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "corpus gate regression failed (status {:?})\nstdout:\n{stdout}\nstderr:\n{stderr}",
        output.status.code()
    );
}
