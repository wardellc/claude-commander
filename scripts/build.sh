#!/usr/bin/env bash
#
# Build/check one fresh generation of local artifacts, then retain that generation.
#
#   build.sh [VERIFY ARGS...]                 discard previous build artifacts
#   build.sh --keep-builds [VERIFY ARGS...]   reuse existing caches and variants
#
# VERIFY ARGS are passed unchanged to scripts/verify.sh (default: Rust lanes).
# For a task's final verification, prefer this entry point. During an edit loop,
# --keep-builds avoids recompiling dependencies. No worktrees or branches are removed.
#
# Fresh builds cover target/, client/rust/target/ and client/build/. Package caches
# are retained. Debug symbols and incremental compilation default off to reduce
# disk usage; explicit CARGO_PROFILE_*_DEBUG / CARGO_INCREMENTAL values override.
set -euo pipefail
# shellcheck source-path=SCRIPTDIR
# shellcheck source=lib/dev-common.sh
source "$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/lib/dev-common.sh"
cd "$CC_REPO_ROOT"

keep=0
if [ "${1:-}" = '--keep-builds' ]; then
  keep=1
  shift
fi
if [ "${1:-}" = '-h' ] || [ "${1:-}" = '--help' ]; then
  cc_usage_from_header "${BASH_SOURCE[0]}"
  exit 0
fi
for arg in "$@"; do
  case "$arg" in
    -p | --package) break ;; # Remaining flags belong to cargo test.
    -h | --help | --list) exec "$CC_REPO_ROOT/scripts/verify.sh" "$@" ;;
  esac
done

CC_VERIFY_CHECK_ARGS_ONLY=1 "$CC_REPO_ROOT/scripts/verify.sh" "$@"
lock_dir="$(git rev-parse --git-path cc-build.lock)"
mkdir "$lock_dir" 2>/dev/null || cc_die "$CC_EXIT_USAGE" \
  "another build.sh run owns '$lock_dir' (remove it only if that run has stopped)"
trap 'rmdir "$lock_dir"' EXIT

# Fail closed before removing anything: a local target-dir symlink or tracked
# file must not turn cache cleanup into deletion of shared output or source.
paths=(target client/rust/target client/build)
if [ "$keep" -eq 0 ]; then
  # Metadata resolves environment and Cargo config overrides. Only delete local
  # output, and reject a redirected build rather than claiming to have cleaned it.
  for workspace in "$CC_REPO_ROOT" "$CC_REPO_ROOT/client/rust"; do
    metadata="$(cc_capture_in_shell '' cargo \
      "cd $(cc_quote_args "$workspace") && cargo metadata --locked --no-deps --format-version 1")"
    expected="$workspace/target"
    case "$expected" in
      *\"* | *\\*) cc_die "$CC_EXIT_USAGE" 'use --keep-builds for paths requiring JSON escaping' ;;
    esac
    case "$metadata" in
      *"\"target_directory\":\"$expected\""*) ;;
      *) cc_die "$CC_EXIT_USAGE" "Cargo output is redirected for '$workspace'; use --keep-builds" ;;
    esac
  done
  for path in "${paths[@]}"; do
    parent="$path"
    while [ "$parent" != '.' ]; do
      if [ -L "$parent" ]; then
        cc_die "$CC_EXIT_USAGE" "refusing to clean symlink '$parent'; use --keep-builds"
      fi
      parent="$(dirname "$parent")"
    done
    tracked="$(git ls-files -- "$path")"
    if [ -n "$tracked" ] || ! git check-ignore -q -- "$path/"; then
      cc_die "$CC_EXIT_USAGE" "refusing to clean '$path': it must be ignored and contain no tracked files"
    fi
  done
  cc_info 'fresh build: removing previous target/, client/rust/target/ and client/build/'
  rm -rf -- "${paths[@]}"
else
  cc_info 'keeping existing build artifacts (--keep-builds)'
fi

export CARGO_PROFILE_DEV_DEBUG="${CARGO_PROFILE_DEV_DEBUG:-0}"
export CARGO_PROFILE_TEST_DEBUG="${CARGO_PROFILE_TEST_DEBUG:-0}"
export CARGO_INCREMENTAL="${CARGO_INCREMENTAL:-0}"
CC_VERIFY_CHECK_ARGS_ONLY=0 "$CC_REPO_ROOT/scripts/verify.sh" "$@"
