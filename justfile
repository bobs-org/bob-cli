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
    cargo test

# Render a themed section banner: blank line, bold colored title + icon, colored rule.
# Emits ANSI styling on a TTY and clean plain text when output is piped/redirected.
_banner color icon label:
    @if [[ -t 1 ]]; then \
        printf '\n%s  \033[1;%sm%s\033[0m\n\033[%sm%s\033[0m\n' '{{icon}}' '{{color}}' '{{label}}' '{{color}}' '{{rule}}'; \
    else \
        printf '\n%s  %s\n%s\n' '{{icon}}' '{{label}}' '{{rule}}'; \
    fi

check-scripts:
    bash -n scripts/bob_notify scripts/bob_pomodoro scripts/bob_sync scripts/tmux_bob_pomodoro scripts/lib/bob_shell.sh

package-list:
    cargo package --list

install-smoke:
    #!/usr/bin/env bash
    set -euo pipefail
    root="$(mktemp -d)"
    cargo install --path . --locked --root "${root}"
    "${root}/bin/bob" --help >/dev/null
    "${root}/bin/bob" bulk-git-commit --help >/dev/null
    "${root}/bin/bob" dataview --help >/dev/null
    "${root}/bin/bob" highlights --help >/dev/null
    "${root}/bin/bob" move-done-tasks --help >/dev/null
    "${root}/bin/bob" nightly --help >/dev/null
    "${root}/bin/bob" notify --help >/dev/null
    "${root}/bin/bob" pomodoro --help >/dev/null
    "${root}/bin/bob" projects --help >/dev/null
    "${root}/bin/bob" tmux-pomodoro --help >/dev/null
    "${root}/bin/bob_notify" --help >/dev/null
    "${root}/bin/bob_pomodoro" --help >/dev/null
    "${root}/bin/bob_sync" --help >/dev/null
    "${root}/bin/tmux_bob_pomodoro" --help >/dev/null
