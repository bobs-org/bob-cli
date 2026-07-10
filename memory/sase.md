---
type: short
parent: AGENTS.md
---

# SASE = Structured Agentic Software Engineering

## Ephemeral `bob-cli_<N>` Workspace Directories

SASE runs agents (like you) from ephemeral workspace directories, which are full clones of the bob-cli repo. These
directories are named `bob-cli_<N>` where `<N>` is some integer. You need to be mindful not to run commands outside of
these workspace directories, since they have their own isolated virtual environments.

IMPORTANT: Do NOT mention your workspace directory (or any sibling workspace directory) in any plan files that you
generate using your `/sase_plan` skill. The agent(s) that implement the plan might not run in the same workspace
directory as you!

## Linked Repositories

Configured linked repositories for this context:

- `bob-plugins`: Source-of-truth monorepo for Bryan's custom Bob Obsidian plugins, deployed to the vault via `bob
  plugins sync`. You should NOT edit these plugins directly in the ~/bob/ directory, as they will be overwritten on the
  next sync. Instead, make changes to this linked repo and, when done, run the `bob plugins sync` command to deploy them
  to the ~/bob/ directory.

When you need to make changes to files in a numbered-workspace linked repo or need to review numbered-workspace linked
repo code, agents MUST run:

```bash
sase workspace open -p <linked_repo> -r "<reason>" <workspace_num>
```

`<workspace_num>` must be the workspace number assigned to the primary repo (check what directory you were started in to
figure this out). Use the path printed by `sase workspace open` as the only linked repo path for numbered-workspace
linked reads/writes.

IMPORTANT REMINDER: Do NOT attempt to look for a linked repo in any other way than by using `sase workspace open`!
