---
title: Roles and access
description: Create tenants and users, and grant roles scoped to the workspace, a tenant, a cluster, a project or an environment.
sidebar:
  order: 1
---

This guide shows how to organise the registry, create users and grant them
roles on exactly the part of the fleet they need.

## Before you begin

- Creating tenants, projects and environments needs `workspace.manage`
  (**Admin** or **Platform admin**). For an environment, you need it on the
  target cluster and on the project.
- Creating users needs `user.manage` on the platform (**Platform admin**).
- Granting and revoking roles needs `user.manage` on the binding's scope
  (**Admin** inside their workspace, or **Platform admin**).
- Creating users and granting roles also need an MFA-verified session:
  Forge asks you to step up with your code or passkey. Revoking a role does
  not, so removing access is never held up. This applies when the server
  requires MFA. With `[security] require_mfa = false` (the default), every session and API key counts
  as verified, so an API key can create users and grant roles: that is the
  operator's choice.
- The API examples use a signed-in cookie session. Mutations need the
  `X-CSRF-Token` header, or use a bearer token instead. The CLI and the
  console call the same endpoints.

For how scopes and inheritance work, read
[Authorization](/docs/concepts/authorization/) first.

## Steps

1. Optional: create a tenant, for example for a customer. Tenants are
   created through the API:

   ```http
   POST /api/v1/tenants
   {"name": "Customer A"}
   ```

2. Create a project and an environment of it in a cluster. In the console,
   open **Projects**, select **New project**, then **New environment**.
   Pick the **Project**, enter a **Name**, pick the **Cluster** and set
   **Protected** if needed. With the API:

   ```http
   POST /api/v1/projects
   {"name": "shop"}

   POST /api/v1/environments
   {"name": "prod", "cluster_id": "<cluster-id>", "project_id": "<project-id>", "protected": true}
   ```

   Names are unique within a cluster. The same name can exist in another
   cluster. The request is audited as `environment.create`.

3. Create a user. Open **Administration → Members & roles** and select
   **Add user**. Enter a **Username**, an **Email** or both, a **Name** and
   an **Initial password** of at least 12 characters. With the API:

   ```http
   POST /api/v1/users
   {"username": "dana", "email": "dana@example.com", "display_name": "Dana", "password": "<initial-password>"}
   ```

   A user needs a `username`, an `email`, or both, and signs in with either.
   Users signing in through SSO are matched by verified email instead.

4. Grant a role. In **Members & roles**, select **Grant role**. Enter the
   **Username or email**, pick the **Role** and the **Scope**: Platform,
   Workspace, Tenant, Cluster, Project or Environment, then select
   **Grant**. With the
   API:

   ```http
   POST /api/v1/role-bindings
   {"email": "dana@example.com", "role": "operator", "scope": "environment:<environment-id>"}
   ```

   `email` also accepts a username (`"email": "dana"`). `scope` is one of
   `platform:root`, `workspace:<id>`, `tenant:<id>`, `cluster:<id>`,
   `project:<id>` (a project: covers all of its environments) or
   `environment:<id>` (one environment). You need `user.manage` on that scope, so workspace admins
   can delegate inside their workspace but nowhere else.

   :::caution
   A role on a cluster covers the cluster's nodes, not the environments placed
   in it or their services. To let someone deploy into an environment, bind
   the role on the environment (or on the workspace).
   :::

5. Revoke a role when it is no longer needed. In **Members & roles**,
   select the **×** on the role badge (or **Revoke** *role* in the row menu)
   and confirm with **Revoke**. Or use the API:

   ```http
   DELETE /api/v1/role-bindings/<binding-id>
   ```

   Revocation takes effect on the next request, including the next step of
   jobs that are already running.

## Verify

- `GET /api/v1/me` returns the caller's bindings and whether the session is
  MFA-verified. Ask the user to call it, or check their row in
  **Members & roles**.
- Lists such as `/servers`, `/jobs` and `/environments` only contain what the
  caller may view. Sign in as the user and check that they see the expected
  environments and nodes.
- The user's sidebar shows only the pages their roles allow.
  `GET /api/v1/me/capabilities` lists them; see
  [What each role sees](/docs/guides/navigate-the-console/#what-each-role-sees).

## Troubleshooting

| Symptom | Cause | Fix |
| --- | --- | --- |
| **Verify it's you** appears when you grant a role or add a user (`403` `step_up_required`) | Handing out access needs an MFA-verified session | Enter your authenticator code or use your passkey. A user without an authenticator sets one up first under **Profile → Password & security** |
| `400` when granting a role | Scope kind is not one of `platform`, `workspace`, `tenant`, `cluster`, `project` or `environment` or `platform_admin` on a non-platform scope | Use a supported scope; bind `platform_admin` only on `platform:root` |
| **Add user** is missing, the **Users** tab shows **Managing users needs the Platform admin role**, or creating a user says **Only platform admins create users** (`403`) | Listing and creating users needs `user.manage` on the platform; an **Admin** holds it only on their workspace | Ask a platform admin to create the account, then grant roles on a scope you manage |
| `403` when granting a role | You lack `user.manage` on that scope | Ask a workspace or platform admin, or grant on a scope you manage |
| A developer cannot open a port tunnel to a service (`no role or grant allows a tunnel`) | `network.tunnel` is held on the service's environment, not the cluster | Bind **Developer** or **Operator** on the environment, or create a grant under **Administration → Tunnels → Grants** ([Open a port tunnel](/docs/guides/port-tunnels/)) |
| Someone needs one bucket of an object store, but no role fits without opening the whole store | Roles on the node, workspace or owning environment reach every bucket of a store | Give the person a bucket grant instead of a role ([Grant a person access to a bucket](/docs/guides/grant-bucket-access/)) |
| An operator on a cluster cannot see or deploy to an environment in it | Cluster bindings do not cover environments | Bind the role on the environment or the workspace |
| `409` when creating an environment | The cluster already has an environment with that name | Choose another name, or use the existing environment |
| `400` when creating an environment | Name empty or longer than 100 characters, or `cluster_id` or `project_id` missing or unknown | Send a valid name, an existing cluster id and an existing project id |

## Next steps

- [Authorization](/docs/concepts/authorization/): how scopes and inheritance work.
- [Cedar schema](/docs/reference/cedar-schema/): which actions each role includes.
- [REST API](/docs/reference/rest-api/#registry): every registry endpoint.
- [Working inside a cluster](/docs/guides/clusters/): environments and services per cluster.
