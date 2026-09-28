//! Projects list tests.

use crate::support::*;

#[test]
fn projects_list_scans_project_notes_and_renders_counts() {
    let temp = TempDir::new("bob-cli-projects-list");
    let vault = temp.path().join("vault");

    write_file(
        &vault.join("Alpha.md"),
        r#"---
type: "[[project]]"
status: wip
---
- [ ] #task Finish Alpha #hide ^prj
- [ ] #task shown one
- [/] #task shown in progress
- [*] #task shown next
- [?] #task dependency blocked
- [ ] #task future scheduled [scheduled::2999-01-01]
- [ ] #task hidden helper #hide
- [x] #task done task
- [-] #task canceled task
"#,
    );
    write_file(
        &vault.join("Beta.md"),
        r#"---
type: [[project]]
status: waiting
---
- [ ] #task Finish Beta [scheduled::2026-06-11] ^prj
- [ ] #task planned #hide
"#,
    );
    write_file(
        &vault.join("Done.md"),
        r#"---
type: [[project]]
status: done
---
- [X] #task Finish Done #hide ^prj
"#,
    );
    write_file(
        &vault.join("Canceled.md"),
        r#"---
type: [[project]]
status: canceled
---
- [-] #task Cancel Canceled #hide ^prj
"#,
    );
    write_file(
        &vault.join("Missing.md"),
        r#"---
type: [[project]]
status: wip
---
- [ ] #task Needs prj
"#,
    );
    write_file(
        &vault.join("Placeholder.md"),
        r#"---
type: [[project]]
status: wip
---
- [ ] #task <short_project_completion_criteria_goes_here> #hide ^prj
"#,
    );
    write_file(
        &vault.join("_templates/Template.md"),
        "---\ntype: [[project]]\n---\n- [ ] #task hidden #hide ^prj\n",
    );
    write_file(
        &vault.join(".obsidian/Hidden.md"),
        "---\ntype: [[project]]\n---\n- [ ] #task hidden #hide ^prj\n",
    );
    write_file(
        &vault.join("done/Archived.md"),
        "---\ntype: [[project]]\n---\n- [ ] #task hidden #hide ^prj\n",
    );

    let output = bob_command()
        .arg("projects")
        .arg("list")
        .arg("--bob-dir")
        .arg(&vault)
        .output()
        .expect("run bob projects list");

    assert_success(&output);
    assert!(
        stderr(&output).is_empty(),
        "unexpected stderr:\n{}",
        stderr(&output)
    );
    assert_stdout_has_no_ansi(&output);
    let out = stdout(&output);
    assert!(
        out.contains("Projects - 3 active - 1 waiting - 1 done - 1 canceled"),
        "unexpected summary:\n{out}"
    );
    assert!(
        out.contains("PROJECT")
            && out.contains("STATUS")
            && out.contains("SHOWN")
            && out.contains("^PRJ"),
        "missing table header:\n{out}"
    );
    assert!(out.contains("Alpha") && out.contains("wip"));
    assert!(
        out.contains("   7      3  open"),
        "unexpected Alpha counts:\n{out}"
    );
    assert!(
        out.contains("Beta") && out.contains("on dash"),
        "missing on-dash Beta row:\n{out}"
    );
    assert!(
        !out.contains("scheduled 2026-06-11"),
        "scheduled field should not render in ^PRJ column:\n{out}"
    );
    assert!(out.contains("Done") && out.contains("done"));
    assert!(out.contains("Canceled") && out.contains("canceled"));
    assert!(out.contains("Missing") && out.contains("missing"));
    assert!(out.contains("Placeholder") && out.contains("placeholder"));
    assert!(
        !out.contains("Template")
            && !out.contains("Hidden")
            && !out.contains("Archived"),
        "excluded directories should not be listed:\n{out}"
    );
    assert_text_order(
        &out,
        &[
            "Alpha",
            "Missing",
            "Placeholder",
            "Beta",
            "Done",
            "Canceled",
        ],
    );
}

#[test]
fn projects_list_reports_prj_errors_without_aborting_scan() {
    let temp = TempDir::new("bob-cli-projects-errors");
    let vault = temp.path().join("vault");
    write_file(
        &vault.join("Good.md"),
        "---\ntype: [[project]]\n---\n- [ ] #task Good project #hide ^prj\n",
    );
    write_file(
        &vault.join("Malformed.md"),
        "---\ntype: [[project]]\n---\nComplete malformed project ^prj\n",
    );
    write_file(
        &vault.join("Multiple.md"),
        "---\ntype: [[project]]\n---\n- [ ] #task One #hide ^prj\n- [ ] #task Two #hide ^prj\n",
    );

    let output = bob_command()
        .arg("projects")
        .arg("list")
        .arg("-b")
        .arg(&vault)
        .output()
        .expect("run bob projects list with errors");

    assert_eq!(
        output.status.code(),
        Some(1),
        "project scan errors should exit 1:\n{}",
        format_output(&output)
    );
    let out = stdout(&output);
    assert!(
        out.contains("Good")
            && out.contains("Malformed")
            && out.contains("Multiple"),
        "list should still render every project row:\n{out}"
    );
    let err = stderr(&output);
    assert!(
        err.contains("Malformed.md:4: malformed ^prj task")
            && err.contains("Multiple.md:5: multiple ^prj tasks"),
        "expected per-file project errors:\n{err}"
    );
    assert_stdout_has_no_ansi(&output);
}
