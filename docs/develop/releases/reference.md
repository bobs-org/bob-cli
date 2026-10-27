---
title: Release train reference
description: Exact schedule, triggers, guard rules, release metadata, and package limits for bob-cli.
---

# Release train reference

This page records the current release.yml, ci.yml, release-plz.toml, and tools/release
configuration. The release train is in rehearsal mode until git_only is changed. It
manages bob-obsidian and bob-obsidian-assets together; they version independently.

## Schedule

The workflow runs at 08:17 UTC and 10:47 UTC every day.

| Season | First slot | Retry slot |
| --- | --- | --- |
| EDT (UTC−4) | 04:17 New York | 06:47 New York |
| EST (UTC−5) | 03:17 New York | 05:47 New York |

The guard compares calendar dates in America/New_York. The second slot retries a day
that had no successful release; after a release, it skips.

## Triggers and dispatch inputs

| Trigger | Behavior |
| --- | --- |
| schedule | Runs at both UTC slots with a real release request |
| workflow_dispatch | Accepts dry_run (boolean, default false), override_reason (string, default empty), and binaries_tag (string, default empty). binaries_tag rebuilds archives for an existing vX.Y.Z Release and skips the guard, CI, and release. |
| push to ci/release/** | Always forces dry-run behavior |

A dry run can execute the guard, full CI, and preview. It does not push, tag, create a
Release, or publish a crate. A non-dry-run release is allowed only from master.

## Repository setting and environment

| Name | Kind | Required value or behavior |
| --- | --- | --- |
| RELEASES_PAUSED | Repository Actions variable | Exactly false arms the train. Missing or any other value keeps it paused. |
| release | GitHub environment | Used by the release job. Future live mode needs a Trusted Publisher for both bob-obsidian and bob-obsidian-assets (owner bobs-org, repository bob-cli, workflow release.yml, environment release). This configuration does not arm or switch the publisher. |

An absent variable is reported as unarmed; a present but non-false value is reported as
paused. Neither state proceeds to release.

## Job graph and permissions

| Job | Depends on | Work | Permissions |
| --- | --- | --- | --- |
| plan | — | Resolve SHA and run the guard | contents: read |
| ci | plan | Reusable ci.yml with the planned SHA and full=true | contents: read |
| release | plan, ci | Run release-plz update, fast-forward push, and release-plz release | contents: write; id-token: write; environment release |
| preview | plan, ci | Update in the checked-out SHA and print the dry-run diff | contents: read |
| binaries | release | Call release-binaries.yml for the tag release-plz reported | contents: write |
| binaries-retry | — | Call release-binaries.yml for workflow_dispatch binaries_tag | contents: write |

The release job needs CI and a GO verdict. The preview job needs a dry-run verdict.
`binaries` and `binaries-retry` call `.github/workflows/release-binaries.yml`. They run
only when release-plz's JSON reports a `bob-obsidian` release or a `binaries_tag` was
given. An assets-only result does not trigger a binary build. They build from the root
package's release tag with `--locked`.

## Daily guard

The guard uses release-plz.toml to choose its data source: git_only=true means
rehearsal; any other value means live.

| Condition | Verdict |
| --- | --- |
| RELEASES_PAUSED is unset or not exactly false | Skip as unarmed or paused |
| Real release from a ref other than master | Refuse |
| Selected SHA already has a v* tag | Skip |
| Release-data lookup fails | Fail closed; no release |
| No previous release | GO, or explain that the next scheduled run would ship |
| Latest release is on an earlier New York date | GO, or dry-run preview |
| Latest release is on a later New York date | Fail closed as clock skew |
| Release already exists today | Skip unless override_reason is non-empty |

The hotfix override bypasses only the same-day ceiling. It does not bypass pause state,
the master-ref rule, an already-tagged SHA, lookup errors, or full CI.

### Data source by mode

| Mode | Source | Guard timestamp |
| --- | --- | --- |
| Rehearsal | Up to 100 GitHub Releases, including drafts and prereleases | Newest createdAt |
| Live | bob-obsidian's crates.io sparse-index entry | Newest pubtime, including yanked versions |

The rehearsal guard counts GitHub Releases rather than tags. A tag without a GitHub
Release does not advance the rehearsal date. In live mode, a successful crates.io
publication advances the guard even if its later GitHub tag or Release step fails.

## release-plz configuration

| Setting | Value | Effect |
| --- | --- | --- |
| workspace.git_only | true | Rehearsal: tags and GitHub Releases only |
| workspace.semver_check | false | src/lib.rs is an internal seam, not the product contract |
| workspace.dependencies_update | false | Do not update dependency versions during release |
| workspace.publish_timeout | 30m | Publishing timeout |
| workspace.git_tag_name | v{{ version }} | Default tag pattern, kept by bob-obsidian |
| package.bob-obsidian.git_tag_name | v{{ version }} | Root tag and GitHub Release name |
| package.bob-obsidian-assets.git_tag_name | bob-obsidian-assets-v{{ version }} | Unique assets tag; cannot collide with vX.Y.Z |
| package.bob-obsidian-assets.git_release_enable | false | Tag and process assets without a GitHub Release |
| changelog.protect_breaking_commits | true | Preserve breaking commits even for otherwise skipped types |

The release-plz version pinned in CI, release.yml, and release-preview is 0.3.173.

## Version and changelog rules

The release_commits expression accepts feat, fix, perf, and revert commits, plus any
Conventional Commit type marked breaking with !. Docs-only, chore, refactor, test, ci,
style, and build commits do not release by themselves. A breaking docs! or refactor!
commit is preserved in the Breaking group.

The changelog groups are Breaking, Added, Fixed, Performance, and Reverted. The release
phase verified these 0.x results:

| Commits since the last release | Result |
| --- | --- |
| feat plus docs | 0.2.1; docs are omitted from the changelog |
| docs only | No version bump |
| feat! | 0.3.0; a breaking change advances the minor version in 0.x |

## Version-bump commit and release names

When release-plz update changes files, the workflow commits them as
chore(release): vX.Y.Z, bob-obsidian-assets-vA.B.C with only the packages whose
versions changed, using the GitHub Actions bot identity and
41898282+github-actions[bot]@users.noreply.github.com. The push is fast-forward only.
If master moved, the push fails and nothing ships in that run.

bob-obsidian tags and GitHub Releases use vX.Y.Z. bob-obsidian-assets tags use
bob-obsidian-assets-vA.B.C and do not create a GitHub Release, so an assets-only
run does not advance the rehearsal daily guard. release-plz owns the assets
changelog at crates/bob-obsidian-assets/CHANGELOG.md; do not hand-author either
version or changelog. The release-plz release step runs even when update makes no
changes, so it can finish a bump left pending by an earlier failed run. Dry-run
previews report both package versions and each changelog that changed.

## Actions and tool versions

| Tool | Pinned version |
| --- | --- |
| Rust toolchain | 1.95.0, with rustfmt and clippy |
| release-plz | 0.3.173 |
| actions/checkout | v6.1.0 |
| Swatinem/rust-cache | v2.9.3 |
| taiki-e/install-action | v2.87.29 |
| astral-sh/setup-uv | v10.3.0 |
| taiki-e/upload-rust-binary-action | v1.30.2 |

Workflow actions are pinned to commit SHAs in .github/workflows/ci.yml,
.github/workflows/release.yml, and .github/workflows/release-binaries.yml.

## Package contents and size

Both workspace members are checked independently. Cargo's compressed archive cap is
10 MiB per crate; just package-check warns each archive above 8 MiB and fails above
9 MiB.

| Package | Contents | Required notices | Latest size |
| --- | --- | --- | --- |
| bob-obsidian 0.4.0 | Cargo metadata, README, CHANGELOG, src, runtime scripts, small native assets | Project MIT license; Moment license | 3,217,425 bytes (3.07 MiB); 4.93 MiB headroom before the warning |
| bob-obsidian-assets 0.1.0 | Print fonts, web-clip fonts, Defuddle and Mermaid vendor bundles | Project MIT license; OFL, DejaVu, Defuddle, and Mermaid notices | 5,004,764 bytes (4.77 MiB); 3.23 MiB headroom before the warning |

Documentation, tests, tools, vault, sase, and .github are excluded from every archive.
The root archive must not contain the moved payload tree. Full package verification
must succeed before the assets crate exists on crates.io.

## Binaries

For each release, the workflow builds bob, bob_pomodoro, bob_notify, and
tmux_bob_pomodoro for:

| Platform | Target | Archive pattern |
| --- | --- | --- |
| Linux x86_64 | x86_64-unknown-linux-gnu | bob-TARGET.tar.gz |
| macOS arm64 | aarch64-apple-darwin | bob-TARGET.tar.gz |

Each archive holds all four binaries at the root. The Cargo binstall metadata uses the
same GitHub Release URL pattern:
{ repo }/releases/download/v{ version }/bob-{ target }.tar.gz.

## Scratch-branch hooks

Push ci/** to run the CI workflow. A branch starting ci/full also installs the extra
Linux test tools. Push ci/release/** to run the release workflow in forced dry-run
mode. Remove scratch branches when the experiment is complete; see
[Test release workflow changes](test-release-workflow-changes.md).

## Related

- [How the release train works](how-the-release-train-works.md)
- [bob-cli CI](../ci.md)
- [Pause and resume releases](pause-and-resume-releases.md)
- [Recover a failed release](recover-a-failed-release.md)
- [Releases and versions](../../reference/releases.md)
