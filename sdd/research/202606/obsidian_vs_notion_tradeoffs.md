---
create_time: 2026-06-12
status: research
topic: Obsidian vs Notion as primary note-taking app
---
# Research: Obsidian vs Notion as Primary Note-Taking App

## Question

Did Bryan make the right choice picking Obsidian over Notion as his primary
note-taking app? Compare and contrast the two tools as of mid-2026 across data
ownership, offline support, sync/pricing, extensibility, querying/databases,
collaboration, AI features, and security/privacy.

## Short Answer

**Yes — Obsidian was the right choice, and the case has gotten stronger, not
weaker, since the choice was made.**

Every dimension Bryan's workflow actually depends on — local plain-text
Markdown ownership, git-friendliness, CLI automation against the vault, full
offline operation, and zero-knowledge end-to-end-encrypted sync at roughly half
Notion's price — favors Obsidian, with each supporting claim surviving 3-0
adversarial verification against primary sources. Notion's genuine advantages
(native relational databases, real-time collaboration, built-in AI agents) map
to team/workspace use cases, not a solo, automation-heavy personal vault.

Critically, Obsidian's 2026 trajectory has moved *toward* Bryan's needs: an
official first-party CLI (GA Feb 2026), headless server-side Sync (open beta,
May 2026), first-party structured data via Bases, two independent security
audits, and a dropped commercial-license requirement. Notion's structural
properties — proprietary block format, server-resident data, employee-accessible
content, selective and partially paywalled offline mode, lossy export — are
disqualifying for a git/CLI-centric primary vault regardless of its strengths
elsewhere.

## Method

Deep-research workflow run on 2026-06-12: question decomposed into 5 search
angles (broad comparison, developer extensibility, data ownership/lock-in,
security/sync/pricing, contrarian/recent developments); 22 sources fetched
(including live fetches of both vendors' pricing/security/help pages); 107
claims extracted; the top 25 put through 3-vote adversarial verification
(2/3 refutes kill a claim). 22 claims confirmed, 3 killed.

## Local Context

- `~/bob/` is Bryan's Obsidian vault: plain-text Markdown, stored locally,
  git-friendly, with CLI automation (`bob`, `ob`) and heavy Dataview usage.
- Prior related research: [obsidian_to_logseq_tradeoffs](obsidian_to_logseq_tradeoffs.md)
  (2026-06-03) reached a similar "stay on Obsidian" conclusion vs Logseq.

## Verified Findings

### 1. Data ownership and file format — Obsidian, decisively

Obsidian stores notes as plain-text Markdown files locally on your device —
readable by any text editor indefinitely and inaccessible to the vendor
("your data is stored locally on your device, making it inaccessible to us" —
[obsidian.md/pricing](https://obsidian.md/pricing)). Notion's canonical data
store is a proprietary block format on Notion-managed AWS servers; its 2025+
offline mode is a local SQLite cache of selected pages, not user-readable
files. *(3-0 verified; note one stricter variant claiming "CommonMark"
specifically was refuted — the verified claim is plain Markdown, not a
specific spec.)*

### 2. Notion portability is lossy — concrete lock-in risk

Notion's own Markdown export omits data: full-page databases flatten to CSV
(losing views, filters, relations, rollups, formulas) and callouts/toggles/
equations degrade. Obsidian's official import docs go as far as recommending
**against** using Notion's Markdown export at all
([help.obsidian.md/import/notion](https://help.obsidian.md/import/notion)).
Notion shipped a Markdown Content API in Feb 2026 that improves programmatic
access, but database semantics still don't survive export faithfully. *(3-0)*

An officially maintained escape hatch exists in the other direction: the
first-party Obsidian Importer plugin (v1.8.12, May 2026) offers API-based
import that preserves Notion databases and formulas as Obsidian Bases. *(3-0)*

### 3. Offline support — Obsidian by construction

Obsidian is fully offline because the entire vault is local files. Notion's
offline mode (launched Aug 2025) is selective and partially paywalled: free
users must manually download individual pages; only paid plans auto-download
Recents/Favorites (~top 20 each); only the first ~50 rows of a database's
first view sync; subpages don't auto-download; and gaps (embeds, AI blocks,
forms, buttons) were still documented as of Feb 2026. *(3-0 on all three
merged claims)*

### 4. Sync and pricing — Obsidian costs half or less

| | Obsidian | Notion |
|---|---|---|
| Core app | Free without limits, no account required | Free tier ($0), account required |
| Paid sync | Sync $4/mo annual ($5 monthly); Sync Plus $8/$10 | Included in plans: Plus $10/member/mo, Business $20 (annual rates) |
| Free sync alternatives | git, Syncthing, iCloud — $0 | None (cloud-only) |
| Solo worst case | $48–60/yr | $120/yr (Plus) |

Verified against live pricing pages on 2026-06-12. Obsidian also dropped its
commercial-license requirement. *(3-0 on all four merged claims)*

### 5. Extensibility and automation — Obsidian's 2026 investments target exactly this workflow

- **Official first-party CLI** (early access Feb 10, 2026 in v1.12.0; GA Feb
  27, 2026 in v1.12.4): "Anything you can do in Obsidian you can do from the
  command line" — programmatic read/search/write (`read`, `search`, `create`,
  `append`, `property:set`, `daily:append`, …), explicitly designed for cron
  jobs and shell scripts ([obsidian.md/cli](https://obsidian.md/cli)). Caveat:
  it remote-controls the running desktop app rather than running headless.
  *(3-0)*
- **Obsidian Headless** (open beta, v0.0.10, May 31, 2026): runs Obsidian Sync
  without a GUI on any server, with the same E2E encryption as the desktop app
  (`ob sync`, `ob sync --continuous` via the `obsidian-headless` npm package).
  Requires an active Sync subscription and Node.js 22+. *(3-0)*
- Notion offers a REST API but no equivalent local-file automation surface.

### 6. Security and privacy — structural advantage to Obsidian

**Obsidian Sync**: E2E encryption by default (AES-256-GCM, scrypt key
derivation), zero-knowledge in practice — the password is never stored and
neither staff nor eavesdroppers can read vault contents. Validated by two
independent audits (Cure53, Oct 2024; Trail of Bits, Dec 2025); the one
high-severity finding (weak randomness) was remediated and auditor-validated
by May 2026. Caveats: a non-default "standard encryption" mode exists where
Obsidian holds keys, and some metadata (device events, timestamps,
deterministic file-hash equality) is not E2E encrypted. *(3-0)*

**Notion**: server-side encryption only (AES-256 at rest, TLS in transit) with
Notion-managed keys; no E2EE anywhere in its documentation or pricing tiers;
employees can technically access note content (restricted by policy, not
cryptography); hosted exclusively on AWS with no self-hosting option at any
tier. Notion deliberately avoids E2EE to preserve collaboration, search, and
content recovery. *(3-0 on all four merged claims)*

### 7. AI features — Notion clearly ahead natively

Notion 3.0 (Sept 18, 2025) shipped built-in AI Agents that autonomously
execute multi-step work in the workspace. Obsidian core ships no native AI;
AI arrives via community plugins (Copilot, Smart Connections) or external
agents over the plain-text vault (e.g., Claude Code with Obsidian Skills) — an
architecture an automation-heavy user may actually prefer. Caveats: Notion's
"20+ minutes of multi-step actions" figure is vendor marketing, and 2026
reviews report hallucinations, agent reliability issues, and a documented
prompt-injection risk. *(3-0)*

### 8. Querying/databases — Notion's strongest card, but weakening (medium confidence)

Notion's relational database model (relations, rollups, views, formulas)
remains its strongest differentiator on paper. Obsidian now has first-party
**Bases** for structured data (the official importer converts Notion databases
into Bases), supplementing Dataview. No verified head-to-head feature
comparison survived verification, so parity is an open question — but note
that database structure is precisely what is *lost* when leaving Notion
(CSV-only export), making it lock-in as much as feature.

### 9. Collaboration — Notion wins, but it barely matters here (medium confidence)

Real-time multi-user editing is core to Notion's cloud architecture (and the
reason it forgoes E2EE). Obsidian's story is thinner: shared vaults exist via
Sync, but should be treated as file-level vault sharing, not real-time
co-editing. This was the weakest-evidenced dimension in the research, and for
a *personal* note-taking app it carries low weight.

## Refuted Claims (excluded from findings)

- "Obsidian Sync lists collaboration on shared vaults as a plan feature" —
  refuted 1-2.
- "Notion had no offline capability at all before Aug 2025" — refuted 1-2
  (Aug 2025 launched the *current* offline mode, not an absolute first).
- "Obsidian stores notes as CommonMark specifically" — refuted 0-3 (plain
  Markdown, no specific spec).

## Caveats and Coverage Gaps

- All pricing/feature facts verified against live pages on 2026-06-12; both
  vendors iterate quickly. Obsidian Headless is explicitly open beta and the
  CLI only went GA in Feb 2026, so stability is unproven.
- No surviving verified claims for: mobile experience, performance benchmarks,
  a direct Dataview/Bases vs Notion databases comparison, or either company's
  long-term financial viability (Obsidian small/bootstrapped vs Notion
  VC-backed — neither characterization independently verified).
- Most findings rest on vendor primary sources — marketing-adjacent but
  factually concrete and independently corroborated. The lossy-export claim
  about Notion originates from a competitor's docs but is corroborated by
  Notion's own help center.

## Open Questions

1. How do Obsidian Bases and Dataview compare to Notion databases on
   relations, rollups, formulas, views, and query performance at scale?
2. Long-term viability: Obsidian's bus factor and Sync revenue sustainability
   vs Notion's VC-backed profitability/acquisition risk — and what happens to
   data if either folds?
3. How do the mobile apps compare in mid-2026 on performance, automation
   support, offline reliability, and sync conflict handling?
4. Does Obsidian Sync's shared-vault model support adequate occasional
   collaboration, or would real collaboration force a secondary tool?

## Recommendation

**Stay on Obsidian.** The choice was right when made and is more right now:
Obsidian's 2026 releases (official CLI, Headless Sync, Bases, dual security
audits) directly serve the Bob vault's local-first, git-driven, CLI-automated
workflow, while Notion's architecture is structurally incompatible with it —
cloud-only, proprietary format, no E2EE, lossy exit, paywalled offline.
Switching would forfeit the entire `bob`/`ob` automation stack, accept a lossy
migration, and roughly double annual cost, in exchange for database and
collaboration features that map to team use cases. If database or AI needs
grow, Bases, external AI agents over plain files, and the first-party Notion
importer provide adequate paths without ever migrating the primary vault.

## Key Sources

- [obsidian.md/pricing](https://obsidian.md/pricing) — pricing, local-storage claims (primary)
- [obsidian.md/cli](https://obsidian.md/cli) — official CLI capabilities (primary)
- [Obsidian Sync security docs](https://obsidian.md/help/sync/security) + Cure53/Trail of Bits audit reports (primary)
- [help.obsidian.md/import/notion](https://help.obsidian.md/import/notion) — Notion import/export fidelity (primary)
- [notion.com/pricing](https://www.notion.com/pricing) — pricing, offline-tier gating (primary)
- [notion.com/help/security-and-privacy](https://www.notion.com/help/security-and-privacy) — server-side-only encryption, employee access (primary)
- [Notion releases 2025-08-19](https://www.notion.com/releases/2025-08-19) (offline mode) and [2025-09-18](https://www.notion.com/releases/2025-09-18) (3.0 Agents) (primary)
- Secondary corroboration: G2, nesslabs.com, nicolevanderhoeven.com, xda-developers.com, dev.to, hamy.xyz (contrarian "moved back to Notion" perspective)
