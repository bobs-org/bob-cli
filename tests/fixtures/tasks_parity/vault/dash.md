---
TQ_extra_instructions: |
  filter by function task.file.path !== query.file.path
  filter by function !task.scheduled.moment || task.scheduled.moment.isSameOrBefore(moment(), "day")
  filter by function !task.tags.includes("#hide")
  is not blocked
  folder does not include _templates
  sort by function task.file.path
  sort by function task.lineNumber
---

# Dashboard

- [/] #task Dashboard WIP [scheduled:: 2026-07-10]
- [*] #task Dashboard NEXT
- [ ] #task Dashboard READY

## WIP

```tasks
status.type is IN_PROGRESS
```

## NEXT

```tasks
status.name includes Next
```

## READY

```tasks
status.type is TODO
```

