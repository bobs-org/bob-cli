---
title: "Install Bob"
description: "Check out bob-cli, install with just, and verify the binary."
search:
  boost: 2
---

# Install Bob

Start from a Git checkout with a current stable Rust toolchain and `just`.
The steps below install from source and confirm the binary answers.

## 1. Check out the source

<!-- tutorial-test: skip network clone needs a remote and user credentials -->
```bash
git clone https://github.com/bobs-org/bob-cli.git
cd bob-cli
```

Use the public checkout above. Keep it; the tutorial and verification steps
run from its root.

## 2. Install the toolchain

<!-- tutorial-test: skip toolchain install changes the host and needs network -->
```bash
rustup update stable
cargo --version
rustc --version
just --version
```

Bob needs a current stable Rust compiler that supports edition 2024, plus
`just` for its recipes. See
[Requirements and compatibility](../reference/compatibility.md) for the full
matrix rather than a version pinned here.

## 3. Install Bob

<!-- tutorial-test: skip install writes outside the tutorial sandbox -->
```bash
just install
```

`just install` builds with Cargo and installs `bob` plus shell completion
into `${CARGO_INSTALL_ROOT:-${CARGO_HOME:-~/.cargo}}`, overriding cargo's
`install.root` so the recipe refreshes the binary it installed. Do not pass
`--force`: cargo already replaces the same package from any checkout. Ensure
that `bin` directory is on `PATH`:

<!-- tutorial-test: skip PATH export edits the reader shell profile -->
```bash
export PATH="$HOME/.cargo/bin:$PATH"
```

## 4. Verify the install

```bash
bob --version
```

```text { .no-copy title="Output" }
bob 0.1.0
```

The exact version changes over time; the check passes when the command
prints a version and exits 0. For command help, run `bob --help`.
`just install` also runs `bob completion install`, which never edits your
rc files. Inspect the state with `bob completion` (same as
`bob completion status`). See [shell completion](../completion.md) for the
full contract.

## Install from Git

Without a local checkout, install the published crate from Git:

<!-- tutorial-test: skip network install needs a remote and writes outside the sandbox -->
```bash
cargo install --git https://github.com/bobs-org/bob-cli.git --locked bob-cli && bob completion install
```

To install the CLI, plugins, and (on macOS) Mac Capture from sibling
checkouts, use [Install everything with one command](../guides/install-everything.md).

## Next

- [Your first Bob day](first-bob-day.md) — capture and plan in a demo vault
- [Install everything with one command](../guides/install-everything.md) — plugins and Mac Capture
- [Requirements and compatibility](../reference/compatibility.md) — supported platforms and floors
