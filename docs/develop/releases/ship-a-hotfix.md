---
title: Ship a hotfix
description: Request a same-day release by dispatching the release workflow with a written reason.
---

# Ship a hotfix

Use a written override reason when a user-visible fix must ship after today's release.
The override skips only the once-per-day ceiling; CI and the other guard checks still
apply.

## Before you start

The release train must be armed, the fix must be on master, and CI must be able to pass
for the exact master SHA. You need permission to dispatch the workflow.

## 1. Dispatch with a reason

~~~bash
gh workflow run release.yml --repo bobs-org/bob-cli --ref master -f dry_run=false -f override_reason='Corrects the released task-loss regression'
~~~

Use a specific reason that explains why a second release is needed. The workflow still
runs full CI on the selected SHA before release-plz updates the version and changelog.

## 2. Follow the run

Open the Actions run and check the guard summary, CI jobs, release job, and any binary
uploads. A successful override summary identifies the reason. The next normal run is
still subject to the daily guard and the latest release time.

## If it goes wrong

An override does not bypass an unset or paused RELEASES_PAUSED variable, a tagged SHA,
a non-master real release, or a failed release-data lookup. Resolve that condition
first. If the bad version needs withdrawing, follow
[Withdraw a bad release](withdraw-a-bad-release.md).

## Related

- [Pause and resume releases](pause-and-resume-releases.md)
- [Recover a failed release](recover-a-failed-release.md)
- [Release train reference](reference.md)
