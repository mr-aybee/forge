#!/usr/bin/env bash
# Builds and publishes one local test release into scripts/setup_testing/releases/<tag>/.
# Run through publish.py (Linux, or Windows via WSL) or directly from the repository root:
#
#   bash scripts/setup_testing/build_release.sh <version> [notes-file]
#
# FORGE_ARCHES="x86_64 aarch64" also builds the arm64 assets, with `cross`
# (cargo install cross --locked; needs Docker). Default: x86_64 only.
#
# Produces the same assets as the GitHub release workflow (raw binaries with
# target names, the self-contained tar.gz, SHA256SUMS, SHA256SUMS.sig), signed
# with a local dev key in scripts/setup_testing/.keys/ (created on first run).
# The package is built, hashed and signed in a WSL-local temp directory
# (the repository usually sits on /mnt/d, a slow 9P mount), then copied onto
# the repository in one go: into releases/.staging-<tag>, renamed into place
# last, so the feed never serves a half-published release.
set -euo pipefail
cd "$(dirname "$0")/../.."
command -v cargo >/dev/null || { [ -f "$HOME/.cargo/env" ] && . "$HOME/.cargo/env"; }
command -v cargo >/dev/null || {
  echo "build_release: no Rust toolchain in this WSL. Run once:" >&2
  echo "  wsl -d Ubuntu-24.04 -u root -- bash scripts/lab/setup-wsl.sh <wsl-user>" >&2
  exit 1
}

VERSION="${1:?usage: build_release.sh <version> [notes-file]}"
NOTES_FILE="${2:-}"
TAG="v$VERSION"
HERE="scripts/setup_testing"
REL="$HERE/releases"
KEYS="$HERE/.keys"
TARGET=x86_64-unknown-linux-musl
ARCHES="${FORGE_ARCHES:-x86_64}"
die() { echo "build_release: $*" >&2; exit 1; }

[ -e "$REL/$TAG" ] && die "$REL/$TAG already exists"

# FORGE_QUICK=1 (publish.py's default): a lab build for testing behaviour —
# opt-level 1, no LTO, many codegen units, incremental — in its own cache so
# quick and full builds never invalidate each other. Release workflows and
# `publish.py --full` keep the real optimisation below.
if [ "${FORGE_QUICK:-0}" = 1 ]; then
    export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$HOME/.cache/forge-target-quick}"
    export CARGO_PROFILE_RELEASE_OPT_LEVEL=1
    export CARGO_PROFILE_RELEASE_LTO=off
    export CARGO_PROFILE_RELEASE_CODEGEN_UNITS=256
    export CARGO_PROFILE_RELEASE_INCREMENTAL=true
fi
# Same build settings as scripts/lab/build.sh (shared cache, low memory).
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$HOME/.cache/forge-target}"
# One job per core; with 12 GB in WSL that fits (each rustc needs ~1.5 GB at
# opt-level 3, less for quick builds).
# Faster linking for local WSL builds: LLD instead of the default GNU linker
# (same output, much quicker link step). Local scripts only; CI and the GitHub
# release workflow are unchanged. Skipped when lld is not installed.
if [ -z "${CARGO_TARGET_X86_64_UNKNOWN_LINUX_MUSL_RUSTFLAGS:-}" ] && command -v ld.lld >/dev/null 2>&1; then
    export CARGO_TARGET_X86_64_UNKNOWN_LINUX_MUSL_RUSTFLAGS="-C link-arg=-fuse-ld=lld"
fi
export CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-$(nproc)}"
export CARGO_PROFILE_RELEASE_LTO="${CARGO_PROFILE_RELEASE_LTO:-thin}"
export CARGO_PROFILE_RELEASE_CODEGEN_UNITS="${CARGO_PROFILE_RELEASE_CODEGEN_UNITS:-16}"
# Compiled into forge-server and forge-agent: the updater compares it.
export FORGE_BUILD_VERSION="$VERSION"
export FORGE_BUILD_SHA="$(git rev-parse --short HEAD 2>/dev/null || echo unknown)"

echo "==> Building $TAG ($TARGET)"
cargo build --release --target "$TARGET" -p forge-server -p forge-agent -p forge-plugin-runner -p forge-analytics-worker
# The `forge` CLI separately, as in release.yml: its features stay out of the
# server and agent binaries.
cargo build --release --target "$TARGET" -p forge-cli

CROSS_DIR="$CARGO_TARGET_DIR-cross"
for arch in $ARCHES; do
  case "$arch" in
    x86_64) ;;
    aarch64)
      command -v cross >/dev/null || die "aarch64 needs cross: cargo install cross --locked (and Docker)"
      echo "==> Building $TAG (aarch64-unknown-linux-musl, cross)"
      # cross runs cargo in a container with its own linker and an older
      # glibc: its own target dir, or build scripts compiled on the host fail
      # to run in it ("GLIBC_2.xx not found"). The host's lld flags are for
      # x86_64 only.
      CARGO_TARGET_DIR="$CROSS_DIR" RUSTC_WRAPPER="" cross build --release --target aarch64-unknown-linux-musl -p forge-server -p forge-agent -p forge-plugin-runner -p forge-analytics-worker
      CARGO_TARGET_DIR="$CROSS_DIR" RUSTC_WRAPPER="" cross build --release --target aarch64-unknown-linux-musl -p forge-cli ;;
    *) die "unknown architecture $arch (x86_64, aarch64)" ;;
  esac
done

out="$CARGO_TARGET_DIR/$TARGET/release"
work="$(mktemp -d)"
bin="$work/bin"
stage="$work/$TAG"
final_stage="$REL/.staging-$TAG"
trap 'rm -rf "$work" "$final_stage"' EXIT
mkdir -p "$bin"
# Asset names as in .github/workflows/release.yml.
cp "$out/forge-server" "$bin/forge-server-$TARGET"
cp "$out/forge-plugin-runner" "$bin/forge-plugin-runner-$TARGET"
cp "$out/forge-analytics-worker" "$bin/forge-analytics-worker-$TARGET"
cp "$out/forge-agent" "$bin/forge-agent-linux-x86_64"
cp "$out/forge-agent-helper" "$bin/forge-agent-helper-linux-x86_64"
cp "$out/forge" "$bin/forge-cli-$TARGET"
if [[ " $ARCHES " == *" aarch64 "* ]]; then
  a="$CROSS_DIR/aarch64-unknown-linux-musl/release"
  cp "$a/forge-server" "$bin/forge-server-aarch64-unknown-linux-musl"
  cp "$a/forge-plugin-runner" "$bin/forge-plugin-runner-aarch64-unknown-linux-musl"
  cp "$a/forge-analytics-worker" "$bin/forge-analytics-worker-aarch64-unknown-linux-musl"
  cp "$a/forge-agent" "$bin/forge-agent-linux-aarch64"
  cp "$a/forge-agent-helper" "$bin/forge-agent-helper-linux-aarch64"
  cp "$a/forge" "$bin/forge-cli-aarch64-unknown-linux-musl"
fi
chmod 0755 "$bin"/*
server="$bin/forge-server-$TARGET"
[ "$("$server" --version | awk '{print $2}')" = "$VERSION" ] || die "built server does not report $VERSION"

echo "==> Packaging"
bash scripts/packaging/package.sh "$bin" "$stage" "$VERSION" >/dev/null

echo "==> Signing with the local dev key"
mkdir -p "$KEYS"
chmod 0700 "$KEYS" 2>/dev/null || true
if [ ! -s "$KEYS/release.key" ]; then
  "$server" release keygen | awk -F': ' '/^private/{print $2}' > "$KEYS/release.key"
  [ -s "$KEYS/release.key" ] || die "could not generate a signing key"
  echo "    new dev signing key → $KEYS/release.key"
fi
chmod 0600 "$KEYS/release.key" 2>/dev/null || true
FORGE_RELEASE_KEY="$(cat "$KEYS/release.key")"
export FORGE_RELEASE_KEY
"$server" release pubkey > "$KEYS/release.pub"
(cd "$stage" && sha256sum -- * > SHA256SUMS && "$server" release sign SHA256SUMS > SHA256SUMS.sig)
unset FORGE_RELEASE_KEY

notes=""
[ -n "$NOTES_FILE" ] && [ -f "$NOTES_FILE" ] && notes="$(cat "$NOTES_FILE")"
NOTES="$notes" TAG="$TAG" python3 - "$stage/release.json" <<'PY'
import json, os, sys
from datetime import datetime, timezone
now = datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")
doc = {"tag_name": os.environ["TAG"], "name": os.environ["TAG"], "body": os.environ["NOTES"], "published_at": now}
with open(sys.argv[1], "w", encoding="utf-8") as f:
    json.dump(doc, f, indent=2)
PY

echo "==> Copying to $REL/$TAG"
mkdir -p "$REL"
rm -rf "$final_stage"
cp -r "$stage" "$final_stage"
mv "$final_stage" "$REL/$TAG"
trap 'rm -rf "$work"' EXIT
echo "==> Published $TAG"
ls -lh "$REL/$TAG"
echo "forge CLI (forge-cli-$TARGET): $(du -h "$REL/$TAG/forge-cli-$TARGET" | cut -f1)"
echo
echo "Release public key (put it in [update] public_key on the Forge server):"
cat "$KEYS/release.pub"
