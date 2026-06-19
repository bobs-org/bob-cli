---
plan: sdd/tales/202606/capture_embedded_bullet_marker.md
---
 We recently added support for a new `#<X>` syntax to the `bob capture` command that causes a bullet to be created instead of an Obsidian task. This `#<X>` must be at the end of the line, but can be before the special `@<note_file_name>` marker. An insight that I just had is that `#<X>` is never really valid unless `@<note_file_name>` is also set. In light of this realization, I think a change in syntax is justified. Can you help me start requiring that the `#<X>` be appended to the end of `@<note_file_name>` instead of being standalone?

- This means that it can now be found at the beginning of the file too, since `@<note_file_name>` is allowed there.
- For example, `Some note. @foo#bar` should now have the same behavior as `Some note. #bar @foo` would currently.

Think this through thoroughly and create a plan using your `/sase_plan` skill. Submit your plan with the
`sase plan propose` command (as the skill instructs) before making any file changes.
