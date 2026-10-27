---
title: "Tutorial: Preview the next release"
description: Preview bob-cli's next package versions, changelogs, archive sizes, and release guard verdict.
search:
  boost: 2
---

# Tutorial: Preview the next release

This tutorial shows what the current release configuration would do for
bob-obsidian and bob-obsidian-assets, without pushing, tagging, creating a
GitHub Release, or publishing a crate.

## Before you start

Use a bob-cli checkout with Git, just, Cargo, and release-plz 0.3.173 installed.
The preview needs network access to clone origin/master and inspect the guard's live
release data. It does not use uncommitted changes in your checkout.

## 1. Preview release-plz's changes

~~~bash
just release-preview
~~~

It clones origin/master into a temporary directory and runs release-plz update there.
Read both reported package versions, the diff summary, and any new CHANGELOG.md or
crates/bob-obsidian-assets/CHANGELOG.md section. If release-plz makes no changes, there
is no user-visible release commit to ship.

## 2. Check both packages

Run the same package check used by CI:

~~~bash
just package-check
~~~

Confirm that bob-obsidian and bob-obsidian-assets each list the required license
notices and that each reported archive size is below the failure threshold. See
[Keep the package under the size cap](keep-the-package-under-the-size-cap.md).

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
