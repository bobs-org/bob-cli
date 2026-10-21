---
title: Publish the first crates.io release
description: Move the rehearsed bob-obsidian package to crates.io with a manual first publish and Trusted Publishing.
---

# Publish the first crates.io release

Follow these steps only when Bryan has approved the move from rehearsal to live
publishing. Crates.io versions are permanent.

## Before you start

The repository must have a successful rehearsal release, a green full CI run, a
package below the upload limit, and the release environment ready. Confirm the
bob-obsidian package name is still available immediately before publishing.

## 1. Pause the train and recheck the name

Set RELEASES_PAUSED to true using
[Pause and resume releases](pause-and-resume-releases.md). Check crates.io for the
bob-obsidian name immediately before the first publish.

## 2. Create a short-lived publishing token

In crates.io, create a token with publish-new and publish-update scopes restricted to
bob-obsidian. Keep it out of shell history and logs.

## 3. Publish the latest rehearsal tag manually

From a clean clone, check out the latest rehearsal tag and verify Cargo can publish it:

~~~bash
git checkout vX.Y.Z
cargo publish --locked --dry-run
cargo publish --locked
~~~

After Cargo confirms the publish, revoke the token. Do not store it in GitHub Actions.

## 4. Configure Trusted Publishing

On crates.io, add a Trusted Publisher for owner bobs-org, repository bob-cli, workflow
release.yml, and environment release. The first manual publish creates the crate;
Trusted Publishing cannot create it.

## 5. Switch the workflow to live mode

Change release-plz.toml to set git_only to false and land that change through the normal
repository flow. The guard then uses crates.io publication times. Do not set
publish = true alongside git_only.

## 6. Resume and observe the train

Set RELEASES_PAUSED to false. Watch two automated releases succeed through Trusted
Publishing before enabling Trusted-Publishing-only mode in crates.io.

## 7. Update reader install instructions

After the automated path is proven, change the install instructions to use
cargo install bob-obsidian --locked. Review README.md, docs/start/install.md,
docs/guides/install-everything.md, and docs/completion.md for source-install guidance.

## If it goes wrong

If Cargo reports a publish error, keep the train paused and check the package name,
token scopes, package contents, and crates.io version state. Do not rerun the publish
blindly: a successful publication cannot be replaced.

## Related

- [How the release train works](how-the-release-train-works.md)
- [Keep the package under the size cap](keep-the-package-under-the-size-cap.md)
- [Release train reference](reference.md)
