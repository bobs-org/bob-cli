---
create_time: 2026-06-19 09:38:09
status: done
prompt: sdd/prompts/202606/capture_bullet_section_matching.md
---
# Plan: Capture Bullet Section Matching Refinement

## Goal

Adjust `bob capture` bullet placement so terminal `#<X>` section-prefix matching behaves the way Bryan expects:

- A matching H1 heading is chosen only when there is no matching non-H1 heading anywhere in the note.
- The `<X>` prefix comparison is case insensitive, so `Some note. @foo #r` and `Some note. @foo #R` select the same
  matching section.

This is a refinement to the bullet-capture behavior added in the previous `capture_bullets` implementation. It is not a
new subcommand or option, and task capture should remain byte-for-byte compatible.

## Context Reviewed

- `src/native/capture.rs`
  - `parse_capture_text` and `peel_terminal_controls` parse the terminal bullet marker and store
    `CaptureKind::Bullet { section_prefix }`.
  - `insert_bullet_line` calls `markdown_headings` and `target_bullet_section`.
  - `target_bullet_section` currently walks headings in document order and selects the first heading where
    `title != "Tasks"` and `title.starts_with(prefix)` when a prefix exists.
  - `markdown_headings` currently returns `(line_index, title)` and skips YAML frontmatter and fenced code blocks.
  - `tasks_section` also uses `markdown_headings`, so any heading representation change must preserve task insertion.
- `tests/cli.rs`
  - Current bullet integration coverage includes routed bullet insertion by prefix, terminal marker order equivalence,
    bare `#`, and JSON output.
- `README.md` and `src/native/capture.rs` long help currently document "first non-Tasks section whose heading title
  starts with the prefix", so they need wording updates after the behavior changes.

I did not read `memory/long/cli_rules.md` because this change does not add or alter CLI subcommands or options.

## Current Behavior

For a note like:

```markdown
# Roadmap

intro

## Research

notes
```

`bob capture "Some note @foo #R"` currently targets `# Roadmap`, because it is the first non-`Tasks` heading whose title
starts with `R`.

It also currently treats the section prefix case sensitively. `#R` can match `Research`, but `#r` does not match
`Research` and may fall back to the zeroth section or some other lowercase heading.

## Desired Semantics

For bullet captures only:

- A heading matches when it is not exactly `Tasks` and its title starts with the marker prefix using case-insensitive
  comparison.
- For a bare `#`, every non-`Tasks` heading is a match.
- Among matching headings, prefer the first non-H1 heading in document order.
- If there is no matching non-H1 heading, choose the first matching H1 heading in document order.
- If there is no matching heading at any level, keep the existing fallback to the zeroth, pre-heading section.
- Once a target heading is chosen, keep the existing insertion mechanics unchanged:
  - scan only that section body for top-level ordinary bullets;
  - insert after the last ordinary bullet block when present;
  - otherwise insert just below the heading;
  - continue skipping headings inside frontmatter and fenced code blocks.

The exact `Tasks` exclusion remains unchanged: only a heading whose parsed title is exactly `Tasks` is skipped.

## Implementation Approach

1. Introduce a richer heading representation in `src/native/capture.rs`.

   Replace the `(usize, &str)` tuple returned by `markdown_headings` with a small struct, for example:

   ```rust
   struct MarkdownHeading<'a> {
       line_index: usize,
       level: usize,
       title: &'a str,
   }
   ```

   Change `atx_heading_title` into a helper that returns both ATX heading level and stripped title, or add a companion
   helper while preserving the existing ATX parsing rules.

2. Preserve existing heading scanning rules.

   `markdown_headings` must continue to:
   - ignore YAML frontmatter headings;
   - ignore fenced-code headings;
   - accept ATX headings indented by up to three spaces;
   - strip valid closing ATX hashes;
   - reject malformed headings exactly as today.

3. Update `tasks_section` to use the new heading struct without changing task behavior.

   It should still find the first parsed heading whose title is exactly `Tasks`, use the same heading end byte offset,
   and stop the task section at the next parsed heading of any level.

4. Update bullet target selection.

   In `target_bullet_section`, split selection into two passes over the parsed headings:
   - first pass: first matching heading where `level != 1`;
   - second pass: first matching heading where `level == 1`.

   Use the selected heading's position in the headings vector to keep existing section-boundary behavior with
   `headings[pos + 1]`.

5. Add a dedicated case-insensitive prefix matcher.

   Add a small helper such as `heading_matches_bullet_prefix(title, section_prefix)` that returns true for a bare marker
   and otherwise compares `title` with the prefix case-insensitively. Prefer a clear implementation over clever slicing;
   computing lowercase comparison strings here is acceptable because this runs on one note during capture, not in a hot
   loop.

   Keep the parsed `section_prefix` value as provided by the user. The behavior change belongs in matching, not in the
   parse result or rendered output.

6. Update docs and help text.

   Revise:
   - `src/native/capture.rs` long help;
   - `README.md` capture section.

   The docs should say the prefix comparison is case insensitive and that matching non-H1 sections are preferred over H1
   headings, with H1 used only as the fallback among matching headings.

## Tests

Add focused unit tests in `src/native/capture.rs`:

- `bullet_prefers_non_h1_match_over_earlier_h1_match`
  - Input note has `# Roadmap` before `## Research`; prefix `R` inserts under `## Research`.
- `bullet_uses_h1_match_when_no_non_h1_match_exists`
  - Input note has only an H1 matching the prefix; insertion still happens under that H1.
- `bare_bullet_marker_prefers_non_h1_section`
  - Input note has an H1 before an H2; bare `#` selects the H2 because every non-`Tasks` heading matches.
- `bullet_section_prefix_matches_case_insensitively`
  - Prefix `r` matches heading `## Research`.

Adjust existing tests if their names or expectations imply "first non-Tasks section" without the H1 preference nuance.

Add at least one CLI integration test in `tests/cli.rs`:

- Use a routed file `foo.md` with both `# Roadmap` and `## Research`.
- Run `bob capture -b <vault> "Some note" "@foo" "#r"` under a fixed `BOB_NOW`.
- Assert the bullet lands under `## Research`.
- Optionally run a parallel `#R` case in a fresh vault and assert it produces identical file contents to `#r`.

## Verification

Run:

```bash
cargo fmt --check
cargo test capture
cargo test --test cli capture_bullet
cargo test
cargo clippy --all-targets
```

Also do a scratch-vault smoke check:

```bash
tmp="$(mktemp -d)"
printf '# Roadmap\nintro\n\n## Research\nnotes\n' > "$tmp/foo.md"
BOB_NOW=2026-06-15 bob capture -b "$tmp" 'Some note' '@foo' '#r'
cat "$tmp/foo.md"
```

Expected result: the new bullet appears under `## Research`, not under `# Roadmap`.

## Risks

- Changing `markdown_headings` affects both task and bullet placement. Mitigation: keep scanner behavior identical and
  only add heading level metadata.
- Case-insensitive matching can be over-implemented. Mitigation: limit the behavior change to section-prefix matching;
  do not alter route parsing, body normalization, rendered output, or `Tasks` exclusion.
- Bare `#` behavior changes for notes with top-level H1 metadata headings. This is intentional under the "H1 only if no
  matching non-H1" rule, and should be documented and tested.

## Success Criteria

- `#r` and `#R` select the same section.
- A matching non-H1 heading wins over an earlier matching H1 heading.
- A matching H1 heading is still used when it is the only matching heading.
- Existing task capture behavior remains unchanged.
- Tests, docs, and help text reflect the new bullet placement rule.
