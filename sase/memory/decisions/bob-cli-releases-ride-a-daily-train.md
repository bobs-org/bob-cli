---
summary: >-
  bob-cli ships only through the scheduled release train, at most one version per New
  York day after full CI on the exact SHA, with no release PR; agents never edit the
  Cargo.toml version or CHANGELOG.md and mark CLI or JSON contract breaks with !.
aliases:
  - bob-cli daily release train
  - daily release train
origin: requested
scope: project
---

# bob-cli Releases Ride a Daily Train

**Applies to.** bob-cli.

**Claim.** bob-cli ships only through the scheduled release train: at most one version
per New York day, and only after full CI passes on the exact SHA. There is no release
PR. Agents never edit the Cargo.toml version or CHANGELOG.md; release-plz owns both.
Agents mark CLI or JSON contract breaks with ! in the Conventional Commit subject.

**Why.** The research report
research:202610/crates_io_daily_release_train/crates_io_daily_release_train.md
compared the release-plz quickstart, a release PR with a cron merge, and a scheduled
release train. The quickstart publishes on every push; a release PR adds a merge model
and persistent review queue that this direct-to-master repository does not use; and an
hourly guarded publisher adds runs without improving the daily decision. The train
checks the exact SHA, uses the registry publication time as the live source of truth,
and rehearses in git_only mode before publishing.

The repository uses release-plz rather than cargo-semver-checks because src/lib.rs is
an internal seam rather than the product contract. cargo-dist is a binary distribution
tool, not the version and changelog authority.

**Cost.** A push can race the full CI gate; the fast-forward-only version bump then
fails and waits for a retry slot. A release may lag master by up to one day, while
Bryan's hosts and companion repositories track master. Once live, crates.io versions
are permanent, including mistakes.

**Reopens when.** The repository moves to pull requests; crates.io has no outside users
after one month of live releases; or the package size limit forces embedded assets into
a separate crate.
