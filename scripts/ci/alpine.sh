#!/usr/bin/env bash
# Native musl release builds in a pinned Alpine container, and the prebuilt
# DuckDB artifact that lets release builds skip compiling DuckDB's bundled
# C++ (D-166). Used by release.yml and duckdb-prebuilt.yml; run from the
# repository root on a Linux runner whose architecture matches <target>.
#
#   scripts/ci/alpine.sh build   <target> [--no-cli]  # release cargo builds
#   scripts/ci/alpine.sh ref     <target>             # print the GHCR reference
#   scripts/ci/alpine.sh restore <target>             # pull + unpack; never fails
#   scripts/ci/alpine.sh pack    <target> <file>      # tar the reusable outputs
#
# Both workflows build with the same image, the same in-container paths
# (/work, /cargo, /work/target/alpine) and the same first cargo command, so
# Cargo's fingerprints for libduckdb-sys match and it is not rebuilt.
#
# Env: FORGE_CI_BUILD_JOBS (default 4), FORGE_CI_CONTAINER_OPTS (docker run
# options, e.g. a memory cap), FORGE_BUILD_SHA / FORGE_BUILD_VERSION (passed
# through), FORGE_DUCKDB_REPO (default ghcr.io/orbyntech/forge-duckdb).
set -euo pipefail

# Pinned by digest. Update it here only; the next release then compiles
# DuckDB once until duckdb-prebuilt.yml has published for the new rustc.
IMAGE="rust:1.100.0-alpine3.22@sha256:f8ddfd4b265d9a9e7fd7c36821af4a55771aee2ac3894294687a66c709319be9"
REPO="${FORGE_DUCKDB_REPO:-ghcr.io/orbyntech/forge-duckdb}"
SERVER_PKGS=(-p forge-server -p forge-agent -p forge-plugin-runner -p forge-analytics-worker -p forge-dns)

cmd=${1:?usage: alpine.sh build|ref|restore|pack <target> ...}
target=${2:?usage: alpine.sh $cmd <target>}
case "$target" in x86_64-unknown-linux-musl|aarch64-unknown-linux-musl) ;; *)
  echo "::error::alpine.sh: unsupported target $target"; exit 2 ;; esac

host_target="$PWD/target/alpine"
cargo_home="${RUNNER_TEMP:-/tmp}/cargo-alpine"

# Runs a shell script in the container: source at /work, CARGO_HOME=/cargo.
# RUSTUP_TOOLCHAIN pins the image's rustc over rust-toolchain.toml ("stable").
in_container() {
  mkdir -p "$host_target" "$cargo_home"
  # shellcheck disable=SC2086 # options are a word list
  docker run --rm ${FORGE_CI_CONTAINER_OPTS:-} \
    -v "$PWD":/work -w /work -v "$cargo_home":/cargo \
    -e CARGO_HOME=/cargo -e CARGO_TARGET_DIR=/work/target/alpine \
    -e CARGO_TERM_COLOR=always -e RUSTC_WRAPPER= \
    -e FORGE_BUILD_SHA -e FORGE_BUILD_VERSION \
    -e T="$target" -e JOBS="${FORGE_CI_BUILD_JOBS:-4}" -e OWNER="$(id -u):$(id -g)" \
    "$IMAGE" sh -euc 'export RUSTUP_TOOLCHAIN="$RUST_VERSION"; '"$1"
}

duckdb_version() {
  awk '/^name = "libduckdb-sys"$/ { getline; gsub(/version = |"/, ""); print; exit }' Cargo.lock
}

rustc_version() { docker run --rm "$IMAGE" sh -c 'RUSTUP_TOOLCHAIN="$RUST_VERSION" rustc -V'; }

ref() {
  local d r
  d=$(duckdb_version); [ -n "$d" ] || { echo "::error::libduckdb-sys not found in Cargo.lock"; exit 1; }
  r=$(rustc_version | awk '{print $2}')
  echo "$REPO:$d-$r-$target"
}

case "$cmd" in
  build)
    cli=true; [ "${3:-}" = --no-cli ] && cli=false
    in_container '
      trap "chown -R \"$OWNER\" /work/target/alpine /cargo" EXIT
      apk add -q --no-cache build-base g++ perl linux-headers
      rustc -vV
      cargo fetch --locked --target "$T"
      # Crate sources get fresh mtimes on every extraction; build scripts
      # with rerun-if-changed would then rerun (DuckDB too). Their content
      # is checksum-pinned by Cargo.lock, so an old mtime is truthful.
      find /cargo/registry/src -exec touch -h -d "2000-01-01 00:00:00" {} +
      cargo build -j "$JOBS" --locked --release --target "$T" '"${SERVER_PKGS[*]}"'
      if [ '"$cli"' = true ]; then
        # The CLI separately: its keyring features stay out of the server.
        cargo build -j "$JOBS" --locked --release --target "$T" -p forge-cli
      fi'
    ;;
  ref) ref ;;
  pack)
    out=${3:?usage: alpine.sh pack <target> <file>}
    [ -d "$host_target/$target/release" ] || { echo "::error::nothing built in $host_target"; exit 1; }
    printf 'libduckdb-sys %s\n%s\ntarget %s\nimage %s\n' \
      "$(duckdb_version)" "$(rustc_version)" "$target" "$IMAGE" > "$host_target/PREBUILT"
    # Cargo's build-dir layout keeps each unit's outputs and fingerprint in
    # <profile>/build/<package>/<hash>/. Dependencies only: workspace crates
    # (forge-*) rebuild from source anyway. DuckDB's object files are
    # dropped; libduckdb.a stays.
    tar -I 'zstd -T0 -3' -cf "$out" -C "$host_target" \
      --exclude='forge-*' --exclude='libduckdb-sys/*.o' \
      PREBUILT release/build "$target"/release/build
    rm "$host_target/PREBUILT"
    ls -la "$out"
    ;;
  restore)
    # Never fails the build: without the artifact, cargo compiles DuckDB.
    restore() {
      local r dir cid want
      r=$(ref) || return 1
      dir="${RUNNER_TEMP:-/tmp}/duckdb-prebuilt"
      rm -rf "$dir"; mkdir -p "$dir" "$host_target"
      if ! docker pull -q "$r" >/dev/null 2>&1; then
        echo "::notice::prebuilt DuckDB not found ($r) - compiling DuckDB. Run the duckdb-prebuilt workflow to publish it."
        return 0
      fi
      cid=$(docker create "$r" /none) || return 1
      docker cp "$cid:/prebuilt.tar.zst" "$dir/" || { docker rm "$cid" >/dev/null; return 1; }
      docker rm "$cid" >/dev/null
      docker rmi "$r" >/dev/null || true
      tar --zstd -xf "$dir/prebuilt.tar.zst" -C "$dir" PREBUILT || return 1
      want="libduckdb-sys $(duckdb_version)"
      if ! grep -qxF "$want" "$dir/PREBUILT" || ! grep -qxF "$(rustc_version)" "$dir/PREBUILT"; then
        echo "::warning::prebuilt DuckDB mismatched ($r: $(tr '\n' ';' < "$dir/PREBUILT")) - compiling DuckDB"
        rm -rf "$dir"; return 0
      fi
      tar --zstd -xf "$dir/prebuilt.tar.zst" -C "$host_target" --exclude=PREBUILT || return 1
      rm -rf "$dir"
      echo "::notice::prebuilt DuckDB restored from $r ($(du -sh "$host_target" | cut -f1))"
    }
    restore || echo "::warning::restoring the prebuilt DuckDB failed - compiling DuckDB"
    ;;
  *) echo "usage: alpine.sh build|ref|restore|pack <target>" >&2; exit 2 ;;
esac
