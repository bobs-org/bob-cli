---
title: "Tutorial: Preview the next release"
description: Preview bob-cli's next version, changelog, package size, and release guard verdict.
search:
  boost: 2
---

# Tutorial: Preview the next release

This tutorial shows what the current release configuration would do, without pushing,
tagging, creating a GitHub Release, or publishing a crate.

## Before you start

Use a bob-cli checkout with Git, just, Cargo, and release-plz 0.3.173 installed.
The preview needs network access to clone origin/master and inspect the guard's live
release data. It does not use uncommitted changes in your checkout.

## 1. Preview release-plz's changes

~~~bash
just release-preview
~~~

It clones origin/master into a temporary directory and runs release-plz update there.
Read the reported next version, the diff summary, and any new CHANGELOG section. If
release-plz makes no changes, there is no user-visible release commit to ship.

## 2. Check the package

Run the same package check used by CI:

~~~bash
just package-check
~~~

Confirm that the package is named bob-obsidian, the required license notices are
present, and the reported archive size is below the failure threshold.

## 3. Read the guard verdict

The release-preview output ends with the guard's explain-only result. Read its mode,
release date, and verdict. A missing RELEASES_PAUSED variable is reported as unarmed;
the train remains paused until the repository variable is exactly false. Explain-only
mode reports missing GitHub access as context instead of failing the preview.

You have now seen whether release-plz would make a release, what it would change, and
whether the daily guard would allow the next scheduled run. Continue with
[How the release train works](how-the-release-train-works.md) or look up the
[release train reference](reference.md).

## Next

- [How the release train works](how-the-release-train-works.md)
- [Release train reference](reference.md)
