#!/usr/bin/env bash
# End-to-end test of the Linux packages and installers in a clean systemd
# container (Debian 12 by default). It builds two release folders from
# x86_64 musl binaries, signs them with a throwaway key, serves them as a
# file:// mirror of GitHub Releases and then, inside the container:
#
#   1. refuses a tampered package (get-forge.sh): nothing is installed;
#   2. installs the first release with get-forge.sh --version (the GitHub path),
#      non-interactive, with a generated admin password and the agent on the
#      server itself; checks the console, API, agent downloads, the analytics
#      worker (DuckDB) on 55132, permissions and the MANIFEST version;
#   3. re-installs from the downloaded package (get-forge.sh --file);
#   4. upgrades to the "latest" release (the second one) and checks that
#      configuration, data and the administrator are kept;
#   5. uninstalls (keeps data), then uninstalls with --purge.
#
#   bash scripts/packaging/test-install.sh [image]            # debian:bookworm
#   bash scripts/packaging/test-install.sh rockylinux:9       # RHEL-like
#   bash scripts/packaging/test-install.sh --syntax-only      # no Docker needed
#
# A manual tool (not run in CI): run it before tagging, against the release
# dry run's packages:
#   gh run download <dry-run id> -n release-assets -D /tmp/rc
#   FORGE_TEST_ASSETS=/tmp/rc/assets bash scripts/packaging/test-install.sh rockylinux:9
# FORGE_TEST_ASSETS uses those packages and their real signature (the pinned
# key); the upgrade step then re-installs the same version.
#
# Binaries: FORGE_TEST_BIN (a folder with forge-server, forge-agent,
# forge-agent-helper, forge-plugin-runner, forge-analytics-worker and forge
# built for x86_64-unknown-linux-musl); default: the lab build
# ($CARGO_TARGET_DIR or ~/.cache/forge-target)/x86_64-unknown-linux-musl/release.
# DB=postgres also installs a local PostgreSQL with --db-local.
# DOCKER=... overrides the docker command (e.g. "sudo docker").
set -euo pipefail
cd "$(dirname "$0")/../.."
IMAGE="${1:-debian:bookworm}"
DOCKER="${DOCKER:-docker}"
P=scripts/packaging

echo "--- syntax and flag parsing (no root needed, changes nothing)"
for f in "$P/install.sh" "$P/get-forge.sh"; do sh -n "$f"; done
for f in "$P/package.sh" "$P/check-release.sh" "$P/release-notes.sh" "$0"; do bash -n "$f"; done
sh "$P/install.sh" --help | grep -q -- "--install-agent"
sh "$P/install.sh" --help | grep -q -- "--uninstall"
sh "$P/get-forge.sh" --help | grep -q -- "--version"
for bad in "--bogus" "--allow-elevated-tokens maybe" "--cluster a/b" "--terminal-account Bad"; do
  # shellcheck disable=SC2086
  if sh "$P/install.sh" $bad >/dev/null 2>&1; then echo "accepted: $bad" >&2; exit 1; fi
done
if sh "$P/get-forge.sh" --version 'v1;rm' >/dev/null 2>&1; then echo "get-forge accepted a bad version" >&2; exit 1; fi
key=$(sed -n 's/^pub const RELEASE_PUBLIC_KEY: &str = "\(.*\)";/\1/p' src/protocol/forge-sdk/src/release.rs)
grep -q "PINNED_KEY=\"$key\"" "$P/get-forge.sh" || { echo "get-forge.sh does not pin RELEASE_PUBLIC_KEY" >&2; exit 1; }
grep -q "\$PinnedKey = \"$key\"" "$P/get-forge.ps1" || { echo "get-forge.ps1 does not pin RELEASE_PUBLIC_KEY" >&2; exit 1; }
echo "flags and pinned keys: ok"
[ "${1:-}" = --syntax-only ] && exit 0

WORK="$(mktemp -d)"
# Unique names: several tests may share one Docker daemon.
SLUG="$(printf '%s' "$IMAGE" | tr -c 'a-z0-9' '-')"
C="forge-pkg-test-$SLUG-$$"
IMG="forge-pkg-test:$SLUG"
trap 'rm -rf "$WORK"; $DOCKER rm -f $C >/dev/null 2>&1 || true' EXIT

if [ -n "${FORGE_TEST_ASSETS:-}" ]; then
  # --- the dry run's signed packages, served as one release (also "latest")
  A="$(cd "$FORGE_TEST_ASSETS" && pwd)"
  pkg=$(cd "$A" && ls forge-*-x86_64-linux.tar.gz | head -1)
  [ -n "$pkg" ] || { echo "no forge-<version>-x86_64-linux.tar.gz in $A" >&2; exit 1; }
  V1=${pkg#forge-}; V1=${V1%-x86_64-linux.tar.gz}; V2=$V1
  TEST_PUB=""
  mkdir -p "$WORK/mirror/download" "$WORK/mirror/latest"
  cp -r "$A" "$WORK/mirror/download/v$V1"
  cp -r "$A" "$WORK/mirror/latest/download"
  sh "$P/get-forge.sh" --verify-only "$WORK/mirror/download/v$V1"
else
  BIN="${FORGE_TEST_BIN:-${CARGO_TARGET_DIR:-$HOME/.cache/forge-target}/x86_64-unknown-linux-musl/release}"
  for b in forge-server forge-agent forge-agent-helper forge-plugin-runner forge-analytics-worker; do
    [ -x "$BIN/$b" ] || { echo "missing $BIN/$b (set FORGE_TEST_BIN)" >&2; exit 1; }
  done
  # --- two releases signed with a throwaway key, in a GitHub-like mirror
  V1=0.0.1-test; V2=0.0.2-test
  t=x86_64-unknown-linux-musl
  mkdir -p "$WORK/bin"
  cp "$BIN/forge-server" "$WORK/bin/forge-server-$t"
  cp "$BIN/forge-plugin-runner" "$WORK/bin/forge-plugin-runner-$t"
  cp "$BIN/forge-analytics-worker" "$WORK/bin/forge-analytics-worker-$t"
  cp "$BIN/forge-agent" "$WORK/bin/forge-agent-linux-x86_64"
  cp "$BIN/forge-agent-helper" "$WORK/bin/forge-agent-helper-linux-x86_64"
  keys=$("$BIN/forge-server" release keygen)
  FORGE_RELEASE_KEY=$(printf '%s\n' "$keys" | sed -n 's/^private.*: *//p')
  TEST_PUB=$(printf '%s\n' "$keys" | sed -n 's/^public.*: *//p')
  unset keys
  for v in "$V1" "$V2"; do
    d="$WORK/mirror/download/v$v"
    bash "$P/package.sh" "$WORK/bin" "$d" "$v" >/dev/null
    install -m 0755 "$P/get-forge.sh" "$d/get-forge.sh"
    (cd "$d" && sha256sum -- * > SHA256SUMS && FORGE_RELEASE_KEY="$FORGE_RELEASE_KEY" "$BIN/forge-server" release sign SHA256SUMS > SHA256SUMS.sig)
  done
  unset FORGE_RELEASE_KEY
  mkdir -p "$WORK/mirror/latest" && cp -r "$WORK/mirror/download/v$V2" "$WORK/mirror/latest/download"
  FORGE_RELEASE_PUBLIC_KEY="$TEST_PUB" sh "$P/get-forge.sh" --verify-only "$WORK/mirror/download/v$V1"
fi
# A tampered copy of the first release: the package changes, the signed sums do not.
cp -r "$WORK/mirror/download/v$V1" "$WORK/mirror/download/v0.0.9-bad"
printf 'x' >> "$WORK/mirror/download/v0.0.9-bad/forge-$V1-x86_64-linux.tar.gz"
du -sh "$WORK/mirror"

# --- a clean systemd machine
$DOCKER build -q -t "$IMG" - >/dev/null <<EOF
FROM $IMAGE
ENV container=docker
RUN if command -v apt-get >/dev/null; then \
      apt-get update && apt-get install -y --no-install-recommends systemd systemd-sysv dbus curl ca-certificates && rm -rf /var/lib/apt/lists/*; \
    else \
      dnf install -y systemd dbus tar gzip procps-ng util-linux findutils && dnf clean all; \
    fi
CMD ["/sbin/init"]
EOF
$DOCKER rm -f $C >/dev/null 2>&1 || true
$DOCKER run -d --name $C --privileged --cgroupns=host -v /sys/fs/cgroup:/sys/fs/cgroup:rw \
  --tmpfs /run --tmpfs /run/lock "$IMG" >/dev/null
for _ in $(seq 60); do
  s=$($DOCKER exec $C systemctl is-system-running 2>/dev/null || true)
  [ "$s" = running ] || [ "$s" = degraded ] && break; sleep 1
done
$DOCKER cp "$WORK/mirror" $C:/root/mirror
$DOCKER cp "$P/get-forge.sh" $C:/root/get-forge.sh
key_env=(); [ -z "$TEST_PUB" ] || key_env=(-e FORGE_RELEASE_PUBLIC_KEY="$TEST_PUB")
run() { $DOCKER exec -e FORGE_RELEASES_URL=file:///root/mirror -e V1="$V1" -e V2="$V2" "${key_env[@]}" $C sh -c "$1"; }

echo "--- 1. a tampered package is refused before anything is installed"
if run 'sh /root/get-forge.sh --version v0.0.9-bad -- --yes' > "$WORK/bad.log" 2>&1; then
  cat "$WORK/bad.log"; echo "FAIL: the tampered package was installed" >&2; exit 1
fi
grep -q "does not match SHA256SUMS" "$WORK/bad.log" || { cat "$WORK/bad.log"; echo "FAIL: wrong refusal" >&2; exit 1; }
run 'test ! -e /opt/forge && test ! -e /etc/forge' || { echo "FAIL: files were installed" >&2; exit 1; }
echo "refused: $(grep -o '[^ ]* does not match SHA256SUMS' "$WORK/bad.log")"

echo "--- 2. fresh install from a release (get-forge.sh --version)"
run 'sh /root/get-forge.sh --version "v$V1" -- --yes --public-url http://localhost:8080 --admin-email admin@example.com \
       --environment production --mfa required --allow-elevated-tokens no \
       --install-agent --cluster default --terminal-account forge-ops '"${DB:+--db-local}" | tee "$WORK/install.log"
grep -q "is installed and running" "$WORK/install.log"
grep -q "NEXT STEPS" "$WORK/install.log"
grep -q "Saved to *\/var\/lib\/forge\/initial-admin-password" "$WORK/install.log"

echo "--- checks"
run '
  set -e
  curl -fsS http://127.0.0.1:8080/api/version; echo
  curl -s -o /tmp/i.html http://127.0.0.1:8080/; grep -q "<div id=\"root\"" /tmp/i.html || { echo "FAIL: console: embedded"; exit 1; }; echo "console: embedded"
  [ "$(curl -s -o /dev/null -w "%{http_code}" http://127.0.0.1:8080/api/nope)" = 404 ] || { echo "FAIL: unknown api path: 404"; exit 1; }; echo "unknown api path: 404"
  curl -fsS -o /dev/null http://127.0.0.1:8080/install.sh || { echo "FAIL: node installer: served"; exit 1; }; echo "node installer: served"
  curl -fsS -o /dev/null http://127.0.0.1:8080/agent/download/linux/x86_64/forge-agent || { echo "FAIL: agent binary: served from the package"; exit 1; }; echo "agent binary: served from the package"
  systemctl is-active forge-server
  grep -q "^allow_elevated_tokens = false" /etc/forge/forge.toml || { echo "FAIL: elevated tokens: off (production, MFA required)"; exit 1; }; echo "elevated tokens: off (production, MFA required)"
  grep -q "^engine = \"duckdb\"" /etc/forge/forge.toml || { echo "FAIL: analytics: duckdb (default)"; exit 1; }; echo "analytics: duckdb (default)"
  # No pgrep in minimal images: look through /proc.
  grep -qs forge-analytics-worker /proc/[0-9]*/cmdline || { echo "analytics worker: NOT running"; exit 1; }
  echo "analytics worker: running"
  rc=0; curl -sk -o /dev/null --max-time 3 https://127.0.0.1:55132/ || rc=$?
  [ "$rc" != 7 ] && [ "$rc" != 28 ] || { echo "analytics upload port 55132: NOT listening (curl rc $rc)"; exit 1; }
  echo "analytics upload port 55132: listening (curl rc $rc, client certificate required)"
  grep -q "\"version\": \"$V1\"" /opt/forge/dist/MANIFEST.json || { echo "FAIL: agent folder: MANIFEST.json $V1"; exit 1; }; echo "agent folder: MANIFEST.json $V1"
  [ "$(stat -c "%a %U" /var/lib/forge/initial-admin-password)" = "600 root" ] || { echo "FAIL: admin password file: 600 root"; exit 1; }; echo "admin password file: 600 root"
  grep -q "^password: ..*" /var/lib/forge/initial-admin-password
  [ "$(stat -c "%a %U" /var/lib/forge)" = "700 forge" ] || { echo "FAIL: data dir: 700 forge"; exit 1; }; echo "data dir: 700 forge"
  [ "$(stat -c "%a %U" /etc/forge/install.sh)" = "700 root" ] || { echo "FAIL: uninstaller copy: 700 root"; exit 1; }; echo "uninstaller copy: 700 root"
  if systemctl is-active -q forge-agent; then echo "agent on the server: active"; else echo "agent on the server: NOT active"; exit 1; fi
'

echo "--- 3. re-install from a local package (get-forge.sh --file)"
run 'mkdir -p /root/pkg && cp "/root/mirror/download/v$V1/forge-$V1-x86_64-linux.tar.gz" /root/mirror/download/v$V1/SHA256SUMS* /root/pkg/ \
     && sh /root/get-forge.sh --file "/root/pkg/forge-$V1-x86_64-linux.tar.gz" -- --yes' | tee "$WORK/reinstall.log" | tail -4
grep -q "Configuration *\/etc\/forge\/forge.toml (kept)" "$WORK/reinstall.log"
grep -q "existing accounts kept" "$WORK/reinstall.log"
run 'systemctl is-active forge-server && systemctl is-active forge-agent'

echo "--- 4. upgrade to the latest release"
run 'sh /root/get-forge.sh -- --yes' | tee "$WORK/upgrade.log" | tail -4
grep -q "Downloading *forge-$V2-x86_64-linux.tar.gz" "$WORK/upgrade.log"
grep -q "upgraded to" "$WORK/upgrade.log"
run '
  set -e
  grep -q "\"version\": \"$V2\"" /opt/forge/dist/MANIFEST.json || { echo "FAIL: agent folder: MANIFEST.json $V2"; exit 1; }; echo "agent folder: MANIFEST.json $V2"
  test -f /var/lib/forge/forge.db || { echo "FAIL: database: kept"; exit 1; }; echo "database: kept"
  grep -q "^engine = \"duckdb\"" /etc/forge/forge.toml || { echo "FAIL: configuration: kept"; exit 1; }; echo "configuration: kept"
  curl -fsS -o /dev/null http://127.0.0.1:8080/api/health || { echo "FAIL: health: ok"; exit 1; }; echo "health: ok"
'

echo "--- 5. uninstall"
run 'sh /etc/forge/install.sh --uninstall --yes' | tail -3
run '! systemctl is-active -q forge-server && test ! -e /opt/forge && test -f /var/lib/forge/forge.db && test -f /etc/forge/forge.toml' \
  || { echo "FAIL: uninstall" >&2; exit 1; }
echo "uninstalled; configuration and data kept"
run 'sh /etc/forge/install.sh --uninstall --purge --yes' | tail -2
run 'test ! -e /etc/forge && test ! -e /var/lib/forge && ! id forge >/dev/null 2>&1' || { echo "FAIL: purge" >&2; exit 1; }
echo "purged"
echo PASS
