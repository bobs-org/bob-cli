use super::*;

use serde_json::Value;

pub(crate) fn fixture() -> Value {
    serde_json::from_str(include_str!(
        "../../../tests/fixtures/task_settle/vectors.json"
    ))
    .expect("settle vectors are valid JSON")
}

pub(crate) fn classification(value: &Value) -> TaskClassification {
    let global_filter = value["global_filter"]
        .as_str()
        .expect("classification global_filter");
    let statuses = value["status_types"]
        .as_array()
        .expect("classification status_types")
        .iter()
        .map(|status| {
            let symbol = status["symbol"]
                .as_str()
                .expect("status symbol")
                .chars()
                .next()
                .expect("one-character status symbol");
            let status_type = status["type"].as_str().expect("status type");
            (symbol, status_type)
        })
        .collect::<Vec<_>>();
    TaskClassification::from_status_types(
        global_filter,
        statuses,
        '*',
        '/',
        '?',
        ' ',
    )
}

pub(crate) fn text(value: &Value) -> String {
    let lines = value["lines"]
        .as_array()
        .expect("text lines")
        .iter()
        .map(|line| line.as_str().expect("text line"))
        .collect::<Vec<_>>();
    let ending = value["ending"].as_str().expect("line ending");
    let mut result = lines.join(ending);
    if value["final_newline"].as_bool().expect("final_newline") {
        result.push_str(ending);
    }
    result
}

#[test]
fn settle_vectors_pin_bytes_maps_and_skip_codes() {
    let vectors = fixture();
    assert_eq!(vectors["version"], 1);
    let today = NaiveDate::parse_from_str(
        vectors["today"].as_str().expect("today"),
        "%Y-%m-%d",
    )
    .expect("valid vector date");
    let daily_path =
        Path::new(vectors["daily_path"].as_str().expect("daily_path"));
    let classification = classification(&vectors["classification"]);

    for case in vectors["cases"].as_array().expect("cases") {
        let input = text(&case["input"]);
        let result = settle_note(
            Path::new(case["path"].as_str().expect("path")),
            &input,
            today,
            &classification,
            daily_path,
        );
        let expected = &case["expect"];
        assert_eq!(
            result.contents,
            text(&expected["text"]),
            "{} bytes",
            case["id"]
        );
        assert_eq!(
            result.changed,
            expected["changed"].as_bool().expect("expected changed"),
            "{} changed",
            case["id"]
        );
        let expected_map = if expected["line_map"] == "identity" {
            task_status_hooks::logical_lines(&input)
                .iter()
                .enumerate()
                .map(|(index, _)| (index, index))
                .collect::<BTreeMap<_, _>>()
        } else {
            expected["line_map"]
                .as_array()
                .expect("expected line_map")
                .iter()
                .map(|pair| {
                    let pair = pair.as_array().expect("line-map pair");
                    (
                        pair[0].as_u64().expect("old line") as usize,
                        pair[1].as_u64().expect("new line") as usize,
                    )
                })
                .collect::<BTreeMap<_, _>>()
        };
        assert_eq!(result.line_map, expected_map, "{} line_map", case["id"]);
        let warning_codes = result
            .warnings
            .iter()
            .map(|warning| warning.code.as_str())
            .collect::<Vec<_>>();
        let expected_warnings = expected["warning_codes"]
            .as_array()
            .expect("expected warning_codes")
            .iter()
            .map(|code| code.as_str().expect("warning code"))
            .collect::<Vec<_>>();
        assert_eq!(
            warning_codes, expected_warnings,
            "{} warning_codes",
            case["id"]
        );
    }
}
