#!/usr/bin/env bash
# Exercise the real build wrapper in an isolated checkout, with verification stubbed.
set -euo pipefail
repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
fixture="$(mktemp -d)"
trap 'rm -rf "$fixture"' EXIT
mkdir -p "$fixture/repo/scripts/lib"
mkdir -p "$fixture/bin"
cat >"$fixture/bin/cargo" <<'STUB'
#!/usr/bin/env bash
if [ -f .redirected-target ] || [ -n "${CARGO_TARGET_DIR:-}" ]; then
  printf '{"target_directory":"/external/cache"}\n'
else
  printf '{"target_directory":"%s/target"}\n' "$PWD"
fi
STUB
chmod +x "$fixture/bin/cargo"
export PATH="$fixture/bin:$PATH"
unset CC_FORCE_NIX CARGO_TARGET_DIR CARGO_BUILD_TARGET_DIR
cp "$repo_root/scripts/build.sh" "$fixture/repo/scripts/build.sh"
cp "$repo_root/scripts/lib/dev-common.sh" "$fixture/repo/scripts/lib/"
git -C "$fixture/repo" init -q
printf '/target/\n/client/rust/target/\n/client/build/\n' >"$fixture/repo/.gitignore"
cat >"$fixture/repo/scripts/verify.sh" <<'STUB'
#!/usr/bin/env bash
if [ "${CC_VERIFY_CHECK_ARGS_ONLY:-0}" = 1 ]; then
  [ "${1:-}" != '--invalid' ]
  exit $?
fi
: >"$CC_TEST_RECORD"
if [ "$#" -gt 0 ]; then printf '%s\n' "$@" >"$CC_TEST_RECORD"; fi
printf '%s:%s:%s\n' "$CARGO_PROFILE_DEV_DEBUG" "$CARGO_PROFILE_TEST_DEBUG" "$CARGO_INCREMENTAL" >>"$CC_TEST_RECORD"
exit "${CC_TEST_EXIT:-0}"
STUB
chmod +x "$fixture/repo/scripts/verify.sh"
export CC_TEST_RECORD="$fixture/args"
unset CARGO_PROFILE_DEV_DEBUG CARGO_PROFILE_TEST_DEBUG CARGO_INCREMENTAL
seed() {
  local path
  for path in target client/rust/target client/build; do
    mkdir -p "$fixture/repo/$path"
    touch "$fixture/repo/$path/old-build"
  done
  printf 'source\n' >"$fixture/repo/source.rs"
}
run() { bash "$fixture/repo/scripts/build.sh" "$@"; }
seed
run --fast
[ ! -e "$fixture/repo/target/old-build" ]
[ ! -e "$fixture/repo/client/rust/target/old-build" ]
[ ! -e "$fixture/repo/client/build/old-build" ]
[ -f "$fixture/repo/source.rs" ]
[ "$(cat "$CC_TEST_RECORD")" = "$(printf '%s\n' --fast 0:0:0)" ]
run --fast # A second fresh run must also work when outputs do not exist yet.
seed
touch "$fixture/repo/.redirected-target"
status=0
run || status=$?
[ "$status" -ne 0 ]
[ -f "$fixture/repo/target/old-build" ]
rm "$fixture/repo/.redirected-target"
touch "$fixture/repo/client/rust/.redirected-target"
status=0
run || status=$?
[ "$status" -ne 0 ]
[ -f "$fixture/repo/target/old-build" ]
rm "$fixture/repo/client/rust/.redirected-target"
status=0
CARGO_TARGET_DIR="$fixture/external" run || status=$?
[ "$status" -ne 0 ]
[ -f "$fixture/repo/target/old-build" ]
run --keep-builds -p core 'a filter with spaces'
[ -f "$fixture/repo/target/old-build" ]
[ "$(cat "$CC_TEST_RECORD")" = "$(printf '%s\n' -p core 'a filter with spaces' 0:0:0)" ]
run --help
[ -f "$fixture/repo/target/old-build" ]
run --fast --help
[ -f "$fixture/repo/target/old-build" ]
run --list
[ -f "$fixture/repo/target/old-build" ]
status=0
run --invalid || status=$?
[ "$status" -ne 0 ]
[ -f "$fixture/repo/target/old-build" ]
mkdir "$fixture/repo/.git/cc-build.lock"
status=0
run || status=$?
[ "$status" -ne 0 ]
[ -f "$fixture/repo/target/old-build" ]
rmdir "$fixture/repo/.git/cc-build.lock"
status=0
CC_TEST_EXIT=13 run --keep-builds || status=$?
[ "$status" -eq 13 ]
CARGO_PROFILE_DEV_DEBUG=2 CARGO_INCREMENTAL=1 run --keep-builds
[ "$(cat "$CC_TEST_RECORD")" = '2:0:1' ]
# Refuse an unsafe tree before deleting any of the three directories.
git -C "$fixture/repo" add -f client/build/old-build
status=0
run || status=$?
[ "$status" -ne 0 ]
[ -f "$fixture/repo/target/old-build" ]
git -C "$fixture/repo" reset -q
mkdir "$fixture/external"
touch "$fixture/external/preserve"
rm -rf "$fixture/repo/client/rust/target"
ln -s "$fixture/external" "$fixture/repo/client/rust/target"
status=0
run || status=$?
[ "$status" -ne 0 ]
[ -f "$fixture/external/preserve" ]
[ -f "$fixture/repo/target/old-build" ]
# Exercise the real verifier's parser too: preflight must neither build nor
# create its target/verify-gitconfig, including in single-crate mode.
cp "$repo_root/scripts/verify.sh" "$fixture/repo/scripts/verify.sh"
rm -rf "$fixture/repo/target"
CC_VERIFY_CHECK_ARGS_ONLY=1 bash "$fixture/repo/scripts/verify.sh" --fast
[ ! -e "$fixture/repo/target" ]
CC_VERIFY_CHECK_ARGS_ONLY=1 bash "$fixture/repo/scripts/verify.sh" -p core 'filter with spaces'
[ ! -e "$fixture/repo/target" ]
for args in '--invalid' '--update' '-p' '-p invalid-crate'; do
  status=0
  # shellcheck disable=SC2086 # Deliberate word splitting of fixed fixture args.
  CC_VERIFY_CHECK_ARGS_ONLY=1 bash "$fixture/repo/scripts/verify.sh" $args || status=$?
  [ "$status" -eq 2 ]
  [ ! -e "$fixture/repo/target" ]
done
echo 'build artifact wrapper tests passed'
