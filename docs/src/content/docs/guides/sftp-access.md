---
title: Give temporary SFTP access
description: Issue, change and revoke SFTP credentials for folders on a node, so a vendor or colleague can transfer files with FileZilla, Cyberduck or WinSCP.
sidebar:
  order: 18
---

This guide shows you how to issue temporary SFTP access to folders on a node,
hand the connection details to someone, and later find, change, reset or
revoke that access. Forge runs the SFTP server itself: nobody gets a node
account or SSH key.

## Before you begin

- An administrator has turned on the SFTP gateway (`[sftp] enabled = true`
  in the Forge configuration; see [Configuration](/docs/reference/configuration/#sftp-gateway)),
  and clients can reach its port (default `2022`) through your firewall.
- You can open the folders you want to share in the **Files** tab, and hold
  the rights you give away: `file.read`, plus `file.write` for uploads and
  `file.delete` for deletes and replacing files (**Operator**, **Admin** or
  **Platform admin**).
- You have an authenticator or a passkey set up; issuing access needs an MFA
  check. See [Multi-factor authentication](/docs/guides/mfa/).

:::caution
Anyone with the user name and password can use the access until it expires
or you revoke it. Send the password through a separate channel from the
user name, give the shortest expiry that works, and restrict the addresses
when you know them.
:::

## Steps

1. Open the node (cluster → **Nodes** → the node) and select the **Files**
   tab.
2. Select **SFTP access**.
3. Enter a **Name** that says what the access is for, such as
   `Vendor attachment upload`.
4. Under **Folders**, choose the roots to share. The current root is
   selected.
5. Choose what the person can do: read only (the default), tick
   **Upload and create**, and also **Delete and replace** if needed.
6. Choose **Expires after**, or **Custom…** and a number of **Hours**, up
   to the maximum your administrator set (`[sftp] max_hours`). **Never** is
   available only when your administrator allows it and you hold
   `file.admin` on the node; otherwise the option is shown disabled with the
   reason. See [Access that never expires](#access-that-never-expires).
7. Optional: open **Restrictions** to limit the client addresses (IPs or
   CIDR ranges), the sessions at once, the bandwidth per session or the total
   transfer.
8. Select **Create SFTP access** and enter your code if asked.
9. Copy the connection details. Click a value, or the copy icon at its end,
   to copy it. Forge shows the password only once.

| Field | Example |
| --- | --- |
| Protocol | `SFTP` |
| Host | `<forge-host>` |
| Port | `2022` |
| Username | `fg3f9a6c1d2e4b5a7c` |
| Password | shown once |
| Host key | `SHA256:…` |

In the client, choose SFTP (not FTP or FTPS), enter the details, and check
that the host key fingerprint the client shows on first connect matches.
With the OpenSSH command line:

```sh
sftp -P 2022 <username>@<forge-host>
```

The person sees one folder per shared root, such as `/node` or `/Vendor-Drop`.

## Verify

- The access appears under **Temporary access** and under
  **Administration → Access → SFTP access** as **active**.
- After someone connects, the entry shows when it was last used and the
  bytes transferred.
- **Operations → Audit log** shows `file.grant.create`, then `file.sftp.connect`,
  `file.sftp.upload` and `file.sftp.download` as they work, and
  `file.grant.update`, `file.grant.reset_password` and `file.grant.revoke`
  when someone manages the access.

## See all access

**Administration → Access → SFTP access** lists every access you may see,
on every node: name, node, folders, permissions, who created it and when,
expiry, last use, bytes transferred and status (**active**, **expired** or
**revoked**). Use the search box and the node and status filters to narrow
the list. Each node's **Files** tab → **SFTP access** shows the same list
for that node under **Temporary access**.

You see every access on nodes where you hold `file.admin` (**Admin**,
**Platform admin**, with a recent MFA check), and only your own on nodes
where you hold `file.read`. Nodes you cannot open are left out.

## Change access

You can change access without giving out a new password. You need the same
rights as for issuing it: a recent MFA check, every right the changed access
carries, and either being its issuer or holding `file.admin`.

1. Open **Administration → Access → SFTP access**, or the node's **Files**
   tab → **SFTP access**.
2. Open the row's **⋯** menu and select **Edit**.
3. Change the **Name**, the **Folders** (only approved folders you can open),
   the permissions (**Upload and create**, **Delete and replace**),
   **Expires after**, or the restrictions (**Allowed addresses**,
   **Sessions at once**, **Bandwidth per session**, **Transfer allowance**).
   **Keep** leaves the expiry as it is; any other choice counts from now.
4. Select **Save changes** and enter your code if asked.

When you add a folder, **Upload and create** or **Delete and replace**, you
become the access's **owner**: from then on it acts with your rights, not
those of the person who issued it. Removing things, renaming, or changing the
expiry or restrictions keeps the owner. The list shows the owner when it
differs from the creator.

Changes apply to new sessions at once. When you take something away, such as
a folder, uploads, deletes, an address, a session, bandwidth, transfer
allowance or time, Forge closes the access's open sessions so the client
reconnects with the new settings.

## Reset the password

1. Open the row's **⋯** menu and select **Reset password**.
2. Confirm. Forge shows the new password once.

The old password stops working and open sessions close. Use this when a
password may have leaked but the access itself is still needed.

## Revoke access

Open the row's **⋯** menu, select **Revoke** and confirm. New logins are
refused and open sessions close at once. Revoked access cannot be changed or
restored; issue new access instead. Access also ends at its expiry.

Every SFTP operation is checked against the owner's rights at that moment:
if the owner loses `file.delete`, deletes through the access are refused; if
they lose `file.read` on the node, the access stops working.

## Access that never expires

An administrator can allow access without an expiry with
`[sftp] allow_never_expiry = true`
(see [Configuration](/docs/reference/configuration/#sftp-gateway)). Even then,
only users with `file.admin` on the node can choose **Never**, when issuing
or when changing access. Such access shows **never expires** in a warning
colour in both lists, the audit log records `never_expires: true`, and you
can still change, reset or revoke it.

:::caution
Access that never expires keeps working for anyone with the password until
someone revokes it or its issuer loses their role. Use it only for automated
transfers you track. Restrict **Allowed addresses**, set a **Transfer
allowance**, and review the list regularly. Turning the setting off does not
end access that already exists: revoke it or give it an expiry.
:::

## Troubleshooting

| Symptom | Cause | Fix |
| --- | --- | --- |
| "The SFTP gateway is off" | The gateway is not enabled | Ask an administrator to set `[sftp] enabled = true` and restart Forge |
| The client cannot connect | The port is blocked, or the host is wrong | Open the port on the firewall; check **Host** and **Port** |
| The client warns about an unknown or changed host key | First connection, or the key was rotated | Compare with the **Host key** shown in Forge; accept only if they match |
| `Permission denied` at login | Wrong user name or password, expired or revoked access, a client address that is not allowed, or the session limit reached | Reset the password, extend the expiry, or check **Allowed addresses** and **Sessions at once** with **Edit** |
| The client disconnects right after you edit the access | You took something away, so Forge closed open sessions | Reconnect; the new settings apply |
| **Never** is disabled in **Expires after** | `[sftp] allow_never_expiry` is off, or you lack `file.admin` on the node | Ask an administrator, or choose an expiry |
| Saving a change fails with "not allowed" | You do not hold a right the changed access would carry, or it is someone else's access and you lack `file.admin` | Ask someone with the rights, or change less |
| A second transfer window or parallel transfer fails to open | Each login and each extra SFTP channel takes one of the **Sessions at once** | Close other sessions, or issue access with more sessions |
| `Permission denied` on a file | The access is read-only, the file is protected, or deleting/replacing was not allowed | Issue access with the needed rights |
| `only sequential writes are supported` | The client writes files in parallel parts | Turn off segmented or parallel transfers for one file in the client |
| Uploading over an existing file fails | Replacing needs **Delete and replace** | Issue access that allows it, or upload under a new name |
| Clicking a value does not copy it | The browser blocks clipboard access (a notice says "Copy blocked by the browser") | Select the value and press `Ctrl+C` |
| A transfer stops partway | The access expired or was revoked, or the transfer allowance ran out | Issue new access and resume the transfer |
| Timestamps or permissions are not kept | Forge does not change times or modes through SFTP | Set them in the **Files** tab or on the node |

## Next steps

- [File explorer](/docs/concepts/file-explorer/#temporary-sftp-access): how the
  gateway checks every request.
- [REST API: Files](/docs/reference/rest-api/#temporary-sftp-access): issue,
  change and revoke access from scripts.
- [`forge nodes files sftp`](/docs/reference/cli/nodes-files/#forge-nodes-files-sftp-list):
  the same from the command line.
