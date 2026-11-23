---
title: Sites and DNS
description: Publish a service on a domain through Nginx edge nodes, with TLS, active/standby upstreams, custom blocks and PowerDNS records.
sidebar:
  order: 8
---

This guide shows you how to publish a service on a domain as an
*endpoint* (a site) on Nginx edge nodes, add your own Nginx directives, and
manage PowerDNS zones and records. For a cluster, set up the ingress layout
once in [Ingress](/docs/guides/ingress/); the steps below apply to every endpoint.

## Before you begin

| Task | Cedar action | Resource | Roles |
| --- | --- | --- | --- |
| Create, edit, delete an endpoint | `workspace.manage` | Workspace | **Platform admin**, **Admin** |
| Save or apply a non-empty custom block | `server.command` (MFA) | Each edge | **Platform admin**, **Admin**, **Operator** |
| Apply, remove, renew; DNS changes | `proxy.manage` (MFA) | Each edge | **Platform admin**, **Admin**, **Operator** |
| Apply with an uploaded certificate | `certificate.use` (MFA) | Workspace (the job checks each edge again) | **Platform admin**, **Admin** |

- Edge nodes run Linux with **Nginx** from the [App Store](/docs/guides/app-store/).
  For HTTPS with Let's Encrypt, install **Certbot** on the edges. For two
  edges in active/standby on one LAN, you can also install **Keepalived** on
  both (one MASTER, one BACKUP), or let DNS spread traffic.
- For DNS, install **PowerDNS** on a node. Its API key stays on that node and
  Forge talks to it through the node's helper.

Endpoints belong to the workspace; endpoints of another workspace are not
found.

## Publish an endpoint

1. Open the cluster, select **Ingress**, then the **Endpoints** tab, and
   select **New endpoint**.
2. Fill in the tabs:

   | Tab | What you set |
   | --- | --- |
   | **Routing** | **Name**, **Domains**, **Active upstreams** and **Standby upstreams** (`host:port`; standby is Nginx `backup`, used only when every active upstream is down), **Balancing** (**Round robin**, **Least connections** or **IP hash**). **Fill upstreams from a service** uses its replicas and standbys. Without a cluster profile, also the **Edge nodes (Nginx installed)** |
   | **TLS & limits** | **TLS** (Let's Encrypt over HTTP-01, with redirect to HTTPS and HSTS, or an uploaded certificate), WebSockets, **Max request body (MB)**, **Rate limit (requests/s per IP, 0 = off)** |
   | **Custom block** | Extra Nginx directives for this site (locations, headers, redirects, limits…), from the [supported list](#custom-blocks) |
   | **DNS** | **Publish DNS records with PowerDNS when applying** |
   | **Preview** | The exact configuration the edge will run: the HTTPS variant with TLS, the HTTP variant when TLS is off |

3. Select **Save**.
4. In the endpoint's row menu, select **Apply to edges**.

Forge writes `/etc/nginx/forge/sites/<site>.conf` (managed) and
`/etc/nginx/forge/custom/<site>.conf` (your block) on every edge, runs
`nginx -t` and reloads. If the test fails, the previous files are restored
and the error from `nginx -t` is shown, so a typo never takes the edge down.
With Let's Encrypt the site is first served over HTTP for the challenge, then
the certificate is issued and HTTPS enabled.

The managed file also turns on the Forge traffic log
(`/var/log/nginx/forge-traffic.log`, used by **Watch traffic**) and the main
access log in each `server` block, with its own `log_format` named after the
site. Edges without a cluster ingress profile get the traffic log too. On an
edge with a profile, the main log uses the profile's **Access log** format
(`combined`, `json` or `off`). After you change that format, apply the
endpoints again.

The managed file is regenerated on every apply; your custom block is never
touched. Every change is recorded in the audit log (`site.update`). An
endpoint without edge nodes cannot be applied or removed. Applying an
endpoint with an uploaded certificate sends its private key only to that
endpoint's edges.

### Custom blocks

Nginx runs custom blocks as root on the edge, so Forge accepts only a fixed
list of directives and refuses the rest when you save:

- blocks: `location`, `if`, `limit_except`, `upstream`, `server`, `map`, `geo`, `split_clients`;
- flow and headers: `return`, `rewrite`, `set`, `error_page`, `add_header`, `expires`, `default_type`, `internal`;
- proxying: `proxy_pass`, `proxy_set_header`, `proxy_hide_header`, `proxy_redirect`, `proxy_cookie_*`, `proxy_buffering`, `proxy_*_timeout`, `proxy_next_upstream*`, `grpc_pass`, `grpc_set_header`;
- clients and limits: `client_*` sizes and timeouts, `limit_req`, `limit_conn`, `limit_rate`, `allow`, `deny`, `satisfy`, `auth_request`, `real_ip_*`;
- content: `gzip*`, `sub_filter*`, `mirror`, `ssl_protocols`, `ssl_ciphers`, `ssl_session_*`, `resolver`.

Directives that take a file path (`root`, `alias`, `include`, `access_log`,
`error_log`, `log_format`, `*_temp_path`, certificate and key files,
`auth_basic_user_file`), that change the nginx process (`user`, `env`,
`worker_*`, `pid`) or that load code (`load_module`, Lua, Perl, njs) are
refused, and so is any Unix socket (`unix:`). Inside `map`, `geo` and
`split_clients` the entries are data; only `include` is refused there.

:::caution
Saving or applying an endpoint with a non-empty custom block needs
`server.command` on every edge, with an MFA-verified session, in addition to
the other permissions.
:::

## Publish DNS records for an endpoint

On the endpoint's **DNS** tab, tick
**Publish DNS records with PowerDNS when applying**, then pick the
**DNS node (PowerDNS installed)**, the **Zone** (with a trailing dot), the
**Edge addresses** and the **Mode**:

- **Round robin — all addresses**: every edge address in the record set;
- **Active/standby — first address only**: switch by reordering the
  addresses and applying again.

When the cluster's ingress profile publishes DNS itself, it owns the records
for every endpoint; see [Ingress](/docs/guides/ingress/).

## Manage zones and records

1. Open **Network → DNS**.
2. Select **New zone**, enter the **Domain** and the **Name servers**, and
   create it. Forge creates the zone with an SOA and NS records.
3. Pick the zone, select **Record**, set the **Name**, **Type** (A, AAAA,
   CNAME, TXT, MX, NS, SRV or CAA), values and **TTL**, then select
   **Add record**.
4. At your registrar, point the zone's NS records at the PowerDNS node to
   make it authoritative.

## Verify

- The endpoint's row shows it applied on its edges, with no
  **renewal failing** badge.
- `curl -I https://<domain>` returns your service's response.
- `dig +short <domain> @<powerdns-node>` returns the edge addresses.

## Troubleshooting

| Symptom | Cause | Fix |
| --- | --- | --- |
| Apply fails with an `nginx -t` error | The configuration or custom block is invalid | Fix the block; the previous files are still in place |
| A custom directive is refused on save | It is not on the supported list | Use a supported directive, or set it in the profile's Nginx tuning |
| `403` when saving a custom block | You lack `server.command` (MFA) on an edge | Verify with MFA, or ask an administrator |
| **DNS** shows "Install PowerDNS on a node" | No node has PowerDNS | Install **PowerDNS** from the App Store |
| Records exist but the world does not see them | The registrar does not delegate to the PowerDNS node | Set the zone's NS records at the registrar |
| Let's Encrypt fails | The domain does not point at the edges yet | Publish DNS first, then apply again |
| An endpoint with hand-picked edges is missing from the cluster's **Ingress** page | None of its edges is a node of that cluster | Check the edges, or [link it to the cluster](/docs/guides/ingress/#endpoints-with-explicit-edges) |
| `/var/log/nginx/forge-traffic.log` is missing on an edge | The endpoint was applied by an older Forge agent | Update the agent, then select **Apply to edges** |

## Next steps

- [Ingress](/docs/guides/ingress/)
- [Services](/docs/guides/services/)
- [App Store](/docs/guides/app-store/) for Nginx, Certbot, Keepalived and PowerDNS.
