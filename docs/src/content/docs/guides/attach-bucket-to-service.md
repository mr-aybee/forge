---
title: Attach a bucket to a service
description: Give a service its own bucket of a managed object store, with the endpoint, bucket and access key delivered as environment variables at deploy.
sidebar:
  order: 15
---

This guide attaches a bucket of a managed object store to a service. Forge
issues an access key limited to that bucket and gives the service the
connection settings as environment variables with its next deploy. For how
this works, see
[Buckets attached to services](/docs/concepts/object-storage/#buckets-attached-to-services).

## Before you begin

- What you need depends on who
  [owns the store](/docs/concepts/object-storage/#ownership):

  | Store | Actions | Roles | MFA |
  | --- | --- | --- | --- |
  | Owned by the service's environment, and no service of another environment uses the bucket | `deploy` on that environment (environment) | **Developer**, **Operator**, **Admin**, **Platform admin** | no |
  | Owned by the service's environment, but a service of another environment already uses the bucket | `deploy` on the service's environment and `storage.manage` on the store's node | **Operator**, **Admin**, **Platform admin** | yes |
  | Shared, or owned by another environment | `deploy` on the service's environment and `storage.manage` on the store's node | **Operator**, **Admin**, **Platform admin** | yes |

  Detaching needs `deploy` on the service's environment and nothing else,
  whoever owns the store.
- The store must be a managed RustFS store with the status `running`. See
  [Install an object store](/docs/guides/install-object-store/). Connected stores
  can't be attached.
- Pick a variable prefix the service doesn't use yet. The default is `S3`.

## Steps

1. Open **Services**, select the service, and open the **Storage** tab.
2. Select **Attach bucket**.
3. Choose the **Store**. Stores owned by the service's environment come
   first, labelled **This environment**. When you choose another store, the
   dialog reminds you that attaching needs `storage.manage` and an
   MFA-verified session.
4. Choose the **Bucket**. To create one, choose **New bucket…** and enter the
   **New bucket name** (3–63 lowercase letters, digits, dots and hyphens).
5. Choose the **Access**: **Read & write** or **Read only**.
6. Set the **Variable prefix**. The box **Variables the service gets** shows
   the five names, for example with the prefix `MEDIA`:

   | Variable | Value |
   | --- | --- |
   | `MEDIA_ENDPOINT` | The store's S3 endpoint |
   | `MEDIA_REGION` | The store's signing region |
   | `MEDIA_BUCKET` | The bucket |
   | `MEDIA_ACCESS_KEY_ID` | The access key issued for this service |
   | `MEDIA_SECRET_ACCESS_KEY` | Its secret |

7. Select **Attach bucket**.
8. Deploy the service again. The variables are added when Forge sends the
   service to its nodes.

:::caution
The secret reaches the nodes the service runs on, in the service's
environment. Anyone who can read that environment on the node, or run code as
the service, can use the bucket with the access you chose.
:::

To do the same from the command line, use
[`forge services storage attach`](/docs/reference/cli/services/#forge-services-storage-attach),
then deploy:

```sh
forge services storage attach <service> --store <store> --bucket media --create-bucket --prefix MEDIA
forge services deploy <service> --wait --yes
```

## Detach a bucket

1. On the **Storage** tab, select **Detach** on the bucket's row.
2. Confirm with **Detach**.

From the command line, look up the attachment's access key and detach it
with
[`forge services storage detach`](/docs/reference/cli/services/#forge-services-storage-detach):

```sh
forge services storage list <service>
forge services storage detach <service> <access-key> --yes
```

:::caution
Detaching revokes the key at once. The service loses access to the bucket
immediately, also while it is running. The variables disappear with the next
deploy. The bucket and its objects are kept.
:::

## Verify

- The **Attached buckets** panel lists the store, bucket, access and
  endpoint. Select the entry in the **Variables** column to see the five
  variable names. `forge services storage list <service>` prints the same list.
- After the deploy, the service can list its bucket with the variables it
  was given, for example from the service's own code or start-up log.
- **Object Storage** → the store → **Access keys** lists a key named after the
  service.
- **Audit** shows `service.storage.attach` with the store, bucket,
  prefix and access key, never the secret.

## Troubleshooting

| Symptom | Cause | Fix |
| --- | --- | --- |
| "No store can be attached" | There is no managed RustFS store, or you can't see one | Install a store, or ask for a role that can view it |
| "Not verified: …" | The store is shared or belongs to another environment, so attaching needs `storage.manage` and an MFA-verified session | Verify with your code or passkey when asked |
| "Not allowed: …" | You lack `deploy` on the service's environment. Or the store is shared or belongs to another environment, and you lack `storage.manage` on it | Ask for the missing role. Or ask someone with `storage.manage` to [assign the store to the service's environment](/docs/guides/install-object-store/#assign-a-store-to-an-environment); `deploy` is then enough |
| `409` with "bucket `<bucket>` is already attached to a service of another environment" | The store belongs to your environment, but another environment's service uses this bucket, so `deploy` alone is not enough | Choose or create another bucket. Or ask someone with `storage.manage` on the store to attach it, with an MFA-verified session |
| "Another attachment of this service already uses this prefix." | Two buckets can't share a prefix | Choose another prefix, for example `BACKUP` |
| "This service already defines `<names>`." | The service's own environment has one of the five variables | Choose another prefix or remove the variable from the service |
| `404` when attaching through the API, or exit `6` from `forge services storage attach` | The bucket doesn't exist | Pass `--create-bucket` (API: `"create_bucket": true`), or create the bucket first |
| The service doesn't see the variables | It hasn't been deployed since you attached | Deploy the service again |
| A deploy fails with "the object store of bucket `<bucket>` no longer exists" | The store was removed while a bucket was attached | Detach the bucket on the **Storage** tab, then deploy again |
| The service gets `403` or `InvalidAccessKeyId` from the store | The key was rotated or the bucket detached after the last deploy | Deploy the service again |

## Next steps

- [Issue object store access keys](/docs/guides/object-store-access-keys/): rotate
  a key, or issue one for something that isn't a Forge service.
- [`forge services storage`](/docs/reference/cli/services/#forge-services-storage-list) and
  [REST API: Service storage](/docs/reference/rest-api/#service-storage)
- [Object storage](/docs/concepts/object-storage/)
