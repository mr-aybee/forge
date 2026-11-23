---
title: Grant a person access to a bucket
description: Let one person browse one bucket of an object store through Forge, read-only or read-write, without a role on the store, and revoke the access again.
sidebar:
  order: 16
---

This guide gives one person access to one bucket of an object store, with a
**bucket grant**. The person needs no role on the store, its node, the
workspace or its environment. They work with the bucket's objects through
Forge: the console, the REST API and the `forge` CLI. For how grants fit in
with roles, ownership and access keys, see
[Bucket grants](/docs/concepts/object-storage/#bucket-grants).

:::note
A grant does not issue an S3 key. The person gets no S3 endpoint and no
credential, so an S3 client such as `aws s3` or `rclone` cannot use a grant.
For an S3 client or a service, issue an
[access key](/docs/guides/object-store-access-keys/) or
[attach the bucket to a service](/docs/guides/attach-bucket-to-service/).
:::

## Before you begin

| Task | Cedar action | Roles | MFA |
| --- | --- | --- | --- |
| Grant access, list a store's grants, revoke a grant | `storage.manage` on the store | **Operator**, **Admin**, **Platform admin** | yes |
| Use a grant | none; the grant itself | any account | no |
| List your own grants | none; you are signed in | any account | no |

- Forge checks `storage.manage` on the store's member node for a managed
  store and on the workspace for a connected store. A role on the
  environment that owns the store is not enough. Listing a store's grants
  needs the same action as changing them.
- `storage.manage` needs an MFA-verified session. The console asks you to
  verify when needed. On a terminal the CLI asks for your authenticator code;
  with `--non-interactive`, run `forge auth step-up` first. See
  [MFA](/docs/guides/mfa/).
- Grants work on managed and connected stores, on every platform a store
  runs on.
- The person has an account that is not disabled. You name them by username,
  email address or user id. A service account works too.
- The bucket exists. Create it first in the
  [object explorer](/docs/guides/browse-objects/). Forge lists the store's buckets
  to check, so the store must be reachable.
- Decide the access level:

  | Access | Console label | The person can |
  | --- | --- | --- |
  | `read` (default) | **Read** | List and download the objects of the bucket |
  | `readwrite` | **Read and write** | Also upload objects and delete objects |

:::caution
`readwrite` lets the person delete every object in the bucket. Forge keeps no
copy of a deleted object. Give `read` unless the person must change the
bucket.
:::

## Grant access in the console

1. Open **Object Storage** and select the store.
2. Find the **Access** panel. If it shows **Show access**, select it and
   verify with your authenticator code or passkey.
3. Select **Grant access**.
4. In **Person**, choose the account that gets the access.
5. In **Bucket**, choose the bucket.
6. In **Access**, choose **Read: list and download** or
   **Read and write: also upload and delete**.
7. In **Expires**, choose **Never**, **1 day**, **7 days**, **30 days** or
   **On a date…**. For **On a date…**, set the date and time in
   **Expires on**.
8. Optional: in **Note**, say why the grant exists, in up to 200 characters.
   The note is shown in the list and stored in the audit log.
9. Select **Grant access**.

The grant works at once. The person does not need to sign in again.

## Grant access with the CLI

Run [`forge storage grant-create`](/docs/reference/cli/storage/#forge-storage-grant-create).
`<store>` is the store's id, name or slug:

```sh
# Read-only, until someone revokes it
forge storage grant-create <store> --user <user> --bucket <bucket>

# Read and write for seven days, with a reason
forge storage grant-create <store> --user <user> --bucket <bucket> \
  --access readwrite --for 7d --note "<reason>"
```

`--for` takes `30m`, `8h`, `3d`, `2w`, a combination such as `1d12h`, or
`never` (the default).

With the REST API, send `lifetime_minutes` or `expires_at`, or neither for a
grant that never expires. The example assumes an MFA-verified session cookie
in `cookies.txt` and the CSRF token in `$CSRF`:

```sh
curl -sS -b cookies.txt -H "X-CSRF-Token: $CSRF" -H 'Content-Type: application/json' \
  -X POST https://<forge-host>/api/v1/object-stores/<store-id>/grants \
  -d '{"user": "<user>", "bucket": "<bucket>", "access": "read", "lifetime_minutes": 10080}'
```

The response is `201` with the grant. See
[Bucket grants](/docs/reference/rest-api/#bucket-grants) for every field.

## Verify

As the administrator:

- The **Access** panel lists the grant with **Person**, **Bucket**,
  **Access**, **Expires**, **Granted by**, **Note** and the **Status**
  **active**. From the CLI:

  ```sh
  forge storage grants <store>
  ```

- The audit log has `storage.grant.create` with the person, the bucket, the
  access and the expiry.

As the person who holds the grant:

- **Object Storage** lists the store, with **By grant** in the
  **Environment** column. The store page says that the access comes from a
  grant, and **Buckets** shows only the granted buckets.
- The person can open the bucket, browse it and download objects. With
  **Read and write** they can also **Upload** and delete objects; with
  **Read** the object explorer shows **Read only** instead. Controls they
  cannot use are hidden: creating and deleting buckets, access keys,
  **Test connection** and the lifecycle actions.
- From the CLI, the person lists what they hold and browses the bucket. The
  list shows the store, the bucket, the access and the expiry; it does not
  show who granted the access or the note:

  ```sh
  forge storage my-grants
  forge storage buckets <store>
  forge storage objects <store> <bucket>
  ```

- A download through the REST API works with the person's own session or
  API key. Pass the key from an environment variable:

  ```sh
  curl -sS -H "Authorization: Bearer $FORGE_API_KEY" -o <file> \
    "https://<forge-host>/api/v1/object-stores/<store-id>/buckets/<bucket>/object?key=<key>"
  ```

Each download, upload and delete made through a grant appears in the audit
log as `storage.object.download`, `storage.object.upload` or
`storage.object.delete`, with the `grant_id` of the grant that allowed it.

## Change or revoke a grant

A grant cannot be edited, and a person holds at most one live grant per
bucket. To change the access level or the expiry, revoke the grant, then
create a new one.

1. Open **Object Storage**, select the store and find the grant in the
   **Access** panel.
2. Open the row menu of the grant and select **Revoke**.
3. In **Revoke access for \<person\>?**, select **Revoke**.

Or from the CLI, with the id from `forge storage grants`:

```sh
forge storage grant-revoke <store> <grant-id>
```

The next request the person makes on that bucket is refused. An expired grant
ends the same way, without any action from you. Forge checks the grant when a
request starts, so a download that is already running finishes. To see grants that are over,
select **Show expired and revoked**, or run `forge storage grants <store> --all`.

Deleting a bucket through Forge revokes the grants on it. Creating a bucket
through Forge revokes any grant still live for that name, so a new bucket
never starts open to the holders of an old one. Both are audited as
`storage.grant.revoke`, with the reason `bucket deleted` or
`bucket recreated`. Removing the store itself (an uninstall that removes it,
or a disconnect) deletes its grants.

:::caution
Forge does not see a bucket that is deleted or created outside it, for
example in the store's own console or with an S3 client. A grant names its
bucket by name: if a bucket is deleted and created again outside Forge, a
grant that is still live opens the new bucket. Revoke the grants yourself
when you delete a bucket outside Forge.
:::

## Troubleshooting

| Symptom | Cause | Fix |
| --- | --- | --- |
| `403 step_up_required`, or CLI exit code `5` | Granting, listing and revoking need an MFA-verified session | Verify with your authenticator code or passkey, or run `forge auth step-up`, then retry |
| `403` when you open the **Access** panel or run `forge storage grants` | You lack `storage.manage` on the store's node or workspace. A role on the owning environment, or a grant of your own, does not let you see or manage grants | Ask an **Operator** or **Admin** of the node or workspace |
| `404` with `no bucket <bucket> in this store` | The bucket does not exist, or the name is misspelled. Names match exactly, including case | Check the name with `forge storage buckets <store>`, or create the bucket first |
| `404` with `unknown or disabled user` | No account has that id, username or email address, or the account is disabled. The answer is the same for both | Check the spelling, create the account first, or for a disabled service account enable it |
| `409` with `already has … access to bucket` | The person holds a live grant for this bucket. The message names its id | Revoke that grant, then create the new one |
| `400` with `give lifetime_minutes or expires_at, not both` | An API request set both fields | Send one of them |
| `422` with `expires_at must be a time in the future` or `the lifetime must be between 1 minute and 10 years` | The expiry is in the past, or the lifetime is out of range | Choose a later time or a lifetime from 1 minute to 10 years |
| The person gets `502` with `the store can't be reached` | The store did not answer or refused the request. A grant holder is not shown the store's address or its error text; the server log has the cause | Ask an operator of the store to check it with **Test connection** |
| `502 store_error` or `502 store_unreachable` when granting | Forge could not list the store's buckets to check the name. The store is down, or a connected store's credential may not list buckets | Start the store or fix the connection. See [Install an object store](/docs/guides/install-object-store/#troubleshooting) |
| The person does not see the store, or gets `403` on the bucket | The grant expired or was revoked, the account is disabled, or the grant names another bucket | Run `forge storage grants <store> --all` to see its status, then create a new grant |
| The person gets `403` on upload or delete | The grant is `read` | Revoke it and create a `readwrite` grant |
| The person's AI connector (MCP) cannot reach the bucket, also after the person approves the connector's request | Grants do not count for AI connectors, and not for a connector's request that its user approved. A connector gets only what its user's roles allow | Use the console, the CLI or an API key instead |
| An S3 client cannot connect, or the person asks for the endpoint and a key | A grant issues no S3 key and shows no endpoint | Issue an [access key](/docs/guides/object-store-access-keys/) for the bucket instead |

## Next steps

- [Bucket grants](/docs/concepts/object-storage/#bucket-grants): what a grant
  opens, what it never opens and how it is checked
- [Browse buckets and objects](/docs/guides/browse-objects/): what the person does
  with the bucket
- [Issue object store access keys](/docs/guides/object-store-access-keys/): S3
  access for services
- [`forge storage`](/docs/reference/cli/storage/#forge-storage-grants): every
  grant command and flag
- [REST API: bucket grants](/docs/reference/rest-api/#bucket-grants)
