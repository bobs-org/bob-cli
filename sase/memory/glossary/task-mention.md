---
keyword: Task Mention
aliases:
  - mention
  - "text mention"
---

An inert, derived back-reference from one task's own text to another, enabled by default. When
task S's title, a note under it, or a Cancel / Schedule / Work / Rotten Log
entry contains a block [[task-link]] to task T (`[[note#^id]]`, including
embeds and struck links), Bob stores `[[S]] (💬 mentions)` on T's generated
🪢 **LINKED FROM:** line. A mention is not a [[task-relation]]: it has no
label and no gestures, it is never a closing label, and it never enters the
label catalog. Nothing is written on S except a silent `^id`. A dependency
or an authored relation on the pair wins. See `docs/task-relations.md` §19.
