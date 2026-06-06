---
create_time: 2026-06-06 10:04:09
status: done
prompt: sdd/prompts/202606/block_id_rename_cache_only.md
---
# Plan: Make `^`-trigger block-ID rename fast (cache-only candidate discovery)

## Summary

Renaming a block ID via the `^`/`^^` trigger in Obsidian is slow because, on **every** rename, the plugin reads **all
~5,796 vault notes (~11.5 MB) from disk, sequentially**, just to find which notes reference the old block ID. We will
drop that full-vault disk scan and discover reference files purely from Obsidian's metadata cache (backlinks +
resolvedLinks), while **always** including the destination note and the current/active note. This turns ~5,796 reads
into a handful and makes the rename effectively instant.

## Target & repo context (read first)

- **File to change:** `/home/bryan/bob/.obsidian/plugins/block-id-prompt/main.js` — the **"Block ID Prompt"** Obsidian
  plugin.
- This file lives in the **Bob vault git repo** (`/home/bryan/bob`), **not** in the `bob-cli` repo this agent is running
  from. The vault is a clean, git-tracked repo, so the change is reviewable/reversible there.
- There is **no TypeScript source and no build step** for this plugin: `main.js` _is_ the source of truth. Edit it
  directly, then reload it in Obsidian. (No `npm`, no bundler.)
- The slowness is **not** in `bob-cli` (the Rust tool); `bob-cli` is unrelated to this code path. The earlier assumption
  that this was a Rust/`collect_done.rs` issue is wrong.

## Root cause (precise)

`collectCandidateReferenceFiles(destinationFile, oldId, source)` (`main.js:1103`) builds the set of notes that might
reference the block. It already does the right, fast thing:

```
this.addCandidatePath(candidatePaths, destinationFile.path);   // 1105 — dest note
this.addCandidatePath(candidatePaths, source.sourcePath);      // 1106 — current/active note
this.addBacklinkCandidatePaths(candidatePaths, destinationFile);     // 1107 — metadata cache backlinks
this.addResolvedLinkCandidatePaths(candidatePaths, destinationFile); // 1108 — metadata cache resolvedLinks
await this.addFallbackCandidatePaths(candidatePaths, oldId, source); // 1109 — *** THE SLOW PART ***
```

`addFallbackCandidatePaths` (`main.js:1168-1193`) is the completeness safety net and the sole source of the slowness:

```
const files = this.app.vault.getMarkdownFiles();   // every note in the vault
const needle = `^${oldId}`;
for (const file of files) {
  if (candidatePaths.has(file.path)) continue;
  const content = await this.readFileSnapshot(file, source); // app.vault.read(file) — disk I/O
  if (content !== null && content.includes(needle)) {
    this.addCandidatePath(candidatePaths, file.path);
  }
}
```

It reads every not-already-a-candidate note from disk and substring-checks it. On a ~5,796-note vault that is thousands
of sequential `app.vault.read` calls per rename.

## Goal

- Rename latency on the large vault drops from seconds to ~instant.
- No regression in the rename's existing correctness/safety guarantees.
- Explicitly guarantee the **current/active note** is always considered (the user frequently renames a block ID
  immediately after Obsidian auto-creates it when linking to an unnamed block — at that instant Obsidian may not have
  re-indexed the active note yet).

## Approach: cache-only + always include the current file

Discover candidate reference files from Obsidian's metadata cache only, dropping the full-vault disk scan. Concretely:

1. **Delete the slow scan.**
   - Remove the call at `main.js:1109` (`await this.addFallbackCandidatePaths(...)`).
   - Delete the now-unused method `addFallbackCandidatePaths` (`main.js:1168-1193`).
   - `app.vault.getMarkdownFiles()` was used only here, so the only remaining O(vault) work is
     `addResolvedLinkCandidatePaths`' in-memory iteration of `resolvedLinks` (microseconds, no disk I/O) — leave it as a
     robustness backstop to backlinks.

2. **Make the current-file guarantee explicit and durable.**
   - `main.js:1106` already adds `source.sourcePath` unconditionally, independent of the metadata cache — so the
     requirement is already met. Add a short inline comment there stating _why_ it must never be removed (covers the
     just-auto-created, not-yet-indexed active note), so a future edit doesn't drop it.
   - Note that the active note is read from the **live editor buffer** (`source.editor.getValue()` in
     `readFileSnapshot`/`readDestinationForValidation`, `main.js:1196-1201` / `main.js:1061-1066`), not from disk — so
     even unsaved edits to the active note are handled correctly. No change needed; call it out so it's preserved.

3. **Tidy the signature.**
   - After removing the fallback, `oldId` is unused in `collectCandidateReferenceFiles`. Drop the parameter and update
     the call site (`main.js:926`) from `this.collectCandidateReferenceFiles(destination.file, source.oldId, source)` to
     `this.collectCandidateReferenceFiles(destination.file, source)`. Keep `source` (still used for
     `source.sourcePath`).

No other code changes. Parsing, edit-planning, duplicate/exactly-once validation, unchanged-before-write verification,
and the actual rewrites are untouched.

## Why cache-only is safe (completeness analysis)

The downstream planner `buildReferenceRewritePlan` (`main.js:925`) re-reads each candidate, re-parses its links, and
only rewrites references whose link **resolves to the destination note** and whose `reference.raw` still matches — and
`applyReferenceRewritePlan` re-verifies every file is unchanged before writing. So narrowing the candidate set never
weakens those checks; it only changes _which_ files we look at. The candidate set after the change:

- **Destination note** — always added (`:1105`). Covers the block definition itself and any self-references (`[[#^id]]`
  / `[[dest#^id]]`) inside the destination.
- **Current/active note** — always added (`:1106`), from the live editor buffer. Covers the just-created link in the
  user's primary workflow even if unindexed.
- **All other referencing notes** — from `getBacklinksForFile` + `resolvedLinks`. This is the _precise_ set of resolved
  links to the destination, and is actually **more accurate** than the old text scan: the scan matched any note
  literally containing `^oldId` and then discarded those not resolving to the destination, so it both did extra disk
  reads and could surface unrelated notes that reuse the same block-id string for a _different_ file. The cache set is
  already scoped to the destination.

These three sources cover every case the old scan covered, except the residual risk below.

## Accepted residual risk (explicitly chosen)

A note that references the block but was edited **externally** (e.g., by `bob-cli`) and not yet re-indexed by Obsidian's
metadata cache could be missed by a rename until Obsidian re-indexes it. This is the small, accepted trade-off of the
cache-only approach. It is mitigated by: (a) Obsidian re-indexes on file change, so the stale window is brief; and (b)
the two most common cases — the active note and the destination note — are always covered regardless of cache freshness.
No code change; documented here and in a brief code comment.

## Verification (manual — no automated test harness exists for this plugin)

Reload the plugin (toggle it off/on in Obsidian Community Plugins, or reload the app), then exercise the flows on the
real vault:

1. **Primary workflow** — link to an unnamed block so Obsidian auto-creates an id, then immediately trigger the `^`/`^^`
   rename. Confirm: the active note's link and the destination block both update to the new id; latency is ~instant.
2. **Multi-reference** — a block id referenced from several _other_ notes. Confirm every backlinked note's reference
   updates (compare the updated set against expectations once, to confirm backlinks/resolvedLinks caught them all).
3. **Duplicate guard** — rename to an id that already exists in the destination. Confirm it is still blocked with the
   existing Notice.
4. **Cancel path** — open the modal and cancel. Confirm the original link is restored (`cancelBlockIdPrompt` path is
   unaffected).
5. **Speed** — confirm wall-clock latency dropped from seconds to near-instant on the ~5,796-note vault (optionally wrap
   candidate collection in `console.time` during testing, then remove).

## Out of scope (keep the change surgical)

- No settings/toggle for the scan — the plugin has no settings infrastructure and the user chose cache-only outright.
- No changes to link parsing, rewrite, or validation logic.
- No rewrite of the `resolvedLinks` iteration (cheap, in-memory; kept as a backstop).
- No work in the `bob-cli` Rust repo — unrelated to this code path.

## Alternatives considered (and why rejected)

- **Cache-first + bounded-parallel safety scan** and **parallelize the scan only**: both retain O(vault) disk I/O per
  rename. The user chose maximum speed (cache-only) with the explicit current-file guarantee, which the existing code
  structure already supports.
