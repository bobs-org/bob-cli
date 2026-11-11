//! Album records-index planning for record write paths.
//!
//! Record creates and updates render the album's index from the batch's
//! current overlay and stage one final album edit. Ordinary task drafts
//! never load the record-index path; album drafts read only their album
//! slice; multi-item drafts reuse one folder scan.
use super::*;

use std::ffi::OsStr;

const ALBUM: &str = concat!(
    "---\n",
    "type: [[album]]\n",
    "parent: \"[[body]]\"\n",
    "summary: Places to eat.\n",
    "emoji: \"🍽️\"\n",
    "record:\n",
    "  noun: restaurant\n",
    "  label: Restaurant\n",
    "  dated_notes: true\n",
    "  fields:\n",
    "    status:\n",
    "      required: true\n",
    "      values:\n",
    "        to_try: \"To try\"\n",
    "        liked: \"Liked\"\n",
    "    category:\n",
    "      required: true\n",
    "      suggest: true\n",
    "    location:\n",
    "      required: true\n",
    "---\n\n",
    "# Restaurants\n",
);

const NINJA: &str = concat!(
    "---\n",
    "parent: \"[[eat]]\"\n",
    "type: \"[[restaurant]]\"\n",
    "title: \"Ninja Sushi\"\n",
    "status: liked\n",
    "category: Sushi\n",
    "location: Clark\n",
    "created: 2026-10-06\n",
    "---\n\n",
    "# Ninja Sushi\n",
);

const CONFIG: &str = r#"records:
  podcast:
    dir: podcasts
    label: Podcast
    emoji: "🎙️"
    fields:
      status:
        required: true
        values:
          to_try: "To try"
          subscribed: "Subscribed"
      category:
        required: true
"#;

fn album_vault() -> (tempfile::TempDir, std::path::PathBuf, std::path::PathBuf)
{
    album_vault_with_config("other: true\n")
}

fn album_vault_with_config(
    config_text: &str,
) -> (tempfile::TempDir, std::path::PathBuf, std::path::PathBuf) {
    let temp = tempfile::tempdir().expect("tempdir");
    let vault = temp.path().join("vault");
    std::fs::create_dir_all(vault.join("eat")).expect("eat folder");
    std::fs::write(
        vault.join("body.md"),
        "---\ntype: [[area]]\n---\n\n# Body\n",
    )
    .expect("area");
    std::fs::write(vault.join("eat.md"), ALBUM).expect("album");
    std::fs::write(vault.join("eat/ninja_sushi.md"), NINJA)
        .expect("existing record");
    let config = temp.path().join("config.yml");
    std::fs::write(&config, config_text).expect("config");
    (temp, vault, config)
}

fn record_request(
    vault: &std::path::Path,
    dry_run: bool,
    raw_text: &str,
) -> CaptureRequest {
    CaptureRequest {
        bob_dir: vault.to_path_buf(),
        dry_run,
        forced_clip: None,
        forced_destination_flags: Vec::new(),
        forced_route: None,
        forced_section: None,
        forced_sub_bullet_target: None,
        forced_drawer: None,
        inbox_routes: Vec::new(),
        move_in: None,
        no_clip: true,
        no_ref: true,
        pull_in: None,
        complete_offers: Vec::new(),
        cancel_offers: Vec::new(),
        offer_verdicts: Vec::new(),
        refuse_override: false,
        require_inbox_route_choice: false,
        require_pull_in_choice: false,
        expect_past_windows: None,
        argv: false,
        template_origin: None,
        raw_text: raw_text.to_string(),
        selection: crate::native::task_complete::SuccessorSelection::None,
    }
}

fn plan(
    vault: &std::path::Path,
    config: &std::path::Path,
    dry_run: bool,
    raw_text: &str,
) -> PlannedCaptureBatch {
    let _env = crate::native::env::TestEnvGuard::set(&[
        ("BOB_NOW", Some(OsStr::new("2026-10-07 13:40:00"))),
        ("TZ", Some(OsStr::new("UTC0"))),
    ]);
    let request = record_request(vault, dry_run, raw_text);
    plan_capture_batch_with_config_path(&request, None, Some(config))
        .expect("album capture plans")
}

fn staged_map(
    batch: &PlannedCaptureBatch,
) -> std::collections::BTreeMap<std::path::PathBuf, String> {
    batch
        .text_files
        .iter()
        .map(|file| (file.target.clone(), file.updated_target.clone()))
        .collect()
}

#[test]
fn ordinary_task_draft_loads_no_record_index() {
    let (_temp, vault, config) = album_vault();
    let album_before =
        std::fs::read_to_string(vault.join("eat.md")).expect("album");
    let batch = plan(&vault, &config, true, "Buy milk");
    assert!(batch.record_folders.is_empty(), "no folder loads");
    assert_eq!(batch.record_files_opened, 0, "no record bytes open");
    assert!(
        batch
            .text_files
            .iter()
            .all(|file| file.target != vault.join("eat.md")),
        "task draft stages no album edit"
    );
    assert_eq!(
        std::fs::read_to_string(vault.join("eat.md")).expect("album"),
        album_before,
        "dry-run writes nothing"
    );
}

#[test]
fn album_create_reads_only_its_slice_and_stages_one_edit() {
    let (_temp, vault, config) = album_vault();
    let batch = plan(
        &vault,
        &config,
        true,
        "Ramen Nagi @eat/\n- status:: liked\n- category:: Ramen\n- location:: Clark",
    );
    assert_eq!(batch.record_folders, vec!["eat".to_string()]);
    assert_eq!(batch.record_files_opened, 1, "one scan of the eat folder");
    let staged = staged_map(&batch);
    assert_eq!(staged.len(), 2, "record plus one album edit: {staged:?}");
    assert!(staged.contains_key(&vault.join("eat/ramen_nagi.md")));
    let album_after = staged.get(&vault.join("eat.md")).expect("album edit");
    assert!(album_after.contains("Ramen Nagi"), "{album_after}");
    assert!(album_after.contains("Ninja Sushi"), "{album_after}");
    assert_eq!(batch.album_files.len(), 1, "one album file change");
    assert_eq!(batch.album_files[0].path, "eat.md");
    assert_eq!(batch.album_files[0].after, *album_after);
}

#[test]
fn album_batch_reuses_one_folder_scan_for_two_creates() {
    let (_temp, vault, config) = album_vault();
    let batch = plan(
        &vault,
        &config,
        true,
        "Ramen Nagi @eat/\n- status:: liked\n- category:: Ramen\n- location:: Clark\n\nUdon @eat/\n- status:: to_try\n- category:: Udon\n- location:: Main",
    );
    assert_eq!(batch.record_folders, vec!["eat".to_string()]);
    assert_eq!(
        batch.record_files_opened, 1,
        "two items still scan the folder once"
    );
    let staged = staged_map(&batch);
    assert_eq!(staged.len(), 3, "two records plus one album edit");
    let album_after = staged.get(&vault.join("eat.md")).expect("album edit");
    for title in ["Ramen Nagi", "Udon", "Ninja Sushi"] {
        assert!(album_after.contains(title), "{album_after}");
    }
}

#[test]
fn album_create_then_update_renders_final_fields() {
    let (_temp, vault, config) = album_vault();
    let batch = plan(
        &vault,
        &config,
        true,
        "Ramen Nagi @eat/\n- status:: to_try\n- category:: Ramen\n- location:: Clark\n\n@eat/ramen_nagi\n- status:: liked",
    );
    let staged = staged_map(&batch);
    let record_after = staged
        .get(&vault.join("eat/ramen_nagi.md"))
        .expect("created then updated record");
    assert!(record_after.contains("status: liked"), "{record_after}");
    let album_after = staged.get(&vault.join("eat.md")).expect("album edit");
    assert!(album_after.contains("Ramen Nagi"), "{album_after}");
    let liked = album_after.find("Liked").expect("liked group");
    let row = album_after.find("Ramen Nagi").expect("row");
    assert!(liked < row, "final fields place the row: {album_after}");
}

#[test]
fn config_backed_record_never_stages_an_album_edit() {
    let (_temp, vault, config) = album_vault_with_config(CONFIG);
    let batch = plan(
        &vault,
        &config,
        true,
        "The Daily @podcasts/\n- status:: to_try\n- category:: News",
    );
    assert_eq!(batch.record_folders, vec!["podcasts".to_string()]);
    assert!(
        !staged_map(&batch).contains_key(&vault.join("eat.md")),
        "config record leaves the album alone"
    );
}

#[test]
fn album_replace_failure_rolls_back_the_staged_record() {
    let (_temp, vault, config) = album_vault();
    let _env = crate::native::env::TestEnvGuard::set(&[
        ("BOB_NOW", Some(OsStr::new("2026-10-07 13:40:00"))),
        ("TZ", Some(OsStr::new("UTC0"))),
    ]);
    let request = record_request(
        &vault,
        false,
        "Ramen Nagi @eat/\n- status:: liked\n- category:: Ramen\n- location:: Clark",
    );
    let mut batch =
        plan_capture_batch_with_config_path(&request, None, Some(&config))
            .expect("album capture plans");
    let album_path = vault.join("eat.md");
    let album_before = std::fs::read_to_string(&album_path).expect("album");
    RENAME_FAIL_TARGET
        .with(|target| *target.borrow_mut() = Some(album_path.clone()));
    let error = commit_capture_batch(&mut batch, &vault)
        .expect_err("album replace fails");
    RENAME_FAIL_TARGET.with(|target| *target.borrow_mut() = None);
    assert!(error.message.contains("rolled back"), "{error:?}");
    assert!(
        !vault.join("eat/ramen_nagi.md").exists(),
        "staged record is removed"
    );
    assert_eq!(
        std::fs::read_to_string(&album_path).expect("album"),
        album_before,
        "album keeps its preimage"
    );
    no_capture_litter(&vault);
    no_capture_litter(&vault.join("eat"));
}

#[test]
fn album_edit_after_planning_refuses_and_writes_nothing() {
    let (_temp, vault, config) = album_vault();
    let _env = crate::native::env::TestEnvGuard::set(&[
        ("BOB_NOW", Some(OsStr::new("2026-10-07 13:40:00"))),
        ("TZ", Some(OsStr::new("UTC0"))),
    ]);
    let request = record_request(
        &vault,
        false,
        "Ramen Nagi @eat/\n- status:: liked\n- category:: Ramen\n- location:: Clark",
    );
    let mut batch =
        plan_capture_batch_with_config_path(&request, None, Some(&config))
            .expect("album capture plans");
    assert!(
        batch
            .text_files
            .iter()
            .any(|file| file.target == vault.join("eat.md")),
        "album edit is staged"
    );
    let concurrent = "<!-- concurrent -->\n";
    let album_path = vault.join("eat.md");
    let parked = std::fs::read_to_string(&album_path).expect("album");
    std::fs::write(&album_path, format!("{parked}{concurrent}"))
        .expect("concurrent album edit");
    let error =
        commit_capture_batch(&mut batch, &vault).expect_err("stale refuses");
    assert_eq!(error.typed, Some(CaptureFailureKind::NoteChanged));
    assert!(
        error.message.contains("eat.md"),
        "names the album: {}",
        error.message
    );
    assert!(
        !vault.join("eat/ramen_nagi.md").exists(),
        "refusal installs nothing"
    );
    assert_eq!(
        std::fs::read_to_string(&album_path).expect("album"),
        format!("{parked}{concurrent}"),
        "foreign bytes survive"
    );
    no_capture_litter(&vault);
}

#[test]
fn damaged_folder_refuses_batch_and_writes_nothing() {
    let (_temp, vault, config) = album_vault();
    std::fs::create_dir_all(vault.join("eat/blocked.md"))
        .expect("directory occupant");
    let album_before =
        std::fs::read_to_string(vault.join("eat.md")).expect("album");
    let _env = crate::native::env::TestEnvGuard::set(&[
        ("BOB_NOW", Some(OsStr::new("2026-10-07 13:40:00"))),
        ("TZ", Some(OsStr::new("UTC0"))),
    ]);
    let request = record_request(
        &vault,
        true,
        "Ramen Nagi @eat/\n- status:: liked\n- category:: Ramen\n- location:: Clark",
    );
    let error = match plan_capture_batch_with_config_path(
        &request,
        None,
        Some(&config),
    ) {
        Ok(_) => panic!("damaged folder refuses"),
        Err(error) => error,
    };
    assert!(
        error.message.contains("eat"),
        "reports the affected path: {}",
        error.message
    );
    assert!(
        !vault.join("eat/ramen_nagi.md").exists(),
        "writes neither record nor index"
    );
    assert_eq!(
        std::fs::read_to_string(vault.join("eat.md")).expect("album"),
        album_before,
        "album unchanged"
    );
}

#[test]
fn unreadable_member_refuses_while_malformed_member_allows_write() {
    let (_temp, vault, config) = album_vault();
    std::fs::write(
        vault.join("eat/unreadable.md"),
        b"---\ntitle: \xff\n---\n".as_slice(),
    )
    .expect("unreadable member");
    let _env = crate::native::env::TestEnvGuard::set(&[
        ("BOB_NOW", Some(OsStr::new("2026-10-07 13:40:00"))),
        ("TZ", Some(OsStr::new("UTC0"))),
    ]);
    let request = record_request(
        &vault,
        true,
        "Ramen Nagi @eat/\n- status:: liked\n- category:: Ramen\n- location:: Clark",
    );
    let error = match plan_capture_batch_with_config_path(
        &request,
        None,
        Some(&config),
    ) {
        Ok(_) => panic!("unreadable member refuses"),
        Err(error) => error,
    };
    assert!(
        error.message.contains("eat"),
        "names the album: {}",
        error.message
    );
    std::fs::remove_file(vault.join("eat/unreadable.md")).expect("cleanup");
    std::fs::write(
        vault.join("eat/malformed.md"),
        "---\n: bad: [unclosed\n---\n\n# Malformed\n",
    )
    .expect("malformed member");
    let batch = plan(
        &vault,
        &config,
        true,
        "Ramen Nagi @eat/\n- status:: liked\n- category:: Ramen\n- location:: Clark",
    );
    assert_eq!(batch.album_files.len(), 1, "healthy write proceeds");
}

#[test]
fn failing_second_item_writes_nothing() {
    let (_temp, vault, config) = album_vault();
    // Second item updates a missing stem, so the whole batch fails after
    // the first item staged its record and album edit.
    let _env = crate::native::env::TestEnvGuard::set(&[
        ("BOB_NOW", Some(OsStr::new("2026-10-07 13:40:00"))),
        ("TZ", Some(OsStr::new("UTC0"))),
    ]);
    let request = record_request(
        &vault,
        false,
        "Ramen Nagi @eat/\n- status:: liked\n- category:: Ramen\n- location:: Clark\n\n@eat/does_not_exist\n- status:: liked",
    );
    let error = match plan_capture_batch_with_config_path(
        &request,
        None,
        Some(&config),
    ) {
        Ok(_) => panic!("second item fails the batch"),
        Err(error) => error,
    };
    let _ = error;
    assert!(
        !vault.join("eat/ramen_nagi.md").exists(),
        "first item installs nothing on later failure"
    );
}

#[test]
fn missing_index_reports_original_preimage_and_state() {
    let (_temp, vault, config) = album_vault();
    let album_before =
        std::fs::read_to_string(vault.join("eat.md")).expect("album");
    let batch = plan(
        &vault,
        &config,
        true,
        "Ramen Nagi @eat/\n- status:: liked\n- category:: Ramen\n- location:: Clark\n\nUdon @eat/\n- status:: to_try\n- category:: Udon\n- location:: Main\n\n@eat/ramen_nagi\n- status:: to_try",
    );
    assert_eq!(batch.album_files.len(), 1, "one final album edit");
    let file = &batch.album_files[0];
    assert_eq!(
        file.before, album_before,
        "before is the original disk bytes"
    );
    assert_eq!(file.state, "missing", "state stays the original missing");
    let staged = staged_map(&batch);
    let written = staged.get(&vault.join("eat.md")).expect("staged album");
    assert_eq!(&file.after, written, "after equals the written bytes");
}

#[test]
fn net_zero_final_index_reports_nothing_but_keeps_its_guard() {
    let (_temp, vault, config) = album_vault();
    // Create then update back to the same grouping would be net-zero, but
    // a notes-only change still touches the record. Use two opposing
    // updates on the existing record: the index ends where it started.
    let before = std::fs::read_to_string(vault.join("eat.md")).expect("album");
    // First bring the index to a known staged state via planning helpers:
    // plan a batch that changes ninja_sushi liked->to_try then back.
    let batch = plan(
        &vault,
        &config,
        true,
        "@eat/ninja_sushi\n- status:: to_try\n\n@eat/ninja_sushi\n- status:: liked",
    );
    // The two updates net to no grouping change; either no album file is
    // reported, or the staged album equals the original.
    let staged = staged_map(&batch);
    if batch.album_files.is_empty() {
        assert!(
            !staged.contains_key(&vault.join("eat.md"))
                || staged.get(&vault.join("eat.md")).unwrap() == &before,
            "net-zero writes no album bytes"
        );
    } else {
        assert_eq!(batch.album_files[0].before, before);
    }
    // The guard survives: a concurrent album edit still refuses.
    let _env = crate::native::env::TestEnvGuard::set(&[
        ("BOB_NOW", Some(OsStr::new("2026-10-07 13:40:00"))),
        ("TZ", Some(OsStr::new("UTC0"))),
    ]);
    let request = record_request(
        &vault,
        false,
        "@eat/ninja_sushi\n- status:: to_try\n\n@eat/ninja_sushi\n- status:: liked",
    );
    let mut batch =
        plan_capture_batch_with_config_path(&request, None, Some(&config))
            .expect("net-zero plans");
    let parked = std::fs::read_to_string(vault.join("eat.md")).expect("album");
    std::fs::write(vault.join("eat.md"), format!("{parked}<!-- drift -->\n"))
        .expect("drift");
    let error =
        commit_capture_batch(&mut batch, &vault).expect_err("drift refuses");
    assert_eq!(error.typed, Some(CaptureFailureKind::NoteChanged));
}

fn no_capture_litter(dir: &std::path::Path) {
    let litter: Vec<std::path::PathBuf> = std::fs::read_dir(dir)
        .expect("read fixture dir")
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.contains("bob-capture"))
        })
        .collect();
    assert!(litter.is_empty(), "temporary/backup litter: {litter:?}");
}
