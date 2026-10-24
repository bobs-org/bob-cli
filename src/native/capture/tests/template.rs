//! The execution oracle for templates: planning a draft must match planning
//! the text produced by `bob capture -E`.

use super::*;
use std::collections::BTreeMap;

fn template_request(vault: &Path, raw_text: &str) -> CaptureRequest {
    CaptureRequest {
        bob_dir: vault.to_path_buf(),
        dry_run: true,
        forced_clip: None,
        forced_destination_flags: Vec::new(),
        forced_route: None,
        forced_section: None,
        forced_sub_bullet_target: None,
        forced_task_section: None,
        no_clip: false,
        no_ref: false,
        pull_in: None,
        refuse_override: false,
        require_inbox_route_choice: false,
        require_pull_in_choice: false,
        template_origin: None,
        raw_text: raw_text.to_string(),
        selection: crate::native::task_complete::SuccessorSelection::None,
    }
}

fn staged_contents(batch: &PlannedCaptureBatch) -> BTreeMap<PathBuf, String> {
    batch
        .text_files
        .iter()
        .map(|file| (file.target.clone(), file.updated_target.clone()))
        .collect()
}

#[test]
fn planning_each_template_matches_planning_its_expansion() {
    let temp = tempfile::tempdir().expect("create template oracle directory");
    let vault = temp.path().join("vault");
    fs::create_dir_all(vault.join("2026")).expect("create vault");
    let tasks_settings = vault.join(".obsidian/plugins/obsidian-tasks-plugin");
    fs::create_dir_all(&tasks_settings).expect("create Tasks settings folder");
    fs::write(
        tasks_settings.join("data.json"),
        r##"{
          "globalFilter": "#task",
          "taskFormat": "dataview",
          "statusSettings": {
            "coreStatuses": [
              {"symbol":" ","name":"Ready","nextStatusSymbol":"x","availableAsCommand":true,"type":"TODO"},
              {"symbol":"x","name":"Done","nextStatusSymbol":" ","availableAsCommand":true,"type":"DONE"}
            ],
            "customStatuses": [
              {"symbol":"/","name":"In Progress","nextStatusSymbol":"x","availableAsCommand":true,"type":"IN_PROGRESS"},
              {"symbol":"*","name":"Next","nextStatusSymbol":"x","availableAsCommand":true,"type":"ON_HOLD"},
              {"symbol":"?","name":"Blocked","nextStatusSymbol":" ","availableAsCommand":true,"type":"ON_HOLD"},
              {"symbol":"-","name":"Canceled","nextStatusSymbol":" ","availableAsCommand":true,"type":"CANCELLED"}
            ]
          }
        }"##,
    )
    .expect("write Tasks settings");
    let work = concat!(
        "## Tasks\n\n",
        "- [ ] #task Existing parent ^parent\n",
        "- [ ] #task Existing one ^existing-one\n",
        "- [ ] #task Existing two ^existing-two\n",
        "- [ ] #task Prerequisite ^prereq\n",
        "- [ ] #task Blocked target ^blocked\n",
        "- [ ] #task Complete one ^done-one\n",
        "- [ ] #task Complete two ^done-two\n",
    );
    fs::write(vault.join("work.md"), work).expect("write work tasks");
    fs::write(vault.join("notes.md"), "## Ideas\n").expect("write notes file");
    let day_file = vault.join("2026/20260615.md");
    fs::write(
        &day_file,
        "## Pomodoros\n- [ ] (**0900-0930** [t:: 30m]) Work\n",
    )
    .expect("write Pomodoro day file");
    let config = temp.path().join("config.yml");
    fs::write(
        &config,
        concat!(
            "properties:\n",
            "  - name: priority\n",
            "    values: priority\n",
            "    schedules: scheduled\n",
            "    levels:\n",
            "      - label: P1\n",
            "        value: high\n",
            "        min_days: 2\n",
            "        max_days: 7\n",
        ),
    )
    .expect("write deterministic priority config");

    let cases = [
        (
            "route",
            "@@template:\nTask {0} @work\n\none\ntwo\n",
        ),
        (
            "requested block ID",
            "@@template:\nTask {0} @work^{1}\n\none first\ntwo second\n",
        ),
        (
            "Pomodoro task link",
            "@@template:\nTask {0} @work:{1}\n\none link-one\ntwo link-two\n",
        ),
        (
            "existing-task sub-bullet",
            "@@template:\n@work+parent Child {0}\n\none\ntwo\n",
        ),
        (
            "section bullet",
            "@@template:\nIdea {0} @notes#Ideas\n\none\ntwo\n",
        ),
        (
            "schedule marker",
            "@@template:\nTask {0} s:1 @work\n\none\ntwo\n",
        ),
        (
            "priority marker",
            "@@template:\nTask {0} $1 @work\n\none\ntwo\n",
        ),
        (
            "prerequisite marker",
            "@@template:\nTask {0} &work:prereq @work^{1}\n\none dep-one\ntwo dep-two\n",
        ),
        (
            "blocked-task marker",
            "@@template:\nTask {0} *work:blocked @work^{1}\n\none block-one\ntwo block-two\n",
        ),
        (
            "bulk completion",
            "@@template:\n!work:{0}\n\ndone-one\ndone-two\n",
        ),
        (
            "existing task link",
            "@@template:\n^work:{0}\n\nexisting-one\nexisting-two\n",
        ),
        (
            "child placeholders",
            "@@template:\nTask {0} @work^{1}\n- child {2}\n\nfirst one detail\nsecond two detail\n",
        ),
        (
            "bare URL references",
            "@@template:\n{0}\n\nhttps://example.com/template-one\nhttps://example.com/template-two\n",
        ),
        (
            "reference listen markers",
            "@@template:\n{0}\n\n\"https://example.com/listen-one %\"\n\"https://example.com/listen-two %\"\n",
        ),
    ];
    let routing = UrlRoutingPolicy::default();
    let state = temp.path().join("state");

    crate::native::env::with_var("BOB_NOW", "2026-06-15 10:11:12", || {
        crate::native::env::with_var(
            "BOB_CONFIG_FILE",
            config.as_os_str().to_os_string(),
            || {
                crate::native::env::with_var(
                    "BOB_PRIORITY_ROLL_SEED",
                    "1234",
                    || {
                        crate::native::env::with_var(
                            "BOB_DAY_FILE",
                            day_file.as_os_str().to_os_string(),
                            || {
                                crate::native::env::with_var(
                                    "XDG_STATE_HOME",
                                    state.as_os_str().to_os_string(),
                                    || {
                                        for (label, draft) in cases {
                                            let expansion =
                                                capture_language::template::expand(draft)
                                                    .expect("template is valid")
                                                    .expect("template expands");
                                            let expanded = expansion.text;
                                            let planned_template =
                                                plan_capture_batch(
                                                    &template_request(
                                                        &vault, draft,
                                                    ),
                                                    Some(&routing),
                                                )
                                                .unwrap_or_else(|error| {
                                                    panic!(
                                                        "{label}: {}",
                                                        error.message
                                                    )
                                                });
                                            let planned_expansion =
                                                plan_capture_batch(
                                                    &template_request(
                                                        &vault, &expanded,
                                                    ),
                                                    Some(&routing),
                                                )
                                                .unwrap_or_else(|error| {
                                                    panic!(
                                                        "{label} expansion: {}",
                                                        error.message
                                                    )
                                                });

                                            assert_eq!(
                                                staged_contents(&planned_template),
                                                staged_contents(&planned_expansion),
                                                "staged file contents differ for {label}",
                                            );
                                            let template_results =
                                                planned_template
                                                    .items
                                                    .iter()
                                                    .map(|item| &item.result)
                                                    .collect::<Vec<_>>();
                                            let expansion_results =
                                                planned_expansion
                                                    .items
                                                    .iter()
                                                    .map(|item| &item.result)
                                                    .collect::<Vec<_>>();
                                            assert_eq!(
                                                template_results,
                                                expansion_results,
                                                "item results differ for {label}",
                                            );
                                        }
                                    },
                                )
                            },
                        )
                    },
                )
            },
        )
    });
}
