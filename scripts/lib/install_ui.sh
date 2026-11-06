# bash 3.2-safe loader for scripts/lib/install_ui.
# Sourced by the justfile `install` recipe and by scripts/install_all.

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

# True only when the live UI can safely own stdout in this terminal.
install_ui_live_eligible() {
  [[ -t 1 && "${TERM:-}" != "dumb" && "${BOB_INSTALL_VERBOSE:-}" != "1" && \
     "${BOB_INSTALL_UI:-}" != "0" ]] || return 1
  install_ui_python >/dev/null || return 1
  [[ -f "${_install_ui_repo_root}/scripts/lib/install_ui/__main__.py" ]] || return 1
  install_ui --help >/dev/null 2>&1
}

# Run a build through the live UI when it is safe to render. Arguments are
# STEP PARSER REPO DONE, optional runner flags, then -- and the child command.
install_ui_run() {
  local step="$1" parser="$2" repo="$3" done_text="$4"
  local crate="" profile="" success_codes=""
  local started_file
  local -a runner_args
  shift 4
  while [[ $# -gt 0 ]]; do
    case "$1" in
      --crate) crate="$2"; shift 2 ;;
      --profile) profile="$2"; shift 2 ;;
      --success-code)
        if [[ -n "$success_codes" ]]; then
          success_codes="$success_codes $2"
        else
          success_codes="$2"
        fi
        shift 2
        ;;
      --) shift; break ;;
      *) break ;;
    esac
  done
  if ! install_ui_live_eligible; then
    "$@"
    return $?
  fi
  started_file="$(mktemp "${TMPDIR:-/tmp}/bob-install-ui-started.XXXXXX")" || {
    "$@"
    return $?
  }
  rm -f "$started_file"
  local status=0
  runner_args=(run --step "$step" --parser "$parser" --repo "$repo"
    --done "$done_text" --started-file "$started_file")
  if [[ -n "$crate" ]]; then runner_args+=(--crate "$crate"); fi
  if [[ -n "$profile" ]]; then runner_args+=(--profile "$profile"); fi
  local success_code
  for success_code in $success_codes; do
    runner_args+=(--success-code "$success_code")
  done
  if install_ui "${runner_args[@]}" -- "$@"; then
    status=0
  else
    status=$?
  fi
  if (( status == 130 )); then
    rm -f "$started_file"
    return 130
  fi
  if [[ ! -e "$started_file" ]]; then
    local fallback_status
    if "$@"; then fallback_status=0; else fallback_status=$?; fi
    rm -f "$started_file"
    return "$fallback_status"
  fi
  rm -f "$started_file"
  return "$status"
}
