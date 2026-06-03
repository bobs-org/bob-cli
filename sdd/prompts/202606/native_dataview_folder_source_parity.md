---
plan: sdd/tales/202606/native_dataview_folder_source_parity.md
---

#fork:bob-cli-4 It appears we do not have complete parity yet (see the output below). Can you help me diagnose the root cause of this issue and fix it? Verify that the below query now works with the native engine once you think you've fixed the issue. Think this through thoroughly and create a plan using your `/sase_plan` skill. Submit your plan with the `sase plan`
command (as the skill instructs) before making any file changes.

```
❮ bob dataview --format markdown --query '
LIST WITHOUT ID title + " (" + url + ")"
FROM "ref"
WHERE
  source_path AND url AND (
    parent = [[ai_ref]]
    OR parent.parent = [[ai_ref]]
    OR parent.parent.parent = [[ai_ref]]
    OR parent.parent.parent.parent = [[ai_ref]]
    OR parent.parent.parent.parent.parent = [[ai_ref]]
  )
SORT title
'
bob dataview: warning: ambiguous Dataview link target "foo"; matched 7 notes
bob dataview: warning: ambiguous Dataview link target "foo#bar"; matched 7 notes
bob dataview: warning: ambiguous Dataview link target "rap"; matched 5 notes
bob dataview: warning: ambiguous Dataview link target "bazbuz"; matched 2 notes
bob dataview: warning: ambiguous Dataview link target "plex"; matched 2 notes
bob dataview: warning: ambiguous Dataview link target "tag"; matched 2 notes
bob dataview: warning: ambiguous Dataview link target "book"; matched 2 notes
```