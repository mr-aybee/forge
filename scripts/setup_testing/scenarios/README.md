# Forge test scenarios

Each file is a self-contained MOP for one way people use Forge: how many
servers, what runs where, and how the cluster is mapped. Run them one at a
time against the Hyper-V lab (lab-a, lab-b, lab-c).

## The lab

Three Debian VMs on Hyper-V on the Windows PC. They're built and reset by
`../vm_lab.py` (see `../MOP-fresh-lab.md`).

```text
 Windows host (Hyper-V, WSL build box, release feed :8000, forge.exe)
   │ vEthernet (ForgeLab) 10.50.0.1/24
   │ NAT "ForgeLabNAT" 10.50.0.0/24 ──► host uplink ──► internet (apt, Docker Hub)
   │
   ├── lab-a  10.50.0.10  Forge server :3880 + agent (worker / edge)
   ├── lab-b  10.50.0.11  agent (worker, Docker host in the scenarios)
   └── lab-c  10.50.0.12  agent (worker, native runtimes in the scenarios)
```

### VM specs

| | lab-a | lab-b | lab-c |
| --- | --- | --- | --- |
| IP | 10.50.0.10/24 | 10.50.0.11/24 | 10.50.0.12/24 |
| Role | Forge server + agent | agent | agent |
| OS | Debian GNU/Linux 13, kernel 6.12 (amd64) | same | same |
| Hyper-V generation | 2 | 2 | 2 |
| vCPU | 2 | 2 | 2 |
| Memory | dynamic: 1 GB at startup, min 512 MB, max 2 GB | same | same |
| Disk | 8 GB (one VHDX) | 8 GB | 8 GB |
| Checkpoint | `clean` (fresh OS, before Forge) | `clean` | `clean` |
| Logins | your admin user, `forge-ops` (Forge terminal account) | same | same |

### Network

| Item | Value |
| --- | --- |
| Hyper-V switch | `ForgeLab` (Internal) |
| Host address | `10.50.0.1` on `vEthernet (ForgeLab)`, network profile **Private** |
| NAT | `ForgeLabNAT` for `10.50.0.0/24`, so the VMs reach the internet through the host |
| Gateway on the VMs | `10.50.0.1` |
| Release feed | `http://10.50.0.1:8000` (`../feed_server.py`), allowed by the firewall rule **Forge local feed** (TCP 8000) |
| Forge console / API | `http://10.50.0.10:3880` (plain HTTP, so the CLI needs `FORGE_ALLOW_HTTP=1`) |
| Ping VM → host | Blocked by the Windows firewall (no ICMP rule). Expected; test with `curl http://10.50.0.1:8000/` instead |
| Site host names | No lab DNS. Use `curl -H "Host: <name>"` or a line in `C:\Windows\System32\drivers\etc\hosts`, such as `10.50.0.10  s03.lab.local` |

### Secrets and where they live

| What | Where |
| --- | --- |
| VM logins, Forge admin password | `scripts/setup_testing/.lab/` (git-ignored; never paste or commit) |
| Release signing key (dev) | `scripts/setup_testing/.keys/` (git-ignored) |

### Limits to keep in mind

- **8 GB disks fill up quickly** with Docker images (Seq, Postgres) and
  several runtimes. Check with `forge nodes metrics <node>` before the
  container scenarios, and prune images with
  `forge nodes exec lab-b -- sudo docker system prune -af`.
- **2 GB max memory per VM.** lab-a runs the server, an agent and Nginx
  together. Watch memory there in scenarios 01 and 07.
- **To reset a VM,** restore the `clean` checkpoint and reinstall
  (`../MOP-fresh-lab.md` "Start over"). Agents and the server must then be
  installed again.
- Three duplicate `ForgeLab` switches left over from earlier rebuilds exist on
  the host. The VMs and the host address use the same one, so they're
  harmless, but they can be removed.

## Scenarios

Every scenario has a Python script next to it (same name, `.py`): stdlib
only, so it runs on Windows or Linux. It does every step, prints PASS/FAIL
per check and exits non-zero on any failure. `run_all.py` (Hyper-V host,
admin shell) runs them all, from Windows and from WSL, each from checkpoint
`clean`: it rebuilds the lab (`labkit/bootstrap.py`), runs the deployment
setup for 11–17, then the scenario, and writes logs and a summary to
`..\.lab\runs\<time>\`. WSL reaches the lab through `labkit/forward.py`.

```powershell
python run_all.py                         # everything, Windows then WSL (several hours)
python run_all.py --os windows --only 03  # one scenario, one OS
python 03-load-balanced-two-nodes.py      # against the lab as it is now
```

| # | Scenario | Nodes | What it proves |
| --- | --- | --- | --- |
| [00](00-common-setup.md) | Common setup | — | CLI login, variables, reset between scenarios. **Run first** |
| [01](01-single-server-nginx-site.md) | One server hosts everything | lab-a | Server + agent + app + Nginx on one machine |
| [02](02-containers-and-native-split.md) | Containers on one node, native apps on the other | lab-b, lab-c | Docker stacks and systemd services side by side |
| [03](03-load-balanced-two-nodes.md) | One service on two nodes behind an edge (native site and container; was 03 + 04) | lab-a edge, lab-b + lab-c | Round-robin, fail-over, balance methods, stack revisions and rollback, node down |
| [05](05-multi-runtime-node.md) | One node, many runtimes (Python, .NET, PHP), one domain each | lab-a edge, lab-c | Several apps and Sites on one worker |
| [06](06-multi-cluster-environments.md) | Two clusters, teams and environments | all | Cluster mapping, environments, scoped roles |
| [07](07-ingress-profile-failover.md) | Cluster ingress profile with two edges | lab-a + lab-b edges | Ingress profile, edge fail-over, routing view |
| [08](08-operator-access.md) | Operator access: tunnels, SFTP, terminal, logs | all | Day-2 access without SSH keys |

### Deployments (`forge deploy`, #73)

Each has a Python script next to it (`NN-deploy-*.py`, stdlib only, runs on
Windows or Linux) that does every step and prints PASS/FAIL per check; it
exits non-zero on any failure. `run_all.py` runs them in order.
Shared helpers are in `labkit/` (the forge CLI wrapper, a request loop
through the edge, the mock GitHub, the test artifacts, a mock CI job).

| # | Scenario | Machines | What it covers |
| --- | --- | --- | --- |
| [10](10-deploy-setup.md) | Deployment lab setup | host + all | Mock GitHub (HTTPS, lab CA), artifacts, edge, environments, tags, approver, checkpoint `deploy-ready`. **Run first** |
| [11](11-deploy-first-release-and-rollback.md) | First release, in-place updates, rollback, the lock | lab-c | Recreate, digest pinning, stop-first refusal, rollback/redeploy, one deployment at a time |
| [12](12-deploy-zero-downtime-rolling.md) | Zero-downtime rolling behind an endpoint | lab-a edge, lab-c | Start-first, slots a/b, no failed request |
| [13](13-deploy-blue-green-approval.md) | Blue-green with approval; pause, resume, abort | lab-a edge, lab-c | Four-eyes approval, all-at-once switch, rollout control |
| [14](14-deploy-canary.md) | Canary and its gates | lab-a edge, lab-c | Weighted steps, error-rate gate, automatic rollback |
| [15](15-deploy-failures-and-resilience.md) | Failures and resilience | all | Never ready, bad checksum, missing commit, server restart, node drop-out |
| [16](16-deploy-native-artifacts.md) | Native services from fetched code | lab-a edge, lab-b, lab-c | .tar.gz, .zip, Git commit, jobs, own accounts, `$PORT` |
| [17](17-deploy-environments-placement-ci.md) | Environments, placement, CI, console | all | Overrides, tag placement, restricted CI token, console checklist |

## How to report what you find

Write a comment **directly under the step** in the scenario file:

```text
> 🐞 COMMENT: <what happened> · exit code <n> · <paste of the error>
> 💡 IDEA: <enhancement>
> ✅ OK
```

Then tell Claude which scenario and step. Each comment becomes a GitHub
issue (labels `bug` or `enhancement`, plus `scenario:<nn>`), and the issue
link is added next to your comment.

## Legend

✅ works from the CLI · 🟡 API (`forge api call`) or UI only · ❌ not implemented yet
· ⚠ known finding (see `../MOP-cli-test-run.md`, "Findings from the test run")
