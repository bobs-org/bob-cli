---
title: How the release train works
description: Why bob-cli uses a scheduled, guarded release workflow and how it gates a release.
search:
  boost: 2
---

# How the release train works

The release train turns one eligible master revision into a versioned GitHub Release.
It runs in rehearsal mode today; a later configuration change switches the same
workflow to crates.io.

## A train day

~~~mermaid
flowchart LR
  schedule[Scheduled slot] --> guard[Daily guard]
  guard -->|GO| ci[Full CI on exact SHA]
  ci --> update[release-plz update]
  update --> push[Fast-forward version and changelog commit]
  push --> release[release-plz release]
  release --> binaries[Linux and macOS archives]
  guard -->|SKIP| done[Wait for next slot]
~~~

The workflow has two UTC schedule slots. The second is a retry if the first found a
failure or a push race; after a release, the daily guard makes it a no-op. A manual
dispatch can run the same workflow or request a dry run.

## Why this design

The release-plz quickstart publishes on every push, which does not impose a daily
ceiling. Replacing push with a cron still does not prevent a same-day retry from
publishing twice. A release pull request with a scheduled merge adds a persistent,
rapidly changing PR and a merge flow this repository does not use. An hourly guarded
publisher adds frequent runs without improving the release decision.

The research report also rejected cargo-semver-checks for this project because
src/lib.rs is an internal seam rather than the product contract. cargo-dist packages
and distributes binaries; it does not replace release-plz's version and changelog
work. See the [release-train decision record](../decisions.md).

## What once a day and green mean

The ceiling uses the calendar date in America/New_York, not a rolling 24-hour window.
The guard compares the most recent release date with today's New York date. A prior-day
release permits a run; a same-day release skips it unless a non-empty hotfix reason is
provided.

Green means the reusable CI workflow passed in full for the exact SHA selected by the
guard. The release workflow passes that SHA to CI and checks Linux and macOS. It does
not accept an earlier green master run as evidence for a newer revision.

The rehearsal guard uses the newest GitHub Release creation time. Live mode uses the
crates.io sparse index publication time, including yanked versions. The registry
timestamp matters because a successful publish is permanent even if creating its tag
or GitHub Release later fails.

## The bump commit and the push race

After full CI passes, release-plz updates Cargo.toml and CHANGELOG.md. The workflow
commits those generated changes with the GitHub Actions bot identity and pushes with
fast-forward-only semantics. The commit contains version and changelog metadata; the
source revision that was checked remains the code being released.

An agent can push to master after CI starts. If that makes the release bump no longer a
fast-forward, Git rejects the push and the run ships nothing. The later schedule slot
then plans from the newer master revision and runs CI for that SHA.

## Rehearsal, live publishing, and Trusted Publishing

release-plz.toml currently sets git_only to true. Rehearsal creates Git tags and GitHub
Releases, but does not publish to crates.io. To go live, the configuration changes to
git_only = false. The guard then reads crates.io publication times instead of GitHub
Release times.

The live workflow grants the release job contents write and id-token write in the
release environment. release-plz performs the Trusted Publishing OIDC exchange; the
workflow does not store or pass a crates.io API token. Trusted Publishing must be
configured for owner bobs-org, repository bob-cli, workflow release.yml, and environment
release. The first crate publication must be done manually because Trusted Publishing
cannot create a new crate.

## One-way doors and operating costs

Once published, a crates.io version cannot be replaced or deleted. The package name
bob-obsidian is public and permanent, and the 10 MiB upload limit is a hard cliff.
Yanking a version does not undo its publication timestamp or reopen that day's guard.

Publishing is separate from deployment: Bryan's hosts and companion repositories track
bob-cli master, so a release may lag their source by up to a day. The train also has a
push race and waits for the full Linux and macOS gate, which takes about 20 minutes
including workflow overhead.

About one month after the first live release, review crates.io downloads. If there are
no outside users, consider a slower crates.io cadence while keeping GitHub Releases
useful. If the package reaches the 8 MiB warning threshold, review its contents; splitting
large embedded assets into a separate crate is an explicit future design choice.

## Related

- [Release train reference](reference.md)
- [Pause and resume releases](pause-and-resume-releases.md)
- [The release-train decision](../decisions.md)
