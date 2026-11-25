use super::*;

use serde_json::Value;

/// Fixture proof shared by every command-settle writer test: settle the
/// writer's staged postimages exactly the way the command does, then prove
/// a follow-up reconcile plans nothing note-local.
///
/// Proof chain: q4.3's
/// `reconcile_composition_matches_pure_settle_for_every_vector` pins
/// reconcile's note-local pass byte-identical to `settle_note`, so a
/// postimage that is a settle fixed point yields no compose output — the
/// later reconcile has nothing note-local to plan. Returns whether the
/// settle changed any postimage, so each writer test can also prove its
/// wire fires (guarding against vacuous fixtures settle would ignore).
pub(crate) fn assert_command_settle_holds(
    postimages: &mut BTreeMap<PathBuf, String>,
) -> bool {
    let temp = tempfile::tempdir().expect("settle fixture vault");
    let before = postimages.clone();
    settle_plan_postimages(temp.path(), postimages);
    let changed = postimages != &before;
    let again = SettleContext::for_vault(temp.path());
    for (relative, contents) in postimages.iter() {
        let fixed = again.settle_written_note(relative, contents);
        assert!(
            !fixed.changed,
            "settled postimage must be a fixed point: {relative:?}"
        );
        assert_eq!(
            &fixed.contents, contents,
            "settled postimage must be a fixed point: {relative:?}"
        );
    }
    changed
}

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

#[test]
fn command_settle_leaves_gmail_filing_with_nothing_note_local() {
    // Fixture proof for phase command-settle (`bob gmail sync`): a filed
    // task appended at intake plus a future-scheduled row settle to a fixed
    // point, so a follow-up reconcile plans nothing note-local.
    let filed = "- [ ] #task Follow up on the thread [[gmail/2026/aa01|Lease renewal]] ^filed";
    let (inserted, _) = crate::native::capture::insert_task_line(
        "---\ntype: [[area]]\n---\n## Tasks\n\n- [ ] #task Existing ^keep\n- [ ] #task Nudge [scheduled:: 2099-06-01] ^nudge\n",
        filed,
    );
    let mut postimages =
        BTreeMap::from([(PathBuf::from("areas/work.md"), inserted)]);
    let fired = assert_command_settle_holds(&mut postimages);
    assert!(fired, "settle must regroup the gmail destination");
    let settled = &postimages[&PathBuf::from("areas/work.md")];
    assert!(settled.contains("- [?] #task Nudge"), "{settled}");
    assert!(settled.contains(filed), "{settled}");
}

#[test]
fn command_settle_leaves_gkeep_pull_with_nothing_note_local() {
    // Fixture proof for phase command-settle (`bob gkeep pull`): imported
    // Keep blocks appended to the pull target settle to a fixed point.
    let imported =
        "- [ ] #task Buy oat milk ^keep1\n- [ ] #task Call the dentist ^keep2";
    let (inserted, _) = crate::native::capture::insert_task_line(
        "---\ntype: [[area]]\nparent: [[inbox]]\n---\n## Tasks\n\n- [ ] #task Existing ^keep\n- [ ] #task Later [scheduled:: 2099-06-01] ^later\n",
        imported,
    );
    let mut postimages =
        BTreeMap::from([(PathBuf::from("areas/keep_inbox.md"), inserted)]);
    let fired = assert_command_settle_holds(&mut postimages);
    assert!(fired, "settle must regroup the gkeep target");
    let settled = &postimages[&PathBuf::from("areas/keep_inbox.md")];
    assert!(settled.contains("- [?] #task Later"), "{settled}");
    assert!(settled.contains("Buy oat milk"), "{settled}");
}

#[test]
fn command_settle_leaves_ref_writes_with_nothing_note_local() {
    // Fixture proof for phase command-settle (`bob ref create` / routed
    // ref-sync writes): an annotation task routed into an area note plus a
    // reading task insert settle to a fixed point.
    let routed =
        "- [ ] #task Revisit the methods section [[ref/papers/x|X]] ^anno1";
    let (inserted, _) = crate::native::capture::insert_task_line(
        "---\ntype: [[area]]\n---\n## Tasks\n\n- [ ] #task Existing ^keep\n- [ ] #task Read soon [scheduled:: 2099-06-01] ^soon\n",
        routed,
    );
    let reading =
        "- [ ] #task #ref [[ref/papers/y|Y]] [created::2026-11-25] ^ref-y\n";
    let mut postimages = BTreeMap::from([
        (PathBuf::from("areas/work.md"), inserted),
        (
            PathBuf::from("projects/thesis.md"),
            format!(
                "---\ntype: [[project]]\nstatus: active\n---\n## Tasks\n\n{reading}"
            ),
        ),
    ]);
    let fired = assert_command_settle_holds(&mut postimages);
    assert!(fired, "settle must regroup the ref destinations");
    let routed_settled = &postimages[&PathBuf::from("areas/work.md")];
    assert!(
        routed_settled.contains("- [?] #task Read soon"),
        "{routed_settled}"
    );
    let reading_settled = &postimages[&PathBuf::from("projects/thesis.md")];
    assert!(reading_settled.contains("^ref-y"), "{reading_settled}");
}
