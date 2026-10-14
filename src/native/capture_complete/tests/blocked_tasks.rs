use super::{
    super::shell::shell_completion, write_file, write_settings, TempDir,
};

#[test]
fn blocked_task_shell_rows_use_the_blocked_tasks_group() {
    let temp = TempDir::new("bob-cli-capture-complete-blocked-shell");
    write_settings(temp.path());
    write_file(
        &temp.path().join("mac_inbox.md"),
        "- [ ] #task Owner ^owner\n",
    );
    write_file(
        &temp.path().join("tasks.md"),
        "- [ ] #task Blocked by new work ^task-id\n",
    );
    let day_file = temp.path().join("2026/20260930.md");
    write_file(&day_file, "## Pomodoros\n- [ ] () — PLAN\n");

    let shell = crate::native::env::with_var("BOB_DAY_FILE", &day_file, || {
        crate::native::env::with_var("BOB_NOW", "2026-09-30 09:02:00", || {
            shell_completion(temp.path(), "New work *", "New work *".len())
                .expect("shell completion")
        })
    })
    .expect("blocked-task shell rows");
    assert_eq!(shell.marker_start, "New work ".len());
    assert!(shell.rows.iter().any(|row| {
        row.full == "*tasks:task-id" && row.group == "blocked tasks"
    }));
}
