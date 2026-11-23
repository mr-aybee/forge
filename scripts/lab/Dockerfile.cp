# Control plane for the lab. forge-server is a static binary; outbound TLS
# (vendor release data, OIDC discovery) verifies against the system CA bundle,
# so production hosts need ca-certificates too.
FROM debian:bookworm-slim
RUN apt-get update \
 && apt-get install -y --no-install-recommends ca-certificates \
 && rm -rf /var/lib/apt/lists/*
