use super::*;
use crate::native::task_settle::settle_note;

#[test]
fn reconcile_composition_matches_pure_settle_for_every_vector() {
    let vectors = crate::native::task_settle::tests::fixture();
    let today = NaiveDate::parse_from_str(
        vectors["today"].as_str().expect("today"),
        "%Y-%m-%d",
    )
    .expect("valid fixture date");
    let daily_relative =
        PathBuf::from(vectors["daily_path"].as_str().expect("daily path"));
    let classification = crate::native::task_settle::tests::classification(
        &vectors["classification"],
    );

    for case in vectors["cases"].as_array().expect("cases") {
        let temp = tempfile::tempdir().expect("temporary reconcile vault");
        let root = temp.path();
        let relative_path = PathBuf::from(case["path"].as_str().expect("path"));
        let path = root.join(&relative_path);
        std::fs::create_dir_all(path.parent().expect("note parent"))
            .expect("create note directory");
        let input = crate::native::task_settle::tests::text(&case["input"]);
        std::fs::write(&path, &input).expect("write fixture note");

        let daily_path = if relative_path == daily_relative {
            path.clone()
        } else {
            root.join(&daily_relative)
        };
        let mut settings = test_settings();
        let status_types = vectors["classification"]["status_types"]
            .as_array()
            .expect("status types");
        for status in status_types {
            if let Some(symbol) =
                status["symbol"].as_str().and_then(|s| s.chars().next())
            {
                let kind = match status["type"].as_str().expect("status type") {
                    "TODO" => TaskStatusType::Todo,
                    "IN_PROGRESS" => TaskStatusType::InProgress,
                    "ON_HOLD" => TaskStatusType::OnHold,
                    "DONE" => TaskStatusType::Done,
                    "CANCELLED" => TaskStatusType::Cancelled,
                    other => panic!("unexpected task status type {other}"),
                };
                settings.status_types.insert(symbol, kind);
            }
        }
        let files = [FileScan {
            path: path.clone(),
            relative_path: relative_path.clone(),
            contents: input.clone(),
            tasks: parse_tasks(&input, &settings),
        }];
        let daily_contents = if relative_path == daily_relative {
            input.as_str()
        } else {
            ""
        };
        let pure = settle_note(
            &relative_path,
            &input,
            today,
            &classification,
            &daily_relative,
        );
        let composed = compose_outputs(
            &files,
            &[],
            ComposeContext {
                daily_path: &daily_path,
                daily_relative_path: Some(&daily_relative),
                previous_daily_path: None,
                today,
                daily_contents,
                normalized_daily_contents: daily_contents,
                settings: &settings,
            },
            &BTreeSet::new(),
            &BTreeMap::new(),
        );
        let output = composed
            .outputs
            .iter()
            .find(|output| output.path == path)
            .map(|output| output.contents.as_str())
            .unwrap_or(&input);
        assert_eq!(output, pure.contents, "{}", case["id"]);
    }
}
