---
create_time: 2026-06-20 12:48:32
status: wip
prompt: sdd/prompts/202606/update_github_org_refs.md
---
# Plan: Update Bob GitHub Organization References

## Goal

Update live `bob-cli` and `bob-plugins` references from Bryan's old personal GitHub owner (`bbugyi200`) to the new
`bobs-org` organization after the repository migration. Keep behavior, docs, and local defaults consistent with the new
canonical remotes:

- `git@github.com:bobs-org/bob-cli.git`
- `git@github.com:bobs-org/bob-plugins.git`

The change should be narrow: organization/owner references only, with no CLI behavior redesign.

## Context Verified

- Current `bob-cli` workspace remote already points to `git@github.com:bobs-org/bob-cli.git`.
- Current `bob-plugins` workspace remote already points to `git@github.com:bobs-org/bob-plugins.git`.
- Canonical local checkouts exist at:
  - `~/projects/github/bobs-org/bob-cli`
  - `~/projects/github/bobs-org/bob-plugins`
- Old canonical local checkouts do not exist at:
  - `~/projects/github/bbugyi200/bob-cli`
  - `~/projects/github/bbugyi200/bob-plugins`
- The closed `bob-cli-8` epic was the prior migration from vault plugin folders into `bbugyi200/bob-plugins`; it is
  useful historical context, but the current desired owner is now `bobs-org`.

## Scope

### `bob-cli`

Update active source, package metadata, configuration, and user-facing docs that still point at `bbugyi200`:

- `Cargo.toml`
  - Change `repository` to `https://github.com/bobs-org/bob-cli`.
- `sase.yml`
  - Change the `bob-plugins` sibling path from `~/projects/github/bbugyi200/bob-plugins` to
    `~/projects/github/bobs-org/bob-plugins`.
- `src/native/env.rs`
  - Change the default `BOB_PLUGINS_DIR` fallback to `~/projects/github/bobs-org/bob-plugins`.
- `src/native/plugins.rs`
  - Update help text and examples that mention the old default plugin repo path.
- `README.md`
  - Update the remote install command for `bob-cli`.
  - Update the `bob-plugins` GitHub link and default local path.
- `docs/plugins.md`
  - Update the `bob-plugins` GitHub link, default local path, example header, JSON example, and command examples.
- `docs/highlights-ref-sync.md`
  - Update the `bob-cli` clone command to use `bobs-org`.

Also review `sdd/tales/202606/merge_bob_v1_into_bob.md`, which contains a user-facing `bob-cli` GitHub link. If it is
still a reusable operational tale rather than immutable history, update that link to `bobs-org`. Keep the change limited
to the link.

### `bob-plugins`

Update the active README link back to `bob-cli`:

- `README.md`
  - Change `https://github.com/bbugyi200/bob-cli` to `https://github.com/bobs-org/bob-cli`.

No plugin code should change.

## Explicit Non-Goals

- Do not modify memory files without explicit approval.
- Do not hand-edit SASE bead event streams or generated bead projections:
  - `sdd/beads/events/**`
  - `sdd/beads/issues.jsonl`
- Do not rewrite historical prompts or research artifacts merely because they mention the old owner. Those are records
  of what was true or asked at the time. If a historical document is also reused as current operational documentation,
  update only the live instruction/link portion.
- Do not change remotes; they are already correct.
- Do not alter `bob plugins` command semantics, arguments, output schema, plugin sync behavior, or plugin files.

## Implementation Steps

1. Make the `bob-cli` reference updates listed above using mechanical replacements where the old owner is plainly part
   of the current GitHub URL or default local checkout path.
2. Make the `bob-plugins` README link update.
3. Re-scan both workspaces for `bbugyi200`, `bobs-org`, `github.com/bbugyi200`, and `projects/github/bbugyi200`.
4. Triage remaining `bbugyi200` hits into:
   - acceptable historical/audit records, or
   - missed live references that need another narrow edit.
5. Run formatting only if a source file's formatting changed in a meaningful way; these edits should not need broad
   formatting.
6. Run targeted validation:
   - `cargo test` or the repo's standard test command if available and reasonably quick.
   - `cargo run -- plugins --help`
   - `cargo run -- plugins list --help`
   - `sase init` or a read-only/interactive-safe equivalent to verify the updated sibling path resolves cleanly.
   - In `bob-plugins`, run `npm run validate` because the repo has a validation script and the check is cheap.

## Verification Criteria

- Live `bob-cli` code, package metadata, SASE config, and docs point to `bobs-org`.
- Live `bob-plugins` README points to `bobs-org/bob-cli`.
- `bob plugins --help` and `bob plugins list --help` show the new default path.
- The default plugin repo path resolves to `~/projects/github/bobs-org/bob-plugins`.
- `sase init` no longer references or warns about the old `~/projects/github/bbugyi200/bob-plugins` sibling path.
- Remaining `bbugyi200` references are limited to intentionally historical SASE records, prompts, research, or memory
  files that were not approved for modification.

## Risks and Mitigations

- **Historical records vs. live docs:** Avoid rewriting audit/event history. Use targeted file triage rather than a
  blind repository-wide replacement.
- **Local path assumptions:** The verified canonical local path is `~/projects/github/bobs-org/...`; keep it overridable
  through `BOB_PLUGINS_DIR` and `-r/--repo`.
- **Cross-repo consistency:** Apply and verify `bob-cli` and `bob-plugins` together so docs point at each other using
  the same organization owner.
