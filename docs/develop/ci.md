---
title: bob-cli CI
description: Triggers, jobs, platform coverage, and local checks for the bob-cli CI workflow.
---

# bob-cli CI

The reusable CI workflow checks Rust code, scripts, and package contents on Linux and
macOS. The release train calls it with full=true for the exact SHA it plans to release.

## Triggers

CI runs on pushes to master and ci/** branches, pull requests, manual workflow
dispatches, and workflow_call. Manual dispatch accepts full, a boolean that defaults to
false. The release workflow passes the SHA and full=true through workflow_call.

## Jobs

| Job | Runner | Checks | Local equivalent |
| --- | --- | --- | --- |
| lint | ubuntu-latest | Format, clippy, scripts, and release-tool tests | just fmt; just lint; just check-scripts; just check-release-tools |
| test | ubuntu-latest and macos-latest | Locked Rust test suite; macOS also lints macOS-only targets | cargo test --locked --no-fail-fast; on macOS also just lint |
| package | ubuntu-latest | Package contents and archive size | just package-check |

The lint job runs release-tool unit tests through check-release-tools. The package job
uses Cargo package verification, checks that required notices are included, and keeps
the archive below its size thresholds. CI does not run docs-check; the docs publishing
pipeline owns that gate.

When full=true, Linux installs zsh, pandoc, poppler utilities, TeX packages, fonts, and
other tools needed by tests that otherwise self-skip. A ci/full* push also enables this
set of tools. macOS installs Bash 5 for tests that need it while preserving its native
BSD utilities.

## Measured GitHub job times

The CI phase measured these wall times on GitHub Actions:

| Run | Lint | Package | Linux tests | macOS tests |
| --- | --- | --- | --- | --- |
| Normal, full=false | 1m13s | 1m15s | 3m49s | 7m10s |
| Full, full=true | 1m43s | 1m48s | 7m10s | 11m35s |

Runner queues and cache state change these times; use them as a reference, not a
service-level target.

## Keep tests portable

Tests must not depend on a developer's HOME configuration, extra PATH tools, locale, or
timezone. Give tests temporary configuration and directories, put any stub executable
on a test-controlled PATH, and set an available UTF-8 locale or make text assertions
locale-independent. Set TZ explicitly when a test depends on a date.

## Run CI on a scratch branch

Push a ci/** branch to run the normal workflow without changing master. Use a branch
starting with ci/full to run the extra Linux tools. Remove the scratch branch after
review. For release workflow changes, use ci/release/**; those runs are always dry
runs.

## Precommit macOS check

`scripts/dev/macos-ci-check` snapshots this checkout's current working tree into a
throwaway commit object through a temporary index, pushes it only to
`refs/heads/ci/precommit/<id>`, waits for the GitHub Actions run for that exact SHA,
prints every macOS step's verdict, and deletes the remote branch. HEAD, local
branches, the caller's index, and working files stay unchanged. The snapshot includes
tracked modifications, deletions, and non-ignored untracked files as they exist on
disk, including partially staged files' worktree contents. Ignored files and dirty
submodules are not uploaded; dirty submodules are refused.

Prerequisites: `python3`, `git`, authenticated `gh`, push access, and a GitHub origin
that identifies `bobs-org/bob-cli`. The command never prints credentials or logs in
for you.

Default deadlines, overridable with positive finite `BOB_MACOS_CI_*_SECONDS` values:

| Variable | Default | Meaning |
| --- | --- | --- |
| `BOB_MACOS_CI_DISCOVERY_SECONDS` | 300 | Find the first matching run after push |
| `BOB_MACOS_CI_TOTAL_SECONDS` | 5400 | Overall check, including push and wait |
| `BOB_MACOS_CI_COMMAND_SECONDS` | 60 | Each git/gh call, capped by remaining time; leftover below 1s (or the command cap) is the deadline |
| `BOB_MACOS_CI_CLEANUP_SECONDS` | 120 | Cancel an unfinished run and delete the ref |
| `BOB_MACOS_CI_POLL_SECONDS` | 10 | Interval between status polls |

The selected `ci.yml` push run must pass, including non-macOS jobs. Required macOS
coverage is job `test (macos-latest)` with steps `Lint macOS-only targets`,
`Run tests`, and `Run archive-move presenter cases`. Conditional diagnostic, upload,
and Linux-only steps may be skipped.

Exit codes: `0` the workflow and required macOS checks passed and remote branch
deletion was verified; `1` the selected run completed with a real CI failure; `2`
verification could not finish (missing prerequisites, no matching run, deadline,
cancellation, missing coverage, or cleanup failure). Cleanup failure takes
precedence and returns `2` while still reporting any observed CI failure.

SIGINT and SIGTERM stop child processes, cancel an unfinished selected run when
possible, delete only this invocation's ref, and verify it is gone. An already-absent
ref is success. A ref that now points at a different SHA is not deleted. If deletion
cannot be verified, the printed repository, ref, snapshot SHA, and deletion command
are the recovery path.

Offline tests, which never invoke the live gate:

```sh
python3 -B scripts/dev/test_macos_ci_check.py
```

From a SASE agent, run the live gate through `sase monitor start` with profile
`verify` and a timeout longer than 90 minutes plus the two-minute cleanup budget
(for example `--timeout 95m`). The command snapshots and pushes before waiting, so
`verify` is the right profile. `--next` should inspect the result, repair if needed,
record verification evidence, and close the assigned bead. Wait for
`sase monitor start` itself to exit.

## Related

- [Release train reference](releases/reference.md)
- [Test release workflow changes](releases/test-release-workflow-changes.md)
- [Build the docs](build.md)
