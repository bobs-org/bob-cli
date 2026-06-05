---
create_time: 2026-06-05 13:50:39
status: wip
prompt: sdd/prompts/202606/obsidian_legacy_links.md
---
# Obsidian Legacy Link Migration Plan

## Context

The target vault is `~/bob`. The requested verification command currently finds 3,927 files with `# ^ =` legacy parent
links:

- 3,786 normal note files where the legacy link block appears near the top of the note after YAML frontmatter.
- 141 `_generated/queries/...` files where the `# ^ =` line appears inside a fenced query-header snapshot.

All affected files have YAML frontmatter. Six affected files already have a `parent:` property, and each existing value
matches its legacy `# ^ =` target, so those files should keep the existing parent field and only remove the old line. No
affected file currently has a `Related notes:` section.

## Migration Rules

1. Parse every file returned by `rg "# \^ = " ~/bob -l`.
2. Extract the wikilink from the first legacy parent line matching `# ^ = [[...]]`.
3. Add `parent: [[...]]` to the existing YAML frontmatter unless a matching `parent:` already exists.
4. Abort the migration if a file has an existing `parent:` that conflicts with the legacy parent target.
5. For normal note files, rewrite only the initial legacy block after frontmatter:
   - Remove separator lines that are exactly `#`.
   - Convert descriptive single-`# ` lines in that block into plain text by removing the leading `# `.
   - Preserve any trailing prose from a parent line, such as `# ^ = [[who]] : Links...`, as plain text.
   - Convert shortcut link lines into a `Related notes:` section.
   - Preserve trailing descriptions on shortcut link lines when present, for example `# 0 = [[build]] | #build` becomes
     `- [[build]] | #build`.
   - Include the observed non-alphanumeric shortcut links (`<`, `>`, `@`) in the same related-notes conversion so the
     rewritten top block does not keep legacy link headings behind.
   - Leave real Markdown headings such as `## Notes` outside the initial legacy block untouched.
6. For generated query snapshot files, add the parent frontmatter field and remove the fenced `# ^ = [[...]]` line; do
   not attempt to restructure the generated query body beyond that mechanical cleanup.

## Implementation Steps

1. Create a small migration script in the workspace that supports dry-run and write modes.
2. Run the script in dry-run mode and inspect summary counts plus representative diffs for:
   - `~/bob/zorg_alt_tools.md`
   - `~/bob/work.md`
   - `~/bob/people.md`
   - one daily/log note with `<`, `>`, and `@` links
   - one `_generated/queries/...` snapshot
3. Run the script in write mode after the dry-run looks correct.
4. Verify with:
   - `rg "# \^ = " ~/bob -l` has no output.
   - `rg "^# [A-Z0-9] = \\[\\[" ~/bob -n` has no matches in files that previously had `# ^ =`.
   - Spot-check the same representative files for readable frontmatter and `Related notes:` formatting.
