---
title: 'Snapshot: "contract" {draft}'
bob-snapshot: true
bob-kicker: NOTE
bob-display-title: Snapshot Contract
bob-dek: 'A \"quoted\" dek\: \{inputs\} \& checks\.'
bob-meta:
- Started Oct 15
- '[Requirements](<#requirements>)'
bob-stats:
- status: open
  count: 2
- status: next
  count: 1
- status: wip
  count: 1
- status: blocked
  count: 1
- status: done
  count: 1
- status: canceled
  count: 1
bob-omitted: 8 closed not shown
bob-snapshot-line: Snapshot Oct 16 · 8 shown
bob-footer: Generated for the contract tests\.
bob-glyph-table: glyphs.lua
bob-return-links: true
---

# Requirements {#requirements}

Punctuation\: \[square\]\, \_marks\_\, \$69\.00\, \:sparkles\:\, and \@ROUTE\; Unicode\: café — naïve\. *emphasis* **strong** ``a `code` sample`` [\[\[due\:\: soon\]\]]{.muted} [inline tag]{.chip}  
after a hard break

~~struck **bold **~~[visible link](<#t-next>)~~ and ~~omitted item~~ tail~~

## Next \& In Progress {#next-lane .lane}

### Heading level three {#heading-level-three}

- []{.st .st-open} Open task [sched Oct 17]{.chip} [high]{.chip .chip-accent} [Keep](<https://example.test/keep?x=1&y=2>){.chip}

- []{.st .st-next} []{#t-next} Next task
  - Nested child
    2. Second level

- []{.st .st-wip} Work in progress

- []{.st .st-blocked} []{#t-intro-proposal} Blocked task
  - Waits on [Task One](<https://example.test/tasks/one>) · Plan

  - Prerequisite notes

- []{.st .st-done} Done task

- []{.st .st-canceled} Canceled task

- [Worked]{.label} *Oct 15* — Shipped a fix [+2 older]{.muted}

``` rust
fn main() {
    println!("literal [[link]]");
}
```

![diagram \& notes](<assets/3.png>){width=62%}

[Missing image · img\/x\.png]{.missing}

::: {.embed source="Snapshot &quot;quoted&quot; &amp; scope › ref"}
Embedded prose\.
:::

::: {.livequery}
[Open live query](<https://example.test/query>)
:::

::: {.callout kind="note" title="A &quot;note&quot; {title}"}
Callout body\.
:::

::: {.compact}
- []{.st .st-done} Compact closed row
:::

[No open tasks · 8 closed not shown]{.muted}

# Closed tasks {#closed .appendix}

::: {.compact}
- []{.st .st-canceled} []{#t-closed-row} Closed appendix row
:::
