set shell := ["bash", "-eu", "-o", "pipefail", "-c"]

fmt:
    cargo fmt --check

lint:
    cargo clippy --all-targets --all-features

test:
    cargo test

check-scripts:
    bash -n scripts/bob_notify scripts/bob_pomodoro scripts/bob_sync scripts/tmux_bob_pomodoro scripts/lib/bob_shell.sh
    mkdir -p target/py_compile
    python3 -c 'import py_compile; py_compile.compile("scripts/bob_pomodoro_runtimes", cfile="target/py_compile/bob_pomodoro_runtimes.pyc", doraise=True)'

package-list:
    cargo package --list

install-smoke:
    root="$(mktemp -d)"; cargo install --path . --root "${root}"; "${root}/bin/bob" pomodoro-runtimes --help >/dev/null
