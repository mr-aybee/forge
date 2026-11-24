# 10 · Deployment lab setup

Prepares the lab for the deployment scenarios (11–17): a stand-in for GitHub
that serves release assets and a git repository over HTTPS, the artifacts,
an nginx edge, three environments, node tags and a second principal that
approves deployments. Save the result as checkpoint `deploy-ready`, then
every deployment scenario starts from it.

**Run it:** `python 10-deploy-setup.py --checkpoint` (Windows host, admin
shell for the checkpoint and the firewall rule). It prints PASS/FAIL per
step and exits non-zero on any failure.

## Prerequisites

- The lab from `../MOP-fresh-lab.md` and `00-common-setup.md` step 1: signed
  in (`forge auth login`), lab-a, lab-b and lab-c **Online** in cluster `lab`.
  The server was installed with `--dev-skip-mfa` (`forge nodes exec` needs no
  prompt).
- lab-c has Docker (`vm_lab.py prepare`); lab-b has none.
- On the machine that runs the scripts: Python 3.10+, `git`, and `openssl`
  (on Windows the one in Git for Windows is found automatically).
- The VMs reach the internet through the host's NAT (Docker Hub images).

## What the deployment scenarios use

```text
 Windows host 10.50.0.1
   ├── mock GitHub  https://10.50.0.1:8443   (labkit/mock_github.py, lab CA in ..\.lab\ca\)
   │     /acme/hello-py/releases/download/<v1|v2|v3|broken>/hello-py-<v>.{tar.gz,zip}
   │     /acme/hello-sh/releases/download/v1/hello-sh-v1.tar.gz
   │     /acme/hello-py.git        (smart HTTP; tags v1, v2, v3)
   ├── forge CLI → http://10.50.0.10:3880
 lab-a  server + nginx edge (endpoints s12…s16.lab.local)
 lab-b  native services (python3)           tags zone:a, runtime:native
 lab-c  containers + native services        tags zone:b, runtime:native, docker:yes
```

| Artifact | What it is | Used in |
| --- | --- | --- |
| `traefik/whoami:v1.10.1`, `v1.11.0` | Echoes the request; `/health` | 11, 12, 15 |
| `hashicorp/http-echo:1.0` | Answers a fixed text (`-text=v1`), or 500 (`-status-code=500`) | 13, 14, 17 |
| `nginx:1.27-alpine` with an exiting command | Never becomes ready | 15 |
| `hello-py` v1, v2 (`.tar.gz`, `.zip` in a top folder) | Python service on `$PORT`, `/ready` → 200 | 15, 16, 17 |
| `hello-py` v3 | Never ready (`/ready` → 503) | (spare) |
| `hello-py` `broken` | Served with one byte changed: checksum never matches | 15 |
| `hello-py.git` | One commit per version | 15, 16 |
| `hello-sh` v1 | A job (`run.sh`) | 16 |

## 1. Artifacts and the mock GitHub

```powershell
cd <repo-root>\scripts\setup_testing\scenarios
python labkit\fixtures.py            # writes ..\.lab\mock-github, prints sha256s and commits
python labkit\mock_github.py         # https://10.50.0.1:8443 (keep it running)
```

On Windows allow the port once (admin): `netsh advfirewall firewall add rule
name="Forge lab mock GitHub 8443" dir=in action=allow protocol=TCP localport=8443`.

## 2. Worker nodes trust the lab CA

A private artifact host is trusted the way any company CA is: in the node's
system store. `git` and `curl` (which fetch the code) use it.

```powershell
Get-Content ..\.lab\ca\lab-ca.crt | forge nodes exec lab-b --login root --shell -- "cat > /usr/local/share/ca-certificates/forge-lab.crt && update-ca-certificates"
forge nodes exec lab-b --login root --shell -- "curl -fsS https://10.50.0.1:8443/healthz"
```

Same for lab-c. Check `python3`, `git`, `curl`, `unzip` are installed.

## 3. Edge, environments, tags

- nginx on lab-a: `forge packages install` with `{package: nginx, version: stable, server_ids: [<lab-a>]}`.
- Environments: `forge environments create dev|staging|prod --cluster lab --project lab`.
- Tags: `forge api request PUT /api/v1/tags/server/<id> --body '{"tags":["zone:a","runtime:native"]}'`
  (lab-b) and `zone:b, runtime:native, docker:yes` (lab-c).

## 4. The approver

Approvals never come from the deployment's author. The second principal is a
service account:

```powershell
forge service-accounts create deploy-approver
forge access profiles create "Deploy approver" --action view --action deploy.approve
forge access assignments add svc-deploy-approver "Deploy approver" project:<lab service id> --yes   # a project, not the cluster: access follows the environment's service (D-056)
forge -o json service-accounts token-create deploy-approver lab --expires-days 30   # secret → ..\.lab\deploy-approver.token
```

## 5. Checkpoint

```powershell
python ..\vm_lab.py checkpoint deploy-ready
# later, before any deployment scenario:
python ..\vm_lab.py reset --checkpoint deploy-ready
```

The script stores ids, URLs, checksums and commits in
`..\.lab\deploy-lab.json` (git-ignored) for scenarios 11–17.

## Verify

- `curl.exe --cacert ..\.lab\ca\lab-ca.crt https://10.50.0.1:8443/healthz` → `ok`
- From lab-b: `git ls-remote https://10.50.0.1:8443/acme/hello-py.git` lists `v1 v2 v3`.
- `forge deploy validate -f <any scenario document>` → valid.

## Run everything

```powershell
python run_all.py --setup            # setup, then 11–17
python run_all.py --reset            # each scenario from deploy-ready
python run_all.py --only 14          # one scenario
```

## Clean up

Stop the mock (Ctrl+C, or end its Python process). Restore `deploy-ready`
or `clean`.
