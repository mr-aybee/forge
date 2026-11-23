---
title: Credential vault
description: Keep accounts for nodes, services and clusters in the vault, reveal a password with an MFA check, and rotate it.
sidebar:
  order: 9
---

This guide shows you how to store an account in the credential vault, reveal
its password and rotate it. The vault has two tabs under
**Administration → Credential vault**:

- **Credentials**: accounts you keep for people.
- **System secrets**: logins Forge uses for onboarding. These are never
  shown; you can only delete them.

## Before you begin

- Only **Platform admin** can use the vault. Everyone else, including
  **Admin**, gets **Platform administrators only**, and the attempt is
  recorded in the audit log.
- Revealing a password is the `secret.read` action. It needs an MFA-verified
  session and a browser session: API keys cannot reveal passwords.
- Adding, changing and deleting credentials is the `secret.manage` action on
  the platform. It also needs an MFA-verified session, so a leaked API key or
  a session without MFA cannot overwrite or delete stored passwords. Listing
  shows names and metadata only and needs no MFA.

## Add a credential

1. Open **Administration → Credential vault** and select **New credential**.
2. Fill in the fields:

   | Field | Notes |
   | --- | --- |
   | **Name** | For example *Postgres root on db-1* |
   | **Username** | Copied with one click from the list |
   | **Password** | Encrypted. **Generate** makes a strong 24-character one |
   | **URL** | Optional: `https://`, `ssh://` or `rdp://` |
   | **Tags** | For filtering, for example `prod` or `database`. Press <kbd>Enter</kbd> or a comma to add |
   | **Linked to** | The nodes, services and clusters the account belongs to |
   | **Notes** | **Not** encrypted: never put secrets here |

3. Select **Save credential**.

Filter by tag or link type, or search across names, users, URLs, tags and
links. A node's **Credentials** tab lists the credentials linked to it.

## Reveal a password

1. Select the dots in the password column, or open the row menu and select
   **Reveal password**.
2. If your session is not MFA-verified, confirm in **Verify it's you**.

The password is shown for 30 seconds, then hidden and removed from the page.
**Copy password** in the row menu copies it without showing it. Clear your
clipboard when you are done. Every reveal is recorded in the audit log
(`vault.reveal`), never the password itself.

## Rotate a password

1. Open the row menu and select **Edit**.
2. Enter the **New password**. Leave it empty to keep the current one.
3. Select **Save and rotate**.

The **Changed** column's tooltip shows when the password last changed.

## Verify

- The credential appears in the list with its tags and links.
- After a reveal, **Operations → Audit log** shows `vault.reveal`.

## Troubleshooting

| Symptom | Cause | Fix |
| --- | --- | --- |
| **Platform administrators only** | You are not a platform administrator | Ask a platform administrator |
| Reveal is refused through the API | Reveal needs a browser session with MFA | Reveal in the console |
| Password disappears | Revealed passwords hide after 30 seconds | Reveal it again, or use **Copy password** |

## Next steps

- API: `GET`/`POST /vault/items`, `PUT`/`DELETE /vault/items/{id}` and
  `POST /vault/items/{id}/reveal` in the [REST API](/docs/reference/rest-api/).
- [Multi-factor authentication](/docs/guides/mfa/)
- [Add nodes](/docs/guides/add-nodes/): onboarding credentials appear under
  **System secrets** when you keep them.
