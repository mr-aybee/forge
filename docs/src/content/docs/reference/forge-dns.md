---
title: forge-dns
description: The cluster DNS server's flags, environment, record file, systemd unit and ports.
---

`forge-dns` answers the names of a cluster's services and replicas
(`<service>.<stack>.<environment>.<cluster>.local` and the reverse names of
their addresses) and forwards every other name to the system's nameservers.
It runs on every node of a cluster with a mesh and, when configured, on the
server host. You normally never run it by hand: the agent (nodes) and the
server installer (server host) write its unit and configuration.

## Behaviour

- **Answers from the file.** A query for a name in or under one of the file's
  `zones` is answered from memory: A, SRV and PTR records. AAAA for a known
  name answers NOERROR with no records. A name that does not exist answers
  NXDOMAIN with the zone's SOA in the authority section. Answers are
  authoritative and carry the file's `ttl` (or `--ttl`).
- **Round robin.** A name with several A records returns all of them, rotated
  by one on every query.
- **Forwarding.** Any other name goes to the first upstream; on a 1 s timeout
  the next one is tried. A truncated UDP reply is retried over TCP. Answers
  are copied, never cached. When no upstream answers, the reply is SERVFAIL.
- **Reloads.** The file's modification time and size are checked every 2 s.
  A changed file is parsed into a new map, which replaces the old one in one
  step. A file that does not parse is logged and the previous map stays.
- **No file yet.** Until a valid file has been read, every query is REFUSED.
- **Sizes.** UDP replies are at most 512 bytes, or the size the client
  advertises with EDNS; larger replies set the TC bit and the client retries
  over TCP, which is served on the same addresses.
- It binds only the `--listen` addresses and rate-limits nothing.

## Flags

| Flag | Environment | Default | Meaning |
| --- | --- | --- | --- |
| `--file <path>` | `FORGE_DNS_FILE` | required | The record file (`names.json`). |
| `--listen <ip:port>` | `FORGE_DNS_LISTEN` | required | Address to serve UDP and TCP on. Repeatable; comma-separated in the environment. |
| `--upstream <ip:port>` | `FORGE_DNS_UPSTREAM` | from `/etc/resolv.conf` | Forwarder for other names, tried in order. Repeatable; comma-separated in the environment. |
| `--ttl <secs>` | `FORGE_DNS_TTL` | the file's `ttl` | TTL of every answer. |
| `--version`, `--help` | | | Print and exit. |

Without `--upstream`, the upstreams are the `nameserver` lines of
`/etc/resolv.conf` on port 53, minus the `--listen` addresses, and minus
`127.0.0.53` (the systemd-resolved stub) unless it is the only one.

Log verbosity follows `RUST_LOG` (default `info`); logs go to standard error.

## Record file

The file is JSON, written by the agent (node) or the server (server host) to a
temporary name and renamed into place. Unknown fields are ignored, so an older
`forge-dns` reads a newer file.

| Path | Role |
| --- | --- |
| `/var/lib/forge-dns/names.json` | Node |
| `/var/lib/forge-dns/names.json` (`[dns] records_file`) | Server host |

```json
{
  "version": 1,
  "generated_at_ms": 1764115200000,
  "zones": ["c1.local", "64.10.in-addr.arpa"],
  "ttl": 5,
  "records": [
    { "name": "api.orders.prod.c1.local", "type": "A", "value": "10.64.10.2" },
    { "name": "api.orders.prod.c1.local", "type": "A", "value": "10.64.12.4" },
    { "name": "_http._tcp.api.orders.prod.c1.local", "type": "SRV",
      "priority": 0, "weight": 0, "port": 8080, "target": "api.orders.prod.c1.local" },
    { "name": "2.10.64.10.in-addr.arpa", "type": "PTR", "value": "api-1.orders.prod.c1.local" }
  ]
}
```

- Names are fully qualified without the trailing dot. Every record must be in
  or under one of `zones`, or the file is rejected.
- `zones` lists the cluster zones and the reverse zones of the mesh subnets.
- Names between a record and its zone (`orders.prod.c1.local` above) exist
  without records: they answer NOERROR, not NXDOMAIN.
- `generated_at_ms` becomes the SOA serial (in seconds).

## systemd unit

`forge-dns.service` runs as the system user `forge-dns` with only the
capability to bind port 53:

```ini
[Unit]
Description=Orbyn Forge cluster DNS
After=network-online.target
Wants=network-online.target

[Service]
User=forge-dns
Group=forge-dns
DynamicUser=no
AmbientCapabilities=CAP_NET_BIND_SERVICE
CapabilityBoundingSet=CAP_NET_BIND_SERVICE
NoNewPrivileges=yes
ProtectSystem=strict
ProtectHome=yes
ReadOnlyPaths=/var/lib/forge-dns
EnvironmentFile=-/etc/forge/forge-dns.env
ExecStart=/usr/local/bin/forge-dns --file ${FORGE_DNS_FILE} $FORGE_DNS_ARGS
Restart=always
RestartSec=1

[Install]
WantedBy=multi-user.target
```

On both, `ReadOnlyPaths` names the folder, not the file, so the unit starts
before the first file exists and sees each renamed-in update.
`/etc/forge/forge-dns.env` sets `FORGE_DNS_FILE` and `FORGE_DNS_ARGS`
(for example `--listen 10.64.0.1:53`):

- **Node:** the agent writes it and restarts the unit when the listen address
  (the node's mesh address) changes.
- **Server host:** the installer writes it from `[dns] listen` in
  `forge.toml`. With `listen` empty (the default) the unit is installed but
  disabled.

## Ports

| Port | Protocol | Bound on | From |
| --- | --- | --- | --- |
| 53 | UDP and TCP | Each `--listen` address only (the mesh address on a node) | Containers and hosts of the cluster |

forge-dns makes outbound queries to its upstreams on port 53 (UDP, and TCP
for truncated replies). See [Network ports](/docs/reference/network-ports/).
