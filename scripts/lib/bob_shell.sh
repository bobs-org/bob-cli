#!/bin/bash

if [[ -z "${SCRIPTNAME:-}" ]]; then
  SCRIPTNAME="$(basename "$0")"
  readonly SCRIPTNAME
fi

if [[ -z "${DEBUG+x}" ]]; then
  DEBUG=false
fi

if [[ -z "${VERBOSE+x}" ]]; then
  VERBOSE=0
fi

function _bob_verbose_enabled() {
  [[ "${DEBUG:-false}" == true ]] && return 0
  [[ "${VERBOSE:-0}" =~ ^[0-9]+$ ]] && [[ "${VERBOSE:-0}" -gt 0 ]]
}

function _bob_log() {
  local level="$1"
  shift

  while [[ "$#" -gt 0 ]]; do
    case "$1" in
    --up | -u)
      shift
      [[ "$#" -gt 0 ]] && shift
      ;;
    --)
      shift
      break
      ;;
    *)
      break
      ;;
    esac
  done

  local message="${1:-}"
  if [[ "$#" -gt 0 ]]; then
    shift
  fi

  printf '%s: %s: ' "${SCRIPTNAME}" "${level}" >&2
  if [[ "$#" -gt 0 ]]; then
    printf "${message}" "$@" >&2
  else
    printf '%s' "${message}" >&2
  fi
  printf '\n' >&2
}

function log::debug() {
  _bob_verbose_enabled || return 0
  _bob_log "debug" "$@"
}

function log::error() {
  _bob_log "error" "$@"
}

function log::info() {
  _bob_log "info" "$@"
}

function log::warn() {
  _bob_log "warning" "$@"
}

# Remove the old package entry that would otherwise conflict with Bob's
# executable names after the package is installed as bob-obsidian. Never
# uninstall a registry package, because the bob-cli name belongs to another
# project there.
function bob_prepare_install_root() {
  local root="${1:?install root is required}"
  local listing line source

  if ! listing="$(cargo install --list --root "$root")"; then
    printf 'Could not inspect Cargo installs under %s; refusing to install.\n' "$root" >&2
    return 1
  fi

  while IFS= read -r line; do
    if [[ "$line" =~ ^bob-cli[[:space:]]+v[^[:space:]]+[[:space:]]+\((.*)\):$ ]]; then
      source="${BASH_REMATCH[1]}"
      case "$source" in
        registry+* | sparse+*)
          printf 'A registry-installed bob-cli is a different project; refusing to uninstall it or install Bob.\n' >&2
          return 1
          ;;
        path+* | git+* | /* | file://* | https://* | ssh://* | git@*)
          printf 'Removing the existing bob-cli install from %s before installing bob-obsidian.\n' "$source" >&2
          cargo uninstall --root "$root" bob-cli
          return
          ;;
        *)
          printf 'Cannot identify the source of bob-cli under %s; refusing to uninstall it or install Bob.\n' "$root" >&2
          return 1
          ;;
      esac
    fi
  done <<< "$listing"
}

function die() {
  local exit_code=1

  if [[ "${1:-}" == "-x" ]]; then
    exit_code="$2"
    shift 2
  fi

  local message="${1:-}"
  if [[ "$#" -gt 0 ]]; then
    shift
  fi

  log::error --up 1 "${message}" "$@"
  exit "${exit_code}"
}

function usage() {
  local patterns=()
  if declare -p USAGE_GRAMMAR >/dev/null 2>&1; then
    patterns=("${USAGE_GRAMMAR[@]}")
  fi

  printf 'usage: '

  if [[ "${#patterns[@]}" -eq 0 ]]; then
    printf '%s\n' "${SCRIPTNAME}"
    return 0
  fi

  local pattern
  local first=true
  for pattern in "${patterns[@]}"; do
    if [[ "${first}" == true ]]; then
      printf '%s %s\n' "${SCRIPTNAME}" "${pattern}"
      first=false
    else
      printf '       %s %s\n' "${SCRIPTNAME}" "${pattern}"
    fi
  done
}
