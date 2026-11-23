---
title: Audit log
description: Append-only, hash-chained, signed record of every mutating action.
sidebar:
  order: 4
---

Every mutating action — and every denied attempt — is recorded.

To find entries by time, action type, actor or outcome, see
[Search the audit log](/docs/guides/search-audit-log/). Searches run in the
database with a sequence-number cursor, so paging stays stable while new
entries arrive and reaches the oldest entry.

## Entry format

| Field | Meaning |
| --- | --- |
| `seq` | 1, 2, 3, … (unique; keeps the chain linear) |
| `at` | Time (epoch ms) |
| `actor` | `user:<id>`, `agent:<id>`, `system` or `system:forge` (Forge acting on its own, or a `forge-server` command run on the host) |
| `action` | e.g. `workspace.create`, `role.grant`, `server.enroll`, `plugin.quarantine` |
| `resource` | e.g. `server:<id>` |
| `outcome` | `success`, `denied` or `failure` |
| `details` | JSON (canonicalised before hashing) |
| `prev_hash` | Hash of the previous entry (genesis is 64 zeros) |
| `hash` | SHA-256 of the canonical JSON of all fields above |
| `signature` | Ed25519 signature of `hash` with the control plane's audit key |

Sign-in and second factors record these actions. Entries never contain
passwords, codes, challenges or key material.

| Action | Actor | Resource | Details |
| --- | --- | --- | --- |
| `auth.login` with `"method": "passkey"` | `user:<id>` on success; `passkey` on a refused sign-in, because the user is not known yet | `platform:root` | `method`; on `denied`, also `reason` |
| `auth.step_up` with `"method": "passkey"` | `user:<id>` | `user:<id>` | `method` on success. A refused step-up is `denied` with `reason` only, for passkeys and codes alike |
| `auth.passkey.add` | `user:<id>` | `user:<id>` | `passkey` (id), `name`; `denied` with `reason` when verification fails |
| `auth.passkey.remove` | `user:<id>` | `user:<id>` | `passkey` (id); `denied` with `reason` when the session did not verify a factor |
| `auth.passkey.remove_all` | the administrator, `user:<id>` | the affected `user:<id>` | `removed` (count), `sessions_revoked`, `api_keys_revoked` |
| `auth.totp.enroll` | `user:<id>` | `user:<id>` | `replaced`; `denied` with `reason` when proof of identity is missing |
| `apikey.revoke` / `apikey.revoke_all` | the key owner or an administrator | the owner's `user:<id>` | `id` and `by_admin`, or `revoked` (count) |
| `user.create` / `role.grant` | the administrator | the new user, or the binding's scope | `denied` with `reason` when the session is not MFA-verified |
| `auth.mfa.reset` | `system:forge` | `platform:root` | `user` (id), `passkeys_removed`, `api_keys_revoked`, `via: "forge-server reset-mfa"` |

Object storage records `storage.install`, `storage.connect`,
`storage.disconnect`, `storage.adopt`, `storage.start`, `storage.stop`,
`storage.restart`, `storage.uninstall`, `storage.bucket.create`,
`storage.bucket.delete`, `storage.object.upload`, `storage.object.download`,
`storage.object.delete`, `storage.key.create`, `storage.key.rotate` and
`storage.key.revoke`. Downloads are audited because they read data.

| Action | Details |
| --- | --- |
| `storage.adopt` | `store_id`, `name`, `installation_id` (the App Store record), `s3_port`, `job_id` |
| `storage.key.create` | `store_id`, `access_key`, `name`, `bucket`, `access` |
| `storage.key.rotate` | `store_id`, `old_access_key`, `access_key` (the new one), `bucket` |
| `storage.key.revoke` | `store_id`, `access_key` |

Key events record access keys, never secrets.

The file explorer records one event per call: `file.list` (with the entry
count), `file.stat`, `file.read` (path, offset and length), `file.hash`,
`file.download` (one event per download, with the size), `file.mkdir`,
`file.upload` (at commit), `file.upload.abort`, `file.move`, `file.copy`,
`file.delete`, `file.delete.permanent`, `file.trash.list`,
`file.trash.restore`, `file.trash.purge`, `file.revision.read`,
`file.search` (whether a name or content pattern was given, never the
patterns themselves), `file.root.create`, `file.root.delete`,
`file.chmod` and `file.chown` (mode or owner, counts and a before/after
sample), `file.acl`, `file.compress`, `file.extract`, `file.archive`,
`file.grant.create` (with `never_expires`), `file.grant.update` (the
changed fields with `before` and `after`, and `sessions_closed`),
`file.grant.reset_password`, `file.grant.revoke`, `file.sftp.connect` (also when a
session limit refuses a login), `file.sftp.disconnect`, `file.sftp.upload`
and `file.sftp.download`. SFTP events name the access (`grant:<id>`) as
the actor. Refusals
are recorded with outcome `denied` and the reason; failed changes with
outcome `failure`. File contents are never recorded.

## What verification detects

`GET /api/v1/audit/verify` (or **Verify chain** in the UI) walks the chain and
reports the first invalid entry:

| Tampering | Detected as |
| --- | --- |
| Editing any field | content does not match hash |
| Editing and re-hashing | invalid signature |
| Deleting an entry | sequence gap |
| Reordering or splicing | prev_hash mismatch |

Re-signing a forged chain requires the private audit key. For defence in depth,
export the head hash periodically to storage the control plane cannot write.

## Concurrency

Appends are serialised in-process, and a `UNIQUE(seq)` constraint makes a
second writer retry instead of forking the chain.
