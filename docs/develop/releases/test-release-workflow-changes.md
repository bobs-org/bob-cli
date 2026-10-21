---
title: Test release workflow changes
description: Exercise CI and release workflow edits on disposable ci/** scratch branches.
---

# Test release workflow changes

Use scratch branches to exercise workflow changes before they reach master. Release
scratch branches are forced to dry-run mode.

## Before you start

You need permission to push and delete branches on origin and to inspect GitHub Actions
runs. Start from a clean checkout containing the workflow changes.

## 1. Exercise CI changes

Create and push a ci/** branch:

~~~bash
git switch -c ci/check-workflow-change
git push -u origin HEAD
~~~

The ci.yml push trigger runs lint, Linux and macOS tests, and package checking. A branch
whose name starts with ci/full also enables the extra Linux test tools.

## 2. Exercise release workflow changes

Create and push a ci/release/** branch:

~~~bash
git switch -c ci/release/check-workflow-change
git push -u origin HEAD
~~~

The release workflow treats every push in this branch family as a forced dry run. It
can exercise the guard, full CI, and preview without pushing a version bump, tag, GitHub
Release, or crates.io version.

## 3. Review and remove the scratch branch

Read the run summary and each job result. Once finished, delete the remote scratch
branch:

~~~bash
git push origin --delete ci/release/check-workflow-change
~~~

Use the branch name you created. Delete a ci/** branch the same way after its CI run.

## If it goes wrong

GitHub only dispatches workflow files present on the default branch. Use the push
trigger for a new or changed release workflow; a manual dispatch cannot test a workflow
definition that GitHub has not registered from master.

## Related

- [bob-cli CI](../ci.md)
- [Release train reference](reference.md)
