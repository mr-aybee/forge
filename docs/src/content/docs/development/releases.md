---
title: CI and releases
description: What CI checks on every push, and how a tag becomes a signed, self-contained release.
sidebar:
  order: 6
---

GitHub Actions does two things: it verifies every push and pull request
with light builds, and it builds, signs and publishes releases from tags.
Everything heavier, such as the installer test and the fleet benchmark, is
a local tool you run by hand.

## Continuous integration

`.github/workflows/ci.yml` runs on every push to `develop` and `master` and on
pull requests. It runs light jobs only. Jobs whose files did not change are
skipped, except on **full** runs: pushes to `master`, pull requests into
`master`, `[full-ci]` in the commit message, and manual runs with **full**
ticked (unticked by default).

| Job | Checks | Runs |
| --- | --- | --- |
| Plan | which jobs the changed files need | always |
| Rust (Linux) | `cargo fmt --check`, `cargo clippy -D warnings`, build, `cargo test --workspace` | always |
| Rust (Windows) | clippy, build, tests | full runs or agent changes |
| cargo deny + cargo audit | licences, advisories, sources | dependency changes |
| Store on PostgreSQL | builds with the `postgres` feature | store or lockfile changes |
| UI, website and docs | `npm ci && npm run build` for each, then `npm audit --omit=dev --audit-level=high` | web changes |

A new commit cancels the older run of the same branch or pull request.
There is no scheduled run.

### Runners

Every job runs on a GitHub-hosted runner: `ubuntu-latest` for Linux jobs,
`windows-latest` for the Windows tests and build, and `macos-14` for the
macOS CLI. The repository is public, so these minutes are free.

- **Build wrapper.** `scripts/ci/build.sh light|heavy <command>` wraps every
  compile and runs it under `nice` and `ionice`. On GitHub-hosted runners it
  takes no lock.
- **Release builds** use `-j 4` (`FORGE_CI_BUILD_JOBS`). Each `cross`
  container is capped with
  `CROSS_CONTAINER_OPTS="--memory=12g --memory-swap=12g"`, so a link that
  runs away fails the job with a clear error.
- **Latest wins.** `ci.yml` and manual website builds cancel the older run
  of the same ref. A newer release dry run of the same tag replaces the
  older one. A tagged release is never cancelled.
- **Build cache.** Rust jobs use `Swatinem/rust-cache`.

### Repository settings

Maintainers keep these settings on `mr-aybee/forge`:

| Setting | Value |
| --- | --- |
| Branch protection on `develop` and `master` | Require a pull request, an approving review and green CI; no direct pushes |
| **Actions** → fork pull request workflows | Require approval for first-time contributors |
| **Security** → private vulnerability reporting | Enabled (see `SECURITY.md`) |
| Actions secret `FORGE_RELEASE_KEY` | The release signing key (see [The release signing key](#the-release-signing-key)); the only secret the workflows use |

## Releasing

Each step needs a maintainer's go-ahead.

1. Set the version in the workspace `Cargo.toml` (`[workspace.package]
   version`). For release candidates the tag adds `-rc-NN` to it.
2. Optional: write the highlights to
   `scripts/packaging/release-notes/<tag>.md`; they head the release notes.
3. Merge everything into `develop` through pull requests and wait for CI to
   pass.
4. Bring `master` up to `develop` with a pull request from `develop` into
   `master`. CI runs a full run on it.

5. Run the release [dry run](#dry-run) on `master`, download its
   `release-assets` and run the installer test against those packages on
   Debian and a RHEL-like, on your own machine:

   ```sh
   gh run download <run-id> -n release-assets -D /tmp/rc
   FORGE_TEST_ASSETS=/tmp/rc/assets bash scripts/packaging/test-install.sh debian:bookworm
   FORGE_TEST_ASSETS=/tmp/rc/assets bash scripts/packaging/test-install.sh rockylinux:9
   ```

6. Tag the `master` commit and push the tag:

   ```sh
   git tag -a v0.2.0-rc-01 origin/master -m "Orbyn Forge 0.2.0-rc-01"
   git push origin v0.2.0-rc-01
   ```

| Tag | Release |
| --- | --- |
| `vX.Y.Z` | Stable. The commit must be on `master`. Marked "latest" |
| `vX.Y.Z-rc-NN` | GitHub pre-release. Never "latest", so `/releases/latest`, the self-updater's feed and `get-forge.sh` without `--version` keep the last stable release. Installs that run a pre-release are offered newer ones |

A tag that does not match `Cargo.toml` fails in the first job. To fix a
tag, delete it (`git push origin :refs/tags/<tag>`), fix, and tag again.

### What the release workflow does

`.github/workflows/release.yml` runs on the tag push:

1. **Check tag, version and signing key** (seconds): the tag format, that a
   stable tag is on `master` (a warning for a release candidate), that the
   tag matches `Cargo.toml`, and that the `FORGE_RELEASE_KEY` secret holds
   the private half of `RELEASE_PUBLIC_KEY` (ring's PKCS#8 document ends
   with the public key) and that the scripts and docs pin that key.
2. **Web console**: builds `src/ui` once and shares it as an artifact.
3. **Build** each target: `forge-server`, `forge-analytics-worker`,
   `forge-plugin-runner` and the agents, then the `forge` CLI in a second
   `cargo build -p forge-cli` (its features stay out of the server and agent
   binaries). Linux targets (`x86_64-unknown-linux-musl`,
   `aarch64-unknown-linux-musl`) build with `cross`; Windows
   (`x86_64-pc-windows-msvc`) with `cargo` and `link.exe`. The release version
   is compiled in (`FORGE_BUILD_VERSION`). The job checks architecture and
   static linking, and that the x86_64 server and worker report the version.
4. **forge CLI (macOS)**: `aarch64-apple-darwin` and `x86_64-apple-darwin` in
   one job on `macos-14`; checks each architecture and that only
   system libraries are linked. Not code-signed or notarised.
5. **Package and sign**: `scripts/packaging/package.sh` assembles the
   tarballs, the Windows zip, the raw binaries the updater downloads and the
   CLI binaries, each package with `dist/MANIFEST.json` naming the version;
   it adds `get-forge.sh` and `get-forge.ps1`. `check-release.sh` checks
   that every expected asset exists, each package's layout, the manifest
   version, and that packaged binaries equal the raw ones. Then it writes
   `SHA256SUMS`, signs it with the built server, checks the key again, and
   verifies the signature independently with OpenSSL the way users do
   (`get-forge.sh --verify-only`). `release-notes.sh` writes the notes. All
   of it is kept as the `release-assets` artifact for 14 days.
6. **Publish**: creates the release in this repository as a draft, uploads
   the assets, then publishes it (pre-release for `-rc-NN`). Only this job
   has write access, through the workflow's `GITHUB_TOKEN`.
7. **Website image**: builds `website/Dockerfile` and, for a published
   release, pushes it to `ghcr.io/orbyntech/forge-website` (see
   [Website and docs site](/docs/development/website/)).

The asset names are a public contract: `install.sh`, `install.ps1`,
`get-forge.*`, the server's self-updater (`update.rs`) and `forge update`
depend on them. Change them only together with those and the docs, and
record the change in `DECISIONS.md`.

### Dry run

A manual run with **publish** unticked runs every job except **Publish**
and builds the branch you run it on, named as the tag you give, which need
not exist yet. Use it before tagging:

```sh
gh workflow run release.yml --ref master -f tag=v0.2.0-rc-01 -f publish=false
gh run watch "$(gh run list --workflow release.yml --limit 1 --json databaseId --jq '.[0].databaseId')"
```

Download the `release-assets` artifact to check the packages and notes.
With **publish** ticked, a manual run builds the existing tag and publishes
it again; existing assets are replaced (`--clobber`).

### Where releases are published

Source and releases live in the same public repository, `mr-aybee/forge`,
under the Apache License 2.0 (the root `LICENSE`, shipped in every package
as `LICENSE.txt`). Releases appear under **Releases** on GitHub.

Running installs see a new stable release within six hours (or at once with
**Check again**) and offer the update.

### The release signing key

The key is Ed25519. The public half is compiled into Forge
(`RELEASE_PUBLIC_KEY` in `src/protocol/forge-sdk/src/release.rs`) and pinned
in `get-forge.sh`, `get-forge.ps1` and the installation guide;
the private half is the repository secret **`FORGE_RELEASE_KEY`** (base64
PKCS#8):

```sh
forge-server release keygen            # prints a new private and public key
FORGE_RELEASE_KEY=… forge-server release pubkey
FORGE_RELEASE_KEY=… forge-server release sign SHA256SUMS > SHA256SUMS.sig
```

Rotating the key means shipping a release (signed with the old key) whose
binary carries the new public key; releases after that are signed with the
new key.

### Testing packages locally

`scripts/packaging/test-install.sh` is a local tool; CI does not run it. It
needs Docker and runs privileged systemd containers.

```sh
bash scripts/packaging/test-install.sh --syntax-only     # no Docker
FORGE_TEST_ASSETS=/tmp/rc/assets bash scripts/packaging/test-install.sh rockylinux:9   # the dry run's packages
FORGE_TEST_BIN=target/cross/x86_64-unknown-linux-musl/release bash scripts/packaging/test-install.sh   # your own build, Debian 12
```

With `FORGE_TEST_ASSETS` the test uses the dry run's packages and their
real signature, checked against the pinned release key; the upgrade step
then re-installs the same version. Without it, the test packages two versions from x86_64 musl binaries, signs them with a
throwaway key and serves them as a `file://` mirror of GitHub Releases
(`FORGE_RELEASES_URL`, `FORGE_RELEASE_PUBLIC_KEY`). In a clean systemd
container it checks that a tampered package is refused before anything is
installed, installs with `get-forge.sh --version`, re-installs from a local
package with `--file`, upgrades to "latest", and uninstalls with and
without `--purge`. After the install it checks the console, the API, the
node installer and agent downloads, the analytics worker on `55132` with
DuckDB, the agent folder's `MANIFEST.json` and file permissions. Set
`DB=1` to install with `--db-local` as well, the path that differs most
between Debian and RHEL-likes. `DOCKER="sudo docker"` runs Docker through
sudo.

## Testing releases locally

`scripts/setup_testing/` stands in for GitHub Releases on your own machine,
so lab VMs can install from a "release" and self-update without anything
being copied to them:

```sh
python3 scripts/setup_testing/publish.py         # build, package, sign, publish
python3 scripts/setup_testing/feed_server.py     # serve on 192.0.2.1:8000
```

`publish.py` versions each build `<next patch version>-dev.<yyyyMMdd.HHmmss>`
(for Cargo version `0.1.0`: `0.1.1-dev.…`, compiled in with
`FORGE_BUILD_VERSION`) and signs it with a local dev key. Forge orders a dev
build or release candidate of `X.Y.Z` below `X.Y.Z`, so the dev build of the
next version sorts above the last release and below the next one.
The local release holds the Linux x86_64 assets, with the CLI as
`forge-cli-x86_64-unknown-linux-musl`; the script prints its size. Add `--arm`
to also build the Linux arm64 (`aarch64`) assets. It cross-compiles with
`cross`, which needs Docker (`cargo install cross --locked`). The CLI
reports its Cargo version, not the dev version.
On the lab server, point the updater at the feed and trust that key:

```toml
[update]
feed_url = "http://192.0.2.1:8000/repos/mr-aybee/forge/releases/latest"
public_key = "<scripts/setup_testing/.keys/release.pub>"
```

The console then offers each new local build under **Update to …**. See
`scripts/setup_testing/README.md` for the first install and firewall setup.
The dev key and published files are git-ignored; never use the dev key for
real releases.
