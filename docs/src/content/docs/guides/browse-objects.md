---
title: Browse buckets and objects
description: Create buckets and upload, download and delete objects in an object store.
sidebar:
  order: 13
---

This guide shows how to work with buckets and objects in any object store,
managed or connected, from the object explorer.

## Before you begin

| Task | Action | Roles | MFA |
| --- | --- | --- | --- |
| List stores | `view` | every role | no |
| List and create buckets, browse, upload, download, delete objects, test the connection | `storage.data` | **Developer**, **Operator**, **Admin**, **Platform admin** | no |
| Delete a bucket | `storage.manage` | **Operator**, **Admin**, **Platform admin** | yes |

Forge checks these actions on the store's member node for a managed store and
on the workspace for a connected store. For a store that belongs to an
environment, `view` and `storage.data` also count when you hold them on that
environment. Deleting a bucket always needs `storage.manage` on the node or
workspace. See [Ownership](/docs/concepts/object-storage/#ownership).

Without any of these roles, a
[bucket grant](/docs/guides/grant-bucket-access/) lets you browse one bucket. You
then see only the granted buckets of the store. A `read` grant lets you list
and download objects; a `readwrite` grant also lets you upload and delete
them. Creating and deleting buckets and **Test connection** stay closed.
Run `forge storage my-grants` to see the grants you hold.

The control plane must reach the store's endpoint. See
[Where S3 calls run](/docs/concepts/object-storage/#where-s3-calls-run).

## Steps

1. Open **Object Storage** and select a store. Its buckets are listed on the
   left, under **Buckets**.
2. To create a bucket, select **+** (**Create bucket**) and enter a name of 3–63
   characters from `a-z`, `0-9`, `.` and `-` that starts and ends with a
   letter or digit.
3. Select a bucket to open the object explorer. Folders are key prefixes
   split on `/`; select one to open it and use the breadcrumbs to go back.
   Lists load 200 entries at a time; select **Load more** for the next page.
   Type in **Filter by key prefix** to narrow the list.
4. To upload, select **Upload** and choose files. They land in the folder you
   are in. Each file can be up to 5 GiB and shows its progress. Larger files
   need multipart uploads, which are not yet supported.
5. To download, select the download icon on the object row. The browser saves
   it as an attachment.
6. To delete an object, select the delete icon and confirm. To delete a
   bucket, empty it first, then select the delete icon next to it.

From the command line, the same tasks are
[`forge storage objects`](/docs/reference/cli/storage/#forge-storage-objects),
[`object-upload`](/docs/reference/cli/storage/#forge-storage-object-upload),
[`object-download`](/docs/reference/cli/storage/#forge-storage-object-download)
and [`object-delete`](/docs/reference/cli/storage/#forge-storage-object-delete):

```sh
forge storage object-upload <store> <bucket> img/logo.png --file ./logo.png --content-type image/png
forge storage object-download <store> <bucket> img/logo.png --out ./logo.png
forge storage object-delete <store> <bucket> img/logo.png --yes
```

Through the API, an upload is a raw `PUT` with a `Content-Length`:

```sh
curl -sS -b cookies.txt -H "X-CSRF-Token: $CSRF" -H 'Content-Type: image/png' \
  -T ./logo.png "https://<forge-host>/api/v1/object-stores/<store-id>/buckets/<bucket>/object?key=img/logo.png"
```

## Verify

- The uploaded object appears in the object explorer with its size.
- Each upload, download and delete appears in the audit log as
  `storage.object.upload`, `storage.object.download` or `storage.object.delete`.
  The event's `grant_id` names the bucket grant that allowed it, or is
  `null` when a role did.

## Troubleshooting

| Symptom | Cause | Fix |
| --- | --- | --- |
| `429 busy` | The server already runs 8 transfers | Retry when other transfers finish |
| `413 too_large` | The file is over 5 GiB | Upload it with an S3 client directly |
| `411 length_required` | An API client sent no `Content-Length` | Send the body with a known length |
| `409` when deleting a bucket | `BucketNotEmpty` | Delete the objects first |
| `409` when creating a bucket | The name is taken (names are global on AWS) | Choose another name |
| `502 store_unreachable` | The control plane cannot reach the endpoint | See [Install an object store](/docs/guides/install-object-store/#troubleshooting) |
| `403` on a bucket you could open before | Your bucket grant expired or was revoked | Ask someone with `storage.manage` on the store for a new grant |

## Next steps

- [Grant a person access to a bucket](/docs/guides/grant-bucket-access/) so someone without a role can browse one bucket
- [Issue object store access keys](/docs/guides/object-store-access-keys/) so a service can use a bucket
- [Connect S3 storage](/docs/guides/connect-s3-storage/)
- [REST API: object storage](/docs/reference/rest-api/#object-storage)
