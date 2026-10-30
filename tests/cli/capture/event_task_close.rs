use crate::support::*;
use serde_json::Value;
use std::fs;

fn vectors() -> Value {
    serde_json::from_str(include_str!(
        "../../fixtures/event_task_close/vectors.json"
    ))
    .expect("Event Task close vectors")
}

#[test]
fn capture_close_matches_shared_event_task_vectors() {
    for case in vectors()["close"].as_array().expect("close vectors") {
        let Some(close_token) = case["close_token"].as_str() else {
            // The untimed guard is covered below; there is no running Pomodoro
            // for bob capture to close in that vector.
            continue;
        };
        let id = case["id"].as_str().expect("case id");
        let temp = TempDir::new(&format!("bob-cli-event-close-{id}"));
        let vault = temp.path().join("vault");
        let day_file = vault.join("2026/20261029.md");
        write_toggle_task_settings(&vault);
        write_file(&day_file, case["day"].as_str().expect("day contents"));
        if let Some(relative) = case["other_note_path"].as_str() {
            write_file(
                &vault.join(relative),
                case["other_note"].as_str().expect("other note"),
            );
        }

        let result = super::pomodoro_close::run_close_json(
            &vault,
            &day_file,
            case["now"].as_str().expect("close time"),
            &[close_token],
        );
        let task_links = result["pomodoro_close"]["task_links"]
            .as_array()
            .expect("close task links");
        let expected_outcomes = case["expected_outcomes"]
            .as_array()
            .expect("expected outcomes");
        let expected_clock = case["expected_clock"]
            .as_array()
            .expect("expected clock classes");
        assert_eq!(task_links.len(), expected_outcomes.len(), "{id}");
        assert_eq!(task_links.len(), expected_clock.len(), "{id}");
        for (index, task_link) in task_links.iter().enumerate() {
            assert_eq!(
                task_link["outcome"],
                expected_outcomes[index],
                "{id} link {}",
                index + 1
            );
            match expected_clock[index].as_str() {
                Some(class) => assert_eq!(
                    task_link["clock"]["class"],
                    class,
                    "{id} link {}",
                    index + 1
                ),
                None => assert!(
                    task_link.get("clock").is_none(),
                    "{id} link {} must not have clock metadata: {task_link}",
                    index + 1
                ),
            }
        }
        if !case["expected_day"].is_null() {
            let actual_day = fs::read_to_string(&day_file).expect("closed day");
            let expected_day =
                case["expected_day"].as_str().expect("expected day");
            assert_eq!(actual_day, expected_day, "{id}");
        }
    }
}

#[test]
fn human_close_adds_event_clock_captions() {
    let case = &vectors()["close"][0];
    let temp = TempDir::new("bob-cli-event-close-captions");
    let vault = temp.path().join("vault");
    let day_file = vault.join("2026/20261029.md");
    write_toggle_task_settings(&vault);
    write_file(&day_file, case["day"].as_str().expect("day"));
    let output = bob_command()
        .arg("capture")
        .arg("-b")
        .arg(&vault)
        .arg("--")
        .arg("x")
        .env("BOB_DAY_FILE", &day_file)
        .env("BOB_NOW", "2026-10-29 10:00:00")
        .output()
        .expect("run human close");
    assert_success(&output);
    let rendered = stdout(&output);
    for caption in [
        "ended 09:00",
        "running until 10:30",
        "starts 10:00",
        "ended 07:30 · before this session",
    ] {
        assert!(
            rendered.contains(caption),
            "missing {caption:?}: {rendered}"
        );
    }
}
