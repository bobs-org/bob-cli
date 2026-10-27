---
title: Releases
description: Maintainer procedures and reference for bob-cli's scheduled release train.
---

# Releases

These pages explain how bob-cli previews, schedules, and recovers releases.

## Learn

- [Tutorial: Preview the next release](preview-the-next-release.md) — inspect the
  next package versions, changelogs, archive sizes, and guard verdict.

## Do

- [Pause and resume releases](pause-and-resume-releases.md)
- [Ship a hotfix](ship-a-hotfix.md)
- [Withdraw a bad release](withdraw-a-bad-release.md)
- [Recover a failed release](recover-a-failed-release.md)
- [Publish the first crates.io release](publish-to-crates-io.md)
- [Keep the package under the size cap](keep-the-package-under-the-size-cap.md)
- [Update the Rust toolchain](update-the-rust-toolchain.md)
- [Test release workflow changes](test-release-workflow-changes.md)

## Understand

- [How the release train works](how-the-release-train-works.md) — the daily gate,
  rehearsal, live publishing, and trade-offs.

## Look up

- [Release train reference](reference.md) — workflow inputs, guard decisions,
  release metadata, and tool versions.
- [bob-cli CI](../ci.md) — the checks the train requires.
- [Releases and versions](../../reference/releases.md) — GitHub Releases,
  0.x numbering, and how to check the installed CLI.
