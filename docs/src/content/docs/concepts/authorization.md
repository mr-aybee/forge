---
title: Authorization
description: Cedar policies, resource-scoped roles, guardrails and enforcement points.
sidebar:
  order: 2
---

Forge authorizes every request with the embedded [Cedar](https://www.cedarpolicy.com)
policy engine. The same authorizer is used at every enforcement point, so a
hidden button is never the only protection.

## Resource hierarchy

```diagram
alt: Cedar entity hierarchy
tree:
  - label: Platform
    children:
      - label: Workspace
        children:
          - Tenant
          - { label: Cluster, note: "── Server (node)" }
          - { label: Project, note: "── Environment" }
          - { label: Environment, note: "── Server / Job / Service\nplaced in one cluster, but not authorized through it" }
```

A workspace contains tenants, clusters, projects and environments. Each environment is *placed* in exactly one cluster, and its name
is unique within that cluster. `dev` can exist in `edge-eu` and in
`windows-workers` as two separate environments. An environment has its project as a
second parent, so a binding on the project reaches all of its environments
(see [Projects, environments and services](/docs/concepts/projects-environments-services/)).

Placement is not authorization ancestry. In Cedar, an environment's parents are
the workspace and its project, not the cluster. A server sits inside its environment, its cluster and
the workspace, so a grant on any of them reaches it. A job sits inside the
resource it targets and that resource's ancestors.

The resource's ancestry is resolved from the registry for each request, and
unknown ids fail closed. The [Cedar schema reference](/docs/reference/cedar-schema/)
lists the exact parents of each entity type.

## Roles

Roles are Cedar *action groups*. Granting a role creates a **role binding**
(user + role + scope), stored in the database and linked as a Cedar template:

```cedar
permit (principal in ?principal, action in Forge::Action::"role:operator", resource in ?resource);
```

`principal in ?principal` matches the user itself, or a group the user is a
member of (see [Groups](#groups)).

`resource in ?resource` means a binding on an environment covers the environment
and everything inside it. A binding's scope is one of `platform:root`,
`workspace:<id>`, `tenant:<id>`, `cluster:<id>`, `project:<id>` or
`environment:<id>`.

| Role | Typical scope | Can |
| --- | --- | --- |
| `platform_admin` | Platform only | Everything, including creating workspaces and managing plugins |
| `workspace_admin` | Workspace | Manage tenants, clusters, projects, environments, users and roles in the workspace; operate servers |
| `operator` | Workspace / cluster / environment | Enroll and manage servers, run and cancel jobs, invoke plugins, approve waiting deployments |
| `developer` | Environment | View, create jobs, deploy services, invoke plugins |
| `dba` | Environment | View, create jobs, database administration (Phase 5) |
| `auditor` | Platform / workspace | View and read the audit log |
| `customer` | Tenant / environment | View only |

## Access profiles

An **access profile** is a named, explicit list of actions, for access the
seven roles don't express ("view the UAT nodes and restart one service, no
shell"). You give a profile to a user, a service account or a **group** at
a **scope**: a workspace, cluster, project (`project`), environment
(`environment`), node (`server`) or service (`service`), and everything
under it. That is an **assignment**. The console calls a profile a
**role**.

- **Cedar stays the only engine.** Each published revision of a profile is a
  Cedar template built from the action list alone:
  `permit (principal in ?principal, action in [...], resource in ?resource)`.
  An assignment links it with the person or group and the scope. No name or
  other text you type ever becomes policy text.
- **Built-in profiles** mirror the roles exactly and are generated from them
  at every start, so they never drift. They are read-only; **duplicate** one
  to change it.
- **No hidden growth.** A custom profile holds exactly the actions it lists.
  Actions Forge adds later are never added to it.
- **Revisions.** Changing a profile publishes a new, immutable revision, and
  every assignment moves to it at once. The old revisions stay for the audit
  trail. A publish names the revision it starts from, so two administrators
  editing at once can't overwrite each other.
- **Several assignments add up.** Access is the union of a person's roles,
  their own assignments and the assignments of every group they are in; the
  guardrails (MFA) still apply on top. There are no "deny" profiles.
- **Ending access.** Removing an assignment or archiving a profile ends
  access at once. An assignment with an expiry stops granting the moment it
  expires; Forge removes the record within 30 seconds.

### Who may hand out access

- Creating, publishing and archiving profiles is `access.manage`
  (**Platform admin** and **Admin**), with an MFA-verified session.
- Assigning a profile needs `user.manage` on the scope, an MFA-verified
  session, **and every action of the profile there yourself**. A delegated
  administrator can never hand out more than they hold.
- Publishing a revision checks the same for every scope the profile is
  assigned at, for the actions it adds. Preview it first with a dry run.
- Removing an assignment needs `user.manage` on its scope and no MFA, so
  removing access is never held up.
- Adding a member to a group hands out everything the group holds. It needs
  `user.manage` on the workspace, an MFA-verified session, and every action
  the group holds at each scope where it is assigned. Removing a member
  needs no MFA.

### Groups

A **group** is a named set of users. A user can be in many groups, and a
profile can be assigned to many groups and users. In Cedar, a group is an
entity (`Group`) and each user is a member of it (`User in [Group]`), so a
template linked to a group grants every member through
`principal in ?principal`. Memberships live in the store and in the engine;
a change takes effect at the next decision. Removing a member, or deleting
a group with its assignments, ends access through it at once.

Groups are local to Forge. They are not synced from an identity provider.

### Held over a scope

Some actions apply to one kind of resource only: `container.manage` acts on
a node, for example. Forge still lets you assign them at a wider scope, and
judges a grant by what it *holds over* the scope: the scope and everything
beneath it. A profile with `container.manage` at an environment or a
cluster lets its holder manage containers on the nodes under it.

The escalation guard and the effective actions on the **Access** page use
the same rule. You can hand out `container.manage` at a cluster if you hold
it over that cluster, and expanding that cluster's row for a user on
**Access** lists it there as allowed. This check is used
only to decide what may be granted and to show access. Every real request
is still authorized on the exact resource it touches.

### Service-level access

A service (`Service` in Cedar) is an assignable scope of its own. The
service list, the service page and the service actions (status, logs,
start, stop, restart, deploy) authorize on the service, whose parents are
its environment and the workspace. A grant on the environment still covers
every service in it; a grant on one service covers only that one. Restart,
start, stop and deploy run as jobs on the service, so they need `view`,
`deploy` and `job.create` there. A deploy with privileged mounts is the
exception: it needs `stack.privileged`, which is held per environment, so
that job is authorized on the environment.

## Service accounts

A **service account** is a non-person identity for CI pipelines, schedules
and other automation, named `svc-<name>`. It has no password, passkey,
authenticator or SSO link, so it never signs in; it uses **tokens**
(hashed, 90 days by default, last use shown). It can do nothing until it
gets an assignment. Its **owner** is accountable for it but lends it no
access, and the list flags an owner who is disabled or gone.

Tokens never hold MFA, so MFA-guarded actions stay refused, unless MFA is
turned off for the server. Disabling the account revokes all its tokens and
ends its sessions, terminals and event streams at once. Enabling it again
does not bring the tokens back: issue new ones. Managing service
accounts and issuing their tokens is `access.manage` with MFA; revoking a
token needs no MFA.

### Cluster scope and environment scope

A role bound on a cluster covers that cluster's nodes. It does not cover the
environments placed in the cluster, or their services. To grant access to an
environment, bind the role on the environment, or on the workspace to cover every
environment.

To cover every environment of one product, bind the role on the
project (`project:<id>`) instead.

This keeps two questions apart: who runs the machines (cluster) and who runs
a workload (environment). An operator of the `edge-eu` nodes does not get to
deploy into every environment that happens to be placed there.

Tenants do not contain environments. A tenant binding covers the tenant itself.

### Escalation guards

- `platform_admin` can only be bound at platform scope.
- Granting any role requires `user.manage` **on that scope**, so a workspace
  admin can grant roles only inside their own workspace.

## Restricted tokens

A personal API key normally acts with all of its owner's access. A
**restricted** key carries a ceiling: a list of up to 50 entries, each a
scope and the actions allowed there. The key may do an action on a resource
only when both hold:

- the owner may do it, judged by the owner's roles, profiles and groups as
  they are now, and
- one entry lists the action at a scope that covers the resource (the scope
  itself or anything beneath it).

The ceiling only narrows. It is not a grant, so it never adds access the
owner lacks, and when the owner loses a role the key loses it too. Lists are
filtered by both checks, and the escalation guard counts only what is inside
the ceiling, so a restricted key cannot hand out more than it may use. A
request outside the ceiling is refused with `outside what this token was
given` and audited as a denial.

At creation, the owner must hold every listed action over each scope, and
each scope must exist. A key still cannot create another key.

Limits:

- A job is checked against the ceiling when the key submits it. The job
  runner re-authorizes every later step against the owner's access for the
  same action and resource, not against the ceiling.
- Service-account tokens have no ceiling. A service account is narrowed by
  its own assignments instead.

## Guardrails

`forbid` policies override any `permit`. Forge ships one, in
`policies/guardrails.cedar` (shortened here):

```cedar
forbid (principal, action in [server.command, process.manage, terminal.open, secret.read, plugin.manage, …], resource)
unless { context.mfa };
```

Sensitive actions therefore require an MFA-verified session, even for a
platform administrator. The [Cedar schema reference](/docs/reference/cedar-schema/#actions)
marks every action the guardrail covers. Read-only agent commands (ping, facts, service status)
use `server.manage` and do not need step-up.

## Enforcement points

| Point | Checks |
| --- | --- |
| API | principal × action × resource (with ancestry) × context; denials are audited |
| Job runner | re-authorizes the job's principal **before every step**: revoking a role stops running jobs |
| Agent hub | client certificate, revocation list, certificate ↔ server binding |
| Agent | signature, target id, expiry, replay cache, local command allowlist |
| Plugin host | plugin state, approved permissions, and a Cedar check for the invoking user on each host call |

## Testing policies

`src/forge/crates/services/forge-authz/tests/policy_matrix.rs` checks a role × action
matrix, scope isolation between workspaces, the MFA guardrail, revocation, and
strict validation of every policy against the schema.
