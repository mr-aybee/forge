---
title: Ingress
description: Publish domains through Nginx on one node or a cluster's edge group, with proxy, load-balancer, redirect and static rules, certificates, caching, access control, custom configuration and per-endpoint logs.
sidebar:
  order: 7
---

This guide shows you how to publish domains through Nginx. Each domain set is
an *endpoint*. You serve it from one node with Nginx, or from a cluster's
edge group, which an ingress profile defines. Every endpoint, wherever it is
served, is in one list on the **Ingress** page, under **Endpoints**.

## Before you begin

| Task | Cedar action | Resource | Roles |
| --- | --- | --- | --- |
| Create, edit, delete an endpoint | `workspace.manage` | Workspace | **Platform admin**, **Admin** |
| Save or apply a non-empty custom configuration | `server.command` (MFA) | Each edge | **Platform admin**, **Admin**, **Operator** |
| Save an endpoint with a static directory | `file.read` | Each edge | **Platform admin**, **Admin**, **Operator** |
| Apply, remove, renew, clear the cache | `proxy.manage` (MFA) | Each edge | **Platform admin**, **Admin**, **Operator** |
| Apply with an uploaded certificate | `certificate.use` (MFA) | Workspace | **Platform admin**, **Admin** |
| Read an endpoint's logs | `server.manage` | That edge | **Platform admin**, **Admin**, **Operator** |
| Edit a cluster's ingress profile | `workspace.manage` on the cluster, and `proxy.manage` (MFA) on each edge to apply it | Cluster, edges | **Platform admin**, **Admin** |

- Edge nodes run Linux. Windows nodes cannot be edges.
- Install **Nginx** from the [App Store](/docs/guides/app-store/) on every
  edge. The App Store installs the distribution's package. Generated
  configs work on Nginx 1.18 and newer: HTTP/2 is turned on with
  `listen 443 ssl http2`, which Nginx 1.25.1 and newer accept with a
  deprecation warning.
- For Let's Encrypt, install **Certbot** on every edge. For Forge-managed DNS,
  install **PowerDNS** on a node.
- To use the newer endpoint features (path rules, redirects, static files,
  cache, compression, access control, per-endpoint logs), update the agent
  on every edge first. See the
  [upgrade note](/docs/reference/upgrade-notes/#ingress-endpoints-one-list-and-a-full-editor).

Proxied requests carry `Host`, `X-Real-IP`, `X-Forwarded-For`,
`X-Forwarded-Proto` and `X-Forwarded-Host`. Edges reach services over the
[cluster network](/docs/guides/cluster-network/), so services need no public
address.

## Choose a placement

Each endpoint has one placement:

| Placement | Edges | Use it for |
| --- | --- | --- |
| **A node with Nginx** | The nodes you tick. Each gets the same config | A single public node, or a few nodes behind DNS or Keepalived that you manage yourself |
| **A cluster's edge group** | The edges of the cluster's [ingress profile](#set-up-a-cluster-edge-group) | Several edges with Forge-managed DNS and failover |

A placement on a cluster needs an ingress profile on that cluster. The
endpoint then follows the profile: add an edge to the profile and the next
apply reaches it.

## Set up a cluster edge group

Skip this section when you serve endpoints from single nodes.

1. Open **Ingress**, pick the cluster, and select the **Profile** tab.
2. Under **Layout**, pick one:
   - **Single edge:** one edge serves every endpoint. Point your domains at
     its public IP yourself; Forge needs no DNS.
   - **Active · standby:** the primary serves traffic. When it fails, DNS
     moves to a backup.
   - **Active · active:** every edge serves traffic, with DNS round-robin
     over the healthy edges.
3. Under **Edge nodes**, tick the nodes that run Nginx and fill in each
   edge's **public IP**, the address clients and DNS use. With
   active-standby, mark one edge primary.
4. Under **Traffic & DNS**, set what you need:
   - **Forge-managed records:** pick a PowerDNS node. Forge keeps an A/AAAA
     record for every endpoint domain in the longest matching zone on that
     node.
   - **Record TTL (s):** 60 by default. Lower values fail over faster.
   - **Automatic failover:** Forge checks each edge every 15 seconds. The
     agent must be online and Nginx on the public IP must answer HTTP. Three
     failed checks in a row take an edge out of DNS; three good checks bring
     it back. Each move is audited as `ingress.dns.failover`.
   - **Failover:** DNS by default. Pick
     **Virtual IP (keepalived, edges on one LAN)** only when the edges share
     one LAN.
5. Optionally open **Nginx tuning** for worker connections, timeouts, body
   size, rate limit, gzip, HSTS, server tokens and the access log format.
   Under **Advanced** are trusted proxies and custom `http {}` directives.
6. Select **Save & apply**. Forge writes the profile to every edge, runs
   `nginx -t` (and puts the previous file back if it fails), and publishes
   DNS. Saves and applies are audited.

From the command line:

```sh
# One edge
forge clusters ingress set <cluster> --edge <node-name> --yes
# Two edges, the first one primary
forge clusters ingress set <cluster> --edge <node-name>=192.0.2.10 --edge <node-name>=192.0.2.11 --mode active_standby --yes
# Push the saved profile to the edges
forge clusters ingress apply <cluster> --yes
```

DNS, VRRP and tuning go in a JSON file; see the
[ingress profile reference](/docs/reference/cli/clusters/#ingress-profile).

Removing the profile (`forge clusters ingress remove`) takes the ingress off
every edge. Each edge it succeeded on loses its edge role. An edge that was
offline or failed keeps the role until you remove the profile again. The
other profile settings stay for when you add an edge again.

:::caution
Custom `http {}` directives run as root on every edge. They use the same
[allowlist as an endpoint's custom configuration](#custom-configuration), and
saving or applying a non-empty block needs `server.command` (MFA) on every
edge.
:::

## Create an endpoint

1. Open **Ingress**, select **Endpoints**, then **New endpoint**.
2. Enter the **Domains**, one per line. The first is the primary. The
   **Name** is filled in from it and used for file names.
3. Under **Default route (/)**, pick what requests to `/` do:

   | Type | What you set |
   | --- | --- |
   | **Proxy** | One **Target** |
   | **Load balancer** | Several **Targets**. Add standbys and pick the balancing method later in the editor |
   | **Redirect** | **Redirect to**, a URL. The new endpoint sends `301` and keeps the path |
   | **Static** | **Directory** on the edges. It must be inside an approved file root on every edge (see [Serve static files](#serve-static-files)) |

4. Under placement, pick **A node with Nginx** and tick the nodes, or
   **A cluster's edge group** and the cluster.
5. Select **Create endpoint**. Saving never touches the edges.
6. In the endpoint's row menu, select **Apply to edges**.

A target is one of:

- **Service:** a Forge service, picked by environment and name. Forge sends
  traffic to every node the service runs on (see
  [Which node address the edges use](#which-node-address-the-edges-use)).
  Replicas are active upstreams; standby placements are used only when every
  replica is down.
- **Node port:** a node and a port.
- **Address:** any `host:port`.

Targets are resolved again on every apply, so they follow deploys and moves.
**Expose through ingress** on a service's page opens this dialog with the
service and its port filled in (see
[Run on several nodes](/docs/guides/services/#run-on-several-nodes)).

## Apply an endpoint

On each edge, **Apply to edges** writes the endpoint's main configuration
to `/etc/nginx/forge/sites/<name>.conf` and its custom configuration to
`/etc/nginx/forge/custom/<name>.conf`. It then runs `nginx -t` and reloads
Nginx. If the test fails, the previous files are put back and the `nginx -t`
error is shown, so a mistake never takes the edge down. With Let's Encrypt
the endpoint is first served over HTTP for the challenge. Then the
certificate is issued and HTTPS turned on.

Each edge is an audited `agent.command` job. An endpoint without edges
cannot be applied or removed. **Remove from edges** takes the files off
every edge; **Delete** removes the endpoint from Forge.

## Edit an endpoint

Select **Edit** in the endpoint's row menu. The editor has these sections:

| Section | What you set |
| --- | --- |
| **Domains** | The domain list, and the **HTTP port** (80 by default) and **HTTPS port** (443 by default). Let's Encrypt needs the HTTP port at 80 |
| **Routing** | The **Default route ( / )** and the **Path rules** (see [Route by path](#route-by-path)) |
| **Performance** | **Cache** and **Compression** (see [Cache and compress](#cache-and-compress)) |
| **Access** | **IP access**, **Basic authentication**, **Limits** and **Hotlink protection** (see [Control access](#control-access)) |
| **SSL** | The current certificate, **Force HTTPS**, **HSTS**, and the certificate source (see [Choose certificates](#choose-certificates)) |
| **Redirects** | Domain and path redirects (see [Redirect domains and paths](#redirect-domains-and-paths)) |
| **Config file** | **Main configuration** (read-only) and **Custom configuration** (see [Custom configuration](#custom-configuration)) |
| **Logs** | The endpoint's access and error logs on each edge (see [Read the logs](#read-the-logs)) |
| **Placement** | **Served by**, **Apply status** per edge, and **DNS** |

Select **Save**, then **Apply to edges**. Changes reach the edges only on
apply. Every save is audited as `site.update`.

### Route by path

The **Default route ( / )** handles every request no path rule matches. Each
**Path rules** entry has a **Match** (**Prefix**, **Exact** or **Regex**,
case-sensitive), a **Path** and a **Rule type**: **Proxy**,
**Load balancer**, **Redirect** or **Static**. Rules are checked before the
default route; use **Move up** and **Move down** to order them. An endpoint
has at most 50 path rules.

Proxy and load-balancer rules have these options:

| Option | Default | Effect |
| --- | --- | --- |
| **Preserve Host** | on | Send the client's `Host` upstream; off sends the target's address |
| **Strip prefix** | off | The upstream sees `/x` instead of `/api/x`. Only for prefix rules whose path ends in `/` |
| **WebSockets** | on | Pass WebSocket upgrades |
| **Buffering** | on | Turn off for streaming responses such as server-sent events |
| **Read timeout (s)** | 300 | 1–3600 |
| **Connect timeout (s)** | 5 | 1–300 |

A load balancer also has **Standby targets** (used only when every active
target is down) and **Balancing**: round robin, least connections or IP hash.

A redirect rule has **Redirect to** (an `http://` or `https://` URL), a
**Code** (301, 302, 307 or 308) and **Keep the path and query**.

### Serve static files

A **Static** rule serves files from a **Directory** with an **Index file**
(`index.html` by default). **SPA fallback** serves the index file for
unknown paths, for single-page apps.

Nginx reads the directory as root, so Forge checks it when you save:

- the directory must be inside an approved file root on every edge: a
  service folder or an approved directory, never the whole node or a
  container. Approve the folder under **Files** on each edge first;
- you need `file.read` on every edge.

Commands that serve a static directory without these checks (for example a
raw `POST /api/v1/servers/{id}/commands`) need `server.command` instead.

### Cache and compress

**Cache** keeps upstream responses on each edge in
`/var/cache/nginx/forge/<name>`:

| Field | Default | Notes |
| --- | --- | --- |
| **Cache key** | `$scheme$request_method$host$request_uri` | Nginx variables only |
| **File extensions** | empty | `css`, `js`, … without the dot. Empty caches every response the upstream allows |
| **Keep for (minutes)** | 60 | 1–525600 |
| **Max size (MiB per edge)** | 1024 | 1–1048576 |
| Ignore upstream cache headers | off | Ignores `Cache-Control`, `Expires` and `Set-Cookie` |

**Clear cache**, in the section or the row menu, empties the cache on every
edge. It needs `proxy.manage` (MFA) on each edge and is audited as
`site.cache_clear`.

**Compression** turns on gzip with a **Level (1-9)** (5 by default) and
**MIME types** (CSS, plain text, JavaScript, JSON and SVG by default; at
most 50). `text/html` is always compressed.

### Control access

| Setting | Effect |
| --- | --- |
| **Deny** | Addresses or CIDRs refused first, such as `192.0.2.0/24` |
| **Allow only** | When not empty, only these addresses or CIDRs are served |
| **Basic authentication** | A **Realm** and 1–100 users with passwords |
| **Rate limit (req/s per IP)** | `0` is off |
| **Burst** | Extra requests queued above the rate. `0` means twice the rate |
| **Max request body (MB)** | 25 by default, 1–10240 |
| **Hotlink protection** | The listed **Extensions** load only from the endpoint's own domains and **Also allowed referers** |

Allow and deny hold at most 500 entries together.

Forge hashes basic-auth passwords (SHA-512 crypt) when you save and never
shows or returns them. To keep a user's password, leave its field empty. On
each edge the users go to `/etc/nginx/forge/htpasswd/<name>`, mode `0640`,
readable by Nginx's group.

### Redirect domains and paths

Each redirect has a **From**, a **To** URL, a **Code** (301 by default; 302,
307 or 308) and whether to keep the path. **From** is either one of the
endpoint's domains (not a wildcard), which sends every request on that
domain elsewhere, or a path such as `/old-path`, which keeps the query
string. A path cannot be both a redirect and an exact rule. An endpoint has
at most 100 redirects.

To redirect `www.example.com` to `example.com`, add both to **Domains**, then
add a redirect from `www.example.com` to `https://example.com`.

## Custom configuration

The **Config file** section shows two files:

- **Main configuration** is read-only. Forge generates it from the editor and
  rewrites it on every apply, so edits on the edge do not last.
- **Custom configuration** is yours. Its directives are included inside the
  endpoint's `server` blocks, after Forge's own.

Use the editor sections for what they cover (basic auth, redirects, cache,
static files). Use the custom file for the rest, such as extra headers or a
`location` the rules cannot express.

Nginx runs the custom file as root, so Forge guards it:

- **Allowlist.** Saving checks every directive against a fixed list and
  shows the first refused line. Directives that take a file path (`root`,
  `alias`, `include`, `access_log`, `error_log`, `log_format`,
  `*_temp_path`, certificate and key files, `auth_basic_user_file`), that
  change the Nginx process (`user`, `env`, `worker_*`, `pid`) or that load
  code (`load_module`, Lua, Perl, njs) are refused, and so is any Unix
  socket (`unix:`). Inside `map`, `geo` and `split_clients` the entries are
  data; only `include` is refused there.
- **Tested and rolled back.** Each apply runs `nginx -t` on every edge. If
  it fails, the previous files are put back.
- **Permission.** Saving or applying a non-empty custom file needs
  `server.command` (MFA) on every edge.
- **Audit.** Every save is audited as `site.update` with the custom file, so
  the audit log keeps each version.

The allowed directives are:

- blocks: `location`, `if`, `limit_except`, `upstream`, `server`, `map`, `geo`, `split_clients`;
- flow and headers: `return`, `rewrite`, `set`, `error_page`, `add_header`, `expires`, `default_type`, `internal`;
- proxying: `proxy_pass`, `proxy_set_header`, `proxy_hide_header`, `proxy_redirect`, `proxy_cookie_*`, `proxy_buffering`, `proxy_*_timeout`, `proxy_next_upstream*`, `grpc_pass`, `grpc_set_header`;
- clients and limits: `client_*` sizes and timeouts, `limit_req`, `limit_conn`, `limit_rate`, `allow`, `deny`, `satisfy`, `auth_request`, `real_ip_*`;
- content: `gzip*`, `sub_filter*`, `mirror`, `ssl_protocols`, `ssl_ciphers`, `ssl_session_*`, `resolver`;
- cache and auth directives that take no file path: `proxy_cache`, `proxy_cache_key`, `proxy_cache_valid` and the other `proxy_cache_*` settings, `proxy_no_cache`, `auth_basic`.

## Choose certificates

In the **SSL** section, pick the certificate source:

- **Let's Encrypt:** enter a **Contact email**. Forge checks every edge every
  6 hours. When 5 days or fewer are left, or the certificate is missing, it
  renews on that edge, runs `nginx -t` and reloads. A failed renewal is
  retried every hour and shows as **renewal failing**. Certbot's own timer
  renews earlier (at 30 days), so Forge's check is a safety net.
  **Renew certificate now** forces a renewal; each renewal is audited as
  `site.certificate.renew`. With several edges, each edge gets its own
  certificate: an edge that does not hold a challenge asks the other edges.
  Wildcard domains are not supported with Let's Encrypt.
- **Upload** or **From certificate store:** for paid or company
  certificates. Forge checks that the key matches, the certificate has not
  expired and it covers every domain. The private key is encrypted at rest,
  never shown again and sent only to this endpoint's edges, which store it in
  `/etc/nginx/forge/tls` with mode `0600`. Uploaded certificates cannot renew
  themselves; the endpoint turns amber 14 days before expiry. A certificate
  in use cannot be deleted.

With a certificate, **Force HTTPS** (on by default) redirects HTTP to HTTPS
and **HSTS** (on by default) sends `Strict-Transport-Security`.
**Disable SSL** serves HTTP only after the next save and apply.

## Read the logs

Each endpoint writes its own logs on each edge:

- `/var/log/nginx/forge/<name>.access.log`
- `/var/log/nginx/forge/<name>.error.log`

In the **Logs** section, pick the **Edge**, **Access log** or **Error log**
and the number of lines. Forge shows at most the last 1000 lines (512 KiB).
Reading needs `server.manage` on that edge, and only this endpoint's two log
files can be read.

On an edge with a cluster profile, the access log uses the profile's
**Access log** format (`combined`, `json` or `off`). Apply the endpoints
again after you change it.

## Publish DNS records for an endpoint

In **Placement**, under **DNS**, tick
**Publish DNS records with PowerDNS when applying** and pick the
**DNS node (PowerDNS installed)**, the **Zone** (with the trailing dot), the
**Edge addresses**, the **Mode** (round robin, or active/standby with the
first address only) and the **TTL (s)**. Forge writes the records on apply.

When the endpoint's cluster profile publishes DNS, the profile owns the
records of every endpoint on it. Manage zones and other records on the
[DNS page](/docs/guides/sites-and-dns/).

## Move an endpoint

Change **Served by** in **Placement**, save and apply. Forge does not take
the configuration off the old edges: first select **Remove from edges** in
the row menu, then change the placement and apply.

An endpoint on single nodes that are members of a cluster also shows on
that cluster's **Routing** tab, with `"source": "edges"` in
`GET /api/v1/clusters/{id}/routing`.

## Which node address the edges use

For **Service** and **Node port** targets, Forge uses the node's
cluster-network (mesh) address only when the node's mesh is up and every
edge of the endpoint is up in the same cluster network. Otherwise it uses
the node's own address, the one the server sees it connect from. One
upstream list serves every edge, so a single edge outside the network makes
Forge use the own addresses. This is the same rule as
[local names](/docs/concepts/local-names/).

:::caution
An edge outside the targets' cluster network reaches them over the plain LAN,
not the encrypted mesh. Anyone on that network can read or change the
traffic. Put the edge in the same cluster network, or use TLS to the backend.
:::

## Check routing and traffic

The cluster's **Routing** tab shows each endpoint's domain, edges (with
Nginx state), target service and its internal name
(`api.payments.<cluster>.forge.internal`), and the node addresses Nginx
forwards to, with each placement's last state and role. Offline nodes and
stopped services are dimmed. An endpoint shows **degraded** when some
upstreams are down and **down** when none is up. These states come from the
agents, not health checks; Nginx may still try a dimmed upstream.

Every edge also logs one short line per proxied request to
`/var/log/nginx/forge-traffic.log`. The agent sums the last minute, so
**Routing**, **Health** and the cluster's **Overview** and **Nodes** show
throughput. `GET /api/v1/traffic?cluster_id=` returns the same figures.

To check an endpoint before saving, send its body to
`POST /api/v1/sites/preview`. It returns the main configuration, the custom
file, the first refused custom line, the edges and the resolved upstreams.
See the [REST API reference](/docs/reference/rest-api/#ingress-sites-and-dns).

## Verify

- **Placement → Apply status** shows every edge applied, with no error.
- `curl -I https://<domain>` returns your service's response with a valid
  certificate.
- **Logs → Access log** shows your request.
- On a cluster, **Health** shows every edge healthy and the DNS records.

## Troubleshooting

| Symptom | Cause | Fix |
| --- | --- | --- |
| **Apply to edges** is disabled: "No edges" | The endpoint has no placement | **Edit → Placement**, choose a node or cluster |
| "this cluster has no ingress profile yet" on save | The cluster placement needs a profile | [Set up the edge group](#set-up-a-cluster-edge-group) first |
| Apply fails and the previous file is back | `nginx -t` rejected the configuration | Read the error, fix the custom configuration or tuning, apply again |
| A custom line is refused on save | The directive is not on the allowlist | Use an editor section or an allowed directive |
| `403` when saving a custom configuration | No `server.command` (MFA) on an edge | Verify with MFA, or ask an administrator |
| "static directory … is not inside an approved file root" | The directory is not approved on that edge | Approve the folder under **Files** on every edge |
| Apply fails with an unknown field error on an edge | The edge's agent is older than the server | Update the agent on that edge, then apply again |
| Endpoint shows **renewal failing** | Let's Encrypt cannot reach the challenge, or Certbot is missing | Point the domain at the edges, install **Certbot**, **Renew certificate now** |
| `502` from the edge | The edge cannot reach the upstream | Check the cluster network and the service's port |
| **Logs** shows the file is missing | The endpoint was not applied on that edge since its agent was updated | Apply the endpoint again |
| An edge is taken out of DNS | Three failed health checks in a row | Check the agent and Nginx on that edge |

## Next steps

- [Sites and DNS](/docs/guides/sites-and-dns/) for PowerDNS zones and records.
- [`forge sites`](/docs/reference/cli/sites-certificates/) and
  [`forge clusters ingress`](/docs/reference/cli/clusters/#forge-clusters-ingress-get)
  from the command line.
- [Publish Forge on a domain](/docs/guides/publish-forge-on-a-domain/).
- [App Store](/docs/guides/app-store/) for Nginx, Certbot and PowerDNS.
