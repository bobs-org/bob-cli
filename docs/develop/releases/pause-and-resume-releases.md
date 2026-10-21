---
title: Pause and resume releases
description: Stop or arm the scheduled release train with its GitHub repository variable.
---

# Pause and resume releases

Use the RELEASES_PAUSED repository variable to control whether the release guard may
proceed.

## Before you start

You need write access to the bobs-org/bob-cli repository and the GitHub CLI. The release
workflow expects a GitHub environment named release. Create that environment and the
repository variable before arming the train.

## 1. Pause the train

Set the repository variable to true:

~~~bash
gh variable set RELEASES_PAUSED --repo bobs-org/bob-cli --body true
~~~

The scheduled workflow may still start, but the guard reports paused and no release
continues.

## 2. Check the setting

~~~bash
gh variable get RELEASES_PAUSED --repo bobs-org/bob-cli
~~~

Any value other than the exact lowercase string false keeps the train paused. If the
variable is absent or empty, the guard calls the state unarmed and fails closed.

## 3. Resume the train

Set the variable to the exact value false:

~~~bash
gh variable set RELEASES_PAUSED --repo bobs-org/bob-cli --body false
~~~

The next scheduled or manually dispatched run can proceed through the guard. Resuming
does not trigger a release by itself.

## If it goes wrong

If a run still reports paused, inspect the repository-level variable spelling and
value. The value is case-sensitive; FALSE, 0, no, whitespace, or an unset variable do
not arm the train. See the [daily guard reference](reference.md#daily-guard).

## Related

- [Ship a hotfix](ship-a-hotfix.md)
- [Release train reference](reference.md)
