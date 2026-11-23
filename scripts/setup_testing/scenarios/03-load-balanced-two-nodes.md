# 03 · One service on two nodes behind an edge

**Who:** a website or API that must stay up when one server goes down, run as
a native service or as a container. (Merges the former 03 "static website on
two nodes" and 04 "the same container on two nodes".)

```text
browser ──► lab-a :80 Nginx
              ├─ Site "s03-web"    (round_robin) ──► lab-b :8080, lab-c :8080   native python site
              └─ Site "s03-whoami" (least_conn)  ──► lab-b :8090, lab-c :8090   container traefik/whoami
```

| Cluster mapping | |
| --- | --- |
| Cluster | `lab` |
| lab-a | edge (Nginx) |
| lab-b, lab-c | workers, the same service on both |
| Environment | `s03` |

**Run it:** `python 03-load-balanced-two-nodes.py [--only 03A|03B] [--node-down] [--keep]`

Prerequisite: the lab bootstrapped (`labkit/bootstrap.py`, or `run_all.py`).

## Part A · Native site on two nodes ✅

1. Python on lab-b and lab-c; service `web` (`python3 -m http.server 8080
   --bind 0.0.0.0 --directory …/site`), placed on both and deployed.
2. Each node gets a small `site.zip` naming the node (`forge services files web
   --node <n> upload … ; extract … /site --replace`).
3. Site `s03-web` for `s03.lab.local`, target `{kind: app, port: 8080}`, edge lab-a.

- [ ] 12 × `200`, answers from both nodes
- [ ] **Fail-over:** `forge services action web stop --node lab-b`: still `200`, only lab-c answers; start it again
- [ ] **Balance methods:** `least_conn`, then `ip_hash` (the same client always reaches the same node)
- [ ] The Site is in `forge clusters routing lab` (and **Cluster → lab → Ingress**)

A zero-downtime release of a new version is `forge deploy` (scenario 12),
not a manual folder swap.

## Part B · The same container on two nodes ✅

1. Docker on lab-b and lab-c; one Compose stack per node (`whoami-b`,
   `whoami-c`) with `traefik/whoami:v1.10.1` on port 8090.
   Mark the port public in the compose file (`x-forge: { public: [8090] }`):
   a stack's ports bind to the node's internal address otherwise (loopback
   when the cluster has no fabric), and the edge on lab-a could not reach them.
2. Site `s03-whoami` for `s03w.lab.local`, `least_conn`, node targets lab-b:8090 and lab-c:8090.

- [ ] Both containers' hostnames appear
- [ ] `forge stacks service whoami-b stop --node lab-b --services whoami`: one hostname, no errors; start it again
- [ ] Update to `v1.11.0` (`forge stacks update whoami-b -f whoami.yml`, `deploy`): `forge stacks revisions whoami-b` shows 2; `rollback --to 1` works
- [ ] `forge stacks logs whoami-b whoami --node lab-b --tail 20` shows the requests
- [ ] The UI lists both containers under each node's **Containers** (manual)

## Part C · Node down (`--node-down`, Hyper-V host)

- [ ] `Stop-VM LAB-C -TurnOff`: requests are still `200`; Forge shows lab-c offline within 3 minutes; `Start-VM LAB-C`

## Clean up

The script removes both Sites, both stacks and the service unless `--keep`.
