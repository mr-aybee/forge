#!/bin/sh
# Downloads an Orbyn Forge release, verifies it and runs its installer.
#
#   curl -fsSLO https://github.com/mr-aybee/forge/releases/latest/download/get-forge.sh
#   sudo sh get-forge.sh                          # latest stable release, interactive
#   sudo sh get-forge.sh --version v0.0.1-rc-01   # one release, also a pre-release
#   sudo sh get-forge.sh --file ./forge-0.0.1-x86_64-linux.tar.gz   # a downloaded package
#   sudo sh get-forge.sh --version latest -- --yes --public-url https://forge.example.com
#
# Options (everything else, or everything after --, goes to install.sh):
#   --version TAG     vX.Y.Z, vX.Y.Z-rc-NN or latest (default latest: the
#                     newest stable release; pre-releases need their tag)
#   --file ARCHIVE    install from a package on disk; SHA256SUMS and
#                     SHA256SUMS.sig must be in the same folder
#   --download-only   verify and unpack into the current folder; do not install
#   --verify-only DIR check DIR/SHA256SUMS against its signature, then every
#                     file it lists that is in DIR; installs nothing
#   -h, --help
#
# Nothing is installed unless SHA256SUMS carries a valid Ed25519 signature
# from the Orbyn Forge release key (pinned below) and the package's SHA-256
# is listed in it. The signature is checked with OpenSSL 1.1.1 or later,
# which is installed with apt-get, dnf, yum or zypper when it is missing.
#
# Environment (testing and mirrors):
#   FORGE_RELEASES_URL         default https://github.com/mr-aybee/forge/releases
#                              (a mirror with the same download/<tag>/<file> and
#                              latest/download/<file> layout; file:// works)
#   FORGE_RELEASE_PUBLIC_KEY   trust another release key (base64url), e.g. a
#                              lab key; a warning is printed
set -eu

# Must equal RELEASE_PUBLIC_KEY in src/protocol/forge-sdk/src/release.rs
# (the release workflow checks this).
PINNED_KEY="6yf7l7jBEw6u446vqWAgt7P7bqEuaY5tpbYnWPHBmbg"
BASE="${FORGE_RELEASES_URL:-https://github.com/mr-aybee/forge/releases}"
KEY="${FORGE_RELEASE_PUBLIC_KEY:-$PINNED_KEY}"

VERSION=latest
FILE=""
MODE=install
VERIFY_DIR=""

die() { printf '\n  x %s\n\n' "$*" >&2; exit 1; }
say() { printf '  + %-22s %s\n' "$1" "$2"; }

ARGS_FILE="$(mktemp)"
trap 'rm -f "$ARGS_FILE"' EXIT
# install.sh options are kept in order; quoted one per line for `set --`.
keep() { printf '%s\n' "$1" | sed "s/'/'\\\\''/g; s/^/'/; s/\$/'/" >> "$ARGS_FILE"; }
while [ $# -gt 0 ]; do
  case "$1" in
    --version) [ $# -ge 2 ] || die "--version needs a value"; VERSION="$2"; shift 2 ;;
    --file) [ $# -ge 2 ] || die "--file needs a path"; FILE="$2"; shift 2 ;;
    --download-only) MODE=download; shift ;;
    --verify-only) [ $# -ge 2 ] || die "--verify-only needs a folder"; MODE=verify; VERIFY_DIR="$2"; shift 2 ;;
    -h|--help) awk 'NR > 1 && /^set -eu$/ { exit } NR > 1' "$0"; exit 0 ;;
    --) shift; while [ $# -gt 0 ]; do keep "$1"; shift; done ;;
    *) keep "$1"; shift ;;
  esac
done

case "$VERSION" in
  latest) TAG=latest ;;
  v[0-9]*) TAG="$VERSION" ;;
  [0-9]*) TAG="v$VERSION" ;;
  *) die "--version must look like v1.2.3, v1.2.3-rc-01 or latest (got $VERSION)" ;;
esac
case "$TAG" in *[!A-Za-z0-9.+_-]*) die "bad version $VERSION" ;; esac
[ "$KEY" = "$PINNED_KEY" ] || printf '  ! %-22s %s\n' "Release key" "FORGE_RELEASE_PUBLIC_KEY overrides the Orbyn Forge key" >&2

# ------------------------------------------------------------------ tools
need_root() { [ "$(id -u)" = 0 ] || die "Run as root: sudo sh $0 …"; }
pkg_install() { # pkg_install <package>: only as root, only with a known manager
  [ "$(id -u)" = 0 ] || return 1
  if command -v apt-get >/dev/null 2>&1; then
    DEBIAN_FRONTEND=noninteractive apt-get install -y -q "$1" >/dev/null 2>&1 \
      || { apt-get update -q >/dev/null 2>&1 && DEBIAN_FRONTEND=noninteractive apt-get install -y -q "$1" >/dev/null; }
  elif command -v dnf >/dev/null 2>&1; then dnf install -y -q "$1" >/dev/null
  elif command -v yum >/dev/null 2>&1; then yum install -y -q "$1" >/dev/null
  elif command -v zypper >/dev/null 2>&1; then zypper -q -n install "$1" >/dev/null
  else return 1; fi
}
need_tool() { # need_tool <command> <package>
  command -v "$1" >/dev/null 2>&1 && return 0
  say "Installing $2" "($1 is needed)"
  pkg_install "$2" && command -v "$1" >/dev/null 2>&1 || die "$1 is missing; install $2 and run this again."
}
sha256() { sha256sum "$1" | awk '{print $1}'; }

# verify_sums <dir>: SHA256SUMS in <dir> is signed with KEY.
verify_sums() {
  d="$1"
  [ -f "$d/SHA256SUMS" ] || die "$d/SHA256SUMS is missing."
  [ -f "$d/SHA256SUMS.sig" ] || die "$d/SHA256SUMS.sig is missing; an unsigned download is not installed."
  need_tool openssl openssl
  w="$(mktemp -d)"
  # The raw 32-byte Ed25519 key in a SubjectPublicKeyInfo (RFC 8410) wrapper.
  k="$(printf '%s' "$KEY" | tr -- '-_' '+/')"
  while [ $(( ${#k} % 4 )) -ne 0 ]; do k="$k="; done
  printf '%s' "$k" | base64 -d > "$w/raw" 2>/dev/null || { rm -rf "$w"; die "the release key is not base64url"; }
  [ "$(wc -c < "$w/raw" | tr -d ' ')" = 32 ] || { rm -rf "$w"; die "the release key is not an Ed25519 key"; }
  { printf '\060\052\060\005\006\003\053\145\160\003\041\000'; cat "$w/raw"; } > "$w/pub.der"
  tr -d '\r\n ' < "$d/SHA256SUMS.sig" | base64 -d > "$w/sig" 2>/dev/null || { rm -rf "$w"; die "SHA256SUMS.sig is not base64"; }
  if ! openssl pkeyutl -verify -pubin -keyform DER -inkey "$w/pub.der" -rawin \
       -in "$d/SHA256SUMS" -sigfile "$w/sig" >/dev/null 2>&1; then
    rm -rf "$w"
    die "SHA256SUMS does not carry a valid Orbyn Forge signature. Nothing was installed. (OpenSSL 1.1.1 or later is needed: $(openssl version 2>/dev/null))"
  fi
  rm -rf "$w"
}
# check_listed <dir> <file>: <file>'s SHA-256 is in the signed sums.
check_listed() {
  want="$(awk -v n="$2" '{ f=$2; sub(/^\*/, "", f); if (f == n) print $1 }' "$1/SHA256SUMS")"
  [ -n "$want" ] || die "$2 is not listed in SHA256SUMS."
  [ "$(sha256 "$1/$2")" = "$want" ] || die "$2 does not match SHA256SUMS (damaged or tampered download). Nothing was installed."
}

if [ "$MODE" = verify ]; then
  verify_sums "$VERIFY_DIR"
  n=0
  for f in $(awk '{ f=$2; sub(/^\*/, "", f); print f }' "$VERIFY_DIR/SHA256SUMS"); do
    [ -f "$VERIFY_DIR/$f" ] || continue   # like sha256sum --ignore-missing
    check_listed "$VERIFY_DIR" "$f"; n=$((n + 1))
  done
  [ "$n" -gt 0 ] || die "none of the files SHA256SUMS lists are in $VERIFY_DIR"
  say "Verified" "signature and $n files in $VERIFY_DIR"
  exit 0
fi

[ "$MODE" = download ] || need_root
case "$(uname -s)" in Linux) ;; *) die "This installer is for Linux. On Windows use get-forge.ps1." ;; esac
case "$(uname -m)" in
  x86_64|amd64) ARCH=x86_64 ;;
  aarch64|arm64) ARCH=aarch64 ;;
  *) die "No Forge package for $(uname -m); x86_64 and aarch64 are supported." ;;
esac
need_tool sha256sum coreutils
need_tool tar tar
need_tool gzip gzip

WORK="$(mktemp -d)"
trap 'rm -f "$ARGS_FILE"; rm -rf "$WORK"' EXIT

# ------------------------------------------------------------------ fetch
if [ -n "$FILE" ]; then
  [ -f "$FILE" ] || die "$FILE not found."
  src="$(cd "$(dirname "$FILE")" && pwd)"
  PKG="$(basename "$FILE")"
  for f in SHA256SUMS SHA256SUMS.sig; do
    [ -f "$src/$f" ] || die "$f is not next to $PKG. Download it from the same release (or use --version)."
    cp "$src/$f" "$WORK/$f"
  done
  ln -s "$src/$PKG" "$WORK/$PKG" 2>/dev/null || cp "$src/$PKG" "$WORK/$PKG"
  say "Package" "$FILE"
else
  need_tool curl curl
  if [ "$TAG" = latest ]; then url="$BASE/latest/download"; else url="$BASE/download/$TAG"; fi
  # HTTPS (GitHub), or a file:// / http:// mirror named in FORGE_RELEASES_URL;
  # the signature, not the transport, makes a download trusted.
  case "$BASE" in http://*) proto='=http,https' ;; file://*) proto='=file' ;; *) proto='=https' ;; esac
  fetch() { curl -fsSL --retry 3 --proto "$proto" --proto-redir '=https' -o "$WORK/$1" "$url/$1"; }
  if ! fetch SHA256SUMS; then
    if [ "$TAG" = latest ]; then
      die "No stable release found at $BASE. For a pre-release, pass its tag: --version vX.Y.Z-rc-NN."
    fi
    die "Release $TAG not found at $BASE (check the tag on the releases page)."
  fi
  fetch SHA256SUMS.sig || die "SHA256SUMS.sig is missing from the release; it is not installed."
  PKG="$(awk '{ f=$2; sub(/^\*/, "", f); print f }' "$WORK/SHA256SUMS" | grep -E "^forge-[0-9][A-Za-z0-9.+_-]*-$ARCH-linux\.tar\.gz\$" | head -1)"
  [ -n "$PKG" ] || die "The release has no Linux $ARCH package."
  say "Downloading" "$PKG"
  fetch "$PKG" || die "Could not download $url/$PKG."
fi

# ------------------------------------------------------------------ verify
verify_sums "$WORK"
say "Signature" "SHA256SUMS signed by the Orbyn Forge release key"
check_listed "$WORK" "$PKG"
say "Checksum" "$PKG matches SHA256SUMS"
case "$PKG" in
  forge-*-"$ARCH"-linux.tar.gz) ;;
  forge-*-linux.tar.gz) die "$PKG is not for this machine ($ARCH)." ;;
  *) die "$PKG is not a Forge Linux package." ;;
esac
NAME="${PKG%.tar.gz}"

# ------------------------------------------------------------------ unpack, install
if [ "$MODE" = download ]; then DEST="$(pwd)"; else DEST="$WORK"; fi
# Only the package's own folder, no absolute paths or ..
LIST="$(tar -tzf "$WORK/$PKG")" || die "$PKG is not a readable tar.gz archive."
if printf '%s\n' "$LIST" | grep -qvE "^$NAME(/|\$)" || printf '%s\n' "$LIST" | grep -qE '(^|/)\.\.(/|$)'; then
  die "$PKG holds files outside $NAME/."
fi
tar -xzf "$WORK/$PKG" -C "$DEST" --no-same-owner
[ -x "$DEST/$NAME/install.sh" ] || die "$PKG has no install.sh."
if [ "$MODE" = download ]; then
  say "Unpacked" "$DEST/$NAME"
  printf '\n  Install with: cd %s && sudo ./install.sh\n\n' "$DEST/$NAME"
  exit 0
fi
eval "set -- $(tr '\n' ' ' < "$ARGS_FILE")"
# Keep the terminal for install.sh's questions when this script was piped in.
if [ ! -t 0 ] && [ -t 1 ] && [ -r /dev/tty ] && { : < /dev/tty; } 2>/dev/null; then
  sh "$DEST/$NAME/install.sh" "$@" < /dev/tty
else
  sh "$DEST/$NAME/install.sh" "$@"
fi
