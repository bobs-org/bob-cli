---
keyword: Bob Mac Capture Is A Thin Client Of bob
aliases:
  - thin client
  - mac capture grammar
  - swift grammar port
summary:
  Bob Mac Capture never parses capture grammar, computes previews, or writes the vault;
  it runs bob, renders the spans, candidates, and previews bob returns, and submits each
  draft as one bob capture call.
metadata:
  status: accepted
  decided: 2026-08-13
---

**Applies to.** Bob Mac Capture, bob-cli.

**Claim.** bob-cli is the only implementation of capture grammar, completion data,
preview, and vault mutation. Bob Mac Capture owns presentation, process orchestration,
the global hotkey, settings, launch at login, and packaging. It spawns `bob` directly —
`capture-parse` for semantic spans, `capture-complete` for candidates and replacement
ranges, `capture --dry-run` for previews — and renders what comes back. It may filter or
rank what `bob` returned, but it never decides syntax itself; `BlockIDRules.swift` puts
it as "Bob is the only authority for the grammar". A draft is submitted as one aggregate
`bob capture`, and the panel hides only after `bob` reports success for the whole draft.
New capture behavior lands in bob-cli and `docs/capture.md` first; the app then decodes
and presents it.

**Why.** The app replaced a Hammerspoon pop-up whose `task_capture.lua` (353 lines) was
a second, independent implementation of the capture grammar in Lua. The replacement
research named that duplication — not the widget — as the root cause of the missing
completion and highlighting, and noted that any client wanting highlighting would need a
third copy. Rejected alternatives:

- **Re-implementing the grammar in Swift** — "the current pain amplified".
- **Linking bob-cli as a `staticlib` over a C ABI** — zero spawn cost, but it needs a
  stable C header and `aarch64-apple-darwin` cross-compilation.
- **A resident daemon or FFI** — judged premature: a
  `bob capture --dry-run --format json` spawn measured about 5 ms, while the old pop-up
  paid about 95 ms per stage for a login shell (both measured on athena, not the Mac).

JSON endpoints also serve any later client (a Raycast extension, an iOS Shortcut, zsh
completion) for free. Epic `bob-cli-2o` followed the rule: its bob-cli phases shipped
`plan_budget`, destination roles, the `=x` drop outcome, and `now_tag` spans, and its
two Mac phases decoded and presented them. Evidence:
`research:202608/bob_mac_capture_replacement/bob_mac_capture_replacement.md` §§2.2, 2.3,
4.2, 4.3; the Mac `README.md` opening paragraph (since `9030832`, 2026-08-13).

**Cost.** Every capture feature lands twice, bob-cli first, and the Mac half is gated by
macOS CI because agent hosts have no Swift toolchain. Every preview is a subprocess,
bounded by a 20-second timeout (`BobProcessClient.defaultTimeout`). `bob` and the app
are installed independently, so the JSON contract must grow additively: the app decodes
new fields with `decodeIfPresent` so an older `bob` still works, yet it rejects a
`schema_version` other than the one it expects.

**Reopens when.** Spawn latency measured on the Mac breaks live preview, or a second
frontend needs an in-process library. Even then exactly one semantic implementation must
remain: reopening can change the transport (a daemon, FFI), never add a second grammar.
