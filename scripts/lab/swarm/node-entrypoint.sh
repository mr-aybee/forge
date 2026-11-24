#!/bin/sh
# Starts this node's own dockerd, then enrolls the Forge agent on first start
# (FORGE_ENROLL_TOKEN) or restarts the enrolled agent on later starts.
set -eu
STATE=/var/lib/forge-agent
: "${FORGE_URL:?set FORGE_URL, for example http://forge-cp:8090}"

# cgroup v2 nesting: move this shell out of the root cgroup so dockerd can
# enable controllers (the same step the official dind entrypoint does).
if [ -f /sys/fs/cgroup/cgroup.controllers ]; then
  mkdir -p /sys/fs/cgroup/init
  xargs -rn1 < /sys/fs/cgroup/cgroup.procs > /sys/fs/cgroup/init/cgroup.procs 2>/dev/null || :
  sed -e 's/ / +/g' -e 's/^/+/' < /sys/fs/cgroup/cgroup.controllers \
    > /sys/fs/cgroup/cgroup.subtree_control 2>/dev/null || :
fi
nohup dockerd --host=unix:///var/run/docker.sock >>/var/log/dockerd.log 2>&1 </dev/null &
i=0
until docker info >/dev/null 2>&1; do
  i=$((i + 1)); [ "$i" -gt 60 ] && { echo "dockerd did not start" >&2; tail -20 /var/log/dockerd.log >&2; exit 1; }
  sleep 1
done
echo "forge-lab-node: dockerd $(docker version -f '{{.Server.Version}}') is up"

run_agent() {
  id forge-agent >/dev/null 2>&1 || useradd --system --home-dir "$STATE" --shell /usr/sbin/nologin forge-agent
  chown -R forge-agent:forge-agent "$STATE"
  if [ -x /usr/local/bin/forge-agent-helper ]; then
    nohup setsid sh -c "while true; do /usr/local/bin/forge-agent-helper serve --agent-dir $STATE; sleep 2; done" \
      >>/var/log/forge-agent-helper.log 2>&1 </dev/null &
  fi
  nohup setsid sh -c "while true; do runuser -u forge-agent -- /usr/local/bin/forge-agent --state-dir $STATE run; sleep 5; done" \
    >>/var/log/forge-agent.log 2>&1 </dev/null &
}

if [ -f "$STATE/identity.json" ]; then
  echo "forge-lab-node: already enrolled; starting the agent"
  run_agent
else
  : "${FORGE_ENROLL_TOKEN:?not enrolled yet: set FORGE_ENROLL_TOKEN}"
  echo "forge-lab-node: enrolling with $FORGE_URL"
  curl -fsSL "$FORGE_URL/install.sh" | sh
fi
unset FORGE_ENROLL_TOKEN
touch /var/log/forge-agent.log /var/log/forge-agent-helper.log
exec tail -F /var/log/forge-agent.log /var/log/forge-agent-helper.log
