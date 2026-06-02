---
plan: sdd/epics/202606/collect_done.md
---
 Can you help me write a new `bob collect-done` command to help me manage the done and canceled tasks that
live in various files throughout my `~/bob/` Obsidian vault?

- This command will first search for any note file that has 10 or more (10 is the default but should be configurable via
  a CLI option) done Obsidian Tasks (i.e. `- [ ]` checkbox bullets which are tagged with `#task`).
- For each of these note files, we will move all done tasks (including any sub-bullets that task has and any other lines
  that are associated with that task) to a new note file (create if it doesn't exist yet) named
  `~/bob/done/<path_to_note_file>_done.md` where `<path_to_note_file>` is the full Obsidian-relative path of the note
  file which contained the done tasks (for example, done tasks in the `~/bob/foo/bar.md` file should be moved to the
  `~/bob/done/foo/bar_done.md` file).
- These `~/bob/done/<path_to_note_file>_done.md` note files should be given a `parent` frontmatter property with a value
  of `[[done]]` (to link to the `~/bob/done.md` file).
- If the `ob` command is available on the machine, this command should run `ob sync` (to sync the Obsidian vault) before
  making any file changes.
- Any file changes this command makes should be git committed and pushed if the `~/bob/` directory is a git repo. If it
  is not, we should NOT initialize it as one (only this machine uses git for `~/bob/`). We should emit a warning message
  to the user if `~/bob/` is not a git repo though.
- Make sure this command has rich output that tells the user what it is doing as it is doing it. I want you to lead the design on this one. Just make sure it looks beautiful!
- Run the `bob collect-done` command when you are done and verify that it worked properly (a new git commit should be
  created with the appropriate changes).

This is a large piece of work that should be split into phases. I'll let you decide how many phases to create, but
keep in mind that each phase will be completed by a distinct agent instance (i.e. a distinct `claude` / `gemini` /
`codex` / `qwen` / `opencode` command). Think this through thoroughly and create a plan using your `/sase_plan` skill. Submit your plan with the `sase plan`
command (as the skill instructs) before making any file changes.

