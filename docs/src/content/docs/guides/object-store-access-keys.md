---
title: Issue object store access keys
description: Give a service its own access key to one bucket of a managed RustFS store, read-only or read-write, and rotate or revoke it.
sidebar:
  order: 14
---

This guide issues an access key for one service, limited to one bucket of
a managed RustFS store, with read-only or read-write access. It also shows how
to rotate and revoke the key. For how keys fit into a store, see
[Access keys](/docs/concepts/object-storage/#access-keys).

:::note
Access keys are for services and S3 clients. To let a person browse one
bucket in the console or with the `forge` CLI, without handing out a secret,
[grant them access to the bucket](/docs/guides/grant-bucket-access/) instead.
:::

## Before you begin

| Task | Cedar action | Roles | MFA |
| --- | --- | --- | --- |
| List keys | `storage.data` | **Developer**, **Operator**, **Admin**, **Platform admin** | no |
| Issue, rotate and revoke keys | `storage.manage` | **Operator**, **Admin**, **Platform admin** | yes |
| Reveal a key's secret | Credential vault access and `secret.read` | **Platform admin** only | yes |

- Forge checks these actions on the store's member node. For a store that
  belongs to an environment, listing keys also works with `storage.data` on
  that environment; the list then holds only the keys attached to that
  environment's services. Issuing, rotating and revoking always need
  `storage.manage` on the node. See
  [Ownership](/docs/concepts/object-storage/#ownership).
- To give a service of the owning environment a key, you can
  [attach a bucket](/docs/guides/attach-bucket-to-service/) instead. That
  needs `deploy` on the environment and no `storage.manage`.
- The store is a **managed RustFS** store (`mode: managed`, `provider: rustfs`)
  with status `running`. This includes an
  [adopted App Store container](/docs/guides/adopt-app-store-rustfs/). Standalone
  and distributed stores are both supported.
- Connected stores are not supported. Forge returns `409` for them. Create the
  key at the provider (AWS IAM, the R2 dashboard, the MinIO console) instead.
- The bucket exists. Create it first in the
  [Object Explorer](/docs/guides/browse-objects/).
- `storage.manage` needs an MFA-verified session. In the console, Forge asks
  for your authenticator code when needed (see [MFA](/docs/guides/mfa/)).
- For the REST API: you have the store id (`id` in
  `GET /api/v1/object-stores`), and you keep an MFA-verified session cookie in
  `cookies.txt` and the CSRF token in `$CSRF`. The API examples use these.

:::caution
The secret goes **only** into the credential vault. Forge never shows it on the
store page and never returns it from the API. Only a platform administrator can
reveal it. Plan who hands the secret to the service before you issue the
key.
:::

## Issue a key

1. Open **Object Storage**, then the store. The **Access keys** section is
   below the bucket list.
2. Select **New key**.
3. In **Name**, enter what the key is for: 1 to 32 letters, digits, `-`, `_`
   or `.`, for example `billing-api`.
4. In **Bucket**, choose the bucket. The list shows the store's buckets.
5. In **Access**, choose `read-write` (the default) or `read`.
6. Select **Create key**. Forge checks that the bucket exists, creates the key
   on the store and seals its secret in a new vault item. The dialog shows the
   access key with a **Copy** button, and a link to the **Credential vault**.
   It never shows the secret.
7. A platform administrator opens **Administration → Credential vault**, finds
   the item `<store name> / <bucket> (billing-api)` (tag `object-storage`),
   and selects **Reveal password** or **Copy password**. Forge asks for an
   authenticator code. The **Username** is the access key and the **URL** is
   the store's S3 endpoint.
8. Configure the service with the endpoint, the access key, the secret,
   region `us-east-1` and path-style addressing. Pass the secret through the
   service's secret store or an environment variable, never in a URL or a
   config file in source control.

With the REST API, send the request instead of steps 1 to 6:

```sh
curl -sS -b cookies.txt -H "X-CSRF-Token: $CSRF" -H 'Content-Type: application/json' \
  -X POST https://<forge-host>/api/v1/object-stores/<store-id>/keys \
  -d '{"name": "billing-api", "bucket": "<bucket>", "access": "read"}'
```

The response is `201` with the access key and the vault item id, never the
secret:

```json
{"access_key": "<access-key>", "name": "billing-api", "bucket": "<bucket>", "access": "read", "vault_item_id": "<vault-item-id>"}
```

The key is a RustFS service account of the store's root user with an inline
policy. It can reach this one bucket and nothing else:

| Resource | `read` | `readwrite` |
| --- | --- | --- |
| `arn:aws:s3:::<bucket>` | `s3:ListBucket`, `s3:GetBucketLocation`, `s3:ListBucketMultipartUploads` | same |
| `arn:aws:s3:::<bucket>/*` | `s3:GetObject` | `s3:GetObject`, `s3:PutObject`, `s3:DeleteObject`, `s3:AbortMultipartUpload`, `s3:ListMultipartUploadParts` |

The key has no `s3:ListAllMyBuckets` and cannot create or delete buckets.

## List keys

The **Access keys** section on the store page lists every service account of
the store's root user, with **Name**, **Access key**, **Bucket**, **Access**
(`read` or `read-write`), **Status** and **Source**:

- **Forge**: Forge issued the key. It shows its bucket and access, and its
  **⋯** menu has **Rotate** and **Revoke**.
- **Store**: the key was created another way, for example in the RustFS
  console. The console shows no actions for it; manage it at the store.

The store must be `running` or `degraded`; otherwise the section says why and
**New key** is disabled. For a connected store, the page says to create keys
at the provider.

With the REST API:

```sh
curl -sS -b cookies.txt https://<forge-host>/api/v1/object-stores/<store-id>/keys
```

Keys Forge issued have `managed_by_forge: true`, their `bucket`, `access` and
`vault_item_id`. Other keys show `managed_by_forge: false`. See the
[REST API](/docs/reference/rest-api/#object-storage) for every field.

## Rotate a key

Rotation replaces the key in five steps:

1. Forge creates a new key with the same name, bucket and access.
2. It tests the new key by listing the bucket. If the test fails, it retries
   for about two seconds, because a new key can take a moment to reach every
   member of a distributed store.
3. It writes the new access key and secret into the **same** vault item.
4. It deletes the old key at the store.
5. It records `storage.key.rotate` in the audit log.

To rotate, open the key's **⋯** menu in **Access keys** (or right-click its row), select **Rotate**,
then confirm. A message shows the new access key. With the REST API:

```sh
curl -sS -b cookies.txt -H "X-CSRF-Token: $CSRF" \
  -X POST https://<forge-host>/api/v1/object-stores/<store-id>/keys/<access-key>/rotate
```

The response is `200 {access_key, bucket, access, vault_item_id}` with the new
access key.

:::caution
The old key stops working as soon as rotation succeeds. Services that use
it fail until you give them the new access key and secret from the vault. To
switch without downtime, issue a second key instead, move the service to
it, then revoke the old key.
:::

You can rotate only keys that Forge issued. Other keys return `404`.

## Revoke a key

Open the key's **⋯** menu in **Access keys** (or right-click its row), select **Revoke**, then confirm.
The confirmation names the key and its bucket. With the REST API:

```sh
curl -sS -b cookies.txt -H "X-CSRF-Token: $CSRF" \
  -X DELETE https://<forge-host>/api/v1/object-stores/<store-id>/keys/<access-key>
```

Forge deletes the key at the store and, if Forge issued it, its vault item. The
response is `204`. Services that use the key lose access at once. The API
also revokes keys that Forge did not issue; the console offers **Revoke** for
Forge-issued keys only.

:::note
The store's root key cannot be revoked here (`409`). Forge uses it for bucket
and object management. It lives in the vault as `<store name> (object store root)`.
:::

## Verify

- **Access keys** on the store page lists the key with source **Forge**, its
  bucket and access. The API (`GET /api/v1/object-stores/<store-id>/keys`)
  shows it with `managed_by_forge: true`.
- The vault item exists under **Administration → Credential vault** with the
  tags `objstore:<store-id>`, `bucket:<bucket>` and `access:<access>`.
- From a host that reaches the store, test the key with the AWS CLI. Export the
  values from the vault first, so they do not end up in your shell history:

  ```sh
  export AWS_ACCESS_KEY_ID=<access-key>
  read -rs AWS_SECRET_ACCESS_KEY && export AWS_SECRET_ACCESS_KEY   # paste the secret, then Enter
  export AWS_DEFAULT_REGION=us-east-1
  aws --endpoint-url http://<node-host>:<s3-port> s3 ls s3://<bucket>/
  ```

  The listing works. With a `read` key, `aws s3 cp` to the bucket fails with
  `AccessDenied`. Listing another bucket fails with `AccessDenied`.
- The audit log has `storage.key.create`, `storage.key.rotate` or
  `storage.key.revoke` with the access key, never the secret.

## Troubleshooting

| Symptom | Cause | Fix |
| --- | --- | --- |
| `404` when you issue a key | The bucket does not exist (`NoSuchBucket`). Forge refuses to create a key for a missing bucket | Check the name or create the bucket, then retry |
| `409` with `access keys are issued for managed RustFS stores` | The store is a connected store | Create the key at the provider |
| `409` with `root key cannot be revoked here` | You tried to revoke the store's root key | Revoke app keys only. The root key stays for management and recovery |
| `400` with `key name` or `access: read or readwrite` | Invalid name or access | Use 1 to 32 of letters, digits, `-`, `_`, `.`, and `read` or `readwrite` |
| `404` with `no Forge-issued key` on rotate | The key was not issued by Forge, or its vault item was deleted | Issue a new key, move the service, then revoke the old one |
| `502 rotation_failed` | The new key did not work on the bucket. Forge deleted the new key and kept the old one; the vault item is unchanged | Check that the store is `running` and the bucket exists, then rotate again |
| Rotate returns `502 store_error` after the vault item changed | The new key works and is in the vault, but Forge could not delete the old key | Revoke the old access key with `DELETE …/keys/<old-access-key>` |
| `InvalidAccessKeyId` for a few seconds right after you issue or rotate a key on a distributed store | The new key has not reached every member yet | Wait a few seconds and retry. If it persists, check the store status for `degraded` members |
| `AccessDenied` on `aws s3 ls` without a bucket | The key has no `s3:ListAllMyBuckets` | Address the bucket directly: `s3 ls s3://<bucket>/` |
| `403 step_up_required` | Your session is not MFA-verified | Confirm your authenticator code, then retry |
| `403` when revealing the secret | Only platform administrators can use the vault | Ask a platform administrator to reveal the secret and hand it over |

## Next steps

- [Object storage](/docs/concepts/object-storage/): how stores and credentials work
- [Grant a person access to a bucket](/docs/guides/grant-bucket-access/): access
  for people, through Forge, without a key
- [Credential vault](/docs/guides/credential-vault/): reveal and copy secrets
- [REST API: object storage](/docs/reference/rest-api/#object-storage): every field and status code
