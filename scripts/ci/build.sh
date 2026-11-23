#!/usr/bin/env bash
# Runs a build command gently on a self-hosted runner, under the host-wide
# build lock (both Linux runners share one server):
#
#   scripts/ci/build.sh light <command...>   # cargo test, npm run build, ...
#   scripts/ci/build.sh heavy <command...>   # release builds (cross, docker)
#
# - light takes the lock shared: two light jobs may compile together.
# - heavy takes it exclusive: it waits until no light or heavy build runs,
#   and nothing else compiles while it runs. Waiting jobs only sleep.
# - The command runs under `nice -n 10` and `ionice -c3` (idle I/O class).
# - heavy also samples host memory every 15 s and prints the peak, so a
#   run's log shows how close the build came to the machine's limit.
#
# On GitHub-hosted runners (and Windows, which has no flock) it just runs
# the command. Tunables (repository variables passed in as env):
#   FORGE_CI_LOCK_FILE   lock path shared by the runners (default /tmp/forge-ci-build.lock)
#   FORGE_CI_LOCK_WAIT   seconds to wait for the lock (default 10800)
set -uo pipefail

kind=${1:?usage: build.sh light|heavy <command...>}; shift
case "$kind" in light|heavy) ;; *) echo "usage: build.sh light|heavy <command...>" >&2; exit 2 ;; esac
[ $# -gt 0 ] || { echo "build.sh: no command" >&2; exit 2; }

gentle=()
command -v nice >/dev/null 2>&1 && gentle+=(nice -n 10)
command -v ionice >/dev/null 2>&1 && gentle+=(ionice -c3)

if [ "${RUNNER_ENVIRONMENT:-}" = "github-hosted" ] || ! command -v flock >/dev/null 2>&1; then
  exec "${gentle[@]}" "$@"
fi

lock=${FORGE_CI_LOCK_FILE:-/tmp/forge-ci-build.lock}
wait_s=${FORGE_CI_LOCK_WAIT:-10800}
mode=-s; [ "$kind" = heavy ] && mode=-x

exec 9>>"$lock" || { echo "::error::cannot open the build lock $lock; set FORGE_CI_LOCK_FILE to a path both runners can write"; exit 1; }
start=$(date +%s)
if ! flock -n "$mode" 9; then
  echo "Waiting for the host build lock ($kind; another build is running) ..."
  flock -w "$wait_s" "$mode" 9 || { echo "::error::waited ${wait_s}s for the host build lock $lock; check for a stuck build on the runner host"; exit 1; }
  echo "Got the build lock after $(( $(date +%s) - start ))s"
fi

mem_line() { free -m 2>/dev/null | awk '/^Mem:/ { printf "used %d MB of %d MB, available %d MB", $3, $2, $7 }'; }
sampler=""
peak_file=""
if [ "$kind" = heavy ]; then
  echo "Host before the build: $(mem_line)"
  peak_file=$(mktemp)
  ( peak=0; while :; do
      u=$(free -m 2>/dev/null | awk '/^Mem:/ { print $3 }'); u=${u:-0}
      [ "$u" -gt "$peak" ] && { peak=$u; echo "$peak" > "$peak_file"; }
      sleep 15
    done ) &
  sampler=$!
fi

"${gentle[@]}" "$@"
rc=$?

if [ -n "$sampler" ]; then
  kill "$sampler" 2>/dev/null; wait "$sampler" 2>/dev/null
  echo "Host memory peak during the build: $(cat "$peak_file" 2>/dev/null || echo ?) MB used (sampled every 15 s); now: $(mem_line)"
  rm -f "$peak_file"
fi
exit "$rc"
