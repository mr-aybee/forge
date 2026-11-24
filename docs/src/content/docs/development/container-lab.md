---
title: Container platform lab
description: Start a local control plane and disposable nodes that each run their own Docker Engine, form a Docker Swarm, and drive every container feature from the forge CLI.
sidebar:
  order: 5
---

`scripts/lab/swarm/lab.py` starts a control plane and up to 26 disposable
"VMs" (`lab-a` … `lab-z`). Each one is a container that runs its **own**
Docker Engine, so together they form a real Docker Swarm. The script does
the whole setup through the `forge` CLI and prints each command as it runs,
so the lab doubles as a worked example of the CLI.

:::caution
The nodes are privileged containers (Docker in Docker). Use this lab only on
a development machine, never on a shared or production host.
:::

## Before you begin

- Linux with Docker Engine, and Python 3 (standard library only).
- Build the binaries and the console:

  ```sh
  cargo build --profile perf -p forge-server -p forge-agent -p forge-cli
  (cd src/ui && npm ci && npm run build)
  ```

## Steps

1. Start the lab: one Swarm manager and two workers.

   ```sh
   python3 scripts/lab/swarm/lab.py up
   ```

   For a manager quorum that survives a failure, use
   `up --nodes 5 --managers 3`.

   The script:
   - stages the binaries and builds the node and control plane images;
   - starts the control plane on `http://localhost:8095`;
   - signs in once and creates a 7-day API key;
   - creates the cluster `lab` in Swarm mode;
   - enrolls each node with `forge nodes enroll-token`;
   - joins the nodes with `forge clusters swarm join … --wait`.

   The lab turns MFA off (`[security] require_mfa = false`), which Forge
   allows only for a loopback public URL. That is why the API key can run
   MFA-guarded actions.

2. Point your shell at the lab:

   ```sh
   eval "$(python3 scripts/lab/swarm/lab.py env)"
   ```

3. Try the container platform:

   ```sh
   forge clusters swarm status lab
   forge projects create shop
   forge environments create prod --cluster lab --project shop
   echo '{"name":"web","runtime":"container","image":"nginx:1.27-alpine","port":80,"swarm":{"replicas":2}}' \
     | forge services create --environment prod --file @-
   forge services swarm deploy web --wait
   forge services swarm scale web 3 --wait
   forge docker images lab-a
   forge docker network create lab-a appnet --driver overlay --attachable
   forge webhooks create ci --service web --signature github --tag-pattern '1.*'
   forge stacks from-template redis cache --environment prod --node lab-b
   ```

4. Run PostgreSQL on the swarm from
   `scripts/lab/swarm/examples/postgres.yml`. It runs one replica pinned to
   a labelled node, keeps its data in the named volume `pgdata`, takes the
   password from a sealed secret, and updates stop-first. The comment at the
   top of the file lists the commands.

## Verify

- `forge clusters swarm status lab` lists every node as a member, and
  `quorum.status` reads `single` (or `healthy` with 3 managers).
- `docker exec lab-a docker node ls` shows the same nodes as `Ready`.
- The console at `http://localhost:8095` shows the cluster and its
  services. The sign-in name and password are in
  `scripts/lab/swarm/.lab/credentials.json`. That folder is git-ignored and
  the file is readable only by you.

## Stop or remove

```sh
python3 scripts/lab/swarm/lab.py down           # stop; `up` resumes
python3 scripts/lab/swarm/lab.py down --reset   # delete nodes, volumes and lab data
```

## Troubleshooting

| Symptom | Cause | Fix |
| --- | --- | --- |
| `missing in target/perf` | The binaries are not built. | Run the `cargo build` command above. |
| A node is not online after 4 minutes | Its dockerd or the installer failed. | Run `docker logs lab-b`; dockerd's own log is `/var/log/dockerd.log` in the node. |
| `dockerd did not start` | The host kernel lacks overlay or iptables support for nested Docker. | Check `docker logs <node>`; the node needs `--privileged`, which the script sets. |
| A new agent build is not picked up | Enrolled nodes keep the agent they installed. | Run `down --reset`, then `up`. |
