# bash 3.2-safe loader for scripts/lib/install_ui.
# Sourced by the justfile `install` recipe (and later by scripts/install_all).

_install_ui_lib_dir="$(CDPATH= cd "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
_install_ui_repo_root="$(CDPATH= cd "${_install_ui_lib_dir}/../.." && pwd)"

_install_ui_python_ok() {
  local py="$1"
  "$py" -c 'import sys; raise SystemExit(0 if sys.version_info >= (3, 9) else 1)' \
    >/dev/null 2>&1
}

# Resolve Python >= 3.9 once and cache it in BOB_INSTALL_UI_PYTHON.
# Honors BOB_INSTALL_UI=0 and BOB_INSTALL_UI_PYTHON=/path. Prints the
# interpreter path on stdout; returns 1 when the helper is unavailable.
install_ui_python() {
  if [[ "${BOB_INSTALL_UI:-}" == "0" ]]; then
    return 1
  fi
  if [[ -n "${BOB_INSTALL_UI_PYTHON:-}" ]]; then
    if _install_ui_python_ok "${BOB_INSTALL_UI_PYTHON}"; then
      printf '%s\n' "${BOB_INSTALL_UI_PYTHON}"
      return 0
    fi
    return 1
  fi
  local cand resolved
  for cand in python3 python; do
    resolved="$(command -v "$cand" 2>/dev/null)" || continue
    if _install_ui_python_ok "$resolved"; then
      BOB_INSTALL_UI_PYTHON="$resolved"
      export BOB_INSTALL_UI_PYTHON
      printf '%s\n' "$resolved"
      return 0
    fi
  done
  return 1
}

install_ui() {
  # Call without command substitution so the BOB_INSTALL_UI_PYTHON cache
  # sticks in this shell (bash command substitution runs in a subshell).
  install_ui_python >/dev/null || return 1
  "${BOB_INSTALL_UI_PYTHON}" -B "${_install_ui_repo_root}/scripts/lib/install_ui" "$@"
}

# Best-effort watermark write; silent on every failure.
install_ui_mark_installed() {
  install_ui mark-installed --repo "$1" --dir "$2" >/dev/null 2>&1 || true
}
