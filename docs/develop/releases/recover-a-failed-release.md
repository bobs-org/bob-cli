---
title: Recover a failed release
description: Diagnose CI, push, publish, release-record, and daily-guard failures.
---

# Recover a failed release

Use the symptom from the Actions run to decide what to repair. The guard and workflow
fail closed when release data cannot be checked.

## Before you start

Open the failed release.yml run and note its mode, SHA, guard verdict, and completed
jobs. Do not move a version tag or edit Cargo.toml's version or CHANGELOG.md by hand;
release-plz owns those files.

## CI is red

Nothing is released while full CI is red. Fix the failure on master, then let the next
schedule slot run or dispatch a dry run to inspect the new SHA. The train will run CI
again for that exact revision.

## The version-bump push was rejected

The train pushes only as a fast-forward. A rejected push means master moved after CI
selected its SHA, so nothing was released. The next schedule slot plans from current
master and runs the full gate again. Do not force the bump onto master.

## The bump was pushed, but release-plz failed

Inspect the release job and tag state. release-plz release runs even when the update
step produced no new bump, so a later run can finish a release left pending by a
failure. In rehearsal, retry only after confirming the GitHub Release data source will
allow the run. The rehearsal guard counts GitHub Releases, not tags; a tag without its
Release is not counted as a completed release.

In live mode, check crates.io before retrying. A version may have published even when
the later tag or Release step failed.

## The crate published, but its tag or GitHub Release is missing

Treat the crates.io publication as permanent. Create the missing vX.Y.Z tag and GitHub
Release by hand at the commit that contains that version. Do not ask the train to
publish the version again: live mode reads crates.io pubtime, so the same-day guard
blocks a second publish.

## The guard lookup failed

The workflow fails closed when it cannot read the active mode's release data. For
rehearsal, check GitHub CLI authentication and Release visibility. For live mode, check
the crates.io sparse-index request and network access. Retry only after the data source
is readable; a hotfix override does not bypass lookup failure.

## If it goes wrong

If the run's mode or SHA is unclear, compare the run summary with the
[release train reference](reference.md) before retrying. For an urgent second release
on the same New York date, use [Ship a hotfix](ship-a-hotfix.md).

## Related

- [Pause and resume releases](pause-and-resume-releases.md)
- [Withdraw a bad release](withdraw-a-bad-release.md)
- [Release train reference](reference.md)
