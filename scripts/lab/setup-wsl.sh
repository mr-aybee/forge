#!/usr/bin/env bash
# One-time setup of the Orbyn Forge Linux lab inside WSL (Ubuntu 24.04).
#
#   From Windows:  wsl -d Ubuntu-24.04 -u root -- bash scripts/lab/setup-wsl.sh <your-wsl-user>
#
# Installs build tools, enables Docker Engine (docker.io, no Docker Desktop),
# adds the user to the docker group, and installs Rust with the musl target
# for that user.
set -euo pipefail

user="${1:-${SUDO_USER:-}}"
[ "$(id -u)" -eq 0 ] || { echo "run as root (wsl -u root)" >&2; exit 1; }
[ -n "$user" ] || { echo "usage: setup-wsl.sh <wsl-user>" >&2; exit 1; }

export DEBIAN_FRONTEND=noninteractive
apt-get update -qq
apt-get install -y -qq docker.io build-essential musl-tools lld pkg-config curl ca-certificates >/dev/null
systemctl enable --now docker >/dev/null
usermod -aG docker "$user"

if ! sudo -u "$user" bash -lc 'command -v rustup' >/dev/null 2>&1; then
  sudo -u "$user" bash -lc 'curl -fsSL https://sh.rustup.rs | sh -s -- -y --profile minimal'
fi
sudo -u "$user" bash -lc 'rustup target add x86_64-unknown-linux-musl >/dev/null'

echo "lab setup done: docker $(docker version --format '{{.Server.Version}}'), $(sudo -u "$user" bash -lc 'rustc --version')"
