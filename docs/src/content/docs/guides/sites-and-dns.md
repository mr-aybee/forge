---
title: Sites and DNS
description: Manage PowerDNS zones and records from Forge, and publish an ingress endpoint's records when you apply it.
sidebar:
  order: 8
---

This guide shows you how to manage PowerDNS zones and records from Forge and
how an endpoint publishes its own records. To publish a domain through Nginx
(endpoints, path rules, certificates, custom configuration), see
[Ingress](/docs/guides/ingress/).

## Before you begin

| Task | Cedar action | Resource | Roles |
| --- | --- | --- | --- |
| Read zones and records | `server.manage` | The PowerDNS node | **Platform admin**, **Admin**, **Operator** |
| Change zones and records | `proxy.manage` (MFA) | The PowerDNS node | **Platform admin**, **Admin**, **Operator** |

- Install **PowerDNS** on a Linux node from the
  [App Store](/docs/guides/app-store/). Its API key stays on that node and
  Forge talks to it through the node's helper.
- Every DNS change is audited as `dns.change`.

## Manage zones and records

1. Open **Network → DNS**.
2. Select **New zone**, enter the **Domain** and the **Name servers**, and
   create it. Forge creates the zone with an SOA and NS records.
3. Pick the zone, select **Record**, set the **Name**, **Type** (A, AAAA,
   CNAME, TXT, MX, NS, SRV or CAA), values and **TTL**, then select
   **Add record**.
4. At your registrar, point the zone's NS records at the PowerDNS node to
   make it authoritative.

## Publish an endpoint's records

1. Open the endpoint's editor and select **Placement**.
2. Under **DNS**, tick **Publish DNS records with PowerDNS when applying** and pick the
   **DNS node (PowerDNS installed)**, the **Zone** (with a trailing dot),
   the **Edge addresses** (public IPs, active first), the **Mode** and the
   **TTL (s)**:
   - **Round robin**: every edge address in the record set;
   - **Active/standby**: the first address only. Switch by reordering the
     addresses and applying again.
3. Select **Save**, then **Apply to edges**. Forge sets an A or AAAA record
   for every domain of the endpoint.

When the endpoint is served by a cluster's edge group whose profile
publishes DNS, the profile owns the records for every endpoint and moves
them off failed edges. See [Ingress](/docs/guides/ingress/#set-up-a-cluster-edge-group).

## Verify

- `dig +short <domain> @<powerdns-node>` returns the edge addresses.
- `dig +short <domain>` from outside returns the same addresses once the
  registrar delegates to PowerDNS.

## Troubleshooting

| Symptom | Cause | Fix |
| --- | --- | --- |
| **DNS** shows "Install PowerDNS on a node" | No node has PowerDNS | Install **PowerDNS** from the App Store |
| "DNS zone must end with a dot" on save | The zone has no trailing dot | Write `example.com.` |
| "… is not inside zone …" on save | A domain of the endpoint is outside the zone | Pick the matching zone, or split the endpoint |
| Records exist but the world does not see them | The registrar does not delegate to the PowerDNS node | Set the zone's NS records at the registrar |
| Let's Encrypt fails on a new endpoint | The domain does not point at the edges yet | Publish DNS first, then apply again |

## Next steps

- [Ingress](/docs/guides/ingress/)
- [App Store](/docs/guides/app-store/) for PowerDNS.
