# 06 · Two clusters, teams and environments

**Who:** an organisation with a web team and a data team. Each team gets its
own cluster of machines and its own environments, and must not touch the
other's.

```text
cluster "web"  (WEB) ── lab-b ── environment web-prod, web-dev
cluster "data" (DAT) ── lab-c ── environment data-prod
cluster "lab"        ── lab-a (server, edge, shared)
```

| Cluster mapping | |
| --- | --- |
| `lab` | lab-a |
| `web` | lab-b |
| `data` | lab-c |
| Users | `alice` (operator on `web`), `bob` (developer on `data-prod`) |

Prerequisite: [00-common-setup](00-common-setup.md). This scenario is
**exploratory**: some steps may have no CLI command. Mark them with a
comment.

## 1. Clusters ✅

```powershell
J @{ name='web';  description='web team';  abbr='WEB' } c1.json | Out-Null
J @{ name='data'; description='data team'; abbr='DAT' } c2.json | Out-Null
forge clusters create --file c1.json
forge clusters create --file c2.json
forge clusters list
```

## 2. Move nodes into the team clusters ❓

A node's cluster is chosen at enrollment. To move an enrolled node:

```powershell
forge nodes --help                 # is there a "move"/"set-cluster" command?
forge api operations --tag servers # look for an update with cluster_id
```

- If neither exists, use the UI (**Node → Settings → Cluster**) or
  re-enroll: revoke the node with `forge nodes revoke lab-b`, then
  `forge nodes enroll-token --cluster web --hostname lab-b` and reinstall on
  the node.
- Log which path worked. A missing CLI command is a finding.

## 3. Environments ✅

```powershell
forge environments create web-prod  --cluster web --project lab
forge environments create web-dev   --cluster web --project lab
forge environments create data-prod --cluster data --project lab
forge environments list
```

## 4. Users and scoped roles ✅ / ❓

```powershell
forge users --help; forge access --help        # check the exact flags
forge users create alice --email alice@lab.local
forge users create bob   --email bob@lab.local
forge access grant --user alice --role operator  --cluster web
forge access grant --user bob   --role developer --environment data-prod
forge access list
```

## 5. Verify isolation

Sign in as each user with a second context:

```powershell
forge contexts add lab-alice --server http://10.50.0.10:8080; forge auth login   # as alice
forge nodes list              # expect lab-b only (and maybe shared nodes)
forge nodes exec lab-c -- id  # expect: permission denied (exit 4)
forge contexts use lab-bob; forge auth login                                          # as bob
forge services list               # data-prod apps only
forge packages install --file pkg.json   # expect denied: a developer can't install packages
forge contexts use lab
```

- [ ] Each user sees only their cluster or environment
- [ ] Denied actions exit `4` and appear in `forge audit` as denials
- [ ] A Site on lab-a (cluster `lab`) can target an app in cluster `web`. Is that allowed? Note the result
- [ ] The **Cluster** filter on **Nodes** and **Services** shows each cluster's nodes and services only

## 6. Clean up

Move the nodes back to `lab` (same method as step 2), remove the users and
grants, delete the environments and the clusters `web` and `data`. Then run
`forge contexts remove lab-alice` and `forge contexts remove lab-bob`.
