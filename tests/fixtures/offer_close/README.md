# Offer-close vectors

`vectors.json` is the shared contract for offer Done/Cancel behavior in bob-cli
and bob-plugins (`docs/task-dependencies.md` §12.22–§12.23). Bob-cli owns this
file; bob-plugins copies it byte-for-byte. The Rust engine cases are SV1–SV10
plus SV17–SV24 (new-log Cancel, plural scope and recovery, WRITE skips, joint
recovery, and row-specific tickler attribution). SV11–SV13 pin capture selection and stale-batch behavior,
while SV14–SV16 pin Obsidian and review-walk behavior. Surface-only cases are
marked `ui_only`.

The vectors pin exact source-note bytes, the pre-write offer identity, root-only
scope, eligibility reasons, Cancel Log behavior, recovered dependents, and the
one-round rule. A behavior change edits the contract and this fixture together.
