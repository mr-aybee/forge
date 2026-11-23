---
title: Search the audit log
description: Search the audit log by text, time range, outcome, action type and actor, read each entry, and verify the hash chain.
sidebar:
  order: 21
---

Use the **Audit log** page to find out who did what, and when. You can
search the log, narrow it by time, outcome, action type and actor, see the
matching entries over time, and check that no entry was changed.

## Before you begin

- You need the `audit.read` action: the **Auditor**, **Workspace admin** or
  **Platform admin** role. Other roles see "You can't read the audit log".
- Any browser the console supports. The page is the same on every server OS.

## Steps

1. Open **Operations → Audit log**. The page opens on the last 24 hours,
   newest entry first.
2. Pick a time range in the joined time control: **Last hour**, **24 h**,
   **7 days**, **30 days** or **All**. For a fixed window, click **Custom**,
   set **From**, **To** or both, and click **Apply**. Leave **To** empty for
   "now"; **To** is exclusive. The control then shows the window, such as
   `Nov 22, 21:09 → now`.
3. Optional: type into the search box and press Enter or click **Run**. The
   text matches the action, the actor id, the resource id and the request
   details, ignoring case. Click **×** to clear it.
4. Optional: click an outcome chip: **Success**, **Denied** or **Failed**.
   Click it again, or **All**, to show every outcome. Each chip shows how
   many entries the chart counts in the window.
5. Optional: choose action types. Open **All types**, type part of a name to
   search, and tick one or more actions. Actions are grouped by their prefix,
   such as `auth`, `job` or `stack`. Click **Select group** to pick a whole
   prefix.
6. Optional: choose an actor. Open **Any actor** and pick a user, or a type:
   **Any user**, **Any node agent**, **Any enrollment token** or **System**.
7. Scroll down to load older entries. They load automatically; **Load more**
   does the same by hand.
8. Click a row, or focus it and press Enter, to expand it. The expanded row
   lists every field, one per line: **Sequence**, **Time**, **Outcome**,
   **Action**, **Actor** and **Actor id**, **Resource** and **Resource id**,
   **Entry id**, **Hash**, **Previous hash**, **Signature** and the
   **Request details**. Copy any id or hash with its copy button. Click
   **Find** next to a detail to search for its value.

To watch new entries arrive, click **Live**. The list and the chart reload
every 5 seconds until you click **Live** again or **Run**.

The filters are part of the page address, so you can bookmark a search or
send the link to a colleague who also has `audit.read`. A preset such as
**24 h** stays relative: the link always means "the last 24 hours".

### Read a row

Each row is one line:

| Part | Shows |
| --- | --- |
| Time | `HH:MM:SS.mmm` today, with the date on other days. Hover for the full time and how long ago it was |
| Outcome chip | `OK`, `DENY` or `FAIL`. Denied and failed rows also get a thicker red or amber left edge |
| Action | The action, such as `job.finish`, coloured by its category |
| Actor | An initial and the user's name, or the node name for an agent. A **key** tag means the call used an API key |
| Resource | A short type, such as `node`, `job` or `cluster`, and its name where Forge can resolve it |
| Details | The request details in one or two lines, on wide windows |

Names come from the users, nodes, clusters, services and projects you can
see. When Forge cannot resolve a name, the row shows the first 8 characters
of the id. The expanded row always shows the raw id.

### Chart

The **Chart** above the list counts the matching entries over the chosen
window, stacked by outcome: green for success, red for denied, amber for
failed. Hover a bar for its time and counts. Click **Chart** to hide or show
it; the browser remembers the choice.

## Verify

- The chart header shows how many entries match, for example `73 entries`.
- The end of the list shows how many entries are loaded out of the total,
  such as `100 of 412`. Once every entry is loaded, it says
  **End of audit log**.
- Click **Clear filters** to reset the search, outcome, types and actor. The
  time range stays.

### Verify the chain

Click **Verify chain** in the page header. Forge recomputes every hash and
signature from the first entry. A strip below the header shows the result:

- **Chain intact**, with the number of entries verified and the head hash.
- **Chain broken at entry #N**, with the reason. Entries before it verify;
  entries from that one on cannot be trusted.

After a check, each expanded row's **Signature** says whether that entry was
verified, fails verification, or is newer than the check.

## Troubleshooting

| Symptom | Cause | Fix |
| --- | --- | --- |
| "You can't read the audit log" | Your role lacks `audit.read`. | Ask an administrator for the **Auditor** role. |
| An action is missing from **All types** | The list shows only actions that occur in the log. | Perform the action once, or type the prefix into the API's `action` parameter. |
| "No entries match" | Nothing in the window matches the filters. The page opens on the last 24 hours. | Click **Clear filters and show all time**, or widen the range. |
| **Apply** is disabled under **Custom** | **From** is not before **To**, or both are empty. | Fix the times. |
| A row shows an 8-character id instead of a name | You cannot list that kind of object, or it was deleted. | Expand the row for the full id. |
| The chart says it needs a newer Forge server | The server has no `/audit/histogram` endpoint. | Upgrade the server. The list and filters work without it. |

## Next steps

- [Audit log](/docs/concepts/audit/): the entry format and how the hash chain
  detects tampering.
- [Roles and access](/docs/guides/roles-and-access/): grant the **Auditor** role.
- [REST API: Audit](/docs/reference/rest-api/#audit): script audit searches.
