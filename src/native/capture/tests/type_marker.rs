use super::*;

#[test]
fn planner_accepts_a_recognized_new_task_type_marker() {
    // The writes phase stages `t:` markers instead of refusing them.
    // A missing vault fails later with an IO error, never the temporary
    // `type_marker_not_supported` code.
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

    if let Err(error) = plan_capture_batch(&request, None) {
        assert_ne!(
            error.code.as_deref(),
            Some("type_marker_not_supported"),
            "writes phase must not emit the temporary refusal"
        );
    }
}
