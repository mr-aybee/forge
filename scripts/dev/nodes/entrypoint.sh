#!/bin/sh
# First start: run the control plane's installer with FORGE_ENROLL_TOKEN.
# Later starts (state volume already enrolled): start the agent and helper
# the same way the installer's no-systemd path does, without a token.
set -eu

STATE=/var/lib/forge-agent
: "${FORGE_URL:?set FORGE_URL, for example http://172.17.0.1:55130}"

if [ -f "$STATE/identity.json" ]; then
  echo "forge-node: already enrolled; fetching agent and starting it"
  case "$(uname -m)" in x86_64) ARCH=x86_64 ;; aarch64) ARCH=aarch64 ;; *) echo "unsupported arch" >&2; exit 1 ;; esac
  for bin in forge-agent forge-agent-helper; do
    curl -fsSL "$FORGE_URL/agent/download/linux/$ARCH/$bin" -o "/usr/local/bin/$bin" \
      && chmod 0755 "/usr/local/bin/$bin" || rm -f "/usr/local/bin/$bin"
  done
  [ -x /usr/local/bin/forge-agent ] || { echo "forge-node: agent download failed" >&2; exit 1; }
  id forge-agent >/dev/null 2>&1 || \
    useradd --system --home-dir "$STATE" --shell /usr/sbin/nologin forge-agent
  chown -R forge-agent:forge-agent "$STATE"
  if [ -x /usr/local/bin/forge-agent-helper ]; then
    nohup setsid sh -c "while true; do /usr/local/bin/forge-agent-helper serve --agent-dir $STATE; sleep 2; done" \
      >>/var/log/forge-agent-helper.log 2>&1 </dev/null &
  fi
  nohup setsid sh -c "while true; do runuser -u forge-agent -- /usr/local/bin/forge-agent --state-dir $STATE run; sleep 5; done" \
    >>/var/log/forge-agent.log 2>&1 </dev/null &
else
  : "${FORGE_ENROLL_TOKEN:?not enrolled yet: set FORGE_ENROLL_TOKEN (one token per node)}"
  echo "forge-node: enrolling with $FORGE_URL"
  curl -fsSL "$FORGE_URL/install.sh" | sh
fi
unset FORGE_ENROLL_TOKEN

touch /var/log/forge-agent.log /var/log/forge-agent-helper.log
exec tail -F /var/log/forge-agent.log /var/log/forge-agent-helper.log
