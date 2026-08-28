#!/bin/bash
# SPDX-License-Identifier: GPL-3.0-only
# See NOTICE for the required Omazen project attribution terms.

set -euo pipefail

operation=${1:-}
program_root=${2:-}
shift 2 || true

if [[ ${OMAZEN_TESTING:-0} != 1 ]]; then
  [[ $EUID == 0 ]] || { printf 'ERROR: privileged helper must run as root\n' >&2; exit 1; }
  [[ $program_root == /opt/zen-browser-bin ]] || {
    printf 'ERROR: unsupported privileged program root: %s\n' "$program_root" >&2
    exit 1
  }
fi
[[ -d $program_root && -f $program_root/application.ini ]] || {
  printf 'ERROR: invalid Zen program root: %s\n' "$program_root" >&2
  exit 1
}

allowed_destination() {
  case "$1" in
    "$program_root/config.js"|"$program_root/defaults/pref/config-prefs.js"|"$program_root/defaults/pref/omazen-prefs.js") return 0 ;;
    *) return 1 ;;
  esac
}

case "$operation" in
  install)
    (( $# > 0 && $# % 3 == 0 )) || exit 2
    while (( $# > 0 )); do
      source=$1 destination=$2 mode=$3
      shift 3
      [[ -f $source && ! -L $source ]] || { printf 'ERROR: invalid program source\n' >&2; exit 1; }
      allowed_destination "$destination" || { printf 'ERROR: refused program destination\n' >&2; exit 1; }
      [[ $mode =~ ^0?644$ ]] || { printf 'ERROR: refused program mode\n' >&2; exit 1; }
      install -D -m "$mode" -- "$source" "$destination"
    done
    ;;
  remove)
    (( $# > 0 )) || exit 2
    for destination in "$@"; do
      allowed_destination "$destination" || { printf 'ERROR: refused program destination\n' >&2; exit 1; }
      rm -f -- "$destination"
    done
    ;;
  *) exit 2 ;;
esac
