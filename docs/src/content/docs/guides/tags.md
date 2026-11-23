---
title: Tag nodes, clusters, environments and services
description: Add free-text or key:value tags to nodes, clusters, environments and services, and filter the node list by tag.
sidebar:
  order: 20
---

Use tags to label parts of your fleet, for example `active`, `standby`,
`dc:fra1` or `location:karachi`, and to filter the node list by those labels.
Tags are for your own reference. They do not grant access or change how Forge
deploys anything.

## Before you begin

Reading tags needs `view` on the item. Changing them needs the right that
manages the item:

| Item | Cedar action to change tags | Roles that have it |
| --- | --- | --- |
| Node | `server.manage` on the node | **Platform admin**, **Admin**, **Operator** |
| Cluster | `workspace.manage` on the cluster | **Platform admin**, **Admin** |
| Environment | `workspace.manage` on the environment | **Platform admin**, **Admin** |
| Service | `deploy` on the service's environment | **Platform admin**, **Admin**, **Operator**, **Developer** |

Tags follow these rules:

- A tag is a plain word (`standby`) or `key:value` (`dc:fra1`). Both sides of
  `key:value` must be non-empty.
- At most 64 characters per tag and 50 tags per item.
- No commas and no control characters.
- Spaces around a tag and around the `:` are trimmed, so ` dc : fra1 ` becomes
  `dc:fra1`.
- Duplicates are dropped case-insensitively (`ACTIVE` and `active` are one tag).

## Steps

1. To tag a node, open **Nodes** and select the node. On the **Overview** tab, the **System** panel has a **Tags** row. Type a tag in
   **Add tag** and press <kbd>Enter</kbd> or a comma. Select **×** on a tag to
   remove it.
2. To tag a cluster, open **Home → Clusters** and select the cluster. Use
   the tag editor on the cluster page in the same way.
3. To tag an environment, open **Projects** and select the environment in
   the tree. Use the tag editor in its pane.
4. To tag a service, open it from **Services**. Use the tag editor on its
   detail page.
5. To filter nodes by tag, open **Nodes** and pick tags from the **Tags**
   filter. Pick more tags to narrow the list: a node must carry every selected
   tag. The search box also matches tag text. The filter is kept in the URL
   (`/nodes?tag=dc:fra1,env:prod`), so you can share the link.

You can set tags through the API too. `PUT` replaces the whole set:

```sh
curl -X PUT "$FORGE_URL/api/v1/tags/server/<node-id>" \
  -H "Authorization: Bearer $FORGE_TOKEN" \
  -H "Content-Type: application/json" \
  -d '{"tags": ["dc:fra1", "active"]}'
```

List nodes that carry both tags (repeat `tag`; matching ignores case):

```sh
curl "$FORGE_URL/api/v1/servers?tag=dc:fra1&tag=active" \
  -H "Authorization: Bearer $FORGE_TOKEN"
```

## Verify

- The tag appears on the item and in the **Filter by tag** list, with a count.
- The response of `GET /api/v1/tags/<kind>/<id>` lists it.
- **Operations → Audit log** shows a `tags.update` event with the old and new set.

## Troubleshooting

| Symptom | Cause | Fix |
| --- | --- | --- |
| No **Add tag** field; the node shows `Changing tags needs server.manage` | You have `view` but not the manage right | Ask for a role from the table above |
| `400` "use `key:value` with both parts, or a plain word" | A tag like `dc:` or `:fra1` | Fill both sides or drop the colon |
| `400` "contains a control character or a comma" | A comma in the tag | Split it into two tags |
| `400` "at most 50 tags per item" | Too many tags | Remove tags you no longer need |
| `?tag=a,b` returns no nodes | The API does not split commas | Repeat the parameter: `?tag=a&tag=b` |

## Next steps

- [Node overview, storage and connectivity](/docs/guides/node-monitoring/)
- [Roles and access](/docs/guides/roles-and-access/)
- [REST API: Tags](/docs/reference/rest-api/#tags)
