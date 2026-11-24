#!/usr/bin/env bash
# Checks assembled release assets before they are signed: every asset the
# installers and updaters expect is there, each package has the layout
# install.sh / install.ps1 need, the agent folder's MANIFEST.json names this
# version, and packaged binaries equal the raw ones the updater downloads.
#   bash scripts/packaging/check-release.sh <assets-dir> <version>
set -euo pipefail
A="$1"; VERSION="$2"
fail=0
err() { echo "::error::$*"; fail=1; }
have() { [ -f "$A/$1" ] || err "missing asset $1"; }
same() { # same <file-a> <file-b>
  [ "$(sha256sum < "$1")" = "$(sha256sum < "$2")" ] || err "$2 differs from $1"
}

for f in LICENSE.txt get-forge.sh get-forge.ps1; do have "$f"; done

linux=0
for arch in x86_64 aarch64; do
  t="$arch-unknown-linux-musl"
  [ -f "$A/forge-server-$t" ] || continue
  linux=$((linux + 1))
  for f in "forge-server-$t" "forge-plugin-runner-$t" "forge-analytics-worker-$t" "forge-cli-$t" \
           "forge-agent-linux-$arch" "forge-agent-helper-linux-$arch" "forge-$VERSION-$arch-linux.tar.gz"; do
    have "$f"
  done
  name="forge-$VERSION-$arch-linux"
  tmp=$(mktemp -d)
  tar -xzf "$A/$name.tar.gz" -C "$tmp"
  d="$tmp/$name"
  [ -d "$d" ] || { err "$name.tar.gz does not unpack to $name/"; continue; }
  for f in install.sh forge-server forge-plugin-runner forge-analytics-worker forge.toml.example README.txt LICENSE.txt \
           dist/MANIFEST.json dist/SHA256SUMS dist/forge-agent-linux-x86_64 dist/forge-agent-linux-aarch64 \
           dist/forge-agent-helper-linux-x86_64 dist/forge-agent-helper-linux-aarch64; do
    [ -f "$d/$f" ] || err "$name.tar.gz has no $f"
  done
  for f in install.sh forge-server forge-plugin-runner forge-analytics-worker; do
    [ -x "$d/$f" ] || err "$name/$f is not executable"
  done
  same "$A/forge-server-$t" "$d/forge-server"
  same "$A/forge-plugin-runner-$t" "$d/forge-plugin-runner"
  same "$A/forge-analytics-worker-$t" "$d/forge-analytics-worker"
  (cd "$d/dist" && sha256sum --quiet -c SHA256SUMS) || err "$name/dist/SHA256SUMS does not match its files"
  v=$(sed -n 's/^ *"version": *"\(.*\)",*$/\1/p' "$d/dist/MANIFEST.json")
  [ "$v" = "$VERSION" ] || err "$name/dist/MANIFEST.json names version '$v', expected $VERSION"
  grep -q "$VERSION" "$d/README.txt" || err "$name/README.txt does not name $VERSION"
  rm -rf "$tmp"
done
[ "$linux" = 2 ] || err "expected both Linux packages (x86_64 and aarch64), found $linux"

t=x86_64-pc-windows-msvc
if [ -f "$A/forge-server-$t.exe" ]; then
  for f in "forge-plugin-runner-$t.exe" "forge-analytics-worker-$t.exe" "forge-cli-$t.exe" \
           forge-agent-windows-x86_64.exe "forge-$VERSION-x86_64-windows.zip"; do
    have "$f"
  done
  listing=$(unzip -Z1 "$A/forge-$VERSION-x86_64-windows.zip")
  for f in install.ps1 forge-server.exe forge-plugin-runner.exe forge-analytics-worker.exe dist/MANIFEST.json dist/forge-agent-windows-x86_64.exe; do
    grep -qx "forge-$VERSION-x86_64-windows/$f" <<<"$listing" || err "the Windows zip has no $f"
  done
else
  # Windows ships the forge CLI only.
  have "forge-cli-$t.exe"
fi

# Every forge-agent-* asset is copied into agent folders by the updater:
# nothing else may use that prefix.
for f in "$A"/forge-agent-*; do
  case "$(basename "$f")" in
    forge-agent-linux-x86_64|forge-agent-linux-aarch64|forge-agent-helper-linux-x86_64|forge-agent-helper-linux-aarch64|forge-agent-windows-x86_64.exe) ;;
    *) err "unexpected asset $(basename "$f"): the updater installs every forge-agent-* asset on nodes" ;;
  esac
done

[ "$fail" = 0 ] || exit 1
echo "release assets for $VERSION: ok"
ls -la "$A"
