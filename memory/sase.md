---
type: short
parent: AGENTS.md
---

# SASE = Structured Agentic Software Engineering

## Ephemeral `bob-cli_<N>` Workspace Directories

SASE runs agents (like you) from ephemeral workspace directories, which are full clones of the bob-cli repo. These
directories are named `bob-cli_<N>` where `<N>` is some integer. You need to be mindful not to run commands outside of
these workspace directories, since they have their own isolated virtual environments.

## Sibling Repositories

Configured sibling repositories for this context:

- `bob-plugins`: Source-of-truth monorepo for Bryan's custom Bob Obsidian plugins, deployed to the vault via `bob
  plugins sync`.

When you need to make changes to files in a numbered-workspace sibling repository or need to review numbered-workspace
sibling repository code, agents MUST run:

```bash
sase workspace open -p <sibling_repo> <workspace_num>
```

`<workspace_num>` must be the workspace number assigned to the primary repo (check what directory you were started in to
figure this out). Use the path printed by `sase workspace open` as the only repository path for numbered-workspace
sibling reads/writes.
