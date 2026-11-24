# Control plane for the container platform lab. The binaries are mounted
# from the build (target/<profile>); trixie's glibc matches recent hosts.
FROM debian:trixie-slim
RUN apt-get update \
 && apt-get install -y --no-install-recommends ca-certificates \
 && rm -rf /var/lib/apt/lists/*
