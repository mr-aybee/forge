---
title: forge sites and forge certificates
description: Create, preview, apply, remove, renew and delete ingress sites, and upload, list and delete TLS certificates.
sidebar:
  order: 19
---

`forge sites` manages sites (endpoints): domains that Nginx edge nodes route
to your upstreams. `forge certificates` manages the TLS certificates you
upload for them. For the tasks behind these commands, see
[Sites and DNS](/docs/guides/sites-and-dns/) and [Ingress](/docs/guides/ingress/).
Every command takes the [global flags](/docs/reference/cli/#global-flags).

## Commands

| Command | What it does |
| --- | --- |
| [`forge sites list`](#forge-sites-list) | Lists the sites of the workspace |
| [`forge sites get`](#forge-sites-get) | Shows one site, with its spec and per-edge status |
| [`forge sites create`](#forge-sites-create) | Creates a site from a JSON spec |
| [`forge sites update`](#forge-sites-update) | Replaces a site's spec |
| [`forge sites preview`](#forge-sites-preview) | Renders the Nginx configuration of a spec without saving it |
| [`forge sites apply`](#forge-sites-apply) | Pushes the site to its edge nodes and publishes its DNS records |
| [`forge sites remove`](#forge-sites-remove) | Takes the site off its edge nodes and keeps the definition |
| [`forge sites renew`](#forge-sites-renew) | Renews the site's Let's Encrypt certificate now |
| [`forge sites delete`](#forge-sites-delete) | Deletes a site definition |
| [`forge certificates list`](#forge-certificates-list) | Lists uploaded certificates |
| [`forge certificates create`](#forge-certificates-create) | Uploads a certificate chain and private key |
| [`forge certificates delete`](#forge-certificates-delete) | Deletes an uploaded certificate |

Where a command takes `<site>`, give the site's id or its name. A name is
matched without regard to case and must match exactly one site.

### forge sites list

Lists the sites of the workspace. Needs `view` on the workspace.

**Synopsis**

```text
forge sites list [OPTIONS]
```

**Arguments and flags**

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `--limit <N>` | integer | `50` | no | Rows per page. `0` prints every row | `--limit 100` |
| `--page <N>` | integer, from 1 | `1` | no | Page to print. The footer under the table says which page comes next | `--page 2` |
| `--all` | flag | off | no | Every row on one page (same as `--limit 0`). Cannot be combined with `--page` | `--all` |

**Examples**

```sh
# Every site, as a table
forge sites list
# Site names only, for scripts
forge -o json sites list | jq -r '.result[].name'
```

**Output**

```text
$ forge sites list
ID  NAME  UPDATED
```

```json
$ forge -o json sites list
{
  "result": [],
  "schema": "forge.cli/v1"
}
```

The capture comes from a system without sites. Each element of `result` has
the shape shown under [`forge sites get`](#forge-sites-get).

Exit codes: `0` success; `3` not signed in; `4` permission denied; `9` server
unreachable.

**See also**: [`forge sites get`](#forge-sites-get),
[`forge clusters routing`](/docs/reference/cli/clusters/#forge-clusters-routing),
[Sites and DNS](/docs/guides/sites-and-dns/).

### forge sites get

Shows one site: its stored spec, edges, targets, DNS settings and the status
on each edge. Needs `view` on the workspace. Basic-auth users show only
their `user`; Forge never returns password hashes.

**Synopsis**

```text
forge sites get [OPTIONS] <SITE>
```

**Arguments and flags**

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `<site>` | site id or name | — | yes | The site to show | `shop` |

**Examples**

```sh
# Show the site
forge sites get shop
# Status of the site on each edge node
forge -o json sites get shop | jq '.result.edge_status'
```

**Output**

The table output prints the site as a key/value list. With `-o json`:

```json
{
  "schema": "forge.cli/v1",
  "result": {
    "id": "<site-id>",
    "workspace_id": "<workspace-id>",
    "name": "shop",
    "config": {
      "spec": { "name": "shop", "domains": ["shop.example.com"], "upstreams": ["192.0.2.11:8080"], "tls": "acme", "…": "…" },
      "placement": { "kind": "cluster", "cluster_id": "<cluster-id>" },
      "edges": ["<edge-node-id>"],
      "cluster_id": "<cluster-id>",
      "targets": [],
      "standby_targets": [],
      "dns": null
    },
    "edge_status": {
      "<edge-node-id>": {
        "status": "applied",
        "error": null,
        "job_id": "<job-id>",
        "updated_at": 1794554342323,
        "cert": { "checked_at": 1794554342323, "days_left": 84 }
      }
    },
    "created_at": 1794554342323,
    "updated_at": 1794554342323
  }
}
```

Shape from the server's schema; not captured on a live system. Per edge,
`status` is `applied`, `removed`, `failed` or `pending`. `cert` appears only
for Let's Encrypt sites, once Forge has checked the certificate.

Exit codes: `0` success; `2` the name matches more than one site; `3` not
signed in; `4` permission denied; `6` no such site; `9` server unreachable.

**See also**: [`forge sites list`](#forge-sites-list),
[`forge sites update`](#forge-sites-update).

### forge sites create

Creates a site from a JSON spec. The site is saved but not yet on any edge;
run [`forge sites apply`](#forge-sites-apply) next. Needs `workspace.manage`
on the workspace (**Admin** or **Platform admin**). A non-empty `custom`
configuration also needs `server.command` on every edge, with an
MFA-verified session, and a static directory needs `file.read` on every edge
and an approved file root there.

**Synopsis**

```text
forge sites create [OPTIONS] --file <FILE>
```

**Arguments and flags**

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `--file <FILE>` | path, `@<path>` or `@-` | — | yes | The [site spec](#site-spec) as JSON. `@-` reads it from stdin | `--file site.json` |

**Examples**

```sh
# Check the rendered Nginx configuration first, then create the site
forge sites preview --file site.json
forge sites create --file site.json
# Create from stdin and keep the new id
forge -o json sites create --file @- < site.json | jq -r '.result.id'
```

**Output**

The table output prints the new site as a key/value list. With `-o json`,
`result` is the site, with the shape shown under
[`forge sites get`](#forge-sites-get) and an empty `edge_status`.

Shape from the server's schema; not captured on a live system.

Exit codes: `0` created; `2` invalid spec (`invalid_body`, or the server's
check: domains, upstreams, missing edges, a certificate that does not cover a
domain or has expired, a DNS zone without the trailing dot, a refused custom
directive), or a cluster without an ingress profile; `3` not signed in; `4`
permission denied; `5` MFA needed for a custom configuration; `7` a site with that
name exists; `9` server unreachable.

**See also**: [`forge sites preview`](#forge-sites-preview),
[`forge sites apply`](#forge-sites-apply),
[`forge certificates create`](#forge-certificates-create).

### forge sites update

Replaces a site's spec with the one in the file. The change takes effect on
the edges only after [`forge sites apply`](#forge-sites-apply). Needs
`workspace.manage` on the workspace; a non-empty `custom` block also needs
`server.command` on every edge, with an MFA-verified session. Each update is
audited as `site.update`, with the custom configuration, so the audit log keeps every
version of it.

**Synopsis**

```text
forge sites update [OPTIONS] --file <FILE> <SITE>
```

**Arguments and flags**

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `<site>` | site id or name | — | yes | The site to change | `shop` |
| `--file <FILE>` | path, `@<path>` or `@-` | — | yes | The complete new [site spec](#site-spec). The site keeps its name | `--file site.json` |
| `--yes` | flag | off | no | Skip the confirmation prompt | `--yes` |

:::caution
`update` replaces the whole spec. A field you leave out falls back to its
default; it does not keep its current value. Start from the stored spec:

```sh
forge -o json sites get <site> | jq '.result.config | .spec + del(.spec)' > site.json
```
:::

**Examples**

```sh
# Replace the spec, with a confirmation prompt
forge sites update shop --file site.json
# Replace and push to the edges without prompting
forge sites update shop --file site.json --yes && forge sites apply shop --yes
```

**Output**

The table output prints the updated site as a key/value list. With `-o json`,
`result` is the site, with the shape shown under
[`forge sites get`](#forge-sites-get).

Shape from the server's schema; not captured on a live system.

Exit codes: `0` updated; `1` you did not confirm; `2` invalid spec, an
ambiguous name, or no `--yes` where no prompt is possible; `3` not signed in;
`4` permission denied; `5` MFA needed for a custom configuration; `6` no such site;
`9` server unreachable.

**See also**: [`forge sites get`](#forge-sites-get),
[`forge sites apply`](#forge-sites-apply).

### forge sites preview

Validates a spec and prints the Nginx configuration the edges would run,
without saving anything or contacting a node. The table output prints only
the variant the edges will use (`http` when `tls` is `off`, else `https`) as
plain text, then the custom configuration if there is one (or the first
refused custom line), then the resolved upstreams. `-o json` prints the full answer. Needs only a signed-in session.

**Synopsis**

```text
forge sites preview [OPTIONS] --file <FILE>
```

**Arguments and flags**

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `--file <FILE>` | path, `@<path>` or `@-` | — | yes | A [site spec](#site-spec) as JSON, the same body as `create`. The placement and targets are resolved as on save | `--file site.json` |

**Examples**

```sh
# Render the configuration of a spec
forge sites preview --file site.json
# Print only the configuration the edges will run
forge -o json sites preview --file site.json | jq -r '.result.config'
```

**Output**

```text
$ forge sites preview --file site-xcweb.json
# Nginx config (http, tls off)
upstream forge_xcweb {
    server 192.0.2.11:8080;
    server 192.0.2.12:8080;
}
server {
    listen 80;
    …
}

# Upstreams
  kind=app service_id=<service-id> port=8080 -> 192.0.2.11:8080 (active), 192.0.2.12:8080 (active)
```

With `-o json`:

```json
{
  "schema": "forge.cli/v1",
  "result": {
    "variant": "http",
    "config": "<the configuration the edges will run>",
    "http": "<the managed Nginx configuration without TLS>",
    "https": null,
    "custom": "<the custom configuration file>",
    "custom_error": null,
    "edges": [],
    "upstreams": [],
    "standby": [],
    "targets": []
  }
}
```

`https` is `null` when `tls` is `off`.

Shape from the server's schema; not captured on a live system.

Exit codes: `0` success; `2` invalid spec; `3` not signed in; `9` server
unreachable.

**See also**: [`forge sites create`](#forge-sites-create),
[Custom blocks](/docs/guides/ingress/#custom-configuration).

### forge sites apply

Pushes the site to each of its edge nodes, then publishes its DNS records if
the site has a `dns` object. On each edge Forge writes the managed and custom
files, runs `nginx -t` and reloads; when the test fails, the previous files
are restored. Each edge is an audited `agent.command` job.

Needs `view` on the workspace and `proxy.manage` on every edge, with an
MFA-verified session (**Operator** or higher). A non-empty custom configuration also
needs `server.command` on every edge, and a site with `tls: "uploaded"`
needs `certificate.use` on the workspace (**Admin** or **Platform admin**),
both with MFA. Nothing changes until every check passes.

**Synopsis**

```text
forge sites apply [OPTIONS] <SITE>
```

**Arguments and flags**

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `<site>` | site id or name | — | yes | The site to push | `shop` |
| `--yes` | flag | off | no | Skip the confirmation prompt (needed without a terminal) | `--yes` |

It asks first unless you pass `--yes`, like the other commands that change
running systems.

**Examples**

```sh
# Push the site to its edges without a prompt
forge sites apply shop --yes
# Update the spec and push it; stop when an edge fails (exit 10)
forge sites update shop --file site.json --yes && forge sites apply shop --yes
# Edges whose job is still running after 150 seconds
forge -o json sites apply shop --yes | jq -r '.result.edges[] | select(.result.status == "running") | .result.job_id'
```

**Output**

The table output prints the result as a key/value list. With `-o json`:

```json
{
  "schema": "forge.cli/v1",
  "result": {
    "edges": [
      {
        "server_id": "<edge-node-id>",
        "result": { "job_id": "<job-id>", "status": "succeeded", "result": { "…": "…" } }
      }
    ],
    "dns": [
      { "name": "shop.example.com", "type": "A", "status": "succeeded", "error": null }
    ]
  }
}
```

Shape from the server's schema; not captured on a live system. Per edge,
`status` is `succeeded`, `failed` (with `error`) or `running` when the job
did not finish within 150 seconds; follow it with
[`forge jobs wait`](/docs/reference/cli/jobs/#forge-jobs-wait). `dns` is `null`
when the site publishes no records. When the cluster's ingress profile
publishes DNS itself, `dns` is `null` and `dns_managed_by` is `"profile"`.

When any edge reports `failed`, the command exits `10` with the error code
`site_failed`. The message lists `<server-id>: <error>` for each failed
edge. The table output prints the result first, then the error on stderr.
With `-o json`, stdout holds only the error envelope. On the failed edges
the previous Nginx files stay in place.

:::note
An edge whose job is still `running` does not count as failed: the command
exits `0`. Follow its job with
[`forge jobs wait`](/docs/reference/cli/jobs/#forge-jobs-wait).
:::

Exit codes: `0` applied, or still running, on every edge; `2` the site has
no edge nodes or its stored spec is invalid, or an ambiguous name; `3` not
signed in; `4` permission denied; `5` MFA needed; `6` no such site; `9`
server unreachable; `10` an edge failed (`site_failed`).

**See also**: [`forge sites remove`](#forge-sites-remove),
[`forge jobs`](/docs/reference/cli/jobs/),
[Sites and DNS](/docs/guides/sites-and-dns/).

### forge sites remove

Takes the site off each of its edge nodes. The site definition stays, so you
can apply it again later. Needs `view` on the workspace and `proxy.manage` on
every edge, with an MFA-verified session.

**Synopsis**

```text
forge sites remove [OPTIONS] <SITE>
```

**Arguments and flags**

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `<site>` | site id or name | — | yes | The site to take off its edges | `shop` |
| `--yes` | flag | off | no | Skip the confirmation prompt | `--yes` |

:::caution
The domains stop being served by these edges as soon as each edge reloads.
DNS records published for the site are not deleted.
:::

**Examples**

```sh
# Take the site off its edges, with a confirmation prompt
forge sites remove shop
# Remove and delete the definition, without prompting
forge sites remove shop --yes && forge sites delete shop --yes
```

**Output**

With `-o json`:

```json
{
  "schema": "forge.cli/v1",
  "result": {
    "edges": [
      {
        "server_id": "<edge-node-id>",
        "result": { "job_id": "<job-id>", "status": "succeeded", "result": { "…": "…" } }
      }
    ]
  }
}
```

Shape from the server's schema; not captured on a live system. Per edge,
`status` is `succeeded`, `failed` (with `error`) or `running`.

As with `apply`, a failed edge makes the command exit `10` (`site_failed`),
with `<server-id>: <error>` for each failed edge. The table output prints the
result first; with `-o json`, stdout holds only the error envelope. An edge
that is still `running` does not count as failed.

Exit codes: `0` removed, or still running, on every edge; `1` you did not
confirm; `2` the site has no edge nodes, an ambiguous name, or no `--yes`
where no prompt is possible; `3` not signed in; `4` permission denied; `5`
MFA needed; `6` no such site; `9` server unreachable; `10` an edge failed
(`site_failed`).

**See also**: [`forge sites apply`](#forge-sites-apply),
[`forge sites delete`](#forge-sites-delete).

### forge sites renew

Renews the site's Let's Encrypt certificate on every edge now
(`certbot --force-renewal`, `nginx -t`, reload). Forge also renews on its
own: it checks every six hours and renews a certificate with 5 days or fewer
left. Only sites with `tls: "acme"` can be renewed; for an uploaded
certificate, upload a new one from your CA. Needs `view` on the workspace and
`proxy.manage` on every edge, with an MFA-verified session. Each renewal is
audited as `site.certificate.renew`.

**Synopsis**

```text
forge sites renew [OPTIONS] <SITE>
```

**Arguments and flags**

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `<site>` | site id or name | — | yes | The Let's Encrypt site to renew | `shop` |
| `--yes` | flag | off | no | Skip the confirmation prompt (needed without a terminal) | `--yes` |

**Examples**

```sh
# Renew now on every edge
forge sites renew shop --yes
# Days left on each edge after the renewal
forge -o json sites renew shop | jq '.result.certificates | map_values(.days_left)'
```

**Output**

With `-o json`:

```json
{
  "schema": "forge.cli/v1",
  "result": {
    "ok": true,
    "certificates": {
      "<edge-node-id>": { "checked_at": 1794554342323, "renewed_at": 1794554342323, "renew_error": null, "days_left": 89 }
    }
  }
}
```

Shape from the server's schema; not captured on a live system. `ok` is
`false` when any edge failed; that edge's `renew_error` says why.

When `ok` is `false`, the command exits `10` with the error code
`site_failed`. The table output prints the result first, then the error on
stderr. With `-o json`, stdout holds only the error envelope, so read the
per-edge `renew_error` with
[`forge sites get`](#forge-sites-get) (`edge_status.<edge>.cert`).

Exit codes: `0` renewed on every edge; `2` the site does not use Let's
Encrypt, has no edge nodes, or the name is ambiguous; `3` not signed in; `4`
permission denied; `5` MFA needed; `6` no such site; `9` server unreachable;
`10` a renewal failed on an edge (`site_failed`).

**See also**: [`forge certificates list`](#forge-certificates-list),
[Choose certificates](/docs/guides/ingress/#choose-certificates).

### forge sites delete

Deletes a site definition. A site that is still applied on any edge cannot be
deleted; run [`forge sites remove`](#forge-sites-remove) first. Needs
`workspace.manage` on the workspace.

**Synopsis**

```text
forge sites delete [OPTIONS] <SITE>
```

**Arguments and flags**

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `<site>` | site id or name | — | yes | The site to delete | `shop` |
| `--yes` | flag | off | no | Skip the confirmation prompt | `--yes` |

**Examples**

```sh
# Delete, with a confirmation prompt
forge sites delete shop
# Delete without prompting
forge sites delete shop --yes
```

**Output**

The table output prints nothing. With `-o json`:

```json
{ "schema": "forge.cli/v1", "result": null }
```

Shape from the server's schema; not captured on a live system.

Exit codes: `0` deleted; `1` you did not confirm; `2` an ambiguous name, or
no `--yes` where no prompt is possible; `3` not signed in; `4` permission
denied; `6` no such site; `7` the site is still applied on an edge; `9`
server unreachable.

**See also**: [`forge sites remove`](#forge-sites-remove).

### forge certificates list

Lists the certificates uploaded to the workspace, with their domains, issuer
and expiry. Private keys are never returned. Needs `view` on the workspace.

**Synopsis**

```text
forge certificates list [OPTIONS]
```

**Arguments and flags**

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `--limit <N>` | integer | `50` | no | Rows per page. `0` prints every row | `--limit 100` |
| `--page <N>` | integer, from 1 | `1` | no | Page to print. The footer under the table says which page comes next | `--page 2` |
| `--all` | flag | off | no | Every row on one page (same as `--limit 0`). Cannot be combined with `--page` | `--all` |

**Examples**

```sh
# Every uploaded certificate
forge certificates list
# Certificates with fewer than 30 days left
forge -o json certificates list | jq '.result[] | select(.days_left < 30) | .name'
```

**Output**

```text
$ forge certificates list
ID  NAME  DOMAINS  EXPIRES  ISSUER
```

```json
$ forge -o json certificates list
{
  "result": [],
  "schema": "forge.cli/v1"
}
```

The capture comes from a system without certificates. Each element of
`result` has this shape (from the server's schema):

```json
{
  "id": "<certificate-id>",
  "name": "shop-2026",
  "domains": ["shop.example.com", "*.shop.example.com"],
  "issuer": "Example CA",
  "not_before": 1794554342323,
  "not_after": 1826090342323,
  "days_left": 364,
  "fingerprint": "<sha-256 of the leaf, hex>",
  "used_by": ["shop"],
  "created_at": 1794554342323,
  "updated_at": 1794554342323
}
```

`EXPIRES` and `not_after` are epoch milliseconds. `used_by` lists the sites
that use the certificate.

Exit codes: `0` success; `3` not signed in; `4` permission denied; `9` server
unreachable.

**See also**: [`forge certificates create`](#forge-certificates-create),
[Choose certificates](/docs/guides/ingress/#choose-certificates).

### forge certificates create

Uploads a certificate chain and its private key. Forge checks that both are
valid PEM, that the key belongs to the leaf certificate and that the leaf has
not expired, then seals the key. The key leaves the server only when a site
that uses it is applied, and only to that site's edges. Needs
`workspace.manage` on the workspace (**Admin** or **Platform admin**).
Audited as `certificate.upload`.

**Synopsis**

```text
forge certificates create [OPTIONS] --file <FILE>
```

**Arguments and flags**

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `--file <FILE>` | path, `@<path>` or `@-` | — | yes | JSON with the fields below. `@-` reads it from stdin | `--file @-` |

The JSON body:

| Field | Type | Description |
| --- | --- | --- |
| `name` | string | Display name, 1 to 100 characters |
| `chain_pem` | string | The certificate chain as PEM, leaf first, at most 64 KB |
| `key_pem` | string | The unencrypted private key as PEM (PKCS#8, PKCS#1 or SEC1; RSA, ECDSA P-256/P-384 or Ed25519), at most 16 KB |

The domains come from the leaf's subject alternative names (or its common
name when it has none), and the issuer from its issuer organisation or
common name. The CLI checks the body against the server's schema before
sending and exits with `2` (`invalid_body`) on a mismatch.

:::caution
The JSON file holds the private key. Build it in a pipe instead of writing it
to disk, and never commit it.
:::

**Examples**

```sh
# Upload from PEM files without writing the key to another file
jq -n --arg name shop-2026 --rawfile chain fullchain.pem --rawfile key privkey.pem \
  '{name: $name, chain_pem: $chain, key_pem: $key}' \
  | forge certificates create --file @-
# Upload and use the certificate in a site spec
CERT_ID=$(jq -n --arg name shop-2026 --rawfile chain fullchain.pem --rawfile key privkey.pem \
  '{name: $name, chain_pem: $chain, key_pem: $key}' \
  | forge -o json certificates create --file @- | jq -r '.result.id')
```

**Output**

The table output prints the certificate as a key/value list. With `-o json`,
`result` has the shape shown under
[`forge certificates list`](#forge-certificates-list), with an empty
`used_by`.

Shape from the server's schema; not captured on a live system.

Exit codes: `0` uploaded; `2` invalid body, PEM that cannot be parsed, a key
that does not match the certificate, an unsupported key type, or an expired
certificate; `3` not signed in; `4` permission denied; `9` server
unreachable.

**See also**: [`forge sites create`](#forge-sites-create) (`tls: "uploaded"`),
[`forge certificates delete`](#forge-certificates-delete).

### forge certificates delete

Deletes an uploaded certificate and its sealed key. A certificate that a site
still uses cannot be deleted; switch those sites to another certificate
first. Needs `workspace.manage` on the workspace. Audited as
`certificate.delete`.

**Synopsis**

```text
forge certificates delete [OPTIONS] <ID>
```

**Arguments and flags**

| Name | Type | Default | Required | Description | Example |
| --- | --- | --- | --- | --- | --- |
| `<id>` | certificate id | — | yes | The certificate's id from `forge certificates list`. Names are not accepted | `<certificate-id>` |
| `--yes` | flag | off | no | Skip the confirmation prompt | `--yes` |

**Examples**

```sh
# Delete, with a confirmation prompt
forge certificates delete <certificate-id>
# Delete every certificate that no site uses, without prompting
forge -o json certificates list | jq -r '.result[] | select(.used_by == []) | .id' \
  | xargs -n1 forge certificates delete --yes
```

**Output**

The table output prints nothing. With `-o json`:

```json
{ "schema": "forge.cli/v1", "result": null }
```

Shape from the server's schema; not captured on a live system.

Exit codes: `0` deleted; `1` you did not confirm; `2` no `--yes` where no
prompt is possible; `3` not signed in; `4` permission denied; `6` no such
certificate; `7` a site still uses it (the message names the sites); `9`
server unreachable.

**See also**: [`forge certificates list`](#forge-certificates-list),
[`forge sites update`](#forge-sites-update).

## Site spec

`forge sites create`, `forge sites update` and `forge sites preview` read a
JSON spec from `--file`. `--file` takes a path, `@<path>`, or `@-` to read
from stdin. Before sending, the CLI checks the body of `create` and `update`
against the server's schema and exits with `2` (`invalid_body`) on a
mismatch; see [`forge api`](/docs/reference/cli/api/). The `preview` body has no
field schema and is checked by the server only.

The spec covers the whole endpoint: its `placement`, the default rule's
`kind`, path `rules`, `redirects`, `cache`, `gzip` and `access`, as in the
[endpoint editor](/docs/guides/ingress/#edit-an-endpoint). Unknown fields are
refused. Edges whose agent is older than the server refuse the newer fields
when you apply.

| Field | Type | Default | Description |
| --- | --- | --- | --- |
| `name` | string | — (required) | Slug: lowercase letters, digits and dashes, starts with a letter, at most 60 characters. Unique in the workspace. `update` keeps the site's current name whatever you send, but the field must be present |
| `domains` | list of strings | — (required) | 1 to 50 host names. The first is the primary. `*.example.com` is allowed, but not with `tls: "acme"` |
| `http_port` | integer | `80` | Port of the plain HTTP server. Must stay `80` with `tls: "acme"` |
| `https_port` | integer | `443` | Port of the HTTPS server. Must differ from `http_port` |
| `placement` | object | from `edges` / `cluster_id` | Where the site is served: `{"kind": "nodes", "server_ids": ["<node-id>"]}` (nodes with Nginx) or `{"kind": "cluster", "cluster_id": "<cluster-id>"}` (the cluster's ingress profile decides the edges) |
| `kind` | enum: `proxy`, `balancer`, `redirect`, `static` | `proxy` | What the default `/` rule does |
| `upstreams` | list of strings | `[]` | Active upstreams as `host:port`. Leave it out when `targets` fill it. A `proxy` or `balancer` site needs at least one upstream, from this list or resolved from `targets`; a `redirect` or `static` site has none |
| `standby` | list of strings | `[]` | Standby upstreams (Nginx `backup`), used only when every active upstream is down |
| `balance` | enum: `round_robin`, `least_conn`, `ip_hash` | `round_robin` | Load-balancing method |
| `proxy` | object | see below | Proxy options of the default rule |
| `redirect` | object | none | With `kind: "redirect"`: `{"to": "https://…", "code": 301, "keep_path": true}`. `code` is 301, 302, 307 or 308 |
| `static_root` | object | none | With `kind: "static"`: `{"root": "/srv/…", "index": "index.html", "spa": false}`. `root` must be inside an approved file root on every edge |
| `rules` | list of rules | `[]` | Up to 50 path rules, matched before `/` (see below) |
| `tls` | enum: `off`, `acme`, `uploaded` | `off` | `acme`: Let's Encrypt over HTTP-01 (needs Certbot on the edges). `uploaded`: the certificate in `cert_id` |
| `acme_email` | string | `""` | Contact email for Let's Encrypt. Required with `tls: "acme"` |
| `cert_id` | id | `""` | Uploaded certificate id. Required with `tls: "uploaded"`; it must cover every domain and must not have expired |
| `force_https` | boolean | `true` | Redirect HTTP to HTTPS when TLS is on |
| `hsts` | boolean | `true` | Send `Strict-Transport-Security` on HTTPS |
| `websockets` | boolean | `true` | Pass WebSocket upgrades |
| `max_body_mb` | integer, MB | `25` | Largest request body, `1` to `10240` |
| `rate_limit` | integer, requests/s | `0` | Requests per second per client IP. `0` is unlimited |
| `rate_burst` | integer, requests | `0` | Requests allowed above the rate, `0` to `100000`. `0` is twice the rate |
| `cache` | object | none | Proxy cache: `{"key": "$scheme$request_method$host$request_uri", "extensions": [], "ttl_minutes": 60, "ignore_headers": false, "max_size_mb": 1024}` (TTL 1–525600, size 1–1048576) |
| `gzip` | object | none | Compression: `{"level": 5, "types": ["text/css", …]}`, level 1–9, at most 50 MIME types |
| `access` | object | none | `{"deny": [], "allow": [], "basic_auth": {"realm": "Restricted", "users": [{"user": "ops", "password": "…"}]}, "hotlink": {"referers": [], "extensions": ["jpg"]}}`. Addresses are IPs or CIDRs (500 in total). Send `{"user": "ops"}` without `password` to keep the stored password; Forge stores only a SHA-512 crypt hash and never returns it |
| `redirects` | list | `[]` | Up to 100 `{"from": "www.example.com" or "/old", "to": "https://…", "code": 301, "keep_path": true}`. A domain `from` must be one of the site's domains |
| `custom` | string | `""` | The custom configuration file, included inside the `server` blocks; directives from the [allowlist](/docs/guides/ingress/#custom-configuration) only |
| `cluster_id` | id | none | Older form of `placement` with `kind: "cluster"`, still read when `placement` is absent |
| `edges` | list of node ids | `[]` | Older form of `placement` with `kind: "nodes"`, still read when `placement` is absent. Ignored when `cluster_id` is set |
| `targets` | list of targets | `[]` | Typed upstreams, resolved to `host:port` into `upstreams` when you save and again on every apply |
| `standby_targets` | list of targets | `[]` | Typed standby upstreams, resolved into `standby` |
| `dns` | object | none | Publish DNS records with PowerDNS on apply (see below) |

A target is one of:

| `kind` | Other fields | Resolves to |
| --- | --- | --- |
| `node` | `server_id` (node id), `port` (1–65535) | The node's address and the port |
| `app` | `service_id` (service id), `port` | Every node the service runs on, at its cluster-network address |
| `external` | `address` (`host:port`) | The address as given |

A path rule:

| Field | Type | Default | Description |
| --- | --- | --- | --- |
| `path` | string | — (required) | Path or regex. Not `/`, which is the default rule |
| `match_kind` | enum: `prefix`, `exact`, `regex` | `prefix` | `regex` is a case-sensitive PCRE |
| `kind` | enum: `proxy`, `balancer`, `redirect`, `static` | `proxy` | What the rule does |
| `targets`, `standby_targets`, `upstreams`, `standby`, `balance` | as above | `[]` / `round_robin` | Where a `proxy` or `balancer` rule sends requests |
| `websockets` | boolean | `true` | Pass WebSocket upgrades |
| `proxy`, `redirect`, `static_root` | objects | as above | Per-rule options |

The `proxy` object:

| Field | Type | Default | Description |
| --- | --- | --- | --- |
| `preserve_host` | boolean | `true` | Send the client's `Host` upstream |
| `strip_prefix` | boolean | `false` | Remove the matched prefix. Only on prefix rules ending in `/` |
| `read_timeout_s` | integer, s | `300` | 1–3600 |
| `connect_timeout_s` | integer, s | `5` | 1–300 |
| `buffering` | boolean | `true` | Turn off for streaming responses |

The `dns` object:

| Field | Type | Default | Description |
| --- | --- | --- | --- |
| `server_id` | node id | — (required) | Node that runs PowerDNS |
| `zone` | string | — (required) | Zone with a trailing dot, such as `example.com.`. Every domain must be inside it |
| `ttl` | integer, seconds | `300` | Record TTL |
| `mode` | enum: `round_robin`, `active_standby` | `round_robin` | `round_robin` publishes every address; `active_standby` only the first |
| `addresses` | list of IP addresses | — (required) | Public addresses of the edge nodes (A and AAAA records) |

Do not send `material` or `acme_peers`: Forge fills them in for each edge.

Example `site.json`, served from a cluster's ingress profile with Let's
Encrypt:

```json
{
  "name": "shop",
  "domains": ["shop.example.com"],
  "upstreams": ["192.0.2.11:8080"],
  "standby": ["192.0.2.12:8080"],
  "balance": "round_robin",
  "tls": "acme",
  "acme_email": "ops@example.com",
  "max_body_mb": 50,
  "placement": { "kind": "cluster", "cluster_id": "<cluster-id>" },
  "redirects": [{ "from": "/old-shop", "to": "https://shop.example.com/" }]
}
```

Example with explicit edges, an uploaded certificate, a service target
and DNS:

```json
{
  "name": "api",
  "domains": ["api.example.com"],
  "tls": "uploaded",
  "cert_id": "<certificate-id>",
  "placement": { "kind": "nodes", "server_ids": ["<edge-node-id>"] },
  "targets": [{ "kind": "app", "service_id": "<service-id>", "port": 8080 }],
  "dns": {
    "server_id": "<dns-node-id>",
    "zone": "example.com.",
    "addresses": ["203.0.113.10"]
  }
}
```
