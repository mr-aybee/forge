#!/bin/sh
set -eu
: "${FORGEOPS_PASSWORD:?FORGEOPS_PASSWORD must be set}"
echo "forgeops:${FORGEOPS_PASSWORD}" | chpasswd
unset FORGEOPS_PASSWORD
ssh-keygen -A >/dev/null 2>&1
# If Forge already installed the agent (container restart), resume it.
if [ -f /var/lib/forge-agent/identity.json ]; then
  if [ -x /usr/local/bin/forge-agent-helper ]; then
    nohup setsid sh -c 'while true; do /usr/local/bin/forge-agent-helper serve --agent-dir /var/lib/forge-agent; sleep 2; done'       >>/var/log/forge-agent-helper.log 2>&1 </dev/null &
  fi
  nohup setsid sh -c 'while true; do runuser -u forge-agent -- /usr/local/bin/forge-agent --state-dir /var/lib/forge-agent run; sleep 5; done' \
    >>/var/log/forge-agent.log 2>&1 </dev/null &
fi
exec /usr/sbin/sshd -D -e
