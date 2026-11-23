# Disposable Forge node for local development: Debian with what the Linux
# installer (install.sh, no-systemd path) and the agent's container features
# need. The agent itself is downloaded from the control plane at first start.
FROM docker:29-cli AS docker-cli

FROM debian:12-slim
RUN apt-get update \
 && apt-get install -y --no-install-recommends \
      ca-certificates curl procps util-linux passwd sudo iproute2 \
 && rm -rf /var/lib/apt/lists/*
# Docker CLI + Compose plugin; the engine is the host's, via the mounted socket.
COPY --from=docker-cli /usr/local/bin/docker /usr/local/bin/docker
COPY --from=docker-cli /usr/local/libexec/docker/cli-plugins/docker-compose /usr/local/libexec/docker/cli-plugins/docker-compose
COPY entrypoint.sh /usr/local/bin/forge-node-entrypoint
RUN chmod 0755 /usr/local/bin/forge-node-entrypoint
ENTRYPOINT ["/usr/local/bin/forge-node-entrypoint"]
