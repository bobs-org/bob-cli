# Bob CLI

Bob is the command-line tool for a GTD workflow that lives in one Obsidian
vault, with optional plugins and a Mac capture panel.

![Colored CLI output of bob capture writing Review the queue input into mac_inbox.md](docs/assets/media/cli/capture.svg)

## Requirements

- Linux or macOS
- A current stable Rust toolchain that supports edition 2024, plus `just`
- An existing vault directory (default `~/bob`; set `BOB_DIR` to override)
- Obsidian 1.8.7 or later with the Tasks plugin, for the desktop interface

The CLI, clipboard discovery, and browser helpers cover Linux and macOS.
See [Requirements and compatibility](https://bobs-org.github.io/reference/compatibility/)
for floors, shims, and runtime tools.

## Install

From a checkout of this repository:

```bash
just install
```

That installs `bob` (and its compatibility binaries) into
`${CARGO_INSTALL_ROOT:-${CARGO_HOME:-~/.cargo}}` and runs
`bob completion install`. Put that `bin` directory on `PATH`. Do not pass
`--force` to these installs.

Install the CLI, the Obsidian plugins, and (on macOS) Mac Capture from
sibling checkouts:

```bash
just install-all
```

From Git without a checkout:

```bash
cargo install --git https://github.com/bobs-org/bob-cli.git --locked bob-obsidian && bob completion install
```

Point `bob` at a vault and confirm it answers:

```bash
export BOB_DIR=/path/to/bob-vault
bob --help
```

## Components

- **bob-cli** (this repository) — the `bob` command
- **[bob-plugins](https://github.com/bobs-org/bob-plugins)** — Obsidian plugins
- **[bob-mac-capture](https://github.com/bobs-org/bob-mac-capture)** — optional macOS menu-bar panel

Keep the checkouts together, for example:

```text
~/src/bobs-org/
├── bob-cli/
├── bob-plugins/
└── bob-mac-capture/
```

## Docs →

- [Get started](https://bobs-org.github.io/start/)
- [Reference](https://bobs-org.github.io/reference/)
- [Develop](https://bobs-org.github.io/develop/)

The site is <https://bobs-org.github.io/>.
`bob --help` is the command index.

## License

[MIT](LICENSE)
