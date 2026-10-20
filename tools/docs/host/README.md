# bobs-org.github.io

This repository publishes the Bob documentation site at
<https://bobs-org.github.io/>. It holds only the caller workflows, this README,
and the durable `published.json` publication log. The site source, reusable
workflows, and publication logic live in
[bobs-org/bob-cli](https://github.com/bobs-org/bob-cli).

Do not treat this repository as documentation source. Edit pages, the theme,
and the publisher in bob-cli, then let the hourly poll (cron `17 * * * *`) or
an operator dispatch publish the result. The Monday weekly check (cron
`41 6 * * 1`) builds the strict Material site and fetches external links with
bounded retries; it never deploys and never writes `published.json`.

`published.json` is the publication log in this repository. It is not
necessarily served as `/published.json` on the live site. The live provenance
file is `/build.json`.

## Operator commands

Run these from a bob-cli checkout. Preview is the default for bootstrap;
pass `--yes` only when you intend to mutate the host.

```bash
just docs-host-bootstrap
just -- docs-host-bootstrap --yes
just docs-publish
just -- docs-publish --force
just -- docs-publish --drill
just -- docs-publish --pinned
just docs-status
```

The same operations exist as `bob-docs host-bootstrap`, `bob-docs publish`,
and `bob-docs status`. `just docs-publish` enables the workflow before
dispatching and watches the run it actually started.

## Rollback with held pinned SHAs

Hand-editing source SHAs in `published.json` is rejected: the planner rebuilds
the inputs at those SHAs and requires them to match the stored fingerprint.
Restore a previous recorded manifest whole instead.

1. In this repository, find the manifest commit to roll back to:

   ```bash
   git log --format='%h %ad %s' -- published.json
   ```

   Each `Record documentation publication` commit names its run.
2. Restore that record whole: `git show <commit>:published.json > published.json`.
   Then set `"hold": true`, commit only `published.json`, and push.
3. Dispatch a pinned publish from bob-cli:

   ```bash
   just -- docs-publish --pinned
   ```

   Pinned builds those exact SHAs and keeps the hold. Scheduled and unpinned
   runs, including `--force`, skip while held.
4. Release the hold by setting `"hold": false`, commit, and push. The next
   ordinary run publishes the current heads.

## Recovery

A failing publish opens or updates one issue titled `Docs publish blocked`
(label `docs-publish`). The next ordinary dispatch sees that open issue and
re-runs the gates even when the docs fingerprint is unchanged. Recover with a
normal publish:

```bash
just docs-publish
```

Do not use `--force` to recover. Force rebuilds unchanged inputs when nothing
is held; recovery is the blocked-issue path. A successful live verification
closes the issue.

## Weekly checks

The `weekly.yml` caller runs on `41 6 * * 1` and manual dispatch. It always
builds the strict Material site with the locked material group and runs
`bob-docs links --online`, even when the publication fingerprint is unchanged
or a publish hold remains. Weekly cannot release the hold or deploy, and it
never writes `published.json`. A failure opens or updates one issue titled
`Docs weekly checks blocked` (label `docs-weekly`); a passing run closes only
that issue.

## Inactivity

GitHub disables scheduled workflows in a public repository after 60 days
with no repository activity (`disabled_inactivity`). No-op publish runs
commit nothing, so they do not count as activity. `just docs-publish` runs
`gh workflow enable` before dispatch. `just docs-status` reports that state.

## Failure boundaries

**Before deploy.** Planner skip, a held unpinned run, or a gate failure
uploads nothing, deploys nothing, and does not rewrite `published.json`. The
last good site and manifest stay untouched. A gate failure opens the blocked
issue.

**Drill.** `--drill` runs the real gates against a staged-only broken link.
It must fail before upload, leave the live site and manifest untouched, and
open the blocked issue. Recover with a normal publish.

**After deploy.** Upload, Pages deploy, recording `published.json`, and live
verification run only after the gates pass. Only failures before deploy
promise that the last good site and manifest are untouched.

- A Pages deploy failure normally leaves the previous deployment serving,
  but the workflow does not prove it. Check `just docs-status`, which reads
  live `/build.json`.
- A record failure happens after a successful deploy. The new site is live,
  but `published.json` still names the previous publication. This includes
  a compare-and-swap rejection: `record` refuses to overwrite a deploy-repo
  change made after planning.
- A live-verification failure means the new site may be serving.

All of these keep the blocked issue open. The next ordinary run takes the
recovery path: it rebuilds, deploys, records, verifies, and closes the
issue.
