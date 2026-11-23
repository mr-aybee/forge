#!/usr/bin/env bash
# Builds static musl binaries for the lab and stages them in scripts/lab/.lab/.
#   wsl -d Ubuntu-24.04 -- bash scripts/lab/build.sh
set -euo pipefail
cd "$(dirname "$0")/../.."
# `wsl -- bash script` runs a non-login shell without rustup's PATH.
command -v cargo >/dev/null || . "$HOME/.cargo/env"
LAB="scripts/lab/.lab"
TARGET=x86_64-unknown-linux-musl
# Keep build artefacts on the Linux filesystem: much faster than /mnt/<drive>.
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$HOME/.cache/forge-target}"
# Keep the lab build within a small WSL memory cap: fat LTO with many
# parallel jobs can use several GB. Release/CI builds keep the full profile.
# Faster linking for local WSL builds: LLD instead of the default GNU linker
# (same output, much quicker link step). Local scripts only; CI and the GitHub
# release workflow are unchanged. Skipped when lld is not installed.
if [ -z "${CARGO_TARGET_X86_64_UNKNOWN_LINUX_MUSL_RUSTFLAGS:-}" ] && command -v ld.lld >/dev/null 2>&1; then
    export CARGO_TARGET_X86_64_UNKNOWN_LINUX_MUSL_RUSTFLAGS="-C link-arg=-fuse-ld=lld"
fi
export CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-$(nproc)}"
export CARGO_PROFILE_RELEASE_LTO="${CARGO_PROFILE_RELEASE_LTO:-thin}"
export CARGO_PROFILE_RELEASE_CODEGEN_UNITS="${CARGO_PROFILE_RELEASE_CODEGEN_UNITS:-16}"

cargo build --release --target "$TARGET" \
  -p forge-server -p forge-agent -p forge-plugin-runner -p forge-analytics-worker

out="$CARGO_TARGET_DIR/$TARGET/release"
mkdir -p "$LAB/bin" "$LAB/dist"
# Copy then rename: replacing a binary the running lab is executing fails
# with "Text file busy", while a rename swaps it atomically.
put() { cp "$1" "$2.new" && mv -f "$2.new" "$2"; }
put "$out/forge-server" "$LAB/bin/forge-server"
put "$out/forge-plugin-runner" "$LAB/bin/forge-plugin-runner"
put "$out/forge-analytics-worker" "$LAB/bin/forge-analytics-worker"
put "$out/forge-agent" "$LAB/dist/forge-agent-linux-x86_64"
put "$out/forge-agent-helper" "$LAB/dist/forge-agent-helper-linux-x86_64"
(cd "$LAB/dist" && sha256sum forge-agent-* > SHA256SUMS)
ls -lh "$LAB/bin" "$LAB/dist"
