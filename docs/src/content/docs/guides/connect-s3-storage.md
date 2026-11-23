---
title: Connect S3 storage
description: Connect AWS S3, an S3-compatible cloud or an existing S3 server so you can browse it in Forge.
sidebar:
  order: 12
---

This guide connects an S3 store that Forge does not run, so you can browse its
buckets and objects from the console. Connected stores have no lifecycle
actions. See [Object storage](/docs/concepts/object-storage/) for the model.

## Before you begin

- You need `storage.manage` on the workspace (role **Operator**,
  **Admin** or **Platform admin**) and an MFA-verified session.
- You have an access key and a secret key for the store.
- `forge-server` can reach the endpoint over HTTP or HTTPS.
- To assign the store to an environment, you also need `deploy` on that
  environment (role **Developer**, **Operator**, **Admin** or
  **Platform admin**). See [Ownership](/docs/concepts/object-storage/#ownership).

## Steps

1. Open **Object Storage** and select **Connect existing**.
2. Enter a name and choose the provider.
3. Enter the **Endpoint** and **Region** from the table below. For AWS S3 the
   console fills in the endpoint from the region.
4. Set **Path-style addressing** as the table says. The console turns it on for
   every provider except AWS S3; turn it off for Wasabi and Backblaze B2.
5. Enter the **Access key** and **Secret key**. The secret is sealed in the
   vault and never shown again.
6. If the key is scoped to one bucket and cannot list all buckets, enter that
   bucket. Forge then checks access by listing one key in it instead.
7. Optional: in **Environment**, choose the environment the store belongs to.
   Its roles can then see the store and browse its buckets. Leave it on
   **Shared (no environment)** for a shared store.
8. Select **Check and connect**. Forge tests the credential first. If the test fails, nothing is
   saved.

| Provider | `provider` | Endpoint | Region | Path-style |
| --- | --- | --- | --- | --- |
| AWS S3 | `aws` | `https://s3.<region>.amazonaws.com` | for example `eu-west-1` | off |
| Cloudflare R2 | `r2` | `https://<account-id>.r2.cloudflarestorage.com` | `auto` | on |
| Wasabi | `wasabi` | `https://s3.<region>.wasabisys.com` | for example `eu-central-1` | off |
| Backblaze B2 | `b2` | `https://s3.<region>.backblazeb2.com` | for example `us-west-004` | off |
| MinIO | `minio` | `https://<minio-host>:9000` | `us-east-1` | on |
| Existing RustFS or Ceph | `rustfs`, `ceph` | `http://<node-host>:<port>` | `us-east-1` | on |
| Garage or any other S3 server | **S3-compatible** (`s3`, default) or **Other** (`other`) | `http(s)://<host>[:<port>]` | as the provider documents | on for self-hosted servers |

Use path-style for self-hosted servers and for any endpoint without wildcard
DNS for bucket subdomains. The endpoint is `http://` or `https://`, a host and
an optional port, with no path.

The same request through the API. Pass the keys from environment variables:

```sh
curl -sS -b cookies.txt -H "X-CSRF-Token: $CSRF" -H 'Content-Type: application/json' \
  -X POST https://<forge-host>/api/v1/object-stores/connect \
  -d "{\"name\":\"Backups\",\"provider\":\"aws\",\"endpoint\":\"https://s3.eu-west-1.amazonaws.com\",\"region\":\"eu-west-1\",\"access_key\":\"$S3_ACCESS_KEY\",\"secret_key\":\"$S3_SECRET_KEY\"}"
```

Add `"environment_id": "<environment-id>"` to the body to assign the store to an
environment. To change or clear the owner later, see
[Assign a store to an environment](/docs/guides/install-object-store/#assign-a-store-to-an-environment).

### Minimum AWS IAM permissions

Grant only what you use:

| Permission | Needed for |
| --- | --- |
| `s3:ListAllMyBuckets` | Connecting without a bucket, and the bucket list |
| `s3:ListBucket` | Browsing objects; connecting with a bucket |
| `s3:GetObject` | Downloads |
| `s3:PutObject` | Uploads |
| `s3:DeleteObject` | Deleting objects |
| `s3:CreateBucket` | Creating buckets (optional) |
| `s3:DeleteBucket` | Deleting buckets (optional) |

A bucket-scoped policy:

```json
{
  "Version": "2012-10-17",
  "Statement": [
    {"Effect": "Allow", "Action": "s3:ListBucket", "Resource": "arn:aws:s3:::<bucket>"},
    {"Effect": "Allow", "Action": ["s3:GetObject", "s3:PutObject", "s3:DeleteObject"],
     "Resource": "arn:aws:s3:::<bucket>/*"}
  ]
}
```

This policy has no `s3:ListAllMyBuckets`, so enter the bucket name when you
connect.

## Verify

- The store shows status `connected` on **Object Storage**.
- Testing the connection returns `ok` and the number of buckets. The test runs `ListBuckets`, so it reports an error for bucket-scoped keys even when browsing the bucket works.

## Troubleshooting

| Symptom | Cause | Fix |
| --- | --- | --- |
| `502 store_unreachable` | DNS, firewall or TLS failure between `forge-server` and the endpoint | From the control plane host, run `curl -I <endpoint>` |
| `502 store_error` with `AccessDenied` | The key lacks `s3:ListAllMyBuckets` | Enter the bucket the key is scoped to, or grant the permission |
| `502 store_error` with `SignatureDoesNotMatch` or a region error | Wrong secret or region | Use the region the provider shows for the bucket |
| Bucket not found although the name is right | Virtual-host addressing on a server without bucket DNS | Turn on path-style |
| `404` with `environment not found` | The environment you chose does not exist in this workspace | Choose an existing environment, or leave the store shared |
| `403` although you have `storage.manage` | You chose an environment where you lack `deploy` | Ask for a role with `deploy` on it, or leave the store shared and let someone else assign it |

To remove a connected store, select **Disconnect**. Forge deletes the store
and its vault credential. Data at the provider is not touched.

## Next steps

- [Browse buckets and objects](/docs/guides/browse-objects/)
- [Credential vault](/docs/guides/credential-vault/)
- [REST API: object storage](/docs/reference/rest-api/#object-storage)
