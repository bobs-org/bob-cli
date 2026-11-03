---
keyword: Buffer Reasons Ride The Description Bullets
summary:
  "A -N reason / +M reason line sets that side's buffer and writes one labelled
  reason bullet at the top of the Google-owned description block."
metadata:
  status: accepted
  decided: 2026-11-03
---

# Buffer Reasons Ride The Description Bullets

**Applies to.** bob-cli and the vault.

**Claim.** A `-N reason` / `+M reason` line sets that side's buffer like a
bare directive and writes one `↤ *Nm before* · …` /
`↦ *Mm after* · …` bullet at the top of the Google-owned description
block; the first value per side wins, a reason attaches when its line sets
or repeats that value, and a disagreeing line stays an ordinary bullet;
reasons share the block's ownership, journal record, and kill switch, and
never count toward the cap.

**Why.** One Google-owned block keeps a single rule: description bullets
follow Google until you edit them. A second tracked block would mean two
ownership rules, a journal schema change, and two adjacent windows to find.
`↤`/`↦` match the hatched lead-in and lead-out wings that Bob Ledger Tools
paints around the time pill; `⇥` already means Tab across Bob's pickers and
footers. The italic `*Nm before*` / `*Nm after*` label uses the existing
pill tooltip words and echoes the `-30` Bryan typed; clock-time labels
would go stale in a hand-edited block when the meeting moves. First value
per side wins preserves the accepted buffer rule and the existing
directive handling.

Rejected alternatives:

- **A separate tracked reason block with its own journal record.** Two
  ownership rules, a journal schema change, and two adjacent windows.
- **The reason inside the field (`[buffer:: -30m "Drive"]`).** Parsed by
  Rust, JS, and Swift, the row gets long, and the reason would be organizer
  prose on the task line.
- **Reasons on the MEET Pomodoro entry.** The ledger records sessions, not
  event logistics, and the link moves between entries by phase.
- **Summing several lines per side.** Contradicts the accepted first value
  per side wins rule.
- **`⇤`/`⇥` glyphs.** `⇥` already means Tab across Bob's pickers and
  footers.
- **Clock-time labels (`↤ *11:30* · …`).** Go stale in a hand-edited block
  and no longer echo the `-30` Bryan typed.

**Cost.** An edited block freezes reasons too. `+1 to that` in someone
else's invite becomes a 1-minute after-buffer with a reason.

**Reopens when.** Reasons must survive hand-edited blocks, or others'
prose trips the grammar.

Evidence: `plan:202611/gcal_buffer_reasons.md`; `docs/gcal.md` § Buffer
reasons; `tests/fixtures/gcal_description/outline.json`.

[[decisions:event-buffers-widen-the-span]]

[[decisions:bob-gcal-is-read-only-and-proposes-once]]
