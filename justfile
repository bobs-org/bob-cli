set shell := ["bash", "-eu", "-o", "pipefail", "-c"]

# Horizontal rule drawn beneath each section title (single source for width/char).
rule := "────────────────────────────────────────────────"

all: fmt lint test
    @if [[ -t 1 ]]; then \
        printf '\n  \033[1;32m✓  ALL CHECKS PASSED\033[0m\n\n'; \
    else \
        printf '\n  ✓  ALL CHECKS PASSED\n\n'; \
    fi

fmt: (_banner "34" "🎨" "FORMAT")
    cargo fmt --check

lint: (_banner "33" "🔍" "LINT")
    cargo clippy --all-targets --all-features

test: (_banner "32" "🧪" "TEST")
    cargo test --no-fail-fast

# Canonical verification gate: formatting, lints, then every test binary
# (lib, all tests/* binaries, doctests) even when one binary fails.
check: (_banner "32" "✅" "CHECK")
    cargo fmt --check
    cargo clippy --all-targets --all-features
    cargo test --no-fail-fast

# Render a themed section banner: blank line, bold colored title + icon, colored rule.
# Emits ANSI styling on a TTY and clean plain text when output is piped/redirected.
_banner color icon label:
    @if [[ -t 1 ]]; then \
        printf '\n%s  \033[1;%sm%s\033[0m\n\033[%sm%s\033[0m\n' '{{icon}}' '{{color}}' '{{label}}' '{{color}}' '{{rule}}'; \
    else \
        printf '\n%s  %s\n%s\n' '{{icon}}' '{{label}}' '{{rule}}'; \
    fi

check-scripts:
    bash -n scripts/bob_notify scripts/bob_pomodoro scripts/tmux_bob_pomodoro scripts/lib/bob_shell.sh scripts/install_all

# Type-check the pinned Keep adapter and run its offline self-test.
# Not part of `all`: the first run fetches the pinned Python deps.
check-adapter:
    python3 -m py_compile scripts/gkeep_adapter.py && uv run --quiet --script scripts/gkeep_adapter.py --self-test

# Type-check the pinned web-clip adapter and run its offline self-test.
# Not part of `all`: the first run fetches the pinned Python deps, and the
# browser-backed fixture checks run only when a browser is discovered
# (otherwise they print "skipped: no browser" and still pass).
check-web-clip-adapter:
    python3 -m py_compile scripts/web_clip/web_clip_adapter.py scripts/web_clip/web_clip_render.py && uv run --quiet --script scripts/web_clip/web_clip_adapter.py --self-test

package-list:
    cargo package --list

# Install bob and its shims from this checkout, then install or refresh shell
# completion. Pass shells to choose explicitly: `just install zsh bash`.
[positional-arguments]
install *shells: (_banner "35" "📦" "INSTALL")
    #!/usr/bin/env bash
    set -euo pipefail
    root="${CARGO_INSTALL_ROOT:-${CARGO_HOME:-$HOME/.cargo}}"
    cargo install --path . --locked --root "$root"
    if ! "$root/bin/bob" completion install "$@"; then
        printf '\nbob is installed at %s; shell completion needs attention (see above).\n' \
            "$root/bin/bob" >&2
        exit 1
    fi

# Offers to clone missing sibling checkouts over SSH; otherwise they are skipped.
# Pull + install bob-cli, bob-plugins, and bob-mac-capture; restart Obsidian if plugins changed.
install-all:
    @scripts/install_all

install-smoke:
    #!/usr/bin/env bash
    set -euo pipefail
    root="$(mktemp -d)"
    cargo install --path . --locked --root "${root}"
    "${root}/bin/bob" --help >/dev/null
    "${root}/bin/bob" capture --help >/dev/null
    "${root}/bin/bob" completion --help >/dev/null
    "${root}/bin/bob" completion install --help >/dev/null
    "${root}/bin/bob" completion status --help >/dev/null
    "${root}/bin/bob" completion uninstall --help >/dev/null
    "${root}/bin/bob" completion bash --help >/dev/null
    "${root}/bin/bob" completion zsh --help >/dev/null
    "${root}/bin/bob" __complete zsh --protocol 1 -- bob cap | grep -q '^capture'
    "${root}/bin/bob" __complete bash --protocol 1 -- bob cap | grep -q '^capture'
    "${root}/bin/bob" capture-complete --help >/dev/null
    "${root}/bin/bob" capture-parse --help >/dev/null
    "${root}/bin/bob" capture-pomodoro-name --help >/dev/null
    "${root}/bin/bob" capture-pomodoros --help >/dev/null
    "${root}/bin/bob" capture-rewrite --help >/dev/null
    "${root}/bin/bob" capture-sections --help >/dev/null
    "${root}/bin/bob" capture-targets --help >/dev/null
    "${root}/bin/bob" capture-task-id --help >/dev/null
    "${root}/bin/bob" capture-task-sections --help >/dev/null
    "${root}/bin/bob" capture-tasks --help >/dev/null
    "${root}/bin/bob" freshness --help >/dev/null
    "${root}/bin/bob" freshness list --help >/dev/null
    "${root}/bin/bob" freshness seed --help >/dev/null
    "${root}/bin/bob" gkeep --help >/dev/null
    "${root}/bin/bob" gkeep doctor --help >/dev/null
    "${root}/bin/bob" gkeep list --help >/dev/null
    "${root}/bin/bob" gkeep login --help >/dev/null
    "${root}/bin/bob" gkeep migrate-markers --help >/dev/null
    "${root}/bin/bob" gkeep pull --help >/dev/null
    "${root}/bin/bob" query --help >/dev/null
    "${root}/bin/bob" randomize --help >/dev/null
    "${root}/bin/bob" ready --help >/dev/null
    "${root}/bin/bob" ref --help >/dev/null
    "${root}/bin/bob" ref clip --help >/dev/null # hidden alias smoke: clip == create
    "${root}/bin/bob" ref create --help >/dev/null
    "${root}/bin/bob" ref find --help >/dev/null
    "${root}/bin/bob" ref list --help >/dev/null
    "${root}/bin/bob" ref show --help >/dev/null
    "${root}/bin/bob" highlights --help >/dev/null
    "${root}/bin/bob" highlights create --help >/dev/null
    "${root}/bin/bob" task-status-hooks --help >/dev/null
    "${root}/bin/bob" move-done-tasks --help >/dev/null
    "${root}/bin/bob" nightly --help >/dev/null
    "${root}/bin/bob" notify --help >/dev/null
    "${root}/bin/bob" plan --help >/dev/null
    "${root}/bin/bob" plugins --help >/dev/null
    "${root}/bin/bob" plugins list --help >/dev/null
    "${root}/bin/bob" plugins sync --help >/dev/null
    "${root}/bin/bob" pomodoro --help >/dev/null
    "${root}/bin/bob" pomodoro notify --help >/dev/null
    "${root}/bin/bob" pomodoro status --help >/dev/null
    "${root}/bin/bob" pomodoro tmux --help >/dev/null
    "${root}/bin/bob" task --help >/dev/null
    "${root}/bin/bob" task archive --help >/dev/null
    "${root}/bin/bob" task reconcile --help >/dev/null
    "${root}/bin/bob" task reroll --help >/dev/null
    "${root}/bin/bob" help plan >/dev/null
    "${root}/bin/bob" projects --help >/dev/null
    "${root}/bin/bob" projects sync --help >/dev/null
    "${root}/bin/bob" tmux-pomodoro --help >/dev/null
    "${root}/bin/bob" vault-sync --help >/dev/null
    "${root}/bin/bob_notify" --help >/dev/null
    "${root}/bin/bob_pomodoro" --help >/dev/null
    "${root}/bin/tmux_bob_pomodoro" --help >/dev/null
    smoke_home="${root}/home"
    mkdir -p "${smoke_home}"
    smoke_env="env HOME=${smoke_home} XDG_STATE_HOME=${smoke_home}/.local/state XDG_DATA_HOME=${smoke_home}/.local/share ZDOTDIR=${smoke_home}/.zdot SHELL=/bin/zsh"
    ${smoke_env} "${root}/bin/bob" completion install zsh -d -t "${root}/zfunc"
    test ! -e "${root}/zfunc"
    test ! -e "${smoke_home}/.local/state/bob-cli/completion/manifest.json"
    ${smoke_env} "${root}/bin/bob" completion install zsh -n -t "${root}/zfunc"
    test -f "${root}/zfunc/_bob"
    ${smoke_env} "${root}/bin/bob" completion status -j > /dev/null
    ${smoke_env} "${root}/bin/bob" completion uninstall zsh
    test ! -e "${root}/zfunc/_bob"
