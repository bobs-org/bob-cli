use super::*;

#[test]
fn planner_rejects_a_recognized_new_task_type_until_writes_land() {
    let request = CaptureRequest {
        bob_dir: Path::new("/path/that/does/not/exist").to_path_buf(),
        dry_run: true,
        forced_clip: None,
        forced_destination_flags: Vec::new(),
        forced_route: None,
        forced_section: None,
        forced_sub_bullet_target: None,
        forced_task_section: None,
        inbox_routes: Vec::new(),
        no_clip: true,
        no_ref: false,
        pull_in: None,
        refuse_override: false,
        require_inbox_route_choice: false,
        require_pull_in_choice: false,
        template_origin: None,
        raw_text: "Fix the parser t:fix".to_string(),
        selection: crate::native::task_complete::SuccessorSelection::None,
    };

    let error = match plan_capture_batch(&request, None) {
        Err(error) => error,
        Ok(_) => panic!("capture type writes belong to the later writes phase"),
    };
    assert_eq!(error.code.as_deref(), Some("type_marker_not_supported"));
    assert!(error.message.contains("writing task types"));
}
