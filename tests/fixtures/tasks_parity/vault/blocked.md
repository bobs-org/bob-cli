---
TQ_extra_instructions: |
  folder does not include _templates
  filter by function task.file.path !== query.file.path
  filter by function !task.tags.includes("#hide")
  group by path
  sort by function task.file.path
  sort by function task.lineNumber
  short mode
  hide toolbar
---

# Blocked

## BLOCKED Tasks

```tasks
(is blocked) OR (status.name includes Blocked)
```
