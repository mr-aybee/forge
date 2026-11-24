#!/usr/bin/env bash
# Assembles release assets from the built binaries.
#   bash scripts/packaging/package.sh <bin-dir> <out-dir> <version>
#
# <bin-dir> holds forge-server-<target>[.exe], forge-plugin-runner-<target>[.exe],
# forge-analytics-worker-<target>[.exe],
# forge-agent-linux-<arch>, forge-agent-helper-linux-<arch> and
# forge-agent-windows-x86_64.exe. <out-dir> gets:
#   * the raw binaries (the self-updater downloads these), and
#   * one self-contained package per server platform:
#       forge-<version>-x86_64-linux.tar.gz
#       forge-<version>-aarch64-linux.tar.gz
#       forge-<version>-x86_64-windows.zip (only when a Windows server build is
#       present; releases currently ship Windows as the forge CLI only)
#     each with the server, the plugin runner, the analytics worker, every agent binary for the
#     nodes (dist/), the installer and an example configuration.
set -euo pipefail
BIN="$1"; OUT="$2"; VERSION="$3"
# The version goes into MANIFEST.json unescaped: allow only version characters.
[[ "$VERSION" =~ ^[0-9A-Za-z][0-9A-Za-z.+~_-]*$ ]] || { echo "bad version: $VERSION" >&2; exit 2; }
HERE="$(cd "$(dirname "$0")" && pwd)"
mkdir -p "$OUT"
OUT="$(cd "$OUT" && pwd)"  # absolute: the zip step runs from a temp dir
cp "$BIN"/* "$OUT"/
cp "$HERE/../../LICENSE" "$OUT/LICENSE.txt"  # also a release asset of its own

agents=()
for a in forge-agent-linux-x86_64 forge-agent-helper-linux-x86_64 forge-agent-linux-aarch64 forge-agent-helper-linux-aarch64 forge-agent-windows-x86_64.exe; do
  [ -f "$BIN/$a" ] && agents+=("$a")
done

# MANIFEST.json: the version of the agents in dist/ and their SHA-256. The
# control plane offers the agents as this version (not its own) and drops a
# file whose hash does not match. Run inside dist/.
manifest() {
  local sep=""
  printf '{\n  "version": "%s",\n  "files": {' "$VERSION"
  for a in "${agents[@]}"; do
    printf '%s\n    "%s": "%s"' "$sep" "$a" "$(sha256sum "$a" | cut -d' ' -f1)"
    sep=","
  done
  printf '\n  }\n}\n'
}

stage() { # stage <dir> <server-target> <exe>
  local d="$1" t="$2" x="$3"
  mkdir -p "$d/dist"
  cp "$BIN/forge-server-$t$x" "$d/forge-server$x"
  cp "$BIN/forge-plugin-runner-$t$x" "$d/forge-plugin-runner$x"
  cp "$BIN/forge-analytics-worker-$t$x" "$d/forge-analytics-worker$x"
  for a in "${agents[@]}"; do cp "$BIN/$a" "$d/dist/$a"; done
  (cd "$d/dist" && sha256sum * > SHA256SUMS && manifest > MANIFEST.json)
  cp "$HERE/forge.toml.example" "$d/forge.toml.example"
  sed "s/@VERSION@/$VERSION/g" "$HERE/README.txt" > "$d/README.txt"
  cp "$HERE/../../LICENSE" "$d/LICENSE.txt"
  chmod 0755 "$d/forge-server$x" "$d/forge-plugin-runner$x" "$d/forge-analytics-worker$x" "$d"/dist/forge-agent* || true
}

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
for arch in x86_64 aarch64; do
  t="$arch-unknown-linux-musl"
  [ -f "$BIN/forge-server-$t" ] || continue
  name="forge-$VERSION-$arch-linux"
  stage "$tmp/$name" "$t" ""
  install -m 0755 "$HERE/install.sh" "$tmp/$name/install.sh"
  tar -C "$tmp" -czf "$OUT/$name.tar.gz" "$name"
done
t=x86_64-pc-windows-msvc
if [ -f "$BIN/forge-server-$t.exe" ]; then
  name="forge-$VERSION-x86_64-windows"
  stage "$tmp/$name" "$t" ".exe"
  cp "$HERE/install.ps1" "$tmp/$name/install.ps1"
  (cd "$tmp" && zip -qr "$OUT/$name.zip" "$name")
fi
ls -la "$OUT"
