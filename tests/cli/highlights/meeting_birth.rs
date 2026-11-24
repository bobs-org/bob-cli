//! Meeting birth: LINKS relation and recap, byte-identical inline vs drain.
//!
//! Synthetic fixtures only: invented people and companies, never real meeting
//! content.

use crate::support::*;
use std::{fs, path::Path};

fn write_file(path: &Path, contents: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("create parent");
    }
    fs::write(path, contents).expect("write file");
}

fn seed_parent(vault: &Path) {
    write_file(
        &vault.join("sase.md"),
        "---\ntype: \"[[area]]\"\n---\n\n# Sase\n",
    );
    write_file(
        &vault.join("job.md"),
        "---\ntype: \"[[project]]\"\nstatus: wip\nparent: \"[[sase]]\"\n---\n# Job\n\n## Tasks\n",
    );
    write_file(&vault.join(".gitignore"), "/xlib/\n");
}

fn seed_event(vault: &Path) {
    write_file(
        &vault.join("2026/20261123.md"),
        "# Day\n## Events\n- [*] #task **1500-1530** Cedar Labs and Driftwood planning [root:: job] ^cedar-driftwood\n## Pomodoros\n",
    );
}

fn dossier_bytes() -> Vec<u8> {
    let text = concat!(
        "---\n",
        "bob_meeting_dossier: 1\n",
        "event_title: Cedar Labs and Driftwood planning\n",
        "date: '2026-11-23'\n",
        "span: '15:00–15:30'\n",
        "root: job\n",
        "producer: VoiceQuill\n",
        "filed_date: '2026-11-23'\n",
        "include_transcript: false\n",
        "edited: false\n",
        "notes_shape: wispr_plain_text\n",
        "warnings: []\n",
        "---\n",
        "::: {meeting-tldr}\n",
        "The CloudCove connector syncs in one pass.\n",
        ":::\n\n",
        "## Next steps\n\n",
        "- (Avery) Ship the connector\n\n",
        "## Risks\n\n",
        "- Quotas apply per workspace\n",
    );
    text.as_bytes().to_vec()
}

fn write_meeting_store(
    vault: &Path,
    stem: &str,
    pdf_sha: &str,
    dossier: &[u8],
) {
    use sha2::{Digest, Sha256};
    let digest = hex::encode(Sha256::digest(dossier));
    let hash12 = digest[..12].to_string();
    let dir = vault.join(format!(".bob/ref-sources/meetings/{stem}--{hash12}"));
    fs::create_dir_all(&dir).expect("store dir");
    fs::write(dir.join("source.md"), dossier).expect("store source");
    let catalog = serde_json::json!({
        "version": 1,
        "captured_at": "2026-11-23T15:00:00Z",
        "route": "job",
        "source": {"sha256": digest, "bytes": dossier.len(), "origin": "test"},
        "pdf": {"path": format!("lib/meetings/{stem}.pdf"), "sha256": pdf_sha},
        "renderer": {"bob": "test", "pandoc": "test"},
        "blocks": [],
    });
    fs::write(
        dir.join("code.json"),
        serde_json::to_string_pretty(&catalog).expect("catalog json"),
    )
    .expect("store catalog");
}

fn meeting_marker() -> String {
    "- status: ready\n- parent: job\n- title: Cedar Labs and Driftwood planning — Meeting Notes (Sun 2026-11-23, 15:00–15:30)\n- event: \"[[2026/20261123#^cedar-driftwood]]\"\n- meeting_start: 2026-11-23T15:00\n- meeting_end: 2026-11-23T15:30\n- notes_producer: VoiceQuill\n".to_string()
}

fn sync_command(
    temp: &TempDir,
    vault: &Path,
    pdf: &Path,
) -> std::process::Command {
    let mut command = bob_command();
    command
        .arg("ref")
        .arg("sync")
        .arg(pdf)
        .env("BOB_DIR", vault)
        .env("BOB_REF_INTAKE_LOCK_WAIT_SECONDS", "0")
        .env("BOB_REF_SCAN_LOCK_WAIT_SECONDS", "0")
        .env("BOB_NOW", "2026-11-23 16:00:00")
        .env(
            "BOB_VAULT_SYNC_LOCK_FILE",
            temp.path().join("bob_sync.lock"),
        )
        .env(
            "BOB_VAULT_SYNC_STATE_FILE",
            temp.path().join("vault-sync.json"),
        )
        .env("XDG_STATE_HOME", temp.path().join("state"));
    command
}

fn scan_command(
    temp: &TempDir,
    vault: &Path,
    args: &[&str],
) -> std::process::Command {
    let mut command = bob_command();
    command
        .arg("ref")
        .arg("scan")
        .args(args)
        .env("BOB_DIR", vault)
        .env("BOB_REF_INTAKE_LOCK_WAIT_SECONDS", "0")
        .env("BOB_REF_SCAN_LOCK_WAIT_SECONDS", "0")
        .env("BOB_NOW", "2026-11-23 16:00:00")
        .env(
            "BOB_VAULT_SYNC_LOCK_FILE",
            temp.path().join("bob_sync.lock"),
        )
        .env(
            "BOB_VAULT_SYNC_STATE_FILE",
            temp.path().join("vault-sync.json"),
        )
        .env("XDG_STATE_HOME", temp.path().join("state"));
    command
}

#[test]
fn meetings_birth_files_links_and_full_recap() {
    let temp = TempDir::new("bob-cli-meeting-birth-basic");
    let (vault, _remote, _peer) = init_vault_sync_pair(&temp);
    seed_parent(&vault);
    seed_event(&vault);
    git_in(
        &vault,
        ["add", ".gitignore", "sase.md", "job.md", "2026/20261123.md"],
    );
    git_in(&vault, ["commit", "-q", "-m", "seed"]);
    git_in(&vault, ["push", "-q", "origin", "master"]);

    let stem = "261123_cedar_driftwood_planning";
    let pdf = vault.join(format!("lib/meetings/{stem}.pdf"));
    write_highlights_pdf(&pdf, &meeting_marker());
    let pdf_sha = hex::encode({
        use sha2::{Digest, Sha256};
        Sha256::digest(fs::read(&pdf).expect("read pdf"))
    });
    write_meeting_store(&vault, stem, &pdf_sha, &dossier_bytes());

    let output = sync_command(&temp, &vault, &pdf)
        .output()
        .expect("run scan");
    // The scan may report via `ref scan` or `highlights scan`; either way it
    // must succeed and file the note.
    assert_success(&output);

    let note = vault.join(format!("ref/meetings/{stem}.md"));
    assert!(note.is_file(), "birth must file the note");
    let body = fs::read_to_string(&note).expect("read note");
    assert!(
        body.contains("<!-- bob:meeting-recap:begin v1 sha256:"),
        "note must carry the owned recap region:\n{body}"
    );
    assert!(
        body.contains("<!-- bob:meeting-recap:end -->"),
        "note must close the recap region:\n{body}"
    );
    assert!(
        body.contains("Ship the connector"),
        "full recap holds next steps:\n{body}"
    );
    assert!(
        body.contains("Quotas apply"),
        "full recap holds section detail:\n{body}"
    );
    assert!(
        body.contains("## Highlights"),
        "recap sits before Highlights:\n{body}"
    );
    let recap_pos = body.find("<!-- bob:meeting-recap:begin").expect("recap");
    let highlights_pos = body.find("## Highlights").expect("highlights");
    assert!(recap_pos < highlights_pos);

    let job = fs::read_to_string(vault.join("job.md")).expect("read job");
    assert!(
        job.contains("notes from [[20261123#^cedar-driftwood]]"),
        "reading task must carry the LINKS child:\n{job}"
    );
}

#[test]
fn meetings_birth_is_identical_inline_and_on_queued_drain() {
    // Same marker, store, event, and clock in two vaults: one PDF already in
    // lib/ (inline sync) and one queued in xlib/ (drained by `scan -i`).
    // The reading-task line, its children, and the note bytes must match.
    let stem = "261123_cedar_driftwood_planning";

    let temp_a = TempDir::new("bob-cli-meeting-birth-inline");
    let (vault_a, _remote_a, _peer_a) = init_vault_sync_pair(&temp_a);
    seed_parent(&vault_a);
    seed_event(&vault_a);
    git_in(
        &vault_a,
        ["add", ".gitignore", "sase.md", "job.md", "2026/20261123.md"],
    );
    git_in(&vault_a, ["commit", "-q", "-m", "seed"]);
    git_in(&vault_a, ["push", "-q", "origin", "master"]);
    let pdf_a = vault_a.join(format!("lib/meetings/{stem}.pdf"));
    write_highlights_pdf(&pdf_a, &meeting_marker());
    let pdf_sha_a = hex::encode({
        use sha2::{Digest, Sha256};
        Sha256::digest(fs::read(&pdf_a).expect("read pdf"))
    });
    write_meeting_store(&vault_a, stem, &pdf_sha_a, &dossier_bytes());
    let output_a = sync_command(&temp_a, &vault_a, &pdf_a)
        .output()
        .expect("run inline sync");
    assert_success(&output_a);

    let temp_b = TempDir::new("bob-cli-meeting-birth-drain");
    let (vault_b, _remote_b, _peer_b) = init_vault_sync_pair(&temp_b);
    seed_parent(&vault_b);
    seed_event(&vault_b);
    git_in(
        &vault_b,
        ["add", ".gitignore", "sase.md", "job.md", "2026/20261123.md"],
    );
    git_in(&vault_b, ["commit", "-q", "-m", "seed"]);
    git_in(&vault_b, ["push", "-q", "origin", "master"]);
    let queued = vault_b.join(format!("xlib/meetings/{stem}.pdf"));
    // Same bytes as the inline PDF so the birth store matches both.
    let pdf_bytes = fs::read(&pdf_a).expect("read inline pdf");
    if let Some(parent) = queued.parent() {
        fs::create_dir_all(parent).expect("create xlib parent");
    }
    fs::write(&queued, &pdf_bytes).expect("queue pdf");
    let pdf_sha_b = hex::encode({
        use sha2::{Digest, Sha256};
        Sha256::digest(&pdf_bytes)
    });
    assert_eq!(pdf_sha_a, pdf_sha_b);
    write_meeting_store(&vault_b, stem, &pdf_sha_b, &dossier_bytes());
    let output_b = scan_command(&temp_b, &vault_b, &["-i"])
        .output()
        .expect("run queued drain");
    assert_success(&output_b);

    let note_a =
        fs::read_to_string(vault_a.join(format!("ref/meetings/{stem}.md")))
            .expect("read inline note");
    let note_b =
        fs::read_to_string(vault_b.join(format!("ref/meetings/{stem}.md")))
            .expect("read drained note");
    // The commit-time `created` line is clock-pinned, so whole notes match.
    assert_eq!(
        note_a, note_b,
        "inline and drain notes must be byte-identical"
    );

    let job_a =
        fs::read_to_string(vault_a.join("job.md")).expect("read inline job");
    let job_b =
        fs::read_to_string(vault_b.join("job.md")).expect("read drained job");
    let task_a = job_a
        .lines()
        .find(|line| {
            line.contains(&format!("^ref-{stem}"))
                || line.contains("ref/meetings")
        })
        .unwrap_or_else(|| panic!("inline task missing:\n{job_a}"))
        .to_string();
    let task_b = job_b
        .lines()
        .find(|line| {
            line.contains(&format!("^ref-{stem}"))
                || line.contains("ref/meetings")
        })
        .unwrap_or_else(|| panic!("drained task missing:\n{job_b}"))
        .to_string();
    assert_eq!(task_a, task_b, "task lines must match");
    let links_a = job_a.matches("🪢 **LINKS:**").count();
    let links_b = job_b.matches("🪢 **LINKS:**").count();
    assert_eq!(links_a, 1, "inline births one LINKS:\n{job_a}");
    assert_eq!(links_b, 1, "drain births one LINKS:\n{job_b}");
}

#[test]
fn meetings_birth_second_sync_and_adopt_preserve_bytes() {
    let temp = TempDir::new("bob-cli-meeting-birth-preserve");
    let (vault, _remote, _peer) = init_vault_sync_pair(&temp);
    seed_parent(&vault);
    seed_event(&vault);
    git_in(
        &vault,
        ["add", ".gitignore", "sase.md", "job.md", "2026/20261123.md"],
    );
    git_in(&vault, ["commit", "-q", "-m", "seed"]);
    git_in(&vault, ["push", "-q", "origin", "master"]);

    let stem = "261123_cedar_driftwood_planning";
    let pdf = vault.join(format!("lib/meetings/{stem}.pdf"));
    write_highlights_pdf(&pdf, &meeting_marker());
    let pdf_sha = hex::encode({
        use sha2::{Digest, Sha256};
        Sha256::digest(fs::read(&pdf).expect("read pdf"))
    });
    write_meeting_store(&vault, stem, &pdf_sha, &dossier_bytes());

    let output = sync_command(&temp, &vault, &pdf)
        .output()
        .expect("run birth sync");
    assert_success(&output);
    let note_before =
        fs::read_to_string(vault.join(format!("ref/meetings/{stem}.md")))
            .expect("read note");
    let job_before =
        fs::read_to_string(vault.join("job.md")).expect("read job");

    // A second sync preserves the recap and the LINKS line byte for byte.
    let again = sync_command(&temp, &vault, &pdf)
        .output()
        .expect("rerun sync");
    assert_success(&again);
    let note_after =
        fs::read_to_string(vault.join(format!("ref/meetings/{stem}.md")))
            .expect("read note again");
    let job_after =
        fs::read_to_string(vault.join("job.md")).expect("read job again");
    assert_eq!(note_before, note_after, "second sync preserves the note");
    assert_eq!(job_before, job_after, "second sync preserves the task");

    // Adopting the orphan after deleting the note never adds a second LINKS.
    fs::remove_file(vault.join(format!("ref/meetings/{stem}.md")))
        .expect("delete note");
    let adopt = sync_command(&temp, &vault, &pdf)
        .output()
        .expect("adopt orphan");
    assert_success(&adopt);
    let job_adopted =
        fs::read_to_string(vault.join("job.md")).expect("read adopted job");
    assert_eq!(
        job_adopted.matches("🪢 **LINKS:**").count(),
        1,
        "adopt must not duplicate LINKS:\n{job_adopted}"
    );
    let note_adopted =
        fs::read_to_string(vault.join(format!("ref/meetings/{stem}.md")))
            .expect("read adopted note");
    assert!(
        note_adopted.contains("<!-- bob:meeting-recap:begin v1 sha256:"),
        "adopted note keeps the recap:\n{note_adopted}"
    );
}

#[test]
fn meetings_birth_missing_store_files_empty_recap_with_warning() {
    let temp = TempDir::new("bob-cli-meeting-birth-missing-store");
    let (vault, _remote, _peer) = init_vault_sync_pair(&temp);
    seed_parent(&vault);
    seed_event(&vault);
    git_in(
        &vault,
        ["add", ".gitignore", "sase.md", "job.md", "2026/20261123.md"],
    );
    git_in(&vault, ["commit", "-q", "-m", "seed"]);
    git_in(&vault, ["push", "-q", "origin", "master"]);

    let stem = "261123_cedar_driftwood_planning";
    let pdf = vault.join(format!("lib/meetings/{stem}.pdf"));
    write_highlights_pdf(&pdf, &meeting_marker());

    let output = sync_command(&temp, &vault, &pdf)
        .output()
        .expect("run scan");
    assert_success(&output);

    let note = vault.join(format!("ref/meetings/{stem}.md"));
    let body = fs::read_to_string(&note).expect("read note");
    assert!(
        body.contains("<!-- bob:meeting-recap:begin v1 sha256:"),
        "missing store still births an owned empty recap:\n{body}"
    );
    let job = fs::read_to_string(vault.join("job.md")).expect("read job");
    assert!(
        job.contains("store is missing"),
        "missing store warns on the reading task:\n{job}"
    );
}

#[test]
fn meetings_birth_missing_event_files_without_dangling_links() {
    let temp = TempDir::new("bob-cli-meeting-birth-missing-event");
    let (vault, _remote, _peer) = init_vault_sync_pair(&temp);
    seed_parent(&vault);
    seed_event(&vault);
    git_in(
        &vault,
        ["add", ".gitignore", "sase.md", "job.md", "2026/20261123.md"],
    );
    git_in(&vault, ["commit", "-q", "-m", "seed"]);
    git_in(&vault, ["push", "-q", "origin", "master"]);

    let stem = "261123_cedar_driftwood_planning";
    let pdf = vault.join(format!("lib/meetings/{stem}.pdf"));
    let marker = "- status: ready\n- parent: job\n- title: Cedar Labs and Driftwood planning — Meeting Notes\n- event: \"[[2026/20261123#^nope]]\"\n- meeting_start: 2026-11-23T15:00\n- meeting_end: 2026-11-23T15:30\n- notes_producer: VoiceQuill\n";
    write_highlights_pdf(&pdf, marker);
    let pdf_sha = hex::encode({
        use sha2::{Digest, Sha256};
        Sha256::digest(fs::read(&pdf).expect("read pdf"))
    });
    write_meeting_store(&vault, stem, &pdf_sha, &dossier_bytes());

    let output = sync_command(&temp, &vault, &pdf)
        .output()
        .expect("run scan");
    assert_success(&output);

    let job = fs::read_to_string(vault.join("job.md")).expect("read job");
    assert!(
        !job.contains("🪢 **LINKS:**"),
        "missing event writes no dangling LINKS:\n{job}"
    );
    assert!(
        job.contains("was not found") || job.contains("no longer resolves"),
        "missing event warns:\n{job}"
    );
}
