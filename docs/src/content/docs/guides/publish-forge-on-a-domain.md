---
title: Publish Forge on a domain
description: Reach the Forge console at a domain such as https://forge.example.com through a cluster ingress endpoint, while the direct IP address keeps working.
sidebar:
  order: 31
---

This guide publishes the Forge console on a domain through an existing
cluster ingress. An Nginx edge node terminates TLS and proxies plain HTTP to
the Forge server. The direct address (`http://<forge-ip>:3880`) keeps working,
and every link Forge sends, in alert emails, webhooks and approvals, uses the
domain.

```diagram
alt: Reaching Forge through an edge node or directly
flow:
  rows:
    - title: Through the edge
      steps:
        - { label: browser, edge: HTTPS }
        - { label: edge node, note: "Nginx, certificate", edge: HTTP }
        - { label: "forge-server `:3880`", tone: accent }
    - title: Direct (still works)
      steps:
        - { label: browser, edge: HTTP }
        - { label: "forge-server `:3880`", tone: accent }
```

Forge needs no certificate of its own. The ingress endpoint handles TLS, with
Let's Encrypt or an uploaded certificate, like for any other portal on the same
edge.

## Before you begin

- A cluster with an [ingress profile](/docs/guides/ingress/) and at least one edge
  node running **Nginx**. For Let's Encrypt, **Certbot** on every edge.
- A DNS record for the domain that points at the edges (yours, or
  Forge-managed through the profile).
- The edges can reach the Forge server's API port (`api_listen`, `3880` by
  default). The installer listens on `0.0.0.0:3880`.
- Creating and applying the endpoint needs `workspace.manage` and
  `proxy.manage` on each edge with an MFA-verified session (**Admin** or
  **Platform admin**).
- Changing the public address needs `system.update` on the platform
  (**Platform admin**) with an MFA-verified session.
- Editing `forge.toml` needs root on the Forge server.

## Steps

1. **Trust the edges.** On the Forge server, add the edge nodes' addresses to
   `[security] trusted_proxies` in `/etc/forge/forge.toml`, then restart:

   ```toml
   [security]
   trusted_proxies = ["<edge-ip>"]
   ```

   ```sh
   sudo systemctl restart forge-server
   ```

   Forge then takes the client address from `X-Forwarded-For` and the
   browser's scheme and host from `X-Forwarded-Proto` and `X-Forwarded-Host`.
   With `https`, it marks session cookies `Secure`. From any other address
   Forge ignores these headers.

2. **Add the endpoint.** Open the cluster, select **Ingress**, then
   **Endpoints** → **New endpoint**.
   - **Name:** `forge-console`.
   - **Domains:** `forge.example.com`.
   - **Active upstreams:** the Forge server's address as the edge reaches it,
     for example `<forge-ip>:3880`. With typed targets, use an **Address**
     target with the same `host:port`.
   - **WebSockets:** on (the default). The console's live updates, terminal
     and tunnels use WebSockets.
   - **TLS:** **Let's Encrypt** with your email, or **Uploaded certificate**.
     See [Choose certificates](/docs/guides/ingress/#choose-certificates).

   Select **Save**, then **Apply to edges** in the endpoint's row menu.

   From the CLI, with `site.json`:

   ```json
   {
     "name": "forge-console",
     "domains": ["forge.example.com"],
     "cluster_id": "<cluster-id>",
     "targets": [{ "kind": "external", "address": "<forge-ip>:3880" }],
     "tls": "acme",
     "acme_email": "<admin-email>",
     "websockets": true
   }
   ```

   ```sh
   forge sites create --file site.json
   forge sites apply forge-console
   ```

3. **Pin the public address.** The console already works on the domain
   without this step. Pinning makes emails, alerts, SSO and AI connector
   links use the domain. Open **Settings → Server**. Under
   **Public address**, enter `https://forge.example.com` and select **Save**.
   The line under the field shows **Links use HTTPS.** From the CLI:

   ```sh
   forge settings public-address set https://forge.example.com
   ```

   On the server host, without signing in (restart to apply):

   ```sh
   sudo runuser -u forge -- /opt/forge/forge-server --config /etc/forge/forge.toml public-address set https://forge.example.com
   sudo systemctl restart forge-server
   ```

   On a new install, pass `--public-address https://forge.example.com
   --trusted-proxies <edge-ip>` to `install.sh` instead of steps 1 and 3.

4. **Narrow the allowed hosts.** In **Settings → Server**, under
   **Allowed hosts**, replace `*` with `forge.example.com` plus any direct
   address you still use, for example `192.0.2.10` and `localhost`. Other
   names then get `421`. From the server host:

   ```sh
   sudo runuser -u forge -- /opt/forge/forge-server --config /etc/forge/forge.toml allowed-hosts set forge.example.com 192.0.2.10 localhost 127.0.0.1
   sudo systemctl restart forge-server
   ```

5. **If you use single sign-on,** register
   `https://forge.example.com/api/v1/auth/oidc/callback` as a redirect URI
   at your identity provider and restart `forge-server`. Forge reads the
   OIDC redirect URL at start.

## Verify

- Open `https://forge.example.com` and sign in. The browser shows a valid
  certificate.
- Open a node's terminal or watch a job. Live updates work, so the WebSocket
  origin check accepts the domain.
- Open `http://<forge-ip>:3880`. The direct address still works when it is
  in the allowed hosts; otherwise it returns `421`.
- When the next alarm fires, the **Open the alarm** link in the email, and
  `links.alarm` in a webhook payload, start with `https://forge.example.com`.
  A **Send test** message carries no alarm link.
- `forge settings public-address get` shows `SOURCE` `setting`.

## Troubleshooting

| Symptom | Cause | Fix |
| --- | --- | --- |
| `502 Bad Gateway` on the domain | The edge cannot reach `<forge-ip>:3880` | Check `api_listen` is not `127.0.0.1`, and that a firewall allows the edge |
| Terminal or live updates fail on the domain with `origin not allowed` | The edge is not in `trusted_proxies`, so Forge sees `http` while the browser uses `https` | Add the edge to `trusted_proxies` and restart, or pin the public address to exactly the origin you open |
| `421 Misdirected Request` | The domain or address is not in **Allowed hosts** | Add it in **Settings → Server**, or run `forge-server allowed-hosts clear` on the server host and restart |
| `value: use the origin only…` when you save | The address has a path, query or user name | Enter only `https://` and the host, with a port only when it is not 443 |
| Sign-in on the domain works but you are signed out on the next page | `cookie_secure = true` is fine; with `false`, the edge is not in `trusted_proxies`, so cookies lack `Secure` and the browser may drop them | Add the edge's address to `[security] trusted_proxies` and restart |
| Sign-in limits hit everyone at once | Forge sees every request as coming from the edge | Add the edge to `trusted_proxies` so Forge uses `X-Forwarded-For` |
| A passkey does not work on the domain | A passkey belongs to the name it was registered on | Sign in with password and code on the domain and register a passkey there |
| SSO returns `redirect_uri mismatch` | The IdP does not know the domain's callback, or the server was not restarted | Register the callback URL from step 5 and restart `forge-server` |
| `forge-server public-address set` changed nothing in the running console | The server reads the setting at start | Restart `forge-server`, or set it in the console or with `forge settings` |

:::caution
Only list your own edge nodes in `trusted_proxies`. A trusted address can set
the client address, host and scheme Forge sees. Emails, alerts and SSO never
use a request's host: they use the pinned public address.
:::

## Next steps

- [Ingress](/docs/guides/ingress/): profiles, edges and certificates.
- [Configuration: public address](/docs/reference/configuration/#public-address)
  and [`trusted_proxies`](/docs/reference/configuration/#security).
- [`forge settings public-address`](/docs/reference/cli/settings/#forge-settings-public-address-set).
- [Email alerts](/docs/guides/email-alerts/).
