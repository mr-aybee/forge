---
title: Cedar schema
description: Entity types, actions and which roles include them.
sidebar:
  order: 4
---

Source: `src/services/forge-authz/schema/forge.cedarschema`. Templates:
`policies/roles.cedar`. Guardrails: `policies/guardrails.cedar`.

## Entity types (namespace `Forge`)

| Type | Parents |
| --- | --- |
| `Platform` | — |
| `Workspace` | `Platform` |
| `Tenant` | `Workspace` |
| `Project` | `Workspace` |
| `Environment` | `Project`, `Workspace` |
| `Cluster` | `Workspace` |
| `Server` | `Environment`, `Cluster`, `Workspace` |
| `Job` | `Environment`, `Workspace`, `Cluster`, `Server`, `Platform`, `Service` |
| `Service` | `Environment`, `Workspace` |
| `Plugin` | `Platform` |
| `AuditLog` | `Workspace`, `Platform` |
| `Group` | — (a user group) |
| `User` | `Group` (principal) |

Context: `{ mfa: Bool }`.

Templates use `principal in ?principal`, so a link to a user grants that
user and a link to a `Group` grants every member. A `Service` is a
service; a job on one service (start, stop, restart, deploy)
has the service as its parent.

An environment is placed in one cluster, but `Cluster` is not its parent. A
binding on a cluster covers its servers, not the environments placed in it or
their services. Tenants have no children.

A `Project` is a product, and an `Environment` is one of its stages. A
binding on a project covers every environment in it, and the services in
those environments.

## Role binding scopes

| Scope | Format | Covers |
| --- | --- | --- |
| Platform | `platform:root` | Everything |
| Workspace | `workspace:<id>` | The workspace and every tenant, cluster, project, environment, server, service and job in it |
| Tenant | `tenant:<id>` | The tenant |
| Cluster | `cluster:<id>` | The cluster, its servers, and jobs that target them |
| Project | `project:<id>` | The project and every environment in it, with what those environments cover |
| Environment | `environment:<id>` | The environment, its servers, services and jobs that target them, and the object stores it owns (`view` and `storage.data` only) |

Access profile assignments take `workspace`, `cluster`, `project`,
`environment`, `server` and `service:<id>`. An assignment at
`server:<id>` covers that server and its jobs; one at `service:<id>`
covers that service and its jobs. See
[Access profiles](/docs/concepts/authorization/#access-profiles).

`view`, `deploy` and `job.create` also apply to a `Service`, so a grant
on one service lets its holder see it, deploy, start, stop and restart it.
`user.manage` applies to `Platform`, `Workspace`, `Tenant`, `Project`,
`Environment`, `Cluster`, `Server` and `Service`, so access can be handed
out at every assignable scope.

`platform_admin` can only be bound on `platform:root`.

## Actions

| Action | platform_admin | workspace_admin | operator | developer | dba | auditor | customer |
| --- | :-: | :-: | :-: | :-: | :-: | :-: | :-: |
| `view` | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ |
| `audit.read` | ✓ | ✓ | | | | ✓ | |
| `workspace.create` | ✓ | | | | | | |
| `system.update` † | ✓ | | | | | | |
| `analytics.configure` † | ✓ | | | | | | |
| `workspace.manage` | ✓ | ✓ | | | | | |
| `user.manage` | ✓ | ✓ | | | | | |
| `server.enroll` | ✓ | ✓ | ✓ | | | | |
| `node.onboard` † | ✓ | ✓ | ✓ | | | | |
| `server.manage` | ✓ | ✓ | ✓ | | | | |
| `server.command` † | ✓ | ✓ | ✓ | | | | |
| `process.manage` † | ✓ | ✓ | ✓ | | | | |
| `network.tunnel` † | ✓ | ✓ | ✓ | ✓ | | | |
| `network.tunnel.any` † | ✓ | ✓ | | | | | |
| `server.rename` | ✓ | ✓ | ✓ | | | | |
| `alert.view` | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | |
| `alert.manage` | ✓ | ✓ | ✓ | | | | |
| `log.read` | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | |
| `log.manage` | ✓ | ✓ | ✓ | ✓ | | | |
| `notification.manage` † | ✓ | ✓ | | | | | |
| `package.manage` † | ✓ | ✓ | ✓ | | | | |
| `proxy.manage` † | ✓ | ✓ | ✓ | | | | |
| `container.manage` | ✓ | ✓ | ✓ | | | | |
| `container.exec` | ✓ | ✓ | ✓ | | | | |
| `container.inspect_env` † | ✓ | ✓ | ✓ | | | | |
| `storage.manage` † | ✓ | ✓ | ✓ | | | | |
| `storage.data` | ✓ | ✓ | ✓ | ✓ | | | |
| `file.read` | ✓ | ✓ | ✓ | | | | |
| `file.write` † | ✓ | ✓ | ✓ | | | | |
| `file.delete` † | ✓ | ✓ | ✓ | | | | |
| `file.admin` † | ✓ | ✓ | | | | | |
| `file.node` | ✓ | ✓ | | | | | |
| `job.create` | ✓ | ✓ | ✓ | ✓ | ✓ | | |
| `job.cancel` | ✓ | ✓ | ✓ | | | | |
| `plugin.manage` † | ✓ | | | | | | |
| `plugin.invoke` | ✓ | ✓ | ✓ | ✓ | | | |
| `health.view` | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ |
| `health.configure` | ✓ | ✓ | ✓ | ✓ | | | |
| `health.command` † | ✓ | ✓ | ✓ | ✓ | | | |
| `terminal.open` † | ✓ | ✓ | ✓ | | | | |
| `deploy` | ✓ | ✓ | ✓ | ✓ | | | |
| `deploy.approve` | ✓ | ✓ | ✓ | | | | |
| `stack.manage` | ✓ | ✓ | ✓ | ✓ | | | |
| `stack.privileged` † | ✓ | ✓ | | | | | |
| `swarm.manage` † | ✓ | ✓ | | | | | |
| `docker.image.manage` | ✓ | ✓ | ✓ | | | | |
| `docker.volume.manage` | ✓ | ✓ | ✓ | | | | |
| `docker.network.manage` | ✓ | ✓ | ✓ | | | | |
| `registry.manage` † | ✓ | ✓ | | | | | |
| `webhook.manage` † | ✓ | ✓ | ✓ | ✓ | | | |
| `certificate.use` † | ✓ | ✓ | | | | | |
| `secret.read` † | ✓ | ✓ | | | | | |
| `secret.manage` † (on the platform: create, change and delete credential vault items) | ✓ | | | | | | |
| `access.manage` † (create, publish and archive access profiles; service accounts and their tokens; tunnel grants) | ✓ | ✓ | | | | | |
| `db.admin` † (declared, not used yet) | ✓ | ✓ | | | ✓ | | |

† requires an MFA-verified session (guardrail).

`process.manage` applies to a server. It covers ending a process or a whole
process tree and changing a process's priority on the node. The agent does
this with full rights on the node, so the guardrail requires an MFA-verified
session.

`notification.manage` applies to a workspace (or the platform).
`alert.view` and `alert.manage` apply to the platform, a workspace, a
cluster, an environment, a server and a service. Rules, alarms, policies
and deliveries are authorized on the workspace. A
[silence](/docs/concepts/silences/) is authorized on everything its scope names,
so an operator of one environment can silence that environment and nothing
wider. `alert.manage` covers alert rules, alarm acknowledgements,
notification policies and silences. `notification.manage` covers notification
channels: creating, changing, deleting and testing one. A channel makes the
control plane send requests to a URL someone chose, so only administrators
hold the action and the guardrail requires an MFA-verified session. See
[Why channels are stricter than policies](/docs/concepts/alerts/#why-channels-are-stricter-than-policies).

`log.read` applies to the platform, a workspace, an environment or a
service; `log.manage` to an environment or a service. The API checks both on
the service. `log.read`
covers reading its events and histogram in the **Logs** console;
`log.manage` covers listing, creating and revoking its log ingest keys.
Creating and revoking keys is audited as `service.log_key.create` and
`service.log_key.revoke`. Sending events needs no role: the ingest key is
the only credential. See [Service logs](/docs/guides/application-logs/).

`storage.manage` applies to a server or a workspace. `storage.data` applies
to a server, a workspace or an environment. An object store is not a Cedar
entity: the API checks a managed store on its first member node and a
connected store on the workspace. For a store that belongs to an environment,
the API also accepts `view` and `storage.data` held on that environment, and
`deploy` on it to attach a bucket to one of its services. `storage.manage`
is never checked on an environment, so an environment role cannot install, stop or
uninstall a store or issue standalone keys. See
[Ownership](/docs/concepts/object-storage/#ownership).

`certificate.use` applies to a workspace or a server. Applying an endpoint
with an uploaded certificate checks it on the workspace; the `agent.command`
job checks it again on each edge before it unseals the private key.

`swarm.manage` applies to a Swarm cluster (and its nodes): it joins and
removes swarm nodes and changes their role, availability and labels.
`stack.manage` and `stack.privileged` apply to an environment (or a cluster).
`stack.privileged` is needed to deploy a compose stack with host-access
settings; see [Compose extensions and policy](/docs/reference/compose-extensions/#policy).

### Port tunnel actions

`network.tunnel` and `network.tunnel.any` cover [port tunnels](/docs/concepts/port-tunnels/).
No other permission implies them: `view`, `deploy` and database rights do
not let anyone reach a service's port.

| Action | Resource types | MFA | Checked when |
| --- | --- | --- | --- |
| `network.tunnel` | `Environment`, `Service`, `Server` | Yes | Opening a tunnel to a service or stack service (on its environment) or to an App Store package (on the node); again every 15 s while the tunnel is open |
| `network.tunnel.any` | `Server` | Yes | Opening a tunnel to a raw port of a node, listed in the service catalogue or not. Administrators only; audited as `raw` |
| `terminal.open` | `Server` | Yes | Opening a tunnel to the node's SSH port (the same people who may open a terminal) |
| `access.manage` | `Workspace` | Yes | Creating and revoking tunnel grants, and tunnels or grants that never expire |

A developer bound on an environment tunnels to that environment's services, not
to a sibling environment's. A person without any role on the environment may
hold an explicit **tunnel grant** instead: an administrator issues it for
one user, one target and optionally one node and some ports, with its own
expiry. The grant alone opens the door; Cedar is not consulted for the
grantee. Raw ports are never granted.

## Changing policy

1. Edit the schema or policy files.
2. `cargo test -p forge-authz` — policies are validated strictly against the
   schema, and the role matrix test documents intended behaviour.
3. Never remove an action that persisted bindings rely on; add new ones.
