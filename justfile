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
    bash -n scripts/bob_notify scripts/bob_pomodoro scripts/tmux_bob_pomodoro scripts/lib/bob_shell.sh

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

install-smoke:
    #!/usr/bin/env bash
    set -euo pipefail
    root="$(mktemp -d)"
    cargo install --path . --locked --root "${root}"
    "${root}/bin/bob" --help >/dev/null
    "${root}/bin/bob" capture --help >/dev/null
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
    "${root}/bin/bob" gkeep pull --help >/dev/null
    "${root}/bin/bob" query --help >/dev/null
    "${root}/bin/bob" randomize --help >/dev/null
    "${root}/bin/bob" highlights --help >/dev/null
    "${root}/bin/bob" highlights clip --help >/dev/null
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
    "${root}/bin/bob" projects --help >/dev/null
    "${root}/bin/bob" projects sync --help >/dev/null
    "${root}/bin/bob" tmux-pomodoro --help >/dev/null
    "${root}/bin/bob" vault-sync --help >/dev/null
    "${root}/bin/bob_notify" --help >/dev/null
    "${root}/bin/bob_pomodoro" --help >/dev/null
    "${root}/bin/tmux_bob_pomodoro" --help >/dev/null
