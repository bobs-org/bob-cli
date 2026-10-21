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

## Related

- [Release train reference](releases/reference.md)
- [Test release workflow changes](releases/test-release-workflow-changes.md)
- [Build the docs](build.md)
