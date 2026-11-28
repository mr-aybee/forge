#!/usr/bin/env bash
# Writes the GitHub release notes to stdout.
#   bash scripts/packaging/release-notes.sh <tag> <prerelease:true|false> <owner/repo> <assets-dir>
#
# The highlights come from scripts/packaging/release-notes/<tag>.md when it
# exists (write it before tagging); the install and verify sections are
# generated from the assets that were actually built, so every command uses a
# real asset name.
set -euo pipefail
TAG="$1"; PRE="$2"; REPO="$3"; A="$4"
V="${TAG#v}"
HERE="$(cd "$(dirname "$0")" && pwd)"
DL="https://github.com/$REPO/releases/download/$TAG"
DOCS="https://forge.orbyntech.com/docs"
KEY=$(sed -n 's/^pub const RELEASE_PUBLIC_KEY: &str = "\(.*\)";/\1/p' "$HERE/../../src/forge/crates/protocol/forge-sdk/src/release.rs")
[ -n "$KEY" ] || { echo "RELEASE_PUBLIC_KEY not found" >&2; exit 1; }

if [ "$PRE" = true ]; then
  echo "> [!NOTE]"
  echo "> **Pre-release** for testing. Only installs that run a pre-release are offered it as an update, and it is not the \"latest\" release."
  echo
fi
if [ -f "$HERE/release-notes/$TAG.md" ]; then
  cat "$HERE/release-notes/$TAG.md"
  echo
else
  echo "Orbyn Forge $V."
  echo
fi

cat <<EOF
## Install

Linux (x86_64 or arm64, systemd), as root. The script verifies the signature and checksum, then runs the installer:

\`\`\`sh
curl -fsSLO $DL/get-forge.sh
sudo sh get-forge.sh --version $TAG
\`\`\`

Or download the package for your machine, verify it and run \`install.sh\` from it:

\`\`\`sh
ARCH=\$(uname -m)   # x86_64 or aarch64
curl -fLO $DL/forge-$V-\$ARCH-linux.tar.gz -fLO $DL/SHA256SUMS -fLO $DL/SHA256SUMS.sig
sha256sum -c SHA256SUMS --ignore-missing
tar xzf forge-$V-\$ARCH-linux.tar.gz && cd forge-$V-\$ARCH-linux && sudo ./install.sh
\`\`\`
EOF
if [ -f "$A/forge-$V-x86_64-windows.zip" ]; then
  cat <<EOF

Windows (PowerShell as Administrator):

\`\`\`powershell
Invoke-WebRequest $DL/get-forge.ps1 -OutFile get-forge.ps1
.\\get-forge.ps1 -Version $TAG
\`\`\`
EOF
else
  echo
  echo "This release has no Windows package."
fi
cat <<EOF

Full guide: [Installation]($DOCS/getting-started/installation/). Upgrading an existing install: read the [upgrade notes]($DOCS/reference/upgrade-notes/) first.

## Files

| File | For |
| --- | --- |
EOF
for arch in x86_64 aarch64; do
  [ -f "$A/forge-$V-$arch-linux.tar.gz" ] && echo "| \`forge-$V-$arch-linux.tar.gz\` | Control plane package, Linux $arch |"
done
[ -f "$A/forge-$V-x86_64-windows.zip" ] && echo "| \`forge-$V-x86_64-windows.zip\` | Control plane package, Windows x86_64 |"
echo "| \`get-forge.sh\`, \`get-forge.ps1\` | Download, verify and install |"
for f in "$A"/forge-cli-*; do
  [ -f "$f" ] && echo "| \`$(basename "$f")\` | The \`forge\` CLI |"
done
cat <<EOF
| \`forge-server-*\`, \`forge-analytics-worker-*\`, \`forge-plugin-runner-*\`, \`forge-agent-*\` | Single binaries for the self-updater |
| \`SHA256SUMS\`, \`SHA256SUMS.sig\` | Checksums and their Ed25519 signature |

## Verify

\`SHA256SUMS.sig\` is an Ed25519 signature of \`SHA256SUMS\` by the Orbyn Forge release key \`$KEY\`. \`get-forge.sh --verify-only <folder>\` checks a folder of downloads; see [Verify a download]($DOCS/getting-started/installation/#verify-a-download).
EOF
